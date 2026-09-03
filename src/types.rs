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
    /// Dual encryption producing both CENC and CBCS streams simultaneously.
    Dual,
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

/// A single rendition declaration (e.g. 720p@2Mbps).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rendition {
    pub id: String,
    pub track_type: TrackType,
    pub quality_tier: QualityTier,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bitrate: u64,
    pub frame_rate: Option<f64>,
    pub codecs: String,
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
            width: Some(width),
            height: Some(height),
            bitrate,
            frame_rate: None,
            codecs: codecs.into(),
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
            width: None,
            height: None,
            bitrate,
            frame_rate: None,
            codecs: codecs.into(),
        }
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
