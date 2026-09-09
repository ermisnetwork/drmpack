use bytes::Bytes;
use drmpack::key::{ContentKey, KeyID, PsshData, RawKeyProvider};
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::{DrmSystem, EncryptionScheme, QualityTier, Rendition, TrackType};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output_dir = std::env::temp_dir().join(format!("drmpack_example_02_{}", Uuid::new_v4()));

    let kid_cenc = KeyID::new(Uuid::new_v4());
    let kid_cbcs = KeyID::new(Uuid::new_v4());

    let key_cenc = ContentKey::new(kid_cenc, [0x22; 16], QualityTier::hd(), TrackType::Video);
    let key_cbcs = ContentKey::new_with_scheme(
        kid_cbcs,
        [0x33; 16],
        QualityTier::hd(),
        TrackType::Video,
        EncryptionScheme::Cbcs,
    );

    let widevine_pssh = PsshData::new(
        DrmSystem::Widevine,
        DrmSystem::Widevine.system_id(),
        Bytes::from_static(b"sample-widevine-pssh"),
    );
    let playready_pssh = PsshData::new(
        DrmSystem::PlayReady,
        DrmSystem::PlayReady.system_id(),
        Bytes::from_static(b"sample-playready-pssh"),
    );

    let key_provider = RawKeyProvider::new()
        .with_key(key_cenc)
        .with_key(key_cbcs)
        .with_pssh(widevine_pssh)
        .with_pssh(playready_pssh);

    let rendition = Rendition::video_hd();

    let config = PackagingSessionConfig::low_latency_dual("ll-dual-stream-02")
        .with_rendition(rendition)
        .with_output_dir(&output_dir);

    println!("Creating Dual PackagingSession (CENC + CBCS) with GPAC supervisors...");
    let mut session = PackagingSession::create(config, &key_provider).await?;
    println!(
        "PackagingSession is active: is_alive = {}",
        session.is_alive()
    );

    let media_data = get_media_data().await?;
    let segments = split_fmp4_into_segments(&media_data);
    println!(
        "Generated source stream containing {} continuous fragments",
        segments.len()
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(16);

    tokio::spawn(async move {
        for (i, seg) in segments.into_iter().enumerate() {
            println!(
                "[media-server] Emitting fragment #{} ({} bytes)",
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
            "[drmpack] Fan-out ingesting chunk ({} bytes) to CENC & CBCS pipes...",
            chunk.len()
        );

        session.push(chunk).await?;
        count += 1;

        if !session.is_alive() {
            eprintln!("A GPAC representation terminated unexpectedly!");
            break;
        }
    }

    println!(
        "Processed {} continuous segments across Dual branches. Finalizing...",
        count
    );

    session.check_status().await?;
    session.close().await?;

    println!("Low-Latency Dual Packaging completed successfully.");
    println!("Output directory: {}", session.output_dir().display());

    let cenc_hls = session.output_dir().join("cenc").join("live.m3u8");
    let cenc_mpd = session.output_dir().join("cenc").join("live.mpd");
    let cbcs_hls = session.output_dir().join("cbcs").join("live.m3u8");
    let cbcs_mpd = session.output_dir().join("cbcs").join("live.mpd");

    println!(
        "CENC HLS:  {} (exists: {})",
        cenc_hls.display(),
        cenc_hls.exists()
    );
    println!(
        "CENC DASH: {} (exists: {})",
        cenc_mpd.display(),
        cenc_mpd.exists()
    );
    println!(
        "CBCS HLS:  {} (exists: {})",
        cbcs_hls.display(),
        cbcs_hls.exists()
    );
    println!(
        "CBCS DASH: {} (exists: {})",
        cbcs_mpd.display(),
        cbcs_mpd.exists()
    );

    assert!(cenc_hls.exists(), "CENC HLS master manifest must exist");
    assert!(cbcs_hls.exists(), "CBCS HLS master manifest must exist");

    let cenc_media_m3u8 = session.output_dir().join("cenc").join("video_720p.m3u8");
    let cbcs_media_m3u8 = session.output_dir().join("cbcs").join("video_720p.m3u8");

    if cenc_media_m3u8.exists() && cbcs_media_m3u8.exists() {
        let cenc_content = tokio::fs::read_to_string(&cenc_media_m3u8).await?;
        let cbcs_content = tokio::fs::read_to_string(&cbcs_media_m3u8).await?;

        assert!(cenc_content.contains("#EXT-X-ENDLIST"));
        assert!(cbcs_content.contains("#EXT-X-ENDLIST"));
        assert!(cenc_content.contains("SAMPLE-AES-CTR"));
        assert!(cbcs_content.contains("SAMPLE-AES"));
        println!("Verified: Both CENC (Widevine/PlayReady) and CBCS (FairPlay) media playlists generated #EXT-X-ENDLIST!");
    }

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

    let temp_file = std::env::temp_dir().join(format!("test_ll_{}.mp4", Uuid::new_v4()));
    let output = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=4:size=1280x720:rate=30",
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
            "-movflags",
            "empty_moov+default_base_moof+frag_keyframe",
            "-f",
            "mp4",
            temp_file.to_str().unwrap(),
        ])
        .output()
        .await?;

    if !output.status.success() {
        return Err(format!("ffmpeg failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }

    let bytes = tokio::fs::read(&temp_file).await?;
    let _ = tokio::fs::remove_file(&temp_file).await;
    Ok(bytes)
}
