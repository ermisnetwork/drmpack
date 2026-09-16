//! GPAC subprocess configuration and command-line synthesis for whole-file VOD batch packaging.

use crate::vod::{VodInputSource, VodMode};
use std::path::PathBuf;

/// GPAC subprocess configuration for whole-file VOD batch packaging.
#[derive(Debug, Clone, PartialEq)]
pub struct GpacVodProcessConfig {
    /// Input media source.
    pub input_source: VodInputSource,
    /// Path to GPAC cecrypt Common Encryption XML.
    pub drm_xml_path: PathBuf,
    /// Target directory for generated manifests and media files.
    pub output_dir: PathBuf,
    /// VOD packaging mode (SingleFile vs Segmented).
    pub vod_mode: VodMode,
    /// Target segment or subsegment duration in seconds.
    pub segment_duration: f64,
    /// Name of root manifest without extension (e.g. `"vod"` -> `vod.mpd` & `vod.m3u8`).
    pub manifest_name: String,
    /// Binary executable name or path for GPAC (defaults to `"gpac"`).
    pub gpac_bin: String,
}

const VOD_REPRESENTATION_MAPPING: &str = concat!(
    ":#Representation=",
    "(video)video_$Height$p,",
    "(video)video,",
    "(audio)(Language=!und)audio_$Language$,",
    "(audio)audio,",
    "(text)(Language=!und)sub_$Language$,",
    "(text)sub",
    ":#HLSPL=",
    "(video)video_$Height$p.m3u8,",
    "(video)video.m3u8,",
    "(audio)(Language=!und)audio_$Language$.m3u8,",
    "(audio)audio.m3u8,",
    "(text)(Language=!und)sub_$Language$.m3u8,",
    "(text)sub.m3u8",
);

impl GpacVodProcessConfig {
    /// Construct a new VOD process configuration with mandatory input, DRM XML, and output directory.
    pub fn new(
        input_source: VodInputSource,
        drm_xml_path: impl Into<PathBuf>,
        output_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            input_source,
            drm_xml_path: drm_xml_path.into(),
            output_dir: output_dir.into(),
            vod_mode: VodMode::SingleFile,
            segment_duration: 2.0,
            manifest_name: "vod".into(),
            gpac_bin: "gpac".into(),
        }
    }

    /// Set VOD mode.
    pub fn with_vod_mode(mut self, mode: VodMode) -> Self {
        self.vod_mode = mode;
        self
    }

    /// Set segment duration in seconds.
    pub fn with_segment_duration(mut self, duration: f64) -> Self {
        self.segment_duration = duration;
        self
    }

    /// Set manifest base name.
    pub fn with_manifest_name(mut self, name: impl Into<String>) -> Self {
        self.manifest_name = name.into();
        self
    }

    /// Override GPAC binary path.
    pub fn with_gpac_bin(mut self, bin: impl Into<String>) -> Self {
        self.gpac_bin = bin.into();
        self
    }

    /// Build the command-line argument vector for GPAC execution.
    pub fn build_args(&self) -> Vec<String> {
        let mut args = Vec::new();

        // 0. Disable ANSI color codes
        args.push("-logs=ncl".into());

        // 1. Enable multi-threaded filter execution
        args.push("-threads=-1".into());

        // 2. Add input files with representation mapping
        for path in self.input_source.paths() {
            args.push("-i".into());
            args.push(format!("{}{}", path.display(), VOD_REPRESENTATION_MAPPING));
        }

        // 3. Cecrypt filter
        args.push(format!("cecrypt:cfile={}", self.drm_xml_path.display()));

        // 4. Dasher filter destination and options
        let manifest_file = format!("{}.mpd", self.manifest_name);
        let destination = self.output_dir.join(manifest_file).display().to_string();

        let mut dasher_opts = vec![
            destination,
            "dual".into(),
            format!("segdur={}", self.segment_duration),
            "pssh=mv".into(),
        ];

        match self.vod_mode {
            VodMode::SingleFile => {
                dasher_opts.push("profile=onDemand".into());
                dasher_opts.push("template=$RepresentationID$".into());
            }
            VodMode::Segmented => {
                dasher_opts.push("template=$RepresentationID$_$Init=init$$Number$".into());
            }
        }

        let dasher_arg = dasher_opts.join(":");
        args.push("-o".into());
        args.push(dasher_arg);

        args
    }
}
