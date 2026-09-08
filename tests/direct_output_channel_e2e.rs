use bytes::Bytes;
use drmpack::key::{ContentKey, KeyID, PsshData, RawKeyProvider};
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::{
    ArtifactKind, DrmSystem, EncryptionScheme, LatencyMode, PackagedArtifact, QualityTier,
    Rendition, TrackType,
};
use uuid::Uuid;

mod common;
use common::find_box;

const WIDEVINE_PSSH_PAYLOAD: &[u8] = b"widevine-pssh-test-payload";

fn create_test_key_provider() -> (RawKeyProvider, KeyID) {
    let kid = KeyID::new(Uuid::from_bytes([
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10,
    ]));
    let key_bytes = [0x42; 16];
    let content_key = ContentKey::new(kid, key_bytes, QualityTier::hd(), TrackType::Video);

    let pssh = PsshData::new(
        DrmSystem::Widevine,
        DrmSystem::Widevine.system_id(),
        Bytes::from_static(WIDEVINE_PSSH_PAYLOAD),
    );

    let provider = RawKeyProvider::new().with_key(content_key).with_pssh(pssh);

    (provider, kid)
}

fn create_dual_test_key_provider() -> (RawKeyProvider, KeyID, KeyID) {
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

    let provider = RawKeyProvider::new()
        .with_key(cenc_key)
        .with_key(cbcs_key)
        .with_pssh(pssh);

    (provider, cenc_kid, cbcs_kid)
}

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

#[tokio::test]
async fn test_direct_output_channel_live_cenc_e2e() {
    if !media_tools_available() {
        require_media_tools();
        return;
    }

    let (provider, kid) = create_test_key_provider();
    let rendition = Rendition::video_hd();
    let out_dir = std::env::temp_dir().join(format!("drmpack_doc_cenc_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::new("e2e-doc-cenc-stream")
        .with_rendition(rendition)
        .with_encryption_scheme(EncryptionScheme::Cenc)
        .with_drm_system(DrmSystem::Widevine)
        .with_latency_mode(LatencyMode::LowLatency)
        .with_segment_duration(2.0)
        .with_chunk_duration(0.2)
        .with_time_shift_buffer(std::time::Duration::from_secs(60))
        .with_output_dir(&out_dir);

    let mut session = PackagingSession::create(config, &provider)
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

    // Spawn consumer task consuming from the output channel
    let consumer_handle = tokio::spawn(async move {
        let mut artifacts: Vec<PackagedArtifact> = Vec::new();
        while let Some(artifact) = rx.recv().await {
            artifacts.push(artifact);
        }
        artifacts
    });

    // Ingest sample media chunks into the session
    let sample_bytes = generate_sample_mp4("cenc_doc", 4).await;
    session
        .push(sample_bytes)
        .await
        .expect("Failed to push fMP4 into GPAC pipe");

    // Close session gracefully: signals EOF, awaits GPAC finalization, flushes harvester
    session
        .close()
        .await
        .expect("Failed to close session cleanly");

    // Await consumer completion (channel closed when harvester finishes)
    let artifacts = consumer_handle
        .await
        .expect("Consumer task panicked or failed");

    // Verify artifacts were emitted over the channel
    assert!(
        !artifacts.is_empty(),
        "Direct output channel must emit packaged artifacts"
    );

    let mut init_segments = Vec::new();
    let mut media_segments = Vec::new();
    let mut manifests = Vec::new();

    for artifact in artifacts {
        assert_eq!(
            artifact.scheme,
            EncryptionScheme::Cenc,
            "Artifact must identify its concrete scheme"
        );
        match artifact.kind {
            ArtifactKind::InitSegment => init_segments.push(artifact),
            ArtifactKind::MediaSegment => media_segments.push(artifact),
            ArtifactKind::Manifest => manifests.push(artifact),
        }
    }

    // 1. Verify InitSegment payload
    assert!(
        !init_segments.is_empty(),
        "At least one InitSegment must be emitted"
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
        tenc.windows(16).any(|bytes| bytes == kid.as_bytes()),
        "tenc must carry the RawKeyProvider KID"
    );

    // 2. Verify MediaSegment payload
    assert!(
        !media_segments.is_empty(),
        "At least one MediaSegment must be emitted"
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
    assert!(!manifests.is_empty(), "Manifests must be emitted");
    let has_mpd = manifests
        .iter()
        .any(|m| m.filename.ends_with(".mpd") && m.data.windows(6).any(|w| w == b"</MPD>"));
    assert!(
        has_mpd,
        "DASH MPD manifest must be emitted with valid closing tag"
    );

    let has_m3u8 = manifests.iter().any(|m| {
        m.filename.ends_with(".m3u8")
            && m.data.starts_with(b"#EXTM3U")
            && String::from_utf8_lossy(&m.data).contains("#EXT-X-ENDLIST")
    });
    assert!(has_m3u8, "HLS manifest must be emitted with #EXT-X-ENDLIST");

    // 4. Verify Ephemeral Staging: ZERO residual files in the staging directory!
    if out_dir.exists() {
        let mut entries = tokio::fs::read_dir(&out_dir).await.unwrap();
        let mut residual_files = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            residual_files.push(entry.file_name().to_string_lossy().to_string());
        }
        assert!(
            residual_files.is_empty(),
            "Ephemeral staging must leave zero residual staging files; found: {:?}",
            residual_files
        );
    }

    // 5. Clean session teardown
    drop(session);
    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_direct_output_channel_dual_scheme_e2e() {
    if !media_tools_available() {
        require_media_tools();
        return;
    }

    let (provider, _cenc_kid, _cbcs_kid) = create_dual_test_key_provider();
    let rendition = Rendition::video_hd();
    let out_dir = std::env::temp_dir().join(format!("drmpack_doc_dual_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::new("e2e-doc-dual-stream")
        .with_rendition(rendition)
        .with_encryption_scheme(EncryptionScheme::Dual)
        .with_all_drm()
        .with_latency_mode(LatencyMode::LowLatency)
        .with_segment_duration(2.0)
        .with_chunk_duration(0.2)
        .with_time_shift_buffer(std::time::Duration::from_secs(60))
        .with_output_dir(&out_dir);

    let mut session = PackagingSession::create(config, &provider)
        .await
        .expect("Failed to create PackagingSession");

    let mut rx = session
        .take_output_receiver()
        .expect("First call to take_output_receiver must return Some(Receiver)");

    let consumer_handle = tokio::spawn(async move {
        let mut artifacts: Vec<PackagedArtifact> = Vec::new();
        while let Some(artifact) = rx.recv().await {
            artifacts.push(artifact);
        }
        artifacts
    });

    let sample_bytes = generate_sample_mp4("dual_doc", 4).await;
    session
        .push(sample_bytes)
        .await
        .expect("Failed to push fMP4 into GPAC pipe");

    session
        .close()
        .await
        .expect("Failed to close session cleanly");

    let artifacts = consumer_handle
        .await
        .expect("Consumer task panicked or failed");

    assert!(
        !artifacts.is_empty(),
        "Dual session must emit artifacts over channel"
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
        "Dual session must emit CENC artifacts"
    );
    assert!(
        !cbcs_artifacts.is_empty(),
        "Dual session must emit CBCS artifacts"
    );

    // Verify ephemeral staging in dual subdirectories
    for scheme_subdir in ["cenc", "cbcs"] {
        let sub = out_dir.join(scheme_subdir);
        if sub.exists() {
            let mut entries = tokio::fs::read_dir(&sub).await.unwrap();
            let mut residual = Vec::new();
            while let Some(entry) = entries.next_entry().await.unwrap() {
                residual.push(entry.file_name().to_string_lossy().to_string());
            }
            assert!(
                residual.is_empty(),
                "Dual scheme subdirectory '{}' must have zero residual files; found: {:?}",
                scheme_subdir,
                residual
            );
        }
    }

    drop(session);
    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_direct_output_channel_receiver_dropped_early() {
    if !media_tools_available() {
        require_media_tools();
        return;
    }

    let (provider, _) = create_test_key_provider();
    let rendition = Rendition::video_hd();
    let out_dir = std::env::temp_dir().join(format!("drmpack_doc_dropped_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::new("e2e-doc-dropped-stream")
        .with_rendition(rendition)
        .with_encryption_scheme(EncryptionScheme::Cenc)
        .with_output_dir(&out_dir);

    let mut session = PackagingSession::create(config, &provider)
        .await
        .expect("Failed to create PackagingSession");

    let rx = session.take_output_receiver().expect("claim receiver");
    // Receiver dropped immediately
    drop(rx);

    let sample_bytes = generate_sample_mp4("drop_doc", 2).await;
    // Pushing data must not fail even when receiver was dropped
    session
        .push(sample_bytes)
        .await
        .expect("Push should succeed even if receiver dropped");

    session.close().await.expect("Close should succeed cleanly");

    drop(session);
    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_direct_output_channel_unclaimed_receiver_preserves_files() {
    if !media_tools_available() {
        require_media_tools();
        return;
    }

    let (provider, _) = create_test_key_provider();
    let rendition = Rendition::video_hd();
    let out_dir = std::env::temp_dir().join(format!("drmpack_doc_unclaimed_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::new("e2e-doc-unclaimed-stream")
        .with_rendition(rendition)
        .with_encryption_scheme(EncryptionScheme::Cenc)
        .with_output_dir(&out_dir);

    let mut session = PackagingSession::create(config, &provider)
        .await
        .expect("Failed to create PackagingSession");

    // Do NOT call take_output_receiver() - legacy mode
    let sample_bytes = generate_sample_mp4("unclaimed_doc", 2).await;
    session
        .push(sample_bytes)
        .await
        .expect("Push should succeed");

    session.close().await.expect("Close should succeed");

    // In legacy unclaimed mode, files are preserved on disk
    let mpd = out_dir.join("live.mpd");
    assert!(mpd.exists(), "live.mpd must exist on disk in legacy mode");

    session.cleanup().await.expect("Cleanup should succeed");
    assert!(!out_dir.exists(), "Output directory cleaned up");
}

#[tokio::test]
async fn test_direct_output_channel_ephemeral_staging_during_active_push() {
    if !media_tools_available() {
        require_media_tools();
        return;
    }

    let (provider, _) = create_test_key_provider();
    let rendition = Rendition::video_hd();
    let out_dir = std::env::temp_dir().join(format!("drmpack_doc_ephemeral_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::new("e2e-doc-ephemeral-active")
        .with_rendition(rendition)
        .with_encryption_scheme(EncryptionScheme::Cenc)
        .with_latency_mode(LatencyMode::LowLatency)
        .with_segment_duration(1.0)
        .with_chunk_duration(0.2)
        .with_time_shift_buffer(std::time::Duration::from_secs(30))
        .with_output_dir(&out_dir);

    let mut session = PackagingSession::create(config, &provider)
        .await
        .expect("Failed to create PackagingSession");

    let mut rx = session
        .take_output_receiver()
        .expect("take_output_receiver must return Some");

    // Push first chunk
    let sample_bytes = generate_sample_mp4("active_ephemeral", 4).await;
    session.push(sample_bytes).await.expect("Push fMP4");

    // Wait until at least one artifact is emitted over rx during active push.
    // Init segments bypass manifest readiness (written atomically by GPAC),
    // so they're emitted as soon as they appear. Media segments require HLS
    // manifest readiness and may only appear after close() in dynauto mode.
    let mut received_artifact = false;
    let mut artifact_filename = String::new();
    let mut artifact_kind = ArtifactKind::Manifest;

    let timeout_result = tokio::time::timeout(std::time::Duration::from_secs(8), async {
        while let Some(artifact) = rx.recv().await {
            if artifact.kind == ArtifactKind::InitSegment || artifact.kind == ArtifactKind::MediaSegment {
                received_artifact = true;
                artifact_filename = artifact.filename;
                artifact_kind = artifact.kind;
                break;
            }
        }
    })
    .await;

    assert!(
        timeout_result.is_ok(),
        "Should receive at least one artifact (init or media segment) during active push"
    );
    assert!(received_artifact);

    // Verify Ephemeral Staging: The emitted segment file must ALREADY be unlinked from disk!
    let emitted_file_on_disk = out_dir.join(&artifact_filename);
    assert!(
        !emitted_file_on_disk.exists(),
        "Consumed segment file '{}' must be unlinked immediately upon channel emission!",
        emitted_file_on_disk.display()
    );

    // Now close session gracefully, which will finalize and emit media segments
    session.close().await.expect("Clean close");

    // Receive remaining artifacts after close
    let mut final_media_segments = Vec::new();
    while let Some(artifact) = rx.recv().await {
        if artifact.kind == ArtifactKind::MediaSegment {
            final_media_segments.push(artifact);
        }
    }
    assert!(
        !final_media_segments.is_empty(),
        "Must receive media segments after finalization"
    );

    drop(session);
    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_direct_output_channel_zero_duration_tsb_rejected() {
    let (provider, _) = create_test_key_provider();
    let config = PackagingSessionConfig::new("zero-tsb-test")
        .with_rendition(Rendition::video_hd())
        .with_time_shift_buffer(std::time::Duration::ZERO);

    let result = PackagingSession::create(config, &provider).await;
    assert!(result.is_err(), "Zero time_shift_buffer must be rejected");
    let err = result.err().unwrap();
    assert!(
        err.to_string().contains("time_shift_buffer"),
        "Error message must mention time_shift_buffer: {err}"
    );
}
