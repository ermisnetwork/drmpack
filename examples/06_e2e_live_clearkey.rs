mod common;

use common::cdn_publisher::{clean_dir, CdnPublisher};
use common::media_feeder::{resolve_or_create_input_media, MediaFeeder, MediaFeederConfig};
use common::playback_server::PlaybackServer;
use drmpack::key::StaticKeySource;
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::{LatencyMode, Rendition};
use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    let port = args
        .windows(2)
        .find(|w| w[0] == "--port")
        .and_then(|w| w[1].parse::<u16>().ok())
        .unwrap_or(8080);
    let max_duration = args
        .windows(2)
        .find(|w| w[0] == "--duration")
        .and_then(|w| w[1].parse::<u64>().ok())
        .map(Duration::from_secs);
    let max_chunks = args
        .windows(2)
        .find(|w| w[0] == "--max-chunks")
        .and_then(|w| w[1].parse::<u64>().ok());
    let custom_input = args
        .windows(2)
        .find(|w| w[0] == "--input")
        .map(|w| w[1].as_str());
    let serve_flag = !args.iter().any(|a| a == "--no-serve");
    let transcode_flag = args.iter().any(|a| a == "--transcode");
    let copy_flag = args.iter().any(|a| a == "--copy");

    println!("====================================================");
    println!("drmpack E2E Live Packaging (ClearKey / Offline)");
    println!("====================================================");

    let cdn_storage = PathBuf::from("scratch/cdn_storage");
    clean_dir(&cdn_storage).await?;

    // 1. Resolve or create input media
    let input_path = resolve_or_create_input_media(custom_input).await?;
    println!("Input Media Source: {}", input_path.display());

    let ll_flag = args.iter().any(|a| a == "--ll" || a == "--low-latency");
    let latency_mode = if ll_flag {
        LatencyMode::LowLatency
    } else {
        LatencyMode::Standard
    };
    let latency_desc = match latency_mode {
        LatencyMode::LowLatency => "Low-Latency (CMAF 200ms chunks)",
        LatencyMode::Standard => "Standard (2.0s segments)",
    };
    println!("Latency Mode:     {latency_desc}");

    // 2. Configure ClearKey packaging session
    let content_id = format!("e2e-clearkey-{}", uuid::Uuid::new_v4());
    let session_config = PackagingSessionConfig::cenc(&content_id)
        .with_rendition(Rendition::video_hd())
        .with_rendition(Rendition::audio())
        .with_segment_duration(2.0)
        .with_latency_mode(latency_mode);

    let key_provider = StaticKeySource::shared_key([0x11; 16]);
    let mut session = PackagingSession::create(session_config, &key_provider).await?;

    // Claim direct output channel (Single-Ownership)
    let rx = session
        .take_output_receiver()
        .expect("Failed to claim direct output receiver");

    let clear_kid_hex = "11111111111111111111111111111111";
    let clear_key_hex = "11111111111111111111111111111111";
    let kid_str = "11111111-1111-1111-1111-111111111111".to_string();

    println!("PackagingSession initialized with ClearKey DRM.");
    println!("Ephemeral Staging: {}", session.output_dir().display());
    println!("CDN Storage:      {}", cdn_storage.display());
    println!("Active Key ID:    {kid_str}");

    // 3. Start CdnPublisher (Receiving media directly from session output channel)
    let cdn_handle = CdnPublisher::spawn_channel(cdn_storage.clone(), rx, false);
    println!("CdnPublisher started (consuming PackagedArtifact stream directly)...");

    // 4. Optionally spawn playback server
    let (shutdown_server_tx, shutdown_server_rx) = broadcast::channel(1);
    let server_handle = if serve_flag {
        let manifest_url = format!("http://127.0.0.1:{port}/live.mpd");
        let listener = TcpListener::bind(format!("127.0.0.1:{port}")).await?;
        println!("Embedded HTTP Server listening on http://127.0.0.1:{port}");
        println!("Player Web UI: http://127.0.0.1:{port}/");
        let server = PlaybackServer::new(cdn_storage.clone(), manifest_url)
            .with_latency_mode(latency_mode)
            .with_drm_scheme("clearkey")
            .with_kids(vec![kid_str.clone()])
            .with_clearkey(clear_kid_hex, clear_key_hex);
        Some(tokio::spawn(server.run(listener, shutdown_server_rx)))
    } else {
        println!("----------------------------------------------------");
        println!("Playback server disabled via --no-serve.");
        println!("To view playback in your browser, in another terminal run:");
        println!("  cargo run --example playback_server -- --port {port}");
        println!("Then open: http://127.0.0.1:{port}/");
        println!("----------------------------------------------------");
        None
    };

    // 6. Feed media to packager
    let feeder_config = MediaFeederConfig::new(input_path)
        .with_duration(max_duration)
        .with_max_chunks(max_chunks)
        .with_force_transcode(transcode_flag)
        .with_force_copy(copy_flag);
    let feeder = MediaFeeder::new(feeder_config);

    let chunks_ingested = feeder.feed_to_session(&mut session).await?;
    println!("Ingestion finished ({chunks_ingested} chunks ingested).");

    // 7. Cleanup & shutdown
    if let Some(handle) = server_handle {
        println!("Stopping embedded HTTP server...");
        let _ = shutdown_server_tx.send(());
        let _ = handle.await;
    }

    println!("Closing GPAC packaging session...");
    session.close().await?;

    println!("Stopping CdnPublisher...");
    cdn_handle.stop().await;

    println!("ClearKey live packaging session closed cleanly.");
    println!("CDN Storage directory: {}", cdn_storage.display());
    Ok(())
}
