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

struct ControlDirGuard {
    path: std::path::PathBuf,
    preserve: bool,
}

impl Drop for ControlDirGuard {
    fn drop(&mut self) {
        if !self.preserve && self.path.exists() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

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
    let mut control_guard = ControlDirGuard {
        path: control_dir.clone(),
        preserve: false,
    };

    // 3. Acquire encryption keys once upfront (single network request for both single and dual schemes)
    let key_set = fetch_vod_key_set(config, key_provider).await?;

    let execution_res = match config.encryption_scheme {
        EncryptionScheme::Dual => {
            let cenc_dir = config.output_dir.join("cenc");
            let cbcs_dir = config.output_dir.join("cbcs");
            tokio::fs::create_dir_all(&cenc_dir).await?;
            tokio::fs::create_dir_all(&cbcs_dir).await?;

            // Execute CENC followed by CBCS sequentially.
            // On resource-constrained environments (e.g. 2-vCPU CI runners),
            // running two concurrent GPAC muxers causes CPU starvation and filter graph race conditions.
            let res_cenc = execute_single_scheme(
                config,
                &key_set,
                EncryptionScheme::Cenc,
                &cenc_dir,
                &control_dir,
            )
            .await;

            match res_cenc {
                Ok(cenc) => {
                    let res_cbcs = execute_single_scheme(
                        config,
                        &key_set,
                        EncryptionScheme::Cbcs,
                        &cbcs_dir,
                        &control_dir,
                    )
                    .await;
                    res_cbcs.map(|cbcs| merge_dual_results(config, cenc, cbcs))
                }
                Err(e) => Err(e),
            }
        }
        scheme => {
            execute_single_scheme(config, &key_set, scheme, &config.output_dir, &control_dir).await
        }
    };

    if execution_res.is_err() && config.preserve_output {
        control_guard.preserve = true;
    }
    execution_res
}

async fn fetch_vod_key_set<P: KeyProvider>(
    config: &VodPackageConfig,
    provider: &P,
) -> Result<KeySet> {
    let plan = KeyPolicyEngine::plan(
        &config.content_id,
        &config.renditions,
        config.key_mapping_policy,
        config.encryption_scheme,
        &config.drm_systems,
    );

    let key_set = match plan.request {
        Some(ref request) => {
            info!(
                content_id = %config.content_id,
                scheme = %config.encryption_scheme,
                "Fetching keys for VOD packaging"
            );
            let fetched = provider.fetch_keys(request).await?;
            KeyPolicyEngine::resolve(&plan, &config.renditions, config.encryption_scheme, fetched)?
        }
        None => KeyPolicyEngine::resolve(
            &plan,
            &config.renditions,
            config.encryption_scheme,
            KeySet::new(),
        )?,
    };
    Ok(key_set)
}

fn validate_config(config: &VodPackageConfig) -> Result<()> {
    if config.content_id.trim().is_empty() {
        return Err(DrmpackError::InvalidConfig(
            "VOD packaging content_id cannot be empty".into(),
        ));
    }

    if config.segment_duration <= 0.0 || config.segment_duration.is_nan() {
        return Err(DrmpackError::InvalidConfig(format!(
            "VOD packaging segment_duration must be positive: {}",
            config.segment_duration
        )));
    }

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

async fn execute_single_scheme(
    config: &VodPackageConfig,
    key_set: &KeySet,
    scheme: EncryptionScheme,
    output_dir: &Path,
    control_dir: &Path,
) -> Result<VodPackageResult> {
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
        // Note: Unencrypted tracks are intentionally omitted from GpacDrmConfig.
        // In GPAC cecrypt, unencrypted PIDs without a CrypTrack pass through in the clear.
        // Emitting <CrypTrack IsEncrypted="0"/> causes MP4Mux to abort with
        // "Invalid CENC key info / Missing CENC Key config".
    }

    let drm_xml = GpacDrmXmlGenerator::generate(key_set, &drm_config)?;
    let drm_xml_path = control_dir.join(format!("drm_{}_{}.xml", scheme, Uuid::new_v4()));
    tokio::fs::write(&drm_xml_path, drm_xml).await?;

    let track_ids: Vec<u32> = config
        .renditions
        .iter()
        .enumerate()
        .map(|(idx, r)| r.effective_container_track_id(idx))
        .collect();

    let mut gpac_config =
        GpacVodProcessConfig::new(config.input.clone(), &drm_xml_path, output_dir)
            .with_vod_mode(config.vod_mode)
            .with_segment_duration(config.segment_duration)
            .with_manifest_name("vod")
            .with_temp_dir(control_dir)
            .with_track_ids(track_ids);

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

    let (code, success) = extract_exit_status(&output.status);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !success {
        error!(status = ?code, stderr = %stderr, "GPAC VOD packaging failed");
        return Err(DrmpackError::ProcessCrashed {
            exit_code: code,
            stderr,
        });
    }
    if !stderr.is_empty() {
        eprintln!("GPAC VOD STDERR:\n{}", stderr);
    }

    inspect_and_validate_output(output_dir, config, scheme, key_set).await
}

pub(crate) fn extract_exit_status(status: &std::process::ExitStatus) -> (Option<i32>, bool) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        let code = status
            .code()
            .or_else(|| status.signal().map(|sig| 128 + sig));
        (code, status.success())
    }
    #[cfg(not(unix))]
    {
        (status.code(), status.success())
    }
}

fn is_input_path(path: &Path, input: &VodInputSource) -> bool {
    input.paths().iter().any(|in_p| {
        path == in_p
            || (path.canonicalize().is_ok() && path.canonicalize().ok() == in_p.canonicalize().ok())
    })
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

    let master_playlist = Some(output_dir.join("vod.m3u8")).filter(|p| p.exists());
    if let Some(ref master) = master_playlist {
        let content = tokio::fs::read_to_string(master)
            .await
            .map_err(DrmpackError::Io)?;
        if !content.starts_with("#EXTM3U") {
            return Err(DrmpackError::Gpac(format!(
                "Master playlist {} is missing #EXTM3U header",
                master.display()
            )));
        }
    }

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
            } else if fname.ends_with("_init.mp4") || fname.ends_with("_init.m4a") {
                init_segments.push(path);
            } else if fname.ends_with(".mp4")
                || fname.ends_with(".m4s")
                || fname.ends_with(".m4a")
                || fname.ends_with(".m4v")
            {
                media_files.push(path);
            }
        }
    }

    if media_files.is_empty() {
        return Err(DrmpackError::Gpac(format!(
            "GPAC finished but produced no media files in {}",
            output_dir.display()
        )));
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
    mut cenc: VodPackageResult,
    cbcs: VodPackageResult,
) -> VodPackageResult {
    cenc.output_dir = config.output_dir.clone();
    cenc.master_playlist = cbcs.master_playlist;
    cenc.variant_playlists.extend(cbcs.variant_playlists);
    cenc.media_files.extend(cbcs.media_files);
    cenc.init_segments.extend(cbcs.init_segments);
    cenc.metadata.scheme = EncryptionScheme::Dual;
    cenc.metadata.keys.extend(cbcs.metadata.keys);
    cenc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn test_extract_exit_code_from_signal() {
        use std::os::unix::process::ExitStatusExt;
        // Signal 11 (SIGSEGV)
        let status = std::process::ExitStatus::from_raw(11);
        let (code, success) = extract_exit_status(&status);
        assert!(!success);
        assert_eq!(code, Some(139)); // 128 + 11

        // Signal 9 (SIGKILL)
        let status_kill = std::process::ExitStatus::from_raw(9);
        let (code_kill, success_kill) = extract_exit_status(&status_kill);
        assert!(!success_kill);
        assert_eq!(code_kill, Some(137)); // 128 + 9

        // Normal success exit code 0
        let status_ok = std::process::ExitStatus::from_raw(0);
        let (code_ok, success_ok) = extract_exit_status(&status_ok);
        assert!(success_ok);
        assert_eq!(code_ok, Some(0));
    }
}
