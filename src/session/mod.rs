pub mod cluster;
pub use cluster::{Representation, RepresentationCluster};

use crate::error::{
    DrmpackError, PackagingOperation, PackagingSessionFailure, RepresentationFailure, Result,
};
use crate::key::{KeyPolicyEngine, KeyProvider, KeySet};
use crate::types::{
    DrmSystem, EncryptionScheme, KeyMappingPolicy, LatencyMode, ManifestFormat, Rendition, Segment,
    TrackType,
};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

const DEFAULT_FINALIZATION_TIMEOUT: Duration = Duration::from_secs(5);
const WATCHDOG_FINALIZATION_TIMEOUT: Duration = Duration::from_secs(2);

/// Default shared memory path for Ramdisk output.
fn default_output_dir(content_id: &str) -> PathBuf {
    let parent = if Path::new("/dev/shm").is_dir() {
        PathBuf::from("/dev/shm")
    } else {
        std::env::temp_dir()
    };
    parent.join(format!("drmpack_{content_id}_{}", Uuid::new_v4()))
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
    pub output_dir: PathBuf,
    /// Parent directory for private, session-scoped GPAC DRM XML files.
    pub control_dir: Option<PathBuf>,
    /// Inactivity timeout for input. This is distinct from `finalization_timeout`.
    pub session_timeout: Option<Duration>,
    /// Per-Representation deadline for GPAC finalization after stdin closes.
    pub finalization_timeout: Duration,
    pub is_live: bool,
    pub gpac_bin: Option<String>,
    pub auto_cleanup: bool,
    pub key_mapping_policy: KeyMappingPolicy,
}

impl PackagingSessionConfig {
    pub fn new(content_id: impl Into<String>) -> Self {
        let cid = content_id.into();
        Self {
            output_dir: default_output_dir(&cid),
            content_id: cid,
            renditions: Vec::new(),
            encryption_scheme: EncryptionScheme::Cenc,
            drm_systems: Vec::new(),
            latency_mode: LatencyMode::LowLatency,
            segment_duration: 2.0,
            chunk_duration: 0.2,
            control_dir: None,
            session_timeout: None,
            finalization_timeout: DEFAULT_FINALIZATION_TIMEOUT,
            is_live: true,
            gpac_bin: None,
            auto_cleanup: false,
            key_mapping_policy: KeyMappingPolicy::default(),
        }
    }

    pub fn with_rendition(mut self, rendition: Rendition) -> Self {
        self.renditions.push(rendition);
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

    pub fn with_segment_duration(mut self, duration: f64) -> Self {
        self.segment_duration = duration;
        self
    }

    pub fn with_chunk_duration(mut self, duration: f64) -> Self {
        self.chunk_duration = duration;
        self
    }

    pub fn with_output_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.output_dir = dir.into();
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

    pub fn with_live(mut self, is_live: bool) -> Self {
        self.is_live = is_live;
        self
    }

    pub fn with_gpac_bin(mut self, bin: impl Into<String>) -> Self {
        self.gpac_bin = Some(bin.into());
        self
    }

    pub fn with_auto_cleanup(mut self, auto_cleanup: bool) -> Self {
        self.auto_cleanup = auto_cleanup;
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

/// A stateful packaging session that orchestrates DRM key acquisition,
/// GPAC child process lifecycle, and low-latency manifest/chunk generation into Ramdisk.
///
/// # Dual Scheme-Aware Keys
/// `EncryptionScheme::Dual` queries scheme-aware keys for both CENC and CBCS Representations
/// (`KeyRequest::encryption_schemes`). When supplied by a scheme-aware KeyProvider (such as CPIX),
/// each Representation receives distinct ContentKeys and KIDs per ADR-0006.
pub struct PackagingSession<P: KeyProvider + 'static> {
    config: PackagingSessionConfig,
    _key_provider: P,
    key_set: KeySet,
    cluster: Arc<RepresentationCluster>,
    control_dir: PathBuf,
    lifecycle: Arc<Mutex<Lifecycle>>,
    is_terminal: Arc<AtomicBool>,
    has_pushed_media: Arc<AtomicBool>,
    heartbeat_tx: Option<mpsc::Sender<()>>,
    cancellation_token: CancellationToken,
    watchdog_handle: Option<JoinHandle<()>>,
}

impl<P: KeyProvider + 'static> std::fmt::Debug for PackagingSession<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackagingSession")
            .field("config", &self.config)
            .field("representations", &self.cluster.schemes())
            .finish_non_exhaustive()
    }
}

impl<P: KeyProvider + 'static> PackagingSession<P> {
    /// Create a new packaging session, fetching its KeySet once and spawning one GPAC process
    /// per concrete Representation.
    #[instrument(skip(key_provider), fields(content_id = %config.content_id))]
    pub async fn create(config: PackagingSessionConfig, key_provider: P) -> Result<Self> {
        validate_config(&config)?;
        let key_set = fetch_key_set(&config, &key_provider).await?;
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
            output_dir_for_cleanup: config.auto_cleanup.then(|| config.output_dir.clone()),
            control_dir: control_dir.clone(),
            lifecycle: Arc::clone(&lifecycle),
            is_terminal: Arc::clone(&is_terminal),
            cluster: Arc::clone(&cluster),
            cancellation_token: cancellation_token.clone(),
        });

        Ok(Self {
            config,
            _key_provider: key_provider,
            key_set,
            cluster,
            control_dir,
            lifecycle,
            is_terminal,
            has_pushed_media,
            heartbeat_tx,
            cancellation_token,
            watchdog_handle,
        })
    }

    fn ping_heartbeat(&self) {
        if let Some(tx) = &self.heartbeat_tx {
            let _ = tx.try_send(());
        }
    }

    /// Push a media Segment to every active Representation. A successful return means every
    /// process accepted and flushed the same ordered bytes.
    #[instrument(skip(self, segment), fields(rendition_id = %segment.rendition_id, seq = segment.sequence_number))]
    pub async fn push_segment(&mut self, segment: Segment) -> Result<()> {
        let is_media = !segment.is_init && !segment.data.is_empty();
        self.push_data(&segment.data, is_media).await
    }

    /// Push raw media bytes to every active Representation.
    #[instrument(skip(self, bytes), fields(len = bytes.len()))]
    pub async fn push_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let is_media = !bytes.is_empty();
        self.push_data(bytes, is_media).await
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
                if let Some(ref failure) = lifecycle.terminal_failure {
                    return Err(DrmpackError::PackagingSession(Arc::clone(failure)));
                }
                drop(lifecycle);
                tokio::time::sleep(Duration::from_millis(5)).await;
                let lifecycle = self.lifecycle.lock().await;
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
    /// even when an earlier finalization fails.
    #[instrument(skip(self))]
    pub async fn close(&mut self) -> Result<()> {
        self.cancellation_token.cancel();

        let mut lifecycle = self.lifecycle.lock().await;
        match lifecycle.state {
            SessionState::Closed => return Ok(()),
            SessionState::Closing => return Ok(()),
            SessionState::Failed => {
                if lifecycle.terminal_failure.is_none() {
                    drop(lifecycle);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    lifecycle = self.lifecycle.lock().await;
                }
                let err = lifecycle.failure_error();
                drop(lifecycle);
                let _ = self.cleanup_control_dir().await;
                if self.config.auto_cleanup {
                    let _ = self.cleanup_output_dir().await;
                }
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

        let control_cleanup = self.cleanup_control_dir().await.err();
        let output_cleanup = if self.config.auto_cleanup {
            self.cleanup_output_dir().await.err()
        } else {
            None
        };

        if failures.is_empty() && control_cleanup.is_none() && output_cleanup.is_none() {
            lifecycle.state = SessionState::Closed;
            self.is_terminal.store(true, Ordering::Release);
            info!("PackagingSession closed successfully");
            Ok(())
        } else {
            lifecycle.state = SessionState::Failed;
            let failure = Arc::new(
                PackagingSessionFailure::from_failures(failures)
                    .with_cleanup_failures(output_cleanup, control_cleanup),
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

    /// Root of the Ramdisk delivery output. Private DRM XML remains outside this directory.
    pub fn output_dir(&self) -> &Path {
        &self.config.output_dir
    }

    async fn ensure_active(&self) -> Result<()> {
        if self.cancellation_token.is_cancelled() {
            let lifecycle = self.lifecycle.lock().await;
            return match lifecycle.state {
                SessionState::Failed => {
                    if let Some(ref failure) = lifecycle.terminal_failure {
                        Err(DrmpackError::PackagingSession(Arc::clone(failure)))
                    } else {
                        drop(lifecycle);
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        let lifecycle = self.lifecycle.lock().await;
                        Err(lifecycle.failure_error())
                    }
                }
                _ => Err(DrmpackError::Session("PackagingSession is closed".into())),
            };
        }

        let lifecycle = self.lifecycle.lock().await;
        match lifecycle.state {
            SessionState::Active => Ok(()),
            SessionState::Failed => {
                if let Some(ref failure) = lifecycle.terminal_failure {
                    Err(DrmpackError::PackagingSession(Arc::clone(failure)))
                } else {
                    drop(lifecycle);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    let lifecycle = self.lifecycle.lock().await;
                    Err(lifecycle.failure_error())
                }
            }
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
        let output_cleanup = if self.config.auto_cleanup {
            self.cleanup_output_dir().await.err()
        } else {
            None
        };
        let failure = Arc::new(
            PackagingSessionFailure::from_failures(failures)
                .with_cleanup_failures(output_cleanup, control_cleanup),
        );
        lifecycle.terminal_failure = Some(Arc::clone(&failure));
        self.is_terminal.store(true, Ordering::Release);
        DrmpackError::PackagingSession(failure)
    }

    /// Access active representations managed in this cluster.
    pub fn representations(&self) -> &[Representation] {
        self.cluster.representations()
    }

    /// Access the underlying RepresentationCluster.
    pub fn cluster(&self) -> &Arc<RepresentationCluster> {
        &self.cluster
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

impl<P: KeyProvider + 'static> Drop for PackagingSession<P> {
    fn drop(&mut self) {
        self.cancellation_token.cancel();
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
        if self.config.auto_cleanup && self.config.output_dir.exists() {
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
    if !config.segment_duration.is_finite() || config.segment_duration <= 0.0 {
        return Err(DrmpackError::InvalidConfig(
            "segment_duration must be finite and greater than zero".into(),
        ));
    }
    if !config.chunk_duration.is_finite() || config.chunk_duration <= 0.0 {
        return Err(DrmpackError::InvalidConfig(
            "chunk_duration must be finite and greater than zero".into(),
        ));
    }
    if config.finalization_timeout.is_zero() {
        return Err(DrmpackError::InvalidConfig(
            "finalization_timeout must be greater than zero".into(),
        ));
    }

    let mut seen_ids = std::collections::HashSet::new();
    let mut seen_track_ids = std::collections::HashSet::new();
    for (index, rendition) in config.renditions.iter().enumerate() {
        if !seen_ids.insert(&rendition.id) {
            return Err(DrmpackError::InvalidConfig(format!(
                "Duplicate rendition id '{}' across renditions",
                rendition.id
            )));
        }

        if rendition.track_type == TrackType::Subtitle && rendition.encrypted {
            return Err(DrmpackError::InvalidConfig(
                "Subtitle renditions must be unencrypted (Common Encryption does not support text tracks)".into(),
            ));
        }

        let track_id = rendition.effective_track_id(index);
        if track_id == 0 {
            return Err(DrmpackError::InvalidConfig(
                "track_id cannot be 0 (ISO-BMFF track IDs must be >= 1)".into(),
            ));
        }
        if !seen_track_ids.insert(track_id) {
            return Err(DrmpackError::InvalidConfig(format!(
                "Duplicate track_id {} across renditions",
                track_id
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

    // Under concurrent execution, retry if parent directory is temporarily being unlinked or modified
    let mut attempts = 0;
    loop {
        match tokio::fs::create_dir_all(&control_dir).await {
            Ok(()) => break,
            Err(e)
                if attempts < 3
                    && (e.raw_os_error() == Some(22)
                        || e.kind() == std::io::ErrorKind::NotFound) =>
            {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(e) => return Err(DrmpackError::Io(e)),
        }
    }

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
    output_dir_for_cleanup: Option<PathBuf>,
    control_dir: PathBuf,
    lifecycle: Arc<Mutex<Lifecycle>>,
    is_terminal: Arc<AtomicBool>,
    cluster: Arc<RepresentationCluster>,
    cancellation_token: CancellationToken,
}

/// Finalize and verify that all HLS media playlists in `output_dir` contain the `#EXT-X-ENDLIST` tag.
///
/// In dynamic live packaging (`dmode=dynamic`), GPAC leaves playlists open for live segments.
/// When finalizing a session where media segments were pushed, this verifies and guarantees
/// the presence of `#EXT-X-ENDLIST` to prevent player/CDN stall while preserving LL-HLS parts.
/// Master playlists (containing `#EXT-X-STREAM-INF:` or `#EXT-X-MEDIA:TYPE=AUDIO`)
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
    let mut media_playlists_found = 0;

    while let Ok(Some(entry)) = dir.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("m3u8") {
            let mut content = tokio::fs::read_to_string(&path).await?;
            if content.contains("#EXT-X-STREAM-INF:") {
                // Master playlist - RFC 8216 forbids EXT-X-ENDLIST in master playlists
                continue;
            }
            if content.contains("#EXT-X-TARGETDURATION:") || content.contains("#EXTINF:") {
                media_playlists_found += 1;
                if !content.contains("#EXT-X-ENDLIST") {
                    if !content.ends_with('\n') {
                        content.push('\n');
                    }
                    content.push_str("#EXT-X-ENDLIST\n");

                    // Atomic write-then-rename in the same directory to prevent CDN readers
                    // from reading a partially written or truncated manifest.
                    let temp_path = path.with_extension(format!("tmp.{}", uuid::Uuid::new_v4()));
                    tokio::fs::write(&temp_path, &content).await.map_err(|e| {
                        DrmpackError::Io(std::io::Error::new(
                            e.kind(),
                            format!(
                                "Failed to write temp manifest '{}': {e}",
                                temp_path.display()
                            ),
                        ))
                    })?;
                    tokio::fs::rename(&temp_path, &path).await.map_err(|e| {
                        let _ = std::fs::remove_file(&temp_path);
                        DrmpackError::Io(std::io::Error::new(
                            e.kind(),
                            format!(
                                "Failed to atomically rename manifest to '{}': {e}",
                                path.display()
                            ),
                        ))
                    })?;
                }
                if !content.contains("#EXT-X-ENDLIST") {
                    return Err(DrmpackError::Session(format!(
                        "HLS media manifest '{}' is missing finalization tag #EXT-X-ENDLIST",
                        path.display()
                    )));
                }
            }
        }
    }

    if media_playlists_found == 0 {
        return Err(DrmpackError::Session(format!(
            "No HLS media playlists found in '{}' to verify #EXT-X-ENDLIST",
            output_dir.display()
        )));
    }

    Ok(())
}

fn build_watchdog(ctx: WatchdogContext) -> (Option<mpsc::Sender<()>>, Option<JoinHandle<()>>) {
    let WatchdogContext {
        timeout,
        finalization_timeout,
        output_dir_for_cleanup,
        control_dir,
        lifecycle,
        is_terminal,
        cluster,
        cancellation_token,
    } = ctx;

    let (heartbeat_tx, mut heartbeat_rx) = mpsc::channel(16);

    let (exit_tx, mut exit_rx) =
        mpsc::channel::<(EncryptionScheme, crate::gpac::process::ProcessExitStatus)>(16);
    for rep in cluster.representations() {
        let scheme = rep.scheme;
        let mut rep_exit_rx = rep.subscribe_exit();
        let tx = exit_tx.clone();
        tokio::spawn(async move {
            if let Ok(status) = rep_exit_rx.recv().await {
                let _ = tx.send((scheme, status)).await;
            }
        });
    }
    drop(exit_tx);

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

                    let is_active = {
                        let mut lifecycle_guard = lifecycle.lock().await;
                        if lifecycle_guard.state == SessionState::Active {
                            lifecycle_guard.state = SessionState::Failed;
                            true
                        } else {
                            false
                        }
                    };
                    if !is_active {
                        break;
                    }

                    warn!(?timeout, "PackagingSession inactivity watchdog elapsed");

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

                    let close_failures = cluster
                        .close(
                            finalization_timeout.min(WATCHDOG_FINALIZATION_TIMEOUT),
                            PackagingOperation::Watchdog,
                        )
                        .await;
                    failures.extend(close_failures);

                    let control_cleanup = tokio::fs::remove_dir_all(&control_dir).await.err().map(|error| {
                        DrmpackError::Io(std::io::Error::new(
                            error.kind(),
                            format!(
                                "Failed to clean up private control directory '{}': {error}",
                                control_dir.display()
                            ),
                        ))
                    });

                    let output_cleanup = match output_dir_for_cleanup {
                        Some(ref output_dir) => tokio::fs::remove_dir_all(output_dir).await.err().map(|error| {
                            DrmpackError::Io(std::io::Error::new(
                                error.kind(),
                                format!(
                                    "Failed to clean up Ramdisk output directory '{}': {error}",
                                    output_dir.display()
                                ),
                            ))
                        }),
                        None => None,
                    };

                    {
                        let mut lifecycle_guard = lifecycle.lock().await;
                        lifecycle_guard.terminal_failure = Some(Arc::new(
                            PackagingSessionFailure::from_failures(failures)
                                .with_cleanup_failures(output_cleanup, control_cleanup),
                        ));
                    }
                    is_terminal.store(true, Ordering::Release);
                    break;
                }
                Some((scheme, exit_status)) = exit_rx.recv() => {
                    if cancellation_token.is_cancelled() {
                        break;
                    }
                    cancellation_token.cancel();

                    let is_active = {
                        let mut lifecycle_guard = lifecycle.lock().await;
                        if lifecycle_guard.state == SessionState::Active {
                            lifecycle_guard.state = SessionState::Failed;
                            true
                        } else {
                            false
                        }
                    };
                    if !is_active {
                        break;
                    }

                    let stderr = cluster.get_recent_stderr(scheme).await;
                    error!(
                        scheme = %scheme,
                        exit_code = ?exit_status.code,
                        stderr = %stderr,
                        "ProcessSupervisor detected premature GPAC subprocess exit"
                    );

                    let crash_error = DrmpackError::ProcessCrashed {
                        exit_code: exit_status.code,
                        stderr: if exit_status.success {
                            format!("GPAC exited prematurely before PackagingSession::close(). Stderr: {stderr}")
                        } else {
                            stderr
                        },
                    };

                    let mut failures = vec![
                        RepresentationFailure::new(scheme, PackagingOperation::Supervisor, crash_error)
                    ];

                    // Dual-mode symmetric fail-fast: immediately teardown remaining peer representation(s)
                    let peer_failures = cluster.abort_peers(scheme).await;
                    failures.extend(peer_failures);

                    let control_cleanup = tokio::fs::remove_dir_all(&control_dir).await.err().map(|error| {
                        DrmpackError::Io(std::io::Error::new(
                            error.kind(),
                            format!(
                                "Failed to clean up private control directory '{}': {error}",
                                control_dir.display()
                            ),
                        ))
                    });

                    let output_cleanup = match output_dir_for_cleanup {
                        Some(ref output_dir) => tokio::fs::remove_dir_all(output_dir).await.err().map(|error| {
                            DrmpackError::Io(std::io::Error::new(
                                error.kind(),
                                format!(
                                    "Failed to clean up Ramdisk output directory '{}': {error}",
                                    output_dir.display()
                                ),
                            ))
                        }),
                        None => None,
                    };

                    {
                        let mut lifecycle_guard = lifecycle.lock().await;
                        lifecycle_guard.terminal_failure = Some(Arc::new(
                            PackagingSessionFailure::from_failures(failures)
                                .with_cleanup_failures(output_cleanup, control_cleanup),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::{ContentKey, KeyID, RawKeyProvider};
    use crate::types::{QualityTier, TrackType};
    use uuid::Uuid;

    fn rendition() -> Rendition {
        Rendition::video(
            "v1080p",
            QualityTier::hd(),
            1920,
            1080,
            5_000_000,
            "avc1.640028",
        )
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
            PackagingSession::create(PackagingSessionConfig::new("test"), provider()).await;
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

        let result = PackagingSession::create(config, provider()).await;
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

        let result = PackagingSession::create(config, provider()).await;
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
        let mut session = PackagingSession::create(config, provider()).await.unwrap();

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
        let mut session = PackagingSession::create(config, provider()).await.unwrap();

        let first_error = session.close().await.unwrap_err();
        let DrmpackError::PackagingSession(failure) = first_error else {
            panic!("close must return structured packaging failure");
        };
        assert_eq!(failure.cenc.len(), 1);
        assert!(failure.cbcs.is_empty());
        assert_eq!(failure.cenc[0].operation, PackagingOperation::Close);

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
        let mut session = PackagingSession::create(config, provider()).await.unwrap();

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
            .with_rendition(Rendition::audio("a1", QualityTier::sd(), 128_000, "mp4a.40.2").clear())
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac");

        // RawKeyProvider is empty (no keys). If fetch_keys were called, it would error.
        let empty_provider = RawKeyProvider::new();
        let mut session = PackagingSession::create(config, empty_provider)
            .await
            .unwrap();

        let drm_xml = tokio::fs::read_to_string(session.control_dir_path().join("cenc.xml"))
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
            .with_rendition(
                Rendition::audio("a1", QualityTier::sd(), 128_000, "mp4a.40.2").clear(), // clear audio
            )
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac");

        // provider() only supplies Video HD key. No Audio key is provided.
        let mut session = PackagingSession::create(config, provider()).await.unwrap();

        let drm_xml = tokio::fs::read_to_string(session.control_dir_path().join("cenc.xml"))
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
        let video_sd = Rendition::video(
            "v_sd",
            QualityTier::sd(),
            854,
            480,
            1_500_000,
            "avc1.4d401f",
        );
        let video_hd = rendition();
        let audio = Rendition::audio("a1", QualityTier::sd(), 128_000, "mp4a.40.2");

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
        let video_sd = Rendition::video(
            "v_sd",
            QualityTier::sd(),
            854,
            480,
            1_500_000,
            "avc1.4d401f",
        );
        let audio_sd = Rendition::audio("a_sd", QualityTier::sd(), 128_000, "mp4a.40.2");
        let audio_hd = Rendition::audio("a_hd", QualityTier::hd(), 256_000, "mp4a.40.2");

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
        let video_sd = Rendition::video(
            "v_sd",
            QualityTier::sd(),
            854,
            480,
            1_500_000,
            "avc1.4d401f",
        );

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

        let mut session = PackagingSession::create(config, provider()).await.unwrap();

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

        let session = PackagingSession::create(config, provider()).await.unwrap();

        // Wait for inactivity watchdog to fire
        tokio::time::sleep(Duration::from_millis(150)).await;

        let status = session.check_status().await;
        assert!(status.is_err());
        let DrmpackError::PackagingSession(failure) = status.unwrap_err() else {
            panic!("Expected PackagingSession failure from watchdog");
        };
        assert!(failure
            .cenc
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

        let mut session = PackagingSession::create(config, provider()).await.unwrap();

        // Sleep sufficiently for watchdog to fire
        tokio::time::sleep(Duration::from_millis(80)).await;

        let close_res = session.close().await;
        assert!(close_res.is_err());
        let DrmpackError::PackagingSession(failure) = close_res.unwrap_err() else {
            panic!("Expected structured PackagingSession failure on close after watchdog timeout");
        };
        assert!(failure
            .cenc
            .iter()
            .any(|f| f.operation == PackagingOperation::Watchdog));
        assert!(session.is_closed(), "Session must be closed");
        let _ = session.cleanup().await;
    }

    #[tokio::test]
    async fn test_validate_config_rejects_encrypted_subtitle() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let mut sub = Rendition::subtitle("sub1", "tx3g");
        sub.encrypted = true;

        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_rendition(sub)
            .with_output_dir(&output_dir);

        let err = PackagingSession::create(config, provider())
            .await
            .unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("Subtitle renditions must be unencrypted"))
        );
    }

    #[tokio::test]
    async fn test_validate_config_rejects_track_id_zero() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let r = rendition().with_track_id(0);

        let config = PackagingSessionConfig::new("test")
            .with_rendition(r)
            .with_output_dir(&output_dir);

        let err = PackagingSession::create(config, provider())
            .await
            .unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("track_id cannot be 0"))
        );
    }

    #[tokio::test]
    async fn test_validate_config_rejects_duplicate_track_ids() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let r1 = Rendition::video(
            "v1",
            QualityTier::hd(),
            1920,
            1080,
            2_000_000,
            "avc1.640028",
        )
        .with_track_id(1);
        let r2 = Rendition::video("v2", QualityTier::sd(), 1280, 720, 1_000_000, "avc1.4d401f")
            .with_track_id(1);

        let config = PackagingSessionConfig::new("test")
            .with_rendition(r1)
            .with_rendition(r2)
            .with_output_dir(&output_dir);

        let err = PackagingSession::create(config, provider())
            .await
            .unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("Duplicate track_id 1"))
        );
    }

    #[tokio::test]
    async fn test_validate_config_rejects_duplicate_rendition_ids() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let r1 = Rendition::video(
            "v1",
            QualityTier::hd(),
            1920,
            1080,
            2_000_000,
            "avc1.640028",
        );
        let r2 = Rendition::video("v1", QualityTier::sd(), 1280, 720, 1_000_000, "avc1.4d401f");

        let config = PackagingSessionConfig::new("test")
            .with_rendition(r1)
            .with_rendition(r2)
            .with_output_dir(&output_dir);

        let err = PackagingSession::create(config, provider())
            .await
            .unwrap_err();
        assert!(
            matches!(err, DrmpackError::InvalidConfig(msg) if msg.contains("Duplicate rendition id 'v1'"))
        );
    }

    #[tokio::test]
    async fn test_verify_hls_endlist_missing_directory() {
        let non_existent = std::env::temp_dir().join(format!("drmpack_missing_{}", Uuid::new_v4()));
        let res = verify_hls_endlist(&non_existent).await;
        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), DrmpackError::Session(msg) if msg.contains("does not exist")));
    }

    #[tokio::test]
    async fn test_verify_hls_endlist_no_media_playlists() {
        let dir = std::env::temp_dir().join(format!("drmpack_no_m3u8_{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let res = verify_hls_endlist(&dir).await;
        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), DrmpackError::Session(msg) if msg.contains("No HLS media playlists found")));
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn test_verify_hls_endlist_skips_master_playlist() {
        let dir = std::env::temp_dir().join(format!("drmpack_master_only_{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let master_path = dir.join("master.m3u8");
        tokio::fs::write(&master_path, "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1280000\nv1.m3u8\n").await.unwrap();
        let res = verify_hls_endlist(&dir).await;
        assert!(res.is_err());
        let content = tokio::fs::read_to_string(&master_path).await.unwrap();
        assert!(!content.contains("#EXT-X-ENDLIST"));
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn test_verify_hls_endlist_appends_and_verifies() {
        let dir = std::env::temp_dir().join(format!("drmpack_append_endlist_{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let media_path = dir.join("live_1.m3u8");
        let initial = "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\nsegment1.m4s\n";
        tokio::fs::write(&media_path, initial).await.unwrap();

        let res = verify_hls_endlist(&dir).await;
        assert!(res.is_ok());

        let content = tokio::fs::read_to_string(&media_path).await.unwrap();
        assert!(content.contains("#EXT-X-ENDLIST"));
        assert!(content.ends_with("#EXT-X-ENDLIST\n"));

        // Re-verification shouldn't duplicate the tag
        let res2 = verify_hls_endlist(&dir).await;
        assert!(res2.is_ok());
        let content2 = tokio::fs::read_to_string(&media_path).await.unwrap();
        assert_eq!(content2.matches("#EXT-X-ENDLIST").count(), 1);

        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[tokio::test]
    async fn test_session_is_alive_healthcheck() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac");

        let mut session = PackagingSession::create(config, provider()).await.unwrap();
        assert!(session.is_alive(), "Active session must report is_alive() == true");

        let _ = session.close().await;
        assert!(!session.is_alive(), "Closed session must report is_alive() == false");
        let _ = session.cleanup().await;
    }

    #[tokio::test]
    async fn test_session_fail_fast_on_premature_exit() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_test_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac");

        let mut session = PackagingSession::create(config, provider()).await.unwrap();
        assert!(session.is_alive());

        // Prematurely kill the GPAC subprocess
        session.representations()[0].kill();

        // Allow ProcessSupervisor to detect exit and watchdog to process it
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(!session.is_alive(), "Session must not be alive after crash");

        // push_bytes must fail immediately with PackagingSession failure containing PackagingOperation::Supervisor
        let res = session.push_bytes(b"dummy fmp4 data").await;
        assert!(res.is_err(), "push_bytes must fail after crash");
        let DrmpackError::PackagingSession(failure) = res.unwrap_err() else {
            panic!("Expected PackagingSession structured failure");
        };

        assert!(failure.cenc.iter().any(|f| f.operation == PackagingOperation::Supervisor));
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

        let session = PackagingSession::create(config, provider()).await.unwrap();
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
        assert!(!cbcs_rep.is_alive(), "CBCS peer must be torn down symmetrically");

        // Status check reports failure
        let status = session.check_status().await;
        assert!(status.is_err());
        let DrmpackError::PackagingSession(failure) = status.unwrap_err() else {
            panic!("Expected structured PackagingSession failure");
        };
        assert_eq!(failure.cenc.len(), 1, "CENC must have exactly 1 crash failure");
        assert_eq!(failure.cbcs.len(), 1, "CBCS peer must have exactly 1 symmetric abort failure");
        assert_eq!(failure.cenc[0].operation, PackagingOperation::Supervisor);
        assert_eq!(failure.cbcs[0].operation, PackagingOperation::Supervisor);
        assert!(matches!(failure.cenc[0].error, DrmpackError::ProcessCrashed { .. }));

        let _ = session.cleanup().await;
    }

    #[tokio::test]
    async fn test_push_segment_init_only_does_not_require_endlist() {
        let output_dir = std::env::temp_dir().join(format!("drmpack_init_only_{}", Uuid::new_v4()));
        let config = PackagingSessionConfig::new("test")
            .with_rendition(rendition())
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac");

        let mut session = PackagingSession::create(config, provider()).await.unwrap();
        assert!(!session.has_pushed_media.load(Ordering::Acquire));

        // Push only an initialization segment (is_init: true)
        session
            .push_segment(Segment {
                rendition_id: "v1".into(),
                sequence_number: 0,
                duration_seconds: 0.0,
                data: bytes::Bytes::new(),
                is_init: true,
            })
            .await
            .expect("Init segment write must succeed");

        // Init segment must not mark has_pushed_media as true
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
        let media_path = dir.join("live_1.m3u8");
        tokio::fs::write(&media_path, "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2.0,\nseg1.m4s\n")
            .await
            .unwrap();

        // Revoke write permission from directory so atomic temp write fails
        tokio::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555))
            .await
            .unwrap();

        let res = verify_hls_endlist(&dir).await;
        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), DrmpackError::Io(_)));

        // Restore permissions for cleanup
        let _ = tokio::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).await;
        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }
}
