use crate::error::{DrmpackError, Result};
use crate::key::{ContentKey, KeyProvider, KeyRequest, KeySet, PsshData};
use crate::types::{EncryptionScheme, QualityTier, TrackType};
use async_trait::async_trait;
use std::collections::HashMap;

/// Static key source supplying manually configured keys for testing and development.
#[derive(Debug, Clone, Default)]
pub struct StaticKeySource {
    keys: HashMap<(Option<EncryptionScheme>, TrackType, QualityTier), ContentKey>,
    pssh: Vec<PsshData>,
}

/// Backwards compatibility alias for StaticKeySource.
pub type RawKeyProvider = StaticKeySource;

impl StaticKeySource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_key(mut self, key: ContentKey) -> Self {
        self.insert_key(key);
        self
    }

    pub fn with_pssh(mut self, pssh: PsshData) -> Self {
        self.pssh.push(pssh);
        self
    }

    pub fn add_key(&mut self, key: ContentKey) {
        self.insert_key(key);
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
}

#[async_trait]
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
                    .or_else(|| {
                        if scheme_opt.is_none() {
                            self.keys
                                .iter()
                                .find(|((_, t, q), _)| *t == *track_type && q == tier)
                                .map(|(_, k)| k)
                        } else {
                            None
                        }
                    });

                if let Some(key) = key {
                    let mut k = key.clone();
                    if k.encryption_scheme.is_none() && scheme_opt.is_some() {
                        k.encryption_scheme = *scheme_opt;
                    }
                    set.insert_key(k);
                } else {
                    let scheme_desc = scheme_opt
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "agnostic".into());
                    return Err(DrmpackError::KeyProvider(format!(
                        "No raw key configured for {:?} / {} (scheme: {})",
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
}
