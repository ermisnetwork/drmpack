mod common;

use common::{find_box, TempDirGuard};
use drmpack::key::{ContentKey, StaticKeySource};
use drmpack::types::{DrmSystem, EncryptionScheme, QualityTier, Rendition, TrackType};
use drmpack::vod::{
    is_complete_isobmff_single_file, package_vod_file, VodInputSource, VodMode, VodPackageConfig,
};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use uuid::Uuid;

fn generate_synthetic_media(path: &Path, video: bool, audio: bool) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut args = vec!["-y"];
    if video {
        args.extend([
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=4:size=640x360:rate=30",
        ]);
    }
    if audio {
        args.extend([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:duration=4:sample_rate=48000",
        ]);
    }
    if video {
        args.extend([
            "-c:v",
            "libx264",
            "-g",
            "60",
            "-keyint_min",
            "60",
            "-sc_threshold",
            "0",
        ]);
    } else {
        args.push("-vn");
    }
    if audio {
        args.extend(["-c:a", "aac", "-b:a", "128k"]);
    } else {
        args.push("-an");
    }
    let path_str = path.to_str().unwrap();
    args.extend(["-f", "mp4", path_str]);

    let status = Command::new("ffmpeg")
        .args(&args)
        .output()
        .expect("failed to run ffmpeg");
    assert!(
        status.status.success(),
        "ffmpeg synthetic generation failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

fn generate_synthetic_mp4(path: &Path) {
    generate_synthetic_media(path, true, true);
}

fn generate_synthetic_video_mp4(path: &Path) {
    generate_synthetic_media(path, true, false);
}

fn generate_synthetic_audio_mp4(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:duration=4:sample_rate=48000",
            "-vn",
            "-streamid",
            "0:2",
            "-use_stream_ids_as_track_ids",
            "1",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-f",
            "mp4",
            path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run ffmpeg");
    assert!(
        status.status.success(),
        "ffmpeg synthetic audio generation failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

fn assert_isobmff_single_files(files: &[std::path::PathBuf]) {
    for media_file in files {
        let data = std::fs::read(media_file).expect("failed to read media file");
        assert!(
            is_complete_isobmff_single_file(&data),
            "media file {:?} must be complete single-file ISOBMFF container",
            media_file
        );
        assert!(
            find_box(&data, b"ftyp").is_some(),
            "media file {:?} missing ftyp box",
            media_file
        );
        assert!(
            find_box(&data, b"moov").is_some(),
            "media file {:?} missing moov box",
            media_file
        );
        assert!(
            find_box(&data, b"sidx").is_some(),
            "media file {:?} missing sidx box",
            media_file
        );
    }
}

#[tokio::test]
async fn test_package_vod_single_file_cbcs() {
    let test_dir = TempDirGuard::new("drmpack_vod_test");
    let input_file = test_dir.join("input.mp4");
    generate_synthetic_mp4(&input_file);

    let output_dir = test_dir.join("out_vod");
    let key_id = Uuid::new_v4();
    let content_key = ContentKey::new(key_id, [0x11; 16], QualityTier::hd(), TrackType::Video);
    let key_source = Arc::new(StaticKeySource::new().with_key(content_key));

    let config = VodPackageConfig::new(
        "test_content",
        VodInputSource::SingleFile(input_file),
        &output_dir,
    )
    .with_vod_mode(VodMode::SingleFile)
    .with_encryption_scheme(EncryptionScheme::Cbcs)
    .with_drm_system(DrmSystem::FairPlay)
    .with_rendition(Rendition::video_hd().with_container_track_id(1))
    .with_rendition(Rendition::audio().with_container_track_id(2).clear());

    let result = package_vod_file(&config, &key_source)
        .await
        .expect("package_vod_file failed");

    assert!(result.mpd_manifest.exists());
    assert!(result.master_playlist.as_ref().unwrap().exists());
    assert!(!result.media_files.is_empty());

    // Verify HLS master and variant playlist
    let master_content = std::fs::read_to_string(result.master_playlist.as_ref().unwrap()).unwrap();
    assert!(master_content.contains("#EXTM3U"));

    // Verify variant playlist contains #EXT-X-ENDLIST and #EXT-X-BYTERANGE
    let variant = &result.variant_playlists[0];
    let variant_content = std::fs::read_to_string(variant).unwrap();
    assert!(variant_content.contains("#EXT-X-ENDLIST"));
    assert!(variant_content.contains("#EXT-X-BYTERANGE:"));

    // Verify MPD manifest contains type="static"
    let mpd_content = std::fs::read_to_string(&result.mpd_manifest).unwrap();
    assert!(mpd_content.contains(r#"type="static""#));
    assert!(mpd_content.contains("<SegmentBase"));

    // Verify generated media files contain ftyp, moov, and sidx boxes
    assert_isobmff_single_files(&result.media_files);
}

#[tokio::test]
async fn test_package_vod_segmented_cenc() {
    let test_dir = TempDirGuard::new("drmpack_vod_test");
    let input_file = test_dir.join("input.mp4");
    generate_synthetic_mp4(&input_file);

    let output_dir = test_dir.join("out_vod_segmented");
    let key_id = Uuid::new_v4();
    let content_key = ContentKey::new(key_id, [0x22; 16], QualityTier::hd(), TrackType::Video);
    let key_source = Arc::new(StaticKeySource::new().with_key(content_key));

    let config = VodPackageConfig::new(
        "test_segmented_content",
        VodInputSource::SingleFile(input_file),
        &output_dir,
    )
    .with_vod_mode(VodMode::Segmented)
    .with_encryption_scheme(EncryptionScheme::Cenc)
    .with_drm_system(DrmSystem::Widevine)
    .with_rendition(Rendition::video_hd().with_container_track_id(1))
    .with_rendition(Rendition::audio().with_container_track_id(2).clear());

    let result = package_vod_file(&config, &key_source)
        .await
        .expect("package_vod_file segmented failed");

    assert!(result.mpd_manifest.exists());
    assert!(!result.media_files.is_empty());
    assert!(!result.init_segments.is_empty());

    let has_m4s = result
        .media_files
        .iter()
        .any(|p| p.extension().and_then(|e| e.to_str()) == Some("m4s"));
    assert!(
        has_m4s,
        "Segmented mode must produce .m4s media segments in media_files"
    );

    let mpd_content = std::fs::read_to_string(&result.mpd_manifest).unwrap();
    assert!(mpd_content.contains(r#"type="static""#));
    assert!(mpd_content.contains("<SegmentTemplate") || mpd_content.contains("<SegmentList"));
}

#[tokio::test]
async fn test_package_vod_dual_scheme() {
    let test_dir = TempDirGuard::new("drmpack_vod_test");
    let input_file = test_dir.join("input.mp4");
    generate_synthetic_mp4(&input_file);

    let output_dir = test_dir.join("out_vod_dual");
    let key_id = Uuid::new_v4();
    let content_key = ContentKey::new(key_id, [0x33; 16], QualityTier::hd(), TrackType::Video);
    let key_source = Arc::new(StaticKeySource::new().with_key(content_key));

    let config = VodPackageConfig::new(
        "test_dual_content",
        VodInputSource::SingleFile(input_file),
        &output_dir,
    )
    .with_vod_mode(VodMode::SingleFile)
    .with_encryption_scheme(EncryptionScheme::Dual)
    .with_drm_system(DrmSystem::Widevine)
    .with_drm_system(DrmSystem::FairPlay)
    .with_rendition(Rendition::video_hd().with_container_track_id(1))
    .with_rendition(Rendition::audio().with_container_track_id(2).clear());

    let result = package_vod_file(&config, &key_source)
        .await
        .expect("package_vod_file dual failed");

    assert!(result.mpd_manifest.exists());
    assert!(result.master_playlist.as_ref().unwrap().exists());
    assert!(!result.media_files.is_empty());

    // Verify subdirectories cenc/ and cbcs/ exist
    assert!(output_dir.join("cenc").exists());
    assert!(output_dir.join("cbcs").exists());
    assert!(output_dir.join("cenc/vod.mpd").exists());
    assert!(output_dir.join("cbcs/vod.mpd").exists());
}

#[tokio::test]
async fn test_package_vod_invalid_config_empty_renditions() {
    let test_dir = TempDirGuard::new("drmpack_vod_test");
    let input_file = test_dir.join("input.mp4");
    generate_synthetic_mp4(&input_file);

    let output_dir = test_dir.join("out_vod_empty");
    let key_source = Arc::new(StaticKeySource::new());

    let config = VodPackageConfig::new(
        "test_empty",
        VodInputSource::SingleFile(input_file),
        &output_dir,
    );

    let err = package_vod_file(&config, &key_source)
        .await
        .expect_err("should fail with empty renditions");

    match err {
        drmpack::error::DrmpackError::InvalidConfig(msg) => {
            assert!(msg.contains("at least one declared Rendition"));
        }
        other => panic!("Unexpected error: {:?}", other),
    }
}

#[tokio::test]
async fn test_package_vod_missing_input_file() {
    let test_dir = TempDirGuard::new("drmpack_vod_missing");
    let non_existent_file = test_dir.join("non_existent_drmpack_input_file.mp4");
    let output_dir = test_dir.join("out_vod_missing");
    let key_source = Arc::new(StaticKeySource::new());

    let config = VodPackageConfig::new(
        "test_missing_input",
        VodInputSource::SingleFile(non_existent_file),
        &output_dir,
    )
    .with_rendition(Rendition::video_hd().with_container_track_id(1));

    let err = package_vod_file(&config, &key_source)
        .await
        .expect_err("should fail with non-existent input file");

    match err {
        drmpack::error::DrmpackError::InvalidConfig(msg) => {
            assert!(msg.contains("does not exist"));
        }
        other => panic!("Unexpected error: {:?}", other),
    }
}

#[tokio::test]
async fn test_package_vod_track_files_input() {
    let test_dir = TempDirGuard::new("drmpack_vod_track_files");
    let video_path = test_dir.join("video.mp4");
    let audio_path = test_dir.join("audio.mp4");
    generate_synthetic_video_mp4(&video_path);
    generate_synthetic_audio_mp4(&audio_path);

    let output_dir = test_dir.join("out_vod_tracks");
    let key_id = Uuid::new_v4();
    let content_key = ContentKey::new(key_id, [0x44; 16], QualityTier::hd(), TrackType::Video);
    let key_source = Arc::new(StaticKeySource::new().with_key(content_key));

    let config = VodPackageConfig::new(
        "test_track_files_content",
        VodInputSource::TrackFiles(vec![video_path, audio_path]),
        &output_dir,
    )
    .with_vod_mode(VodMode::SingleFile)
    .with_encryption_scheme(EncryptionScheme::Cbcs)
    .with_drm_system(DrmSystem::FairPlay)
    .with_rendition(Rendition::video_hd().with_container_track_id(1))
    .with_rendition(Rendition::audio().with_container_track_id(2).clear());

    let result = package_vod_file(&config, &key_source)
        .await
        .expect("package_vod_file with TrackFiles failed");

    assert!(result.mpd_manifest.exists());
    assert!(result.master_playlist.as_ref().unwrap().exists());
    assert_eq!(result.media_files.len(), 2);
    assert_eq!(result.variant_playlists.len(), 2);

    // Verify HLS master and variant playlists
    let master_content = std::fs::read_to_string(result.master_playlist.as_ref().unwrap()).unwrap();
    assert!(master_content.contains("#EXTM3U"));

    for variant in &result.variant_playlists {
        let variant_content = std::fs::read_to_string(variant).unwrap();
        assert!(variant_content.contains("#EXT-X-ENDLIST"));
        assert!(variant_content.contains("#EXT-X-BYTERANGE:"));
    }

    // Verify MPD manifest
    let mpd_content = std::fs::read_to_string(&result.mpd_manifest).unwrap();
    assert!(mpd_content.contains(r#"type="static""#));
    assert!(mpd_content.contains("<SegmentBase"));

    // Verify media files contain ftyp, moov, and sidx
    assert_isobmff_single_files(&result.media_files);
}
