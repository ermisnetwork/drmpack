//! DRM key management, key sets, PSSH boxes, and key provider abstractions.
//!
//! Provides [`ContentKey`], [`KeyID`], [`KeySet`], and the [`KeyProvider`] trait
//! implemented by vendor adapters and local test doubles.

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

/// Key mapping policy evaluation and resolution engine.
pub mod policy;
/// In-memory pre-shared key store and test double.
pub mod raw;

pub use policy::{KeyPlan, KeyPolicyEngine};
pub use raw::{RawKeyProvider, StaticKeySource};

/// 128-bit KeyID (KID).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyID(pub Uuid);

impl KeyID {
    /// Construct a KeyID wrapping a known UUID.
    pub fn new(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Generate a random version-4 UUID KeyID.
    pub fn random() -> Self {
        Self(Uuid::new_v4())
    }

    /// Return the raw 16-byte representation.
    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }

    /// Return the 32-character lowercase hex string without hyphens.
    pub fn to_hex(&self) -> String {
        self.0.simple().to_string()
    }
}

/// AES-128 Content Key with associated KeyID and metadata.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentKey {
    /// Associated KeyID.
    pub kid: KeyID,
    /// 128-bit raw AES encryption key bytes.
    pub key: [u8; 16],
    /// Target quality tier bound to this key.
    pub quality_tier: QualityTier,
    /// Elementary track type (video or audio) bound to this key.
    pub track_type: TrackType,
    /// Optional explicit 128-bit initialization vector.
    pub iv: Option<[u8; 16]>,
    /// Optional concrete encryption scheme (CENC or CBCS).
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
    /// Construct a new ContentKey without explicit IV or scheme.
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

    /// Construct a new ContentKey bound to a specific encryption scheme.
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

    /// Set an explicit 128-bit IV for this key.
    pub fn with_iv(mut self, iv: [u8; 16]) -> Self {
        self.iv = Some(iv);
        self
    }

    /// Set an explicit encryption scheme for this key.
    pub fn with_encryption_scheme(mut self, scheme: EncryptionScheme) -> Self {
        self.encryption_scheme = Some(scheme);
        self
    }
}

/// PSSH (Protection System Specific Header) box data for a specific DRM system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PsshData {
    /// Target DRM system.
    pub drm_system: DrmSystem,
    /// 16-byte UUID identifying the DRM system.
    pub system_id: [u8; 16],
    /// Binary PSSH box payload bytes.
    pub data: Bytes,
    /// Optional KeyID bound to this PSSH box.
    pub kid: Option<KeyID>,
    /// Optional encryption scheme bound to this PSSH box.
    pub encryption_scheme: Option<EncryptionScheme>,
}

impl PsshData {
    /// Construct PSSH box data for a target DRM system.
    pub fn new(drm_system: DrmSystem, system_id: [u8; 16], data: Bytes) -> Self {
        Self {
            drm_system,
            system_id,
            data,
            kid: None,
            encryption_scheme: None,
        }
    }

    /// Associate a specific KeyID with this PSSH box.
    pub fn with_kid(mut self, kid: KeyID) -> Self {
        self.kid = Some(kid);
        self
    }

    /// Associate a specific encryption scheme with this PSSH box.
    pub fn with_encryption_scheme(mut self, scheme: EncryptionScheme) -> Self {
        self.encryption_scheme = Some(scheme);
        self
    }
}

/// Description of keys requested from a KeyProvider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRequest {
    /// Content or asset identifier string.
    pub content_id: String,
    /// Requested combinations of track types and quality tiers.
    pub requested_quality_tiers: Vec<(TrackType, QualityTier)>,
    /// Target DRM systems requiring PSSH boxes.
    pub drm_systems: Vec<DrmSystem>,
    /// Target encryption schemes.
    pub encryption_schemes: Vec<EncryptionScheme>,
}

impl KeyRequest {
    /// Create a new key request for a content ID.
    pub fn new(content_id: impl Into<String>) -> Self {
        Self {
            content_id: content_id.into(),
            requested_quality_tiers: Vec::new(),
            drm_systems: Vec::new(),
            encryption_schemes: Vec::new(),
        }
    }

    /// Request a key for a given track type and quality tier.
    pub fn with_quality_tier(mut self, track_type: TrackType, tier: QualityTier) -> Self {
        self.requested_quality_tiers.push((track_type, tier));
        self
    }

    /// Alias for backwards compatibility with earlier code.
    pub fn with_tier(self, track_type: TrackType, tier: QualityTier) -> Self {
        self.with_quality_tier(track_type, tier)
    }

    /// Add a target DRM system to the request.
    pub fn with_drm_system(mut self, drm: DrmSystem) -> Self {
        self.drm_systems.push(drm);
        self
    }

    /// Add a target encryption scheme to the request.
    pub fn with_encryption_scheme(mut self, scheme: EncryptionScheme) -> Self {
        self.encryption_schemes.push(scheme);
        self
    }

    /// Return deduplicated concrete encryption schemes for this request.
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
    /// Map of keys indexed by scheme, track type, and quality tier.
    pub keys: HashMap<(Option<EncryptionScheme>, TrackType, QualityTier), ContentKey>,
    /// Collected PSSH box descriptors for manifest signaling.
    pub pssh: Vec<PsshData>,
}

impl KeySet {
    /// Construct an empty key set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a ContentKey into the set.
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

    /// Retrieve a key matching the track type and quality tier regardless of scheme.
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
            .or_else(|| self.lookup_audio_compat_fallback(None, track_type, quality_tier))
    }

    /// Retrieve a key matching an explicit encryption scheme, track type, and quality tier.
    pub fn get_key_for_scheme(
        &self,
        scheme: EncryptionScheme,
        track_type: TrackType,
        quality_tier: &QualityTier,
    ) -> Option<&ContentKey> {
        self.keys
            .get(&(Some(scheme), track_type, quality_tier.clone()))
            .or_else(|| self.keys.get(&(None, track_type, quality_tier.clone())))
            .or_else(|| self.lookup_audio_compat_fallback(Some(scheme), track_type, quality_tier))
    }

    /// Audio backwards-compatibility fallback: try `AUDIO↔SD` alternate tier when an audio key
    /// isn't found under the requested tier. Returns `None` for non-audio track types.
    fn lookup_audio_compat_fallback(
        &self,
        scheme: Option<EncryptionScheme>,
        track_type: TrackType,
        quality_tier: &QualityTier,
    ) -> Option<&ContentKey> {
        if track_type != TrackType::Audio {
            return None;
        }
        quality_tier.audio_compat_fallback().and_then(|alt| {
            self.keys
                .get(&(scheme, track_type, alt.clone()))
                .or_else(|| self.keys.get(&(None, track_type, alt.clone())))
                .or_else(|| {
                    // Linear scan: the key may be stored under a different scheme variant.
                    self.keys
                        .iter()
                        .find(|((_, t, q), _)| *t == track_type && q == &alt)
                        .map(|(_, k)| k)
                })
        })
    }

    /// Add a PSSH box to the set if not already present.
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

    /// Iterate over all stored ContentKeys.
    pub fn all_keys(&self) -> impl Iterator<Item = &ContentKey> {
        self.keys.values()
    }

    /// Check whether the key set contains no keys.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Return the total number of keys in the set.
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

    /// Generate a signed Axinom JWT entitlement token using an [`AxinomSigningConfig`](crate::vendor::axinom::AxinomSigningConfig).
    pub fn generate_axinom_jwt_with_config(
        &self,
        signing_config: &crate::vendor::axinom::AxinomSigningConfig,
    ) -> Result<String> {
        signing_config.generate_jwt_for_keyset(self)
    }
}

/// Pluggable trait for DRM key acquisition.
pub trait KeyProvider: Send + Sync {
    /// Fetch keys matching the provided key request.
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
