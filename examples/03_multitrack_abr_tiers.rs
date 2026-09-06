use bytes::Bytes;
use drmpack::key::{ContentKey, KeyID, PsshData, RawKeyProvider};
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::{
    DrmSystem, EncryptionScheme, KeyMappingPolicy, QualityTier, Rendition, TrackType,
};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output_dir = std::env::temp_dir().join(format!("drmpack_example_03_{}", Uuid::new_v4()));

    let key_video_4k = ContentKey::new(
        KeyID::new(Uuid::new_v4()),
        [0x41; 16],
        QualityTier::uhd_4k(),
        TrackType::Video,
    );
    let key_video_hd = ContentKey::new(
        KeyID::new(Uuid::new_v4()),
        [0x42; 16],
        QualityTier::hd(),
        TrackType::Video,
    );
    let key_video_sd = ContentKey::new(
        KeyID::new(Uuid::new_v4()),
        [0x43; 16],
        QualityTier::sd(),
        TrackType::Video,
    );
    let key_audio = ContentKey::new(
        KeyID::new(Uuid::new_v4()),
        [0x44; 16],
        QualityTier::sd(),
        TrackType::Audio,
    );

    let widevine_pssh = PsshData::new(
        DrmSystem::Widevine,
        DrmSystem::Widevine.system_id(),
        Bytes::from_static(b"sample-widevine-pssh"),
    );

    let key_provider = RawKeyProvider::new()
        .with_key(key_video_4k)
        .with_key(key_video_hd)
        .with_key(key_video_sd)
        .with_key(key_audio)
        .with_pssh(widevine_pssh);

    let config = PackagingSessionConfig::new("multitrack-abr-stream-03")
        .with_renditions(vec![
            Rendition::video_4k(),
            Rendition::video_hd(),
            Rendition::video(QualityTier::sd()),
            Rendition::audio(),
            Rendition::audio(),
            Rendition::subtitle(),
        ])
        .with_key_mapping_policy(KeyMappingPolicy::PerTierAndTrack)
        .with_encryption_scheme(EncryptionScheme::Cenc)
        .with_drm_system(DrmSystem::Widevine)
        .with_output_dir(&output_dir);

    println!("Creating Multi-Track ABR PackagingSession with GPAC supervisor...");
    let mut session = PackagingSession::create(config, &key_provider).await?;
    println!(
        "PackagingSession is active: is_alive = {}",
        session.is_alive()
    );

    let media_data = get_media_data().await?;
    let segments = split_fmp4_into_segments(&media_data);
    println!(
        "Generated 6-track source stream containing {} continuous fragments",
        segments.len()
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(16);

    tokio::spawn(async move {
        for (i, seg) in segments.into_iter().enumerate() {
            println!(
                "[media-server] Emitting multi-track fragment #{} ({} bytes)",
                i,
                seg.len()
            );
            if tx.send(seg).await.is_err() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
        println!("[media-server] Upstream stream ended (EOF)");
    });

    let mut count = 0;
    while let Some(chunk) = rx.recv().await {
        println!(
            "[drmpack] Ingesting multi-track chunk ({} bytes) into GPAC stdin pipe...",
            chunk.len()
        );

        session.push(chunk).await?;
        count += 1;

        if !session.is_alive() {
            eprintln!("GPAC process terminated unexpectedly!");
            break;
        }
    }

    println!(
        "Processed {} continuous multi-track segments. Finalizing session...",
        count
    );

    session.check_status().await?;
    session.close().await?;

    println!("Multi-Track ABR Packaging completed successfully.");
    println!("Output directory: {}", session.output_dir().display());
    println!(
        "HLS Manifest:     {}",
        session.hls_manifest_path()?.display()
    );
    println!(
        "DASH Manifest:    {}",
        session.dash_manifest_path()?.display()
    );

    let master_m3u8 = tokio::fs::read_to_string(session.hls_manifest_path()?).await?;
    assert!(
        master_m3u8.contains("TYPE=AUDIO"),
        "HLS master must declare audio tracks"
    );
    assert!(
        master_m3u8.contains("TYPE=SUBTITLES"),
        "HLS master must declare subtitle tracks"
    );
    println!("Verified: HLS master manifest has Multi-Track Audio and Subtitle signaling!");

    let _ = tokio::fs::remove_dir_all(&output_dir).await;
    println!("Cleaned up temporary directory.");

    Ok(())
}

fn split_fmp4_into_segments(data: &[u8]) -> Vec<Bytes> {
    let mut segments = Vec::new();
    let mut offset = 0;
    let mut init_bytes = Vec::new();

    while offset + 8 <= data.len() {
        let size = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let box_type = &data[offset + 4..offset + 8];
        let box_size = if size == 0 {
            data.len() - offset
        } else if size == 1 {
            if offset + 16 > data.len() {
                break;
            }
            u64::from_be_bytes(data[offset + 8..offset + 16].try_into().unwrap()) as usize
        } else {
            size
        };

        if box_size < 8 || offset + box_size > data.len() {
            break;
        }

        let box_data = &data[offset..offset + box_size];
        offset += box_size;

        if box_type == b"ftyp" || box_type == b"moov" {
            init_bytes.extend_from_slice(box_data);
            if box_type == b"moov" {
                segments.push(Bytes::from(std::mem::take(&mut init_bytes)));
            }
        } else if box_type == b"moof" {
            let mut frag_bytes = Vec::from(box_data);
            if offset + 8 <= data.len() {
                let next_size =
                    u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
                let next_box_type = &data[offset + 4..offset + 8];
                let next_box_size = if next_size == 0 {
                    data.len() - offset
                } else if next_size == 1 {
                    if offset + 16 > data.len() {
                        0
                    } else {
                        u64::from_be_bytes(data[offset + 8..offset + 16].try_into().unwrap())
                            as usize
                    }
                } else {
                    next_size
                };

                if next_box_type == b"mdat"
                    && next_box_size >= 8
                    && offset + next_box_size <= data.len()
                {
                    frag_bytes.extend_from_slice(&data[offset..offset + next_box_size]);
                    offset += next_box_size;
                }
            }

            segments.push(Bytes::from(frag_bytes));
        }
    }

    if segments.is_empty() && !data.is_empty() {
        segments.push(Bytes::copy_from_slice(data));
    }

    segments
}

async fn get_media_data() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && !args[1].starts_with('-') {
        return Ok(tokio::fs::read(&args[1]).await?);
    }

    let tmp_dir = std::env::temp_dir();
    let sub_path = tmp_dir.join(format!("sub_{}.srt", Uuid::new_v4()));
    let srt_content = "1\n00:00:00,000 --> 00:00:02,000\nXin chao ABR\n\n2\n00:00:02,000 --> 00:00:04,000\nMulti-Track Demo\n";
    tokio::fs::write(&sub_path, srt_content).await?;

    let temp_file = tmp_dir.join(format!("test_multi_{}.mp4", Uuid::new_v4()));
    let output = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=4:size=1920x1080:rate=30",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=4:size=1280x720:rate=30",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=4:size=854x480:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:duration=4",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1200:duration=4",
            "-i",
            sub_path.to_str().unwrap(),
            "-map",
            "0:v",
            "-map",
            "1:v",
            "-map",
            "2:v",
            "-map",
            "3:a",
            "-map",
            "4:a",
            "-map",
            "5:s",
            "-c:v",
            "libx264",
            "-g",
            "30",
            "-keyint_min",
            "30",
            "-profile:v",
            "baseline",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-c:s",
            "mov_text",
            "-movflags",
            "empty_moov+default_base_moof+frag_keyframe",
            "-f",
            "mp4",
            temp_file.to_str().unwrap(),
        ])
        .output()
        .await?;

    let _ = tokio::fs::remove_file(&sub_path).await;

    if !output.status.success() {
        return Err(format!("ffmpeg failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }

    let bytes = tokio::fs::read(&temp_file).await?;
    let _ = tokio::fs::remove_file(&temp_file).await;
    Ok(bytes)
}
