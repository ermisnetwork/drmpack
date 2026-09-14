use crate::session::isobmff::{
    is_complete_isobmff_init_segment, is_complete_isobmff_media_segment,
};
use crate::types::{ArtifactKind, EncryptionScheme, PackagedArtifact};
use bytes::Bytes;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

const MAX_EMITTED_HISTORY: usize = 5000;
const WATCHDOG_INTERVAL_MS: u64 = 50;
/// Minimum byte threshold for a candidate ISOBMFF segment (at least two 8-byte box headers).
const MIN_SEGMENT_FILE_SIZE_BYTES: u64 = 16;

struct ManifestMeta {
    mtime: Option<SystemTime>,
    size: u64,
    data: Bytes,
    hls_refs: HashSet<String>,
}

#[derive(Default)]
struct HarvesterState {
    emitted_segments: HashMap<EncryptionScheme, HashSet<String>>,
    emitted_order: std::collections::VecDeque<(EncryptionScheme, String)>,
    manifest_cache: HashMap<(EncryptionScheme, String), ManifestMeta>,
    max_segments: HashMap<EncryptionScheme, u64>,
    watched_dirs: HashSet<PathBuf>,
}

impl HarvesterState {
    fn is_emitted(&self, scheme: EncryptionScheme, file_name: &str) -> bool {
        self.emitted_segments
            .get(&scheme)
            .is_some_and(|set| set.contains(file_name))
    }

    fn record_emitted(&mut self, scheme: EncryptionScheme, file_name: String) {
        let set = self.emitted_segments.entry(scheme).or_default();
        if set.insert(file_name.clone()) {
            self.emitted_order.push_back((scheme, file_name.clone()));
            if self.emitted_order.len() > MAX_EMITTED_HISTORY {
                if let Some((old_scheme, old_name)) = self.emitted_order.pop_front() {
                    if let Some(old_set) = self.emitted_segments.get_mut(&old_scheme) {
                        old_set.remove(&old_name);
                    }
                }
            }
        }
        if let Some(num) = parse_segment_number(&file_name) {
            let entry = self.max_segments.entry(scheme).or_insert(0);
            if num > *entry {
                *entry = num;
            }
        }
    }
}

/// Parse the numeric sequence number from a segment filename (e.g. `"video_1080p_1.m4s"` -> `Some(1)`).
pub fn parse_segment_number(filename: &str) -> Option<u64> {
    if let Some(stem) = filename.strip_suffix(".m4s") {
        if let Some(pos) = stem.rfind('_') {
            return stem[pos + 1..].parse::<u64>().ok();
        }
    }
    None
}

/// Parse ISO 8601 duration string into seconds (f64).
/// Supports formats like "PT0H0M10.021S", "PT10.021S", "PT1M30S".
pub fn parse_iso8601_duration(s: &str) -> Option<f64> {
    let rest = s.trim().strip_prefix("PT")?;
    let mut total_secs = 0.0;
    let mut start = 0;

    for (i, c) in rest.char_indices() {
        let mult = match c {
            'H' | 'h' => 3600.0,
            'M' | 'm' => 60.0,
            'S' | 's' => 1.0,
            _ => continue,
        };
        let val = rest[start..i].parse::<f64>().ok()?;
        total_secs += val * mult;
        start = i + c.len_utf8();
    }
    (start == rest.len() && start > 0).then_some(total_secs)
}

/// Format seconds into GPAC standard ISO 8601 duration "PT{}H{}M{:.3}S".
pub fn format_iso8601_duration(secs: f64) -> String {
    let hours = (secs / 3600.0).floor() as u64;
    let rem = secs - (hours as f64 * 3600.0);
    let mins = (rem / 60.0).floor() as u64;
    let s = rem - (mins as f64 * 60.0);
    format!("PT{}H{}M{:.3}S", hours, mins, s)
}

/// Extract XML attribute value from a tag string.
pub fn extract_xml_attribute<'a>(tag: &'a str, attr: &str) -> Option<&'a str> {
    let pattern = format!("{attr}=");
    let start_idx = tag.find(&pattern)?;
    let after_key = &tag[start_idx + pattern.len()..];
    let quote = after_key.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &after_key[1..];
    let end_idx = rest.find(quote)?;
    Some(&rest[..end_idx])
}

/// Extract segment duration in seconds from <SegmentTemplate ... timescale="..." duration="..." />.
pub fn parse_segment_template_duration(xml: &str) -> Option<f64> {
    let template_idx = xml.find("<SegmentTemplate")?;
    let template_slice = &xml[template_idx..];
    let end_idx = template_slice.find('>')?;
    let tag = &template_slice[..end_idx];

    let timescale = extract_xml_attribute(tag, "timescale")?
        .parse::<u64>()
        .ok()?;
    let duration = extract_xml_attribute(tag, "duration")?
        .parse::<u64>()
        .ok()?;
    if timescale > 0 && duration > 0 {
        Some(duration as f64 / timescale as f64)
    } else {
        None
    }
}

/// Sanitize static MPD presentation duration and period duration to align with
/// the maximum available segment number, preventing players (like Shaka Player)
/// from ceiling-rounding and requesting a non-existent segment N+1.
pub fn sanitize_static_mpd(xml: &str, max_segment_number: Option<u64>) -> String {
    if !xml.contains("type=\"static\"") {
        return xml.to_string();
    }

    let max_seg = match max_segment_number {
        Some(n) if n > 0 => n,
        _ => return xml.to_string(),
    };

    let seg_dur = match parse_segment_template_duration(xml) {
        Some(d) if d > 0.0 => d,
        _ => return xml.to_string(),
    };

    let max_valid_dur = max_seg as f64 * seg_dur;

    let mpd_dur_str = match extract_xml_attribute(xml, "mediaPresentationDuration") {
        Some(s) => s,
        None => return xml.to_string(),
    };

    let cur_dur = match parse_iso8601_duration(mpd_dur_str) {
        Some(d) => d,
        None => return xml.to_string(),
    };

    if cur_dur <= max_valid_dur {
        return xml.to_string();
    }

    let clamped_dur_str = format_iso8601_duration(max_valid_dur);

    let target_mpd = format!("mediaPresentationDuration=\"{mpd_dur_str}\"");
    let replacement_mpd = format!("mediaPresentationDuration=\"{clamped_dur_str}\"");
    let mut result = xml.replace(&target_mpd, &replacement_mpd);

    if let Some(period_tag) = result.find("<Period").map(|i| &result[i..]) {
        if let Some(end) = period_tag.find('>') {
            if let Some(p_dur) = extract_xml_attribute(&period_tag[..end], "duration") {
                let target_period = format!("duration=\"{p_dur}\"");
                let replacement_period = format!("duration=\"{clamped_dur_str}\"");
                result = result.replace(&target_period, &replacement_period);
            }
        }
    }

    result
}

fn is_valid_hls_manifest(text: &str) -> bool {
    if !text.starts_with("#EXTM3U") {
        return false;
    }
    if text.contains("#EXT-X-STREAM-INF:") {
        true
    } else if text.contains("#EXT-X-TARGETDURATION:") {
        text.contains("#EXTINF:") || text.contains("#EXT-X-ENDLIST")
    } else {
        false
    }
}

/// Extract all segment and init-segment URIs referenced by an HLS manifest.
fn extract_hls_segment_refs(text: &str) -> HashSet<String> {
    let mut refs = HashSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !trimmed.starts_with('#') {
            refs.insert(trimmed.to_string());
        } else if let Some(rest) = trimmed.strip_prefix("#EXT-X-MAP:") {
            if let Some(uri) = extract_xml_attribute(rest, "URI") {
                refs.insert(uri.to_string());
            }
        }
    }
    refs
}

async fn harvest_target(
    dir: &Path,
    scheme: EncryptionScheme,
    tx: &mpsc::Sender<PackagedArtifact>,
    state: &mut HarvesterState,
    is_final: bool,
    shutdown: &CancellationToken,
) -> Result<(), ()> {
    if !dir.exists() {
        return Ok(());
    }

    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(d) => d,
        Err(e) => {
            warn!(?dir, error = %e, "ArtifactHarvester: failed to read staging directory");
            return Ok(());
        }
    };

    let mut manifests = Vec::new();
    let mut segments = Vec::new();

    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let path = entry.path();
        let file_type = match entry.file_type().await {
            Ok(ft) => ft,
            Err(e) => {
                debug!(?path, error = %e, "ArtifactHarvester: failed to get entry file_type");
                continue;
            }
        };
        if !file_type.is_file() {
            continue;
        }
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        if file_name.starts_with('.') || file_name.ends_with(".tmp") {
            continue;
        }

        if file_name.ends_with(".m3u8") || file_name.ends_with(".mpd") {
            manifests.push((path, file_name));
        } else if file_name.ends_with(".m4s") || file_name.ends_with("init.mp4") {
            segments.push((path, file_name));
        }
    }

    let mut valid_manifests = Vec::new();
    let mut hls_segments: HashSet<String> = HashSet::new();

    for (path, file_name) in manifests {
        // Metadata guard: skip reading file if mtime and size unchanged
        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(e) => {
                debug!(?path, error = %e, "ArtifactHarvester: failed to read manifest metadata");
                continue;
            }
        };
        let mtime = meta.modified().ok();
        let file_size = meta.len();
        let cache_key = (scheme, file_name.clone());
        if !is_final {
            if let Some(cached) = state.manifest_cache.get(&cache_key) {
                if cached.mtime == mtime && cached.size == file_size && mtime.is_some() {
                    // Manifest unchanged — reuse cached hls_refs directly
                    hls_segments.extend(cached.hls_refs.iter().cloned());
                    continue;
                }
            }
        }

        if let Ok(bytes) = tokio::fs::read(&path).await {
            let (is_valid, text_opt) = if file_name.ends_with(".mpd") {
                (
                    bytes.windows(6).any(|w| w == b"</MPD>"),
                    std::str::from_utf8(&bytes).ok(),
                )
            } else if file_name.ends_with(".m3u8") {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    (is_valid_hls_manifest(text), Some(text))
                } else {
                    (false, None)
                }
            } else {
                (false, None)
            };

            if is_valid {
                let mut refs = HashSet::new();
                if let Some(text) = text_opt {
                    if file_name.ends_with(".m3u8") {
                        refs = extract_hls_segment_refs(text);
                        hls_segments.extend(refs.iter().cloned());
                    }
                }
                valid_manifests.push((path, file_name, bytes, mtime, file_size, refs));
            }
        }
    }

    let mut ready_segments = Vec::new();
    for (path, file_name) in segments {
        if state.is_emitted(scheme, &file_name) {
            continue;
        }

        let is_init = file_name.ends_with("init.mp4");

        // HLS manifest is the canonical readiness signal for media segments (ADR-0015).
        // DASH MPD uses SegmentTemplate patterns present from session start
        // and cannot signal individual segment completion.
        // Init segments are small (~1KB) and written atomically by GPAC,
        // so they bypass manifest readiness and rely solely on ISOBMFF completeness.
        let in_manifest = is_init || hls_segments.contains(&file_name) || is_final;

        let (is_ready, cached_data) = if in_manifest {
            let meta = match tokio::fs::metadata(&path).await {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.len() < MIN_SEGMENT_FILE_SIZE_BYTES {
                continue;
            }

            if let Ok(data) = tokio::fs::read(&path).await {
                let complete = if is_init {
                    is_complete_isobmff_init_segment(&data)
                } else {
                    is_complete_isobmff_media_segment(&data)
                };
                if complete {
                    (true, Some(data))
                } else {
                    debug!(?path, is_init, "ArtifactHarvester: segment is not yet a complete ISOBMFF box; waiting for next flush");
                    (false, None)
                }
            } else {
                debug!(
                    ?path,
                    "ArtifactHarvester: failed to read segment file from staging"
                );
                (false, None)
            }
        } else {
            (false, None)
        };

        if is_ready {
            if let Some(data) = cached_data {
                let kind = if is_init {
                    ArtifactKind::InitSegment
                } else {
                    ArtifactKind::MediaSegment
                };
                ready_segments.push((path, file_name, Bytes::from(data), kind));
            }
        }
    }

    // Sort ready segments: InitSegment first, then by segment number, then by filename
    ready_segments.sort_by(|a, b| match (a.3, b.3) {
        (ArtifactKind::InitSegment, ArtifactKind::MediaSegment) => std::cmp::Ordering::Less,
        (ArtifactKind::MediaSegment, ArtifactKind::InitSegment) => std::cmp::Ordering::Greater,
        _ => parse_segment_number(&a.1)
            .cmp(&parse_segment_number(&b.1))
            .then_with(|| a.1.cmp(&b.1)),
    });

    for (path, file_name, data, kind) in ready_segments {
        let artifact = PackagedArtifact {
            filename: file_name.clone(),
            data,
            kind,
            scheme,
        };
        tokio::select! {
            result = tx.send(artifact) => {
                if result.is_err() {
                    debug!("ArtifactHarvester: output channel receiver dropped");
                    return Err(());
                }
                debug!(filename = %file_name, ?kind, ?scheme, "Emitted packaged artifact");
                state.record_emitted(scheme, file_name);
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    warn!(?path, error = %e, "ArtifactHarvester: failed to remove ephemeral segment");
                }
            }
            _ = shutdown.cancelled() => { return Err(()); }
        }
    }

    valid_manifests.sort_by(|a, b| a.1.cmp(&b.1));
    for (path, file_name, bytes, mtime, file_size, hls_refs) in &valid_manifests {
        let mut data_bytes = bytes.clone();
        if file_name.ends_with(".mpd") {
            if let Ok(text) = std::str::from_utf8(&data_bytes) {
                if text.contains("type=\"static\"") {
                    let max_seg = state.max_segments.get(&scheme).copied();
                    let sanitized = sanitize_static_mpd(text, max_seg);
                    data_bytes = sanitized.into_bytes();
                }
            }
        }

        let key = (scheme, file_name.clone());
        let data = Bytes::from(data_bytes);
        let changed = state
            .manifest_cache
            .get(&key)
            .map(|cached| cached.data != data)
            .unwrap_or(true);
        // Always update metadata so mtime/size stay fresh (fixes stale cache loop)
        state.manifest_cache.insert(
            key,
            ManifestMeta {
                mtime: *mtime,
                size: *file_size,
                data: data.clone(),
                hls_refs: hls_refs.clone(),
            },
        );
        if changed {
            let artifact = PackagedArtifact {
                filename: file_name.clone(),
                data,
                kind: ArtifactKind::Manifest,
                scheme,
            };
            tokio::select! {
                result = tx.send(artifact) => {
                    if result.is_err() {
                        debug!("ArtifactHarvester: output channel receiver dropped on manifest send");
                        return Err(());
                    }
                    debug!(filename = %file_name, kind = ?ArtifactKind::Manifest, ?scheme, "Emitted manifest artifact");
                }
                _ = shutdown.cancelled() => { return Err(()); }
            }
        }
        if is_final {
            if let Err(e) = tokio::fs::remove_file(path).await {
                debug!(?path, error = %e, "ArtifactHarvester: failed to remove final manifest");
            }
        }
    }

    Ok(())
}

/// Background harvester task monitoring staging directories for packaged artifacts.
pub struct Harvester {
    shutdown_token: CancellationToken,
    join_handle: Option<JoinHandle<()>>,
}

/// Try to lazily register directories with the watcher when they first appear.
/// Marks dir as attempted even on failure to avoid spamming warnings.
fn try_register_watches(
    watcher: &mut RecommendedWatcher,
    targets: &[(PathBuf, EncryptionScheme)],
    watched_dirs: &mut HashSet<PathBuf>,
) {
    for (dir, _) in targets {
        if dir.exists() && !watched_dirs.contains(dir) {
            if let Err(e) = watcher.watch(dir, RecursiveMode::NonRecursive) {
                warn!(?dir, error = %e, "ArtifactHarvester: failed to register directory watch, relying on watchdog");
            }
            // Mark as attempted regardless — watchdog timer covers this dir
            watched_dirs.insert(dir.clone());
        }
    }
}

impl Harvester {
    /// Spawn a background harvester task monitoring staging directories.
    pub fn spawn(
        targets: Vec<(PathBuf, EncryptionScheme)>,
        tx: mpsc::Sender<PackagedArtifact>,
    ) -> Self {
        let shutdown_token = CancellationToken::new();
        let loop_shutdown = shutdown_token.clone();

        // Create filesystem watcher bridged to Tokio via unbounded channel.
        // If creation fails, fall back to watchdog-only mode.
        let (fs_tx, mut fs_rx) = mpsc::unbounded_channel();
        let watcher = RecommendedWatcher::new(
            move |res| {
                let _ = fs_tx.send(res);
            },
            notify::Config::default(),
        )
        .inspect_err(|e| {
            warn!(error = %e, "ArtifactHarvester: failed to create watcher, falling back to watchdog-only mode");
        })
        .ok();

        let join_handle = tokio::spawn(async move {
            let mut state = HarvesterState::default();
            let mut watcher = watcher;
            let mut watchdog = tokio::time::interval(Duration::from_millis(WATCHDOG_INTERVAL_MS));
            watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                // Lazy watch registration: register dirs as they appear
                if let Some(ref mut w) = watcher {
                    try_register_watches(w, &targets, &mut state.watched_dirs);
                }

                tokio::select! {
                    _ = loop_shutdown.cancelled() => break,
                    Some(_) = fs_rx.recv() => {}  // kernel event wake-up
                    _ = watchdog.tick() => {}       // watchdog safety net
                }

                // Single reconciliation pass — shared by all wake-up sources
                let mut should_break = false;
                for (dir, scheme) in &targets {
                    if harvest_target(dir, *scheme, &tx, &mut state, false, &loop_shutdown)
                        .await
                        .is_err()
                    {
                        if loop_shutdown.is_cancelled() {
                            should_break = true;
                            break;
                        } else {
                            // Receiver was dropped while not shutting down
                            return;
                        }
                    }
                }
                if should_break {
                    break;
                }
            }

            // Final harvest pass to flush remaining segments and unlink all staging manifests.
            // Use a fresh token — loop_shutdown is already cancelled at this point.
            let flush_token = CancellationToken::new();
            for (dir, scheme) in &targets {
                let _ = harvest_target(dir, *scheme, &tx, &mut state, true, &flush_token).await;
            }
        });

        Self {
            shutdown_token,
            join_handle: Some(join_handle),
        }
    }

    /// Abort the background harvester task immediately.
    pub fn cancel(&mut self) {
        if let Some(handle) = self.join_handle.take() {
            handle.abort();
        }
    }

    /// Signal graceful shutdown, perform a final flush pass, and wait for task termination.
    pub async fn finish_and_flush(mut self) {
        self.shutdown_token.cancel();
        if let Some(handle) = self.join_handle.take() {
            match tokio::time::timeout(Duration::from_secs(5), handle).await {
                Ok(_) => {}
                Err(_) => {
                    warn!("ArtifactHarvester: finish_and_flush timed out after 5s");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_box(box_type: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = (8 + payload.len()) as u32;
        let mut buf = Vec::with_capacity(size as usize);
        buf.extend_from_slice(&size.to_be_bytes());
        buf.extend_from_slice(box_type);
        buf.extend_from_slice(payload);
        buf
    }

    fn make_box_size_zero(box_type: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(box_type);
        buf.extend_from_slice(payload);
        buf
    }

    fn make_box_largesize(box_type: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = (16 + payload.len()) as u64;
        let mut buf = Vec::with_capacity(size as usize);
        buf.extend_from_slice(&1u32.to_be_bytes());
        buf.extend_from_slice(box_type);
        buf.extend_from_slice(&size.to_be_bytes());
        buf.extend_from_slice(payload);
        buf
    }

    #[test]
    fn test_is_complete_isobmff_media_segment_complete() {
        let mut data = Vec::new();
        data.extend_from_slice(&make_box(b"moof", b"moof_data_payload"));
        data.extend_from_slice(&make_box(b"mdat", b"mdat_media_samples"));
        assert!(is_complete_isobmff_media_segment(&data));
    }

    #[test]
    fn test_is_complete_isobmff_media_segment_truncated() {
        let mut data = Vec::new();
        data.extend_from_slice(&make_box(b"moof", b"moof_data"));
        let mdat = make_box(b"mdat", b"mdat_media_samples_long");
        // Truncate mdat by omitting last 5 bytes
        data.extend_from_slice(&mdat[..mdat.len() - 5]);
        assert!(!is_complete_isobmff_media_segment(&data));

        // Incomplete box header (< 8 bytes)
        assert!(!is_complete_isobmff_media_segment(&[0, 0, 0]));
    }

    #[test]
    fn test_is_complete_isobmff_media_segment_size_zero() {
        let mut data = Vec::new();
        data.extend_from_slice(&make_box(b"moof", b"moof_payload"));
        data.extend_from_slice(&make_box_size_zero(b"mdat", b"media_bytes_until_eof"));
        assert!(
            is_complete_isobmff_media_segment(&data),
            "Final box with size == 0 (extends to EOF) must be supported"
        );
    }

    #[test]
    fn test_is_complete_isobmff_media_segment_size_one_largesize() {
        let mut data = Vec::new();
        data.extend_from_slice(&make_box(b"moof", b"moof_payload"));
        data.extend_from_slice(&make_box_largesize(b"mdat", b"largesize_payload"));
        assert!(
            is_complete_isobmff_media_segment(&data),
            "Box with size == 1 (64-bit largesize) must be supported"
        );
    }

    #[test]
    fn test_is_complete_isobmff_illegal_box_sizes_and_malformed() {
        let mut data = Vec::new();
        data.extend_from_slice(&make_box(b"moof", b"moof_payload"));

        // Illegal box size 5 (sizes 2..=7 are forbidden by ISO-BMFF specification)
        let mut illegal_size_box = Vec::new();
        illegal_size_box.extend_from_slice(&5u32.to_be_bytes());
        illegal_size_box.extend_from_slice(b"mdat");
        illegal_size_box.extend_from_slice(b"xyz");
        let mut malformed_data = data.clone();
        malformed_data.extend_from_slice(&illegal_size_box);
        assert!(
            !is_complete_isobmff_media_segment(&malformed_data),
            "Illegal box size between 2 and 7 must be safely rejected without panic"
        );

        // Truncated largesize header: size == 1, but total length < 16 bytes (only 12 bytes)
        let mut truncated_largesize = Vec::new();
        truncated_largesize.extend_from_slice(&1u32.to_be_bytes());
        truncated_largesize.extend_from_slice(b"mdat");
        truncated_largesize.extend_from_slice(&[0u8; 4]); // Only 4 bytes of largesize, need 8
        let mut malformed_largesize = data.clone();
        malformed_largesize.extend_from_slice(&truncated_largesize);
        assert!(
            !is_complete_isobmff_media_segment(&malformed_largesize),
            "Truncated 64-bit largesize header must be rejected without panic"
        );

        // Largesize value smaller than header (e.g. s64 == 10 < 16)
        let mut small_largesize = Vec::new();
        small_largesize.extend_from_slice(&1u32.to_be_bytes());
        small_largesize.extend_from_slice(b"mdat");
        small_largesize.extend_from_slice(&10u64.to_be_bytes());
        let mut malformed_small_largesize = data.clone();
        malformed_small_largesize.extend_from_slice(&small_largesize);
        assert!(
            !is_complete_isobmff_media_segment(&malformed_small_largesize),
            "Largesize value < 16 must be rejected without panic"
        );
    }

    #[test]
    fn test_is_complete_isobmff_media_segment_padding_boxes() {
        let mut data = Vec::new();
        data.extend_from_slice(&make_box(b"free", b"padding1"));
        data.extend_from_slice(&make_box(b"moof", b"moof_payload"));
        data.extend_from_slice(&make_box(b"skip", b"padding2"));
        data.extend_from_slice(&make_box(b"mdat", b"mdat_payload"));
        data.extend_from_slice(&make_box(b"free", b"padding3"));
        assert!(
            is_complete_isobmff_media_segment(&data),
            "Padding boxes (free/skip) should be safely skipped"
        );
    }

    #[test]
    fn test_is_complete_isobmff_init_segment() {
        let ftyp = make_box(b"ftyp", b"iso6mp41");
        let moov = make_box(b"moov", b"moov_metadata");

        // ftyp only must be rejected
        assert!(
            !is_complete_isobmff_init_segment(&ftyp),
            "Init segment with only ftyp must be rejected"
        );

        // moov only must be rejected
        assert!(
            !is_complete_isobmff_init_segment(&moov),
            "Init segment with only moov must be rejected"
        );

        // ftyp + moov must be accepted
        let mut valid_init = Vec::new();
        valid_init.extend_from_slice(&ftyp);
        valid_init.extend_from_slice(&moov);
        assert!(
            is_complete_isobmff_init_segment(&valid_init),
            "Init segment with ftyp + moov must be accepted"
        );

        // Truncated moov must be rejected
        let mut truncated_init = Vec::new();
        truncated_init.extend_from_slice(&ftyp);
        truncated_init.extend_from_slice(&moov[..moov.len() - 3]);
        assert!(
            !is_complete_isobmff_init_segment(&truncated_init),
            "Truncated init segment must be rejected"
        );
    }

    #[tokio::test]
    async fn test_harvest_target_hls_truncated_segment_not_emitted() {
        let temp_dir =
            std::env::temp_dir().join(format!("drmpack_hls_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        // Write an HLS manifest that lists video_720p_1.m4s
        let playlist =
            "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\nvideo_720p_1.m4s\n";
        tokio::fs::write(temp_dir.join("video_720p.m3u8"), playlist)
            .await
            .unwrap();

        // Write a truncated segment file on disk (only 6 bytes, incomplete box)
        let seg_path = temp_dir.join("video_720p_1.m4s");
        tokio::fs::write(&seg_path, b"trunc!").await.unwrap();

        let (tx, mut rx) = mpsc::channel(16);
        let mut state = HarvesterState::default();

        let res = harvest_target(
            &temp_dir,
            EncryptionScheme::Cenc,
            &tx,
            &mut state,
            false,
            &CancellationToken::new(),
        )
        .await;
        assert!(res.is_ok());

        // Drain channel to check if any media segment was emitted
        let mut emitted_segments = Vec::new();
        while let Ok(art) = rx.try_recv() {
            if art.kind == ArtifactKind::MediaSegment {
                emitted_segments.push(art.filename);
            }
        }

        assert!(
            emitted_segments.is_empty(),
            "Truncated media segment must NOT be emitted even if referenced in manifest; got {:?}",
            emitted_segments
        );
        assert!(
            seg_path.exists(),
            "Truncated media segment file must NOT be unlinked from staging"
        );

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_harvest_target_media_segment_not_in_manifest_not_emitted() {
        let temp_dir = std::env::temp_dir().join(format!(
            "drmpack_manifest_readiness_{}",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        // Write an HLS manifest that ONLY lists video_720p_1.m4s
        let playlist =
            "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\nvideo_720p_1.m4s\n";
        tokio::fs::write(temp_dir.join("video_720p.m3u8"), playlist)
            .await
            .unwrap();

        // Write complete video_720p_1.m4s
        let mut seg1_data = Vec::new();
        seg1_data.extend_from_slice(&make_box(b"moof", b"moof_1"));
        seg1_data.extend_from_slice(&make_box(b"mdat", b"mdat_1"));
        let seg1_path = temp_dir.join("video_720p_1.m4s");
        tokio::fs::write(&seg1_path, &seg1_data).await.unwrap();

        // Write complete video_720p_2.m4s which is NOT YET in the playlist!
        let mut seg2_data = Vec::new();
        seg2_data.extend_from_slice(&make_box(b"moof", b"moof_2"));
        seg2_data.extend_from_slice(&make_box(b"mdat", b"mdat_2"));
        let seg2_path = temp_dir.join("video_720p_2.m4s");
        tokio::fs::write(&seg2_path, &seg2_data).await.unwrap();

        let (tx, mut rx) = mpsc::channel(16);
        let mut state = HarvesterState::default();

        let res = harvest_target(
            &temp_dir,
            EncryptionScheme::Cenc,
            &tx,
            &mut state,
            false,
            &CancellationToken::new(),
        )
        .await;
        assert!(res.is_ok());

        let mut emitted_segments = Vec::new();
        while let Ok(art) = rx.try_recv() {
            if art.kind == ArtifactKind::MediaSegment {
                emitted_segments.push(art.filename);
            }
        }

        // Only seg1 should be emitted! seg2 is NOT in the manifest!
        assert_eq!(
            emitted_segments,
            vec!["video_720p_1.m4s"],
            "Only segment listed in manifest must be emitted; got: {:?}",
            emitted_segments
        );
        assert!(
            !seg1_path.exists(),
            "Emitted seg1 must be unlinked from staging"
        );
        assert!(
            seg2_path.exists(),
            "Seg2 is not in manifest yet, so it must NOT be unlinked from staging!"
        );

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[test]
    fn test_is_valid_hls_manifest() {
        // Not starting with #EXTM3U
        assert!(!is_valid_hls_manifest("hello world"));

        // Master playlist with #EXT-X-STREAM-INF: is valid
        let valid_master =
            "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-STREAM-INF:BANDWIDTH=1280000\nv1.m3u8\n";
        assert!(is_valid_hls_manifest(valid_master));

        // Master playlist without #EXT-X-STREAM-INF: is invalid
        let incomplete_master = "#EXTM3U\n#EXT-X-VERSION:6\n";
        assert!(!is_valid_hls_manifest(incomplete_master));

        // Media playlist without #EXT-X-TARGETDURATION: is invalid
        let invalid_media = "#EXTM3U\n#EXT-X-VERSION:6\n#EXTINF:2.0,\nseg1.m4s\n";
        assert!(!is_valid_hls_manifest(invalid_media));

        // Media playlist with #EXT-X-TARGETDURATION: but no segments or endlist is invalid
        let empty_media = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n";
        assert!(!is_valid_hls_manifest(empty_media));

        // Media playlist with only #EXT-X-MAP:URI="...init.mp4" but no segment entries or endlist is invalid
        let init_map_only = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n#EXT-X-MAP:URI=\"video_720p_init.mp4\"\n";
        assert!(
            !is_valid_hls_manifest(init_map_only),
            "Media playlist with only init map but no segment entries or endlist must be invalid"
        );

        // Media playlist with #EXT-X-TARGETDURATION: and segment entry is valid
        let valid_media =
            "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\nseg1.m4s\n";
        assert!(is_valid_hls_manifest(valid_media));

        // Media playlist with #EXT-X-TARGETDURATION: and #EXT-X-ENDLIST is valid
        let endlist_media = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n#EXT-X-ENDLIST\n";
        assert!(is_valid_hls_manifest(endlist_media));
    }

    #[test]
    fn test_harvester_state_bounded_history() {
        let mut state = HarvesterState::default();
        for i in 0..6000 {
            state.record_emitted(EncryptionScheme::Cenc, format!("seg_{i}.m4s"));
        }

        let total_emitted: usize = state.emitted_segments.values().map(|s| s.len()).sum();
        assert_eq!(
            total_emitted, MAX_EMITTED_HISTORY,
            "emitted_segments must be bounded to MAX_EMITTED_HISTORY"
        );
        assert_eq!(
            state.emitted_order.len(),
            MAX_EMITTED_HISTORY,
            "emitted_order must be bounded to MAX_EMITTED_HISTORY"
        );

        // Oldest segments (0..1000) must have been evicted
        assert!(!state.is_emitted(EncryptionScheme::Cenc, "seg_0.m4s"));
        assert!(!state.is_emitted(EncryptionScheme::Cenc, "seg_999.m4s"));

        // Recent segments (5000..6000) must still be retained
        assert!(state.is_emitted(EncryptionScheme::Cenc, "seg_5500.m4s"));
        assert!(state.is_emitted(EncryptionScheme::Cenc, "seg_5999.m4s"));
    }

    #[tokio::test]
    async fn test_harvest_target_init_segment_complete_and_incomplete() {
        let temp_dir =
            std::env::temp_dir().join(format!("drmpack_init_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        // Manifest referencing rendition video_720p
        let playlist = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n#EXT-X-MAP:URI=\"video_720p_init.mp4\"\n#EXTINF:2.0,\nvideo_720p_1.m4s\n";
        tokio::fs::write(temp_dir.join("video_720p.m3u8"), playlist)
            .await
            .unwrap();

        // Write incomplete init segment (ftyp only)
        let init_path = temp_dir.join("video_720p_init.mp4");
        tokio::fs::write(&init_path, make_box(b"ftyp", b"iso6"))
            .await
            .unwrap();

        let (tx, mut rx) = mpsc::channel(16);
        let mut state = HarvesterState::default();

        let _ = harvest_target(
            &temp_dir,
            EncryptionScheme::Cenc,
            &tx,
            &mut state,
            false,
            &CancellationToken::new(),
        )
        .await;
        while let Ok(art) = rx.try_recv() {
            assert_ne!(
                art.kind,
                ArtifactKind::InitSegment,
                "Incomplete init segment must not be emitted"
            );
        }
        assert!(
            init_path.exists(),
            "Incomplete init segment must not be unlinked"
        );

        // Now write complete init segment (ftyp + moov)
        let mut complete_init = Vec::new();
        complete_init.extend_from_slice(&make_box(b"ftyp", b"iso6"));
        complete_init.extend_from_slice(&make_box(b"moov", b"moov_data"));
        tokio::fs::write(&init_path, &complete_init).await.unwrap();

        let _ = harvest_target(
            &temp_dir,
            EncryptionScheme::Cenc,
            &tx,
            &mut state,
            false,
            &CancellationToken::new(),
        )
        .await;
        let mut emitted_init = None;
        while let Ok(art) = rx.try_recv() {
            if art.kind == ArtifactKind::InitSegment {
                emitted_init = Some(art);
            }
        }
        let emitted = emitted_init.expect("Complete init segment must be emitted");
        assert_eq!(emitted.filename, "video_720p_init.mp4");
        assert_eq!(emitted.kind, ArtifactKind::InitSegment);
        assert!(
            !init_path.exists(),
            "Emitted init segment must be unlinked from staging"
        );

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[test]
    fn test_parse_and_format_iso8601_duration() {
        assert_eq!(parse_iso8601_duration("PT0H0M10.021S"), Some(10.021));
        assert_eq!(parse_iso8601_duration("PT10.021S"), Some(10.021));
        assert_eq!(parse_iso8601_duration("PT1M30S"), Some(90.0));
        assert_eq!(parse_iso8601_duration("PT1H2M3.5S"), Some(3723.5));
        assert_eq!(parse_iso8601_duration("INVALID"), None);

        assert_eq!(format_iso8601_duration(10.0), "PT0H0M10.000S");
        assert_eq!(format_iso8601_duration(10.021), "PT0H0M10.021S");
        assert_eq!(format_iso8601_duration(3661.5), "PT1H1M1.500S");
    }

    #[test]
    fn test_parse_segment_template_duration() {
        let xml = r#"<MPD><Period><AdaptationSet><SegmentTemplate timescale="15360" startNumber="1" duration="30720"/></AdaptationSet></Period></MPD>"#;
        assert_eq!(parse_segment_template_duration(xml), Some(2.0));
    }

    #[test]
    fn test_sanitize_static_mpd_clamping() {
        let raw_mpd = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" minBufferTime="PT2.000S" type="static" mediaPresentationDuration="PT0H0M10.021S" profiles="urn:mpeg:dash:profile:isoff-live:2011">
 <Period id="DID1" duration="PT0H0M10.021S">
  <AdaptationSet mimeType="video/mp4">
   <SegmentTemplate media="$RepresentationID$_$Number$.m4s" initialization="$RepresentationID$_init.mp4" timescale="15360" startNumber="1" duration="30720"/>
   <Representation id="video_720p" width="1280" height="720"/>
  </AdaptationSet>
 </Period>
</MPD>"#;

        // With max_seg = 5 (5 * 2.0s = 10.0s), duration 10.021s must be clamped to 10.000s
        let sanitized = sanitize_static_mpd(raw_mpd, Some(5));
        assert!(sanitized.contains(r#"mediaPresentationDuration="PT0H0M10.000S""#));
        assert!(sanitized.contains(r#"<Period id="DID1" duration="PT0H0M10.000S">"#));
        assert!(!sanitized.contains("10.021S"));

        // With max_seg = 6 (6 * 2.0s = 12.0s >= 10.021s), duration must NOT be modified
        let unchanged = sanitize_static_mpd(raw_mpd, Some(6));
        assert_eq!(unchanged, raw_mpd);
    }

    #[tokio::test]
    async fn test_harvest_target_dash_only_no_early_segment_read() {
        let temp_dir =
            std::env::temp_dir().join(format!("drmpack_dash_only_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        // Write a valid DASH MPD with SegmentTemplate containing rep_id "video_720p"
        // but NO HLS manifest at all
        let mpd = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic" minBufferTime="PT2.000S">
 <Period id="P1">
  <AdaptationSet mimeType="video/mp4">
   <SegmentTemplate media="$RepresentationID$_$Number$.m4s" initialization="$RepresentationID$_init.mp4" timescale="15360" startNumber="1" duration="30720"/>
   <Representation id="video_720p" width="1280" height="720"/>
  </AdaptationSet>
 </Period>
</MPD>"#;
        tokio::fs::write(temp_dir.join("live.mpd"), mpd)
            .await
            .unwrap();

        // Write a complete segment on disk
        let mut seg_data = Vec::new();
        seg_data.extend_from_slice(&make_box(b"moof", b"moof_payload"));
        seg_data.extend_from_slice(&make_box(b"mdat", b"mdat_payload"));
        let seg_path = temp_dir.join("video_720p_1.m4s");
        tokio::fs::write(&seg_path, &seg_data).await.unwrap();

        let (tx, mut rx) = mpsc::channel(16);
        let mut state = HarvesterState::default();

        // Non-final harvest: segment should NOT be emitted (no HLS readiness signal)
        let res = harvest_target(
            &temp_dir,
            EncryptionScheme::Cenc,
            &tx,
            &mut state,
            false,
            &CancellationToken::new(),
        )
        .await;
        assert!(res.is_ok());

        let mut emitted = Vec::new();
        while let Ok(art) = rx.try_recv() {
            if art.kind == ArtifactKind::MediaSegment {
                emitted.push(art.filename);
            }
        }
        assert!(
            emitted.is_empty(),
            "DASH-only: segment must NOT be emitted without HLS readiness signal; got {:?}",
            emitted
        );
        assert!(
            seg_path.exists(),
            "Segment must NOT be unlinked without HLS signal"
        );

        // Final harvest: segment SHOULD be emitted (is_final overrides)
        let res = harvest_target(
            &temp_dir,
            EncryptionScheme::Cenc,
            &tx,
            &mut state,
            true,
            &CancellationToken::new(),
        )
        .await;
        assert!(res.is_ok());

        let mut final_emitted = Vec::new();
        while let Ok(art) = rx.try_recv() {
            if art.kind == ArtifactKind::MediaSegment {
                final_emitted.push(art.filename);
            }
        }
        assert_eq!(
            final_emitted,
            vec!["video_720p_1.m4s"],
            "Final harvest must emit the segment regardless of manifest type"
        );

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[test]
    fn test_sanitize_static_mpd_ignores_dynamic() {
        let dynamic_mpd = r#"<MPD type="dynamic" mediaPresentationDuration="PT0H0M10.021S"><Period duration="PT0H0M10.021S"><SegmentTemplate timescale="1000" duration="2000"/></Period></MPD>"#;
        assert_eq!(sanitize_static_mpd(dynamic_mpd, Some(5)), dynamic_mpd);
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -- test_harvest_performance --ignored --nocapture
    async fn test_harvest_performance_n_segments() {
        let temp_dir =
            std::env::temp_dir().join(format!("drmpack_perf_bench_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let total_segments = 100;
        let listed_in_manifest = 50;

        // Create complete segment files
        let mut seg_data = Vec::new();
        seg_data.extend_from_slice(&make_box(b"moof", b"moof_bench_payload_data"));
        seg_data.extend_from_slice(&make_box(b"mdat", b"mdat_bench_media_samples"));

        for i in 1..=total_segments {
            let path = temp_dir.join(format!("video_720p_{i}.m4s"));
            tokio::fs::write(&path, &seg_data).await.unwrap();
        }

        // HLS manifest listing first 50 segments
        let mut playlist = String::from("#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n");
        for i in 1..=listed_in_manifest {
            playlist.push_str(&format!("#EXTINF:2.0,\nvideo_720p_{i}.m4s\n"));
        }
        tokio::fs::write(temp_dir.join("video_720p.m3u8"), &playlist)
            .await
            .unwrap();

        let (tx, mut rx) = mpsc::channel(256);
        let mut state = HarvesterState::default();

        let start = std::time::Instant::now();
        let res = harvest_target(
            &temp_dir,
            EncryptionScheme::Cenc,
            &tx,
            &mut state,
            false,
            &CancellationToken::new(),
        )
        .await;
        let elapsed = start.elapsed();

        assert!(res.is_ok());

        let mut emitted_count = 0;
        while let Ok(art) = rx.try_recv() {
            if art.kind == ArtifactKind::MediaSegment {
                emitted_count += 1;
            }
        }

        println!("=== Harvest Performance ===");
        println!("  Total segments on disk: {total_segments}");
        println!("  Listed in HLS manifest: {listed_in_manifest}");
        println!("  Emitted: {emitted_count}");
        println!("  Elapsed: {elapsed:?}");
        println!("===========================");

        assert_eq!(emitted_count, listed_in_manifest);
        assert!(
            elapsed < std::time::Duration::from_millis(200),
            "harvest_target for {total_segments} segments took {elapsed:?}, expected < 200ms"
        );

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_harvester_cancellation_unblocks_full_channel() {
        let temp_dir =
            std::env::temp_dir().join(format!("drmpack_cancel_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        // Write complete init segment (emitted without manifest)
        let mut complete_init = Vec::new();
        complete_init.extend_from_slice(&make_box(b"ftyp", b"isom"));
        complete_init.extend_from_slice(&make_box(b"moov", b"moov_payload"));
        let init_path = temp_dir.join("video_720p_init.mp4");
        tokio::fs::write(&init_path, &complete_init).await.unwrap();

        // Write an HLS manifest that lists video_720p_1.m4s
        let playlist =
            "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\nvideo_720p_1.m4s\n";
        tokio::fs::write(temp_dir.join("video_720p.m3u8"), playlist)
            .await
            .unwrap();

        let mut seg_data = Vec::new();
        seg_data.extend_from_slice(&make_box(b"moof", b"moof_data"));
        seg_data.extend_from_slice(&make_box(b"mdat", b"mdat_data"));

        let seg1_path = temp_dir.join("video_720p_1.m4s");
        tokio::fs::write(&seg1_path, &seg_data).await.unwrap();

        // Channel buffer capacity 1:
        // InitSegment will fill the buffer (1/1).
        // Next segment (video_720p_1.m4s) will block on tx.send()!
        let (tx, _rx) = mpsc::channel(1);
        let mut state = HarvesterState::default();
        let shutdown = CancellationToken::new();

        let shutdown_clone = shutdown.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            shutdown_clone.cancel();
        });

        let res = harvest_target(
            &temp_dir,
            EncryptionScheme::Cenc,
            &tx,
            &mut state,
            false,
            &shutdown,
        )
        .await;

        // Must return Err(()) due to cancellation without hanging
        assert!(
            res.is_err(),
            "harvest_target must exit with error on cancellation"
        );

        // First item (init segment) was sent and deleted
        assert!(
            !init_path.exists(),
            "Init segment was sent and should be deleted"
        );

        // The blocked segment was NOT sent, so it MUST NOT be deleted from disk
        assert!(seg1_path.exists(), "Blocked segment must remain on disk");
        assert!(
            !state.is_emitted(EncryptionScheme::Cenc, "video_720p_1.m4s"),
            "Blocked segment must not be recorded as emitted"
        );

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_harvester_finish_and_flush_graceful_shutdown() {
        let temp_dir =
            std::env::temp_dir().join(format!("drmpack_flush_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let (tx, mut rx) = mpsc::channel(16);
        let harvester = Harvester::spawn(vec![(temp_dir.clone(), EncryptionScheme::Cenc)], tx);

        // Spawn a background consumer to keep receiving artifacts
        let consumer = tokio::spawn(async move {
            let mut count = 0;
            while let Some(_art) = rx.recv().await {
                count += 1;
            }
            count
        });

        // Write an init segment
        let mut complete_init = Vec::new();
        complete_init.extend_from_slice(&make_box(b"ftyp", b"isom"));
        complete_init.extend_from_slice(&make_box(b"moov", b"moov_payload"));
        tokio::fs::write(temp_dir.join("video_init.mp4"), &complete_init)
            .await
            .unwrap();

        // Calling finish_and_flush should complete promptly and flush remaining artifacts
        harvester.finish_and_flush().await;

        let received = consumer.await.unwrap();
        assert!(
            received >= 1,
            "Should receive at least the init segment on flush"
        );

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
