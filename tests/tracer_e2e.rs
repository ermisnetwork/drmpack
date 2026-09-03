use base64::prelude::*;
use bytes::Bytes;
use drmpack::error::DrmpackError;
use drmpack::key::{ContentKey, KeyID, PsshData, RawKeyProvider};
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::{
    DrmSystem, EncryptionScheme, LatencyMode, QualityTier, Rendition, Segment, TrackType,
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

    let pssh = PsshData {
        drm_system: DrmSystem::Widevine,
        system_id: DrmSystem::Widevine.system_id(),
        data: Bytes::from_static(WIDEVINE_PSSH_PAYLOAD),
    };

    let provider = RawKeyProvider::new().with_key(content_key).with_pssh(pssh);

    (provider, kid)
}

fn expected_widevine_pssh() -> Vec<u8> {
    let system_id = DrmSystem::Widevine.system_id();
    let size = 32u32 + WIDEVINE_PSSH_PAYLOAD.len() as u32;
    let mut pssh = Vec::with_capacity(size as usize);
    pssh.extend_from_slice(&size.to_be_bytes());
    pssh.extend_from_slice(b"pssh");
    pssh.extend_from_slice(&0u32.to_be_bytes());
    pssh.extend_from_slice(&system_id);
    pssh.extend_from_slice(&(WIDEVINE_PSSH_PAYLOAD.len() as u32).to_be_bytes());
    pssh.extend_from_slice(WIDEVINE_PSSH_PAYLOAD);
    pssh
}

#[tokio::test]
async fn test_tracer_session_detects_missing_gpac() {
    let (provider, _) = create_test_key_provider();
    let rendition = Rendition::video(
        "v1080p",
        QualityTier::hd(),
        1920,
        1080,
        5_000_000,
        "avc1.640028",
    );

    let out_dir = std::env::temp_dir().join(format!("drmpack_test_missing_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::new("test-missing-gpac")
        .with_rendition(rendition)
        .with_output_dir(&out_dir)
        .with_gpac_bin("non_existent_gpac_binary_xyz_123");

    let result = PackagingSession::create(config, provider).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, DrmpackError::Gpac(_)),
        "Expected DrmpackError::Gpac, got: {:?}",
        err
    );
    let err_msg = err.to_string();
    assert!(err_msg.contains("non_existent_gpac_binary_xyz_123"));

    // Cleanup
    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_tracer_gpac_e2e_live_packaging() {
    // Check if gpac is in PATH
    let gpac_check = std::process::Command::new("which").arg("gpac").output();

    let has_gpac = match gpac_check {
        Ok(output) => output.status.success(),
        Err(_) => false,
    };

    if !has_gpac {
        println!("SKIPPING test_tracer_gpac_e2e_live_packaging: 'gpac' binary not found in PATH");
        return;
    }

    let (provider, kid) = create_test_key_provider();
    let rendition = Rendition::video(
        "v720p",
        QualityTier::hd(),
        1280,
        720,
        2_500_000,
        "avc1.4d401f",
    );

    let out_dir = std::env::temp_dir().join(format!("drmpack_gpac_e2e_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::new("e2e-live-stream")
        .with_rendition(rendition)
        .with_latency_mode(LatencyMode::LowLatency)
        .with_segment_duration(1.0)
        .with_chunk_duration(0.2)
        .with_output_dir(&out_dir)
        .with_encryption_scheme(EncryptionScheme::Cenc)
        .with_drm_system(DrmSystem::Widevine);

    let mut session = PackagingSession::create(config, provider)
        .await
        .expect("Failed to create PackagingSession with real GPAC");

    // Verify drm.xml was generated in output directory
    let drm_xml_path = out_dir.join("drm.xml");
    assert!(drm_xml_path.exists());
    let drm_xml_content = tokio::fs::read_to_string(&drm_xml_path).await.unwrap();
    assert!(drm_xml_content.contains(r#"<GPACDRM type="CENC">"#));
    assert!(drm_xml_content.contains(&format!("0x{}", kid.to_hex())));

    // Generate a 1-second synthetic H.264 fMP4 clip via ffmpeg to push into the pipe
    let sample_mp4_path = std::env::temp_dir().join(format!("sample_{}.mp4", Uuid::new_v4()));
    let ffmpeg_status = std::process::Command::new("ffmpeg")
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=640x360:rate=30",
            "-c:v",
            "libx264",
            "-profile:v",
            "baseline",
            "-pix_fmt",
            "yuv420p",
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

    // Push a Segment through the public session API into the GPAC pipe.
    session
        .push_segment(Segment {
            rendition_id: "v720p".into(),
            sequence_number: 0,
            duration_seconds: 1.0,
            data: Bytes::from(sample_bytes),
            is_init: false,
        })
        .await
        .expect("Failed to push fMP4 Segment into GPAC pipe");

    // Close session gracefully, signaling EOF and waiting for finalization
    session
        .close()
        .await
        .expect("Failed to close session cleanly");

    // Verify generated artifacts in the output directory
    let live_mpd = out_dir.join("live.mpd");
    assert!(live_mpd.exists(), "live.mpd manifest must exist");
    let mpd_content = tokio::fs::read_to_string(&live_mpd).await.unwrap();
    assert!(mpd_content.contains("<ContentProtection"));
    assert!(mpd_content.contains("availabilityTimeOffset"));
    assert!(
        mpd_content.contains(&kid.0.hyphenated().to_string()),
        "DASH manifest must signal the RawKeyProvider KID; manifest was:\n{}",
        mpd_content
    );
    let expected_pssh = expected_widevine_pssh();
    let expected_pssh_base64 = BASE64_STANDARD.encode(&expected_pssh);
    assert!(
        mpd_content.contains(&format!("<cenc:pssh>{expected_pssh_base64}</cenc:pssh>")),
        "DASH manifest must signal the exact Widevine PSSH; manifest was:\n{}",
        mpd_content
    );

    let live_m3u8 = out_dir.join("live_1.m3u8");
    assert!(live_m3u8.exists(), "live_1.m3u8 media playlist must exist");
    let m3u8_content = tokio::fs::read_to_string(&live_m3u8).await.unwrap();
    assert!(m3u8_content.contains("#EXT-X-KEY:METHOD=SAMPLE-AES-CTR"));
    assert!(
        m3u8_content.contains(&format!(
            "URI=\"data:text/plain;base64,{expected_pssh_base64}\""
        )),
        "HLS Manifest must carry the configured inline PSSH URI; manifest was:\n{}",
        m3u8_content
    );
    assert!(
        m3u8_content.contains("KEYFORMAT=\"urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed\""),
        "HLS Manifest must identify the Widevine key format; manifest was:\n{}",
        m3u8_content
    );
    assert!(!m3u8_content.contains("locator:null"));
    assert!(m3u8_content.contains("#EXT-X-PART-INF:PART-TARGET=0.2"));
    assert!(m3u8_content.contains("#EXT-X-PART:DURATION=0.2"));

    let init_mp4 = out_dir.join("stdin_dashinit.mp4");
    assert!(init_mp4.exists(), "Init segment must exist");
    let init_bytes = tokio::fs::read(&init_mp4).await.unwrap();
    assert_eq!(
        find_box(&init_bytes, b"pssh"),
        Some(expected_pssh.as_slice()),
        "Init segment must contain the exact Widevine PSSH"
    );
    let tenc = find_box(&init_bytes, b"tenc").expect("Init segment must contain a tenc box");
    assert!(
        tenc.windows(16).any(|bytes| bytes == kid.as_bytes()),
        "tenc must carry the RawKeyProvider KID"
    );

    let seg_m4s = out_dir.join("stdin_dash1.m4s");
    assert!(seg_m4s.exists(), "Encrypted CMAF media segment must exist");
    let seg_bytes = tokio::fs::read(&seg_m4s).await.unwrap();
    for box_type in [b"senc", b"saiz", b"saio"] {
        assert!(
            find_box(&seg_bytes, box_type).is_some(),
            "Encrypted CMAF media segment must contain {}",
            String::from_utf8_lossy(box_type)
        );
    }

    // Cleanup output
    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}
