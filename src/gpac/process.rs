use crate::error::{DrmpackError, Result};
use crate::types::LatencyMode;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tracing::{debug, error, info, instrument, warn};

/// Configuration for launching a GPAC packaging process.
#[derive(Debug, Clone)]
pub struct GpacProcessConfig {
    pub drm_xml_path: PathBuf,
    pub output_dir: PathBuf,
    pub latency_mode: LatencyMode,
    pub segment_duration: f64,
    pub chunk_duration: f64,
    pub gpac_bin: String,
}

impl GpacProcessConfig {
    pub fn new(drm_xml_path: impl Into<PathBuf>, output_dir: impl Into<PathBuf>) -> Self {
        Self {
            drm_xml_path: drm_xml_path.into(),
            output_dir: output_dir.into(),
            latency_mode: LatencyMode::LowLatency,
            segment_duration: 2.0,
            chunk_duration: 0.2,
            gpac_bin: "gpac".into(),
        }
    }

    pub fn with_latency_mode(mut self, mode: LatencyMode) -> Self {
        self.latency_mode = mode;
        self
    }

    pub fn with_segment_duration(mut self, duration: f64) -> Self {
        self.segment_duration = duration;
        self
    }

    pub fn with_chunk_duration(mut self, duration: f64) -> Self {
        self.chunk_duration = duration;
        self
    }

    pub fn with_gpac_bin(mut self, bin: impl Into<String>) -> Self {
        self.gpac_bin = bin.into();
        self
    }

    /// Build the command-line arguments for the `gpac` executable.
    pub fn build_args(&self) -> Vec<String> {
        let mut args = Vec::new();

        // 1. Input filter: read continuous fMP4 from stdin pipe without memory buffering delay
        args.push("-i".into());
        args.push("stdin:ext=mp4:mstore_samples=0:mstore_purge=0".into());

        // 2. Encryption filter: cecrypt with generated DRM XML
        args.push(format!("cecrypt:cfile={}", self.drm_xml_path.display()));

        // 3. Dasher output filter: generate both DASH and HLS manifests in output_dir
        let manifest_path = self.output_dir.join("live.mpd");
        let mut dasher_opt = format!(
            "{}:dual:profile=live:dmode=dynamic:segdur={}:pssh=mv",
            manifest_path.display(),
            self.segment_duration
        );

        if self.latency_mode == LatencyMode::LowLatency {
            let asto = (self.segment_duration - self.chunk_duration).max(0.1);
            dasher_opt.push_str(&format!(
                ":cdur={}:asto={:.1}:llhls=br:cmaf=cmfc",
                self.chunk_duration, asto
            ));
        }

        args.push("-o".into());
        args.push(dasher_opt);

        args
    }
}

/// Managed GPAC child process instance.
pub struct GpacProcess {
    config: GpacProcessConfig,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stderr_buffer: Arc<Mutex<Vec<String>>>,
}

impl GpacProcess {
    /// Spawn a new long-running GPAC subprocess with anonymous pipes.
    #[instrument(skip_all, fields(output_dir = %config.output_dir.display()))]
    pub async fn spawn(config: GpacProcessConfig) -> Result<Self> {
        let args = config.build_args();
        debug!(bin = %config.gpac_bin, args = ?args, "Spawning GPAC process");

        let mut cmd = Command::new(&config.gpac_bin);
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| {
            DrmpackError::Gpac(format!(
                "Failed to spawn GPAC binary '{}': {}. Please ensure GPAC is installed and in PATH.",
                config.gpac_bin, e
            ))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            DrmpackError::Gpac("Failed to capture stdin pipe for GPAC process".into())
        })?;

        let stderr = child.stderr.take().ok_or_else(|| {
            DrmpackError::Gpac("Failed to capture stderr pipe for GPAC process".into())
        })?;

        let stderr_buffer = Arc::new(Mutex::new(Vec::with_capacity(64)));
        let buffer_clone = Arc::clone(&stderr_buffer);

        // Background task to read stderr line-by-line and log to tracing
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                debug!(target: "gpac", "{}", line);
                let mut buf = buffer_clone.lock().unwrap();
                if buf.len() >= 64 {
                    buf.remove(0);
                }
                buf.push(line);
            }
        });

        info!(bin = %config.gpac_bin, "GPAC subprocess successfully spawned");

        Ok(Self {
            config,
            child: Some(child),
            stdin: Some(stdin),
            stderr_buffer,
        })
    }

    /// Write media segment bytes directly into GPAC's stdin pipe.
    pub async fn write_data(&mut self, data: &[u8]) -> Result<()> {
        self.check_status()?;

        let stderr_buffer = Arc::clone(&self.stderr_buffer);
        let get_stderr = move || {
            let buf = stderr_buffer.lock().unwrap();
            buf.join("\n")
        };

        if let Some(ref mut stdin) = self.stdin {
            stdin
                .write_all(data)
                .await
                .map_err(|e| DrmpackError::ProcessCrashed {
                    exit_code: None,
                    stderr: format!(
                        "Failed to write to GPAC stdin: {}. Stderr: {}",
                        e,
                        get_stderr()
                    ),
                })?;
            stdin
                .flush()
                .await
                .map_err(|e| DrmpackError::ProcessCrashed {
                    exit_code: None,
                    stderr: format!(
                        "Failed to flush GPAC stdin: {}. Stderr: {}",
                        e,
                        get_stderr()
                    ),
                })?;
            Ok(())
        } else {
            Err(DrmpackError::Session(
                "Cannot write data: GPAC stdin pipe is closed".into(),
            ))
        }
    }

    /// Check if the GPAC process has crashed or terminated unexpectedly.
    pub fn check_status(&mut self) -> Result<()> {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let stderr = self.get_recent_stderr();
                    let code = status.code();
                    if !status.success() {
                        error!(code = ?code, stderr = %stderr, "GPAC process crashed");
                        Err(DrmpackError::ProcessCrashed {
                            exit_code: code,
                            stderr,
                        })
                    } else {
                        Ok(())
                    }
                }
                Ok(None) => Ok(()), // Still running
                Err(e) => Err(DrmpackError::Gpac(format!(
                    "Failed to check GPAC child process status: {}",
                    e
                ))),
            }
        } else {
            Ok(())
        }
    }

    /// Get the most recent stderr log lines captured from the child process.
    pub fn get_recent_stderr(&self) -> String {
        let buf = self.stderr_buffer.lock().unwrap();
        buf.join("\n")
    }

    /// Gracefully close the stdin pipe and await GPAC completion.
    pub async fn close_and_wait(&mut self, timeout: Duration) -> Result<()> {
        // 1. Close stdin to signal EOF to GPAC
        self.stdin.take(); // Dropping ChildStdin closes the write pipe
        debug!("Closed GPAC stdin pipe, awaiting graceful finalization");

        // 2. Wait for process exit with timeout
        if let Some(mut child) = self.child.take() {
            let wait_future = child.wait();
            match tokio::time::timeout(timeout, wait_future).await {
                Ok(Ok(status)) => {
                    if status.success() {
                        info!("GPAC process exited successfully with status 0");
                        Ok(())
                    } else {
                        let stderr = self.get_recent_stderr();
                        error!(code = ?status.code(), stderr = %stderr, "GPAC exited with failure");
                        Err(DrmpackError::ProcessCrashed {
                            exit_code: status.code(),
                            stderr,
                        })
                    }
                }
                Ok(Err(e)) => Err(DrmpackError::Gpac(format!(
                    "Error awaiting GPAC exit: {}",
                    e
                ))),
                Err(_) => {
                    warn!(
                        "GPAC finalization timed out after {:?}, sending SIGKILL",
                        timeout
                    );
                    let _ = child.kill().await;
                    let stderr = self.get_recent_stderr();
                    Err(DrmpackError::ProcessCrashed {
                        exit_code: None,
                        stderr: format!(
                            "GPAC process timed out after {:?}. Stderr: {}",
                            timeout, stderr
                        ),
                    })
                }
            }
        } else {
            Ok(())
        }
    }

    /// Check if the child process is currently running.
    pub fn is_alive(&mut self) -> bool {
        if let Some(ref mut child) = self.child {
            matches!(child.try_wait(), Ok(None))
        } else {
            false
        }
    }

    /// Access the configuration.
    pub fn config(&self) -> &GpacProcessConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpac_process_config_args_low_latency() {
        let config = GpacProcessConfig::new("/tmp/drm.xml", "/dev/shm/test_stream")
            .with_latency_mode(LatencyMode::LowLatency)
            .with_segment_duration(2.0)
            .with_chunk_duration(0.2);

        let args = config.build_args();

        assert_eq!(args[0], "-i");
        assert_eq!(args[1], "stdin:ext=mp4:mstore_samples=0:mstore_purge=0");
        assert_eq!(args[2], "cecrypt:cfile=/tmp/drm.xml");
        assert_eq!(args[3], "-o");
        assert!(args[4].contains("/dev/shm/test_stream/live.mpd:dual"));
        assert!(args[4].contains("profile=live:dmode=dynamic:segdur=2:pssh=mv"));
        assert!(args[4].contains(":cdur=0.2:asto=1.8:llhls=br:cmaf=cmfc"));
    }

    #[test]
    fn test_gpac_process_config_args_standard_latency() {
        let config = GpacProcessConfig::new("/tmp/drm.xml", "/dev/shm/test_stream")
            .with_latency_mode(LatencyMode::Standard)
            .with_segment_duration(6.0);

        let args = config.build_args();

        assert_eq!(args[3], "-o");
        assert!(args[4].contains("segdur=6:pssh=mv"));
        // Standard latency does NOT contain LL-HLS or CMAF chunking flags
        assert!(!args[4].contains(":cdur="));
        assert!(!args[4].contains(":llhls="));
    }
}
