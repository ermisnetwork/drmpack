//! Core batch execution engine for whole-file VOD packaging.

use crate::error::{DrmpackError, Result};
use crate::gpac::vod::GpacVodProcessConfig;
use crate::gpac::xml::{GpacDrmConfig, GpacDrmXmlGenerator};
use crate::key::{KeyPolicyEngine, KeyProvider, KeySet};
use crate::session::DrmStreamMetadata;
use crate::types::EncryptionScheme;
use crate::vod::{VodInputSource, VodPackageConfig, VodPackageResult};
use std::path::Path;
use tokio::process::Command;
use tracing::{error, info, instrument};
use uuid::Uuid;

/// Package a static media file or set of track files into encrypted VOD DASH and HLS assets.
#[instrument(skip(key_provider), fields(content_id = %config.content_id))]
pub async fn package_vod_file<P: KeyProvider>(
    config: &VodPackageConfig,
    key_provider: &P,
) -> Result<VodPackageResult> {
    // 1. Validate inputs
    validate_config(config)?;

    // 2. Prepare output and control directories
    tokio::fs::create_dir_all(&config.output_dir).await?;

    let control_dir = std::env::temp_dir().join(format!("drmpack_vod_ctrl_{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&control_dir).await?;

    let execution_res = match config.encryption_scheme {
        EncryptionScheme::Dual => {
            let cenc_dir = config.output_dir.join("cenc");
            let cbcs_dir = config.output_dir.join("cbcs");
            tokio::fs::create_dir_all(&cenc_dir).await?;
            tokio::fs::create_dir_all(&cbcs_dir).await?;

            let (res_cenc, res_cbcs) = tokio::try_join!(
                execute_single_scheme(
                    config,
                    key_provider,
                    EncryptionScheme::Cenc,
                    &cenc_dir,
                    &control_dir
                ),
                execute_single_scheme(
                    config,
                    key_provider,
                    EncryptionScheme::Cbcs,
                    &cbcs_dir,
                    &control_dir
                )
            )?;

            Ok(merge_dual_results(config, res_cenc, res_cbcs))
        }
        scheme => {
            execute_single_scheme(
                config,
                key_provider,
                scheme,
                &config.output_dir,
                &control_dir,
            )
            .await
        }
    };

    if !config.preserve_output {
        let _ = tokio::fs::remove_dir_all(&control_dir).await;
    }
    execution_res
}

fn validate_config(config: &VodPackageConfig) -> Result<()> {
    match &config.input {
        VodInputSource::SingleFile(path) => {
            if !path.exists() {
                return Err(DrmpackError::InvalidConfig(format!(
                    "VOD input file does not exist: {}",
                    path.display()
                )));
            }
        }
        VodInputSource::TrackFiles(paths) => {
            if paths.is_empty() {
                return Err(DrmpackError::InvalidConfig(
                    "VOD TrackFiles input cannot be empty".into(),
                ));
            }
            for path in paths {
                if !path.exists() {
                    return Err(DrmpackError::InvalidConfig(format!(
                        "VOD input track file does not exist: {}",
                        path.display()
                    )));
                }
            }
        }
    }

    if config.renditions.is_empty() {
        return Err(DrmpackError::InvalidConfig(
            "VOD packaging requires at least one declared Rendition".into(),
        ));
    }

    Ok(())
}

async fn execute_single_scheme<P: KeyProvider>(
    config: &VodPackageConfig,
    key_provider: &P,
    scheme: EncryptionScheme,
    output_dir: &Path,
    control_dir: &Path,
) -> Result<VodPackageResult> {
    let plan = KeyPolicyEngine::plan(
        &config.content_id,
        &config.renditions,
        config.key_mapping_policy,
        scheme,
        &config.drm_systems,
    );

    let key_set = match plan.request {
        Some(ref request) => {
            info!(
                content_id = %config.content_id,
                scheme = %scheme,
                "Fetching keys for VOD packaging"
            );
            let fetched = key_provider.fetch_keys(request).await?;
            KeyPolicyEngine::resolve(&plan, &config.renditions, scheme, fetched)?
        }
        None => KeyPolicyEngine::resolve(&plan, &config.renditions, scheme, KeySet::new())?,
    };

    let mut drm_config = GpacDrmConfig::new(scheme);
    for (idx, rendition) in config.renditions.iter().enumerate() {
        if rendition.encrypted {
            let container_track_id = rendition.effective_container_track_id(idx);
            drm_config = drm_config.with_track(
                container_track_id,
                rendition.track_type,
                rendition.quality_tier.clone(),
            );
        }
    }

    let drm_xml = GpacDrmXmlGenerator::generate(&key_set, &drm_config)?;
    let drm_xml_path = control_dir.join(format!("drm_{}_{}.xml", scheme, Uuid::new_v4()));
    tokio::fs::write(&drm_xml_path, drm_xml).await?;

    let mut gpac_config =
        GpacVodProcessConfig::new(config.input.clone(), &drm_xml_path, output_dir)
            .with_vod_mode(config.vod_mode)
            .with_segment_duration(config.segment_duration)
            .with_manifest_name("vod");

    if let Some(ref bin) = config.gpac_bin {
        gpac_config = gpac_config.with_gpac_bin(bin);
    }

    let args = gpac_config.build_args();
    info!(scheme = %scheme, args = ?args, "Executing GPAC VOD batch packaging");

    let mut cmd = Command::new(&gpac_config.gpac_bin);
    cmd.args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let child = cmd.spawn().map_err(DrmpackError::Io)?;

    let output = match tokio::time::timeout(config.timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(DrmpackError::Io(e)),
        Err(_) => {
            return Err(DrmpackError::ProcessCrashed {
                exit_code: None,
                stderr: format!("GPAC VOD packaging timed out after {:?}", config.timeout),
            });
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        error!(status = ?output.status.code(), stderr = %stderr, "GPAC VOD packaging failed");
        return Err(DrmpackError::ProcessCrashed {
            exit_code: output.status.code(),
            stderr,
        });
    }

    inspect_and_validate_output(output_dir, config, scheme, &key_set).await
}

fn is_input_path(path: &Path, input: &VodInputSource) -> bool {
    let check_match = |input_path: &Path| -> bool {
        if path == input_path {
            return true;
        }
        if let (Ok(c1), Ok(c2)) = (path.canonicalize(), input_path.canonicalize()) {
            return c1 == c2;
        }
        false
    };

    match input {
        VodInputSource::SingleFile(p) => check_match(p),
        VodInputSource::TrackFiles(paths) => paths.iter().any(|p| check_match(p)),
    }
}

async fn inspect_and_validate_output(
    output_dir: &Path,
    config: &VodPackageConfig,
    scheme: EncryptionScheme,
    key_set: &KeySet,
) -> Result<VodPackageResult> {
    let mpd_manifest = output_dir.join("vod.mpd");
    if !mpd_manifest.exists() {
        return Err(DrmpackError::Gpac(format!(
            "GPAC did not produce expected DASH manifest at {}",
            mpd_manifest.display()
        )));
    }
    let mpd_content = tokio::fs::read_to_string(&mpd_manifest)
        .await
        .map_err(DrmpackError::Io)?;
    if !mpd_content.contains(r#"type="static""#) {
        return Err(DrmpackError::Gpac(format!(
            "MPD manifest at {} is not static VOD manifest (missing type=\"static\")",
            mpd_manifest.display()
        )));
    }

    let master_playlist = {
        let p = output_dir.join("vod.m3u8");
        if p.exists() {
            Some(p)
        } else {
            None
        }
    };

    let mut variant_playlists = Vec::new();
    let mut media_files = Vec::new();
    let mut init_segments = Vec::new();

    let mut entries = tokio::fs::read_dir(output_dir)
        .await
        .map_err(DrmpackError::Io)?;

    while let Some(entry) = entries.next_entry().await.map_err(DrmpackError::Io)? {
        let path = entry.path();
        if path.is_file() {
            if is_input_path(&path, &config.input) {
                continue;
            }
            let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if fname.ends_with(".m3u8") && fname != "vod.m3u8" {
                let content = tokio::fs::read_to_string(&path)
                    .await
                    .map_err(DrmpackError::Io)?;
                if !content.contains("#EXT-X-ENDLIST") {
                    return Err(DrmpackError::Gpac(format!(
                        "Variant playlist {} missing #EXT-X-ENDLIST",
                        path.display()
                    )));
                }
                variant_playlists.push(path);
            } else if fname.ends_with("_init.mp4") {
                init_segments.push(path);
            } else if fname.ends_with(".mp4") || fname.ends_with(".m4s") {
                media_files.push(path);
            }
        }
    }

    variant_playlists.sort();
    media_files.sort();
    init_segments.sort();

    let metadata = DrmStreamMetadata::from_session(&config.content_id, scheme, key_set);

    Ok(VodPackageResult {
        content_id: config.content_id.clone(),
        output_dir: output_dir.to_path_buf(),
        master_playlist,
        mpd_manifest,
        variant_playlists,
        media_files,
        init_segments,
        metadata,
    })
}

fn merge_dual_results(
    config: &VodPackageConfig,
    cenc: VodPackageResult,
    cbcs: VodPackageResult,
) -> VodPackageResult {
    let mut media_files = cenc.media_files;
    media_files.extend(cbcs.media_files);
    let mut init_segments = cenc.init_segments;
    init_segments.extend(cbcs.init_segments);
    let mut variant_playlists = cenc.variant_playlists;
    variant_playlists.extend(cbcs.variant_playlists);

    let mut keys = cenc.metadata.keys;
    keys.extend(cbcs.metadata.keys);

    let metadata = DrmStreamMetadata {
        content_id: config.content_id.clone(),
        scheme: EncryptionScheme::Dual,
        keys,
    };

    VodPackageResult {
        content_id: config.content_id.clone(),
        output_dir: config.output_dir.clone(),
        master_playlist: cbcs.master_playlist,
        mpd_manifest: cenc.mpd_manifest,
        variant_playlists,
        media_files,
        init_segments,
        metadata,
    }
}
