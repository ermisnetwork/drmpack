use bytes::Bytes;
use drmpack::key::{ContentKey, KeyID, PsshData, RawKeyProvider};
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::{
    DrmSystem, EncryptionScheme, KeyMappingPolicy, LatencyMode, QualityTier, Rendition, TrackType,
};
use std::time::Duration;
use uuid::Uuid;

mod common;
use common::find_box;

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
    println!("SKIPPING multi-track ABR e2e test: 'gpac' and 'ffmpeg' are required");
}

/// Generate synthetic 4-track fMP4:
/// Track 1: Video 1080p (HD, 30fps, H.264)
/// Track 2: Video 720p (SD, 30fps, H.264)
/// Track 3: Audio (44.1kHz AAC)
/// Track 4: Subtitle (tx3g / mov_text)
fn generate_multi_track_fmp4(duration_secs: u32) -> Vec<u8> {
    let tmp_dir = std::env::temp_dir();
    let sub_path = tmp_dir.join(format!("test_sub_{}.srt", Uuid::new_v4()));
    let srt_content = "1\n00:00:00,000 --> 00:00:02,000\nHello World ABR\n\n2\n00:00:02,000 --> 00:00:04,000\nMulti-Track Packaging\n";
    std::fs::write(&sub_path, srt_content).expect("Failed to write test SRT file");

    let out_mp4_path = tmp_dir.join(format!("test_multi_{}.mp4", Uuid::new_v4()));

    let ffmpeg_status = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc=duration={duration_secs}:size=1920x1080:rate=30"),
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc=duration={duration_secs}:size=1280x720:rate=30"),
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency=1000:duration={duration_secs}"),
            "-i",
            sub_path.to_str().unwrap(),
            "-map",
            "0:v",
            "-map",
            "1:v",
            "-map",
            "2:a",
            "-map",
            "3:s",
            "-c:v:0",
            "libx264",
            "-b:v:0",
            "2000k",
            "-g",
            "30",
            "-keyint_min",
            "30",
            "-sc_threshold",
            "0",
            "-c:v:1",
            "libx264",
            "-b:v:1",
            "1000k",
            "-g",
            "30",
            "-keyint_min",
            "30",
            "-sc_threshold",
            "0",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-c:s",
            "mov_text",
            "-movflags",
            "empty_moov+default_base_moof+frag_keyframe",
            "-f",
            "mp4",
            out_mp4_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute FFmpeg to generate 4-track fMP4");

    assert!(
        ffmpeg_status.status.success(),
        "FFmpeg multi-track fMP4 generation failed: {}",
        String::from_utf8_lossy(&ffmpeg_status.stderr)
    );

    let mp4_bytes =
        std::fs::read(&out_mp4_path).expect("Failed to read generated multi-track fMP4");
    let _ = std::fs::remove_file(&sub_path);
    let _ = std::fs::remove_file(&out_mp4_path);
    mp4_bytes
}

/// Generate synthetic 5-track fMP4:
/// Track 1: Video 1080p (HD, 30fps, H.264)
/// Track 2: Video 720p (SD, 30fps, H.264)
/// Track 3: Audio English (44.1kHz AAC)
/// Track 4: Audio Spanish (44.1kHz AAC)
/// Track 5: Subtitle English (tx3g / mov_text)
fn generate_5track_fmp4(duration_secs: u32) -> Vec<u8> {
    let tmp_dir = std::env::temp_dir();
    let sub_path = tmp_dir.join(format!("test_sub5_{}.srt", Uuid::new_v4()));
    let srt_content = "1\n00:00:00,000 --> 00:00:02,000\nHello Multi-Audio\n\n2\n00:00:02,000 --> 00:00:04,000\nMulti-Track Audio Packaging\n";
    std::fs::write(&sub_path, srt_content).expect("Failed to write test SRT file");

    let out_mp4_path = tmp_dir.join(format!("test_multi5_{}.mp4", Uuid::new_v4()));

    let ffmpeg_status = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc=duration={duration_secs}:size=1920x1080:rate=30"),
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc=duration={duration_secs}:size=1280x720:rate=30"),
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency=1000:duration={duration_secs}"),
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency=500:duration={duration_secs}"),
            "-i",
            sub_path.to_str().unwrap(),
            "-map",
            "0:v",
            "-map",
            "1:v",
            "-map",
            "2:a",
            "-map",
            "3:a",
            "-map",
            "4:s",
            "-c:v:0",
            "libx264",
            "-b:v:0",
            "2000k",
            "-g",
            "30",
            "-keyint_min",
            "30",
            "-sc_threshold",
            "0",
            "-c:v:1",
            "libx264",
            "-b:v:1",
            "1000k",
            "-g",
            "30",
            "-keyint_min",
            "30",
            "-sc_threshold",
            "0",
            "-c:a:0",
            "aac",
            "-b:a:0",
            "128k",
            "-metadata:s:a:0",
            "language=eng",
            "-c:a:1",
            "aac",
            "-b:a:1",
            "96k",
            "-metadata:s:a:1",
            "language=spa",
            "-c:s",
            "mov_text",
            "-metadata:s:s:0",
            "language=eng",
            "-movflags",
            "empty_moov+default_base_moof+frag_keyframe",
            "-f",
            "mp4",
            out_mp4_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute FFmpeg to generate 5-track fMP4");

    assert!(
        ffmpeg_status.status.success(),
        "FFmpeg 5-track generation failed: {}",
        String::from_utf8_lossy(&ffmpeg_status.stderr)
    );

    let mp4_bytes = std::fs::read(&out_mp4_path).expect("Failed to read generated 5-track fMP4");
    let _ = std::fs::remove_file(&sub_path);
    let _ = std::fs::remove_file(&out_mp4_path);
    mp4_bytes
}

fn create_multi_track_key_provider() -> (RawKeyProvider, KeyID, KeyID, KeyID) {
    let kid_v_hd = KeyID::new(Uuid::from_bytes([
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10,
    ]));
    let kid_v_sd = KeyID::new(Uuid::from_bytes([
        0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
        0x20,
    ]));
    let kid_audio = KeyID::new(Uuid::from_bytes([
        0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f,
        0x30,
    ]));

    let key_v_hd = ContentKey::new(kid_v_hd, [0x42; 16], QualityTier::hd(), TrackType::Video);
    let key_v_sd = ContentKey::new(kid_v_sd, [0x43; 16], QualityTier::sd(), TrackType::Video);
    let key_audio = ContentKey::new(
        kid_audio,
        [0x44; 16],
        QualityTier::audio(),
        TrackType::Audio,
    );

    let widevine_pssh = PsshData::new(
        DrmSystem::Widevine,
        DrmSystem::Widevine.system_id(),
        Bytes::from_static(b"widevine-multi-track-payload"),
    );

    let fairplay_pssh = PsshData::new(
        DrmSystem::FairPlay,
        DrmSystem::FairPlay.system_id(),
        Bytes::from_static(b""),
    );

    let provider = RawKeyProvider::new()
        .with_key(key_v_hd)
        .with_key(key_v_sd)
        .with_key(key_audio)
        .with_pssh(widevine_pssh)
        .with_pssh(fairplay_pssh);

    (provider, kid_v_hd, kid_v_sd, kid_audio)
}

#[tokio::test]
async fn test_multi_track_abr_e2e_cenc() {
    if !media_tools_available() {
        require_media_tools();
        return;
    }

    let (provider, kid_v_hd, kid_v_sd, kid_audio) = create_multi_track_key_provider();

    let r_v_hd = Rendition::video_hd().with_container_track_id(1);
    let r_v_sd = Rendition::video(QualityTier::sd()).with_container_track_id(2);
    let r_audio = Rendition::audio().with_container_track_id(3);
    let r_sub = Rendition::subtitle().with_container_track_id(4);

    let out_dir = std::env::temp_dir().join(format!("drmpack_abr_cenc_{}", Uuid::new_v4()));

    let max_attempts = 3;
    let mut last_err = String::new();
    for attempt in 1..=max_attempts {
        let _ = tokio::fs::remove_dir_all(&out_dir).await;

        let config = PackagingSessionConfig::new("abr-cenc-stream")
            .with_rendition(r_v_hd.clone())
            .with_rendition(r_v_sd.clone())
            .with_rendition(r_audio.clone())
            .with_rendition(r_sub.clone())
            .with_key_mapping_policy(KeyMappingPolicy::PerTierAndTrack)
            .with_encryption_scheme(EncryptionScheme::Cenc)
            .with_drm_system(DrmSystem::Widevine)
            .with_latency_mode(LatencyMode::LowLatency)
            .with_finalization_timeout(Duration::from_secs(30))
            .with_segment_duration(2.0)
            .with_chunk_duration(0.2)
            .with_output_dir(&out_dir);

        let mut session = match PackagingSession::create(config, &provider).await {
            Ok(s) => s,
            Err(e) => {
                last_err = format!("{e}");
                eprintln!(
                    "CENC test attempt {attempt}/{max_attempts} failed on create: {last_err}"
                );
                continue;
            }
        };

        // Feed synthetic 4-track fMP4
        let sample_bytes = generate_multi_track_fmp4(4);
        if let Err(e) = session.push(sample_bytes).await {
            last_err = format!("{e}");
            eprintln!("CENC test attempt {attempt}/{max_attempts} failed on push: {last_err}");
            continue;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;

        if let Err(e) = session.close().await {
            last_err = format!("{e}");
            eprintln!("CENC test attempt {attempt}/{max_attempts} failed on close: {last_err}");
            continue;
        }

        break;
    }

    assert!(
        out_dir.exists(),
        "All {max_attempts} attempts failed. Last error: {last_err}"
    );

    // 1. Verify DASH MPD
    let mpd_path = out_dir.join("live.mpd");
    assert!(mpd_path.exists(), "live.mpd must exist");
    let mpd = tokio::fs::read_to_string(&mpd_path).await.unwrap();

    // Video AdaptationSet with multiple Representations
    assert!(
        mpd.contains(r#"mimeType="video/mp4""#),
        "Must contain video AdaptationSet"
    );
    assert!(
        !mpd.contains("trickmode"),
        "DASH MPD must NOT declare trickmode for secondary video representation"
    );
    assert!(
        mpd.contains(r#"width="1920""#),
        "Must contain 1080p video Representation"
    );
    assert!(
        mpd.contains(r#"width="1280""#),
        "Must contain 720p video Representation"
    );
    assert!(
        mpd.contains(&kid_v_hd.0.hyphenated().to_string())
            || mpd.contains(&kid_v_sd.0.hyphenated().to_string()),
        "Video AdaptationSet must signal ContentProtection default_KID"
    );

    // Audio AdaptationSet
    assert!(
        mpd.contains(r#"mimeType="audio/mp4""#),
        "Must contain audio AdaptationSet"
    );
    assert!(
        mpd.contains("mp4a.40.2"),
        "Audio Representation must specify AAC codec"
    );

    // Subtitle AdaptationSet (clear / unencrypted)
    assert!(
        mpd.contains(r#"mimeType="application/mp4""#) || mpd.contains(r#"codecs="tx3g""#),
        "Must contain subtitle AdaptationSet"
    );

    // 2. Verify HLS Master Manifest
    let m3u8_path = out_dir.join("live.m3u8");
    assert!(m3u8_path.exists(), "live.m3u8 master manifest must exist");
    let master = tokio::fs::read_to_string(&m3u8_path).await.unwrap();

    assert!(
        master.contains("#EXT-X-STREAM-INF"),
        "Master manifest must contain rendition streams"
    );
    assert!(
        master.contains(r#"AUDIO="audio""#),
        "Rendition streams must link to audio group"
    );
    assert!(
        master.contains(r#"SUBTITLES="subs""#),
        "Rendition streams must link to subtitles group"
    );
    assert!(
        master.contains("#EXT-X-MEDIA:TYPE=AUDIO"),
        "Master manifest must declare #EXT-X-MEDIA:TYPE=AUDIO"
    );
    assert!(
        master.contains("#EXT-X-MEDIA:TYPE=SUBTITLES"),
        "Master manifest must declare #EXT-X-MEDIA:TYPE=SUBTITLES"
    );
    // 3. Verify Media Manifests
    // Video rendition 1 (1080p) must be encrypted
    let v1_m3u8 = tokio::fs::read_to_string(out_dir.join("video_1080p.m3u8"))
        .await
        .unwrap();
    assert!(
        v1_m3u8.contains("#EXT-X-KEY:METHOD=SAMPLE-AES-CTR"),
        "Video rendition 1 must have SAMPLE-AES-CTR key"
    );

    // Video rendition 2 (720p) must be encrypted
    let v2_m3u8 = tokio::fs::read_to_string(out_dir.join("video_720p.m3u8"))
        .await
        .unwrap();
    assert!(
        v2_m3u8.contains("#EXT-X-KEY:METHOD=SAMPLE-AES-CTR"),
        "Video rendition 2 must have SAMPLE-AES-CTR key"
    );

    // Audio media manifest must be encrypted
    let a_m3u8 = tokio::fs::read_to_string(out_dir.join("audio.m3u8"))
        .await
        .unwrap();
    assert!(
        a_m3u8.contains("#EXT-X-KEY:METHOD=SAMPLE-AES-CTR"),
        "Audio manifest must have SAMPLE-AES-CTR key"
    );

    // Subtitle media manifest must be CLEAR (NO #EXT-X-KEY)
    let s_m3u8 = tokio::fs::read_to_string(out_dir.join("sub.m3u8"))
        .await
        .unwrap();
    assert!(
        !s_m3u8.contains("#EXT-X-KEY"),
        "Subtitle manifest must NOT contain #EXT-X-KEY (must be clear text)"
    );

    // 4. Verify CMAF Init Boxes with find_box
    let mut found_video_hd = false;
    let mut found_video_sd = false;
    let mut found_audio = false;
    let mut found_sub = false;

    for entry in std::fs::read_dir(&out_dir).unwrap().filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with("_init.mp4") {
            let data = std::fs::read(entry.path()).unwrap();
            if name.starts_with("video_1080p") {
                let tenc = find_box(&data, b"tenc")
                    .expect("Video 1080p init segment must contain tenc box");
                assert!(
                    tenc.windows(16).any(|b| b == kid_v_hd.as_bytes()),
                    "Video 1080p tenc must carry HD KID"
                );
                assert!(
                    find_box(&data, b"pssh").is_some(),
                    "Video 1080p init segment must contain pssh box"
                );
                let schm =
                    find_box(&data, b"schm").expect("Video 1080p init must contain schm box");
                assert!(
                    schm.windows(4).any(|b| b == b"cenc"),
                    "Video 1080p schm must declare cenc"
                );
                found_video_hd = true;
            } else if name.starts_with("video_720p") {
                let tenc = find_box(&data, b"tenc")
                    .expect("Video 720p init segment must contain tenc box");
                assert!(
                    tenc.windows(16).any(|b| b == kid_v_sd.as_bytes()),
                    "Video 720p tenc must carry SD KID"
                );
                assert!(
                    find_box(&data, b"pssh").is_some(),
                    "Video 720p init segment must contain pssh box"
                );
                let schm = find_box(&data, b"schm").expect("Video 720p init must contain schm box");
                assert!(
                    schm.windows(4).any(|b| b == b"cenc"),
                    "Video 720p schm must declare cenc"
                );
                found_video_sd = true;
            } else if name.starts_with("audio") {
                let tenc =
                    find_box(&data, b"tenc").expect("Audio init segment must contain tenc box");
                assert!(
                    tenc.windows(16).any(|b| b == kid_audio.as_bytes()),
                    "Audio tenc must carry audio KID"
                );
                let schm = find_box(&data, b"schm").expect("Audio init must contain schm box");
                assert!(
                    schm.windows(4).any(|b| b == b"cenc"),
                    "Audio schm must declare cenc"
                );
                found_audio = true;
            } else if name.starts_with("sub") {
                assert!(
                    find_box(&data, b"tenc").is_none(),
                    "Subtitle init segment must NOT contain tenc box (must be clear)"
                );
                assert!(
                    find_box(&data, b"schm").is_none(),
                    "Subtitle init segment must NOT contain schm box (must be clear)"
                );
                found_sub = true;
            }
        }
    }
    assert!(found_video_hd, "Must have verified HD video init segment");
    assert!(found_video_sd, "Must have verified SD video init segment");
    assert!(found_audio, "Must have verified audio init segment");
    assert!(found_sub, "Must have verified subtitle init segment");

    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_multi_track_abr_e2e_cbcs() {
    if !media_tools_available() {
        require_media_tools();
        return;
    }

    let (provider, kid_v_hd, kid_v_sd, kid_audio) = create_multi_track_key_provider();

    let r_v_hd = Rendition::video_hd().with_container_track_id(1);
    let r_v_sd = Rendition::video(QualityTier::sd()).with_container_track_id(2);
    let r_audio = Rendition::audio().with_container_track_id(3);
    let r_sub = Rendition::subtitle().with_container_track_id(4);

    let out_dir = std::env::temp_dir().join(format!("drmpack_abr_cbcs_{}", Uuid::new_v4()));

    let max_attempts = 3;
    let mut last_err = String::new();
    for attempt in 1..=max_attempts {
        let _ = tokio::fs::remove_dir_all(&out_dir).await;

        let config = PackagingSessionConfig::new("abr-cbcs-stream")
            .with_rendition(r_v_hd.clone())
            .with_rendition(r_v_sd.clone())
            .with_rendition(r_audio.clone())
            .with_rendition(r_sub.clone())
            .with_key_mapping_policy(KeyMappingPolicy::PerTierAndTrack)
            .with_encryption_scheme(EncryptionScheme::Cbcs)
            .with_drm_system(DrmSystem::FairPlay)
            .with_latency_mode(LatencyMode::LowLatency)
            .with_finalization_timeout(Duration::from_secs(30))
            .with_segment_duration(2.0)
            .with_chunk_duration(0.2)
            .with_output_dir(&out_dir);

        let mut session = match PackagingSession::create(config, &provider).await {
            Ok(s) => s,
            Err(e) => {
                last_err = format!("{e}");
                eprintln!(
                    "CBCS test attempt {attempt}/{max_attempts} failed on create: {last_err}"
                );
                continue;
            }
        };

        let sample_bytes = generate_multi_track_fmp4(4);
        if let Err(e) = session.push(sample_bytes).await {
            last_err = format!("{e}");
            eprintln!("CBCS test attempt {attempt}/{max_attempts} failed on push: {last_err}");
            continue;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;

        if let Err(e) = session.close().await {
            last_err = format!("{e}");
            eprintln!("CBCS test attempt {attempt}/{max_attempts} failed on close: {last_err}");
            continue;
        }

        break;
    }

    assert!(
        out_dir.exists(),
        "All {max_attempts} attempts failed. Last error: {last_err}"
    );

    // 1. Verify HLS Master Manifest
    let master_path = out_dir.join("live.m3u8");
    assert!(master_path.exists(), "live.m3u8 master manifest must exist");
    let master = tokio::fs::read_to_string(&master_path).await.unwrap();

    assert!(master.contains("#EXT-X-STREAM-INF"));
    assert!(master.contains(r#"AUDIO="audio""#));
    assert!(master.contains(r#"SUBTITLES="subs""#));
    assert!(master.contains("#EXT-X-MEDIA:TYPE=AUDIO"));
    assert!(master.contains("#EXT-X-MEDIA:TYPE=SUBTITLES"));

    // 2. Verify HLS Media Manifests (FairPlay SAMPLE-AES)
    let v1_m3u8 = tokio::fs::read_to_string(out_dir.join("video_1080p.m3u8"))
        .await
        .unwrap();
    assert!(
        v1_m3u8.contains("#EXT-X-KEY:METHOD=SAMPLE-AES"),
        "CBCS video rendition 1 must have METHOD=SAMPLE-AES"
    );
    assert!(
        v1_m3u8.contains(r#"KEYFORMAT="com.apple.streamingkeydelivery""#),
        "CBCS video rendition 1 must specify FairPlay key format"
    );
    assert!(
        v1_m3u8.contains("URI=\"skd://"),
        "CBCS video rendition 1 must contain skd:// URI"
    );

    let v2_m3u8 = tokio::fs::read_to_string(out_dir.join("video_720p.m3u8"))
        .await
        .unwrap();
    assert!(
        v2_m3u8.contains("#EXT-X-KEY:METHOD=SAMPLE-AES"),
        "CBCS video rendition 2 must have METHOD=SAMPLE-AES"
    );
    assert!(
        v2_m3u8.contains(r#"KEYFORMAT="com.apple.streamingkeydelivery""#),
        "CBCS video rendition 2 must specify FairPlay key format"
    );

    let a_m3u8 = tokio::fs::read_to_string(out_dir.join("audio.m3u8"))
        .await
        .unwrap();
    assert!(
        a_m3u8.contains("#EXT-X-KEY:METHOD=SAMPLE-AES"),
        "CBCS audio rendition must have METHOD=SAMPLE-AES"
    );

    // Subtitle manifest must be CLEAR
    let s_m3u8 = tokio::fs::read_to_string(out_dir.join("sub.m3u8"))
        .await
        .unwrap();
    assert!(
        !s_m3u8.contains("#EXT-X-KEY"),
        "Subtitle manifest must NOT contain #EXT-X-KEY (cleartext)"
    );

    // 3. Verify CMAF Init Boxes with find_box
    let mut found_video_hd = false;
    let mut found_video_sd = false;
    let mut found_audio = false;
    let mut found_sub = false;

    for entry in std::fs::read_dir(&out_dir).unwrap().filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with("_init.mp4") {
            let data = std::fs::read(entry.path()).unwrap();
            if name.starts_with("video_1080p") {
                let schm =
                    find_box(&data, b"schm").expect("Video 1080p init must contain schm box");
                assert!(
                    schm.windows(4).any(|b| b == b"cbcs"),
                    "Video 1080p schm must declare cbcs"
                );
                let tenc =
                    find_box(&data, b"tenc").expect("Video 1080p init must contain tenc box");
                assert!(
                    tenc.windows(16).any(|b| b == kid_v_hd.as_bytes()),
                    "Video 1080p tenc must carry HD KID"
                );
                found_video_hd = true;
            } else if name.starts_with("video_720p") {
                let schm = find_box(&data, b"schm").expect("Video 720p init must contain schm box");
                assert!(
                    schm.windows(4).any(|b| b == b"cbcs"),
                    "Video 720p schm must declare cbcs"
                );
                let tenc = find_box(&data, b"tenc").expect("Video 720p init must contain tenc box");
                assert!(
                    tenc.windows(16).any(|b| b == kid_v_sd.as_bytes()),
                    "Video 720p tenc must carry SD KID"
                );
                found_video_sd = true;
            } else if name.starts_with("audio") {
                let schm = find_box(&data, b"schm").expect("Audio init must contain schm box");
                assert!(
                    schm.windows(4).any(|b| b == b"cbcs"),
                    "Audio schm must declare cbcs"
                );
                let tenc = find_box(&data, b"tenc").expect("Audio init must contain tenc box");
                assert!(
                    tenc.windows(16).any(|b| b == kid_audio.as_bytes()),
                    "Audio tenc must carry audio KID"
                );
                found_audio = true;
            } else if name.starts_with("sub") {
                assert!(
                    find_box(&data, b"tenc").is_none(),
                    "Subtitle init must be unencrypted"
                );
                assert!(
                    find_box(&data, b"schm").is_none(),
                    "Subtitle init must not contain schm"
                );
                found_sub = true;
            }
        }
    }
    assert!(
        found_video_hd,
        "Must have verified HD video init segment in CBCS"
    );
    assert!(
        found_video_sd,
        "Must have verified SD video init segment in CBCS"
    );
    assert!(found_audio, "Must have verified audio init segment in CBCS");
    assert!(
        found_sub,
        "Must have verified subtitle init segment in CBCS"
    );

    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_multi_track_abr_e2e_dual() {
    if !media_tools_available() {
        require_media_tools();
        return;
    }

    let out_dir = std::env::temp_dir().join(format!("drmpack_abr_dual_{}", Uuid::new_v4()));

    // Dual mode spawns two concurrent GPAC processes (CENC + CBCS) that can race on
    // filesystem I/O, causing intermittent crashes.  Retry up to 3 times.
    let max_attempts = 3;
    let mut last_err = String::new();
    for attempt in 1..=max_attempts {
        // Clean output dir between attempts
        let _ = tokio::fs::remove_dir_all(&out_dir).await;

        let (provider, _, _, _) = create_multi_track_key_provider();

        let r_v_hd = Rendition::video_hd().with_container_track_id(1);
        let r_v_sd = Rendition::video(QualityTier::sd()).with_container_track_id(2);
        let r_audio = Rendition::audio().with_container_track_id(3);
        let r_sub = Rendition::subtitle().with_container_track_id(4);

        let config = PackagingSessionConfig::new("abr-dual-stream")
            .with_rendition(r_v_hd)
            .with_rendition(r_v_sd)
            .with_rendition(r_audio)
            .with_rendition(r_sub)
            .with_key_mapping_policy(KeyMappingPolicy::PerTierAndTrack)
            .with_encryption_scheme(EncryptionScheme::Dual)
            .with_drm_system(DrmSystem::Widevine)
            .with_drm_system(DrmSystem::FairPlay)
            .with_latency_mode(LatencyMode::LowLatency)
            .with_finalization_timeout(Duration::from_secs(30))
            .with_segment_duration(2.0)
            .with_chunk_duration(0.2)
            .with_output_dir(&out_dir);

        let mut session = PackagingSession::create(config, &provider)
            .await
            .expect("Failed to create PackagingSession for multi-track Dual");

        let sample_bytes = generate_multi_track_fmp4(4);
        let push_result = session.push(sample_bytes).await;
        if let Err(e) = push_result {
            last_err = format!("{e}");
            eprintln!("Dual test attempt {attempt}/{max_attempts} failed on push: {last_err}");
            continue;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;

        let close_result = session.close().await;
        if let Err(e) = close_result {
            last_err = format!("{e}");
            eprintln!("Dual test attempt {attempt}/{max_attempts} failed on close: {last_err}");
            continue;
        }

        // Success — fall through to assertions
        break;
    }

    // If out_dir doesn't exist after all retries, all attempts failed
    assert!(
        out_dir.exists(),
        "All {max_attempts} attempts failed. Last error: {last_err}"
    );

    let (_, kid_v_hd, kid_v_sd, _) = create_multi_track_key_provider();

    // Verify manifest path existence
    assert!(out_dir.join("cenc/live.mpd").exists());
    assert!(out_dir.join("cenc/live.m3u8").exists());
    assert!(out_dir.join("cbcs/live.mpd").exists());
    assert!(out_dir.join("cbcs/live.m3u8").exists());

    // 1. Verify CENC Representation
    let cenc_mpd = tokio::fs::read_to_string(out_dir.join("cenc/live.mpd"))
        .await
        .unwrap();
    assert!(cenc_mpd.contains(r#"mimeType="video/mp4""#));
    assert!(
        !cenc_mpd.contains("trickmode"),
        "CENC MPD must NOT declare trickmode for secondary video"
    );
    assert!(cenc_mpd.contains(r#"width="1920""#));
    assert!(cenc_mpd.contains(r#"width="1280""#));
    assert!(cenc_mpd.contains(r#"mimeType="audio/mp4""#));
    assert!(cenc_mpd.contains(r#"codecs="tx3g""#));
    assert!(
        cenc_mpd.contains(&kid_v_hd.0.hyphenated().to_string())
            || cenc_mpd.contains(&kid_v_sd.0.hyphenated().to_string())
    );

    let cenc_master = tokio::fs::read_to_string(out_dir.join("cenc/live.m3u8"))
        .await
        .unwrap();
    assert!(cenc_master.contains("#EXT-X-STREAM-INF"));
    assert!(cenc_master.contains("#EXT-X-MEDIA:TYPE=AUDIO"));
    assert!(cenc_master.contains("#EXT-X-MEDIA:TYPE=SUBTITLES"));

    let cenc_v1 = tokio::fs::read_to_string(out_dir.join("cenc/video_1080p.m3u8"))
        .await
        .unwrap();
    assert!(cenc_v1.contains("#EXT-X-KEY:METHOD=SAMPLE-AES-CTR"));

    let cenc_v2 = tokio::fs::read_to_string(out_dir.join("cenc/video_720p.m3u8"))
        .await
        .unwrap();
    assert!(cenc_v2.contains("#EXT-X-KEY:METHOD=SAMPLE-AES-CTR"));

    let cenc_audio = tokio::fs::read_to_string(out_dir.join("cenc/audio.m3u8"))
        .await
        .unwrap();
    assert!(cenc_audio.contains("#EXT-X-KEY:METHOD=SAMPLE-AES-CTR"));

    let cenc_sub = tokio::fs::read_to_string(out_dir.join("cenc/sub.m3u8"))
        .await
        .unwrap();
    assert!(
        !cenc_sub.contains("#EXT-X-KEY"),
        "CENC subtitle must be unencrypted"
    );

    // 2. Verify CBCS Representation
    let cbcs_master = tokio::fs::read_to_string(out_dir.join("cbcs/live.m3u8"))
        .await
        .unwrap();
    assert!(cbcs_master.contains("#EXT-X-STREAM-INF"));
    assert!(cbcs_master.contains("#EXT-X-MEDIA:TYPE=AUDIO"));
    assert!(cbcs_master.contains("#EXT-X-MEDIA:TYPE=SUBTITLES"));

    let cbcs_v1 = tokio::fs::read_to_string(out_dir.join("cbcs/video_1080p.m3u8"))
        .await
        .unwrap();
    assert!(cbcs_v1.contains("#EXT-X-KEY:METHOD=SAMPLE-AES"));
    assert!(cbcs_v1.contains("com.apple.streamingkeydelivery"));

    let cbcs_v2 = tokio::fs::read_to_string(out_dir.join("cbcs/video_720p.m3u8"))
        .await
        .unwrap();
    assert!(cbcs_v2.contains("#EXT-X-KEY:METHOD=SAMPLE-AES"));
    assert!(cbcs_v2.contains("com.apple.streamingkeydelivery"));

    let cbcs_audio = tokio::fs::read_to_string(out_dir.join("cbcs/audio.m3u8"))
        .await
        .unwrap();
    assert!(cbcs_audio.contains("#EXT-X-KEY:METHOD=SAMPLE-AES"));

    let cbcs_sub = tokio::fs::read_to_string(out_dir.join("cbcs/sub.m3u8"))
        .await
        .unwrap();
    assert!(
        !cbcs_sub.contains("#EXT-X-KEY"),
        "CBCS subtitle must be unencrypted"
    );

    // 3. Verify init segments in CENC directory
    let mut found_cenc_hd = false;
    let mut found_cenc_sd = false;
    let mut found_cenc_audio = false;
    let mut found_cenc_sub = false;

    for entry in std::fs::read_dir(out_dir.join("cenc"))
        .unwrap()
        .filter_map(|e| e.ok())
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with("_init.mp4") {
            let data = std::fs::read(entry.path()).unwrap();
            if name.starts_with("video_1080p") {
                let schm =
                    find_box(&data, b"schm").expect("CENC HD video init must contain schm box");
                assert!(
                    schm.windows(4).any(|b| b == b"cenc"),
                    "CENC HD schm must declare cenc"
                );
                let tenc =
                    find_box(&data, b"tenc").expect("CENC HD video init must contain tenc box");
                assert!(tenc.windows(16).any(|b| b == kid_v_hd.as_bytes()));
                found_cenc_hd = true;
            } else if name.starts_with("video_720p") {
                let schm =
                    find_box(&data, b"schm").expect("CENC SD video init must contain schm box");
                assert!(
                    schm.windows(4).any(|b| b == b"cenc"),
                    "CENC SD schm must declare cenc"
                );
                let tenc =
                    find_box(&data, b"tenc").expect("CENC SD video init must contain tenc box");
                assert!(tenc.windows(16).any(|b| b == kid_v_sd.as_bytes()));
                found_cenc_sd = true;
            } else if name.starts_with("audio") {
                let schm = find_box(&data, b"schm").expect("CENC Audio init must contain schm box");
                assert!(
                    schm.windows(4).any(|b| b == b"cenc"),
                    "CENC audio schm must declare cenc"
                );
                found_cenc_audio = true;
            } else if name.starts_with("sub") {
                assert!(
                    find_box(&data, b"tenc").is_none(),
                    "CENC subtitle must be unencrypted"
                );
                found_cenc_sub = true;
            }
        }
    }
    assert!(found_cenc_hd, "Must verify CENC HD video init");
    assert!(found_cenc_sd, "Must verify CENC SD video init");
    assert!(found_cenc_audio, "Must verify CENC audio init");
    assert!(found_cenc_sub, "Must verify CENC subtitle init");

    // 4. Verify init segments in CBCS directory
    let mut found_cbcs_hd = false;
    let mut found_cbcs_sd = false;
    let mut found_cbcs_audio = false;
    let mut found_cbcs_sub = false;

    for entry in std::fs::read_dir(out_dir.join("cbcs"))
        .unwrap()
        .filter_map(|e| e.ok())
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with("_init.mp4") {
            let data = std::fs::read(entry.path()).unwrap();
            if name.starts_with("video_1080p") {
                let schm =
                    find_box(&data, b"schm").expect("CBCS HD video init must contain schm box");
                assert!(
                    schm.windows(4).any(|b| b == b"cbcs"),
                    "CBCS HD schm must declare cbcs"
                );
                let tenc =
                    find_box(&data, b"tenc").expect("CBCS HD video init must contain tenc box");
                assert!(tenc.windows(16).any(|b| b == kid_v_hd.as_bytes()));
                found_cbcs_hd = true;
            } else if name.starts_with("video_720p") {
                let schm =
                    find_box(&data, b"schm").expect("CBCS SD video init must contain schm box");
                assert!(
                    schm.windows(4).any(|b| b == b"cbcs"),
                    "CBCS SD schm must declare cbcs"
                );
                let tenc =
                    find_box(&data, b"tenc").expect("CBCS SD video init must contain tenc box");
                assert!(tenc.windows(16).any(|b| b == kid_v_sd.as_bytes()));
                found_cbcs_sd = true;
            } else if name.starts_with("audio") {
                let schm = find_box(&data, b"schm").expect("CBCS Audio init must contain schm box");
                assert!(
                    schm.windows(4).any(|b| b == b"cbcs"),
                    "CBCS audio schm must declare cbcs"
                );
                found_cbcs_audio = true;
            } else if name.starts_with("sub") {
                assert!(
                    find_box(&data, b"tenc").is_none(),
                    "CBCS subtitle must be unencrypted"
                );
                found_cbcs_sub = true;
            }
        }
    }
    assert!(found_cbcs_hd, "Must verify CBCS HD video init");
    assert!(found_cbcs_sd, "Must verify CBCS SD video init");
    assert!(found_cbcs_audio, "Must verify CBCS audio init");
    assert!(found_cbcs_sub, "Must verify CBCS subtitle init");

    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_multi_track_abr_e2e_fallback_track_id() {
    if !media_tools_available() {
        require_media_tools();
        return;
    }

    let (provider, kid_v_hd, kid_v_sd, kid_audio) = create_multi_track_key_provider();

    // Do NOT specify with_container_track_id; verify fallback to 1-based index (1, 2, 3, 4)
    let r_v_hd = Rendition::video_hd();
    let r_v_sd = Rendition::video(QualityTier::sd());
    let r_audio = Rendition::audio();
    let r_sub = Rendition::subtitle();

    assert_eq!(r_v_hd.container_track_id, None);
    assert_eq!(r_v_sd.container_track_id, None);
    assert_eq!(r_audio.container_track_id, None);
    assert_eq!(r_sub.container_track_id, None);

    let out_dir = std::env::temp_dir().join(format!("drmpack_abr_fallback_{}", Uuid::new_v4()));

    let max_attempts = 3;
    let mut last_err = String::new();
    for attempt in 1..=max_attempts {
        let _ = tokio::fs::remove_dir_all(&out_dir).await;

        let config = PackagingSessionConfig::new("abr-fallback-stream")
            .with_rendition(r_v_hd.clone())
            .with_rendition(r_v_sd.clone())
            .with_rendition(r_audio.clone())
            .with_rendition(r_sub.clone())
            .with_key_mapping_policy(KeyMappingPolicy::PerTierAndTrack)
            .with_encryption_scheme(EncryptionScheme::Cenc)
            .with_drm_system(DrmSystem::Widevine)
            .with_latency_mode(LatencyMode::LowLatency)
            .with_finalization_timeout(Duration::from_secs(30))
            .with_segment_duration(2.0)
            .with_chunk_duration(0.2)
            .with_output_dir(&out_dir);

        let mut session = match PackagingSession::create(config, &provider).await {
            Ok(s) => s,
            Err(e) => {
                last_err = format!("{e}");
                eprintln!(
                    "Fallback test attempt {attempt}/{max_attempts} failed on create: {last_err}"
                );
                continue;
            }
        };

        let sample_bytes = generate_multi_track_fmp4(4);
        if let Err(e) = session.push(sample_bytes).await {
            last_err = format!("{e}");
            eprintln!("Fallback test attempt {attempt}/{max_attempts} failed on push: {last_err}");
            continue;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;

        if let Err(e) = session.close().await {
            last_err = format!("{e}");
            eprintln!("Fallback test attempt {attempt}/{max_attempts} failed on close: {last_err}");
            continue;
        }

        break;
    }

    assert!(
        out_dir.exists(),
        "All {max_attempts} attempts failed. Last error: {last_err}"
    );

    let mpd = tokio::fs::read_to_string(out_dir.join("live.mpd"))
        .await
        .unwrap();
    assert!(!mpd.contains("trickmode"));
    assert!(mpd.contains(r#"width="1920""#));
    assert!(mpd.contains(r#"width="1280""#));
    assert!(mpd.contains("mp4a.40.2"));

    let master = tokio::fs::read_to_string(out_dir.join("live.m3u8"))
        .await
        .unwrap();
    assert!(master.contains("#EXT-X-STREAM-INF"));
    assert!(master.contains(r#"AUDIO="audio""#));

    // Verify init segments have expected encryption
    let mut found_hd = false;
    let mut found_sd = false;
    let mut found_audio = false;
    let mut found_sub = false;

    for entry in std::fs::read_dir(&out_dir).unwrap().filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with("_init.mp4") {
            let data = std::fs::read(entry.path()).unwrap();
            if name.starts_with("video_1080p") {
                let tenc = find_box(&data, b"tenc").expect("HD video init must contain tenc");
                assert!(tenc.windows(16).any(|b| b == kid_v_hd.as_bytes()));
                found_hd = true;
            } else if name.starts_with("video_720p") {
                let tenc = find_box(&data, b"tenc").expect("SD video init must contain tenc");
                assert!(tenc.windows(16).any(|b| b == kid_v_sd.as_bytes()));
                found_sd = true;
            } else if name.starts_with("audio") {
                let tenc = find_box(&data, b"tenc").expect("Audio init must contain tenc");
                assert!(tenc.windows(16).any(|b| b == kid_audio.as_bytes()));
                found_audio = true;
            } else if name.starts_with("sub") {
                assert!(find_box(&data, b"tenc").is_none());
                found_sub = true;
            }
        }
    }
    assert!(found_hd, "Fallback must correctly map HD video (track 1)");
    assert!(found_sd, "Fallback must correctly map SD video (track 2)");
    assert!(found_audio, "Fallback must correctly map audio (track 3)");
    assert!(found_sub, "Fallback must correctly map subtitle (track 4)");

    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_multi_track_abr_e2e_multi_audio_languages() {
    if !media_tools_available() {
        require_media_tools();
        return;
    }

    let kid_v_hd = KeyID::new(Uuid::from_bytes([0x01; 16]));
    let kid_v_sd = KeyID::new(Uuid::from_bytes([0x02; 16]));
    let kid_audio_en = KeyID::new(Uuid::from_bytes([0x03; 16]));
    let kid_audio_es = KeyID::new(Uuid::from_bytes([0x04; 16]));

    let key_v_hd = ContentKey::new(kid_v_hd, [0x11; 16], QualityTier::hd(), TrackType::Video);
    let key_v_sd = ContentKey::new(kid_v_sd, [0x22; 16], QualityTier::sd(), TrackType::Video);
    let key_a_en = ContentKey::new(
        kid_audio_en,
        [0x33; 16],
        QualityTier::new("en"),
        TrackType::Audio,
    );
    let key_a_es = ContentKey::new(
        kid_audio_es,
        [0x44; 16],
        QualityTier::new("es"),
        TrackType::Audio,
    );

    let widevine_pssh = PsshData::new(
        DrmSystem::Widevine,
        DrmSystem::Widevine.system_id(),
        Bytes::from_static(b"widevine-multi-lang-payload"),
    );

    let provider = RawKeyProvider::new()
        .with_key(key_v_hd)
        .with_key(key_v_sd)
        .with_key(key_a_en)
        .with_key(key_a_es)
        .with_pssh(widevine_pssh);

    let r_v_hd = Rendition::video_hd().with_container_track_id(1);
    let r_v_sd = Rendition::video(QualityTier::sd()).with_container_track_id(2);
    let r_a_en = Rendition::audio_tier(QualityTier::new("en")).with_container_track_id(3);
    let r_a_es = Rendition::audio_tier(QualityTier::new("es")).with_container_track_id(4);
    let r_sub = Rendition::subtitle().with_container_track_id(5);

    let out_dir = std::env::temp_dir().join(format!("drmpack_abr_lang_{}", Uuid::new_v4()));

    let max_attempts = 3;
    let mut last_err = String::new();
    for attempt in 1..=max_attempts {
        let _ = tokio::fs::remove_dir_all(&out_dir).await;

        let config = PackagingSessionConfig::new("abr-lang-stream")
            .with_rendition(r_v_hd.clone())
            .with_rendition(r_v_sd.clone())
            .with_rendition(r_a_en.clone())
            .with_rendition(r_a_es.clone())
            .with_rendition(r_sub.clone())
            .with_key_mapping_policy(KeyMappingPolicy::PerTierAndTrack)
            .with_encryption_scheme(EncryptionScheme::Cenc)
            .with_drm_system(DrmSystem::Widevine)
            .with_latency_mode(LatencyMode::LowLatency)
            .with_finalization_timeout(Duration::from_secs(30))
            .with_segment_duration(2.0)
            .with_chunk_duration(0.2)
            .with_output_dir(&out_dir);

        let mut session = match PackagingSession::create(config, &provider).await {
            Ok(s) => s,
            Err(e) => {
                last_err = format!("{e}");
                eprintln!(
                    "Multi-lang test attempt {attempt}/{max_attempts} failed on create: {last_err}"
                );
                continue;
            }
        };

        let sample_bytes = generate_5track_fmp4(4);
        if let Err(e) = session.push(sample_bytes).await {
            last_err = format!("{e}");
            eprintln!(
                "Multi-lang test attempt {attempt}/{max_attempts} failed on push: {last_err}"
            );
            continue;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;

        if let Err(e) = session.close().await {
            last_err = format!("{e}");
            eprintln!(
                "Multi-lang test attempt {attempt}/{max_attempts} failed on close: {last_err}"
            );
            continue;
        }

        break;
    }

    assert!(
        out_dir.exists(),
        "All {max_attempts} attempts failed. Last error: {last_err}"
    );

    // 1. Verify DASH MPD with multiple audio AdaptationSets with distinct languages
    let mpd = tokio::fs::read_to_string(out_dir.join("live.mpd"))
        .await
        .unwrap();
    assert!(!mpd.contains("trickmode"));
    assert!(mpd.contains(r#"mimeType="video/mp4""#));
    assert!(
        mpd.contains(r#"lang="en""#),
        "Must declare English audio AdaptationSet"
    );
    assert!(
        mpd.contains(r#"lang="es""#),
        "Must declare Spanish audio AdaptationSet"
    );
    assert!(
        mpd.contains(&kid_audio_en.0.hyphenated().to_string()),
        "Must signal English audio KID in MPD"
    );
    assert!(
        mpd.contains(&kid_audio_es.0.hyphenated().to_string()),
        "Must signal Spanish audio KID in MPD"
    );

    // 2. Verify HLS Master Manifest has distinct LANGUAGE attributes
    let master = tokio::fs::read_to_string(out_dir.join("live.m3u8"))
        .await
        .unwrap();
    assert!(master.contains(r#"#EXT-X-MEDIA:TYPE=AUDIO"#));
    assert!(
        master.contains(r#"LANGUAGE="eng""#),
        "HLS master must declare English audio with LANGUAGE=\"eng\""
    );
    assert!(
        master.contains(r#"LANGUAGE="spa""#),
        "HLS master must declare Spanish audio with LANGUAGE=\"spa\""
    );
    assert!(
        master.contains(r#"#EXT-X-MEDIA:TYPE=SUBTITLES"#),
        "HLS master must declare subtitles"
    );

    // 3. Verify media manifests
    let a_en_m3u8 = tokio::fs::read_to_string(out_dir.join("audio_eng.m3u8"))
        .await
        .unwrap();
    assert!(a_en_m3u8.contains("#EXT-X-KEY:METHOD=SAMPLE-AES-CTR"));

    let a_es_m3u8 = tokio::fs::read_to_string(out_dir.join("audio_spa.m3u8"))
        .await
        .unwrap();
    assert!(a_es_m3u8.contains("#EXT-X-KEY:METHOD=SAMPLE-AES-CTR"));

    let sub_m3u8 = tokio::fs::read_to_string(out_dir.join("sub_eng.m3u8"))
        .await
        .unwrap();
    assert!(!sub_m3u8.contains("#EXT-X-KEY"), "Subtitles must be clear");

    // 4. Verify all 5 init segments
    let mut found_hd = false;
    let mut found_sd = false;
    let mut found_a_en = false;
    let mut found_a_es = false;
    let mut found_sub = false;

    for entry in std::fs::read_dir(&out_dir).unwrap().filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with("_init.mp4") {
            let data = std::fs::read(entry.path()).unwrap();
            if name.starts_with("video_1080p") {
                let tenc = find_box(&data, b"tenc").expect("HD video init must contain tenc");
                assert!(tenc.windows(16).any(|b| b == kid_v_hd.as_bytes()));
                found_hd = true;
            } else if name.starts_with("video_720p") {
                let tenc = find_box(&data, b"tenc").expect("SD video init must contain tenc");
                assert!(tenc.windows(16).any(|b| b == kid_v_sd.as_bytes()));
                found_sd = true;
            } else if name.starts_with("audio_eng") {
                let tenc = find_box(&data, b"tenc").expect("Audio EN init must contain tenc");
                assert!(tenc.windows(16).any(|b| b == kid_audio_en.as_bytes()));
                found_a_en = true;
            } else if name.starts_with("audio_spa") {
                let tenc = find_box(&data, b"tenc").expect("Audio ES init must contain tenc");
                assert!(tenc.windows(16).any(|b| b == kid_audio_es.as_bytes()));
                found_a_es = true;
            } else if name.starts_with("sub") {
                assert!(find_box(&data, b"tenc").is_none());
                found_sub = true;
            }
        }
    }
    assert!(found_hd, "Must verify HD video init in 5-track stream");
    assert!(found_sd, "Must verify SD video init in 5-track stream");
    assert!(found_a_en, "Must verify Audio EN init in 5-track stream");
    assert!(found_a_es, "Must verify Audio ES init in 5-track stream");
    assert!(found_sub, "Must verify Subtitle init in 5-track stream");

    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}
