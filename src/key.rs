use crate::error::{DrmpackError, Result};
use crate::types::{DrmSystem, QualityTier, TrackType};
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// 128-bit Key Identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyID(pub Uuid);

impl KeyID {
    pub fn new(uuid: Uuid) -> Self {
        Self(uuid)
    }

    pub fn random() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0.as_bytes())
    }
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            use std::fmt::Write;
            let _ = write!(s, "{:02x}", b);
        }
        s
    }
}

/// AES-128 Content Key with associated KeyID and metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentKey {
    pub kid: KeyID,
    pub key: [u8; 16],
    pub quality_tier: QualityTier,
    pub track_type: TrackType,
    pub iv: Option<[u8; 16]>,
}

impl ContentKey {
    pub fn new(
        kid: KeyID,
        key: [u8; 16],
        quality_tier: QualityTier,
        track_type: TrackType,
    ) -> Self {
        Self {
            kid,
            key,
            quality_tier,
            track_type,
            iv: None,
        }
    }

    pub fn with_iv(mut self, iv: [u8; 16]) -> Self {
        self.iv = Some(iv);
        self
    }
}

/// PSSH (Protection System Specific Header) box data for a specific DRM system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PsshData {
    pub drm_system: DrmSystem,
    pub system_id: [u8; 16],
    pub data: Bytes,
}

/// Description of keys requested from a KeyProvider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRequest {
    pub content_id: String,
    pub requested_tiers: Vec<(TrackType, QualityTier)>,
    pub drm_systems: Vec<DrmSystem>,
}

/// The set of ContentKeys and PSSH boxes returned by a KeyProvider.
#[derive(Debug, Clone, Default)]
pub struct KeySet {
    pub keys: HashMap<(TrackType, QualityTier), ContentKey>,
    pub pssh: Vec<PsshData>,
}

impl KeySet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_key(&mut self, key: ContentKey) {
        self.keys
            .insert((key.track_type, key.quality_tier.clone()), key);
    }

    pub fn get_key(
        &self,
        track_type: TrackType,
        quality_tier: &QualityTier,
    ) -> Option<&ContentKey> {
        self.keys.get(&(track_type, quality_tier.clone()))
    }

    pub fn add_pssh(&mut self, pssh: PsshData) {
        self.pssh.push(pssh);
    }
}

/// Pluggable trait for DRM key acquisition.
#[async_trait]
pub trait KeyProvider: Send + Sync {
    async fn fetch_keys(&self, request: &KeyRequest) -> Result<KeySet>;
}

/// Raw key provider supplying manually configured keys for testing and development.
#[derive(Debug, Clone, Default)]
pub struct RawKeyProvider {
    keys: HashMap<(TrackType, QualityTier), ContentKey>,
    pssh: Vec<PsshData>,
}

impl RawKeyProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_key(mut self, key: ContentKey) -> Self {
        self.keys
            .insert((key.track_type, key.quality_tier.clone()), key);
        self
    }

    pub fn with_pssh(mut self, pssh: PsshData) -> Self {
        self.pssh.push(pssh);
        self
    }

    pub fn add_key(&mut self, key: ContentKey) {
        self.keys
            .insert((key.track_type, key.quality_tier.clone()), key);
    }
}

#[async_trait]
impl KeyProvider for RawKeyProvider {
    async fn fetch_keys(&self, request: &KeyRequest) -> Result<KeySet> {
        let mut set = KeySet::new();
        for (track_type, tier) in &request.requested_tiers {
            if let Some(key) = self.keys.get(&(*track_type, tier.clone())) {
                set.insert_key(key.clone());
            } else {
                return Err(DrmpackError::KeyProvider(format!(
                    "No raw key configured for {:?} / {}",
                    track_type, tier
                )));
            }
        }
        set.pssh = self.pssh.clone();
        Ok(set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_raw_key_provider_success() {
        let kid = KeyID::random();
        let key_bytes = [1u8; 16];
        let content_key = ContentKey::new(kid, key_bytes, QualityTier::hd(), TrackType::Video);

        let provider = RawKeyProvider::new().with_key(content_key.clone());

        let req = KeyRequest {
            content_id: "test-content".into(),
            requested_tiers: vec![(TrackType::Video, QualityTier::hd())],
            drm_systems: vec![DrmSystem::Widevine],
        };

        let keyset = provider.fetch_keys(&req).await.unwrap();
        let fetched_key = keyset
            .get_key(TrackType::Video, &QualityTier::hd())
            .unwrap();
        assert_eq!(fetched_key.kid, kid);
        assert_eq!(fetched_key.key, key_bytes);
    }

    #[tokio::test]
    async fn test_raw_key_provider_missing_key() {
        let provider = RawKeyProvider::new();
        let req = KeyRequest {
            content_id: "test-content".into(),
            requested_tiers: vec![(TrackType::Video, QualityTier::hd())],
            drm_systems: vec![DrmSystem::Widevine],
        };

        let result = provider.fetch_keys(&req).await;
        assert!(result.is_err());
    }
}
