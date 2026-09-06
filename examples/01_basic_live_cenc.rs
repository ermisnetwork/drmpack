use bytes::Bytes;
use drmpack::key::StaticKeySource;
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::Rendition;
use std::time::Duration;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting Example 01: Continuous Live DRM Packaging (CENC / Widevine)");

    let output_dir = std::env::temp_dir().join(format!("drmpack_example_01_{}", Uuid::new_v4()));
    let key_provider = StaticKeySource::shared_key([0x11; 16]);
    let rendition = Rendition::video_hd();

    let config = PackagingSessionConfig::cenc("live-stream-01")
        .with_rendition(rendition)
        .with_output_dir(&output_dir);

    println!("Creating PackagingSession and spawning GPAC supervisor...");
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
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        println!("[media-server] Upstream stream ended (EOF)");
    });

    let mut count = 0;
    while let Some(chunk) = rx.recv().await {
        println!(
            "[drmpack] Ingesting chunk ({} bytes) into GPAC stdin pipe...",
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
        "Processed {} continuous segments. Finalizing session...",
        count
    );

    session.check_status().await?;
    session.close().await?;

    println!("Session closed cleanly.");
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
    println!("Master HLS Manifest contents:\n{}", master_m3u8.trim());

    let media_m3u8_path = session.output_dir().join("live_1.m3u8");
    if media_m3u8_path.exists() {
        let media_m3u8 = tokio::fs::read_to_string(&media_m3u8_path).await?;
        assert!(
            media_m3u8.contains("#EXT-X-ENDLIST"),
            "Media manifest must contain #EXT-X-ENDLIST"
        );
        assert!(media_m3u8.contains("#EXT-X-KEY:METHOD=SAMPLE-AES-CTR"));
        println!("Verified: Media playlist (live_1.m3u8) has Widevine DRM key signaling and #EXT-X-ENDLIST!");
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

    let temp_file = std::env::temp_dir().join(format!("test_{}.mp4", Uuid::new_v4()));
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
