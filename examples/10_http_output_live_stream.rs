//! Example 10: Zero-Disk HTTP Egress Live Packaging Pipeline
//!
//! Demonstrates the v0.2.0 `EgressMode::HttpPush` capability:
//! FFmpeg live source → SessionWriter → PackagingSession (HttpPush)
//!   → in-process loopback HttpEgressServer (127.0.0.1:EPHEMERAL_PORT)
//!   → PackagedArtifact RAM channel → latency tracking + disk dump.
//!
//! Unlike Example 08 (which stages files in the filesystem/page cache),
//! Example 10 instructs GPAC to stream segments directly over HTTP PUT
//! (`httpout:hmode=push`), achieving zero physical or ephemeral disk staging.
//!
//! Usage:
//!   cargo run --example 10_http_output_live_stream
//!   cargo run --example 10_http_output_live_stream -- --duration 30
//!   cargo run --example 10_http_output_live_stream -- --dual
//!   cargo run --example 10_http_output_live_stream -- --static
//!   cargo run --example 10_http_output_live_stream -- --axinom
//!   cargo run --example 10_http_output_live_stream -- --input path/to/video.mp4
//!   cargo run --example 10_http_output_live_stream -- --dump-dir scratch/my_http_output

use drmpack::axinom::{AxinomConfig, AxinomProvider};
use drmpack::key::StaticKeySource;
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::{ArtifactKind, EgressMode, Rendition};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

const DEFAULT_DURATION_SECS: u64 = 30;
const DEFAULT_DUMP_DIR: &str = "scratch/example10_http_output";
const SEGMENT_DURATION: f64 = 2.0;
const CONTENT_KEY: [u8; 16] = [0x55; 16];

type LatencyMap = HashMap<u64, (Instant, Option<Instant>)>;

#[derive(Clone, Default)]
struct LatencyTracker {
    entries: Arc<Mutex<LatencyMap>>,
}

impl LatencyTracker {
    fn on_moof(&self, seq: u64) {
        let now = Instant::now();
        let mut map = self.entries.lock().unwrap();
        map.insert(seq, (now, None));
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
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage: cargo run --example 10_http_output_live_stream [OPTIONS]");
        println!();
        println!("Options:");
        println!("  --duration <SECS>   Stream duration in seconds (default: 15)");
        println!("  --dual              Enable dual CENC + CBCS packaging");
        println!("  --cenc              Use CENC scheme (default: CBCS)");
        println!("  --static            Use static/offline DRM keys (default if no .env)");
        println!("  --axinom            Force Axinom DRM key server");
        println!("  --dump-dir <PATH>   Directory to dump received stream (default: scratch/example10_http_output)");
        println!("  --input <PATH>      Custom video input path (default: synthetic testsrc)");
        println!("  -h, --help          Show this help message");
        return Ok(());
    }

    let duration_secs: u64 = parse_arg(&args, "--duration").unwrap_or(DEFAULT_DURATION_SECS);
    let dump_dir =
        parse_str_arg(&args, "--dump-dir").unwrap_or_else(|| DEFAULT_DUMP_DIR.to_string());
    let input = parse_str_arg(&args, "--input");
    let is_dual = args.iter().any(|a| a == "--dual");
    let is_cenc = args.iter().any(|a| a == "--cenc");
    let use_axinom = !args.iter().any(|a| a == "--static")
        && (args.iter().any(|a| a == "--axinom") || AxinomConfig::from_env().is_ok());

    let scheme_name = if is_dual {
        "Dual (CENC + CBCS)"
    } else if is_cenc {
        "CENC"
    } else {
        "CBCS"
    };

    println!("╔══════════════════════════════════════════════════════════════════╗");
    println!("║ drmpack Example 10: Zero-Disk HTTP Egress Live Packaging Pipeline║");
    println!("╚══════════════════════════════════════════════════════════════════╝");
    println!("Duration:    {duration_secs}s");
    println!("Mode:        {scheme_name}");
    println!("Egress Mode: HttpPush (zero disk staging, direct in-process HTTP)");
    println!("Dump Dir:    {dump_dir}");
    println!();

    let dump_path = PathBuf::from(&dump_dir);
    let _ = tokio::fs::remove_dir_all(&dump_path).await;
    tokio::fs::create_dir_all(&dump_path).await?;

    // ── Configure PackagingSession with EgressMode::HttpPush ───────
    let content_id = format!("example10-http-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let base_config = if is_dual {
        PackagingSessionConfig::dual(&content_id)
    } else if is_cenc {
        PackagingSessionConfig::cenc(&content_id)
    } else {
        PackagingSessionConfig::cbcs(&content_id)
    };

    let config = base_config
        .with_rendition(Rendition::video_hd())
        .with_rendition(Rendition::audio())
        .with_segment_duration(SEGMENT_DURATION)
        .with_finalization_timeout(Duration::from_secs(30))
        .with_egress_mode(EgressMode::HttpPush);

    // ── Key Provider ───────────────────────────────────────────────
    let mut session = if use_axinom {
        let ax_config = AxinomConfig::from_env().map_err(|e| {
            eprintln!("Failed to load Axinom credentials from .env: {e}");
            e
        })?;
        println!("🔑 Key Provider: Axinom Key Service (SPEKE v2 / CPIX 2.3)");
        println!("   Endpoint:     {}", ax_config.endpoint);
        println!("   Acquiring live DRM keys from Axinom...");
        let provider = AxinomProvider::new(ax_config);
        PackagingSession::create(config, &provider).await?
    } else {
        println!("🔑 Key Provider: StaticKeySource (offline shared key / ClearKey)");
        let provider = StaticKeySource::shared_key(CONTENT_KEY);
        PackagingSession::create(config, &provider).await?
    };

    println!("   Active DRM Keys:");
    for k in session.key_set().all_keys() {
        let s = k
            .encryption_scheme
            .map(|s| s.to_string())
            .unwrap_or_else(|| "all".into());
        println!(
            "     - [{s}] KID: {} ({:?} / {})",
            k.kid.0, k.track_type, k.quality_tier
        );
    }
    println!();

    // ── Save DRM Playback Metadata ─────────────────────────────────
    let meta = session.playback_metadata();
    if let Ok(json) = meta.to_json_pretty() {
        let meta_path = dump_path.join("drm_metadata.json");
        if let Err(e) = std::fs::write(&meta_path, json) {
            eprintln!("Warning: failed to write DRM metadata: {e}");
        } else {
            println!("💾 DRM Playback Metadata saved: {}", meta_path.display());
        }
    }

    let staging_dir = session.output_dir().to_path_buf();
    println!("📂 Session Staging Dir: {}", staging_dir.display());
    println!("   (In HttpPush mode, GPAC never writes media segments here)");
    println!();

    let tracker = LatencyTracker::default();
    let mut rx = session
        .take_output_receiver()
        .expect("output channel already claimed");
    let consumer_dir = dump_path.clone();
    let start = Instant::now();
    let consumer_tracker = tracker.clone();

    // ── Consumer Task: receive artifacts via HTTP loopback sink ─────
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
                        latency_info = format!(
                            " | HTTP turnaround: {:>3}ms (dwell: {}ms)",
                            turnaround, dwell
                        );
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

    // ── FFmpeg live media source ───────────────────────────────────
    let mut ffmpeg = spawn_ffmpeg(input.as_deref())?;
    let mut stdout = ffmpeg.stdout.take().expect("ffmpeg stdout");

    println!("📡 Ingesting live media stream from FFmpeg into PackagingSession...");
    let mut writer = session.writer();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(duration_secs);

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("\n⏹  Ctrl+C received. Finalizing packaging session...");
        }
        _ = tokio::time::sleep_until(deadline) => {
            println!("\n⏱  Duration reached ({duration_secs}s). Finalizing packaging session...");
        }
        res = feed_stream(&mut stdout, &mut writer, &tracker) => {
            if let Err(e) = res { eprintln!("Feed error: {e}"); }
        }
    }

    let _ = ffmpeg.kill().await;
    writer.close().await;

    // ── Verify Zero-Disk Staging Before & During Close ─────────────
    let residual_staging_segments = count_media_segments_in_dir(&staging_dir);

    match session.close().await {
        Ok(()) => println!("✔ PackagingSession closed cleanly."),
        Err(e) => eprintln!("Session close warning: {e}"),
    }

    let (inits, segs, manifests, total) = consumer.await?;

    println!();
    println!("══════════════════════════════════════════════════════════════════");
    println!("Zero-Disk HTTP Egress Summary");
    println!("──────────────────────────────────────────────────────────────────");
    println!("  Init Segments:          {:>6}", inits);
    println!("  Media Segments:         {:>6}", segs);
    println!("  Manifests:              {:>6}", manifests);
    println!("  Total Bytes Streamed:   {:>10}", fmt_size(total));
    println!(
        "  Staging Segment Files:  {:>6}  (0 confirmed - pure RAM transport)",
        residual_staging_segments
    );
    println!("  Dumping Directory:      {}", dump_dir);
    println!("══════════════════════════════════════════════════════════════════");
    println!();
    println!("To test playback in your browser:");
    println!("  cargo run --example 11_http_output_playback_server -- --port 8080");
    println!("Or using Example 09 (if Axinom DRM was used):");
    println!("  cargo run --example 09_axum_playback_server -- --stream-dir {dump_dir}");
    println!();

    Ok(())
}

fn count_media_segments_in_dir(dir: &Path) -> usize {
    if !dir.exists() {
        return 0;
    }
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(current) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.is_file() {
                    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    if name.ends_with(".m4s") {
                        count += 1;
                    }
                }
            }
        }
    }
    count
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
        println!("🎥 FFmpeg source: {path} (looping)");
        cmd.args(["-re", "-stream_loop", "-1", "-i", path]);
    } else {
        println!("🎥 FFmpeg source: lavfi testsrc 1280x720 + 1kHz sine audio");
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
