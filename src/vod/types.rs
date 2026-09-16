//! Core domain types and configuration builders for VOD batch packaging.

use crate::session::DrmStreamMetadata;
use crate::types::{DrmSystem, EncryptionScheme, KeyMappingPolicy, Rendition};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Packaging delivery mode for VOD whole-file batch jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum VodMode {
    /// Byte-Range Single-File mode (`profile=onDemand`). Generates one self-initializing `.mp4`
    /// per rendition containing an `sidx` box, `#EXT-X-BYTERANGE` in HLS, and `<SegmentBase>` in DASH.
    /// Reduces storage file count by 99%.
    #[default]
    SingleFile,
    /// Discrete Multi-Segment mode. Emits independent `.m4s` segments and an `init.mp4` per rendition.
    Segmented,
}

/// Input media source for VOD batch packaging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VodInputSource {
    /// A single multiplexed container containing video, audio, and/or subtitle tracks.
    SingleFile(PathBuf),
    /// Multiple discrete rendition track files (e.g. video and audio files).
    TrackFiles(Vec<PathBuf>),
}

impl VodInputSource {
    /// Return the list of input media file paths.
    pub fn paths(&self) -> &[PathBuf] {
        match self {
            Self::SingleFile(path) => std::slice::from_ref(path),
            Self::TrackFiles(paths) => paths.as_slice(),
        }
    }
}

/// Configuration for a whole-file VOD batch packaging job.
#[derive(Debug, Clone, PartialEq)]
pub struct VodPackageConfig {
    /// Content or asset identifier.
    pub content_id: String,
    /// Input media source.
    pub input: VodInputSource,
    /// Directory where packaged manifests and media segments will be written.
    pub output_dir: PathBuf,
    /// VOD packaging mode (SingleFile onDemand vs Discrete Multi-Segment).
    pub vod_mode: VodMode,
    /// Encryption scheme (CBCS, CENC, or Dual).
    pub encryption_scheme: EncryptionScheme,
    /// Target DRM systems to generate signaling and PSSH boxes for.
    pub drm_systems: Vec<DrmSystem>,
    /// Declared track renditions.
    pub renditions: Vec<Rendition>,
    /// Subsegment or segment duration in seconds (default 2.0s).
    pub segment_duration: f64,
    /// ContentKey mapping policy across declared renditions.
    pub key_mapping_policy: KeyMappingPolicy,
    /// Optional custom path or binary name for GPAC.
    pub gpac_bin: Option<String>,
    /// Subprocess execution timeout.
    pub timeout: Duration,
    /// Whether to preserve staging files on error.
    pub preserve_output: bool,
}

impl VodPackageConfig {
    /// Create a new VOD packaging configuration with standard defaults.
    pub fn new(
        content_id: impl Into<String>,
        input: VodInputSource,
        output_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            content_id: content_id.into(),
            input,
            output_dir: output_dir.into(),
            vod_mode: VodMode::SingleFile,
            encryption_scheme: EncryptionScheme::Cbcs,
            drm_systems: Vec::new(),
            renditions: Vec::new(),
            segment_duration: 2.0,
            key_mapping_policy: KeyMappingPolicy::SharedAll,
            gpac_bin: None,
            timeout: Duration::from_secs(120),
            preserve_output: false,
        }
    }

    /// Set VOD packaging mode.
    pub fn with_vod_mode(mut self, mode: VodMode) -> Self {
        self.vod_mode = mode;
        self
    }

    /// Set encryption scheme.
    pub fn with_encryption_scheme(mut self, scheme: EncryptionScheme) -> Self {
        self.encryption_scheme = scheme;
        self
    }

    /// Add a target DRM system without duplicating existing entries.
    pub fn with_drm_system(mut self, drm: DrmSystem) -> Self {
        if !self.drm_systems.contains(&drm) {
            self.drm_systems.push(drm);
        }
        self
    }

    /// Add a rendition.
    pub fn with_rendition(mut self, rendition: Rendition) -> Self {
        self.renditions.push(rendition);
        self
    }

    /// Set segment duration in seconds.
    pub fn with_segment_duration(mut self, duration: f64) -> Self {
        self.segment_duration = duration;
        self
    }

    /// Set key mapping policy.
    pub fn with_key_mapping_policy(mut self, policy: KeyMappingPolicy) -> Self {
        self.key_mapping_policy = policy;
        self
    }

    /// Override GPAC executable name or path.
    pub fn with_gpac_bin(mut self, bin: impl Into<String>) -> Self {
        self.gpac_bin = Some(bin.into());
        self
    }

    /// Set execution timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set preserve output on failure flag.
    pub fn with_preserve_output(mut self, preserve: bool) -> Self {
        self.preserve_output = preserve;
        self
    }
}

/// Result of a completed VOD batch packaging job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VodPackageResult {
    /// Content identifier.
    pub content_id: String,
    /// Root output directory containing all artifacts.
    pub output_dir: PathBuf,
    /// Master HLS playlist (`vod.m3u8`), if generated.
    pub master_playlist: Option<PathBuf>,
    /// Static DASH MPD manifest (`vod.mpd`).
    pub mpd_manifest: PathBuf,
    /// Emitted variant HLS playlists (e.g. `video_720p.m3u8`, `audio.m3u8`).
    pub variant_playlists: Vec<PathBuf>,
    /// Generated media files (Single-File `.mp4` files or discrete `.m4s` segments).
    pub media_files: Vec<PathBuf>,
    /// Emitted initialization segments (e.g. `video_720p_init.mp4`), present in Segmented mode.
    pub init_segments: Vec<PathBuf>,
    /// DRM playback metadata extracted during key acquisition and packaging.
    pub metadata: DrmStreamMetadata,
}
