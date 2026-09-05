use bytes::Bytes;
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

/// A single rendition declaration (e.g. 720p@2Mbps).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rendition {
    pub id: String,
    pub track_type: TrackType,
    pub quality_tier: QualityTier,
    pub track_id: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bitrate: u64,
    pub frame_rate: Option<f64>,
    pub codecs: String,
    pub encrypted: bool,
}

impl Rendition {
    pub fn video(
        id: impl Into<String>,
        quality_tier: QualityTier,
        width: u32,
        height: u32,
        bitrate: u64,
        codecs: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            track_type: TrackType::Video,
            quality_tier,
            track_id: None,
            width: Some(width),
            height: Some(height),
            bitrate,
            frame_rate: None,
            codecs: codecs.into(),
            encrypted: true,
        }
    }

    pub fn audio(
        id: impl Into<String>,
        quality_tier: QualityTier,
        bitrate: u64,
        codecs: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            track_type: TrackType::Audio,
            quality_tier,
            track_id: None,
            width: None,
            height: None,
            bitrate,
            frame_rate: None,
            codecs: codecs.into(),
            encrypted: true,
        }
    }

    /// Construct a clear (unencrypted) subtitle rendition.
    pub fn subtitle(id: impl Into<String>, codecs: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            track_type: TrackType::Subtitle,
            quality_tier: QualityTier::new("default"),
            track_id: None,
            width: None,
            height: None,
            bitrate: 0,
            frame_rate: None,
            codecs: codecs.into(),
            encrypted: false,
        }
    }

    /// Set explicit track ID (1-based ISO-BMFF track ID).
    pub fn with_track_id(mut self, track_id: u32) -> Self {
        self.track_id = Some(track_id);
        self
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

/// A media segment to be packaged.
#[derive(Debug, Clone)]
pub struct Segment {
    pub rendition_id: String,
    pub sequence_number: u64,
    pub duration_seconds: f64,
    pub data: Bytes,
    pub is_init: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rendition_video_constructor() {
        let r = Rendition::video(
            "v1",
            QualityTier::hd(),
            1920,
            1080,
            2_000_000,
            "avc1.640028",
        );
        assert_eq!(r.id, "v1");
        assert_eq!(r.track_type, TrackType::Video);
        assert_eq!(r.quality_tier, QualityTier::hd());
        assert_eq!(r.track_id, None);
        assert_eq!(r.width, Some(1920));
        assert_eq!(r.height, Some(1080));
        assert_eq!(r.bitrate, 2_000_000);
        assert_eq!(r.codecs, "avc1.640028");
        assert!(r.encrypted);
    }

    #[test]
    fn test_rendition_audio_constructor() {
        let r = Rendition::audio("a1", QualityTier::sd(), 128_000, "mp4a.40.2");
        assert_eq!(r.id, "a1");
        assert_eq!(r.track_type, TrackType::Audio);
        assert_eq!(r.quality_tier, QualityTier::sd());
        assert_eq!(r.track_id, None);
        assert_eq!(r.width, None);
        assert_eq!(r.height, None);
        assert_eq!(r.bitrate, 128_000);
        assert_eq!(r.codecs, "mp4a.40.2");
        assert!(r.encrypted);
    }

    #[test]
    fn test_rendition_subtitle_constructor() {
        let r = Rendition::subtitle("subs_en", "tx3g");
        assert_eq!(r.id, "subs_en");
        assert_eq!(r.track_type, TrackType::Subtitle);
        assert_eq!(r.quality_tier, QualityTier::new("default"));
        assert_eq!(r.track_id, None);
        assert_eq!(r.width, None);
        assert_eq!(r.height, None);
        assert_eq!(r.bitrate, 0);
        assert_eq!(r.codecs, "tx3g");
        assert!(
            !r.encrypted,
            "Subtitle renditions must default to unencrypted"
        );
    }

    #[test]
    fn test_rendition_with_track_id() {
        let r = Rendition::subtitle("subs_en", "tx3g").with_track_id(4);
        assert_eq!(r.track_id, Some(4));

        let v = Rendition::video(
            "v1",
            QualityTier::hd(),
            1920,
            1080,
            2_000_000,
            "avc1.640028",
        )
        .with_track_id(1);
        assert_eq!(v.track_id, Some(1));
    }

    #[test]
    fn test_rendition_clear_and_with_encrypted() {
        let mut r = Rendition::video(
            "v1",
            QualityTier::hd(),
            1920,
            1080,
            2_000_000,
            "avc1.640028",
        );
        assert!(r.encrypted);

        r = r.clear();
        assert!(!r.encrypted);

        r = r.with_encrypted(true);
        assert!(r.encrypted);
    }
}
