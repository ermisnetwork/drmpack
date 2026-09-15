//! End-to-end integration tests for `EgressMode::HttpPush` in `PackagingSession`.
//!
//! Validates that GPAC pushes manifests and media segments directly to the in-process
//! `HttpEgressServer` loopback sink in memory without any filesystem staging or segment files
//! written to disk.

use bytes::Bytes;
use drmpack::key::{ContentKey, KeyID, PsshData, StaticKeySource};
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::{
    ArtifactKind, DrmSystem, EgressMode, EncryptionScheme, PackagedArtifact, QualityTier,
    Rendition, TrackType,
};
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

mod common;
use common::find_box;

const WIDEVINE_PSSH_PAYLOAD: &[u8] = b"widevine-pssh-test-payload";

fn media_tools_available() -> bool {
    ["gpac", "ffmpeg"].into_iter().all(|binary| {
        std::process::Command::new("which")
            .arg(binary)
            .output()
            .is_ok_and(|output| output.status.success())
    })
}

fn require_media_tools() {
    if media_tools_available() {
        return;
    }
    if std::env::var_os("DRMPACK_REQUIRE_MEDIA_TOOLS").is_some() {
        panic!("GPAC and FFmpeg are required when DRMPACK_REQUIRE_MEDIA_TOOLS is set");
    }
    println!("SKIPPING test: 'gpac' and 'ffmpeg' are required");
}

fn create_dual_test_key_provider() -> (StaticKeySource, KeyID, KeyID) {
    let cenc_kid = KeyID::new(Uuid::from_bytes([
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10,
    ]));
    let cbcs_kid = KeyID::new(Uuid::from_bytes([
        0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
        0x20,
    ]));

    let cenc_key = ContentKey::new_with_scheme(
        cenc_kid,
        [0x42; 16],
        QualityTier::hd(),
        TrackType::Video,
        EncryptionScheme::Cenc,
    );
    let cbcs_key = ContentKey::new_with_scheme(
        cbcs_kid,
        [0x43; 16],
        QualityTier::hd(),
        TrackType::Video,
        EncryptionScheme::Cbcs,
    );

    let pssh = PsshData::new(
        DrmSystem::Widevine,
        DrmSystem::Widevine.system_id(),
        Bytes::from_static(WIDEVINE_PSSH_PAYLOAD),
    );

    let provider = StaticKeySource::new()
        .with_key(cenc_key)
        .with_key(cbcs_key)
        .with_pssh(pssh);

    (provider, cenc_kid, cbcs_kid)
}

async fn generate_sample_mp4(prefix: &str, duration_secs: u32) -> Vec<u8> {
    let sample_mp4_path = std::env::temp_dir().join(format!("{prefix}_{}.mp4", Uuid::new_v4()));
    let ffmpeg_status = std::process::Command::new("ffmpeg")
        .args([
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc=duration={duration_secs}:size=640x360:rate=30"),
            "-c:v",
            "libx264",
            "-profile:v",
            "baseline",
            "-pix_fmt",
            "yuv420p",
            "-g",
            "30",
            "-keyint_min",
            "30",
            "-sc_threshold",
            "0",
            "-movflags",
            "empty_moov+default_base_moof+frag_keyframe",
            "-f",
            "mp4",
            sample_mp4_path.to_str().unwrap(),
            "-y",
        ])
        .output()
        .expect("Failed to run ffmpeg to generate test fMP4");
    assert!(ffmpeg_status.status.success(), "ffmpeg generation failed");

    let sample_bytes = tokio::fs::read(&sample_mp4_path).await.unwrap();
    let _ = tokio::fs::remove_file(&sample_mp4_path).await;
    sample_bytes
}

fn assert_no_media_segment_files(dir: &Path) {
    if !dir.exists() {
        return;
    }
    let mut dirs_to_visit = vec![dir.to_path_buf()];
    while let Some(current) = dirs_to_visit.pop() {
        if let Ok(entries) = std::fs::read_dir(&current) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs_to_visit.push(path);
                } else if path.is_file() {
                    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    assert!(
                        !file_name.ends_with(".m4s"),
                        "No .m4s segment files should be written to disk in HttpPush mode, found: {}",
                        path.display()
                    );
                    assert!(
                        !file_name.ends_with(".mp4"),
                        "No .mp4 files should be written to disk in HttpPush mode, found: {}",
                        path.display()
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn test_http_egress_live_cenc_e2e() {
    if !media_tools_available() {
        require_media_tools();
        return;
    }

    let key_bytes = [0x42; 16];
    let key_provider = StaticKeySource::shared_key(key_bytes);
    let expected_kid = KeyID::new(Uuid::from_bytes(key_bytes));

    let rendition = Rendition::video_hd();
    let out_dir = std::env::temp_dir().join(format!("drmpack_http_cenc_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::cenc("e2e-http-cenc-stream")
        .with_rendition(rendition)
        .with_egress_mode(EgressMode::HttpPush)
        .with_segment_duration(2.0)
        .with_chunk_duration(0.2)
        .with_time_shift_buffer(Duration::from_secs(60))
        .with_finalization_timeout(Duration::from_secs(30))
        .with_output_dir(&out_dir);

    let mut session = PackagingSession::create(config, &key_provider)
        .await
        .expect("Failed to create PackagingSession");

    // Single-ownership assertion: take_output_receiver returns Some once, None thereafter
    let mut rx = session
        .take_output_receiver()
        .expect("First call to take_output_receiver must return Some(Receiver)");
    assert!(
        session.take_output_receiver().is_none(),
        "Subsequent call to take_output_receiver must return None"
    );

    // Spawn consumer task collecting PackagedArtifact items from rx
    let consumer_handle = tokio::spawn(async move {
        let mut artifacts: Vec<PackagedArtifact> = Vec::new();
        while let Some(artifact) = rx.recv().await {
            artifacts.push(artifact);
        }
        artifacts
    });

    // Ingest sample media chunks into the session
    let sample_bytes = generate_sample_mp4("http_cenc", 4).await;
    session
        .push(sample_bytes)
        .await
        .expect("Failed to push fMP4 into session");

    // Allow GPAC filter pipeline to ingest and begin processing before signaling EOF
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Gracefully close session: signals EOF, awaits GPAC finalization, shuts down HttpEgressServer
    session
        .close()
        .await
        .expect("Failed to close session cleanly");

    // Await consumer completion (channel closed when HttpEgressServer shuts down)
    let artifacts = consumer_handle
        .await
        .expect("Consumer task panicked or failed");

    assert!(
        !artifacts.is_empty(),
        "HttpPush egress channel must emit packaged artifacts"
    );

    let mut init_segments = Vec::new();
    let mut media_segments = Vec::new();
    let mut manifests = Vec::new();

    for artifact in artifacts {
        assert_eq!(
            artifact.scheme,
            EncryptionScheme::Cenc,
            "Artifact scheme must match configured CENC scheme"
        );
        match artifact.kind {
            ArtifactKind::InitSegment => init_segments.push(artifact),
            ArtifactKind::MediaSegment => media_segments.push(artifact),
            ArtifactKind::Manifest => manifests.push(artifact),
        }
    }

    // 1. Verify InitSegment payload and box integrity
    assert!(
        !init_segments.is_empty(),
        "At least one InitSegment must be emitted over HTTP egress"
    );
    let init = &init_segments[0];
    assert!(
        init.filename.contains("init.mp4"),
        "Init segment filename must contain init.mp4, got {}",
        init.filename
    );
    assert!(
        find_box(&init.data, b"ftyp").is_some(),
        "Init segment must contain ftyp box"
    );
    assert!(
        find_box(&init.data, b"moov").is_some(),
        "Init segment must contain moov box"
    );
    let tenc = find_box(&init.data, b"tenc").expect("Init segment must contain a tenc box");
    assert!(
        tenc.windows(16)
            .any(|bytes| bytes == expected_kid.as_bytes()),
        "tenc box must carry the StaticKeySource KID"
    );

    // 2. Verify MediaSegment payload and box integrity
    assert!(
        !media_segments.is_empty(),
        "At least one MediaSegment must be emitted over HTTP egress"
    );
    let seg = &media_segments[0];
    assert!(
        seg.filename.ends_with(".m4s"),
        "Media segment filename must end with .m4s, got {}",
        seg.filename
    );
    assert!(
        find_box(&seg.data, b"moof").is_some(),
        "Media segment must contain moof box"
    );
    assert!(
        find_box(&seg.data, b"mdat").is_some(),
        "Media segment must contain mdat box"
    );
    for box_type in [b"senc", b"saiz", b"saio"] {
        assert!(
            find_box(&seg.data, box_type).is_some(),
            "Encrypted media segment must contain {}",
            String::from_utf8_lossy(box_type)
        );
    }

    // 3. Verify Manifest payloads
    assert!(
        !manifests.is_empty(),
        "Manifests must be emitted over HTTP egress"
    );
    let has_mpd = manifests
        .iter()
        .any(|m| m.filename.ends_with(".mpd") && m.data.windows(6).any(|w| w == b"</MPD>"));
    assert!(
        has_mpd,
        "DASH MPD manifest must be emitted with valid closing tag"
    );

    let has_m3u8 = manifests
        .iter()
        .any(|m| m.filename.ends_with(".m3u8") && m.data.starts_with(b"#EXTM3U"));
    assert!(
        has_m3u8,
        "HLS manifest must be emitted starting with #EXTM3U"
    );

    // 4. Verify ZERO segment files on disk in HttpPush mode
    assert_no_media_segment_files(&out_dir);

    // Clean up temp output directory
    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_http_egress_dual_scheme_e2e() {
    if !media_tools_available() {
        require_media_tools();
        return;
    }

    let (key_provider, cenc_kid, cbcs_kid) = create_dual_test_key_provider();
    let rendition = Rendition::video_hd();
    let out_dir = std::env::temp_dir().join(format!("drmpack_http_dual_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::dual("e2e-http-dual-stream")
        .with_rendition(rendition)
        .with_egress_mode(EgressMode::HttpPush)
        .with_segment_duration(2.0)
        .with_chunk_duration(0.2)
        .with_time_shift_buffer(Duration::from_secs(60))
        .with_finalization_timeout(Duration::from_secs(30))
        .with_output_dir(&out_dir);

    let mut session = PackagingSession::create(config, &key_provider)
        .await
        .expect("Failed to create dual PackagingSession");

    let mut rx = session
        .take_output_receiver()
        .expect("Failed to claim output receiver");

    // Spawn consumer task collecting PackagedArtifact items
    let consumer_handle = tokio::spawn(async move {
        let mut artifacts: Vec<PackagedArtifact> = Vec::new();
        while let Some(artifact) = rx.recv().await {
            artifacts.push(artifact);
        }
        artifacts
    });

    // Ingest sample media
    let sample_bytes = generate_sample_mp4("http_dual", 4).await;
    session
        .push(sample_bytes)
        .await
        .expect("Failed to push fMP4 into dual session");

    // Allow GPAC filter pipeline to ingest and begin processing before signaling EOF
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Gracefully close dual session
    session
        .close()
        .await
        .expect("Failed to close dual session cleanly");

    let artifacts = consumer_handle
        .await
        .expect("Consumer task panicked or failed");

    assert!(
        !artifacts.is_empty(),
        "Dual HTTP egress must emit packaged artifacts"
    );

    let cenc_artifacts: Vec<_> = artifacts
        .iter()
        .filter(|a| a.scheme == EncryptionScheme::Cenc)
        .collect();
    let cbcs_artifacts: Vec<_> = artifacts
        .iter()
        .filter(|a| a.scheme == EncryptionScheme::Cbcs)
        .collect();

    assert!(
        !cenc_artifacts.is_empty(),
        "Must receive CENC artifacts in dual HttpPush mode"
    );
    assert!(
        !cbcs_artifacts.is_empty(),
        "Must receive CBCS artifacts in dual HttpPush mode"
    );

    // Verify CENC artifacts in RAM
    let cenc_inits: Vec<_> = cenc_artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::InitSegment)
        .collect();
    let cenc_segs: Vec<_> = cenc_artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::MediaSegment)
        .collect();
    let cenc_manifests: Vec<_> = cenc_artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::Manifest)
        .collect();

    assert!(!cenc_inits.is_empty(), "CENC must have InitSegment in RAM");
    assert!(!cenc_segs.is_empty(), "CENC must have MediaSegment in RAM");
    assert!(!cenc_manifests.is_empty(), "CENC must have Manifest in RAM");

    let cenc_tenc = find_box(&cenc_inits[0].data, b"tenc").expect("CENC init has tenc box");
    assert!(
        cenc_tenc
            .windows(16)
            .any(|bytes| bytes == cenc_kid.as_bytes()),
        "CENC tenc must carry CENC KID"
    );
    assert!(
        find_box(&cenc_segs[0].data, b"moof").is_some(),
        "CENC media segment must contain moof box"
    );
    assert!(
        find_box(&cenc_segs[0].data, b"mdat").is_some(),
        "CENC media segment must contain mdat box"
    );

    // Verify CBCS artifacts in RAM
    let cbcs_inits: Vec<_> = cbcs_artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::InitSegment)
        .collect();
    let cbcs_segs: Vec<_> = cbcs_artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::MediaSegment)
        .collect();
    let cbcs_manifests: Vec<_> = cbcs_artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::Manifest)
        .collect();

    assert!(!cbcs_inits.is_empty(), "CBCS must have InitSegment in RAM");
    assert!(!cbcs_segs.is_empty(), "CBCS must have MediaSegment in RAM");
    assert!(!cbcs_manifests.is_empty(), "CBCS must have Manifest in RAM");

    let cbcs_tenc = find_box(&cbcs_inits[0].data, b"tenc").expect("CBCS init has tenc box");
    assert!(
        cbcs_tenc
            .windows(16)
            .any(|bytes| bytes == cbcs_kid.as_bytes()),
        "CBCS tenc must carry CBCS KID"
    );
    assert!(
        find_box(&cbcs_segs[0].data, b"moof").is_some(),
        "CBCS media segment must contain moof box"
    );
    assert!(
        find_box(&cbcs_segs[0].data, b"mdat").is_some(),
        "CBCS media segment must contain mdat box"
    );

    // Verify ZERO segment files on disk in dual HttpPush mode
    assert_no_media_segment_files(&out_dir);

    // Clean up temp output directory
    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_http_egress_unclaimed_receiver_closed_cleanly() {
    if !media_tools_available() {
        require_media_tools();
        return;
    }

    let key_provider = StaticKeySource::shared_key([0x33; 16]);
    let out_dir = std::env::temp_dir().join(format!("drmpack_http_unclaimed_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::cenc("e2e-http-unclaimed")
        .with_rendition(Rendition::video_hd())
        .with_egress_mode(EgressMode::HttpPush)
        .with_segment_duration(1.0)
        .with_chunk_duration(0.2)
        .with_time_shift_buffer(Duration::from_secs(60))
        .with_finalization_timeout(Duration::from_secs(30))
        .with_output_dir(&out_dir);

    let mut session = PackagingSession::create(config, &key_provider)
        .await
        .expect("Failed to create PackagingSession");

    let sample_bytes = generate_sample_mp4("unclaimed_http", 4).await;
    session
        .push(sample_bytes)
        .await
        .expect("Push must succeed even if receiver was unclaimed");

    // Allow GPAC filter pipeline to ingest and begin processing before signaling EOF
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Do NOT claim receiver, close session directly
    session
        .close()
        .await
        .expect("Unclaimed HttpPush session must close cleanly without deadlock");

    assert!(
        session.take_output_receiver().is_none(),
        "take_output_receiver after close must return None"
    );

    assert_no_media_segment_files(&out_dir);
    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_http_egress_receiver_dropped_early() {
    if !media_tools_available() {
        require_media_tools();
        return;
    }

    let key_provider = StaticKeySource::shared_key([0x44; 16]);
    let out_dir = std::env::temp_dir().join(format!("drmpack_http_dropped_rx_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::cenc("e2e-http-dropped-rx")
        .with_rendition(Rendition::video_hd())
        .with_egress_mode(EgressMode::HttpPush)
        .with_finalization_timeout(Duration::from_secs(30))
        .with_output_dir(&out_dir);

    let mut session = PackagingSession::create(config, &key_provider)
        .await
        .expect("Failed to create PackagingSession");

    // Claim receiver and immediately drop it (simulating consumer crash)
    let rx = session.take_output_receiver().expect("Must yield receiver");
    drop(rx);

    // Ingestion should not panic or hang even when receiver is dead
    let sample_bytes = generate_sample_mp4("dropped_rx_http", 4).await;
    let _ = session.push(sample_bytes).await;

    // Allow GPAC filter pipeline to ingest and begin processing before signaling EOF
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Teardown must succeed without deadlock
    let close_res = session.close().await;
    assert!(
        close_res.is_ok(),
        "Session close must succeed cleanly after early receiver drop: {close_res:?}"
    );

    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}
