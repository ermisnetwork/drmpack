use crate::error::{DrmpackError, Result};
use crate::key::{ContentKey, KeyID, KeyProvider, KeyRequest, KeySet, PsshData};
use crate::types::{EncryptionScheme, QualityTier, TrackType};
use std::collections::HashMap;

/// In-memory test double and pre-shared key store supplying manually configured
/// ContentKeys and PSSH boxes for unit/E2E testing and offline packaging without network I/O.
#[derive(Clone, Default)]
pub struct StaticKeySource {
    keys: HashMap<(Option<EncryptionScheme>, TrackType, QualityTier), ContentKey>,
    pssh: Vec<PsshData>,
    shared_fallback: Option<(KeyID, [u8; 16])>,
}

impl std::fmt::Debug for StaticKeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticKeySource")
            .field("keys", &self.keys)
            .field("pssh", &self.pssh)
            .field(
                "shared_fallback",
                &self
                    .shared_fallback
                    .as_ref()
                    .map(|(kid, _)| (kid, "[REDACTED]")),
            )
            .finish()
    }
}

/// Legacy alias for [`StaticKeySource`]. Prefer using [`StaticKeySource`].
pub type RawKeyProvider = StaticKeySource;

impl StaticKeySource {
    /// Create an empty static key source.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a test provider with a single shared ContentKey for all tracks/tiers
    /// and default Widevine PSSH box.
    pub fn shared_key(key_bytes: [u8; 16]) -> Self {
        let kid = crate::key::KeyID::new(uuid::Uuid::from_bytes(key_bytes));
        let pssh = PsshData::new(
            crate::types::DrmSystem::Widevine,
            crate::types::DrmSystem::Widevine.system_id(),
            bytes::Bytes::from_static(b"widevine-pssh-payload"),
        );
        let mut source = Self::new()
            .with_key(ContentKey::new(
                kid,
                key_bytes,
                QualityTier::hd(),
                TrackType::Video,
            ))
            .with_key(ContentKey::new(
                kid,
                key_bytes,
                QualityTier::audio(),
                TrackType::Audio,
            ))
            .with_key(ContentKey::new(
                kid,
                key_bytes,
                QualityTier::sd(),
                TrackType::Audio,
            ))
            .with_pssh(pssh);
        source.shared_fallback = Some((kid, key_bytes));
        source
    }

    /// Builder method to register a ContentKey.
    pub fn with_key(mut self, key: ContentKey) -> Self {
        self.insert_key(key);
        self
    }

    /// Builder method to register a PSSH box.
    pub fn with_pssh(mut self, pssh: PsshData) -> Self {
        self.pssh.push(pssh);
        self
    }

    /// Insert a ContentKey into the static key store.
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
}

impl KeyProvider for StaticKeySource {
    async fn fetch_keys(&self, request: &KeyRequest) -> Result<KeySet> {
        let mut set = KeySet::new();
        let schemes = request.concrete_schemes();

        for scheme_opt in &schemes {
            for (track_type, tier) in &request.requested_quality_tiers {
                let key = self
                    .keys
                    .get(&(*scheme_opt, *track_type, tier.clone()))
                    .or_else(|| self.keys.get(&(None, *track_type, tier.clone())))
                    .cloned()
                    .or_else(|| {
                        if *track_type == TrackType::Audio {
                            tier.audio_compat_fallback().and_then(|alt| {
                                self.keys
                                    .get(&(*scheme_opt, *track_type, alt.clone()))
                                    .or_else(|| self.keys.get(&(None, *track_type, alt)))
                                    .cloned()
                            })
                        } else {
                            None
                        }
                    })
                    .or_else(|| {
                        if scheme_opt.is_none() {
                            self.keys
                                .iter()
                                .find(|((_, t, q), _)| *t == *track_type && q == tier)
                                .map(|(_, k)| k.clone())
                        } else {
                            None
                        }
                    })
                    .or_else(|| {
                        self.shared_fallback
                            .map(|(kid, kb)| ContentKey::new(kid, kb, tier.clone(), *track_type))
                    });

                if let Some(mut k) = key {
                    if k.encryption_scheme.is_none() && scheme_opt.is_some() {
                        k.encryption_scheme = *scheme_opt;
                    }
                    k.quality_tier = tier.clone();
                    set.insert_key(k);
                } else {
                    let scheme_desc = scheme_opt
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "agnostic".into());
                    return Err(DrmpackError::KeyProvider(format!(
                        "No static key configured for {:?} / {} (scheme: {})",
                        track_type, tier, scheme_desc
                    )));
                }
            }
        }
        set.pssh = self.pssh.clone();
        Ok(set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::KeyID;
    use crate::types::DrmSystem;

    #[tokio::test]
    async fn test_raw_key_provider_success() {
        let kid = KeyID::random();
        let key_bytes = [1u8; 16];
        let content_key = ContentKey::new(kid, key_bytes, QualityTier::hd(), TrackType::Video);

        let provider = StaticKeySource::new().with_key(content_key.clone());

        let req = KeyRequest {
            content_id: "test-content".into(),
            requested_quality_tiers: vec![(TrackType::Video, QualityTier::hd())],
            drm_systems: vec![DrmSystem::Widevine],
            encryption_schemes: vec![EncryptionScheme::Cenc],
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
        let provider = StaticKeySource::new();
        let req = KeyRequest {
            content_id: "test-content".into(),
            requested_quality_tiers: vec![(TrackType::Video, QualityTier::hd())],
            drm_systems: vec![DrmSystem::Widevine],
            encryption_schemes: vec![EncryptionScheme::Cenc],
        };

        let result = provider.fetch_keys(&req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_raw_key_provider_dual_scheme_expansion() {
        let kid = KeyID::random();
        let provider = StaticKeySource::new().with_key(ContentKey::new(
            kid,
            [0xaa; 16],
            QualityTier::hd(),
            TrackType::Video,
        ));

        let req = KeyRequest::new("dual-content")
            .with_tier(TrackType::Video, QualityTier::hd())
            .with_encryption_scheme(EncryptionScheme::Dual);

        let keyset = provider.fetch_keys(&req).await.unwrap();
        // Both Cenc and Cbcs keys must be resolved
        assert!(keyset
            .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
            .is_some());
        assert!(keyset
            .get_key_for_scheme(EncryptionScheme::Cbcs, TrackType::Video, &QualityTier::hd())
            .is_some());
    }

    #[tokio::test]
    async fn test_raw_key_provider_shared_key() {
        let key_bytes = [0x42; 16];
        let provider = StaticKeySource::shared_key(key_bytes);

        // Test with arbitrary tiers and track types
        let req = KeyRequest::new("stream-42")
            .with_tier(TrackType::Video, QualityTier::uhd_4k())
            .with_tier(TrackType::Video, QualityTier::hd())
            .with_tier(TrackType::Audio, QualityTier::new("custom_audio"))
            .with_encryption_scheme(EncryptionScheme::Cenc);

        let keyset = provider.fetch_keys(&req).await.unwrap();
        assert!(keyset
            .get_key(TrackType::Video, &QualityTier::uhd_4k())
            .is_some());
        assert!(keyset
            .get_key(TrackType::Video, &QualityTier::hd())
            .is_some());
        assert!(keyset
            .get_key(TrackType::Audio, &QualityTier::new("custom_audio"))
            .is_some());
        assert_eq!(keyset.pssh.len(), 1);
        assert_eq!(keyset.pssh[0].drm_system, DrmSystem::Widevine);
    }

    #[test]
    fn test_static_key_source_debug_redaction() {
        let key_bytes = [0x55; 16];
        let provider = StaticKeySource::shared_key(key_bytes);
        let debug_output = format!("{provider:?}");
        assert!(debug_output.contains("[REDACTED]"));
        assert!(!debug_output.contains("85, 85, 85, 85"));
    }
}
