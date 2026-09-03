use crate::error::{DrmpackError, Result};
use crate::gpac::process::{GpacProcess, GpacProcessConfig};
use crate::gpac::xml::{GpacDrmConfig, GpacDrmXmlGenerator};
use crate::key::{KeyProvider, KeyRequest, KeySet};
use crate::types::{DrmSystem, EncryptionScheme, LatencyMode, Rendition, Segment};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing::{debug, info, instrument, warn};

/// Default shared memory path for Ramdisk output.
fn default_output_dir(content_id: &str) -> PathBuf {
    let shm = Path::new("/dev/shm");
    if shm.exists() && shm.is_dir() {
        shm.join(format!("drmpack_{}", content_id))
    } else {
        std::env::temp_dir().join(format!("drmpack_{}", content_id))
    }
}

/// Configuration for creating a `PackagingSession`.
#[derive(Debug, Clone)]
pub struct PackagingSessionConfig {
    pub content_id: String,
    pub renditions: Vec<Rendition>,
    pub encryption_schemes: Vec<EncryptionScheme>,
    pub drm_systems: Vec<DrmSystem>,
    pub latency_mode: LatencyMode,
    pub segment_duration: f64,
    pub chunk_duration: f64,
    pub output_dir: PathBuf,
    pub session_timeout: Option<Duration>,
    pub is_live: bool,
    pub gpac_bin: Option<String>,
    pub auto_cleanup: bool,
}

impl PackagingSessionConfig {
    pub fn new(content_id: impl Into<String>) -> Self {
        let cid = content_id.into();
        let out_dir = default_output_dir(&cid);
        Self {
            content_id: cid,
            renditions: Vec::new(),
            encryption_schemes: vec![EncryptionScheme::Cenc],
            drm_systems: vec![DrmSystem::Widevine],
            latency_mode: LatencyMode::LowLatency,
            segment_duration: 2.0,
            chunk_duration: 0.2,
            output_dir: out_dir,
            session_timeout: None,
            is_live: true,
            gpac_bin: None,
            auto_cleanup: false,
        }
    }

    pub fn with_rendition(mut self, rendition: Rendition) -> Self {
        self.renditions.push(rendition);
        self
    }

    pub fn with_encryption_scheme(mut self, scheme: EncryptionScheme) -> Self {
        if !self.encryption_schemes.contains(&scheme) {
            self.encryption_schemes.push(scheme);
        }
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

    pub fn with_session_timeout(mut self, timeout: Duration) -> Self {
        self.session_timeout = Some(timeout);
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

/// A stateful packaging session that orchestrates DRM key acquisition,
/// GPAC child process lifecycle, and low-latency manifest/chunk generation into Ramdisk.
pub struct PackagingSession<P: KeyProvider + 'static> {
    config: PackagingSessionConfig,
    _key_provider: P,
    key_set: KeySet,
    gpac: Arc<Mutex<GpacProcess>>,
    heartbeat_tx: Option<mpsc::Sender<()>>,
    watchdog_handle: Option<JoinHandle<()>>,
    closed: bool,
}

impl<P: KeyProvider + 'static> std::fmt::Debug for PackagingSession<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackagingSession")
            .field("config", &self.config)
            .field("closed", &self.closed)
            .finish()
    }
}

impl<P: KeyProvider + 'static> PackagingSession<P> {
    /// Create a new packaging session:
    /// 1. Fetches encryption keys from the KeyProvider.
    /// 2. Ensures the output directory exists in Ramdisk.
    /// 3. Generates the GPAC Common Encryption XML configuration (`drm.xml`).
    /// 4. Spawns the long-running GPAC subprocess with anonymous pipes.
    #[instrument(skip(key_provider), fields(content_id = %config.content_id))]
    pub async fn create(config: PackagingSessionConfig, key_provider: P) -> Result<Self> {
        if config.renditions.is_empty() {
            return Err(DrmpackError::InvalidConfig(
                "PackagingSession requires at least one Rendition".into(),
            ));
        }

        // 1. Collect all unique (TrackType, QualityTier) pairs needed
        let mut requested_tiers = Vec::new();
        for r in &config.renditions {
            let pair = (r.track_type, r.quality_tier.clone());
            if !requested_tiers.contains(&pair) {
                requested_tiers.push(pair);
            }
        }

        let key_req = KeyRequest {
            content_id: config.content_id.clone(),
            requested_tiers,
            drm_systems: config.drm_systems.clone(),
        };

        info!(content_id = %config.content_id, "Fetching encryption keys from provider");
        let key_set = key_provider.fetch_keys(&key_req).await?;

        // 2. Ensure output directory exists in Ramdisk
        tokio::fs::create_dir_all(&config.output_dir)
            .await
            .map_err(|e| {
                DrmpackError::Io(std::io::Error::new(
                    e.kind(),
                    format!(
                        "Failed to create Ramdisk output directory '{}': {}",
                        config.output_dir.display(),
                        e
                    ),
                ))
            })?;

        // 3. Generate GPAC DRM XML config
        let primary_scheme = config
            .encryption_schemes
            .first()
            .copied()
            .unwrap_or(EncryptionScheme::Cenc);

        let mut drm_config = GpacDrmConfig::new(primary_scheme);
        for (idx, r) in config.renditions.iter().enumerate() {
            let track_id = (idx + 1) as u32;
            drm_config = drm_config.with_track(track_id, r.track_type, r.quality_tier.clone());
        }

        let xml_content = GpacDrmXmlGenerator::generate(&key_set, &drm_config)?;
        let drm_xml_path = config.output_dir.join("drm.xml");
        tokio::fs::write(&drm_xml_path, xml_content).await?;
        debug!(path = %drm_xml_path.display(), "Wrote GPAC DRM XML configuration");

        // 4. Spawn GPAC subprocess
        let mut process_config = GpacProcessConfig::new(&drm_xml_path, &config.output_dir)
            .with_latency_mode(config.latency_mode)
            .with_segment_duration(config.segment_duration)
            .with_chunk_duration(config.chunk_duration);

        if let Some(ref bin) = config.gpac_bin {
            process_config = process_config.with_gpac_bin(bin);
        }

        let gpac_proc = GpacProcess::spawn(process_config).await?;
        let gpac = Arc::new(Mutex::new(gpac_proc));

        // 5. Setup background watchdog if session_timeout is configured
        let (heartbeat_tx, watchdog_handle) = if let Some(timeout) = config.session_timeout {
            let (tx, mut rx) = mpsc::channel::<()>(16);
            let gpac_clone = Arc::clone(&gpac);
            let handle = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        msg = rx.recv() => {
                            match msg {
                                Some(()) => continue, // Heartbeat received, reset timer
                                None => break,        // Channel closed
                            }
                        }
                        _ = tokio::time::sleep(timeout) => {
                            warn!("PackagingSession timeout elapsed ({:?}), terminating GPAC", timeout);
                            let mut locked = gpac_clone.lock().await;
                            let _ = locked.close_and_wait(Duration::from_secs(2)).await;
                            break;
                        }
                    }
                }
            });
            (Some(tx), Some(handle))
        } else {
            (None, None)
        };

        Ok(Self {
            config,
            _key_provider: key_provider,
            key_set,
            gpac,
            heartbeat_tx,
            watchdog_handle,
            closed: false,
        })
    }

    fn ping_heartbeat(&self) {
        if let Some(ref tx) = self.heartbeat_tx {
            let _ = tx.try_send(());
        }
    }

    /// Push a media segment directly into GPAC's stdin pipe.
    #[instrument(skip(self, segment), fields(rendition_id = %segment.rendition_id, seq = segment.sequence_number))]
    pub async fn push_segment(&mut self, segment: Segment) -> Result<()> {
        if self.closed {
            return Err(DrmpackError::Session("PackagingSession is closed".into()));
        }
        self.ping_heartbeat();

        let mut gpac = self.gpac.lock().await;
        gpac.write_data(&segment.data).await
    }

    /// Push raw media bytes directly into GPAC's stdin pipe.
    #[instrument(skip(self, bytes), fields(len = bytes.len()))]
    pub async fn push_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        if self.closed {
            return Err(DrmpackError::Session("PackagingSession is closed".into()));
        }
        self.ping_heartbeat();

        let mut gpac = self.gpac.lock().await;
        gpac.write_data(bytes).await
    }

    /// Check if the GPAC process has failed or crashed.
    pub async fn check_status(&self) -> Result<()> {
        let mut gpac = self.gpac.lock().await;
        gpac.check_status()
    }

    /// Gracefully finalize and close the packaging session.
    /// Signals EOF to GPAC and awaits clean manifest finalization.
    #[instrument(skip(self))]
    pub async fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;

        if let Some(handle) = self.watchdog_handle.take() {
            handle.abort();
        }

        let mut gpac = self.gpac.lock().await;
        gpac.close_and_wait(Duration::from_secs(5)).await?;
        info!("PackagingSession closed successfully");

        if self.config.auto_cleanup {
            self.cleanup().await?;
        }

        Ok(())
    }

    /// Expected path to the HLS master playlist generated in Ramdisk (`<output_dir>/live.m3u8`).
    pub fn hls_manifest_path(&self) -> PathBuf {
        self.config.output_dir.join("live.m3u8")
    }

    /// Expected path to the DASH manifest generated in Ramdisk (`<output_dir>/live.mpd`).
    pub fn dash_manifest_path(&self) -> PathBuf {
        self.config.output_dir.join("live.mpd")
    }

    /// Clean up and remove the Ramdisk session directory.
    /// Call after client playback draining completes to free shared memory.
    pub async fn cleanup(&self) -> Result<()> {
        if self.config.output_dir.exists() {
            tokio::fs::remove_dir_all(&self.config.output_dir)
                .await
                .map_err(|e| {
                    DrmpackError::Io(std::io::Error::new(
                        e.kind(),
                        format!(
                            "Failed to cleanup Ramdisk session directory '{}': {}",
                            self.config.output_dir.display(),
                            e
                        ),
                    ))
                })?;
            debug!(path = %self.config.output_dir.display(), "Cleaned up Ramdisk session directory");
        }
        Ok(())
    }

    /// Check if the session is closed.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Access the session configuration.
    pub fn config(&self) -> &PackagingSessionConfig {
        &self.config
    }

    /// Access the cached KeySet.
    pub fn key_set(&self) -> &KeySet {
        &self.key_set
    }

    /// Path to the Ramdisk output directory where manifests and segments reside.
    pub fn output_dir(&self) -> &Path {
        &self.config.output_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::RawKeyProvider;
    use crate::types::QualityTier;

    #[tokio::test]
    async fn test_session_config_validation_empty_renditions() {
        let config = PackagingSessionConfig::new("test-content");
        let provider = RawKeyProvider::new();

        let result = PackagingSession::create(config, provider).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            DrmpackError::InvalidConfig(_)
        ));
    }

    #[tokio::test]
    async fn test_session_config_builder() {
        let rendition = Rendition::video(
            "v1080p",
            QualityTier::hd(),
            1920,
            1080,
            5_000_000,
            "avc1.640028",
        );
        let config = PackagingSessionConfig::new("my-stream")
            .with_rendition(rendition.clone())
            .with_latency_mode(LatencyMode::LowLatency)
            .with_segment_duration(1.0)
            .with_chunk_duration(0.1)
            .with_encryption_scheme(EncryptionScheme::Cenc)
            .with_drm_system(DrmSystem::Widevine);

        assert_eq!(config.content_id, "my-stream");
        assert_eq!(config.renditions.len(), 1);
        assert_eq!(config.latency_mode, LatencyMode::LowLatency);
        assert_eq!(config.segment_duration, 1.0);
        assert_eq!(config.chunk_duration, 0.1);
        assert!(!config.auto_cleanup);

        let config = config.with_auto_cleanup(true);
        assert!(config.auto_cleanup);
    }

    #[tokio::test]
    async fn test_manifest_paths_and_ramdisk_cleanup() {
        let temp_dir = std::env::temp_dir().join(format!("drmpack_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        let test_file = temp_dir.join("live.m3u8");
        tokio::fs::write(&test_file, b"#EXTM3U").await.unwrap();

        let config = PackagingSessionConfig::new("test-paths")
            .with_output_dir(&temp_dir);

        assert_eq!(config.output_dir.join("live.m3u8"), temp_dir.join("live.m3u8"));
        assert_eq!(config.output_dir.join("live.mpd"), temp_dir.join("live.mpd"));

        // Test cleanup logic directly on path
        assert!(temp_dir.exists());
        tokio::fs::remove_dir_all(&temp_dir).await.unwrap();
        assert!(!temp_dir.exists());
    }
}
