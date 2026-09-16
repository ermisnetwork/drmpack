//! Standalone whole-file VOD batch packaging example.
//!
//! Demonstrates how to take an existing MP4 file and package it into DRM-protected
//! DASH and HLS assets using Single-File Byte-Range mode (`profile=onDemand`).

use drmpack::key::{ContentKey, StaticKeySource};
use drmpack::types::{DrmSystem, EncryptionScheme, QualityTier, Rendition, TrackType};
use drmpack::vod::{package_vod_file, VodInputSource, VodMode, VodPackageConfig};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize console tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,drmpack=debug".into()),
        )
        .init();

    println!("=== drmpack 12_vod_batch_packaging ===");

    let sample_path = PathBuf::from("scratch/sample.mp4");
    if !sample_path.exists() {
        println!(
            "Generating 4s synthetic test media at {}",
            sample_path.display()
        );
        std::fs::create_dir_all("scratch")?;
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=4:size=1280x720:rate=30",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=1000:duration=4:sample_rate=48000",
                "-c:v",
                "libx264",
                "-g",
                "60",
                "-keyint_min",
                "60",
                "-sc_threshold",
                "0",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                "-f",
                "mp4",
                sample_path.to_str().unwrap(),
            ])
            .output()?;
        if !status.status.success() {
            eprintln!("FFmpeg failed: {}", String::from_utf8_lossy(&status.stderr));
            return Ok(());
        }
    }

    let output_dir = PathBuf::from("scratch/vod_output");
    if output_dir.exists() {
        let _ = std::fs::remove_dir_all(&output_dir);
    }

    // Set up static test keys (ClearKey / test credentials)
    let video_kid = Uuid::new_v4();
    let video_key = ContentKey::new(video_kid, [0xaa; 16], QualityTier::hd(), TrackType::Video);
    let key_provider = Arc::new(StaticKeySource::new().with_key(video_key));

    // Configure VOD packaging for SingleFile onDemand mode
    let config = VodPackageConfig::new(
        "movie_demo_01",
        VodInputSource::SingleFile(sample_path),
        &output_dir,
    )
    .with_vod_mode(VodMode::SingleFile)
    .with_encryption_scheme(EncryptionScheme::Cbcs)
    .with_drm_system(DrmSystem::FairPlay)
    .with_rendition(Rendition::video_hd().with_container_track_id(1))
    .with_rendition(Rendition::audio().with_container_track_id(2).clear());

    println!("Starting VOD batch packaging (SingleFile onDemand mode)...");
    let result = package_vod_file(&config, &key_provider).await?;

    println!("\n✅ Packaging complete!");
    println!("DASH MPD: {}", result.mpd_manifest.display());
    if let Some(ref m3u8) = result.master_playlist {
        println!("HLS Master: {}", m3u8.display());
    }
    println!("Variant Playlists: {:?}", result.variant_playlists);
    println!("Media Files: {:?}", result.media_files);
    println!(
        "DRM Metadata: Content ID={}, Scheme={}",
        result.metadata.content_id, result.metadata.scheme
    );

    Ok(())
}
