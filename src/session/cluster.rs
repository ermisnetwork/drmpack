use crate::error::{
    DrmpackError, PackagingOperation, PackagingSessionFailure, RepresentationFailure, Result,
};
use crate::gpac::process::{GpacProcess, GpacProcessConfig};
use crate::gpac::xml::{GpacDrmConfig, GpacDrmXmlGenerator, GpacTrackConfig};
use crate::key::KeySet;
use crate::session::PackagingSessionConfig;
use crate::types::EncryptionScheme;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// An individual media Representation managed within a `RepresentationCluster`.
pub struct Representation {
    pub scheme: EncryptionScheme,
    pub gpac: Arc<Mutex<GpacProcess>>,
}

impl Representation {
    pub fn new(scheme: EncryptionScheme, gpac: GpacProcess) -> Self {
        Self {
            scheme,
            gpac: Arc::new(Mutex::new(gpac)),
        }
    }

    pub async fn write_data(&self, bytes: &[u8]) -> (EncryptionScheme, Result<()>) {
        (self.scheme, self.gpac.lock().await.write_data(bytes).await)
    }

    pub async fn check_status(&self) -> Result<()> {
        self.gpac.lock().await.check_status()
    }

    pub async fn close_and_wait(&self, timeout: Duration) -> Result<()> {
        self.gpac.lock().await.close_and_wait(timeout).await
    }
}

impl std::fmt::Debug for Representation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Representation")
            .field("scheme", &self.scheme)
            .finish_non_exhaustive()
    }
}

/// The coordinated group of one or more active Representations managed together
/// for a `PackagingSession`, handling joint media fan-out, lifecycle, and teardown.
pub struct RepresentationCluster {
    representations: Vec<Representation>,
}

impl std::fmt::Debug for RepresentationCluster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepresentationCluster")
            .field("schemes", &self.schemes())
            .finish()
    }
}

impl RepresentationCluster {
    /// Spawn GPAC processes for each concrete scheme defined in `config.encryption_scheme`.
    ///
    /// Performs atomic spawn rollback: if creating directories, generating DRM XML,
    /// or spawning any GPAC process fails, any already spawned processes are immediately
    /// finalized and rolled back, created directories are cleaned up, and a structured
    /// `DrmpackError::PackagingSession` error is returned.
    pub async fn spawn(
        config: &PackagingSessionConfig,
        key_set: &KeySet,
        control_dir: &Path,
        output_dir_created: bool,
    ) -> Result<Self> {
        let is_dual = config.encryption_scheme == EncryptionScheme::Dual;
        let schemes = config.encryption_scheme.concrete_schemes();
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
                rollback_creation(&config.output_dir, output_dir_created, Some(control_dir)).await;
                return Err(creation_failure(
                    scheme,
                    DrmpackError::Io(error),
                    shutdown_failures,
                ));
            }

            let drm_path = control_dir.join(format!("{scheme}.xml"));
            let mut drm_config = GpacDrmConfig::new(scheme);
            for (index, rendition) in config.renditions.iter().enumerate() {
                let track_id = rendition.effective_track_id(index);
                let mut track = GpacTrackConfig::new(
                    track_id,
                    rendition.track_type,
                    rendition.quality_tier.clone(),
                );
                track.encrypted = rendition.encrypted;
                drm_config.tracks.push(track);
            }

            let xml = match GpacDrmXmlGenerator::generate(key_set, &drm_config) {
                Ok(xml) => xml,
                Err(error) => {
                    let shutdown_failures =
                        shutdown_representations(&mut representations, config.finalization_timeout)
                            .await;
                    rollback_creation(&config.output_dir, output_dir_created, Some(control_dir))
                        .await;
                    return Err(creation_failure(scheme, error, shutdown_failures));
                }
            };

            if let Err(error) = tokio::fs::write(&drm_path, xml).await {
                let shutdown_failures =
                    shutdown_representations(&mut representations, config.finalization_timeout)
                        .await;
                rollback_creation(&config.output_dir, output_dir_created, Some(control_dir)).await;
                return Err(creation_failure(
                    scheme,
                    DrmpackError::Io(error),
                    shutdown_failures,
                ));
            }

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ =
                    tokio::fs::set_permissions(&drm_path, std::fs::Permissions::from_mode(0o600))
                        .await;
            }

            let mut process_config = GpacProcessConfig::new(&drm_path, &output_dir)
                .with_latency_mode(config.latency_mode)
                .with_segment_duration(config.segment_duration)
                .with_chunk_duration(config.chunk_duration);
            if let Some(bin) = &config.gpac_bin {
                process_config = process_config.with_gpac_bin(bin);
            }

            match GpacProcess::spawn(process_config).await {
                Ok(process) => {
                    representations.push(Representation::new(scheme, process));
                }
                Err(error) => {
                    let shutdown_failures =
                        shutdown_representations(&mut representations, config.finalization_timeout)
                            .await;
                    rollback_creation(&config.output_dir, output_dir_created, Some(control_dir))
                        .await;
                    return Err(creation_failure(scheme, error, shutdown_failures));
                }
            }
        }

        Ok(Self { representations })
    }

    /// Push raw media bytes concurrently to all active Representations via `tokio::join!`.
    pub async fn write_data(&self, bytes: &[u8]) -> Vec<RepresentationFailure> {
        let write_results = match self.representations.as_slice() {
            [] => Vec::new(),
            [rep] => vec![rep.write_data(bytes).await],
            [first, second] => {
                let (first_result, second_result) =
                    tokio::join!(first.write_data(bytes), second.write_data(bytes),);
                vec![first_result, second_result]
            }
            reps => {
                let mut results = Vec::with_capacity(reps.len());
                for rep in reps {
                    results.push(rep.write_data(bytes).await);
                }
                results
            }
        };
        write_results
            .into_iter()
            .filter_map(|(scheme, result)| {
                result.err().map(|error| {
                    RepresentationFailure::new(scheme, PackagingOperation::Write, error)
                })
            })
            .collect()
    }

    /// Check all Representations for unexpected child exits.
    pub async fn check_status(&self) -> Vec<RepresentationFailure> {
        let mut failures = Vec::new();
        for rep in &self.representations {
            if let Err(error) = rep.check_status().await {
                failures.push(RepresentationFailure::new(
                    rep.scheme,
                    PackagingOperation::Status,
                    error,
                ));
            }
        }
        failures
    }

    /// Finalize and close all Representations concurrently.
    pub async fn close(
        &self,
        finalization_timeout: Duration,
        operation: PackagingOperation,
    ) -> Vec<RepresentationFailure> {
        match self.representations.as_slice() {
            [] => Vec::new(),
            [rep] => {
                let mut failures = Vec::new();
                if let Err(error) = rep.close_and_wait(finalization_timeout).await {
                    failures.push(RepresentationFailure::new(rep.scheme, operation, error));
                }
                failures
            }
            [first, second] => {
                let (first_res, second_res) = tokio::join!(
                    first.close_and_wait(finalization_timeout),
                    second.close_and_wait(finalization_timeout),
                );
                let mut failures = Vec::new();
                if let Err(error) = first_res {
                    failures.push(RepresentationFailure::new(first.scheme, operation, error));
                }
                if let Err(error) = second_res {
                    failures.push(RepresentationFailure::new(second.scheme, operation, error));
                }
                failures
            }
            _ => {
                let mut failures = Vec::new();
                for rep in &self.representations {
                    if let Err(error) = rep.close_and_wait(finalization_timeout).await {
                        failures.push(RepresentationFailure::new(rep.scheme, operation, error));
                    }
                }
                failures
            }
        }
    }

    pub fn schemes(&self) -> Vec<EncryptionScheme> {
        self.representations.iter().map(|r| r.scheme).collect()
    }

    pub fn has_scheme(&self, scheme: EncryptionScheme) -> bool {
        self.representations.iter().any(|r| r.scheme == scheme)
    }

    pub fn len(&self) -> usize {
        self.representations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.representations.is_empty()
    }

    pub fn representations(&self) -> &[Representation] {
        &self.representations
    }
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
            failures.push(RepresentationFailure::new(
                scheme,
                PackagingOperation::Close,
                error,
            ));
        }
    }
    failures
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::{ContentKey, KeyID};
    use crate::types::{QualityTier, Rendition, TrackType};
    use uuid::Uuid;

    #[tokio::test]
    async fn test_cluster_spawn_rollback_on_missing_gpac() {
        let output_dir =
            std::env::temp_dir().join(format!("drmpack_test_cluster_{}", Uuid::new_v4()));
        let control_dir =
            std::env::temp_dir().join(format!("drmpack_control_cluster_{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&control_dir).await.unwrap();

        let config = PackagingSessionConfig::new("cluster-test")
            .with_rendition(Rendition::video(
                "v1",
                QualityTier::hd(),
                1920,
                1080,
                5_000_000,
                "avc1.640028",
            ))
            .with_encryption_scheme(EncryptionScheme::Dual)
            .with_output_dir(&output_dir)
            .with_gpac_bin("nonexistent-gpac-bin");

        let mut key_set = KeySet::new();
        key_set.insert_key(ContentKey::new(
            KeyID::random(),
            [0x11; 16],
            QualityTier::hd(),
            TrackType::Video,
        ));

        let result = RepresentationCluster::spawn(&config, &key_set, &control_dir, true).await;
        assert!(result.is_err());
        let DrmpackError::PackagingSession(failure) = result.unwrap_err() else {
            panic!("Expected PackagingSession error");
        };
        assert_eq!(failure.cenc.len(), 1);
        assert_eq!(failure.cenc[0].operation, PackagingOperation::Create);
        assert!(!output_dir.exists());
        assert!(!control_dir.exists());
    }

    #[tokio::test]
    async fn test_cluster_spawn_rollback_on_partial_failure() {
        let output_dir =
            std::env::temp_dir().join(format!("drmpack_test_cluster_partial_{}", Uuid::new_v4()));
        let control_dir = std::env::temp_dir().join(format!(
            "drmpack_control_cluster_partial_{}",
            Uuid::new_v4()
        ));
        tokio::fs::create_dir_all(&control_dir).await.unwrap();

        let config = PackagingSessionConfig::new("cluster-partial-test")
            .with_rendition(Rendition::video(
                "v1",
                QualityTier::hd(),
                1920,
                1080,
                5_000_000,
                "avc1.640028",
            ))
            .with_encryption_scheme(EncryptionScheme::Dual)
            .with_output_dir(&output_dir)
            .with_gpac_bin("gpac");

        // KeySet only contains CENC key, so Cbcs XML generation will fail
        let mut key_set = KeySet::new();
        key_set.insert_key(ContentKey::new_with_scheme(
            KeyID::random(),
            [0x11; 16],
            QualityTier::hd(),
            TrackType::Video,
            EncryptionScheme::Cenc,
        ));

        let result = RepresentationCluster::spawn(&config, &key_set, &control_dir, true).await;
        assert!(result.is_err());
        let DrmpackError::PackagingSession(failure) = result.unwrap_err() else {
            panic!("Expected PackagingSession error");
        };
        // CBCS failed during create
        assert_eq!(failure.cbcs.len(), 1);
        assert_eq!(failure.cbcs[0].operation, PackagingOperation::Create);
        // Cleaned up directories
        assert!(!output_dir.exists());
        assert!(!control_dir.exists());
    }
}
