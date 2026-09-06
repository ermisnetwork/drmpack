use crate::error::{DrmpackError, Result};
use crate::types::LatencyMode;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, warn};

pub const DEFAULT_STDERR_RING_BUFFER_CAPACITY: usize = 64;

/// Log severity classification for GPAC stderr output lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSeverity {
    Error,
    Warn,
    Info,
    Debug,
}

/// Classify a stderr line into log severity for structured tracing emission.
///
/// Lines containing "error" or "failed to" are classified as `LogSeverity::Error`.
/// Lines containing "warning" are classified as `LogSeverity::Warn`.
/// Lines containing "info" are classified as `LogSeverity::Info`.
/// All other lines default to `LogSeverity::Debug`.
pub fn classify_log_severity(line: &str) -> LogSeverity {
    let lower = line.to_ascii_lowercase();
    if lower.contains("error") || lower.contains("failed to") || lower.contains("fatal") {
        LogSeverity::Error
    } else if lower.contains("warning") || lower.contains("warn") {
        LogSeverity::Warn
    } else if lower.contains("info") {
        LogSeverity::Info
    } else {
        LogSeverity::Debug
    }
}

/// Captured exit status of a GPAC child process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessExitStatus {
    pub code: Option<i32>,
    pub success: bool,
}

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

        // 0. Disable ANSI color codes for clean machine-readable log parsing
        args.push("-logs=ncl".into());

        // 1. Input filter: read continuous fMP4 from stdin pipe without memory buffering delay
        args.push("-i".into());
        args.push("stdin:ext=mp4:alltk:mstore_samples=0:mstore_purge=0".into());

        // 2. Encryption filter: cecrypt with generated DRM XML
        args.push(format!("cecrypt:cfile={}", self.drm_xml_path.display()));

        // 3. Dasher output filter: generate both DASH and HLS manifests in output_dir
        let manifest_path = self.output_dir.join("live.mpd");
        let mut dasher_opt = format!(
            "{}:dual:profile=live:dmode=dynauto:segdur={}:pssh=mv",
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

/// Managed GPAC child process instance monitored by a ProcessSupervisor task.
pub struct GpacProcess {
    config: GpacProcessConfig,
    stdin: Option<ChildStdin>,
    stderr_buffer: Arc<Mutex<VecDeque<String>>>,
    pub(crate) is_running: Arc<AtomicBool>,
    status_rx: watch::Receiver<Option<ProcessExitStatus>>,
    pub(crate) exit_tx: broadcast::Sender<ProcessExitStatus>,
    pub(crate) kill_token: CancellationToken,
    _supervisor_handle: Option<JoinHandle<()>>,
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

        let stderr_buffer = Arc::new(Mutex::new(VecDeque::with_capacity(
            DEFAULT_STDERR_RING_BUFFER_CAPACITY,
        )));
        let buffer_clone = Arc::clone(&stderr_buffer);

        // Background task to read stderr line-by-line using raw byte buffers
        // to prevent premature reader termination on non-UTF-8 bytes.
        let stderr_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line_bytes = Vec::new();
            loop {
                line_bytes.clear();
                match reader.read_until(b'\n', &mut line_bytes).await {
                    Ok(0) => break, // EOF reached
                    Ok(_) => {
                        let line_str = String::from_utf8_lossy(&line_bytes);
                        let trimmed = line_str.trim_end_matches(&['\r', '\n'][..]);
                        if !trimmed.is_empty() {
                            match classify_log_severity(trimmed) {
                                LogSeverity::Error => error!(target: "gpac", "{}", trimmed),
                                LogSeverity::Warn => warn!(target: "gpac", "{}", trimmed),
                                LogSeverity::Info => info!(target: "gpac", "{}", trimmed),
                                LogSeverity::Debug => debug!(target: "gpac", "{}", trimmed),
                            }
                            let mut buf = buffer_clone.lock().unwrap();
                            if buf.len() >= DEFAULT_STDERR_RING_BUFFER_CAPACITY {
                                buf.pop_front();
                            }
                            buf.push_back(trimmed.to_string());
                        }
                    }
                    Err(e) => {
                        debug!("Error reading GPAC stderr: {e}");
                        break;
                    }
                }
            }
        });

        let is_running = Arc::new(AtomicBool::new(true));
        let is_running_clone = Arc::clone(&is_running);
        let (status_tx, status_rx) = watch::channel(None);
        let (exit_tx, _) = broadcast::channel(16);
        let exit_tx_clone = exit_tx.clone();
        let kill_token = CancellationToken::new();
        let kill_token_clone = kill_token.clone();

        // Background ProcessSupervisor task owning child.wait()
        let supervisor_handle = tokio::spawn(async move {
            let exit_status = tokio::select! {
                res = child.wait() => {
                    match res {
                        Ok(status) => ProcessExitStatus {
                            code: status.code(),
                            success: status.success(),
                        },
                        Err(e) => {
                            error!("Error waiting on GPAC child: {e}");
                            ProcessExitStatus {
                                code: None,
                                success: false,
                            }
                        }
                    }
                }
                _ = kill_token_clone.cancelled() => {
                    debug!("ProcessSupervisor received kill signal; terminating child process");
                    let _ = child.start_kill();
                    let wait_res = child.wait().await;
                    ProcessExitStatus {
                        code: wait_res.ok().and_then(|s| s.code()),
                        success: false,
                    }
                }
            };

            // Allow stderr reader task to finish draining remaining buffered stderr lines from the pipe
            let _ = tokio::time::timeout(Duration::from_millis(200), stderr_handle).await;

            is_running_clone.store(false, Ordering::Release);
            let _ = status_tx.send(Some(exit_status.clone()));
            let _ = exit_tx_clone.send(exit_status);
        });

        info!(bin = %config.gpac_bin, "GPAC subprocess successfully spawned");

        Ok(Self {
            config,
            stdin: Some(stdin),
            stderr_buffer,
            is_running,
            status_rx,
            exit_tx,
            kill_token,
            _supervisor_handle: Some(supervisor_handle),
        })
    }

    /// Write media segment bytes directly into GPAC's stdin pipe.
    pub async fn write_data(&mut self, data: &[u8]) -> Result<()> {
        self.check_status()?;

        if let Some(ref mut stdin) = self.stdin {
            if let Err(e) = stdin.write_all(data).await {
                return Err(self.map_stdin_io_error("write to", e).await);
            }
            if let Err(e) = stdin.flush().await {
                return Err(self.map_stdin_io_error("flush", e).await);
            }
            Ok(())
        } else {
            Err(DrmpackError::Session(
                "Cannot write data: GPAC stdin pipe is closed".into(),
            ))
        }
    }

    async fn map_stdin_io_error(&self, op: &str, err: std::io::Error) -> DrmpackError {
        if self.status_rx.borrow().is_none() {
            let mut rx = self.status_rx.clone();
            let _ = tokio::time::timeout(Duration::from_millis(50), rx.changed()).await;
        }
        let exit_code = self.status_rx.borrow().as_ref().and_then(|s| s.code);
        let stderr = self.get_recent_stderr();
        DrmpackError::ProcessCrashed {
            exit_code,
            stderr: format!("Failed to {op} GPAC stdin: {err}. Stderr: {stderr}"),
        }
    }

    /// Check if the GPAC process has crashed or terminated unexpectedly.
    pub fn check_status(&self) -> Result<()> {
        if let Some(ref status) = *self.status_rx.borrow() {
            let stderr = self.get_recent_stderr();
            let code = status.code;
            error!(code = ?code, stderr = %stderr, "GPAC process exited unexpectedly");
            Err(DrmpackError::ProcessCrashed {
                exit_code: code,
                stderr: if status.success {
                    format!("GPAC exited successfully before PackagingSession::close(). Stderr: {stderr}")
                } else {
                    stderr
                },
            })
        } else {
            Ok(())
        }
    }

    /// Get the most recent stderr log lines captured from the child process.
    pub fn get_recent_stderr(&self) -> String {
        let buf = self.stderr_buffer.lock().unwrap();
        buf.iter().cloned().collect::<Vec<_>>().join("\n")
    }

    /// Gracefully close the stdin pipe and await GPAC completion.
    pub async fn close_and_wait(&mut self, timeout: Duration) -> Result<()> {
        // 1. Close stdin to signal EOF to GPAC
        self.stdin.take(); // Dropping ChildStdin closes the write pipe
        debug!("Closed GPAC stdin pipe, awaiting graceful finalization");

        // 2. Await ProcessSupervisor completion with timeout
        let mut rx = self.status_rx.clone();
        if rx.borrow().is_none() {
            let wait_exit = async {
                while rx.borrow().is_none() {
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
            };
            match tokio::time::timeout(timeout, wait_exit).await {
                Ok(_) => {}
                Err(_) => {
                    warn!(
                        "GPAC finalization timed out after {:?}, sending SIGKILL",
                        timeout
                    );
                    self.kill_token.cancel();
                    let _ = tokio::time::timeout(Duration::from_millis(500), rx.changed()).await;
                    let stderr = self.get_recent_stderr();
                    return Err(DrmpackError::ProcessCrashed {
                        exit_code: None,
                        stderr: format!(
                            "GPAC process timed out after {:?}. Stderr: {}",
                            timeout, stderr
                        ),
                    });
                }
            }
        }

        let status = rx.borrow().clone().unwrap_or(ProcessExitStatus {
            code: None,
            success: false,
        });

        if status.success {
            info!("GPAC process exited successfully with status 0");
            Ok(())
        } else {
            let stderr = self.get_recent_stderr();
            error!(code = ?status.code, stderr = %stderr, "GPAC exited with failure");
            Err(DrmpackError::ProcessCrashed {
                exit_code: status.code,
                stderr,
            })
        }
    }

    /// Check if the child process is currently running.
    pub fn is_alive(&self) -> bool {
        self.is_running.load(Ordering::Acquire)
    }

    /// Subscribe to exit notifications broadcast by the ProcessSupervisor.
    pub fn subscribe_exit(&self) -> broadcast::Receiver<ProcessExitStatus> {
        self.exit_tx.subscribe()
    }

    /// Terminate the child process immediately via SIGKILL.
    pub fn kill(&self) {
        self.kill_token.cancel();
    }

    /// Access the configuration.
    pub fn config(&self) -> &GpacProcessConfig {
        &self.config
    }
}

impl Drop for GpacProcess {
    fn drop(&mut self) {
        if self.is_running.load(Ordering::Acquire) {
            self.kill_token.cancel();
        }
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

        assert_eq!(args[0], "-logs=ncl");
        assert_eq!(args[1], "-i");
        assert_eq!(
            args[2],
            "stdin:ext=mp4:alltk:mstore_samples=0:mstore_purge=0"
        );
        assert_eq!(args[3], "cecrypt:cfile=/tmp/drm.xml");
        assert_eq!(args[4], "-o");
        assert!(args[5].contains("/dev/shm/test_stream/live.mpd:dual"));
        assert!(args[5].contains("profile=live:dmode=dynauto:segdur=2:pssh=mv"));
        assert!(args[5].contains(":cdur=0.2:asto=1.8:llhls=br:cmaf=cmfc"));
    }

    #[test]
    fn test_gpac_process_config_args_standard_latency() {
        let config = GpacProcessConfig::new("/tmp/drm.xml", "/dev/shm/test_stream")
            .with_latency_mode(LatencyMode::Standard)
            .with_segment_duration(6.0);

        let args = config.build_args();

        assert_eq!(args[0], "-logs=ncl");
        assert_eq!(args[4], "-o");
        assert!(args[5].contains("segdur=6:pssh=mv"));
        assert!(!args[5].contains(":cdur="));
        assert!(!args[5].contains(":llhls="));
    }

    #[test]
    fn test_classify_log_severity() {
        assert_eq!(
            classify_log_severity("[Error] Filter fout failed"),
            LogSeverity::Error
        );
        assert_eq!(
            classify_log_severity("Failed to setup socket connection"),
            LogSeverity::Error
        );
        assert_eq!(
            classify_log_severity("ERROR: bad packet"),
            LogSeverity::Error
        );
        assert_eq!(
            classify_log_severity("FAILED TO initialize context"),
            LogSeverity::Error
        );
        assert_eq!(
            classify_log_severity("[Warning] DTS is smaller than previous PTS"),
            LogSeverity::Warn
        );
        assert_eq!(
            classify_log_severity("[Warn] DTS is smaller than previous PTS"),
            LogSeverity::Warn
        );
        assert_eq!(
            classify_log_severity("warn: timestamp discontinuity"),
            LogSeverity::Warn
        );
        assert_eq!(
            classify_log_severity("FATAL error in filter chain"),
            LogSeverity::Error
        );
        assert_eq!(
            classify_log_severity("[Info] Initializing GPAC core"),
            LogSeverity::Info
        );
        assert_eq!(
            classify_log_severity("info: DASHER session created"),
            LogSeverity::Info
        );
        assert_eq!(
            classify_log_severity("GPAC filter engine version 26.07"),
            LogSeverity::Debug
        );
        assert_eq!(
            classify_log_severity("Processing segment 42"),
            LogSeverity::Debug
        );
    }

    #[test]
    fn test_bounded_circular_buffer() {
        let mut buf = VecDeque::with_capacity(3);
        let cap = 3;
        for i in 0..5 {
            if buf.len() >= cap {
                buf.pop_front();
            }
            buf.push_back(format!("line {i}"));
        }
        assert_eq!(buf.len(), 3);
        let slice: Vec<String> = buf.into_iter().collect();
        assert_eq!(slice, vec!["line 2", "line 3", "line 4"]);
    }
}
