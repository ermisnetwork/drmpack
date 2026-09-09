use crate::error::Result;
use crate::types::{DrmSystem, EncryptionScheme, QualityTier, TrackType};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use std::fmt;

#[cfg(feature = "cpix")]
pub use crate::cpix::CpixProvider;

#[cfg(feature = "speke-v2")]
pub use crate::speke::{
    SigV4Credentials, SpekeAuth, SpekeClient, SpekeConfig, SpekeExchangeResponse, SpekeSigner,
    SpekeV2Config, SpekeV2Provider,
};

#[cfg(feature = "axinom")]
pub use crate::vendor::axinom::AxinomProvider;

pub mod policy;
pub mod raw;

pub mod extract;
pub use extract::extract_keys_from_dir;

pub use policy::{KeyPlan, KeyPolicyEngine};
pub use raw::{RawKeyProvider, StaticKeySource};

/// 128-bit KeyID (KID).
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
        self.0.simple().to_string()
    }
}

/// AES-128 Content Key with associated KeyID and metadata.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentKey {
    pub kid: KeyID,
    pub key: [u8; 16],
    pub quality_tier: QualityTier,
    pub track_type: TrackType,
    pub iv: Option<[u8; 16]>,
    pub encryption_scheme: Option<EncryptionScheme>,
}

impl fmt::Debug for ContentKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContentKey")
            .field("kid", &self.kid)
            .field("key", &"[REDACTED]")
            .field("quality_tier", &self.quality_tier)
            .field("track_type", &self.track_type)
            .field("iv", &self.iv.as_ref().map(|_| "[REDACTED]"))
            .field("encryption_scheme", &self.encryption_scheme)
            .finish()
    }
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
            encryption_scheme: None,
        }
    }

    pub fn new_with_scheme(
        kid: KeyID,
        key: [u8; 16],
        quality_tier: QualityTier,
        track_type: TrackType,
        scheme: EncryptionScheme,
    ) -> Self {
        Self {
            kid,
            key,
            quality_tier,
            track_type,
            iv: None,
            encryption_scheme: Some(scheme),
        }
    }

    pub fn with_iv(mut self, iv: [u8; 16]) -> Self {
        self.iv = Some(iv);
        self
    }

    pub fn with_encryption_scheme(mut self, scheme: EncryptionScheme) -> Self {
        self.encryption_scheme = Some(scheme);
        self
    }
}

/// PSSH (Protection System Specific Header) box data for a specific DRM system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PsshData {
    pub drm_system: DrmSystem,
    pub system_id: [u8; 16],
    pub data: Bytes,
    pub kid: Option<KeyID>,
    pub encryption_scheme: Option<EncryptionScheme>,
}

impl PsshData {
    pub fn new(drm_system: DrmSystem, system_id: [u8; 16], data: Bytes) -> Self {
        Self {
            drm_system,
            system_id,
            data,
            kid: None,
            encryption_scheme: None,
        }
    }

    pub fn with_kid(mut self, kid: KeyID) -> Self {
        self.kid = Some(kid);
        self
    }

    pub fn with_encryption_scheme(mut self, scheme: EncryptionScheme) -> Self {
        self.encryption_scheme = Some(scheme);
        self
    }
}

/// Description of keys requested from a KeyProvider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRequest {
    pub content_id: String,
    pub requested_quality_tiers: Vec<(TrackType, QualityTier)>,
    pub drm_systems: Vec<DrmSystem>,
    pub encryption_schemes: Vec<EncryptionScheme>,
}

impl KeyRequest {
    pub fn new(content_id: impl Into<String>) -> Self {
        Self {
            content_id: content_id.into(),
            requested_quality_tiers: Vec::new(),
            drm_systems: Vec::new(),
            encryption_schemes: Vec::new(),
        }
    }

    pub fn with_quality_tier(mut self, track_type: TrackType, tier: QualityTier) -> Self {
        self.requested_quality_tiers.push((track_type, tier));
        self
    }

    /// Alias for backwards compatibility with earlier code.
    pub fn with_tier(self, track_type: TrackType, tier: QualityTier) -> Self {
        self.with_quality_tier(track_type, tier)
    }

    pub fn with_drm_system(mut self, drm: DrmSystem) -> Self {
        self.drm_systems.push(drm);
        self
    }

    pub fn with_encryption_scheme(mut self, scheme: EncryptionScheme) -> Self {
        self.encryption_schemes.push(scheme);
        self
    }
    pub fn concrete_schemes(&self) -> Vec<Option<EncryptionScheme>> {
        if self.encryption_schemes.is_empty() {
            vec![None]
        } else {
            let mut schemes = Vec::new();
            for scheme in &self.encryption_schemes {
                for concrete in scheme.concrete_schemes() {
                    let opt = Some(*concrete);
                    if !schemes.contains(&opt) {
                        schemes.push(opt);
                    }
                }
            }
            schemes
        }
    }
}

/// The set of ContentKeys and PSSH boxes returned by a KeyProvider.
#[derive(Debug, Clone, Default)]
pub struct KeySet {
    pub keys: HashMap<(Option<EncryptionScheme>, TrackType, QualityTier), ContentKey>,
    pub pssh: Vec<PsshData>,
}

impl KeySet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_key(&mut self, key: ContentKey) {
        self.keys.insert(
            (
                key.encryption_scheme,
                key.track_type,
                key.quality_tier.clone(),
            ),
            key,
        );
    }

    pub fn get_key(
        &self,
        track_type: TrackType,
        quality_tier: &QualityTier,
    ) -> Option<&ContentKey> {
        self.keys
            .get(&(None, track_type, quality_tier.clone()))
            .or_else(|| {
                self.keys
                    .iter()
                    .find(|((_, t, q), _)| *t == track_type && q == quality_tier)
                    .map(|(_, k)| k)
            })
    }

    pub fn get_key_for_scheme(
        &self,
        scheme: EncryptionScheme,
        track_type: TrackType,
        quality_tier: &QualityTier,
    ) -> Option<&ContentKey> {
        self.keys
            .get(&(Some(scheme), track_type, quality_tier.clone()))
            .or_else(|| self.keys.get(&(None, track_type, quality_tier.clone())))
    }

    pub fn add_pssh(&mut self, pssh: PsshData) {
        if !self.pssh.iter().any(|p| p == &pssh) {
            self.pssh.push(pssh);
        }
    }

    /// Query PSSH data associated with a specific KeyID.
    pub fn pssh_for_kid<'a>(&'a self, kid: &'a KeyID) -> impl Iterator<Item = &'a PsshData> {
        self.pssh
            .iter()
            .filter(move |p| p.kid.as_ref().is_none_or(|k| k == kid))
    }

    /// Query PSSH data associated with a specific EncryptionScheme.
    pub fn pssh_for_scheme(&self, scheme: EncryptionScheme) -> impl Iterator<Item = &PsshData> {
        self.pssh
            .iter()
            .filter(move |p| p.encryption_scheme.is_none_or(|s| s == scheme))
    }

    pub fn all_keys(&self) -> impl Iterator<Item = &ContentKey> {
        self.keys.values()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }
}

#[cfg(feature = "axinom")]
impl KeySet {
    /// Build deduplicated, sorted Axinom key configs from this KeySet.
    ///
    /// Automatically derives IV for CBCS keys (IV = KID bytes, FairPlay convention).
    /// Handles sort by KID and dedup — caller never needs to touch `AxinomKeyConfig` directly.
    pub fn to_axinom_key_configs(&self) -> Vec<crate::vendor::axinom::AxinomKeyConfig> {
        let mut configs: Vec<crate::vendor::axinom::AxinomKeyConfig> = self
            .all_keys()
            .map(crate::vendor::axinom::AxinomKeyConfig::from)
            .collect();
        configs.sort_by(|a, b| a.kid.cmp(&b.kid));
        configs.dedup_by(|a, b| a.kid == b.kid);
        configs
    }

    /// Generate a signed Axinom JWT entitlement token for all keys in this set.
    ///
    /// One-liner that handles key config extraction, IV derivation, sort, dedup, and signing.
    pub fn generate_axinom_jwt(&self, com_key_id: &str, com_key: &str) -> Result<String> {
        let configs = self.to_axinom_key_configs();
        crate::vendor::axinom::generate_axinom_jwt(com_key_id, com_key, &configs)
    }
}

/// Pluggable trait for DRM key acquisition.
pub trait KeyProvider: Send + Sync {
    fn fetch_keys(
        &self,
        request: &KeyRequest,
    ) -> impl std::future::Future<Output = Result<KeySet>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_scheme_aware_key_differentiation() {
        let kid_cenc = KeyID::random();
        let kid_cbcs = KeyID::random();
        let key_cenc = ContentKey::new_with_scheme(
            kid_cenc,
            [0x11; 16],
            QualityTier::hd(),
            TrackType::Video,
            EncryptionScheme::Cenc,
        );
        let key_cbcs = ContentKey::new_with_scheme(
            kid_cbcs,
            [0x22; 16],
            QualityTier::hd(),
            TrackType::Video,
            EncryptionScheme::Cbcs,
        );

        let mut keyset = KeySet::new();
        keyset.insert_key(key_cenc);
        keyset.insert_key(key_cbcs);

        let fetched_cenc = keyset
            .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
            .unwrap();
        let fetched_cbcs = keyset
            .get_key_for_scheme(EncryptionScheme::Cbcs, TrackType::Video, &QualityTier::hd())
            .unwrap();

        assert_eq!(fetched_cenc.kid, kid_cenc);
        assert_eq!(fetched_cenc.key, [0x11; 16]);
        assert_eq!(fetched_cbcs.kid, kid_cbcs);
        assert_eq!(fetched_cbcs.key, [0x22; 16]);
    }

    #[test]
    fn test_keyset_pssh_deduplication() {
        let mut keyset = KeySet::new();
        let pssh = PsshData::new(
            DrmSystem::Widevine,
            DrmSystem::Widevine.system_id(),
            Bytes::from_static(b"pssh-payload"),
        );
        keyset.add_pssh(pssh.clone());
        keyset.add_pssh(pssh);
        assert_eq!(keyset.pssh.len(), 1, "Duplicate PSSH must be deduplicated");
    }

    #[tokio::test]
    async fn test_key_provider_native_future_is_send() {
        struct NativeMockProvider;
        impl KeyProvider for NativeMockProvider {
            async fn fetch_keys(&self, _request: &KeyRequest) -> Result<KeySet> {
                Ok(KeySet::new())
            }
        }

        let provider = NativeMockProvider;
        let request = KeyRequest::new("test");
        let handle = tokio::spawn(async move { provider.fetch_keys(&request).await });
        let res = handle.await.unwrap();
        assert!(res.is_ok());
    }
}
