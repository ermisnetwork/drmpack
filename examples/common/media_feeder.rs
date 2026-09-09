use bytes::Bytes;
use drmpack::session::PackagingSession;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tracing::info;

#[derive(Debug, Clone)]
pub struct MediaFeederConfig {
    pub input_path: PathBuf,
    pub max_duration: Option<Duration>,
    pub max_chunks: Option<u64>,
    pub force_transcode: bool,
    pub force_copy: bool,
}

impl MediaFeederConfig {
    pub fn new(input_path: impl Into<PathBuf>) -> Self {
        Self {
            input_path: input_path.into(),
            max_duration: None,
            max_chunks: None,
            force_transcode: false,
            force_copy: false,
        }
    }

    pub fn with_duration(mut self, duration: Option<Duration>) -> Self {
        self.max_duration = duration;
        self
    }

    pub fn with_max_chunks(mut self, max_chunks: Option<u64>) -> Self {
        self.max_chunks = max_chunks;
        self
    }

    pub fn with_force_transcode(mut self, force: bool) -> Self {
        self.force_transcode = force;
        self
    }

    pub fn with_force_copy(mut self, force: bool) -> Self {
        self.force_copy = force;
        self
    }
}

pub async fn is_valid_media_file(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    match tokio::fs::metadata(path).await {
        Ok(m) => m.len() > 0,
        Err(_) => false,
    }
}

pub async fn resolve_or_create_input_media(
    custom_input: Option<&str>,
) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(path_str) = custom_input {
        let is_url = path_str.starts_with("http://")
            || path_str.starts_with("https://")
            || path_str.starts_with("rtmp://")
            || path_str.starts_with("rtsp://");
        let path = PathBuf::from(path_str);
        if is_url || is_valid_media_file(&path).await {
            info!("Using custom input media: {}", path.display());
            return Ok(path);
        } else {
            return Err(
                format!("Specified input file does not exist or is empty: {path_str}").into(),
            );
        }
    }

    let default_sample = PathBuf::from("scratch/sample.mp4");
    if is_valid_media_file(&default_sample).await {
        info!(
            "Found existing sample media at {}",
            default_sample.display()
        );
        return Ok(default_sample);
    }

    let legacy_test = PathBuf::from("scratch/test.mp4");
    if is_valid_media_file(&legacy_test).await {
        info!("Found existing test media at {}", legacy_test.display());
        return Ok(legacy_test);
    }

    ensure_sample_media(&default_sample).await?;
    Ok(default_sample)
}

pub async fn ensure_sample_media(path: &Path) -> Result<(), Box<dyn Error>> {
    if is_valid_media_file(path).await {
        return Ok(());
    }

    if path.exists() {
        let _ = tokio::fs::remove_file(path).await;
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    println!(
        "Generating synthetic 10s test video with 2.0s GOP (60 frames): {}",
        path.display()
    );

    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=10:size=1280x720:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:duration=10:sample_rate=48000",
            "-c:v",
            "libx264",
            "-g",
            "60",
            "-keyint_min",
            "60",
            "-sc_threshold",
            "0",
            "-profile:v",
            "baseline",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-ar",
            "48000",
            "-f",
            "mp4",
            path.to_str().unwrap_or("scratch/sample.mp4"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;

    if !status.success() {
        return Err(format!("ffmpeg failed to create test video at {}", path.display()).into());
    }

    println!("Sample media created successfully: {}", path.display());
    Ok(())
}

pub async fn check_stream_copy(path: &Path) -> bool {
    let probe = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            path.to_str().unwrap_or(""),
            "-t",
            "0.1",
            "-c",
            "copy",
            "-movflags",
            "empty_moov+default_base_moof+frag_keyframe",
            "-f",
            "mp4",
            "-y",
            "/dev/null",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    matches!(probe, Ok(status) if status.success())
}

pub struct MediaFeeder {
    config: MediaFeederConfig,
}

impl MediaFeeder {
    pub fn new(config: MediaFeederConfig) -> Self {
        Self { config }
    }

    pub async fn feed_to_session(
        &self,
        session: &mut PackagingSession,
    ) -> Result<u64, Box<dyn Error>> {
        let input_str = self
            .config
            .input_path
            .to_str()
            .ok_or("Invalid input file path")?;

        let is_known_sample = self.config.input_path.ends_with("scratch/sample.mp4")
            || self.config.input_path.ends_with("scratch/test.mp4");

        let can_copy = if self.config.force_transcode {
            false
        } else if self.config.force_copy {
            true
        } else if is_known_sample {
            check_stream_copy(&self.config.input_path).await
        } else {
            // For arbitrary custom input media, default to transcoding to guarantee
            // fixed 2.0s GOP (60 frames), Baseline H.264 + AAC 48k as required.
            false
        };

        let mut cmd = tokio::process::Command::new("ffmpeg");
        cmd.arg("-re")
            .arg("-stream_loop")
            .arg("-1")
            .arg("-i")
            .arg(input_str);

        if can_copy {
            println!("Feeding via FFmpeg stream copy (-c copy)...");
            cmd.arg("-c").arg("copy");
        } else {
            println!("Feeding via FFmpeg live transcoder (GOP=60 / 2.0s, H.264 Baseline + AAC)...");
            cmd.args([
                "-c:v",
                "libx264",
                "-g",
                "60",
                "-keyint_min",
                "60",
                "-sc_threshold",
                "0",
                "-profile:v",
                "baseline",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                "-ar",
                "48000",
            ]);
        }

        cmd.args([
            "-movflags",
            "empty_moov+default_base_moof+frag_keyframe",
            "-f",
            "mp4",
            "pipe:1",
        ]);
        cmd.stdout(Stdio::piped()).stderr(Stdio::null());

        let mut ffmpeg_child = cmd.spawn()?;
        let mut stdout = ffmpeg_child
            .stdout
            .take()
            .ok_or("Failed to capture FFmpeg stdout pipe")?;

        println!("Piping FFmpeg live fMP4 stream into PackagingSession...");
        println!("Press Ctrl+C to terminate live packaging session.");

        let mut buf = vec![0u8; 65536];
        let mut chunk_count: u64 = 0;
        let mut stream_active = true;

        let duration_timer = self.config.max_duration.map(tokio::time::sleep);
        tokio::pin!(duration_timer);

        while stream_active {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    println!("\nShutdown signal received (Ctrl+C). Terminating live ingest...");
                    stream_active = false;
                }
                _ = async {
                    if let Some(ref mut timer) = duration_timer.as_mut().as_pin_mut() {
                        timer.await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    println!("Duration limit reached. Terminating live ingest...");
                    stream_active = false;
                }
                res = stdout.read(&mut buf) => {
                    match res {
                        Ok(0) => {
                            println!("FFmpeg stdout reached EOF.");
                            stream_active = false;
                        }
                        Ok(n) => {
                            if let Err(err) = session.push(Bytes::copy_from_slice(&buf[..n])).await {
                                eprintln!("Error pushing media fragment to GPAC: {err}");
                                stream_active = false;
                            }
                            chunk_count += 1;
                            if chunk_count.is_multiple_of(30) {
                                println!("Ingested {chunk_count} media chunks into live packager...");
                            }
                            if let Some(max) = self.config.max_chunks {
                                if chunk_count >= max {
                                    println!("Max chunks reached ({chunk_count}). Terminating live ingest...");
                                    stream_active = false;
                                }
                            }
                            if !session.is_alive() {
                                eprintln!("GPAC session terminated unexpectedly!");
                                stream_active = false;
                            }
                        }
                        Err(err) => {
                            eprintln!("Error reading from FFmpeg stdout: {err}");
                            stream_active = false;
                        }
                    }
                }
            }
        }

        println!("Stopping FFmpeg ingest process...");
        let _ = ffmpeg_child.kill().await;
        let _ = ffmpeg_child.wait().await;

        Ok(chunk_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_media_feeder_config_builder() {
        let config = MediaFeederConfig::new("scratch/test.mp4")
            .with_duration(Some(Duration::from_secs(5)))
            .with_max_chunks(Some(100))
            .with_force_transcode(true)
            .with_force_copy(false);

        assert_eq!(config.input_path, PathBuf::from("scratch/test.mp4"));
        assert_eq!(config.max_duration, Some(Duration::from_secs(5)));
        assert_eq!(config.max_chunks, Some(100));
        assert!(config.force_transcode);
        assert!(!config.force_copy);
    }

    #[tokio::test]
    async fn test_is_valid_media_file() {
        let tmp = std::env::temp_dir().join(format!("test_valid_media_{}", uuid::Uuid::new_v4()));
        assert!(!is_valid_media_file(&tmp).await);

        tokio::fs::write(&tmp, b"").await.unwrap();
        assert!(!is_valid_media_file(&tmp).await); // 0 bytes is not valid

        tokio::fs::write(&tmp, b"some-content").await.unwrap();
        assert!(is_valid_media_file(&tmp).await);

        let _ = tokio::fs::remove_file(&tmp).await;
    }
}
