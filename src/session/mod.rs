pub mod cluster;
pub use cluster::{Representation, RepresentationCluster};
pub mod harvester;
use harvester::Harvester;
pub mod metadata;
pub use metadata::{DrmKeyEntry, DrmStreamMetadata};

use crate::error::{
    DrmpackError, PackagingOperation, PackagingSessionFailure, RepresentationFailure, Result,
};
use crate::key::{KeyPolicyEngine, KeyProvider, KeySet};
use crate::types::{
    DrmSystem, EncryptionScheme, KeyMappingPolicy, LatencyMode, ManifestFormat, PackagedArtifact,
    Rendition, TrackType,
};
use bytes::Bytes;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio_util::sync::{CancellationToken, PollSender};
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

const DEFAULT_FINALIZATION_TIMEOUT: Duration = Duration::from_secs(5);
const WATCHDOG_FINALIZATION_TIMEOUT: Duration = Duration::from_secs(2);

pub const DEFAULT_SEGMENT_DURATION: f64 = 2.0;
pub const DEFAULT_CHUNK_DURATION: f64 = 0.2;
pub const MIN_SEGMENT_DURATION: f64 = 0.5;
pub const MAX_SEGMENT_DURATION: f64 = 30.0;
pub use crate::gpac::process::DEFAULT_TIME_SHIFT_BUFFER;

/// Default storage staging directory for packaging output.
fn default_output_dir(content_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("drmpack_{content_id}_{}", Uuid::new_v4()))
}

/// Configuration for creating a `PackagingSession`.
#[derive(Debug, Clone)]
pub struct PackagingSessionConfig {
    pub content_id: String,
    pub renditions: Vec<Rendition>,
    /// The effective encryption mode. `Dual` creates one CENC and one CBCS Representation.
    pub encryption_scheme: EncryptionScheme,
    pub drm_systems: Vec<DrmSystem>,
    pub latency_mode: LatencyMode,
    pub segment_duration: f64,
    pub chunk_duration: f64,
    pub availability_time_offset: Option<f64>,
    pub time_shift_buffer: Duration,
    pub output_dir: PathBuf,
    pub is_custom_output_dir: bool,
    pub preserve_output: bool,
    /// Parent directory for private, session-scoped GPAC DRM XML files.
    pub control_dir: Option<PathBuf>,
    /// Inactivity timeout for input. This is distinct from `finalization_timeout`.
    pub session_timeout: Option<Duration>,
    /// Per-Representation deadline for GPAC finalization after stdin closes.
    pub finalization_timeout: Duration,
    pub gpac_bin: Option<String>,
    pub key_mapping_policy: KeyMappingPolicy,
}

impl PackagingSessionConfig {
    pub fn new(content_id: impl Into<String>) -> Self {
        let cid = content_id.into();
        Self {
            output_dir: default_output_dir(&cid),
            is_custom_output_dir: false,
            preserve_output: false,
            content_id: cid,
            renditions: Vec::new(),
            encryption_scheme: EncryptionScheme::Cbcs,
            drm_systems: Vec::new(),
            latency_mode: LatencyMode::Standard,
            segment_duration: DEFAULT_SEGMENT_DURATION,
            chunk_duration: DEFAULT_CHUNK_DURATION,
            availability_time_offset: None,
            time_shift_buffer: DEFAULT_TIME_SHIFT_BUFFER,
            control_dir: None,
            session_timeout: None,
            finalization_timeout: DEFAULT_FINALIZATION_TIMEOUT,
            gpac_bin: None,
            key_mapping_policy: KeyMappingPolicy::default(),
        }
    }

    /// Preconfigured preset for standard live CENC streaming (Widevine + PlayReady, LowLatency).
    pub fn cenc(content_id: impl Into<String>) -> Self {
        let mut s = Self::new(content_id);
        s.encryption_scheme = EncryptionScheme::Cenc;
        s.drm_systems = vec![DrmSystem::Widevine, DrmSystem::PlayReady];
        s.latency_mode = LatencyMode::LowLatency;
        s
    }

    /// Preconfigured preset for standard live CBCS streaming (FairPlay + Widevine + PlayReady, Standard latency).
    pub fn cbcs(content_id: impl Into<String>) -> Self {
        let mut s = Self::new(content_id);
        s.drm_systems = vec![
            DrmSystem::FairPlay,
            DrmSystem::Widevine,
            DrmSystem::PlayReady,
        ];
        s
    }

    /// Preconfigured preset for standard live dual-scheme streaming (CENC + CBCS, Widevine + FairPlay + PlayReady, Standard latency).
    pub fn dual(content_id: impl Into<String>) -> Self {
        let mut s = Self::new(content_id);
        s.encryption_scheme = EncryptionScheme::Dual;
        s.drm_systems = vec![
            DrmSystem::Widevine,
            DrmSystem::FairPlay,
            DrmSystem::PlayReady,
        ];
        s
    }

    /// Preconfigured preset for dual-scheme low-latency streaming (CENC + CBCS, Widevine + FairPlay + PlayReady).
    pub fn low_latency_dual(content_id: impl Into<String>) -> Self {
        let mut s = Self::new(content_id);
        s.encryption_scheme = EncryptionScheme::Dual;
        s.drm_systems = vec![
            DrmSystem::Widevine,
            DrmSystem::FairPlay,
            DrmSystem::PlayReady,
        ];
        s.latency_mode = LatencyMode::LowLatency;
        s
    }

    pub fn with_rendition(mut self, rendition: Rendition) -> Self {
        self.renditions.push(rendition);
        self
    }

    pub fn with_renditions(mut self, renditions: impl IntoIterator<Item = Rendition>) -> Self {
        self.renditions.extend(renditions);
        self
    }

    /// Enable all primary DRM systems (Widevine, FairPlay, PlayReady).
    pub fn with_all_drm(mut self) -> Self {
        for drm in [
            DrmSystem::Widevine,
            DrmSystem::FairPlay,
            DrmSystem::PlayReady,
        ] {
            if !self.drm_systems.contains(&drm) {
                self.drm_systems.push(drm);
            }
        }
        self
    }

    pub fn with_key_mapping_policy(mut self, policy: KeyMappingPolicy) -> Self {
        self.key_mapping_policy = policy;
        self
    }

    /// Set the effective encryption mode. The most recent call wins.
    pub fn with_encryption_scheme(mut self, scheme: EncryptionScheme) -> Self {
        self.encryption_scheme = scheme;
        self
    }

    pub fn with_drm_system(mut self, drm: DrmSystem) -> Self {
        if !self.drm_systems.contains(&drm) {
            self.drm_systems.push(drm);
        }
        self
    }

    pub fn with_latency_mode(mut self, mode: LatencyMode) -> Self {
        self.latency_mode = mode;
        self
    }

    /// Sets the target duration for media segments in seconds (default: 2.0s).
    ///
    /// Must be between 0.5s and 30.0s, and strictly greater than `chunk_duration`.
    /// Integer durations (e.g. 1.0, 2.0, 4.0, 6.0) aligned with the upstream encoder's
    /// GOP size are strongly recommended.
    pub fn with_segment_duration(mut self, duration: f64) -> Self {
        self.segment_duration = duration;
        self
    }

    /// Sets the CMAF chunk duration for low-latency streaming in seconds (default: 0.2s).
    ///
    /// Must be finite, greater than zero, and strictly less than `segment_duration`.
    pub fn with_chunk_duration(mut self, duration: f64) -> Self {
        self.chunk_duration = duration;
        self
    }

    /// Sets an explicit availability time offset (`asto`) for Low-Latency DASH.
    ///
    /// When `None` (default), GPAC does not signal `@availabilityTimeOffset` (or uses `asto=0`),
    /// ensuring standard file-based HTTP origins do not return 404 on live-edge requests.
    /// When running a true Chunked Transfer Encoding (CTE) streaming origin, set this to
    /// e.g. `Some(segment_duration - chunk_duration)`.
    pub fn with_availability_time_offset(mut self, asto: impl Into<Option<f64>>) -> Self {
        self.availability_time_offset = asto.into();
        self
    }

    /// Sets the time-shift buffer (DVR sliding window) duration (default: 60s).
    pub fn with_time_shift_buffer(mut self, buffer: Duration) -> Self {
        self.time_shift_buffer = buffer;
        self
    }

    pub fn with_output_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.output_dir = dir.into();
        self.is_custom_output_dir = true;
        self
    }

    /// Preserve output files upon Drop, preventing automatic deletion of auto-allocated directories.
    pub fn preserve_output(mut self) -> Self {
        self.preserve_output = true;
        self
    }

    /// Set the parent directory used to create a private, opaque control directory.
    pub fn with_control_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.control_dir = Some(dir.into());
        self
    }

    pub fn with_session_timeout(mut self, timeout: Duration) -> Self {
        self.session_timeout = Some(timeout);
        self
    }

    pub fn with_finalization_timeout(mut self, timeout: Duration) -> Self {
        self.finalization_timeout = timeout;
        self
    }

    pub fn with_gpac_bin(mut self, bin: impl Into<String>) -> Self {
        self.gpac_bin = Some(bin.into());
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    Active,
    Closing,
    Closed,
    Failed,
}

#[derive(Debug)]
struct Lifecycle {
    state: SessionState,
    terminal_failure: Option<Arc<PackagingSessionFailure>>,
}

impl Lifecycle {
    fn new() -> Self {
        Self {
            state: SessionState::Active,
            terminal_failure: None,
        }
    }

    fn failure_error(&self) -> DrmpackError {
        self.terminal_failure
            .as_ref()
            .map(|failure| DrmpackError::PackagingSession(Arc::clone(failure)))
            .unwrap_or_else(|| DrmpackError::Session("PackagingSession has failed".into()))
    }
}

/// Output manifest paths and packaging statistics returned by `PackagingSession::run_to_completion`.
#[derive(Debug, Clone)]
pub struct PackagingResult {
    pub segments_ingested: u64,
    pub output_dir: PathBuf,
    pub manifests: Vec<PathBuf>,
    pub hls_manifest: Option<PathBuf>,
    pub dash_manifest: Option<PathBuf>,
    pub cenc_hls: Option<PathBuf>,
    pub cenc_dash: Option<PathBuf>,
    pub cbcs_hls: Option<PathBuf>,
    pub cbcs_dash: Option<PathBuf>,
}

impl PackagingResult {
    pub fn manifest_path(
        &self,
        scheme: EncryptionScheme,
        format: ManifestFormat,
    ) -> Option<PathBuf> {
        match (scheme, format) {
            (EncryptionScheme::Cenc, ManifestFormat::Hls) => {
                self.cenc_hls.clone().or_else(|| self.hls_manifest.clone())
            }
            (EncryptionScheme::Cenc, ManifestFormat::Dash) => self
                .cenc_dash
                .clone()
                .or_else(|| self.dash_manifest.clone()),
            (EncryptionScheme::Cbcs, ManifestFormat::Hls) => {
                self.cbcs_hls.clone().or_else(|| self.hls_manifest.clone())
            }
            (EncryptionScheme::Cbcs, ManifestFormat::Dash) => self
                .cbcs_dash
                .clone()
                .or_else(|| self.dash_manifest.clone()),
            (EncryptionScheme::Dual, _) => None,
        }
    }

    pub async fn cleanup(&self) -> Result<()> {
        if self.output_dir.exists() {
            tokio::fs::remove_dir_all(&self.output_dir)
                .await
                .map_err(|error| {
                    DrmpackError::Io(std::io::Error::new(
                        error.kind(),
                        format!(
                            "Failed to clean up Ramdisk output directory '{}': {error}",
                            self.output_dir.display()
                        ),
                    ))
                })?;
        }
        Ok(())
    }
}

/// A stateful packaging session that orchestrates DRM key acquisition,
/// GPAC child process lifecycle, and low-latency manifest/chunk generation into Ramdisk.
///
/// # Dual Scheme-Aware Keys
/// `EncryptionScheme::Dual` queries scheme-aware keys for both CENC and CBCS Representations
/// (`KeyRequest::encryption_schemes`). When supplied by a scheme-aware KeyProvider (such as CPIX),
/// each Representation receives distinct ContentKeys and KIDs per ADR-0006.
pub struct PackagingSession {
    config: PackagingSessionConfig,
    key_set: KeySet,
    cluster: Arc<RepresentationCluster>,
    control_dir: PathBuf,
    lifecycle: Arc<Mutex<Lifecycle>>,
    is_terminal: Arc<AtomicBool>,
    has_pushed_media: Arc<AtomicBool>,
    heartbeat_tx: Option<mpsc::Sender<()>>,
    cancellation_token: CancellationToken,
    watchdog_handle: Option<JoinHandle<()>>,
    preserve_output: bool,
    harvester: Option<Harvester>,
    output_receiver_claimed: bool,
}

impl std::fmt::Debug for PackagingSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackagingSession")
            .field("config", &self.config)
            .field("representations", &self.cluster.schemes())
            .finish_non_exhaustive()
    }
}

impl PackagingSession {
    /// Create a new packaging session, fetching its KeySet once and spawning one GPAC process
    /// per concrete Representation.
    #[instrument(skip(key_provider), fields(content_id = %config.content_id))]
    pub async fn create<P: KeyProvider>(
        config: PackagingSessionConfig,
        key_provider: &P,
    ) -> Result<Self> {
        validate_config(&config)?;
        let key_set = fetch_key_set(&config, key_provider).await?;
        let is_dual = config.encryption_scheme == EncryptionScheme::Dual;
        let output_dir_created = prepare_output_dir(&config.output_dir, is_dual).await?;
        let control_dir = match create_control_dir(&config).await {
            Ok(control_dir) => control_dir,
            Err(error) => {
                if output_dir_created {
                    let _ = tokio::fs::remove_dir_all(&config.output_dir).await;
                }
                return Err(error);
            }
        };

        let cluster = Arc::new(
            RepresentationCluster::spawn(&config, &key_set, &control_dir, output_dir_created)
                .await?,
        );

        let lifecycle = Arc::new(Mutex::new(Lifecycle::new()));
        let is_terminal = Arc::new(AtomicBool::new(false));
        let has_pushed_media = Arc::new(AtomicBool::new(false));
        let cancellation_token = CancellationToken::new();
        let (heartbeat_tx, watchdog_handle) = build_watchdog(WatchdogContext {
            timeout: config.session_timeout,
            finalization_timeout: config.finalization_timeout,
            control_dir: control_dir.clone(),
            lifecycle: Arc::clone(&lifecycle),
            is_terminal: Arc::clone(&is_terminal),
            cluster: Arc::clone(&cluster),
            cancellation_token: cancellation_token.clone(),
        });

        let preserve_output = config.preserve_output;

        Ok(Self {
            config,
            key_set,
            cluster,
            control_dir,
            lifecycle,
            is_terminal,
            has_pushed_media,
            heartbeat_tx,
            cancellation_token,
            watchdog_handle,
            preserve_output,
            harvester: None,
            output_receiver_claimed: false,
        })
    }

    fn ping_heartbeat(&self) {
        if let Some(tx) = &self.heartbeat_tx {
            let _ = tx.try_send(());
        }
    }

    /// Retrieve the asynchronous output channel receiver yielding packaged artifacts.
    ///
    /// Single-ownership semantics: returns `Some(receiver)` on first invocation,
    /// and `None` on all subsequent invocations.
    /// When claimed, activates an asynchronous background harvester that monitors
    /// the session staging directory, emits artifacts (init segments, media segments,
    /// and manifests) into memory, and immediately unlinks consumed segment files
    /// (ephemeral staging). Zero overhead if not claimed.
    pub fn take_output_receiver(&mut self) -> Option<mpsc::Receiver<PackagedArtifact>> {
        if self.output_receiver_claimed || self.is_closed() {
            return None;
        }
        self.output_receiver_claimed = true;

        let (tx, rx) = mpsc::channel(1024);
        let is_dual = self.config.encryption_scheme == EncryptionScheme::Dual;
        let targets = if is_dual {
            vec![
                (self.config.output_dir.join("cenc"), EncryptionScheme::Cenc),
                (self.config.output_dir.join("cbcs"), EncryptionScheme::Cbcs),
            ]
        } else {
            vec![(
                self.config.output_dir.clone(),
                self.config.encryption_scheme,
            )]
        };

        let harvester = Harvester::spawn(targets, tx);
        self.harvester = Some(harvester);
        Some(rx)
    }

    /// Create an [`AsyncWrite`](tokio::io::AsyncWrite) handle for this session.
    ///
    /// Spawns an internal forwarding task that replicates [`push()`](Self::push) semantics
    /// (lifecycle checks, heartbeat, cluster fan-out). Call [`SessionWriter::close()`] to
    /// drain buffered bytes, or simply drop the writer to detach.
    ///
    /// ```no_run
    /// # async fn example(session: &drmpack::PackagingSession, reader: &mut (impl tokio::io::AsyncRead + Unpin)) {
    /// let mut writer = session.writer();
    /// tokio::io::copy(reader, &mut writer).await.unwrap();
    /// writer.close().await;
    /// # }
    /// ```
    pub fn writer(&self) -> SessionWriter {
        let (tx, mut rx) = mpsc::channel::<Bytes>(64);
        let cluster = Arc::clone(&self.cluster);
        let is_terminal = Arc::clone(&self.is_terminal);
        let cancellation_token = self.cancellation_token.clone();
        let heartbeat_tx = self.heartbeat_tx.clone();
        let has_pushed_media = Arc::clone(&self.has_pushed_media);

        let forward_task = tokio::spawn(async move {
            while let Some(bytes) = rx.recv().await {
                if is_terminal.load(Ordering::Acquire) || cancellation_token.is_cancelled() {
                    break;
                }
                if let Some(ref htx) = heartbeat_tx {
                    let _ = htx.try_send(());
                }
                if bytes.windows(4).any(|w| w == b"moof") {
                    has_pushed_media.store(true, Ordering::Release);
                }
                if !cluster.write_data(&bytes).await.is_empty() {
                    break;
                }
            }
        });

        SessionWriter {
            sender: PollSender::new(tx),
            forward_task: Some(forward_task),
        }
    }

    /// Mark output files to be preserved upon Drop, preventing automatic cleanup.
    pub fn preserve_output(&mut self) {
        self.preserve_output = true;
    }

    /// Push raw media bytes or chunk to every active Representation.
    /// Accepts `&[u8]`, `Vec<u8>`, `Bytes`, etc. with zero heap allocations.
    #[instrument(skip(self, bytes), fields(len = bytes.as_ref().len()))]
    pub async fn push(&mut self, bytes: impl AsRef<[u8]>) -> Result<()> {
        let slice = bytes.as_ref();
        let is_media = slice.windows(4).any(|w| w == b"moof");
        self.push_data(slice, is_media).await
    }

    /// Stream media chunks from an async channel to stdin until EOF, returning total chunk count.
    pub async fn ingest_stream(&mut self, mut rx: mpsc::Receiver<Bytes>) -> Result<u64> {
        let mut count = 0u64;
        while let Some(chunk) = rx.recv().await {
            self.push(chunk).await?;
            count += 1;
        }
        Ok(count)
    }

    /// Ingest an entire stream channel and cleanly close the session, returning manifest paths.
    pub async fn run_to_completion(mut self, rx: mpsc::Receiver<Bytes>) -> Result<PackagingResult> {
        let segments_ingested = self.ingest_stream(rx).await?;
        let output_dir = self.config.output_dir.clone();
        let is_dual = self.config.encryption_scheme == EncryptionScheme::Dual;

        let (hls_manifest, dash_manifest, cenc_hls, cenc_dash, cbcs_hls, cbcs_dash, manifests) =
            if is_dual {
                let c_hls = output_dir.join("cenc").join("live.m3u8");
                let c_dash = output_dir.join("cenc").join("live.mpd");
                let cb_hls = output_dir.join("cbcs").join("live.m3u8");
                let cb_dash = output_dir.join("cbcs").join("live.mpd");
                let all = vec![
                    c_hls.clone(),
                    c_dash.clone(),
                    cb_hls.clone(),
                    cb_dash.clone(),
                ];
                (
                    None,
                    None,
                    Some(c_hls),
                    Some(c_dash),
                    Some(cb_hls),
                    Some(cb_dash),
                    all,
                )
            } else {
                let hls = output_dir.join("live.m3u8");
                let dash = output_dir.join("live.mpd");
                let all = vec![hls.clone(), dash.clone()];
                (Some(hls), Some(dash), None, None, None, None, all)
            };

        self.close().await?;
        self.preserve_output = true;

        Ok(PackagingResult {
            segments_ingested,
            output_dir,
            manifests,
            hls_manifest,
            dash_manifest,
            cenc_hls,
            cenc_dash,
            cbcs_hls,
            cbcs_dash,
        })
    }

    async fn push_data(&mut self, bytes: &[u8], is_media: bool) -> Result<()> {
        self.ensure_active().await?;
        self.ping_heartbeat();
        if is_media {
            self.has_pushed_media.store(true, Ordering::Release);
        }

        let failures = self.cluster.write_data(bytes).await;
        self.finish_operation_failures(failures).await
    }

    /// Check every Representation for an unexpected exit.
    pub async fn check_status(&self) -> Result<()> {
        {
            let lifecycle = self.lifecycle.lock().await;
            if lifecycle.state == SessionState::Failed {
                return Err(lifecycle.failure_error());
            }
            if lifecycle.state != SessionState::Active {
                return Err(DrmpackError::Session("PackagingSession is closed".into()));
            }
        }

        let failures = self.cluster.check_status().await;
        self.finish_operation_failures(failures).await
    }

    /// Gracefully finalize every Representation. Both subprocesses are always attempted,
    /// even when an earlier finalization fails. Output files on Ramdisk are preserved.
    #[instrument(skip(self))]
    pub async fn close(&mut self) -> Result<()> {
        self.cancellation_token.cancel();

        let mut lifecycle = self.lifecycle.lock().await;
        match lifecycle.state {
            SessionState::Closed => return Ok(()),
            SessionState::Closing => return Ok(()),
            SessionState::Failed => {
                let err = lifecycle.failure_error();
                drop(lifecycle);
                let _ = self.cleanup_control_dir().await;
                self.is_terminal.store(true, Ordering::Release);
                return Err(err);
            }
            SessionState::Active => {
                lifecycle.state = SessionState::Closing;
            }
        }

        self.stop_watchdog();
        let mut failures = self
            .cluster
            .close(self.config.finalization_timeout, PackagingOperation::Close)
            .await;

        // If media segments were pushed, verify #EXT-X-ENDLIST in HLS manifests
        if failures.is_empty() && self.has_pushed_media.load(Ordering::Acquire) {
            let is_dual = self.config.encryption_scheme == EncryptionScheme::Dual;
            for rep in self.cluster.representations() {
                let rep_output_dir = if is_dual {
                    self.config.output_dir.join(rep.scheme.to_string())
                } else {
                    self.config.output_dir.clone()
                };
                if let Err(err) = verify_hls_endlist(&rep_output_dir).await {
                    failures.push(RepresentationFailure::new(
                        rep.scheme,
                        PackagingOperation::Close,
                        err,
                    ));
                }
            }
        }

        if let Some(harvester) = self.harvester.take() {
            harvester.finish_and_flush().await;
        }

        let control_cleanup = self.cleanup_control_dir().await.err();

        if failures.is_empty() && control_cleanup.is_none() {
            lifecycle.state = SessionState::Closed;
            self.is_terminal.store(true, Ordering::Release);
            info!("PackagingSession closed successfully");
            Ok(())
        } else {
            lifecycle.state = SessionState::Failed;
            let failure = Arc::new(
                PackagingSessionFailure::from_failures(failures)
                    .with_cleanup_failures(None, control_cleanup),
            );
            lifecycle.terminal_failure = Some(Arc::clone(&failure));
            self.is_terminal.store(true, Ordering::Release);
            Err(DrmpackError::PackagingSession(failure))
        }
    }

    /// Resolve a deterministic public Manifest path without waiting for GPAC to write it.
    pub fn manifest_path(
        &self,
        scheme: EncryptionScheme,
        format: ManifestFormat,
    ) -> Result<PathBuf> {
        let is_dual = self.config.encryption_scheme == EncryptionScheme::Dual;
        if scheme == EncryptionScheme::Dual || !self.cluster.has_scheme(scheme) {
            return Err(DrmpackError::InvalidConfig(format!(
                "{} is not a Representation in this PackagingSession",
                scheme
            )));
        }

        let output_dir = if is_dual {
            self.config.output_dir.join(scheme.to_string())
        } else {
            self.config.output_dir.clone()
        };
        Ok(match format {
            ManifestFormat::Dash => output_dir.join("live.mpd"),
            ManifestFormat::Hls => output_dir.join("live.m3u8"),
        })
    }

    /// Expected HLS master Manifest for a single-scheme session.
    /// Returns an error for Dual sessions; call `manifest_path` with a concrete scheme instead.
    pub fn hls_manifest_path(&self) -> Result<PathBuf> {
        self.single_scheme_manifest_path(ManifestFormat::Hls)
    }

    /// Expected DASH Manifest for a single-scheme session.
    /// Returns an error for Dual sessions; call `manifest_path` with a concrete scheme instead.
    pub fn dash_manifest_path(&self) -> Result<PathBuf> {
        self.single_scheme_manifest_path(ManifestFormat::Dash)
    }

    /// Remove delivery output after the session has reached a terminal lifecycle state.
    pub async fn cleanup(&self) -> Result<()> {
        let state = self.lifecycle.lock().await.state;
        if matches!(state, SessionState::Active | SessionState::Closing) {
            return Err(DrmpackError::Session(
                "Cannot clean up an active PackagingSession".into(),
            ));
        }
        self.cleanup_output_dir().await
    }

    /// Check whether the session has reached a terminal lifecycle state.
    pub fn is_closed(&self) -> bool {
        self.is_terminal.load(Ordering::Acquire)
    }

    /// Check whether the session is currently active and healthy.
    ///
    /// Returns true if and only if the session has not reached terminal state,
    /// is not cancelled, and all underlying GPAC representations are currently running.
    pub fn is_alive(&self) -> bool {
        !self.is_terminal.load(Ordering::Acquire)
            && !self.cancellation_token.is_cancelled()
            && self.cluster.is_alive()
    }

    /// Access the session configuration.
    pub fn config(&self) -> &PackagingSessionConfig {
        &self.config
    }

    /// Access the cached KeySet. The in-process control plane remains trusted.
    pub fn key_set(&self) -> &KeySet {
        &self.key_set
    }

    /// Return the public DRM stream metadata for this packaging session.
    ///
    /// Suitable for application-level state persistence (e.g. PostgreSQL, Redis) or
    /// transfer to a playback authorization backend.
    /// Strictly excludes secret keys while preserving KIDs, IVs, track bindings,
    /// and encryption schemes needed to issue playback entitlement tokens (ADR-0017).
    pub fn playback_metadata(&self) -> DrmStreamMetadata {
        DrmStreamMetadata::from_session(
            &self.config.content_id,
            self.config.encryption_scheme,
            &self.key_set,
        )
    }

    /// Root of the Ramdisk delivery output. Private DRM XML remains outside this directory.
    pub fn output_dir(&self) -> &Path {
        &self.config.output_dir
    }

    async fn ensure_active(&self) -> Result<()> {
        if self.cancellation_token.is_cancelled() {
            let lifecycle = self.lifecycle.lock().await;
            return match lifecycle.state {
                SessionState::Failed => Err(lifecycle.failure_error()),
                _ => Err(DrmpackError::Session("PackagingSession is closed".into())),
            };
        }

        let lifecycle = self.lifecycle.lock().await;
        match lifecycle.state {
            SessionState::Active => Ok(()),
            SessionState::Failed => Err(lifecycle.failure_error()),
            SessionState::Closing | SessionState::Closed => {
                Err(DrmpackError::Session("PackagingSession is closed".into()))
            }
        }
    }

    async fn finish_operation_failures(&self, failures: Vec<RepresentationFailure>) -> Result<()> {
        if failures.is_empty() {
            Ok(())
        } else {
            Err(self.fail_close(failures).await)
        }
    }

    async fn fail_close(&self, mut failures: Vec<RepresentationFailure>) -> DrmpackError {
        self.stop_watchdog();
        self.cancellation_token.cancel();

        let mut lifecycle = self.lifecycle.lock().await;
        if lifecycle.state == SessionState::Failed {
            return lifecycle.failure_error();
        }
        lifecycle.state = SessionState::Failed;

        failures.extend(
            self.cluster
                .close(self.config.finalization_timeout, PackagingOperation::Close)
                .await,
        );
        let control_cleanup = self.cleanup_control_dir().await.err();
        let failure = Arc::new(
            PackagingSessionFailure::from_failures(failures)
                .with_cleanup_failures(None, control_cleanup),
        );
        lifecycle.terminal_failure = Some(Arc::clone(&failure));
        self.is_terminal.store(true, Ordering::Release);
        DrmpackError::PackagingSession(failure)
    }

    /// Access active representations managed in this cluster.
    pub fn representations(&self) -> &[Representation] {
        self.cluster.representations()
    }

    fn stop_watchdog(&self) {
        self.cancellation_token.cancel();
    }

    fn single_scheme_manifest_path(&self, format: ManifestFormat) -> Result<PathBuf> {
        if self.config.encryption_scheme == EncryptionScheme::Dual {
            return Err(DrmpackError::InvalidConfig(
                "A Dual PackagingSession requires a concrete EncryptionScheme when resolving a Manifest"
                    .into(),
            ));
        }
        self.manifest_path(self.config.encryption_scheme, format)
    }

    async fn cleanup_output_dir(&self) -> Result<()> {
        if self.config.output_dir.exists() {
            tokio::fs::remove_dir_all(&self.config.output_dir)
                .await
                .map_err(|error| {
                    DrmpackError::Io(std::io::Error::new(
                        error.kind(),
                        format!(
                            "Failed to clean up Ramdisk output directory '{}': {error}",
                            self.config.output_dir.display()
                        ),
                    ))
                })?;
            debug!(path = %self.config.output_dir.display(), "Cleaned up Ramdisk session directory");
        }
        Ok(())
    }

    async fn cleanup_control_dir(&self) -> Result<()> {
        if self.control_dir.exists() {
            tokio::fs::remove_dir_all(&self.control_dir)
                .await
                .map_err(|error| {
                    warn!(path = %self.control_dir.display(), %error, "Failed to remove private control directory");
                    DrmpackError::Io(std::io::Error::new(
                        error.kind(),
                        format!(
                            "Failed to remove private control directory '{}': {error}",
                            self.control_dir.display()
                        ),
                    ))
                })?;
            if let Some(parent) = self.control_dir.parent() {
                if parent.file_name().and_then(|n| n.to_str()) == Some("drmpack-control") {
                    let _ = tokio::fs::remove_dir(parent).await;
                }
            }
        }
        Ok(())
    }

    /// Path to the session's private control directory containing GPAC DRM XML definitions.
    pub fn control_dir_path(&self) -> &Path {
        &self.control_dir
    }
}

/// An owned write handle implementing [`tokio::io::AsyncWrite`].
///
/// Created via [`PackagingSession::writer()`]. Internally backed by a bounded
/// channel and a forwarding task that replicates `push()` semantics.
pub struct SessionWriter {
    sender: PollSender<Bytes>,
    forward_task: Option<JoinHandle<()>>,
}

impl SessionWriter {
    /// Shut down the writer and drain any buffered bytes to the session.
    pub async fn close(mut self) {
        self.sender.close();
        if let Some(task) = self.forward_task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for SessionWriter {
    fn drop(&mut self) {
        self.sender.close();
    }
}

impl tokio::io::AsyncWrite for SessionWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        match this.sender.poll_reserve(cx) {
            Poll::Ready(Ok(())) => {
                let len = buf.len();
                this.sender
                    .send_item(Bytes::copy_from_slice(buf))
                    .map_err(|_| {
                        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "session closed")
                    })?;
                Poll::Ready(Ok(len))
            }
            Poll::Ready(Err(_)) => Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "session closed",
            ))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        self.get_mut().sender.close();
        Poll::Ready(Ok(()))
    }
}

impl Drop for PackagingSession {
    fn drop(&mut self) {
        self.cancellation_token.cancel();
        if let Some(mut harvester) = self.harvester.take() {
            harvester.cancel();
        }
        if let Some(handle) = &self.watchdog_handle {
            handle.abort();
        }
        if self.control_dir.exists() {
            if let Err(error) = std::fs::remove_dir_all(&self.control_dir) {
                debug!(path = %self.control_dir.display(), %error, "Failed to remove private control directory during drop");
            }
            if let Some(parent) = self.control_dir.parent() {
                if parent.file_name().and_then(|n| n.to_str()) == Some("drmpack-control") {
                    let _ = std::fs::remove_dir(parent);
                }
            }
        }
        if !self.config.is_custom_output_dir
            && !self.preserve_output
            && self.config.output_dir.exists()
        {
            if let Err(error) = std::fs::remove_dir_all(&self.config.output_dir) {
                debug!(path = %self.config.output_dir.display(), %error, "Failed to remove output directory during drop");
            }
        }
    }
}

fn validate_config(config: &PackagingSessionConfig) -> Result<()> {
    if config.renditions.is_empty() {
        return Err(DrmpackError::InvalidConfig(
            "PackagingSession requires at least one Rendition".into(),
        ));
    }
    if !config.segment_duration.is_finite()
        || config.segment_duration < MIN_SEGMENT_DURATION
        || config.segment_duration > MAX_SEGMENT_DURATION
    {
        return Err(DrmpackError::InvalidConfig(format!(
            "segment_duration must be between {MIN_SEGMENT_DURATION}s and {MAX_SEGMENT_DURATION}s (got {})",
            config.segment_duration
        )));
    }
    if !config.chunk_duration.is_finite() || config.chunk_duration <= 0.0 {
        return Err(DrmpackError::InvalidConfig(
            "chunk_duration must be finite and greater than zero".into(),
        ));
    }
    if config.chunk_duration >= config.segment_duration {
        return Err(DrmpackError::InvalidConfig(format!(
            "chunk_duration ({:.2}s) must be strictly less than segment_duration ({:.2}s)",
            config.chunk_duration, config.segment_duration
        )));
    }
    if let Some(asto) = config.availability_time_offset {
        if !asto.is_finite() || asto < 0.0 {
            return Err(DrmpackError::InvalidConfig(
                "availability_time_offset must be finite and non-negative".into(),
            ));
        }
        if asto >= config.segment_duration {
            return Err(DrmpackError::InvalidConfig(format!(
                "availability_time_offset ({:.2}s) must be strictly less than segment_duration ({:.2}s)",
                asto, config.segment_duration
            )));
        }
    }
    if (config.segment_duration.fract()).abs() > 1e-6 {
        warn!(
            duration = config.segment_duration,
            "Non-integer segment_duration ({:.2}s); verify that encoder GOP matches to prevent uneven pacing or missing keyframes at segment boundaries",
            config.segment_duration
        );
    }
    if config.finalization_timeout.is_zero() {
        return Err(DrmpackError::InvalidConfig(
            "finalization_timeout must be greater than zero".into(),
        ));
    }
    if config.time_shift_buffer.is_zero() {
        return Err(DrmpackError::InvalidConfig(
            "time_shift_buffer must be greater than zero".into(),
        ));
    }

    let mut seen_logical_ids = std::collections::HashSet::new();
    let mut seen_container_ids = std::collections::HashSet::new();
    for (index, rendition) in config.renditions.iter().enumerate() {
        if rendition.track_id.trim().is_empty() {
            return Err(DrmpackError::InvalidConfig(
                "track_id cannot be empty".into(),
            ));
        }
        if !seen_logical_ids.insert(&rendition.track_id) {
            return Err(DrmpackError::InvalidConfig(format!(
                "Duplicate track_id '{}' across renditions",
                rendition.track_id
            )));
        }

        if rendition.track_type == TrackType::Subtitle && rendition.encrypted {
            return Err(DrmpackError::InvalidConfig(
                "Subtitle renditions must be unencrypted (Common Encryption does not support text tracks)".into(),
            ));
        }

        let container_track_id = rendition.effective_container_track_id(index);
        if container_track_id == 0 {
            return Err(DrmpackError::InvalidConfig(
                "container_track_id cannot be 0 (ISO-BMFF track IDs must be >= 1)".into(),
            ));
        }
        if !seen_container_ids.insert(container_track_id) {
            return Err(DrmpackError::InvalidConfig(format!(
                "Duplicate container_track_id {} across renditions",
                container_track_id
            )));
        }
    }

    Ok(())
}

async fn fetch_key_set<P: KeyProvider>(
    config: &PackagingSessionConfig,
    provider: &P,
) -> Result<KeySet> {
    let plan = KeyPolicyEngine::plan(
        &config.content_id,
        &config.renditions,
        config.key_mapping_policy,
        config.encryption_scheme,
        &config.drm_systems,
    );

    match plan.request {
        Some(ref request) => {
            info!(content_id = %config.content_id, "Fetching encryption keys from provider");
            let fetched = provider.fetch_keys(request).await?;
            KeyPolicyEngine::resolve(&plan, &config.renditions, config.encryption_scheme, fetched)
        }
        None => {
            info!(
                content_id = %config.content_id,
                "All renditions are unencrypted; skipping key acquisition"
            );
            KeyPolicyEngine::resolve(
                &plan,
                &config.renditions,
                config.encryption_scheme,
                KeySet::new(),
            )
        }
    }
}

async fn prepare_output_dir(output_dir: &Path, is_dual: bool) -> Result<bool> {
    match tokio::fs::metadata(output_dir).await {
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Err(DrmpackError::InvalidConfig(format!(
                    "Output path '{}' is not a directory",
                    output_dir.display()
                )));
            }
            if is_dual {
                let mut entries = tokio::fs::read_dir(output_dir).await?;
                if entries.next_entry().await?.is_some() {
                    return Err(DrmpackError::InvalidConfig(format!(
                        "Dual PackagingSession output directory '{}' must be empty",
                        output_dir.display()
                    )));
                }
            }
            Ok(false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tokio::fs::create_dir_all(output_dir).await?;
            Ok(true)
        }
        Err(error) => Err(DrmpackError::Io(error)),
    }
}

async fn create_control_dir(config: &PackagingSessionConfig) -> Result<PathBuf> {
    let parent = config
        .control_dir
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("drmpack-control"));
    let control_dir = parent.join(format!("{}-{}", config.content_id, Uuid::new_v4()));

    tokio::fs::create_dir_all(&control_dir).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&control_dir, std::fs::Permissions::from_mode(0o700)).await?;
    }

    Ok(control_dir)
}

struct WatchdogContext {
    timeout: Option<Duration>,
    finalization_timeout: Duration,
    control_dir: PathBuf,
    lifecycle: Arc<Mutex<Lifecycle>>,
    is_terminal: Arc<AtomicBool>,
    cluster: Arc<RepresentationCluster>,
    cancellation_token: CancellationToken,
}

/// Verify that all HLS media manifests in `output_dir` contain the `#EXT-X-ENDLIST` tag.
///
/// In dynamic live packaging (`dmode=dynauto`), GPAC moves manifests to static upon EOF on stdin
/// and appends `#EXT-X-ENDLIST` to HLS media manifests. When finalizing a session where media
/// segments were pushed, this verifies the presence of `#EXT-X-ENDLIST` to prevent player/CDN stall.
/// Master manifests (containing `#EXT-X-STREAM-INF:` or `#EXT-X-MEDIA:TYPE=AUDIO`)
/// do not contain `#EXT-X-ENDLIST` per RFC 8216 and are skipped.
pub async fn verify_hls_endlist(output_dir: &Path) -> Result<()> {
    if !output_dir.exists() {
        return Err(DrmpackError::Session(format!(
            "Output directory '{}' does not exist for #EXT-X-ENDLIST verification",
            output_dir.display()
        )));
    }

    let mut dir = match tokio::fs::read_dir(output_dir).await {
        Ok(d) => d,
        Err(e) => return Err(DrmpackError::Io(e)),
    };
    let mut media_manifests_found = 0;

    while let Ok(Some(entry)) = dir.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("m3u8") {
            let content = tokio::fs::read_to_string(&path).await?;
            if content.contains("#EXT-X-STREAM-INF:") {
                // Master manifest - RFC 8216 forbids EXT-X-ENDLIST in master manifests
                continue;
            }
            if content.contains("#EXT-X-TARGETDURATION:") || content.contains("#EXTINF:") {
                media_manifests_found += 1;
                if !content.contains("#EXT-X-ENDLIST") {
                    return Err(DrmpackError::Session(format!(
                        "HLS media manifest '{}' is missing finalization tag #EXT-X-ENDLIST",
                        path.display()
                    )));
                }
            }
        }
    }

    if media_manifests_found == 0 {
        return Err(DrmpackError::Session(format!(
            "No HLS media manifests found in '{}' to verify #EXT-X-ENDLIST",
            output_dir.display()
        )));
    }

    Ok(())
}

async fn cleanup_watchdog_dirs(control_dir: &Path) -> Option<DrmpackError> {
    tokio::fs::remove_dir_all(control_dir)
        .await
        .err()
        .map(|error| {
            DrmpackError::Io(std::io::Error::new(
                error.kind(),
                format!(
                    "Failed to clean up private control directory '{}': {error}",
                    control_dir.display()
                ),
            ))
        })
}

fn build_watchdog(ctx: WatchdogContext) -> (Option<mpsc::Sender<()>>, Option<JoinHandle<()>>) {
    let WatchdogContext {
        timeout,
        finalization_timeout,
        control_dir,
        lifecycle,
        is_terminal,
        cluster,
        cancellation_token,
    } = ctx;

    let (heartbeat_tx, mut heartbeat_rx) = mpsc::channel(16);

    let handle = tokio::spawn(async move {
        loop {
            let timeout_fut = async {
                match timeout {
                    Some(dur) => tokio::time::sleep(dur).await,
                    None => std::future::pending().await,
                }
            };

            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    debug!("Watchdog received cancellation signal");
                    break;
                }
                heartbeat = heartbeat_rx.recv() => match heartbeat {
                    Some(()) => continue,
                    None => break,
                },
                _ = timeout_fut => {
                    if cancellation_token.is_cancelled() {
                        break;
                    }
                    cancellation_token.cancel();

                    warn!(?timeout, "PackagingSession inactivity watchdog elapsed");

                    let is_active = {
                        let mut lifecycle_guard = lifecycle.lock().await;
                        if lifecycle_guard.state == SessionState::Active {
                            lifecycle_guard.state = SessionState::Failed;
                            let initial_failures = cluster
                                .schemes()
                                .into_iter()
                                .map(|scheme| {
                                    RepresentationFailure::new(
                                        scheme,
                                        PackagingOperation::Watchdog,
                                        DrmpackError::Session(
                                            "PackagingSession inactivity watchdog elapsed".into(),
                                        ),
                                    )
                                })
                                .collect();
                            lifecycle_guard.terminal_failure = Some(Arc::new(
                                PackagingSessionFailure::from_failures(initial_failures),
                            ));
                            true
                        } else {
                            false
                        }
                    };
                    if !is_active {
                        break;
                    }

                    let close_failures = cluster
                        .close(
                            finalization_timeout.min(WATCHDOG_FINALIZATION_TIMEOUT),
                            PackagingOperation::Watchdog,
                        )
                        .await;

                    let control_cleanup = cleanup_watchdog_dirs(&control_dir).await;

                    {
                        let mut lifecycle_guard = lifecycle.lock().await;
                        let mut failures = cluster
                            .schemes()
                            .into_iter()
                            .map(|scheme| {
                                RepresentationFailure::new(
                                    scheme,
                                    PackagingOperation::Watchdog,
                                    DrmpackError::Session(
                                        "PackagingSession inactivity watchdog elapsed".into(),
                                    ),
                                )
                            })
                            .collect::<Vec<_>>();
                        failures.extend(close_failures);
                        lifecycle_guard.terminal_failure = Some(Arc::clone(&failure_ptr(
                            PackagingSessionFailure::from_failures(failures)
                                .with_cleanup_failures(None, control_cleanup),
                        )));
                    }
                    is_terminal.store(true, Ordering::Release);
                    break;
                }
                (scheme, exit_status) = cluster.wait_for_exit() => {
                    if cancellation_token.is_cancelled() {
                        break;
                    }
                    cancellation_token.cancel();

                    let stderr = cluster.get_recent_stderr(scheme).await;
                    error!(
                        scheme = %scheme,
                        exit_code = ?exit_status.code,
                        stderr = %stderr,
                        "ProcessSupervisor detected premature GPAC subprocess exit"
                    );

                    let crash_msg = if exit_status.success {
                        format!("GPAC exited prematurely before PackagingSession::close(). Stderr: {stderr}")
                    } else {
                        stderr.clone()
                    };

                    let is_active = {
                        let mut lifecycle_guard = lifecycle.lock().await;
                        if lifecycle_guard.state == SessionState::Active {
                            lifecycle_guard.state = SessionState::Failed;
                            let initial_failure = RepresentationFailure::new(
                                scheme,
                                PackagingOperation::Supervisor,
                                DrmpackError::ProcessCrashed {
                                    exit_code: exit_status.code,
                                    stderr: crash_msg.clone(),
                                },
                            );
                            lifecycle_guard.terminal_failure = Some(Arc::new(
                                PackagingSessionFailure::from_failures(vec![initial_failure]),
                            ));
                            true
                        } else {
                            false
                        }
                    };
                    if !is_active {
                        break;
                    }

                    // Dual-mode symmetric fail-fast: immediately teardown remaining peer representation(s)
                    let peer_failures = cluster.abort_peers(scheme).await;

                    let control_cleanup = cleanup_watchdog_dirs(&control_dir).await;

                    {
                        let mut lifecycle_guard = lifecycle.lock().await;
                        let mut all_failures = vec![
                            RepresentationFailure::new(
                                scheme,
                                PackagingOperation::Supervisor,
                                DrmpackError::ProcessCrashed {
                                    exit_code: exit_status.code,
                                    stderr: crash_msg,
                                },
                            )
                        ];
                        all_failures.extend(peer_failures);
                        lifecycle_guard.terminal_failure = Some(Arc::new(
                            PackagingSessionFailure::from_failures(all_failures)
                                .with_cleanup_failures(None, control_cleanup),
                        ));
                    }
                    is_terminal.store(true, Ordering::Release);
                    break;
                }
            }
        }
    });

    (Some(heartbeat_tx), Some(handle))
}

fn failure_ptr(f: PackagingSessionFailure) -> Arc<PackagingSessionFailure> {
    Arc::new(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::{ContentKey, KeyID, RawKeyProvider};
    use crate::types::{QualityTier, TrackType};
    use uuid::Uuid;

    fn rendition() -> Rendition {
        Rendition::video_hd()
    }

    fn provider() -> RawKeyProvider {
        RawKeyProvider::new().with_key(ContentKey::new(
            KeyID::new(Uuid::from_bytes([0x01; 16])),
            [0x42; 16],
            QualityTier::hd(),
            TrackType::Video,
        ))
    }

    #[tokio::test]
    async fn session_config_requires_a_rendition() {
        let result =
            PackagingSession::create(PackagingSessionConfig::new("test"), &provider()).await;
        assert!(matches!(result, Err(DrmpackError::InvalidConfig(_))));
    }

    #[test]
    fn encryption_mode_builder_last_call_wins() {
        let config = PackagingSessionConfig::new("test")
            .with_encryption_scheme(EncryptionScheme::Cbcs)
            .with_encryption_scheme(EncryptionScheme::Dual);
        assert_eq!(config.encryption_scheme, EncryptionScheme::Dual);
    }

    #[tokio::test]
    async fn dual_rejects_a_nonempty_output_directory() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&output_dir).await.unwrap();
        tokio::fs::write(output_dir.join("existing"), b"x")
            .await
            .unwrap();
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_encryption_scheme(EncryptionScheme::Dual)
            .with_output_dir(&output_dir);

        let result = PackagingSession::create(config, &provider()).await;
        assert!(matches!(result, Err(DrmpackError::InvalidConfig(_))));
        tokio::fs::remove_dir_all(output_dir).await.unwrap();
    }

    #[tokio::test]
    async fn dual_creation_rollback_removes_created_output() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_encryption_scheme(EncryptionScheme::Dual)
            .with_output_dir(&output_dir)
            .with_gpac_bin("missing-gpac-for-rollback-test");

        let result = PackagingSession::create(config, &provider()).await;
        let Err(DrmpackError::PackagingSession(failure)) = result else {
            panic!("creation failure must be reported as PackagingSession failure");
        };
        assert_eq!(failure.cenc.len(), 1);
        assert_eq!(failure.cenc[0].operation, PackagingOperation::Create);
        assert!(!output_dir.exists());
    }

    #[tokio::test]
    async fn dual_session_resolves_isolated_manifest_paths() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let control_parent =
            std::env::temp_dir().join(format!("drmpack_control_test_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_encryption_scheme(EncryptionScheme::Dual)
            .with_output_dir(&output_dir)
            .with_control_dir(&control_parent)
            .with_gpac_bin("gpac");
        let mut session = PackagingSession::create(config, &provider()).await.unwrap();

        assert_eq!(
            session
                .manifest_path(EncryptionScheme::Cenc, ManifestFormat::Dash)
                .unwrap(),
            output_dir.join("cenc/live.mpd")
        );
        assert_eq!(
            session
                .manifest_path(EncryptionScheme::Cbcs, ManifestFormat::Hls)
                .unwrap(),
            output_dir.join("cbcs/live.m3u8")
        );
        assert!(matches!(
            session.manifest_path(EncryptionScheme::Dual, ManifestFormat::Dash),
            Err(DrmpackError::InvalidConfig(_))
        ));
        assert!(matches!(
            session.hls_manifest_path(),
            Err(DrmpackError::InvalidConfig(_))
        ));
        assert!(!output_dir.join("drm.xml").exists());
        assert!(session.control_dir_path().join("cenc.xml").exists());
        assert!(session.control_dir_path().join("cbcs.xml").exists());

        let _ = session.close().await;
        assert!(!control_parent.exists() || control_parent.read_dir().unwrap().next().is_none());
        tokio::fs::remove_dir_all(&output_dir).await.unwrap();
        let _ = tokio::fs::remove_dir_all(&control_parent).await;
    }

    #[tokio::test]
    async fn status_after_creation_failure_returns_structured_representation_failure() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac");
        let mut session = PackagingSession::create(config, &provider()).await.unwrap();

        let first_error = session.close().await.unwrap_err();
        let DrmpackError::PackagingSession(failure) = first_error else {
            panic!("close must return structured packaging failure");
        };
        assert_eq!(failure.cbcs.len(), 1);
        assert!(failure.cenc.is_empty());
        assert_eq!(failure.cbcs[0].operation, PackagingOperation::Close);

        let later_error = session.check_status().await.unwrap_err();
        assert!(matches!(later_error, DrmpackError::PackagingSession(_)));
        session.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn cleanup_rejects_an_active_session() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac");
        let mut session = PackagingSession::create(config, &provider()).await.unwrap();

        assert!(matches!(
            session.cleanup().await,
            Err(DrmpackError::Session(_))
        ));
        let _ = session.close().await;
        // A failed GPAC finalization is still terminal and must permit cleanup.
        assert!(session.is_closed());
        session.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn all_clear_renditions_bypasses_key_provider() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition().clear())
            .with_rendition(Rendition::audio().clear())
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac");

        // RawKeyProvider is empty (no keys). If fetch_keys were called, it would error.
        let empty_provider = RawKeyProvider::new();
        let mut session = PackagingSession::create(config, &empty_provider)
            .await
            .unwrap();

        let drm_xml = tokio::fs::read_to_string(session.control_dir_path().join("cbcs.xml"))
            .await
            .unwrap();
        assert!(drm_xml.contains(r#"<CrypTrack trackID="1" IsEncrypted="0"/>"#));
        assert!(drm_xml.contains(r#"<CrypTrack trackID="2" IsEncrypted="0"/>"#));

        let _ = session.close().await;
        session.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn mixed_renditions_selective_encryption_gpac_xml() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition()) // encrypted video HD
            .with_rendition(Rendition::audio().clear()) // clear audio
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac");

        // provider() only supplies Video HD key. No Audio key is provided.
        let mut session = PackagingSession::create(config, &provider()).await.unwrap();

        let drm_xml = tokio::fs::read_to_string(session.control_dir_path().join("cbcs.xml"))
            .await
            .unwrap();
        // Track 1 is encrypted with video key
        assert!(drm_xml.contains(r#"<CrypTrack trackID="1" IsEncrypted="1""#));
        assert!(drm_xml.contains("42424242424242424242424242424242"));
        // Track 2 is unencrypted
        assert!(drm_xml.contains(r#"<CrypTrack trackID="2" IsEncrypted="0"/>"#));

        let _ = session.close().await;
        session.cleanup().await.unwrap();
    }

    #[tokio::test]
    async fn key_mapping_policy_shared_all() {
        let video_sd = Rendition::video(QualityTier::sd());
        let video_hd = rendition();
        let audio = Rendition::audio();

        let config = PackagingSessionConfig::new("test")
            .with_key_mapping_policy(KeyMappingPolicy::SharedAll)
            .with_rendition(video_hd)
            .with_rendition(video_sd)
            .with_rendition(audio);

        // Provider has only 1 key for HD Video
        let key_set = fetch_key_set(&config, &provider()).await.unwrap();

        let v_hd = key_set
            .get_key(TrackType::Video, &QualityTier::hd())
            .unwrap();
        let v_sd = key_set
            .get_key(TrackType::Video, &QualityTier::sd())
            .unwrap();
        let a_sd = key_set
            .get_key(TrackType::Audio, &QualityTier::sd())
            .unwrap();

        assert_eq!(v_hd.kid, v_sd.kid);
        assert_eq!(v_hd.kid, a_sd.kid);
        assert_eq!(v_hd.key, [0x42; 16]);
    }

    #[tokio::test]
    async fn key_mapping_policy_shared_video_single_audio() {
        let video_hd = rendition();
        let video_sd = Rendition::video(QualityTier::sd());
        let audio_sd = Rendition::audio();
        let audio_hd = Rendition::audio_tier(QualityTier::hd());

        let config = PackagingSessionConfig::new("test")
            .with_key_mapping_policy(KeyMappingPolicy::SharedVideoSingleAudio)
            .with_rendition(video_hd)
            .with_rendition(video_sd)
            .with_rendition(audio_sd)
            .with_rendition(audio_hd);

        let kid_video = KeyID::new(Uuid::from_bytes([0x01; 16]));
        let kid_audio = KeyID::new(Uuid::from_bytes([0x02; 16]));

        let provider = RawKeyProvider::new()
            .with_key(ContentKey::new(
                kid_video,
                [0x11; 16],
                QualityTier::hd(),
                TrackType::Video,
            ))
            .with_key(ContentKey::new(
                kid_audio,
                [0x22; 16],
                QualityTier::sd(),
                TrackType::Audio,
            ));

        let key_set = fetch_key_set(&config, &provider).await.unwrap();

        let v_hd = key_set
            .get_key(TrackType::Video, &QualityTier::hd())
            .unwrap();
        let v_sd = key_set
            .get_key(TrackType::Video, &QualityTier::sd())
            .unwrap();
        let a_sd = key_set
            .get_key(TrackType::Audio, &QualityTier::sd())
            .unwrap();
        let a_hd = key_set
            .get_key(TrackType::Audio, &QualityTier::hd())
            .unwrap();

        assert_eq!(v_hd.kid, kid_video);
        assert_eq!(v_sd.kid, kid_video);
        assert_eq!(a_sd.kid, kid_audio);
        assert_eq!(a_hd.kid, kid_audio);
    }

    #[tokio::test]
    async fn key_mapping_policy_per_tier_and_track() {
        let video_hd = rendition();
        let video_sd = Rendition::video(QualityTier::sd());

        let config = PackagingSessionConfig::new("test")
            .with_key_mapping_policy(KeyMappingPolicy::PerTierAndTrack)
            .with_rendition(video_hd)
            .with_rendition(video_sd);

        let kid_hd = KeyID::new(Uuid::from_bytes([0x01; 16]));
        let kid_sd = KeyID::new(Uuid::from_bytes([0x02; 16]));

        let provider = RawKeyProvider::new()
            .with_key(ContentKey::new(
                kid_hd,
                [0x11; 16],
                QualityTier::hd(),
                TrackType::Video,
            ))
            .with_key(ContentKey::new(
                kid_sd,
                [0x22; 16],
                QualityTier::sd(),
                TrackType::Video,
            ));

        let key_set = fetch_key_set(&config, &provider).await.unwrap();

        let v_hd = key_set
            .get_key(TrackType::Video, &QualityTier::hd())
            .unwrap();
        let v_sd = key_set
            .get_key(TrackType::Video, &QualityTier::sd())
            .unwrap();

        assert_eq!(v_hd.kid, kid_hd);
        assert_eq!(v_sd.kid, kid_sd);
        assert_ne!(v_hd.kid, v_sd.kid);
    }

    #[tokio::test]
    async fn test_watchdog_cancellation_on_close_prevents_race() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac")
            .with_session_timeout(Duration::from_millis(50));

        let mut session = PackagingSession::create(config, &provider()).await.unwrap();

        // Close immediately, which cancels the token
        let _ = session.close().await;

        // Sleep longer than the watchdog timeout
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Session must remain Closed, not overwritten by Watchdog failure
        assert!(session.is_closed());
        let _ = session.cleanup().await;
    }

    #[tokio::test]
    async fn test_watchdog_inactivity_timeout_triggers_failure() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac")
            .with_session_timeout(Duration::from_millis(50));

        let session = PackagingSession::create(config, &provider()).await.unwrap();

        // Wait for inactivity watchdog to fire
        tokio::time::sleep(Duration::from_millis(150)).await;

        let status = session.check_status().await;
        assert!(status.is_err());
        let DrmpackError::PackagingSession(failure) = status.unwrap_err() else {
            panic!("Expected PackagingSession failure from watchdog");
        };
        assert!(failure
            .cbcs
            .iter()
            .any(|f| f.operation == PackagingOperation::Watchdog));
        let _ = session.cleanup().await;
    }

    #[tokio::test]
    async fn test_watchdog_timeout_concurrent_with_close() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac")
            .with_session_timeout(Duration::from_millis(30));

        let mut session = PackagingSession::create(config, &provider()).await.unwrap();

        // Sleep sufficiently for watchdog to fire
        tokio::time::sleep(Duration::from_millis(80)).await;

        let close_res = session.close().await;
        assert!(close_res.is_err());
        let DrmpackError::PackagingSession(failure) = close_res.unwrap_err() else {
            panic!("Expected structured PackagingSession failure on close after watchdog timeout");
        };
        assert!(failure
            .cbcs
            .iter()
            .any(|f| f.operation == PackagingOperation::Watchdog));
        assert!(session.is_closed(), "Session must be closed");
        let _ = session.cleanup().await;
    }

    #[tokio::test]
    async fn test_validate_config_rejects_encrypted_subtitle() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let mut sub = Rendition::subtitle();
        sub.encrypted = true;

        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_rendition(sub)
            .with_output_dir(&output_dir);

        let err = PackagingSession::create(config, &provider())
            .await
            .unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("Subtitle renditions must be unencrypted"))
        );
    }

    #[tokio::test]
    async fn test_validate_config_rejects_track_id_zero() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let r = rendition().with_container_track_id(0);

        let config = PackagingSessionConfig::new("test")
            .with_rendition(r)
            .with_output_dir(&output_dir);

        let err = PackagingSession::create(config, &provider())
            .await
            .unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("container_track_id cannot be 0"))
        );
    }

    #[tokio::test]
    async fn test_validate_config_rejects_duplicate_track_ids() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let r1 = Rendition::video(QualityTier::hd()).with_container_track_id(1);
        let r2 = Rendition::video(QualityTier::sd()).with_container_track_id(1);

        let config = PackagingSessionConfig::new("test")
            .with_rendition(r1)
            .with_rendition(r2)
            .with_output_dir(&output_dir);

        let err = PackagingSession::create(config, &provider())
            .await
            .unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("Duplicate container_track_id 1"))
        );
    }

    #[tokio::test]
    async fn test_validate_config_rejects_duplicate_logical_track_ids() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let mut r1 = Rendition::video_hd();
        r1.track_id = "v_custom".to_string();
        let mut r2 = Rendition::video(QualityTier::sd());
        r2.track_id = "v_custom".to_string();

        let config = PackagingSessionConfig::new("test")
            .with_rendition(r1)
            .with_rendition(r2)
            .with_output_dir(&output_dir);

        let err = PackagingSession::create(config, &provider())
            .await
            .unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("Duplicate track_id 'v_custom' across renditions"))
        );
    }

    #[tokio::test]
    async fn test_validate_config_rejects_empty_track_id() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let mut r = Rendition::video_hd();
        r.track_id = "   ".to_string();

        let config = PackagingSessionConfig::new("test")
            .with_rendition(r)
            .with_output_dir(&output_dir);

        let err = PackagingSession::create(config, &provider())
            .await
            .unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("track_id cannot be empty"))
        );
    }

    #[tokio::test]
    async fn test_validate_config_accepts_non_sequential_container_track_ids() {
        let r1 = Rendition::video_hd().with_container_track_id(10);
        let r2 = Rendition::video(QualityTier::sd()).with_container_track_id(20);
        let r3 = Rendition::audio().with_container_track_id(30);

        let config = PackagingSessionConfig::new("test")
            .with_rendition(r1)
            .with_rendition(r2)
            .with_rendition(r3);

        assert!(super::validate_config(&config).is_ok());
    }

    #[tokio::test]
    async fn test_validate_config_rejects_invalid_segment_duration() {
        let r = rendition();
        let base = PackagingSessionConfig::new("test").with_rendition(r);

        // Less than minimum (0.5s)
        let config = base.clone().with_segment_duration(0.4);
        let err = super::validate_config(&config).unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("segment_duration must be between 0.5s and 30s"))
        );

        // Greater than maximum (30s)
        let config = base.clone().with_segment_duration(30.1);
        let err = super::validate_config(&config).unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("segment_duration must be between 0.5s and 30s"))
        );

        // NaN
        let config = base.clone().with_segment_duration(f64::NAN);
        let err = super::validate_config(&config).unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("segment_duration must be between 0.5s and 30s"))
        );

        // Valid boundaries: 0.5s and 30.0s
        let config = base
            .clone()
            .with_segment_duration(0.5)
            .with_chunk_duration(0.1);
        assert!(super::validate_config(&config).is_ok());

        let config = base.clone().with_segment_duration(30.0);
        assert!(super::validate_config(&config).is_ok());
    }

    #[tokio::test]
    async fn test_validate_config_rejects_invalid_chunk_duration() {
        let r = rendition();
        let base = PackagingSessionConfig::new("test").with_rendition(r);

        // chunk_duration == segment_duration
        let config = base
            .clone()
            .with_segment_duration(2.0)
            .with_chunk_duration(2.0);
        let err = super::validate_config(&config).unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("chunk_duration (2.00s) must be strictly less than segment_duration (2.00s)"))
        );

        // chunk_duration > segment_duration
        let config = base
            .clone()
            .with_segment_duration(2.0)
            .with_chunk_duration(2.5);
        let err = super::validate_config(&config).unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("must be strictly less than segment_duration"))
        );

        // non-positive chunk_duration
        let config = base.clone().with_chunk_duration(0.0);
        let err = super::validate_config(&config).unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("chunk_duration must be finite and greater than zero"))
        );

        // negative chunk_duration
        let config = base.clone().with_chunk_duration(-0.2);
        let err = super::validate_config(&config).unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("chunk_duration must be finite and greater than zero"))
        );
    }

    #[tokio::test]
    async fn test_validate_config_availability_time_offset() {
        let r = rendition();
        let base = PackagingSessionConfig::new("test")
            .with_rendition(r)
            .with_segment_duration(2.0);

        // Valid asto
        let config = base.clone().with_availability_time_offset(Some(1.8));
        assert!(super::validate_config(&config).is_ok());

        let config = base.clone().with_availability_time_offset(Some(0.0));
        assert!(super::validate_config(&config).is_ok());

        // asto >= segment_duration
        let config = base.clone().with_availability_time_offset(Some(2.0));
        let err = super::validate_config(&config).unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("must be strictly less than segment_duration"))
        );

        // negative asto
        let config = base.clone().with_availability_time_offset(Some(-0.5));
        let err = super::validate_config(&config).unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("must be finite and non-negative"))
        );
    }

    #[tokio::test]
    async fn test_multiple_renditions_same_tier_no_id_collision() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let r1 = Rendition::video(QualityTier::hd());
        let r2 = Rendition::video(QualityTier::hd());

        assert_ne!(r1.track_id, r2.track_id);

        let config = PackagingSessionConfig::new("test")
            .with_rendition(r1)
            .with_rendition(r2)
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac");

        let mut session = PackagingSession::create(config, &provider()).await.unwrap();

        assert_eq!(session.config.renditions.len(), 2);
        assert_ne!(
            session.config.renditions[0].track_id,
            session.config.renditions[1].track_id
        );
        let _ = session.close().await;
        let _ = session.cleanup().await;
    }

    #[tokio::test]
    async fn test_verify_hls_endlist_missing_directory() {
        let non_existent = std::env::temp_dir().join(format!("drmpack_missing_{}", Uuid::new_v4()));
        let res = verify_hls_endlist(&non_existent).await;
        assert!(res.is_err());
        assert!(
            matches!(res.unwrap_err(), DrmpackError::Session(msg) if msg.contains("does not exist"))
        );
    }

    #[tokio::test]
    async fn test_verify_hls_endlist_no_media_manifests() {
        let dir = std::env::temp_dir().join(format!("drmpack_no_m3u8_{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let res = verify_hls_endlist(&dir).await;
        assert!(res.is_err());
        assert!(
            matches!(res.unwrap_err(), DrmpackError::Session(msg) if msg.contains("No HLS media manifests found"))
        );
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn test_verify_hls_endlist_skips_master_manifest() {
        let dir = std::env::temp_dir().join(format!("drmpack_master_only_{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let master_path = dir.join("master.m3u8");
        tokio::fs::write(
            &master_path,
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1280000\nv1.m3u8\n",
        )
        .await
        .unwrap();
        let res = verify_hls_endlist(&dir).await;
        assert!(res.is_err());
        let content = tokio::fs::read_to_string(&master_path).await.unwrap();
        assert!(!content.contains("#EXT-X-ENDLIST"));
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn test_verify_hls_endlist_missing_endlist_fails() {
        let dir = std::env::temp_dir().join(format!("drmpack_missing_endlist_{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let media_path = dir.join("video_720p.m3u8");
        let initial = "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\nsegment1.m4s\n";
        tokio::fs::write(&media_path, initial).await.unwrap();

        let res = verify_hls_endlist(&dir).await;
        assert!(res.is_err());
        assert!(
            matches!(res.unwrap_err(), DrmpackError::Session(msg) if msg.contains("missing finalization tag #EXT-X-ENDLIST"))
        );
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn test_verify_hls_endlist_present_succeeds() {
        let dir = std::env::temp_dir().join(format!("drmpack_valid_endlist_{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let media_path = dir.join("video_720p.m3u8");
        let initial =
            "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\nsegment1.m4s\n#EXT-X-ENDLIST\n";
        tokio::fs::write(&media_path, initial).await.unwrap();

        let res = verify_hls_endlist(&dir).await;
        assert!(res.is_ok());

        let content = tokio::fs::read_to_string(&media_path).await.unwrap();
        assert!(content.contains("#EXT-X-ENDLIST"));
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn test_session_is_alive_healthcheck() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac");

        let mut session = PackagingSession::create(config, &provider()).await.unwrap();
        assert!(
            session.is_alive(),
            "Active session must report is_alive() == true"
        );

        let _ = session.close().await;
        assert!(
            !session.is_alive(),
            "Closed session must report is_alive() == false"
        );
        let _ = session.cleanup().await;
    }

    #[tokio::test]
    async fn test_session_fail_fast_on_premature_exit() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac");

        let mut session = PackagingSession::create(config, &provider()).await.unwrap();
        assert!(session.is_alive());

        // Prematurely kill the GPAC subprocess
        session.representations()[0].kill();

        // Allow ProcessSupervisor to detect exit and watchdog to process it
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(!session.is_alive(), "Session must not be alive after crash");

        // push must fail immediately with PackagingSession failure containing PackagingOperation::Supervisor
        let res = session.push(b"dummy fmp4 data").await;
        assert!(res.is_err(), "push must fail after crash");
        let DrmpackError::PackagingSession(failure) = res.unwrap_err() else {
            panic!("Expected PackagingSession structured failure");
        };

        assert!(failure
            .cbcs
            .iter()
            .any(|f| f.operation == PackagingOperation::Supervisor));
        let _ = session.cleanup().await;
    }

    #[tokio::test]
    async fn test_dual_mode_symmetric_fail_fast() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_encryption_scheme(EncryptionScheme::Dual)
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac");

        let session = PackagingSession::create(config, &provider()).await.unwrap();
        assert_eq!(session.representations().len(), 2);
        assert!(session.is_alive());

        // Kill CENC representation process
        let cenc_rep = session
            .representations()
            .iter()
            .find(|r| r.scheme == EncryptionScheme::Cenc)
            .expect("CENC representation must exist");
        cenc_rep.kill();

        // Wait for fail-fast teardown
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Session must be dead
        assert!(!session.is_alive());

        // CBCS representation must be terminated symmetrically
        let cbcs_rep = session
            .representations()
            .iter()
            .find(|r| r.scheme == EncryptionScheme::Cbcs)
            .expect("CBCS representation must exist");
        assert!(
            !cbcs_rep.is_alive(),
            "CBCS peer must be torn down symmetrically"
        );

        // Status check reports failure
        let status = session.check_status().await;
        assert!(status.is_err());
        let DrmpackError::PackagingSession(failure) = status.unwrap_err() else {
            panic!("Expected structured PackagingSession failure");
        };
        assert_eq!(
            failure.cenc.len(),
            1,
            "CENC must have exactly 1 crash failure"
        );
        assert_eq!(
            failure.cbcs.len(),
            1,
            "CBCS peer must have exactly 1 symmetric abort failure"
        );
        assert_eq!(failure.cenc[0].operation, PackagingOperation::Supervisor);
        assert_eq!(failure.cbcs[0].operation, PackagingOperation::Supervisor);
        assert!(matches!(
            failure.cenc[0].error,
            DrmpackError::ProcessCrashed { .. }
        ));

        let _ = session.cleanup().await;
    }

    #[tokio::test]
    async fn test_push_init_only_does_not_require_endlist() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_init_only_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac");

        let mut session = PackagingSession::create(config, &provider()).await.unwrap();
        assert!(!session.has_pushed_media.load(Ordering::Acquire));

        // Push only empty/init bytes (no moof box)
        session
            .push(bytes::Bytes::new())
            .await
            .expect("Push write must succeed");

        // Init bytes must not mark has_pushed_media as true
        assert!(!session.has_pushed_media.load(Ordering::Acquire));

        let _ = session.close().await;
        let _ = session.cleanup().await;
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_verify_hls_endlist_permission_denied() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("drmpack_ro_dir_{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let media_path = dir.join("video_720p.m3u8");
        tokio::fs::write(
            &media_path,
            "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\nseg1.m4s\n#EXT-X-ENDLIST\n",
        )
        .await
        .unwrap();

        // Revoke read permission from directory so read_dir fails
        tokio::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000))
            .await
            .unwrap();

        let res = verify_hls_endlist(&dir).await;
        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), DrmpackError::Io(_)));

        // Restore permissions for cleanup
        let _ = tokio::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).await;
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[test]
    fn test_presets_and_builder_methods() {
        let cenc_cfg =
            PackagingSessionConfig::cenc("cenc_id").with_rendition(Rendition::video_hd());
        assert_eq!(cenc_cfg.content_id, "cenc_id");
        assert_eq!(cenc_cfg.encryption_scheme, EncryptionScheme::Cenc);
        assert_eq!(
            cenc_cfg.drm_systems,
            vec![DrmSystem::Widevine, DrmSystem::PlayReady]
        );
        assert_eq!(cenc_cfg.latency_mode, LatencyMode::LowLatency);
        assert_eq!(cenc_cfg.segment_duration, 2.0);
        assert_eq!(cenc_cfg.chunk_duration, 0.2);

        let cbcs_cfg =
            PackagingSessionConfig::cbcs("cbcs_id").with_rendition(Rendition::video_hd());
        assert_eq!(cbcs_cfg.content_id, "cbcs_id");
        assert_eq!(cbcs_cfg.encryption_scheme, EncryptionScheme::Cbcs);
        assert_eq!(
            cbcs_cfg.drm_systems,
            vec![
                DrmSystem::FairPlay,
                DrmSystem::Widevine,
                DrmSystem::PlayReady,
            ]
        );
        assert_eq!(cbcs_cfg.latency_mode, LatencyMode::Standard);
        assert_eq!(cbcs_cfg.segment_duration, 2.0);
        assert_eq!(cbcs_cfg.chunk_duration, 0.2);

        let dual_cfg = PackagingSessionConfig::low_latency_dual("dual_id")
            .with_rendition(Rendition::video_hd());
        assert_eq!(dual_cfg.content_id, "dual_id");
        assert_eq!(dual_cfg.encryption_scheme, EncryptionScheme::Dual);
        assert_eq!(
            dual_cfg.drm_systems,
            vec![
                DrmSystem::Widevine,
                DrmSystem::FairPlay,
                DrmSystem::PlayReady
            ]
        );
        assert_eq!(dual_cfg.latency_mode, LatencyMode::LowLatency);
        assert_eq!(dual_cfg.segment_duration, 2.0);
        assert_eq!(dual_cfg.chunk_duration, 0.2);

        let custom_cfg = PackagingSessionConfig::new("custom")
            .with_all_drm()
            .with_renditions(vec![
                Rendition::video_hd(),
                Rendition::video_4k(),
                Rendition::audio(),
            ]);
        assert_eq!(
            custom_cfg.drm_systems,
            vec![
                DrmSystem::Widevine,
                DrmSystem::FairPlay,
                DrmSystem::PlayReady
            ]
        );
        assert_eq!(custom_cfg.renditions.len(), 3);
        assert_eq!(custom_cfg.renditions[0].effective_container_track_id(0), 1);
        assert_eq!(custom_cfg.renditions[1].effective_container_track_id(1), 2);
        assert_eq!(custom_cfg.renditions[2].effective_container_track_id(2), 3);
    }

    #[test]
    fn test_default_output_dir_uses_temp_dir() {
        let config = PackagingSessionConfig::new("test_content");
        assert!(
            config.output_dir.starts_with(std::env::temp_dir()),
            "default_output_dir must use std::env::temp_dir(); got {:?}",
            config.output_dir
        );
        assert_eq!(config.time_shift_buffer, Duration::from_secs(60));

        let custom_tsb =
            PackagingSessionConfig::new("test").with_time_shift_buffer(Duration::from_secs(120));
        assert_eq!(custom_tsb.time_shift_buffer, Duration::from_secs(120));

        let custom_dir = PackagingSessionConfig::new("test").with_output_dir("/custom/path");
        assert_eq!(custom_dir.output_dir, PathBuf::from("/custom/path"));
        assert!(custom_dir.is_custom_output_dir);
    }

    #[tokio::test]
    async fn test_take_output_receiver_single_claim() {
        let mock_bin = std::env::temp_dir().join(format!("mock_gpac_{}.sh", Uuid::new_v4()));
        std::fs::write(&mock_bin, "#!/bin/sh\ncat > /dev/null\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&mock_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let config = PackagingSessionConfig::cenc("single_claim_test")
            .with_rendition(Rendition::video_hd().clear())
            .with_gpac_bin(mock_bin.to_str().unwrap());

        let mut session = PackagingSession::create(config, &RawKeyProvider::new())
            .await
            .unwrap();

        let first_claim = session.take_output_receiver();
        assert!(
            first_claim.is_some(),
            "First call to take_output_receiver must return Some(Receiver)"
        );

        let second_claim = session.take_output_receiver();
        assert!(
            second_claim.is_none(),
            "Second call to take_output_receiver must return None (single-ownership)"
        );

        session.close().await.unwrap();
        let _ = tokio::fs::remove_file(&mock_bin).await;
    }

    #[tokio::test]
    async fn test_take_output_receiver_closed_session_returns_none() {
        let mock_bin = std::env::temp_dir().join(format!("mock_gpac_{}.sh", Uuid::new_v4()));
        std::fs::write(&mock_bin, "#!/bin/sh\ncat > /dev/null\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&mock_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let config = PackagingSessionConfig::cenc("closed_receiver_test")
            .with_rendition(Rendition::video_hd().clear())
            .with_gpac_bin(mock_bin.to_str().unwrap());

        let mut session = PackagingSession::create(config, &RawKeyProvider::new())
            .await
            .unwrap();

        session.close().await.unwrap();

        let claim = session.take_output_receiver();
        assert!(
            claim.is_none(),
            "take_output_receiver on closed session must return None"
        );

        let _ = tokio::fs::remove_file(&mock_bin).await;
    }

    #[tokio::test]
    async fn test_ramdisk_lifecycle_drop_without_close_deletes() {
        let config = PackagingSessionConfig::cenc("lifecycle_drop_test")
            .with_rendition(Rendition::video_hd().clear())
            .with_gpac_bin("gpac");

        let out_dir;
        {
            let session = PackagingSession::create(config, &RawKeyProvider::new())
                .await
                .unwrap();
            out_dir = session.output_dir().to_path_buf();
            assert!(out_dir.exists(), "Auto-allocated output dir must exist");
            // PackagingSession dropped here without close()
        }
        assert!(
            !out_dir.exists(),
            "Auto-allocated output dir must be deleted on Drop when not closed"
        );
    }

    #[tokio::test]
    async fn test_ramdisk_lifecycle_close_cleans_up_after_drop() {
        let mock_bin = std::env::temp_dir().join(format!("mock_gpac_{}.sh", Uuid::new_v4()));
        std::fs::write(&mock_bin, "#!/bin/sh\ncat > /dev/null\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&mock_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let config = PackagingSessionConfig::cenc("lifecycle_close_test")
            .with_rendition(Rendition::video_hd().clear())
            .with_gpac_bin(mock_bin.to_str().unwrap());

        let out_dir;
        {
            let mut session = PackagingSession::create(config, &RawKeyProvider::new())
                .await
                .unwrap();
            out_dir = session.output_dir().to_path_buf();
            assert!(out_dir.exists(), "Auto-allocated output dir must exist");

            session.close().await.unwrap();
            assert!(
                out_dir.exists(),
                "Output dir must still exist after close() (before drop)"
            );
        }
        // PackagingSession dropped here — Drop cleans up auto-allocated output dir
        assert!(
            !out_dir.exists(),
            "Auto-allocated output dir must be deleted after Drop (no preserve_output)"
        );
        let _ = tokio::fs::remove_file(&mock_bin).await;
    }

    #[tokio::test]
    async fn test_ramdisk_lifecycle_failed_close_deletes_output_dir() {
        let mock_bin = std::env::temp_dir().join(format!("mock_gpac_fail_{}.sh", Uuid::new_v4()));
        std::fs::write(&mock_bin, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&mock_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let config = PackagingSessionConfig::cenc("lifecycle_close_fail_test")
            .with_rendition(Rendition::video_hd().clear())
            .with_gpac_bin(mock_bin.to_str().unwrap());

        let out_dir;
        {
            let mut session = PackagingSession::create(config, &RawKeyProvider::new())
                .await
                .unwrap();
            out_dir = session.output_dir().to_path_buf();
            assert!(out_dir.exists(), "Auto-allocated output dir must exist");

            let res = session.close().await;
            assert!(
                res.is_err(),
                "Session close must fail when GPAC exits with error"
            );
        }
        // PackagingSession dropped here after FAILED close()
        // Because close() failed, self.preserve_output remained false, so Drop deletes out_dir!
        assert!(
            !out_dir.exists(),
            "Auto-allocated output dir must be deleted on Drop after close() failure"
        );
        let _ = tokio::fs::remove_file(&mock_bin).await;
    }

    #[tokio::test]
    async fn test_ramdisk_lifecycle_preserve_output() {
        let config = PackagingSessionConfig::cenc("preserve_test")
            .with_rendition(Rendition::video_hd().clear())
            .preserve_output()
            .with_gpac_bin("gpac");

        let out_dir;
        {
            let mut session = PackagingSession::create(config, &RawKeyProvider::new())
                .await
                .unwrap();
            out_dir = session.output_dir().to_path_buf();
            assert!(out_dir.exists());
            let _ = session.close().await;
        }
        // Output directory must still exist because preserve_output was configured
        assert!(out_dir.exists(), "Preserved output dir must survive Drop");
        let _ = tokio::fs::remove_dir_all(&out_dir).await;
    }

    #[tokio::test]
    async fn test_ramdisk_lifecycle_custom_output_dir_never_dropped() {
        let custom_dir = std::env::temp_dir().join(format!("drmpack_custom_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::cenc("custom_dir_test")
            .with_rendition(Rendition::video_hd().clear())
            .with_output_dir(&custom_dir)
            .with_gpac_bin("gpac");

        {
            let mut session = PackagingSession::create(config, &RawKeyProvider::new())
                .await
                .unwrap();
            assert!(custom_dir.exists());
            let _ = session.close().await;
        }
        // Custom caller path must not be deleted on Drop
        assert!(
            custom_dir.exists(),
            "Custom output dir must NOT be deleted by Drop"
        );
        let _ = tokio::fs::remove_dir_all(&custom_dir).await;
    }

    #[tokio::test]
    async fn test_push_accepts_into_bytes() {
        let custom_dir = std::env::temp_dir().join(format!("drmpack_push_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::cenc("push_test")
            .with_rendition(Rendition::video_hd().clear())
            .with_output_dir(&custom_dir)
            .with_gpac_bin("gpac");

        let mut session = PackagingSession::create(config, &RawKeyProvider::new())
            .await
            .unwrap();

        // Push slice
        session.push(&b"init header"[..]).await.unwrap();
        // Push Vec<u8>
        session.push(vec![0u8; 16]).await.unwrap();
        // Push Bytes with fake moof
        let moof_chunk = vec![0x00, 0x00, 0x00, 0x08, b'm', b'o', b'o', b'f'];
        session.push(Bytes::from(moof_chunk)).await.unwrap();

        assert!(session.has_pushed_media.load(Ordering::Acquire));

        let _ = session.close().await;
        let _ = session.cleanup().await;
    }

    #[tokio::test]
    async fn test_ingest_stream_and_run_to_completion() {
        let custom_dir = std::env::temp_dir().join(format!("drmpack_stream_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::cenc("stream_test")
            .with_rendition(Rendition::video_hd().clear())
            .with_output_dir(&custom_dir)
            .with_gpac_bin("gpac");

        let mut session = PackagingSession::create(config, &RawKeyProvider::new())
            .await
            .unwrap();

        let (tx, rx) = tokio::sync::mpsc::channel(10);
        tokio::spawn(async move {
            tx.send(Bytes::from_static(b"chunk1")).await.unwrap();
            tx.send(Bytes::from_static(b"chunk2")).await.unwrap();
        });

        let count = session.ingest_stream(rx).await.unwrap();
        assert_eq!(count, 2);

        let _ = session.close().await;
        let _ = session.cleanup().await;
    }

    #[tokio::test]
    async fn test_run_to_completion() {
        let custom_dir = std::env::temp_dir().join(format!("drmpack_r2c_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::cenc("r2c_test")
            .with_rendition(Rendition::video_hd().clear())
            .with_output_dir(&custom_dir)
            .with_gpac_bin("gpac");

        let session = PackagingSession::create(config, &RawKeyProvider::new())
            .await
            .unwrap();

        let (tx, rx) = tokio::sync::mpsc::channel(10);
        tokio::spawn(async move {
            tx.send(Bytes::from_static(b"segment1")).await.unwrap();
            tx.send(Bytes::from_static(b"segment2")).await.unwrap();
        });

        let result = session.run_to_completion(rx).await.unwrap();
        assert_eq!(result.segments_ingested, 2);
        assert_eq!(result.output_dir, custom_dir);
        assert!(result
            .manifest_path(EncryptionScheme::Cenc, ManifestFormat::Hls)
            .is_some());
        assert!(result
            .manifest_path(EncryptionScheme::Cenc, ManifestFormat::Dash)
            .is_some());

        result.cleanup().await.unwrap();
        assert!(!custom_dir.exists());
    }

    #[tokio::test]
    async fn test_multi_rendition_same_tier_packaging_session() {
        let r1 = Rendition::video(QualityTier::hd());
        let r2 = Rendition::video(QualityTier::hd());
        let r_audio = Rendition::audio();

        let key_source = crate::key::StaticKeySource::shared_key([0x55; 16]);
        let config = PackagingSessionConfig::cenc("same_tier_stream")
            .with_renditions(vec![r1, r2, r_audio])
            .with_gpac_bin("gpac");

        let session = PackagingSession::create(config, &key_source).await;
        assert!(
            session.is_ok(),
            "PackagingSession with duplicate quality tiers must not fail with ID collisions"
        );

        let mut session = session.unwrap();
        let _ = session.close().await;
    }
}
