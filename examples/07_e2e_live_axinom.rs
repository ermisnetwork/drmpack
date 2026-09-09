mod common;

use common::cdn_publisher::{clean_dir, CdnPublisher};
use common::media_feeder::{resolve_or_create_input_media, MediaFeeder, MediaFeederConfig};
use common::playback_server::PlaybackServer;
use drmpack::axinom::{AxinomConfig, AxinomProvider, AxinomSigningConfig};
use drmpack::license::{AxinomLicenseConfig, LicenseProxy};
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
    let _ = dotenvy::dotenv();

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
    println!("drmpack E2E Live Packaging (Axinom SPEKE v2 / Widevine)");
    println!("====================================================");

    // Fail-fast validation of Axinom credentials
    let ax_config = AxinomConfig::from_env().map_err(|e| {
        eprintln!("FATAL: Axinom credentials missing in environment or .env: {e}");
        eprintln!("(Note: To test without Axinom credentials, use `cargo run --example 06_e2e_live_clearkey`).");
        e
    })?;
    let signing_config = AxinomSigningConfig::from_env().map_err(|e| {
        eprintln!("FATAL: Axinom communication credentials missing: {e}");
        eprintln!("(Note: To test without Axinom credentials, use `cargo run --example 06_e2e_live_clearkey`).");
        "Missing Axinom communication credentials"
    })?;

    let cdn_storage = PathBuf::from("scratch/cdn_storage");
    clean_dir(&cdn_storage).await?;

    // 1. Resolve or create input media
    let input_path = resolve_or_create_input_media(custom_input).await?;
    println!("Input Media Source: {}", input_path.display());

    let fairplay_flag = args.iter().any(|a| a == "--fairplay" || a == "--fps");
    let dual_flag = args.iter().any(|a| a == "--dual");
    let ll_flag = args.iter().any(|a| a == "--ll" || a == "--low-latency");
    let latency_mode = if ll_flag {
        LatencyMode::LowLatency
    } else {
        LatencyMode::Standard
    };

    let scheme_desc = if fairplay_flag {
        "Axinom FairPlay (CBCS / Apple Safari)"
    } else if dual_flag {
        "Axinom Dual (CENC + CBCS / Widevine + FairPlay)"
    } else {
        "Axinom Widevine (CENC / SPEKE v2)"
    };
    let latency_desc = match latency_mode {
        LatencyMode::LowLatency => "Low-Latency (CMAF 200ms chunks)",
        LatencyMode::Standard => "Standard (2.0s segments)",
    };
    println!("DRM Scheme Mode:  {scheme_desc}");
    println!("Latency Mode:     {latency_desc}");

    // 2. Configure Axinom packaging session
    let content_id = format!("e2e-live-axinom-{}", uuid::Uuid::new_v4());
    let session_config = if fairplay_flag {
        PackagingSessionConfig::new(&content_id)
            .with_encryption_scheme(drmpack::types::EncryptionScheme::Cbcs)
            .with_drm_system(drmpack::types::DrmSystem::FairPlay)
    } else if dual_flag {
        PackagingSessionConfig::low_latency_dual(&content_id)
    } else {
        PackagingSessionConfig::cenc(&content_id)
    };
    let session_config = session_config
        .with_rendition(Rendition::video_hd())
        .with_rendition(Rendition::audio())
        .with_segment_duration(2.0)
        .with_latency_mode(latency_mode);

    println!("Requesting live encryption keys from Axinom Key Service (SPEKE v2)...");
    let provider = AxinomProvider::new(ax_config);

    // Fail-fast if key acquisition fails
    let mut session = PackagingSession::create(session_config, &provider).await?;

    // Claim direct output channel (Single-Ownership)
    let rx = session
        .take_output_receiver()
        .expect("Failed to claim direct output receiver");

    let key_configs = session.key_set().to_axinom_key_configs();

    println!("Axinom PackagingSession initialized successfully.");
    println!("Ephemeral Staging: {}", session.output_dir().display());
    println!("CDN Storage:      {}", cdn_storage.display());
    println!("Active Key IDs:");
    for kc in &key_configs {
        if let Some(iv) = kc.iv {
            let iv_hex: String = iv.iter().map(|b| format!("{b:02x}")).collect();
            println!("  - {} (IV: 0x{iv_hex})", kc.kid);
        } else {
            println!("  - {}", kc.kid);
        }
    }

    let token = session
        .key_set()
        .generate_axinom_jwt_with_config(&signing_config)?;

    // 3. Initialize LicenseProxy for server-side DRM license and certificate handling
    let license_config = AxinomLicenseConfig::from_env().map_err(|e| {
        eprintln!("FATAL: Axinom license configuration missing: {e}");
        eprintln!("\nPlease ensure the following environment variables are set in your .env:");
        eprintln!("  AXINOM_WIDEVINE_LICENSE_URL=https://<tenant-id>.drm-widevine-licensing.axprod.net/AcquireLicense");
        eprintln!("  AXINOM_FAIRPLAY_LICENSE_URL=https://<tenant-id>.drm-fairplay-licensing.axprod.net/AcquireLicense");
        eprintln!("  AXINOM_PLAYREADY_LICENSE_URL=https://<tenant-id>.drm-playready-licensing.axprod.net/AcquireLicense");
        eprintln!("  AXINOM_FAIRPLAY_CERT_URL=https://<tenant-id>.drm-fairplay-licensing.axprod.net/v2/Certificate");
        e
    })?;
    let license_proxy = LicenseProxy::new(license_config);

    if fairplay_flag || dual_flag {
        if let Ok(path) = env::var("AXINOM_FAIRPLAY_CERT_PATH") {
            println!("Loading FairPlay Certificate from local file: {path}");
            let cert_bytes = tokio::fs::read(&path).await?;
            license_proxy
                .set_fairplay_certificate(bytes::Bytes::from(cert_bytes))
                .await;
        } else {
            println!(
                "Preloading FairPlay Application Certificate via LicenseProxy: {}",
                license_proxy.config().fairplay_cert_url
            );
            match license_proxy.preload_fairplay_certificate().await {
                Ok(cert) => println!(
                    "FairPlay Certificate preloaded into LicenseProxy: {} bytes",
                    cert.len()
                ),
                Err(err) => eprintln!("Warning: Failed to preload FairPlay certificate: {err}"),
            }
        }
    }

    // 4. Start CdnPublisher (Receiving media directly from session output channel)
    let cdn_handle = CdnPublisher::spawn_channel(cdn_storage.clone(), rx, dual_flag);
    println!("CdnPublisher started (consuming PackagedArtifact stream directly)...");

    // 5. Optionally spawn playback server
    let (shutdown_server_tx, shutdown_server_rx) = broadcast::channel(1);
    let server_handle = if serve_flag {
        let manifest_url = if dual_flag {
            format!("http://127.0.0.1:{port}/cenc/live.mpd")
        } else {
            format!("http://127.0.0.1:{port}/live.mpd")
        };
        let listener = TcpListener::bind(format!("127.0.0.1:{port}")).await?;
        println!("Embedded HTTP Server listening on http://127.0.0.1:{port}");
        println!("Player Web UI: http://127.0.0.1:{port}/");
        if dual_flag {
            println!("Dual CENC DASH Manifest: http://127.0.0.1:{port}/cenc/live.mpd");
            println!("Dual CBCS HLS Manifest:  http://127.0.0.1:{port}/cbcs/live.m3u8");
        }
        let scheme = if fairplay_flag {
            "fairplay"
        } else if dual_flag {
            "dual"
        } else {
            "widevine"
        };
        let kids_strings: Vec<String> = key_configs.iter().map(|k| k.kid.clone()).collect();
        let server = PlaybackServer::new(cdn_storage.clone(), manifest_url)
            .with_latency_mode(latency_mode)
            .with_drm_scheme(scheme)
            .with_kids(kids_strings)
            .with_license_proxy(license_proxy, token);

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

    println!("Axinom live packaging session closed cleanly.");
    println!("CDN Storage directory: {}", cdn_storage.display());
    Ok(())
}
