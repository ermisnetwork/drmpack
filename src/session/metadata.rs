//! DRM playback stream metadata and serialization for state persistence.
//!
//! Exposes [`DrmStreamMetadata`] and [`DrmKeyEntry`] for transferring public DRM
//! metadata from packaging sessions to state stores (PostgreSQL, Redis) and playback backends.
//!
//! Strictly excludes raw AES secret keys to prevent accidental leakage in playback services.

use crate::key::{KeyID, KeySet};
use crate::types::{EncryptionScheme, QualityTier, TrackType};
use serde::{Deserialize, Serialize};

/// A single content key entry in stream DRM metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrmKeyEntry {
    /// Associated KeyID.
    pub kid: KeyID,
    /// Concrete encryption scheme (CENC or CBCS).
    pub scheme: EncryptionScheme,
    /// Elementary track type (video or audio).
    pub track_type: TrackType,
    /// Associated quality tier (e.g. SD, HD, 4K).
    pub quality_tier: QualityTier,
    /// Optional 128-bit initialization vector.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iv: Option<[u8; 16]>,
}

/// Public DRM stream metadata emitted by `PackagingSession::playback_metadata()`.
///
/// Designed to be serialized to JSON and stored in a database or cache alongside
/// stream records. Contains everything a playback authorization backend needs to
/// mint client entitlement tokens (e.g. Axinom JWT) without filesystem access or
/// raw key leakage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrmStreamMetadata {
    /// Content or stream identifier string.
    pub content_id: String,
    /// Packaging encryption mode.
    pub scheme: EncryptionScheme,
    /// List of public key entries.
    pub keys: Vec<DrmKeyEntry>,
}

impl DrmStreamMetadata {
    /// Construct metadata from session parameters and a resolved [`KeySet`].
    pub fn from_session(
        content_id: impl Into<String>,
        scheme: EncryptionScheme,
        key_set: &KeySet,
    ) -> Self {
        let content_id = content_id.into();
        let mut keys = Vec::new();

        for key in key_set.all_keys() {
            let effective_scheme = key.encryption_scheme.unwrap_or(scheme);
            let iv = key.iv.or_else(|| {
                if effective_scheme == EncryptionScheme::Cbcs {
                    // Apple FairPlay / Axinom convention: IV = 16 bytes of KID UUID
                    Some(*key.kid.as_bytes())
                } else {
                    None
                }
            });

            keys.push(DrmKeyEntry {
                kid: key.kid,
                scheme: effective_scheme,
                track_type: key.track_type,
                quality_tier: key.quality_tier.clone(),
                iv,
            });
        }

        Self {
            content_id,
            scheme,
            keys,
        }
    }

    /// Return all unique Key IDs associated with this stream.
    pub fn kids(&self) -> Vec<KeyID> {
        let mut seen = std::collections::HashSet::new();
        let mut res = Vec::new();
        for k in &self.keys {
            if seen.insert(k.kid) {
                res.push(k.kid);
            }
        }
        res
    }

    /// Return all unique Key IDs formatted as hyphenated UUID strings.
    pub fn kid_strings(&self) -> Vec<String> {
        self.kids()
            .into_iter()
            .map(|k| k.0.hyphenated().to_string())
            .collect()
    }

    /// Convert to JSON string for persistence (e.g. into PostgreSQL or Redis).
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Convert to pretty JSON string.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Load from a JSON string retrieved from database or storage.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[cfg(feature = "axinom")]
impl DrmStreamMetadata {
    /// Convert metadata entries into deduplicated, sorted Axinom key configurations.
    pub fn to_axinom_key_configs(&self) -> Vec<crate::vendor::axinom::AxinomKeyConfig> {
        let mut configs: Vec<crate::vendor::axinom::AxinomKeyConfig> = self
            .keys
            .iter()
            .map(|k| {
                let mut cfg =
                    crate::vendor::axinom::AxinomKeyConfig::new(k.kid.0.hyphenated().to_string());
                if let Some(iv) = k.iv {
                    cfg = cfg.with_iv(iv);
                }
                cfg
            })
            .collect();
        configs.sort_by(|a, b| a.kid.cmp(&b.kid));
        configs.dedup_by(|a, b| a.kid == b.kid);
        configs
    }

    /// Generate an Axinom JWT entitlement token using the provided signing credentials.
    pub fn generate_axinom_jwt(
        &self,
        signing_config: &crate::vendor::axinom::AxinomSigningConfig,
    ) -> crate::error::Result<String> {
        signing_config.generate_jwt_for_metadata(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drm_stream_metadata_serialization_roundtrip() {
        let kid = KeyID::random();
        let meta = DrmStreamMetadata {
            content_id: "test-stream-123".to_string(),
            scheme: EncryptionScheme::Dual,
            keys: vec![
                DrmKeyEntry {
                    kid,
                    scheme: EncryptionScheme::Cenc,
                    track_type: TrackType::Video,
                    quality_tier: QualityTier::hd(),
                    iv: None,
                },
                DrmKeyEntry {
                    kid,
                    scheme: EncryptionScheme::Cbcs,
                    track_type: TrackType::Video,
                    quality_tier: QualityTier::hd(),
                    iv: Some(*kid.as_bytes()),
                },
            ],
        };

        let json = meta.to_json().expect("serialization should succeed");
        assert!(json.contains(r#""scheme":"dual""#));
        assert!(json.contains(r#""scheme":"cenc""#));
        assert!(json.contains(r#""scheme":"cbcs""#));
        assert!(json.contains(r#""track_type":"video""#));

        let decoded = DrmStreamMetadata::from_json(&json).expect("deserialization should succeed");

        assert_eq!(meta, decoded);
        assert_eq!(decoded.kids().len(), 1);
        assert_eq!(decoded.kid_strings(), vec![kid.0.hyphenated().to_string()]);
    }

    #[test]
    fn test_drm_stream_metadata_legacy_casing_compatibility() {
        // Simulates legacy JSON stored in PostgreSQL / Redis with PascalCase or uppercase tokens
        let legacy_json = r#"{
            "content_id": "legacy-stream-456",
            "scheme": "Dual",
            "keys": [
                {
                    "kid": "00000000-0000-0000-0000-000000000001",
                    "scheme": "Cenc",
                    "track_type": "Video",
                    "quality_tier": "HD"
                },
                {
                    "kid": "00000000-0000-0000-0000-000000000002",
                    "scheme": "CBCS",
                    "track_type": "AUDIO",
                    "quality_tier": "SD",
                    "iv": [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
                }
            ]
        }"#;

        let meta = DrmStreamMetadata::from_json(legacy_json)
            .expect("Legacy PascalCase and uppercase JSON must deserialize successfully");

        assert_eq!(meta.content_id, "legacy-stream-456");
        assert_eq!(meta.scheme, EncryptionScheme::Dual);
        assert_eq!(meta.keys[0].scheme, EncryptionScheme::Cenc);
        assert_eq!(meta.keys[0].track_type, TrackType::Video);
        assert_eq!(meta.keys[1].scheme, EncryptionScheme::Cbcs);
        assert_eq!(meta.keys[1].track_type, TrackType::Audio);

        // When re-serialized, output is modernized to lowercase
        let modern_json = meta.to_json().expect("Serialization must succeed");
        assert!(modern_json.contains(r#""scheme":"dual""#));
        assert!(modern_json.contains(r#""scheme":"cenc""#));
        assert!(modern_json.contains(r#""scheme":"cbcs""#));
        assert!(modern_json.contains(r#""track_type":"video""#));
        assert!(modern_json.contains(r#""track_type":"audio""#));
    }
}
