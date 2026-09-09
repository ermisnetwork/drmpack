use serde::{Deserialize, Serialize};
use std::fmt;

/// The encryption scheme used for protecting media segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EncryptionScheme {
    /// Common Encryption using AES-128 in CTR mode (Widevine / PlayReady).
    Cenc,
    /// Common Encryption using AES-128 in CBC mode with 10% pattern encryption (FairPlay / modern Widevine).
    Cbcs,
    /// Dual encryption producing both CENC and CBCS representations simultaneously.
    Dual,
}

impl EncryptionScheme {
    /// Return the concrete encryption schemes represented by this scheme.
    /// `Cenc` -> `[Cenc]`, `Cbcs` -> `[Cbcs]`, `Dual` -> `[Cenc, Cbcs]`.
    pub fn concrete_schemes(&self) -> &'static [EncryptionScheme] {
        match self {
            EncryptionScheme::Cenc => &[EncryptionScheme::Cenc],
            EncryptionScheme::Cbcs => &[EncryptionScheme::Cbcs],
            EncryptionScheme::Dual => &[EncryptionScheme::Cenc, EncryptionScheme::Cbcs],
        }
    }
}

impl fmt::Display for EncryptionScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncryptionScheme::Cenc => write!(f, "cenc"),
            EncryptionScheme::Cbcs => write!(f, "cbcs"),
            EncryptionScheme::Dual => write!(f, "dual"),
        }
    }
}

/// The manifest protocol used to deliver a Representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ManifestFormat {
    Dash,
    Hls,
}

impl fmt::Display for ManifestFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestFormat::Dash => write!(f, "dash"),
            ManifestFormat::Hls => write!(f, "hls"),
        }
    }
}

/// The streaming delivery latency profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LatencyMode {
    /// Standard delivery with traditional segment durations (2s - 6s).
    Standard,
    /// Low-latency streaming with CMAF chunking, LL-HLS partial segments, and LL-DASH availability time offset.
    #[default]
    LowLatency,
}

impl fmt::Display for LatencyMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LatencyMode::Standard => write!(f, "standard"),
            LatencyMode::LowLatency => write!(f, "low-latency"),
        }
    }
}

/// DRM system targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DrmSystem {
    Widevine,
    FairPlay,
    PlayReady,
}

impl DrmSystem {
    /// System ID UUID for the DRM system.
    pub fn system_id(&self) -> [u8; 16] {
        match self {
            // edef8ba9-79d6-4ace-a3c8-27dcd51d21ed
            DrmSystem::Widevine => [
                0xed, 0xef, 0x8b, 0xa9, 0x79, 0xd6, 0x4a, 0xce, 0xa3, 0xc8, 0x27, 0xdc, 0xd5, 0x1d,
                0x21, 0xed,
            ],
            // 94ce86fb-07ff-4f43-adb8-93d2fa968ca2
            DrmSystem::FairPlay => [
                0x94, 0xce, 0x86, 0xfb, 0x07, 0xff, 0x4f, 0x43, 0xad, 0xb8, 0x93, 0xd2, 0xfa, 0x96,
                0x8c, 0xa2,
            ],
            // 9a04f079-9840-4286-ab92-e65be0885f95
            DrmSystem::PlayReady => [
                0x9a, 0x04, 0xf0, 0x79, 0x98, 0x40, 0x42, 0x86, 0xab, 0x92, 0xe6, 0x5b, 0xe0, 0x88,
                0x5f, 0x95,
            ],
        }
    }
}

/// Media track type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrackType {
    Video,
    Audio,
    Subtitle,
}

impl fmt::Display for TrackType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrackType::Video => write!(f, "video"),
            TrackType::Audio => write!(f, "audio"),
            TrackType::Subtitle => write!(f, "subtitle"),
        }
    }
}

/// Quality tier grouping renditions that share a ContentKey.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QualityTier(pub String);

impl QualityTier {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn sd() -> Self {
        Self("SD".into())
    }

    pub fn hd() -> Self {
        Self("HD".into())
    }

    pub fn uhd_4k() -> Self {
        Self("4K".into())
    }
}

impl fmt::Display for QualityTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The orchestration policy governing how ContentKeys are assigned across Renditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum KeyMappingPolicy {
    /// Single ContentKey shared across all encrypted Renditions (default).
    #[default]
    SharedAll,
    /// One ContentKey for all video Renditions, and one ContentKey for all audio Renditions.
    SharedVideoSingleAudio,
    /// Granular ContentKey per (TrackType, QualityTier) combination (ADR-0003).
    PerTierAndTrack,
}

/// A single rendition declaration bound to a QualityTier for DRM key association.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rendition {
    pub track_id: String,
    pub track_type: TrackType,
    pub quality_tier: QualityTier,
    pub container_track_id: Option<u32>,
    pub encrypted: bool,
}

fn generate_track_identifier(track_type: TrackType) -> String {
    format!("track_{}_{:.8}", track_type, uuid::Uuid::new_v4().simple())
}

impl Rendition {
    /// Construct an encrypted video rendition with the specified quality tier.
    pub fn video(quality_tier: QualityTier) -> Self {
        Self {
            track_id: generate_track_identifier(TrackType::Video),
            track_type: TrackType::Video,
            quality_tier,
            container_track_id: None,
            encrypted: true,
        }
    }

    /// Construct an encrypted video rendition with the standard HD quality tier.
    pub fn video_hd() -> Self {
        Self::video(QualityTier::hd())
    }

    /// Construct an encrypted video rendition with the UHD 4K quality tier.
    pub fn video_4k() -> Self {
        Self::video(QualityTier::uhd_4k())
    }

    /// Construct an encrypted audio rendition with the default SD quality tier.
    pub fn audio() -> Self {
        Self::audio_tier(QualityTier::sd())
    }

    /// Construct an encrypted audio rendition with the specified quality tier.
    pub fn audio_tier(quality_tier: QualityTier) -> Self {
        Self {
            track_id: generate_track_identifier(TrackType::Audio),
            track_type: TrackType::Audio,
            quality_tier,
            container_track_id: None,
            encrypted: true,
        }
    }

    /// Construct a clear (unencrypted) subtitle rendition.
    pub fn subtitle() -> Self {
        Self {
            track_id: generate_track_identifier(TrackType::Subtitle),
            track_type: TrackType::Subtitle,
            quality_tier: QualityTier::new("default"),
            container_track_id: None,
            encrypted: false,
        }
    }

    /// Set explicit container track ID (1-based ISO-BMFF track ID).
    pub fn with_container_track_id(mut self, track_id: u32) -> Self {
        self.container_track_id = Some(track_id);
        self
    }

    /// Returns the configured `container_track_id`, or falls back to `(index + 1) as u32` (1-based ISO-BMFF convention).
    pub fn effective_container_track_id(&self, index: usize) -> u32 {
        self.container_track_id.unwrap_or((index + 1) as u32)
    }

    /// Mark this rendition as clear (unencrypted), bypassing GPAC cecrypt.
    pub fn clear(mut self) -> Self {
        self.encrypted = false;
        self
    }

    /// Set explicit encryption state.
    pub fn with_encrypted(mut self, encrypted: bool) -> Self {
        self.encrypted = encrypted;
        self
    }
}

/// Classification of an emitted PackagedArtifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ArtifactKind {
    /// An initialization segment containing container headers (e.g. `ftyp`, `moov`).
    InitSegment,
    /// A media segment containing encoded samples (e.g. `moof`, `mdat`).
    MediaSegment,
    /// A manifest playlist or description file (e.g. `.m3u8` or `.mpd`).
    Manifest,
}

impl fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArtifactKind::InitSegment => write!(f, "init_segment"),
            ArtifactKind::MediaSegment => write!(f, "media_segment"),
            ArtifactKind::Manifest => write!(f, "manifest"),
        }
    }
}

/// An encrypted media segment or updated manifest emitted directly to callers via an async output channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagedArtifact {
    /// Relative filename of the artifact (e.g. `video_1080p_1.m4s`, `live.m3u8`).
    pub filename: String,
    /// In-memory binary payload.
    pub data: bytes::Bytes,
    /// Structural classification of the artifact.
    pub kind: ArtifactKind,
    /// Concrete encryption scheme of the artifact.
    pub scheme: EncryptionScheme,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rendition_video_constructor() {
        let r = Rendition::video(QualityTier::hd());
        assert!(r.track_id.starts_with("track_video_"));
        assert_eq!(r.track_type, TrackType::Video);
        assert_eq!(r.quality_tier, QualityTier::hd());
        assert_eq!(r.container_track_id, None);
        assert!(r.encrypted);
    }

    #[test]
    fn test_rendition_video_presets() {
        let hd = Rendition::video_hd();
        assert!(hd.track_id.starts_with("track_video_"));
        assert_eq!(hd.quality_tier, QualityTier::hd());

        let uhd = Rendition::video_4k();
        assert!(uhd.track_id.starts_with("track_video_"));
        assert_eq!(uhd.quality_tier, QualityTier::uhd_4k());
    }

    #[test]
    fn test_rendition_audio_constructor() {
        let r = Rendition::audio();
        assert!(r.track_id.starts_with("track_audio_"));
        assert_eq!(r.track_type, TrackType::Audio);
        assert_eq!(r.quality_tier, QualityTier::sd());
        assert_eq!(r.container_track_id, None);
        assert!(r.encrypted);

        let r_tier = Rendition::audio_tier(QualityTier::new("lossless"));
        assert!(r_tier.track_id.starts_with("track_audio_"));
        assert_eq!(r_tier.quality_tier, QualityTier::new("lossless"));
    }

    #[test]
    fn test_rendition_subtitle_constructor() {
        let r = Rendition::subtitle();
        assert!(r.track_id.starts_with("track_subtitle_"));
        assert_eq!(r.track_type, TrackType::Subtitle);
        assert_eq!(r.quality_tier, QualityTier::new("default"));
        assert_eq!(r.container_track_id, None);
        assert!(
            !r.encrypted,
            "Subtitle renditions must default to unencrypted"
        );
    }

    #[test]
    fn test_rendition_with_container_track_id() {
        let r = Rendition::subtitle().with_container_track_id(4);
        assert_eq!(r.container_track_id, Some(4));

        let v = Rendition::video_hd().with_container_track_id(1);
        assert_eq!(v.container_track_id, Some(1));
    }

    #[test]
    fn test_rendition_effective_container_track_id() {
        let r_default = Rendition::audio();
        assert_eq!(r_default.effective_container_track_id(0), 1);
        assert_eq!(r_default.effective_container_track_id(2), 3);

        let r_custom = r_default.with_container_track_id(10);
        assert_eq!(r_custom.effective_container_track_id(0), 10);
    }

    #[test]
    fn test_rendition_clear_and_with_encrypted() {
        let mut r = Rendition::video_hd();
        assert!(r.encrypted);

        r = r.clear();
        assert!(!r.encrypted);

        r = r.with_encrypted(true);
        assert!(r.encrypted);
    }

    #[test]
    fn test_multiple_renditions_same_tier_no_id_collision() {
        let r1 = Rendition::video(QualityTier::hd());
        let r2 = Rendition::video(QualityTier::hd());
        assert_ne!(
            r1.track_id, r2.track_id,
            "Renditions sharing the same tier must have distinct IDs"
        );
        assert_eq!(r1.effective_container_track_id(0), 1);
        assert_eq!(r2.effective_container_track_id(1), 2);
    }

    #[test]
    fn test_rendition_serde_json() {
        let mut r = Rendition::video_hd().with_container_track_id(1);
        r.track_id = "track_video_custom123".to_string();

        let json = serde_json::to_string(&r).expect("Serialization failed");
        assert!(json.contains(r#""track_id":"track_video_custom123""#));
        assert!(json.contains(r#""container_track_id":1"#));
        assert!(json.contains(r#""encrypted":true"#));
        let deserialized: Rendition = serde_json::from_str(&json).expect("Deserialization failed");
        assert_eq!(r, deserialized);
    }

    #[test]
    fn test_packaged_artifact_and_kind() {
        let artifact = PackagedArtifact {
            filename: "video_1080p_1.m4s".to_string(),
            data: bytes::Bytes::from_static(b"test-segment-bytes"),
            kind: ArtifactKind::MediaSegment,
            scheme: EncryptionScheme::Cbcs,
        };
        assert_eq!(artifact.filename, "video_1080p_1.m4s");
        assert_eq!(artifact.data.as_ref(), b"test-segment-bytes");
        assert_eq!(artifact.kind, ArtifactKind::MediaSegment);
        assert_eq!(artifact.scheme, EncryptionScheme::Cbcs);

        let json = serde_json::to_string(&artifact).expect("Serialization failed");
        assert!(json.contains(r#""filename":"video_1080p_1.m4s""#));
        assert!(json.contains(r#""kind":"MediaSegment""#));
        assert!(json.contains(r#""scheme":"Cbcs""#));

        let deserialized: PackagedArtifact =
            serde_json::from_str(&json).expect("Deserialization failed");
        assert_eq!(deserialized, artifact);

        assert_eq!(ArtifactKind::InitSegment.to_string(), "init_segment");
        assert_eq!(ArtifactKind::MediaSegment.to_string(), "media_segment");
        assert_eq!(ArtifactKind::Manifest.to_string(), "manifest");
    }
}
