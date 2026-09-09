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

use drmpack::key::StaticKeySource;
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::{ArtifactKind, Rendition};
use std::process::Stdio;
use std::time::{Duration, Instant};

// ── Tunable defaults ───────────────────────────────────────────────
const DEFAULT_DURATION_SECS: u64 = 30;
const DEFAULT_DUMP_DIR: &str = "scratch/example08_live";
const SEGMENT_DURATION: f64 = 2.0;
const CONTENT_KEY: [u8; 16] = [0x55; 16];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let duration_secs: u64 = parse_arg(&args, "--duration").unwrap_or(DEFAULT_DURATION_SECS);
    let dump_dir =
        parse_str_arg(&args, "--dump-dir").unwrap_or_else(|| DEFAULT_DUMP_DIR.to_string());
    let input = parse_str_arg(&args, "--input");

    println!("drmpack Example 08: Live Packaging Pipeline");
    println!("────────────────────────────────────────────");
    println!("Duration: {duration_secs}s | Dump: {dump_dir}\n");

    let dump_path = std::path::PathBuf::from(&dump_dir);
    let _ = tokio::fs::remove_dir_all(&dump_path).await;
    tokio::fs::create_dir_all(&dump_path).await?;

    // ── Session ────────────────────────────────────────────────────
    let key_provider = StaticKeySource::shared_key(CONTENT_KEY);
    let config = PackagingSessionConfig::cbcs("example08-live")
        .with_rendition(Rendition::video_hd())
        .with_rendition(Rendition::audio())
        .with_segment_duration(SEGMENT_DURATION);

    let mut session = PackagingSession::create(config, &key_provider).await?;

    // ── Consumer: log artifacts + dump to disk ─────────────────────
    let mut rx = session
        .take_output_receiver()
        .expect("output channel already claimed");
    let consumer_dir = dump_path.clone();
    let start = Instant::now();

    let consumer = tokio::spawn(async move {
        let (mut inits, mut segs, mut manifests, mut total) = (0u32, 0u32, 0u32, 0usize);

        while let Some(art) = rx.recv().await {
            let e = start.elapsed().as_secs();
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
            println!(
                "[{:02}:{:02}] {tag} {:<32} {}",
                e / 60,
                e % 60,
                art.filename,
                fmt_size(size)
            );
            let _ = tokio::fs::write(consumer_dir.join(&art.filename), &art.data).await;
        }

        (inits, segs, manifests, total)
    });

    // ── FFmpeg live source ──────────────────────────────────────────
    let mut ffmpeg = spawn_ffmpeg(input.as_deref())?;
    let mut stdout = ffmpeg.stdout.take().expect("ffmpeg stdout");

    // ── Feed via SessionWriter ─────────────────────────────────────
    let mut writer = session.writer();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(duration_secs);

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("\nCtrl+C. Finalizing...");
        }
        _ = tokio::time::sleep_until(deadline) => {
            println!("\nDuration reached. Finalizing...");
        }
        res = tokio::io::copy(&mut stdout, &mut writer) => {
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

// ── Helpers ────────────────────────────────────────────────────────

fn spawn_ffmpeg(input: Option<&str>) -> Result<tokio::process::Child, Box<dyn std::error::Error>> {
    let mut cmd = tokio::process::Command::new("ffmpeg");

    if let Some(path) = input {
        println!("FFmpeg source: {path} (loop)");
        cmd.args(["-re", "-stream_loop", "-1", "-i", path]);
    } else {
        println!("FFmpeg source: lavfi testsrc 1280x720 + sine");
        cmd.args(["-re", "-f", "lavfi", "-i", "testsrc=size=1280x720:rate=30"]);
        cmd.args(["-f", "lavfi", "-i", "sine=frequency=1000:sample_rate=48000"]);
    }

    cmd.args([
        "-c:v",
        "libx264",
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
