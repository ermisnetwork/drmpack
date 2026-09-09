//! Example 08: Live Packaging Pipeline Test
//!
//! Self-contained test of the drmpack packaging pipeline:
//! FFmpeg live source → SessionWriter → PackagingSession → artifact logging + disk dump.
//!
//! Usage:
//!   cargo run --example 08_in_memory_live_stream
//!   cargo run --example 08_in_memory_live_stream -- --duration 60
//!   cargo run --example 08_in_memory_live_stream -- --input path/to/video.mp4
//!   cargo run --example 08_in_memory_live_stream -- --dump-dir /tmp/my_output

use drmpack::axinom::{AxinomConfig, AxinomProvider};
use drmpack::key::StaticKeySource;
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::{ArtifactKind, Rendition};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

const DEFAULT_DURATION_SECS: u64 = 30;
const DEFAULT_DUMP_DIR: &str = "scratch/example08_live";
const SEGMENT_DURATION: f64 = 2.0;
const CONTENT_KEY: [u8; 16] = [0x55; 16];

type LatencyMap = HashMap<u64, (Instant, Option<Instant>)>;

#[derive(Clone, Default)]
struct LatencyTracker {
    // Maps segment_seq -> (T_ingress_start, Option<T_ingress_complete>)
    entries: Arc<Mutex<LatencyMap>>,
}

impl LatencyTracker {
    fn on_moof(&self, seq: u64) {
        let now = Instant::now();
        let mut map = self.entries.lock().unwrap();
        map.insert(seq, (now, None));
        // When fragment seq starts entering, fragment (seq - 1) has finished entering GPAC
        if seq > 1 {
            if let Some(prev) = map.get_mut(&(seq - 1)) {
                if prev.1.is_none() {
                    prev.1 = Some(now);
                }
            }
        }
    }

    fn mark_final(&self, seq: u64) {
        let now = Instant::now();
        let mut map = self.entries.lock().unwrap();
        if let Some(entry) = map.get_mut(&seq) {
            if entry.1.is_none() {
                entry.1 = Some(now);
            }
        }
    }

    fn get_latencies(&self, seq: u64) -> Option<(u128, u128)> {
        let map = self.entries.lock().unwrap();
        let (start, pushed) = map.get(&seq)?;
        let dwell = start.elapsed().as_millis();
        let turnaround = pushed.map(|p| p.elapsed().as_millis()).unwrap_or(0);
        Some((dwell, turnaround))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    let args: Vec<String> = std::env::args().collect();
    let duration_secs: u64 = parse_arg(&args, "--duration").unwrap_or(DEFAULT_DURATION_SECS);
    let dump_dir =
        parse_str_arg(&args, "--dump-dir").unwrap_or_else(|| DEFAULT_DUMP_DIR.to_string());
    let input = parse_str_arg(&args, "--input");
    let is_dual = args.iter().any(|a| a == "--dual");
    let use_axinom = !args.iter().any(|a| a == "--static")
        && (args.iter().any(|a| a == "--axinom") || AxinomConfig::from_env().is_ok());

    println!("drmpack Example 08: Live Packaging Pipeline");
    println!("────────────────────────────────────────────");
    println!(
        "Duration: {duration_secs}s | Mode: {} | Dump: {dump_dir}",
        if is_dual { "Dual (CENC+CBCS)" } else { "CBCS" }
    );

    let dump_path = std::path::PathBuf::from(&dump_dir);
    let _ = tokio::fs::remove_dir_all(&dump_path).await;
    tokio::fs::create_dir_all(&dump_path).await?;

    // ── Session ────────────────────────────────────────────────────
    let content_id = format!("example08-live-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let config = if is_dual {
        PackagingSessionConfig::dual(&content_id)
    } else {
        PackagingSessionConfig::cbcs(&content_id)
    }
    .with_rendition(Rendition::video_hd())
    .with_rendition(Rendition::audio())
    .with_segment_duration(SEGMENT_DURATION);

    let mut session = if use_axinom {
        let ax_config = AxinomConfig::from_env().map_err(|e| {
            eprintln!("Failed to load Axinom credentials from .env: {e}");
            e
        })?;
        println!("Provider: Axinom Key Service (SPEKE v2 / CPIX 2.3)");
        println!("Endpoint: {}", ax_config.endpoint);
        println!("Acquiring live DRM keys from Axinom...");
        let provider = AxinomProvider::new(ax_config);
        PackagingSession::create(config, &provider).await?
    } else {
        println!("Provider: StaticKeySource (offline shared key)");
        let provider = StaticKeySource::shared_key(CONTENT_KEY);
        PackagingSession::create(config, &provider).await?
    };

    println!("Session initialized. Active Keys:");
    for k in session.key_set().all_keys() {
        let scheme = k
            .encryption_scheme
            .map(|s| s.to_string())
            .unwrap_or_else(|| "all".into());
        println!(
            "  - [{scheme}] KID: {} ({:?} / {})",
            k.kid.0, k.track_type, k.quality_tier
        );
    }
    println!();
    let tracker = LatencyTracker::default();
    // Consumer: log artifacts + dump to disk
    let mut rx = session
        .take_output_receiver()
        .expect("output channel already claimed");
    let consumer_dir = dump_path.clone();
    let start = Instant::now();
    let consumer_tracker = tracker.clone();

    let consumer = tokio::spawn(async move {
        let (mut inits, mut segs, mut manifests, mut total) = (0u32, 0u32, 0u32, 0usize);

        while let Some(art) = rx.recv().await {
            let elapsed_ms = start.elapsed().as_millis();
            let secs = elapsed_ms / 1000;
            let ms = elapsed_ms % 1000;
            let size = art.data.len();
            let tag = match art.kind {
                ArtifactKind::InitSegment => {
                    inits += 1;
                    "INIT    "
                }
                ArtifactKind::MediaSegment => {
                    segs += 1;
                    "SEGMENT "
                }
                ArtifactKind::Manifest => {
                    manifests += 1;
                    "MANIFEST"
                }
            };
            total += size;

            let mut latency_info = String::new();
            if art.kind == ArtifactKind::MediaSegment {
                if let Some(seq) = parse_segment_seq(&art.filename) {
                    if let Some((dwell, turnaround)) = consumer_tracker.get_latencies(seq) {
                        latency_info =
                            format!(" | latency: {:>3}ms (dwell: {}ms)", turnaround, dwell);
                    }
                }
            }

            println!(
                "[{:02}:{:02}.{:03}] {tag} [{}] {:<24} {:>9}{latency_info}",
                secs / 60,
                secs % 60,
                ms,
                art.scheme,
                art.filename,
                fmt_size(size)
            );
            let target = if is_dual {
                consumer_dir
                    .join(art.scheme.to_string())
                    .join(&art.filename)
            } else {
                consumer_dir.join(&art.filename)
            };
            if let Some(parent) = target.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(target, &art.data).await;
        }

        (inits, segs, manifests, total)
    });

    // FFmpeg live source
    let mut ffmpeg = spawn_ffmpeg(input.as_deref())?;
    let mut stdout = ffmpeg.stdout.take().expect("ffmpeg stdout");

    // Feed via SessionWriter with latency tracking
    let mut writer = session.writer();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(duration_secs);

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("\nCtrl+C. Finalizing...");
        }
        _ = tokio::time::sleep_until(deadline) => {
            println!("\nDuration reached. Finalizing...");
        }
        res = feed_stream(&mut stdout, &mut writer, &tracker) => {
            if let Err(e) = res { eprintln!("Feed error: {e}"); }
        }
    }

    let _ = ffmpeg.kill().await;
    writer.close().await;

    match session.close().await {
        Ok(()) => println!("Session closed."),
        Err(e) => eprintln!("Session close warning: {e}"),
    }

    let (inits, segs, manifests, total) = consumer.await?;
    println!("\nSummary");
    println!("  Init segments:  {:>4}", inits);
    println!("  Media segments: {:>4}", segs);
    println!("  Manifests:      {:>4}", manifests);
    println!("  Total bytes:    {:>8}", fmt_size(total));
    println!("  Dump dir:       {dump_dir}");

    Ok(())
}

/// Feeds fMP4 stream box-by-box into the writer while timestamping fragment boundaries.
async fn feed_stream(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    tracker: &LatencyTracker,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(reader);
    let mut hdr = [0u8; 8];
    let mut last_seq = 0u64;

    while reader.read_exact(&mut hdr).await.is_ok() {
        let size = u32::from_be_bytes(hdr[0..4].try_into().unwrap()) as usize;
        let box_type = &hdr[4..8];
        let payload_size = size.saturating_sub(8);

        let mut payload = vec![0u8; payload_size];
        if reader.read_exact(&mut payload).await.is_err() {
            break;
        }

        if box_type == b"moof" {
            let seq = if payload.len() >= 16 && &payload[4..8] == b"mfhd" {
                u32::from_be_bytes(payload[12..16].try_into().unwrap()) as u64
            } else {
                last_seq + 1
            };
            last_seq = seq;
            tracker.on_moof(seq);
        }

        writer.write_all(&hdr).await?;
        writer.write_all(&payload).await?;
    }

    tracker.mark_final(last_seq);
    Ok(())
}

fn parse_segment_seq(filename: &str) -> Option<u64> {
    filename
        .rsplit_once('_')?
        .1
        .strip_suffix(".m4s")?
        .parse::<u64>()
        .ok()
}

fn spawn_ffmpeg(input: Option<&str>) -> Result<tokio::process::Child, Box<dyn std::error::Error>> {
    let mut cmd = tokio::process::Command::new("ffmpeg");

    if let Some(path) = input {
        println!("FFmpeg source: {path} (loop)");
        cmd.args(["-re", "-stream_loop", "-1", "-i", path]);
    } else {
        println!("FFmpeg source: lavfi testsrc 1280x720 + sine");
        cmd.args(["-re", "-f", "lavfi", "-i", "testsrc=size=1280x720:rate=30"]);
        cmd.args([
            "-re",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:sample_rate=48000",
        ]);
    }

    cmd.args([
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-tune",
        "zerolatency",
        "-g",
        "60",
        "-keyint_min",
        "60",
        "-sc_threshold",
        "0",
        "-profile:v",
        "baseline",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-b:a",
        "128k",
        "-ar",
        "48000",
        "-movflags",
        "empty_moov+default_base_moof+frag_keyframe",
        "-f",
        "mp4",
        "pipe:1",
    ]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());

    Ok(cmd.spawn()?)
}

fn fmt_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn parse_arg<T: std::str::FromStr>(args: &[String], flag: &str) -> Option<T> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
}

fn parse_str_arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
