use drmpack::types::{ArtifactKind, PackagedArtifact};
use std::io;
use std::path::{Path, PathBuf};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub struct CdnPublisher;

impl CdnPublisher {
    /// Spawn a direct channel-driven publisher that writes PackagedArtifacts as they arrive.
    /// Eliminates filesystem polling, directory scanning, and multi-track race conditions.
    pub fn spawn_channel(
        target_dir: impl Into<PathBuf>,
        mut rx: tokio::sync::mpsc::Receiver<PackagedArtifact>,
        is_dual: bool,
    ) -> CdnPublisherHandle {
        let target_dir = target_dir.into();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let join_handle = tokio::spawn(async move {
            let _ = tokio::fs::create_dir_all(&target_dir).await;
            if is_dual {
                let _ = tokio::fs::create_dir_all(target_dir.join("cenc")).await;
                let _ = tokio::fs::create_dir_all(target_dir.join("cbcs")).await;
            }

            let mut max_segments = std::collections::HashMap::new();

            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        while let Ok(artifact) = rx.try_recv() {
                            Self::write_artifact(&target_dir, &artifact, is_dual, &mut max_segments).await;
                        }
                        break;
                    }
                    artifact_opt = rx.recv() => {
                        match artifact_opt {
                            Some(artifact) => {
                                Self::write_artifact(&target_dir, &artifact, is_dual, &mut max_segments).await;
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        CdnPublisherHandle {
            shutdown_tx: Some(shutdown_tx),
            join_handle,
        }
    }

    async fn write_artifact(
        target_dir: &Path,
        artifact: &PackagedArtifact,
        is_dual: bool,
        max_segments: &mut std::collections::HashMap<drmpack::types::EncryptionScheme, u64>,
    ) {
        if artifact.kind == ArtifactKind::MediaSegment {
            if let Some(num) = drmpack::session::harvester::parse_segment_number(&artifact.filename)
            {
                let entry = max_segments.entry(artifact.scheme).or_insert(0);
                *entry = (*entry).max(num);
            }
        }

        let dest_dir = if is_dual {
            target_dir.join(artifact.scheme.to_string())
        } else {
            target_dir.to_path_buf()
        };

        let dest_path = dest_dir.join(&artifact.filename);
        let temp_path = dest_dir.join(format!(
            ".{}_{}.tmp",
            artifact.filename,
            uuid::Uuid::new_v4().simple()
        ));

        let mut data = artifact.data.clone();
        if artifact.kind == ArtifactKind::Manifest {
            if let Ok(text) = std::str::from_utf8(&data) {
                if text.contains("#EXT-X-STREAM-INF") {
                    data = bytes::Bytes::from(sanitize_master_playlist(text).into_bytes());
                } else if artifact.filename.ends_with(".mpd") && text.contains("type=\"static\"") {
                    let max_seg = max_segments.get(&artifact.scheme).copied();
                    let sanitized = drmpack::session::harvester::sanitize_static_mpd(text, max_seg);
                    data = bytes::Bytes::from(sanitized.into_bytes());
                }
            }
        }

        if let Err(e) = tokio::fs::write(&temp_path, &data).await {
            eprintln!(
                "[CDN ERROR] Failed to write temp file {}: {e}",
                temp_path.display()
            );
        } else if let Err(e) = tokio::fs::rename(&temp_path, &dest_path).await {
            eprintln!(
                "[CDN ERROR] Failed to rename {} to {}: {e}",
                temp_path.display(),
                dest_path.display()
            );
        }
    }
}

pub struct CdnPublisherHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: JoinHandle<()>,
}

impl CdnPublisherHandle {
    pub async fn stop(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        let _ = self.join_handle.await;
    }
}

pub async fn clean_dir(dir: &Path) -> io::Result<()> {
    if !dir.exists() {
        tokio::fs::create_dir_all(dir).await?;
        return Ok(());
    }
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_file() {
            let _ = tokio::fs::remove_file(&path).await;
        } else if path.is_dir() {
            let _ = tokio::fs::remove_dir_all(&path).await;
        }
    }
    Ok(())
}

pub fn sanitize_master_playlist(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut header = Vec::new();
    let mut media = Vec::new();
    let mut stream_infs = Vec::new();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() {
            i += 1;
            continue;
        }
        if line.starts_with("#EXT-X-MEDIA:") {
            media.push(line.to_string());
            i += 1;
        } else if line.starts_with("#EXT-X-STREAM-INF:") {
            let mut block = vec![line.to_string()];
            if i + 1 < lines.len() && !lines[i + 1].trim().starts_with('#') {
                block.push(lines[i + 1].trim().to_string());
                i += 1;
            }
            stream_infs.push(block.join("\n"));
            i += 1;
        } else {
            header.push(line.to_string());
            i += 1;
        }
    }

    let mut out = header;
    if !media.is_empty() {
        out.push(String::new());
        out.extend(media);
    }
    if !stream_infs.is_empty() {
        out.push(String::new());
        out.extend(stream_infs);
    }
    out.push(String::new());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_clean_dir() {
        let temp_dir =
            std::env::temp_dir().join(format!("drmpack_clean_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        tokio::fs::write(temp_dir.join("file1.txt"), b"hello")
            .await
            .unwrap();
        tokio::fs::create_dir_all(temp_dir.join("subdir"))
            .await
            .unwrap();
        tokio::fs::write(temp_dir.join("subdir").join("file2.txt"), b"world")
            .await
            .unwrap();

        assert!(temp_dir.exists());
        assert!(temp_dir.join("file1.txt").exists());

        clean_dir(&temp_dir).await.unwrap();

        // The directory itself must still exist!
        assert!(temp_dir.exists());
        assert!(!temp_dir.join("file1.txt").exists());
        assert!(!temp_dir.join("subdir").exists());

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_cdn_publisher_spawn_channel() {
        let temp_dir =
            std::env::temp_dir().join(format!("drmpack_cdn_channel_test_{}", uuid::Uuid::new_v4()));
        let cdn_storage = temp_dir.join("cdn_storage");
        let (tx, rx) = tokio::sync::mpsc::channel(16);

        let handle = CdnPublisher::spawn_channel(&cdn_storage, rx, false);

        tx.send(PackagedArtifact {
            filename: "video_720p_init.mp4".to_string(),
            data: bytes::Bytes::from_static(b"ftyp-init"),
            kind: ArtifactKind::InitSegment,
            scheme: drmpack::types::EncryptionScheme::Cenc,
        })
        .await
        .unwrap();

        tx.send(PackagedArtifact {
            filename: "video_720p_1.m4s".to_string(),
            data: bytes::Bytes::from_static(b"moof-segment"),
            kind: ArtifactKind::MediaSegment,
            scheme: drmpack::types::EncryptionScheme::Cenc,
        })
        .await
        .unwrap();

        drop(tx);
        handle.stop().await;

        assert!(cdn_storage.join("video_720p_init.mp4").exists());
        assert!(cdn_storage.join("video_720p_1.m4s").exists());
        assert_eq!(
            tokio::fs::read(cdn_storage.join("video_720p_1.m4s"))
                .await
                .unwrap(),
            b"moof-segment"
        );

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[test]
    fn test_sanitize_master_playlist() {
        let raw = r#"#EXTM3U
#EXT-X-VERSION:6
#EXT-X-STREAM-INF:BANDWIDTH=1280000,RESOLUTION=1280x720
video_720p.m3u8
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="audio",NAME="English",URI="audio.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=2560000,RESOLUTION=1920x1080
video_1080p.m3u8
"#;

        let sanitized = sanitize_master_playlist(raw);
        let media_pos = sanitized.find("#EXT-X-MEDIA:").unwrap();
        let stream_inf_pos = sanitized.find("#EXT-X-STREAM-INF:").unwrap();
        assert!(
            media_pos < stream_inf_pos,
            "#EXT-X-MEDIA must precede #EXT-X-STREAM-INF"
        );
        assert!(sanitized.starts_with("#EXTM3U\n#EXT-X-VERSION:6"));
    }

    #[tokio::test]
    async fn test_cdn_publisher_sanitizes_static_mpd() {
        let temp_dir =
            std::env::temp_dir().join(format!("drmpack_cdn_mpd_test_{}", uuid::Uuid::new_v4()));
        let cdn_storage = temp_dir.join("cdn_storage");
        let (tx, rx) = tokio::sync::mpsc::channel(16);

        let handle = CdnPublisher::spawn_channel(&cdn_storage, rx, false);

        // Send 5 segments
        for i in 1..=5 {
            tx.send(PackagedArtifact {
                filename: format!("video_720p_{i}.m4s"),
                data: bytes::Bytes::from_static(b"moof-segment"),
                kind: ArtifactKind::MediaSegment,
                scheme: drmpack::types::EncryptionScheme::Cenc,
            })
            .await
            .unwrap();
        }

        // Send static MPD with 10.021s duration (GPAC audio frame excess)
        let raw_mpd = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" minBufferTime="PT2.000S" type="static" mediaPresentationDuration="PT0H0M10.021S">
 <Period id="DID1" duration="PT0H0M10.021S">
  <AdaptationSet mimeType="video/mp4">
   <SegmentTemplate media="$RepresentationID$_$Number$.m4s" initialization="$RepresentationID$_init.mp4" timescale="15360" startNumber="1" duration="30720"/>
   <Representation id="video_720p" width="1280" height="720"/>
  </AdaptationSet>
 </Period>
</MPD>"#;

        tx.send(PackagedArtifact {
            filename: "live.mpd".to_string(),
            data: bytes::Bytes::from(raw_mpd.as_bytes()),
            kind: ArtifactKind::Manifest,
            scheme: drmpack::types::EncryptionScheme::Cenc,
        })
        .await
        .unwrap();

        drop(tx);
        handle.stop().await;

        let published_mpd = tokio::fs::read_to_string(cdn_storage.join("live.mpd"))
            .await
            .unwrap();
        assert!(published_mpd.contains(r#"mediaPresentationDuration="PT0H0M10.000S""#));
        assert!(published_mpd.contains(r#"<Period id="DID1" duration="PT0H0M10.000S">"#));
        assert!(!published_mpd.contains("10.021S"));

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
