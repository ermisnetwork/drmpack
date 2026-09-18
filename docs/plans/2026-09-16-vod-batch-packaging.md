# VOD Batch Packaging Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement standalone whole-file VOD batch packaging (`drmpack::vod`) supporting Single-File Byte-Range (`profile=onDemand`, `sidx`, `#EXT-X-BYTERANGE`) and Discrete Multi-Segment modes with static manifest generation and DRM encryption.

**Architecture:** Decoupled batch runner executing GPAC subprocess with file input(s) and static dasher configuration (`onDemand` or `segdur`), reusing `KeyPolicyEngine` and `GpacDrmXmlGenerator` for CENC/CBCS/Dual encryption, producing static manifests (`vod.m3u8` with `#EXT-X-ENDLIST`, `vod.mpd` with `type="static"`) and returning validated `VodPackageResult` with `DrmStreamMetadata`.

**Tech Stack:** Rust (Tokio, std::process, quick-xml, memchr), GPAC (`gpac`, `cecrypt`, `dasher`), DASH-IF CPIX / AWS SPEKE v2.

---

### Task 1: Domain Types for VOD Packaging (`src/vod/types.rs` & `src/vod/mod.rs`)

**Files:**
- Create: `src/vod/types.rs`
- Create: `src/vod/mod.rs`
- Modify: `src/lib.rs`
- Test: `tests/vod_types_test.rs`

**Step 1: Write the failing test**

In `tests/vod_types_test.rs`:
```rust
use drmpack::types::{DrmSystem, EncryptionScheme, KeyMappingPolicy, Rendition};
use drmpack::vod::{VodInputSource, VodMode, VodPackageConfig, VodPackageResult};
use std::path::PathBuf;
use std::time::Duration;

#[test]
fn test_vod_package_config_builder_defaults() {
    let input = VodInputSource::SingleFile(PathBuf::from("video.mp4"));
    let output_dir = PathBuf::from("output/vod");
    let config = VodPackageConfig::new("movie_123", input.clone(), output_dir.clone())
        .with_rendition(Rendition::video_hd())
        .with_rendition(Rendition::audio());

    assert_eq!(config.content_id, "movie_123");
    assert_eq!(config.input, input);
    assert_eq!(config.output_dir, output_dir);
    assert_eq!(config.vod_mode, VodMode::SingleFile);
    assert_eq!(config.encryption_scheme, EncryptionScheme::Cbcs);
    assert_eq!(config.segment_duration, 2.0);
    assert_eq!(config.renditions.len(), 2);
    assert_eq!(config.key_mapping_policy, KeyMappingPolicy::SharedAll);
    assert_eq!(config.timeout, Duration::from_secs(120));
}

#[test]
fn test_vod_package_config_builder_customizations() {
    let input = VodInputSource::TrackFiles(vec![
        PathBuf::from("video_1080p.mp4"),
        PathBuf::from("audio_en.mp4"),
    ]);
    let output_dir = PathBuf::from("output/segmented_vod");
    let config = VodPackageConfig::new("movie_456", input, output_dir)
        .with_vod_mode(VodMode::Segmented)
        .with_encryption_scheme(EncryptionScheme::Dual)
        .with_drm_system(DrmSystem::Widevine)
        .with_drm_system(DrmSystem::FairPlay)
        .with_segment_duration(6.0)
        .with_timeout(Duration::from_secs(300))
        .with_key_mapping_policy(KeyMappingPolicy::PerTierAndTrack);

    assert_eq!(config.vod_mode, VodMode::Segmented);
    assert_eq!(config.encryption_scheme, EncryptionScheme::Dual);
    assert_eq!(config.drm_systems.len(), 2);
    assert_eq!(config.segment_duration, 6.0);
    assert_eq!(config.timeout, Duration::from_secs(300));
    assert_eq!(config.key_mapping_policy, KeyMappingPolicy::PerTierAndTrack);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test vod_types_test`
Expected: FAIL with "unresolved import `drmpack::vod`"

**Step 3: Write minimal implementation**

In `src/vod/types.rs`:
```rust
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VodInputSource {
    /// A single multiplexed container containing video, audio, and/or subtitle tracks.
    SingleFile(PathBuf),
    /// Multiple discrete rendition track files (e.g. video and audio files).
    TrackFiles(Vec<PathBuf>),
}

/// Configuration for a whole-file VOD batch packaging job.
#[derive(Debug, Clone)]
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

    /// Add a target DRM system.
    pub fn with_drm_system(mut self, drm: DrmSystem) -> Self {
        self.drm_systems.push(drm);
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
#[derive(Debug, Clone)]
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
```

In `src/vod/mod.rs`:
```rust
//! Standalone whole-file VOD batch packaging module.
//!
//! Provides [`package_vod_file`], [`VodPackageConfig`], [`VodMode`], [`VodInputSource`],
//! and [`VodPackageResult`].

pub mod types;
pub use types::{VodInputSource, VodMode, VodPackageConfig, VodPackageResult};
```

In `src/lib.rs`:
```rust
/// Standalone whole-file VOD batch packaging engine.
pub mod vod;
```

**Step 4: Run test to verify it passes**

Run: `cargo test --test vod_types_test`
Expected: PASS

**Step 5: Commit**

```bash
git add src/vod/ src/lib.rs tests/vod_types_test.rs
git commit -m "feat(vod): introduce core VOD packaging types and config builder"
```

---

### Task 2: ISOBMFF Box Verification for VOD Single-File (`sidx`) and Manifest Integrity (`src/session/isobmff.rs`)

**Files:**
- Modify: `src/session/isobmff.rs`
- Test: `src/session/isobmff.rs` (unit tests)

**Step 1: Write the failing test**

Add to `src/session/isobmff.rs`:
```rust
#[test]
fn test_isobmff_single_file_media_verification() {
    let ftyp = make_box(b"ftyp", b"isom");
    let moov = make_box(b"moov", b"moov_payload");
    let sidx = make_box(b"sidx", b"sidx_payload");
    let moof = make_box(b"moof", b"moof_payload");
    let mdat = make_box(b"mdat", b"mdat_payload");

    let mut valid_single_file = Vec::new();
    valid_single_file.extend_from_slice(&ftyp);
    valid_single_file.extend_from_slice(&moov);
    valid_single_file.extend_from_slice(&sidx);
    valid_single_file.extend_from_slice(&moof);
    valid_single_file.extend_from_slice(&mdat);

    assert!(is_complete_isobmff_single_file(&valid_single_file));

    // Missing sidx should fail
    let mut missing_sidx = Vec::new();
    missing_sidx.extend_from_slice(&ftyp);
    missing_sidx.extend_from_slice(&moov);
    missing_sidx.extend_from_slice(&moof);
    missing_sidx.extend_from_slice(&mdat);
    assert!(!is_complete_isobmff_single_file(&missing_sidx));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib session::isobmff::tests::test_isobmff_single_file_media_verification`
Expected: FAIL with "cannot find function `is_complete_isobmff_single_file` in this scope"

**Step 3: Write minimal implementation**

In `src/session/isobmff.rs`:
```rust
/// Verify if `data` contains an ISOBMFF box with matching 4-byte ASCII box identifier.
pub(crate) fn has_isobmff_box(data: &[u8], target_box: &[u8; 4]) -> bool {
    let mut offset = 0;
    while let Some((box_type, box_size)) = next_isobmff_box(data, offset) {
        if box_type == target_box {
            return true;
        }
        offset += box_size;
    }
    false
}

/// Verify if `data` is a valid self-initializing Single-File media container (e.g. DASH onDemand / HLS byte-range)
/// containing `ftyp`, `moov`, and `sidx` (Segment Index) boxes.
pub(crate) fn is_complete_isobmff_single_file(data: &[u8]) -> bool {
    let mut offset = 0;
    let (mut has_ftyp, mut has_moov, mut has_sidx) = (false, false, false);

    while let Some((box_type, box_size)) = next_isobmff_box(data, offset) {
        if box_type == b"ftyp" {
            has_ftyp = true;
        } else if box_type == b"moov" {
            has_moov = true;
        } else if box_type == b"sidx" {
            has_sidx = true;
        }
        offset += box_size;
    }

    has_ftyp && has_moov && has_sidx && !data.is_empty() && offset == data.len()
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test --lib session::isobmff::tests`
Expected: PASS

**Step 5: Commit**

```bash
git add src/session/isobmff.rs
git commit -m "feat(isobmff): add verification for self-initializing single-file containers with sidx"
```

---

### Task 3: GPAC VOD Process Configuration and Command Builder (`src/gpac/vod.rs`)

**Files:**
- Create: `src/gpac/vod.rs`
- Modify: `src/gpac/mod.rs`
- Test: `tests/gpac_vod_args_test.rs`

**Step 1: Write the failing test**

In `tests/gpac_vod_args_test.rs`:
```rust
use drmpack::gpac::vod::GpacVodProcessConfig;
use drmpack::vod::{VodInputSource, VodMode};
use std::path::PathBuf;

#[test]
fn test_gpac_vod_single_file_args() {
    let input = VodInputSource::SingleFile(PathBuf::from("/inputs/movie.mp4"));
    let config = GpacVodProcessConfig::new(
        input,
        PathBuf::from("/keys/drm.xml"),
        PathBuf::from("/out/vod"),
    )
    .with_vod_mode(VodMode::SingleFile)
    .with_manifest_name("vod");

    let args = config.build_args();

    assert!(args.contains(&"-logs=ncl".to_string()));
    assert!(args.contains(&"-threads=-1".to_string()));
    assert!(args.contains(&"-i".to_string()));
    // Input must include representation mapping
    let input_arg = args.iter().find(|a| a.starts_with("/inputs/movie.mp4")).unwrap();
    assert!(input_arg.contains("#Representation="));
    assert!(input_arg.contains("#HLSPL="));

    // Cecrypt filter
    assert!(args.contains(&"cecrypt:cfile=/keys/drm.xml".to_string()));

    // Output dasher options
    assert!(args.contains(&"-o".to_string()));
    let dasher_arg = args.iter().find(|a| a.starts_with("/out/vod/vod.mpd:")).unwrap();
    assert!(dasher_arg.contains(":dual:"));
    assert!(dasher_arg.contains(":profile=onDemand:"));
    assert!(dasher_arg.contains(":pssh=mv:"));
    assert!(dasher_arg.contains(":template=$RepresentationID$"));
}

#[test]
fn test_gpac_vod_segmented_args() {
    let input = VodInputSource::SingleFile(PathBuf::from("/inputs/movie.mp4"));
    let config = GpacVodProcessConfig::new(
        input,
        PathBuf::from("/keys/drm.xml"),
        PathBuf::from("/out/segmented"),
    )
    .with_vod_mode(VodMode::Segmented)
    .with_segment_duration(4.0)
    .with_manifest_name("index");

    let args = config.build_args();
    let dasher_arg = args.iter().find(|a| a.starts_with("/out/segmented/index.mpd:")).unwrap();
    assert!(dasher_arg.contains(":dual:"));
    assert!(dasher_arg.contains(":segdur=4:"));
    assert!(dasher_arg.contains(":template=$RepresentationID$_$Init=init$$Number$"));
    assert!(!dasher_arg.contains(":profile=onDemand:"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test gpac_vod_args_test`
Expected: FAIL with "unresolved import `drmpack::gpac::vod`"

**Step 3: Write minimal implementation**

In `src/gpac/vod.rs`:
```rust
use crate::vod::{VodInputSource, VodMode};
use std::path::PathBuf;

/// GPAC subprocess configuration for whole-file VOD batch packaging.
#[derive(Debug, Clone)]
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
        match &self.input_source {
            VodInputSource::SingleFile(path) => {
                args.push("-i".into());
                args.push(format!("{}{}", path.display(), VOD_REPRESENTATION_MAPPING));
            }
            VodInputSource::TrackFiles(paths) => {
                for path in paths {
                    args.push("-i".into());
                    args.push(format!("{}{}", path.display(), VOD_REPRESENTATION_MAPPING));
                }
            }
        }

        // 3. Cecrypt filter
        args.push(format!("cecrypt:cfile={}", self.drm_xml_path.display()));

        // 4. Dasher filter destination and options
        let manifest_file = format!("{}.mpd", self.manifest_name);
        let destination = self.output_dir.join(manifest_file).display().to_string();

        let mut dasher_opts = vec![
            destination,
            "dual".into(),
            "pssh=mv".into(),
        ];

        match self.vod_mode {
            VodMode::SingleFile => {
                dasher_opts.push("profile=onDemand".into());
                dasher_opts.push("template=$RepresentationID$".into());
            }
            VodMode::Segmented => {
                dasher_opts.push(format!("segdur={}", self.segment_duration));
                dasher_opts.push("template=$RepresentationID$_$Init=init$$Number$".into());
            }
        }

        let dasher_arg = dasher_opts.join(":");
        args.push("-o".into());
        args.push(dasher_arg);

        args
    }
}
```

In `src/gpac/mod.rs`:
```rust
/// Whole-file VOD GPAC subprocess configuration.
pub mod vod;
```

**Step 4: Run test to verify it passes**

Run: `cargo test --test gpac_vod_args_test`
Expected: PASS

**Step 5: Commit**

```bash
git add src/gpac/vod.rs src/gpac/mod.rs tests/gpac_vod_args_test.rs
git commit -m "feat(gpac): add GpacVodProcessConfig command generator for SingleFile and Segmented VOD"
```

---

### Task 4: Core VOD Batch Packaging Execution Engine (`src/vod/engine.rs`)

**Files:**
- Create: `src/vod/engine.rs`
- Modify: `src/vod/mod.rs`
- Test: `tests/vod_batch_e2e.rs`

**Step 1: Write the failing test**

In `tests/vod_batch_e2e.rs`:
```rust
use drmpack::key::{ContentKey, KeySet, StaticKeySource};
use drmpack::types::{DrmSystem, EncryptionScheme, QualityTier, Rendition, TrackType};
use drmpack::vod::{package_vod_file, VodInputSource, VodMode, VodPackageConfig};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use uuid::Uuid;

fn generate_synthetic_mp4(path: &PathBuf) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f", "lavfi", "-i", "testsrc=duration=4:size=640x360:rate=30",
            "-f", "lavfi", "-i", "sine=frequency=1000:duration=4:sample_rate=48000",
            "-c:v", "libx264", "-g", "60", "-keyint_min", "60", "-sc_threshold", "0",
            "-c:a", "aac", "-b:a", "128k",
            "-f", "mp4", path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run ffmpeg");
    assert!(status.status.success(), "ffmpeg synthetic generation failed");
}

#[tokio::test]
async fn test_package_vod_single_file_cbcs() {
    let test_dir = std::env::temp_dir().join(format!("drmpack_vod_test_{}", Uuid::new_v4()));
    let input_file = test_dir.join("input.mp4");
    generate_synthetic_mp4(&input_file);

    let output_dir = test_dir.join("out_vod");
    let key_id = Uuid::new_v4();
    let content_key = ContentKey::new(key_id, [0x11; 16], QualityTier::hd(), TrackType::Video);
    let key_set = KeySet::new(vec![content_key]);
    let key_source = Arc::new(StaticKeySource::new(key_set));

    let config = VodPackageConfig::new("test_content", VodInputSource::SingleFile(input_file), &output_dir)
        .with_vod_mode(VodMode::SingleFile)
        .with_encryption_scheme(EncryptionScheme::Cbcs)
        .with_drm_system(DrmSystem::FairPlay)
        .with_rendition(Rendition::video_hd().with_container_track_id(1))
        .with_rendition(Rendition::audio().with_container_track_id(2).clear());

    let result = package_vod_file(&config, &(key_source as Arc<dyn drmpack::key::KeyProvider>))
        .await
        .expect("package_vod_file failed");

    assert!(result.mpd_manifest.exists());
    assert!(result.master_playlist.as_ref().unwrap().exists());
    assert!(!result.media_files.is_empty());

    // Verify HLS master and variant playlist
    let master_content = std::fs::read_to_string(result.master_playlist.as_ref().unwrap()).unwrap();
    assert!(master_content.contains("#EXTM3U"));

    // Verify variant playlist contains #EXT-X-ENDLIST and #EXT-X-BYTERANGE
    let variant = &result.variant_playlists[0];
    let variant_content = std::fs::read_to_string(variant).unwrap();
    assert!(variant_content.contains("#EXT-X-ENDLIST"));
    assert!(variant_content.contains("#EXT-X-BYTERANGE:"));

    // Verify MPD manifest contains type="static"
    let mpd_content = std::fs::read_to_string(&result.mpd_manifest).unwrap();
    assert!(mpd_content.contains(r#"type="static""#));
    assert!(mpd_content.contains("<SegmentBase"));

    let _ = std::fs::remove_dir_all(&test_dir);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test vod_batch_e2e`
Expected: FAIL with "cannot find function `package_vod_file` in module `drmpack::vod`"

**Step 3: Write minimal implementation**

In `src/vod/engine.rs`:
```rust
use crate::error::{DrmpackError, PackagingOperation, Result};
use crate::gpac::vod::GpacVodProcessConfig;
use crate::gpac::xml::{GpacDrmConfig, GpacDrmXmlGenerator};
use crate::key::{KeyPolicyEngine, KeyProvider};
use crate::session::DrmStreamMetadata;
use crate::types::{EncryptionScheme, LatencyMode};
use crate::vod::{VodInputSource, VodPackageConfig, VodPackageResult};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::process::Command;
use tracing::{error, info, instrument};
use uuid::Uuid;

/// Package a static media file or set of track files into encrypted VOD DASH and HLS assets.
#[instrument(skip(key_provider), fields(content_id = %config.content_id))]
pub async fn package_vod_file(
    config: &VodPackageConfig,
    key_provider: &Arc<dyn KeyProvider>,
) -> Result<VodPackageResult> {
    // 1. Validate inputs
    validate_config(config)?;

    // 2. Prepare output and control directories
    tokio::fs::create_dir_all(&config.output_dir)
        .await
        .map_err(|e| DrmpackError::Io(PackagingOperation::ProcessSpawn, e))?;

    let control_dir = std::env::temp_dir().join(format!("drmpack_vod_ctrl_{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&control_dir)
        .await
        .map_err(|e| DrmpackError::Io(PackagingOperation::ProcessSpawn, e))?;

    let execution_res = match config.encryption_scheme {
        EncryptionScheme::Dual => {
            let cenc_dir = config.output_dir.join("cenc");
            let cbcs_dir = config.output_dir.join("cbcs");
            tokio::fs::create_dir_all(&cenc_dir)
                .await
                .map_err(|e| DrmpackError::Io(PackagingOperation::ProcessSpawn, e))?;
            tokio::fs::create_dir_all(&cbcs_dir)
                .await
                .map_err(|e| DrmpackError::Io(PackagingOperation::ProcessSpawn, e))?;

            let (res_cenc, res_cbcs) = tokio::try_join!(
                execute_single_scheme(config, key_provider, EncryptionScheme::Cenc, &cenc_dir, &control_dir),
                execute_single_scheme(config, key_provider, EncryptionScheme::Cbcs, &cbcs_dir, &control_dir)
            )?;

            Ok(merge_dual_results(config, res_cenc, res_cbcs))
        }
        scheme => {
            execute_single_scheme(config, key_provider, scheme, &config.output_dir, &control_dir).await
        }
    };

    let _ = tokio::fs::remove_dir_all(&control_dir).await;
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

async fn execute_single_scheme(
    config: &VodPackageConfig,
    key_provider: &Arc<dyn KeyProvider>,
    scheme: EncryptionScheme,
    output_dir: &Path,
    control_dir: &Path,
) -> Result<VodPackageResult> {
    let (key_set, playback_keys) = KeyPolicyEngine::acquire_keys_with_playback(
        key_provider.as_ref(),
        &config.content_id,
        scheme,
        &config.renditions,
        config.key_mapping_policy,
    )
    .await?;

    let mut drm_config = GpacDrmConfig::new(scheme);
    for (idx, rendition) in config.renditions.iter().enumerate() {
        let container_track_id = rendition.effective_container_track_id(idx);
        if rendition.encrypted {
            drm_config = drm_config.with_track(
                container_track_id,
                rendition.track_type,
                rendition.quality_tier.clone(),
            );
        } else {
            drm_config = drm_config.with_clear_track(
                container_track_id,
                rendition.track_type,
                rendition.quality_tier.clone(),
            );
        }
    }

    let drm_xml = GpacDrmXmlGenerator::generate(&key_set, &drm_config)?;
    let drm_xml_path = control_dir.join(format!("drm_{}_{}.xml", scheme, Uuid::new_v4()));
    tokio::fs::write(&drm_xml_path, drm_xml)
        .await
        .map_err(|e| DrmpackError::Io(PackagingOperation::ProcessSpawn, e))?;

    let mut gpac_config = GpacVodProcessConfig::new(
        config.input.clone(),
        &drm_xml_path,
        output_dir,
    )
    .with_vod_mode(config.vod_mode)
    .with_segment_duration(config.segment_duration)
    .with_manifest_name("vod");

    if let Some(ref bin) = config.gpac_bin {
        gpac_config = gpac_config.with_gpac_bin(bin);
    }

    let args = gpac_config.build_args();
    info!(scheme = %scheme, args = ?args, "Executing GPAC VOD batch packaging");

    let mut child = Command::new(&gpac_config.gpac_bin)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| DrmpackError::Io(PackagingOperation::ProcessSpawn, e))?;

    let timeout_duration = config.timeout;
    let status_res = tokio::time::timeout(timeout_duration, child.wait()).await;

    let status = match status_res {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(DrmpackError::Io(PackagingOperation::ProcessSpawn, e)),
        Err(_) => {
            let _ = child.kill().await;
            return Err(DrmpackError::SessionTimeout);
        }
    };

    if !status.success() {
        let stderr = if let Some(mut err) = child.stderr.take() {
            use tokio::io::AsyncReadExt;
            let mut err_str = String::new();
            let _ = err.read_to_string(&mut err_str).await;
            err_str
        } else {
            String::new()
        };
        error!(status = ?status.code(), stderr = %stderr, "GPAC VOD packaging failed");
        return Err(DrmpackError::ProcessSupervisorFailure(format!(
            "GPAC exited with code {:?}: {}",
            status.code(),
            stderr
        )));
    }

    inspect_and_validate_output(output_dir, config, playback_keys).await
}

async fn inspect_and_validate_output(
    output_dir: &Path,
    config: &VodPackageConfig,
    playback_keys: Vec<crate::session::metadata::DrmKeyEntry>,
) -> Result<VodPackageResult> {
    let mpd_manifest = output_dir.join("vod.mpd");
    if !mpd_manifest.exists() {
        return Err(DrmpackError::ProcessSupervisorFailure(format!(
            "GPAC did not produce expected DASH manifest at {}",
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
        .map_err(|e| DrmpackError::Io(PackagingOperation::Harvest, e))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| DrmpackError::Io(PackagingOperation::Harvest, e))?
    {
        let path = entry.path();
        if path.is_file() {
            let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if fname.ends_with(".m3u8") && fname != "vod.m3u8" {
                let content = tokio::fs::read_to_string(&path)
                    .await
                    .map_err(|e| DrmpackError::Io(PackagingOperation::Harvest, e))?;
                if !content.contains("#EXT-X-ENDLIST") {
                    return Err(DrmpackError::ProcessSupervisorFailure(format!(
                        "Variant playlist {} missing #EXT-X-ENDLIST",
                        path.display()
                    )));
                }
                variant_playlists.push(path);
            } else if fname.ends_with("_init.mp4") {
                init_segments.push(path);
            } else if fname.ends_with(".mp4") && fname != "input.mp4" {
                media_files.push(path);
            } else if fname.ends_with(".m4s") {
                media_files.push(path);
            }
        }
    }

    variant_playlists.sort();
    media_files.sort();
    init_segments.sort();

    let metadata = DrmStreamMetadata::new(
        &config.content_id,
        config.encryption_scheme,
        LatencyMode::Standard,
        playback_keys,
        &config.drm_systems,
        false,
    );

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

    let metadata = DrmStreamMetadata::new(
        &config.content_id,
        EncryptionScheme::Dual,
        LatencyMode::Standard,
        keys,
        &config.drm_systems,
        true,
    );

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
```

In `src/vod/mod.rs`:
```rust
pub mod engine;
pub mod types;

pub use engine::package_vod_file;
pub use types::{VodInputSource, VodMode, VodPackageConfig, VodPackageResult};
```

**Step 4: Run test to verify it passes**

Run: `cargo test --test vod_batch_e2e -- test_package_vod_single_file_cbcs`
Expected: PASS

**Step 5: Commit**

```bash
git add src/vod/engine.rs src/vod/mod.rs tests/vod_batch_e2e.rs
git commit -m "feat(vod): implement standalone package_vod_file batch packaging engine"
```

---

### Task 5: End-to-End Tests for Segmented Mode, Dual Scheme, and Multi-Track (`tests/vod_batch_e2e.rs`)

**Files:**
- Modify: `tests/vod_batch_e2e.rs`
- Test: `tests/vod_batch_e2e.rs`

**Step 1: Write the tests**

Add tests:
- `test_package_vod_segmented_cbcs`
- `test_package_vod_dual_cenc_cbcs`
- `test_package_vod_missing_input_fails`

**Step 2: Run tests to verify**

Run: `cargo test --test vod_batch_e2e`
Expected: PASS for all tests

**Step 3: Commit**

```bash
git add tests/vod_batch_e2e.rs
git commit -m "test(vod): add integration tests for Segmented mode, Dual scheme, and error handling"
```

---

### Task 6: Example `12_vod_batch_packaging`, README, and ROADMAP Updates

**Files:**
- Create: `examples/12_vod_batch_packaging.rs`
- Modify: `README.md`
- Modify: `ROADMAP.md`

**Step 1: Write the example**

In `examples/12_vod_batch_packaging.rs`:
Runnable example generating a synthetic MP4 and executing `package_vod_file`.

**Step 2: Verify the example compiles and runs**

Run: `cargo run --example 12_vod_batch_packaging`
Expected: Outputs "Packaging complete!" with manifest paths.

**Step 3: Update `ROADMAP.md` and `README.md`**

Check off completed VOD items in `ROADMAP.md` and document `drmpack::vod` in `README.md`.

**Step 4: Commit**

```bash
git add examples/12_vod_batch_packaging.rs README.md ROADMAP.md
git commit -m "docs(vod): add 12_vod_batch_packaging example and update documentation"
```
