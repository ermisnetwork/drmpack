use drmpack::session::DrmStreamMetadata;
use drmpack::types::{DrmSystem, EncryptionScheme, KeyMappingPolicy, Rendition};
use drmpack::vod::{VodInputSource, VodMode, VodPackageConfig, VodPackageResult};
use std::path::PathBuf;
use std::time::Duration;

#[test]
fn test_vod_package_config_builder_defaults() {
    let input = VodInputSource::SingleFile(PathBuf::from("video.mp4"));
    let output_dir = PathBuf::from("output/vod");
    let config = VodPackageConfig::new("movie_123", input.clone(), output_dir.clone())
        .with_rendition(Rendition::video_hd())
        .with_rendition(Rendition::audio());

    assert_eq!(config.content_id, "movie_123");
    assert_eq!(config.input, input);
    assert_eq!(config.output_dir, output_dir);
    assert_eq!(config.vod_mode, VodMode::SingleFile);
    assert_eq!(config.encryption_scheme, EncryptionScheme::Cbcs);
    assert_eq!(config.segment_duration, 2.0);
    assert_eq!(config.renditions.len(), 2);
    assert_eq!(config.key_mapping_policy, KeyMappingPolicy::SharedAll);
    assert_eq!(config.timeout, Duration::from_secs(120));
}

#[test]
fn test_vod_package_config_builder_customizations() {
    let input = VodInputSource::TrackFiles(vec![
        PathBuf::from("video_1080p.mp4"),
        PathBuf::from("audio_en.mp4"),
    ]);
    let output_dir = PathBuf::from("output/segmented_vod");
    let config = VodPackageConfig::new("movie_456", input, output_dir)
        .with_vod_mode(VodMode::Segmented)
        .with_encryption_scheme(EncryptionScheme::Dual)
        .with_drm_system(DrmSystem::Widevine)
        .with_drm_system(DrmSystem::FairPlay)
        .with_segment_duration(6.0)
        .with_timeout(Duration::from_secs(300))
        .with_key_mapping_policy(KeyMappingPolicy::PerTierAndTrack);

    assert_eq!(config.vod_mode, VodMode::Segmented);
    assert_eq!(config.encryption_scheme, EncryptionScheme::Dual);
    assert_eq!(config.drm_systems.len(), 2);
    assert_eq!(config.segment_duration, 6.0);
    assert_eq!(config.timeout, Duration::from_secs(300));
    assert_eq!(config.key_mapping_policy, KeyMappingPolicy::PerTierAndTrack);
}

#[test]
fn test_vod_package_result_fields() {
    let result = VodPackageResult {
        content_id: "movie_123".to_string(),
        output_dir: PathBuf::from("output/vod"),
        master_playlist: Some(PathBuf::from("output/vod/vod.m3u8")),
        mpd_manifest: PathBuf::from("output/vod/vod.mpd"),
        variant_playlists: vec![PathBuf::from("output/vod/video_720p.m3u8")],
        media_files: vec![PathBuf::from("output/vod/video_720p.mp4")],
        init_segments: Vec::new(),
        metadata: DrmStreamMetadata {
            content_id: "movie_123".to_string(),
            scheme: EncryptionScheme::Cbcs,
            keys: Vec::new(),
        },
    };

    assert_eq!(result.content_id, "movie_123");
    assert_eq!(result.output_dir, PathBuf::from("output/vod"));
    assert_eq!(
        result.master_playlist,
        Some(PathBuf::from("output/vod/vod.m3u8"))
    );
    assert_eq!(result.variant_playlists.len(), 1);
    assert_eq!(result.media_files.len(), 1);
    assert!(result.init_segments.is_empty());
}
