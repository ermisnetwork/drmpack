use crate::error::{
    DrmpackError, PackagingOperation, PackagingSessionFailure, RepresentationFailure, Result,
};
use crate::gpac::process::{GpacProcess, GpacProcessConfig};
use crate::gpac::xml::{GpacDrmConfig, GpacDrmXmlGenerator};
use crate::key::{KeyProvider, KeyRequest, KeySet};
use crate::types::{DrmSystem, EncryptionScheme, LatencyMode, ManifestFormat, Rendition, Segment};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing::{debug, info, instrument, warn};
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
        }
    }

    pub fn with_rendition(mut self, rendition: Rendition) -> Self {
        self.renditions.push(rendition);
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

struct Representation {
    scheme: EncryptionScheme,
    gpac: Arc<Mutex<GpacProcess>>,
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
/// # Dual topology warning
/// `EncryptionScheme::Dual` currently reuses one temporary KeySet for both CENC and CBCS
/// Representations. It proves packaging topology only and is not production-safe until
/// scheme-aware selection provides distinct ContentKeys and KIDs for each Representation.
pub struct PackagingSession<P: KeyProvider + 'static> {
    config: PackagingSessionConfig,
    _key_provider: P,
    key_set: KeySet,
    representations: Vec<Representation>,
    control_dir: PathBuf,
    lifecycle: Arc<Mutex<Lifecycle>>,
    is_terminal: Arc<AtomicBool>,
    heartbeat_tx: Option<mpsc::Sender<()>>,
    watchdog_handle: Option<JoinHandle<()>>,
}

impl<P: KeyProvider + 'static> std::fmt::Debug for PackagingSession<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackagingSession")
            .field("config", &self.config)
            .field(
                "representations",
                &self
                    .representations
                    .iter()
                    .map(|representation| representation.scheme)
                    .collect::<Vec<_>>(),
            )
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
                rollback_creation(&config.output_dir, output_dir_created, None).await;
                return Err(error);
            }
        };

        let schemes = concrete_schemes(config.encryption_scheme);
        let mut representations = Vec::with_capacity(schemes.len());

        for &scheme in schemes {
            let output_dir = if is_dual {
                config.output_dir.join(scheme.to_string())
            } else {
                config.output_dir.clone()
            };
            if let Err(error) = tokio::fs::create_dir_all(&output_dir).await {
                let shutdown_failures =
                    shutdown_representations(&mut representations, config.finalization_timeout)
                        .await;
                rollback_creation(&config.output_dir, output_dir_created, Some(&control_dir)).await;
                return Err(creation_failure(
                    scheme,
                    DrmpackError::Io(error),
                    shutdown_failures,
                ));
            }

            let drm_path = control_dir.join(format!("{scheme}.xml"));
            let result =
                spawn_representation(&config, &key_set, scheme, &output_dir, &drm_path).await;
            match result {
                Ok(representation) => representations.push(representation),
                Err(error) => {
                    let shutdown_failures =
                        shutdown_representations(&mut representations, config.finalization_timeout)
                            .await;
                    rollback_creation(&config.output_dir, output_dir_created, Some(&control_dir))
                        .await;
                    return Err(creation_failure(scheme, error, shutdown_failures));
                }
            }
        }

        let lifecycle = Arc::new(Mutex::new(Lifecycle::new()));
        let is_terminal = Arc::new(AtomicBool::new(false));
        let (heartbeat_tx, watchdog_handle) = build_watchdog(
            config.session_timeout,
            config.finalization_timeout,
            config.auto_cleanup.then(|| config.output_dir.clone()),
            control_dir.clone(),
            Arc::clone(&lifecycle),
            Arc::clone(&is_terminal),
            representations
                .iter()
                .map(|representation| (representation.scheme, Arc::clone(&representation.gpac)))
                .collect(),
        );

        Ok(Self {
            config,
            _key_provider: key_provider,
            key_set,
            representations,
            control_dir,
            lifecycle,
            is_terminal,
            heartbeat_tx,
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
        self.push_bytes(&segment.data).await
    }

    /// Push raw media bytes to every active Representation.
    #[instrument(skip(self, bytes), fields(len = bytes.len()))]
    pub async fn push_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.ensure_active().await?;
        self.ping_heartbeat();

        let write_results = match self.representations.as_slice() {
            [] => Vec::new(),
            [representation] => vec![write_representation(representation, bytes).await],
            [first, second] => {
                let (first_result, second_result) = tokio::join!(
                    write_representation(first, bytes),
                    write_representation(second, bytes),
                );
                vec![first_result, second_result]
            }
            _ => unreachable!("PackagingSession supports at most CENC and CBCS Representations"),
        };
        let failures = write_results
            .into_iter()
            .filter_map(|(scheme, result)| {
                result
                    .err()
                    .map(|error| RepresentationFailure::new(scheme, PackagingOperation::Write, error))
            })
            .collect();

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

        let mut failures = Vec::new();
        for representation in &self.representations {
            let result = representation.gpac.lock().await.check_status();
            if let Err(error) = result {
                failures.push(RepresentationFailure::new(
                    representation.scheme,
                    PackagingOperation::Status,
                    error,
                ));
            }
        }

        self.finish_operation_failures(failures).await
    }

    /// Gracefully finalize every Representation. Both subprocesses are always attempted,
    /// even when an earlier finalization fails.
    #[instrument(skip(self))]
    pub async fn close(&mut self) -> Result<()> {
        let state = { self.lifecycle.lock().await.state };
        match state {
            SessionState::Closed => return Ok(()),
            SessionState::Failed => {
                self.stop_watchdog();
                let _ = self.cleanup_control_dir().await;
                if self.config.auto_cleanup {
                    let _ = self.cleanup_output_dir().await;
                }
                return Err(self.lifecycle.lock().await.failure_error());
            }
            SessionState::Closing => return Ok(()),
            SessionState::Active => {
                self.lifecycle.lock().await.state = SessionState::Closing;
            }
        }

        self.stop_watchdog();
        let failures = self.close_representations(PackagingOperation::Close).await;
        let control_cleanup = self.cleanup_control_dir().await.err();
        let output_cleanup = if self.config.auto_cleanup {
            self.cleanup_output_dir().await.err()
        } else {
            None
        };

        if failures.is_empty() && control_cleanup.is_none() && output_cleanup.is_none() {
            self.lifecycle.lock().await.state = SessionState::Closed;
            self.is_terminal.store(true, Ordering::Release);
            info!("PackagingSession closed successfully");
            Ok(())
        } else {
            {
                let mut lifecycle = self.lifecycle.lock().await;
                lifecycle.state = SessionState::Failed;
            }
            self.is_terminal.store(true, Ordering::Release);
            Err(self
                .record_new_failure(failures, output_cleanup, control_cleanup)
                .await)
        }
    }

    /// Resolve a deterministic public Manifest path without waiting for GPAC to write it.
    pub fn manifest_path(
        &self,
        scheme: EncryptionScheme,
        format: ManifestFormat,
    ) -> Result<PathBuf> {
        let is_dual = self.config.encryption_scheme == EncryptionScheme::Dual;
        if scheme == EncryptionScheme::Dual || !self.has_representation(scheme) {
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
        {
            let mut lifecycle = self.lifecycle.lock().await;
            if lifecycle.state == SessionState::Failed {
                return lifecycle.failure_error();
            }
            lifecycle.state = SessionState::Failed;
        }

        self.stop_watchdog();
        failures.extend(self.close_representations(PackagingOperation::Close).await);
        let control_cleanup = self.cleanup_control_dir().await.err();
        let output_cleanup = if self.config.auto_cleanup {
            self.cleanup_output_dir().await.err()
        } else {
            None
        };
        self.record_new_failure(failures, output_cleanup, control_cleanup)
            .await
    }

    async fn record_new_failure(
        &self,
        failures: Vec<RepresentationFailure>,
        output_cleanup: Option<DrmpackError>,
        control_cleanup: Option<DrmpackError>,
    ) -> DrmpackError {
        let failure = Arc::new(
            PackagingSessionFailure::from_failures(failures)
                .with_cleanup_failures(output_cleanup, control_cleanup),
        );
        let mut lifecycle = self.lifecycle.lock().await;
        lifecycle.terminal_failure = Some(Arc::clone(&failure));
        self.is_terminal.store(true, Ordering::Release);
        DrmpackError::PackagingSession(failure)
    }

    async fn close_representations(&self, operation: PackagingOperation) -> Vec<RepresentationFailure> {
        let mut failures = Vec::new();
        for representation in &self.representations {
            let result = representation
                .gpac
                .lock()
                .await
                .close_and_wait(self.config.finalization_timeout)
                .await;
            if let Err(error) = result {
                failures.push(RepresentationFailure::new(representation.scheme, operation, error));
            }
        }
        failures
    }

    fn stop_watchdog(&self) {
        if let Some(handle) = &self.watchdog_handle {
            handle.abort();
        }
    }

    fn has_representation(&self, scheme: EncryptionScheme) -> bool {
        self.representations
            .iter()
            .any(|representation| representation.scheme == scheme)
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

    #[cfg(test)]
    fn control_dir_path(&self) -> &Path {
        &self.control_dir
    }
}

impl<P: KeyProvider + 'static> Drop for PackagingSession<P> {
    fn drop(&mut self) {
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
    Ok(())
}

async fn fetch_key_set<P: KeyProvider>(
    config: &PackagingSessionConfig,
    provider: &P,
) -> Result<KeySet> {
    let mut requested_tiers = Vec::new();
    for rendition in &config.renditions {
        let pair = (rendition.track_type, rendition.quality_tier.clone());
        if !requested_tiers.contains(&pair) {
            requested_tiers.push(pair);
        }
    }

    let drm_systems = if config.drm_systems.is_empty() {
        vec![DrmSystem::Widevine]
    } else {
        config.drm_systems.clone()
    };

    info!(content_id = %config.content_id, "Fetching encryption keys from provider");
    provider
        .fetch_keys(&KeyRequest {
            content_id: config.content_id.clone(),
            requested_tiers,
            drm_systems,
        })
        .await
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
            if is_dual && std::fs::read_dir(output_dir)?.next().is_some() {
                return Err(DrmpackError::InvalidConfig(format!(
                    "Dual PackagingSession output directory '{}' must be empty",
                    output_dir.display()
                )));
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
    tokio::fs::create_dir_all(&parent).await?;
    let control_dir = parent.join(format!("{}-{}", config.content_id, Uuid::new_v4()));
    tokio::fs::create_dir(&control_dir).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&control_dir, std::fs::Permissions::from_mode(0o700)).await?;
    }

    Ok(control_dir)
}

async fn spawn_representation(
    config: &PackagingSessionConfig,
    key_set: &KeySet,
    scheme: EncryptionScheme,
    output_dir: &Path,
    drm_path: &Path,
) -> Result<Representation> {
    debug_assert!(matches!(
        scheme,
        EncryptionScheme::Cenc | EncryptionScheme::Cbcs
    ));

    let mut drm_config = GpacDrmConfig::new(scheme);
    for (index, rendition) in config.renditions.iter().enumerate() {
        drm_config = drm_config.with_track(
            (index + 1) as u32,
            rendition.track_type,
            rendition.quality_tier.clone(),
        );
    }
    let xml = GpacDrmXmlGenerator::generate(key_set, &drm_config)?;
    tokio::fs::write(drm_path, xml).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(drm_path, std::fs::Permissions::from_mode(0o600)).await?;
    }

    let mut process_config = GpacProcessConfig::new(drm_path, output_dir)
        .with_latency_mode(config.latency_mode)
        .with_segment_duration(config.segment_duration)
        .with_chunk_duration(config.chunk_duration);
    if let Some(bin) = &config.gpac_bin {
        process_config = process_config.with_gpac_bin(bin);
    }

    Ok(Representation {
        scheme,
        gpac: Arc::new(Mutex::new(GpacProcess::spawn(process_config).await?)),
    })
}

fn concrete_schemes(mode: EncryptionScheme) -> &'static [EncryptionScheme] {
    match mode {
        EncryptionScheme::Cenc => &[EncryptionScheme::Cenc],
        EncryptionScheme::Cbcs => &[EncryptionScheme::Cbcs],
        EncryptionScheme::Dual => &[EncryptionScheme::Cenc, EncryptionScheme::Cbcs],
    }
}

async fn write_representation(
    representation: &Representation,
    bytes: &[u8],
) -> (EncryptionScheme, Result<()>) {
    (
        representation.scheme,
        representation.gpac.lock().await.write_data(bytes).await,
    )
}

fn creation_failure(
    scheme: EncryptionScheme,
    error: DrmpackError,
    mut shutdown_failures: Vec<RepresentationFailure>,
) -> DrmpackError {
    shutdown_failures.push(RepresentationFailure::new(
        scheme,
        PackagingOperation::Create,
        error,
    ));
    DrmpackError::PackagingSession(Arc::new(PackagingSessionFailure::from_failures(
        shutdown_failures,
    )))
}

async fn shutdown_representations(
    representations: &mut Vec<Representation>,
    finalization_timeout: Duration,
) -> Vec<RepresentationFailure> {
    let mut failures = Vec::new();
    for representation in representations.drain(..) {
        let scheme = representation.scheme;
        if let Err(error) = representation
            .gpac
            .lock()
            .await
            .close_and_wait(finalization_timeout)
            .await
        {
            failures.push(RepresentationFailure::new(scheme, PackagingOperation::Close, error));
        }
    }
    failures
}

async fn rollback_creation(
    output_dir: &Path,
    output_dir_created: bool,
    control_dir: Option<&Path>,
) {
    if let Some(control_dir) = control_dir {
        let _ = tokio::fs::remove_dir_all(control_dir).await;
        if let Some(parent) = control_dir.parent() {
            if parent.file_name().and_then(|n| n.to_str()) == Some("drmpack-control") {
                let _ = tokio::fs::remove_dir(parent).await;
            }
        }
    }
    if output_dir_created {
        let _ = tokio::fs::remove_dir_all(output_dir).await;
    } else {
        let _ = tokio::fs::remove_dir_all(output_dir.join("cenc")).await;
        let _ = tokio::fs::remove_dir_all(output_dir.join("cbcs")).await;
    }
}

fn build_watchdog(
    timeout: Option<Duration>,
    finalization_timeout: Duration,
    output_dir_for_cleanup: Option<PathBuf>,
    control_dir: PathBuf,
    lifecycle: Arc<Mutex<Lifecycle>>,
    is_terminal: Arc<AtomicBool>,
    representations: Vec<(EncryptionScheme, Arc<Mutex<GpacProcess>>)>,
) -> (Option<mpsc::Sender<()>>, Option<JoinHandle<()>>) {
    let Some(timeout) = timeout else {
        return (None, None);
    };

    let (tx, mut rx) = mpsc::channel(16);
    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                heartbeat = rx.recv() => match heartbeat {
                    Some(()) => continue,
                    None => break,
                },
                _ = tokio::time::sleep(timeout) => {
                    warn!(?timeout, "PackagingSession inactivity watchdog elapsed");
                    {
                        let mut lifecycle_guard = lifecycle.lock().await;
                        if lifecycle_guard.state != SessionState::Active {
                            break;
                        }
                        lifecycle_guard.state = SessionState::Failed;
                    }

                    let mut failures = representations
                        .iter()
                        .map(|(scheme, _)| {
                            RepresentationFailure::new(
                                *scheme,
                                PackagingOperation::Watchdog,
                                DrmpackError::Session(
                                    "PackagingSession inactivity watchdog elapsed".into(),
                                ),
                            )
                        })
                        .collect::<Vec<_>>();
                    for (scheme, process) in representations {
                        if let Err(error) = process
                            .lock()
                            .await
                            .close_and_wait(finalization_timeout.min(WATCHDOG_FINALIZATION_TIMEOUT))
                            .await
                        {
                            failures.push(RepresentationFailure::new(
                                scheme,
                                PackagingOperation::Watchdog,
                                error,
                            ));
                        }
                    }
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
                        Some(output_dir) => tokio::fs::remove_dir_all(&output_dir).await.err().map(|error| {
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
                    let mut lifecycle_guard = lifecycle.lock().await;
                    lifecycle_guard.terminal_failure = Some(Arc::new(
                        PackagingSessionFailure::from_failures(failures)
                            .with_cleanup_failures(output_cleanup, control_cleanup),
                    ));
                    is_terminal.store(true, Ordering::Release);
                    break;
                }
            }
        }
    });
    (Some(tx), Some(handle))
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
}
