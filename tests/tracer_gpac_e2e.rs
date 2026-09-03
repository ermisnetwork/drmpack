use drmpack::error::DrmpackError;
use drmpack::key::{ContentKey, KeyID, PsshData, RawKeyProvider};
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::{DrmSystem, QualityTier, Rendition, TrackType};
use uuid::Uuid;

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
        data: bytes::Bytes::from_static(b"widevine-pssh-test-payload"),
    };

    let provider = RawKeyProvider::new().with_key(content_key).with_pssh(pssh);

    (provider, kid)
}

#[tokio::test]
async fn test_packaging_session_detects_missing_gpac_binary() {
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
