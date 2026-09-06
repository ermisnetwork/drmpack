use crate::error::{DrmpackError, Result};
use crate::key::{KeyRequest, KeySet};
use crate::types::{
    DrmSystem, EncryptionScheme, KeyMappingPolicy, QualityTier, Rendition, TrackType,
};

/// The execution plan computed by `KeyPolicyEngine::plan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPlan {
    pub policy: KeyMappingPolicy,
    pub request: Option<KeyRequest>,
    pub shared_all_source: Option<(TrackType, QualityTier)>,
    pub shared_video_tier: Option<QualityTier>,
    pub shared_audio_tier: Option<QualityTier>,
}

fn video_tier_rank(rendition: &Rendition) -> u32 {
    let name = rendition.quality_tier.0.to_uppercase();
    if name.contains("8K") {
        4320
    } else if name.contains("4K") || name.contains("UHD") {
        2160
    } else if name.contains("1080") || name.contains("FHD") {
        1080
    } else if name.contains("HD") || name.contains("720") {
        720
    } else if name.contains("SD") || name.contains("480") || name.contains("360") {
        480
    } else {
        0
    }
}

fn audio_tier_rank(rendition: &Rendition) -> u32 {
    let name = rendition.quality_tier.0.to_uppercase();
    if name.contains("HD") || name.contains("HIGH") {
        256
    } else if name.contains("SD") || name.contains("STANDARD") {
        128
    } else {
        64
    }
}

/// Orchestrates key planning and resolution across renditions and mapping policies.
pub struct KeyPolicyEngine;

impl KeyPolicyEngine {
    /// Phase 1: Analyze renditions, policy, and DRM requirements to construct a `KeyPlan`.
    ///
    /// If all renditions are unencrypted (Selective Encryption), `KeyPlan.request` is `None`
    /// to bypass provider key acquisition entirely.
    ///
    /// For `KeyMappingPolicy::SharedAll`, picks the highest video tier (or HD if present);
    /// if audio-only, picks the audio tier.
    pub fn plan(
        content_id: &str,
        renditions: &[Rendition],
        policy: KeyMappingPolicy,
        mode: EncryptionScheme,
        drm_systems: &[DrmSystem],
    ) -> KeyPlan {
        let encrypted_renditions: Vec<&Rendition> =
            renditions.iter().filter(|r| r.encrypted).collect();

        if encrypted_renditions.is_empty() {
            return KeyPlan {
                policy,
                request: None,
                shared_all_source: None,
                shared_video_tier: None,
                shared_audio_tier: None,
            };
        }

        let mut requested_quality_tiers = Vec::new();
        let mut shared_all_source = None;
        let mut shared_video_tier = None;
        let mut shared_audio_tier = None;

        match policy {
            KeyMappingPolicy::SharedAll => {
                let video_renditions: Vec<&Rendition> = encrypted_renditions
                    .iter()
                    .filter(|r| r.track_type == TrackType::Video)
                    .copied()
                    .collect();

                let source = if !video_renditions.is_empty() {
                    // Pick HD if present, otherwise highest video tier
                    if video_renditions
                        .iter()
                        .any(|r| r.quality_tier == QualityTier::hd())
                    {
                        (TrackType::Video, QualityTier::hd())
                    } else {
                        let best = video_renditions
                            .iter()
                            .max_by_key(|r| video_tier_rank(r))
                            .unwrap();
                        (TrackType::Video, best.quality_tier.clone())
                    }
                } else {
                    // Audio-only: pick the audio tier
                    let audio_renditions: Vec<&Rendition> = encrypted_renditions
                        .iter()
                        .filter(|r| r.track_type == TrackType::Audio)
                        .copied()
                        .collect();

                    if !audio_renditions.is_empty() {
                        let best_audio = audio_renditions
                            .iter()
                            .max_by_key(|r| audio_tier_rank(r))
                            .unwrap();
                        (TrackType::Audio, best_audio.quality_tier.clone())
                    } else {
                        let first = encrypted_renditions[0];
                        (first.track_type, first.quality_tier.clone())
                    }
                };

                requested_quality_tiers.push(source.clone());
                shared_all_source = Some(source);
            }
            KeyMappingPolicy::SharedVideoSingleAudio => {
                let video_renditions: Vec<&Rendition> = encrypted_renditions
                    .iter()
                    .filter(|r| r.track_type == TrackType::Video)
                    .copied()
                    .collect();
                let audio_renditions: Vec<&Rendition> = encrypted_renditions
                    .iter()
                    .filter(|r| r.track_type == TrackType::Audio)
                    .copied()
                    .collect();

                if !video_renditions.is_empty() {
                    let v_tier = if video_renditions
                        .iter()
                        .any(|r| r.quality_tier == QualityTier::hd())
                    {
                        QualityTier::hd()
                    } else {
                        let best = video_renditions
                            .iter()
                            .max_by_key(|r| video_tier_rank(r))
                            .unwrap();
                        best.quality_tier.clone()
                    };
                    requested_quality_tiers.push((TrackType::Video, v_tier.clone()));
                    shared_video_tier = Some(v_tier);
                }

                if !audio_renditions.is_empty() {
                    let a_tier = if audio_renditions
                        .iter()
                        .any(|r| r.quality_tier == QualityTier::sd())
                    {
                        QualityTier::sd()
                    } else {
                        let best = audio_renditions
                            .iter()
                            .max_by_key(|r| audio_tier_rank(r))
                            .unwrap();
                        best.quality_tier.clone()
                    };
                    requested_quality_tiers.push((TrackType::Audio, a_tier.clone()));
                    shared_audio_tier = Some(a_tier);
                }
            }
            KeyMappingPolicy::PerTierAndTrack => {
                for rendition in &encrypted_renditions {
                    let pair = (rendition.track_type, rendition.quality_tier.clone());
                    if !requested_quality_tiers.contains(&pair) {
                        requested_quality_tiers.push(pair);
                    }
                }
            }
        }

        let drm = if drm_systems.is_empty() {
            vec![DrmSystem::Widevine]
        } else {
            drm_systems.to_vec()
        };

        let schemes = mode.concrete_schemes().to_vec();

        let mut request = KeyRequest::new(content_id);
        for (track_type, tier) in requested_quality_tiers {
            request = request.with_quality_tier(track_type, tier);
        }
        for d in drm {
            request = request.with_drm_system(d);
        }
        for s in schemes {
            request = request.with_encryption_scheme(s);
        }

        KeyPlan {
            policy,
            request: Some(request),
            shared_all_source,
            shared_video_tier,
            shared_audio_tier,
        }
    }

    /// Phase 2: Resolve fetched keys according to the `KeyPlan` and renditions.
    ///
    /// For `KeyMappingPolicy::SharedAll`, replicates the acquired key across all encrypted renditions.
    pub fn resolve(
        plan: &KeyPlan,
        renditions: &[Rendition],
        mode: EncryptionScheme,
        fetched_keys: KeySet,
    ) -> Result<KeySet> {
        let encrypted_renditions: Vec<&Rendition> =
            renditions.iter().filter(|r| r.encrypted).collect();

        if plan.request.is_none() || encrypted_renditions.is_empty() {
            return Ok(KeySet::new());
        }

        let schemes = mode.concrete_schemes();
        let mut final_set = KeySet::new();

        match plan.policy {
            KeyMappingPolicy::SharedAll => {
                for &scheme in schemes {
                    let source_key =
                        if let Some((ref track_type, ref tier)) = plan.shared_all_source {
                            fetched_keys
                                .get_key_for_scheme(scheme, *track_type, tier)
                                .or_else(|| {
                                    fetched_keys
                                        .keys
                                        .iter()
                                        .find(|((s, _, _), _)| *s == Some(scheme) || s.is_none())
                                        .map(|(_, k)| k)
                                })
                        } else {
                            fetched_keys
                                .keys
                                .iter()
                                .find(|((s, _, _), _)| *s == Some(scheme) || s.is_none())
                                .map(|(_, k)| k)
                        };

                    let Some(key) = source_key else {
                        return Err(DrmpackError::KeyProvider(format!(
                            "No ContentKey available in KeySet for scheme {scheme} to satisfy SharedAll policy"
                        )));
                    };

                    for rendition in &encrypted_renditions {
                        let mut k = key.clone();
                        k.track_type = rendition.track_type;
                        k.quality_tier = rendition.quality_tier.clone();
                        k.encryption_scheme = Some(scheme);
                        final_set.insert_key(k);
                    }
                }
            }
            KeyMappingPolicy::SharedVideoSingleAudio => {
                for &scheme in schemes {
                    let video_source = plan
                        .shared_video_tier
                        .as_ref()
                        .and_then(|tier| {
                            fetched_keys.get_key_for_scheme(scheme, TrackType::Video, tier)
                        })
                        .or_else(|| {
                            fetched_keys.get_key_for_scheme(
                                scheme,
                                TrackType::Video,
                                &QualityTier::hd(),
                            )
                        })
                        .or_else(|| {
                            fetched_keys
                                .keys
                                .iter()
                                .find(|((s, tt, _), _)| {
                                    *tt == TrackType::Video && (*s == Some(scheme) || s.is_none())
                                })
                                .map(|(_, k)| k)
                        });

                    let audio_source = plan
                        .shared_audio_tier
                        .as_ref()
                        .and_then(|tier| {
                            fetched_keys.get_key_for_scheme(scheme, TrackType::Audio, tier)
                        })
                        .or_else(|| {
                            fetched_keys.get_key_for_scheme(
                                scheme,
                                TrackType::Audio,
                                &QualityTier::sd(),
                            )
                        })
                        .or_else(|| {
                            fetched_keys
                                .keys
                                .iter()
                                .find(|((s, tt, _), _)| {
                                    *tt == TrackType::Audio && (*s == Some(scheme) || s.is_none())
                                })
                                .map(|(_, k)| k)
                        });

                    for rendition in &encrypted_renditions {
                        match rendition.track_type {
                            TrackType::Video => {
                                if let Some(key) = video_source {
                                    let mut k = key.clone();
                                    k.quality_tier = rendition.quality_tier.clone();
                                    k.encryption_scheme = Some(scheme);
                                    final_set.insert_key(k);
                                } else {
                                    return Err(DrmpackError::KeyProvider(format!(
                                        "No video ContentKey available for scheme {scheme}"
                                    )));
                                }
                            }
                            TrackType::Audio => {
                                if let Some(key) = audio_source {
                                    let mut k = key.clone();
                                    k.quality_tier = rendition.quality_tier.clone();
                                    k.encryption_scheme = Some(scheme);
                                    final_set.insert_key(k);
                                } else {
                                    return Err(DrmpackError::KeyProvider(format!(
                                        "No audio ContentKey available for scheme {scheme}"
                                    )));
                                }
                            }
                            TrackType::Subtitle => {}
                        }
                    }
                }
            }
            KeyMappingPolicy::PerTierAndTrack => {
                for &scheme in schemes {
                    for rendition in &encrypted_renditions {
                        if let Some(key) = fetched_keys.get_key_for_scheme(
                            scheme,
                            rendition.track_type,
                            &rendition.quality_tier,
                        ) {
                            let mut k = key.clone();
                            if k.encryption_scheme.is_none() {
                                k.encryption_scheme = Some(scheme);
                            }
                            final_set.insert_key(k);
                        } else {
                            return Err(DrmpackError::KeyProvider(format!(
                                "No ContentKey found for scheme {scheme}, track {:?} / {}",
                                rendition.track_type, rendition.quality_tier
                            )));
                        }
                    }
                }
            }
        }

        final_set.pssh = fetched_keys.pssh;
        Ok(final_set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::{ContentKey, KeyID, PsshData};
    use bytes::Bytes;

    fn rendition_hd() -> Rendition {
        Rendition::video_hd()
    }

    fn rendition_4k() -> Rendition {
        Rendition::video_4k()
    }

    fn rendition_sd() -> Rendition {
        Rendition::video(QualityTier::sd())
    }

    fn rendition_audio() -> Rendition {
        Rendition::audio()
    }

    #[test]
    fn test_plan_all_clear_renditions_bypasses_provider() {
        let renditions = vec![rendition_hd().clear(), rendition_audio().clear()];
        let plan = KeyPolicyEngine::plan(
            "test-content",
            &renditions,
            KeyMappingPolicy::SharedAll,
            EncryptionScheme::Cenc,
            &[DrmSystem::Widevine],
        );

        assert!(plan.request.is_none());
        assert!(plan.shared_all_source.is_none());

        let resolved =
            KeyPolicyEngine::resolve(&plan, &renditions, EncryptionScheme::Cenc, KeySet::new())
                .unwrap();
        assert!(resolved.is_empty());
    }

    #[test]
    fn test_plan_shared_all_picks_hd_when_present() {
        // Renditions include 4K, HD, and SD. HD is present, so HD should be picked.
        let renditions = vec![
            rendition_4k(),
            rendition_hd(),
            rendition_sd(),
            rendition_audio(),
        ];
        let plan = KeyPolicyEngine::plan(
            "content1",
            &renditions,
            KeyMappingPolicy::SharedAll,
            EncryptionScheme::Cenc,
            &[DrmSystem::Widevine],
        );

        let req = plan.request.as_ref().expect("request should exist");
        assert_eq!(
            req.requested_quality_tiers,
            vec![(TrackType::Video, QualityTier::hd())]
        );
        assert_eq!(
            plan.shared_all_source,
            Some((TrackType::Video, QualityTier::hd()))
        );

        // Resolve replicates key across all encrypted renditions
        let kid = KeyID::random();
        let mut fetched = KeySet::new();
        fetched.insert_key(ContentKey::new(
            kid,
            [0xaa; 16],
            QualityTier::hd(),
            TrackType::Video,
        ));

        let resolved =
            KeyPolicyEngine::resolve(&plan, &renditions, EncryptionScheme::Cenc, fetched).unwrap();

        assert_eq!(resolved.len(), 4);
        assert_eq!(
            resolved
                .get_key(TrackType::Video, &QualityTier::uhd_4k())
                .unwrap()
                .kid,
            kid
        );
        assert_eq!(
            resolved
                .get_key(TrackType::Video, &QualityTier::hd())
                .unwrap()
                .kid,
            kid
        );
        assert_eq!(
            resolved
                .get_key(TrackType::Video, &QualityTier::sd())
                .unwrap()
                .kid,
            kid
        );
        assert_eq!(
            resolved
                .get_key(TrackType::Audio, &QualityTier::sd())
                .unwrap()
                .kid,
            kid
        );
    }

    #[test]
    fn test_plan_shared_all_picks_highest_video_tier_when_hd_absent() {
        // Only 4K and SD video (no HD)
        let renditions = vec![rendition_4k(), rendition_sd(), rendition_audio()];
        let plan = KeyPolicyEngine::plan(
            "content2",
            &renditions,
            KeyMappingPolicy::SharedAll,
            EncryptionScheme::Cenc,
            &[DrmSystem::Widevine],
        );

        let req = plan.request.as_ref().expect("request should exist");
        assert_eq!(
            req.requested_quality_tiers,
            vec![(TrackType::Video, QualityTier::uhd_4k())]
        );
        assert_eq!(
            plan.shared_all_source,
            Some((TrackType::Video, QualityTier::uhd_4k()))
        );

        let kid = KeyID::random();
        let mut fetched = KeySet::new();
        fetched.insert_key(ContentKey::new(
            kid,
            [0xbb; 16],
            QualityTier::uhd_4k(),
            TrackType::Video,
        ));

        let resolved =
            KeyPolicyEngine::resolve(&plan, &renditions, EncryptionScheme::Cenc, fetched).unwrap();

        assert_eq!(resolved.len(), 3);
        assert_eq!(
            resolved
                .get_key(TrackType::Video, &QualityTier::uhd_4k())
                .unwrap()
                .kid,
            kid
        );
        assert_eq!(
            resolved
                .get_key(TrackType::Video, &QualityTier::sd())
                .unwrap()
                .kid,
            kid
        );
        assert_eq!(
            resolved
                .get_key(TrackType::Audio, &QualityTier::sd())
                .unwrap()
                .kid,
            kid
        );
    }

    #[test]
    fn test_plan_shared_all_audio_only() {
        let audio_low = Rendition::audio_tier(QualityTier::sd());
        let audio_high = Rendition::audio_tier(QualityTier::hd());
        let renditions = vec![audio_low, audio_high];

        let plan = KeyPolicyEngine::plan(
            "audio-only",
            &renditions,
            KeyMappingPolicy::SharedAll,
            EncryptionScheme::Cenc,
            &[DrmSystem::Widevine],
        );

        let req = plan.request.as_ref().expect("request should exist");
        assert_eq!(
            req.requested_quality_tiers,
            vec![(TrackType::Audio, QualityTier::hd())]
        );

        let kid = KeyID::random();
        let mut fetched = KeySet::new();
        fetched.insert_key(ContentKey::new(
            kid,
            [0xcc; 16],
            QualityTier::hd(),
            TrackType::Audio,
        ));

        let resolved =
            KeyPolicyEngine::resolve(&plan, &renditions, EncryptionScheme::Cenc, fetched).unwrap();

        assert_eq!(resolved.len(), 2);
        assert_eq!(
            resolved
                .get_key(TrackType::Audio, &QualityTier::sd())
                .unwrap()
                .kid,
            kid
        );
        assert_eq!(
            resolved
                .get_key(TrackType::Audio, &QualityTier::hd())
                .unwrap()
                .kid,
            kid
        );
    }

    #[test]
    fn test_plan_shared_all_dual_scheme() {
        let renditions = vec![rendition_hd(), rendition_audio()];
        let plan = KeyPolicyEngine::plan(
            "dual-content",
            &renditions,
            KeyMappingPolicy::SharedAll,
            EncryptionScheme::Dual,
            &[DrmSystem::Widevine, DrmSystem::FairPlay],
        );

        let req = plan.request.as_ref().expect("request should exist");
        assert_eq!(
            req.encryption_schemes,
            vec![EncryptionScheme::Cenc, EncryptionScheme::Cbcs]
        );

        let kid_cenc = KeyID::random();
        let kid_cbcs = KeyID::random();
        let mut fetched = KeySet::new();
        fetched.insert_key(ContentKey::new_with_scheme(
            kid_cenc,
            [0x11; 16],
            QualityTier::hd(),
            TrackType::Video,
            EncryptionScheme::Cenc,
        ));
        fetched.insert_key(ContentKey::new_with_scheme(
            kid_cbcs,
            [0x22; 16],
            QualityTier::hd(),
            TrackType::Video,
            EncryptionScheme::Cbcs,
        ));

        let resolved =
            KeyPolicyEngine::resolve(&plan, &renditions, EncryptionScheme::Dual, fetched).unwrap();

        // 2 renditions x 2 schemes = 4 keys
        assert_eq!(resolved.len(), 4);
        assert_eq!(
            resolved
                .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
                .unwrap()
                .kid,
            kid_cenc
        );
        assert_eq!(
            resolved
                .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Audio, &QualityTier::sd())
                .unwrap()
                .kid,
            kid_cenc
        );
        assert_eq!(
            resolved
                .get_key_for_scheme(EncryptionScheme::Cbcs, TrackType::Video, &QualityTier::hd())
                .unwrap()
                .kid,
            kid_cbcs
        );
        assert_eq!(
            resolved
                .get_key_for_scheme(EncryptionScheme::Cbcs, TrackType::Audio, &QualityTier::sd())
                .unwrap()
                .kid,
            kid_cbcs
        );
    }

    #[test]
    fn test_plan_shared_video_single_audio() {
        let renditions = vec![
            rendition_hd(),
            rendition_sd(),
            rendition_audio(),
            Rendition::audio_tier(QualityTier::hd()),
        ];

        let plan = KeyPolicyEngine::plan(
            "content3",
            &renditions,
            KeyMappingPolicy::SharedVideoSingleAudio,
            EncryptionScheme::Cenc,
            &[DrmSystem::Widevine],
        );

        let req = plan.request.as_ref().expect("request should exist");
        assert_eq!(
            req.requested_quality_tiers,
            vec![
                (TrackType::Video, QualityTier::hd()),
                (TrackType::Audio, QualityTier::sd())
            ]
        );

        let kid_v = KeyID::random();
        let kid_a = KeyID::random();
        let mut fetched = KeySet::new();
        fetched.insert_key(ContentKey::new(
            kid_v,
            [0x11; 16],
            QualityTier::hd(),
            TrackType::Video,
        ));
        fetched.insert_key(ContentKey::new(
            kid_a,
            [0x22; 16],
            QualityTier::sd(),
            TrackType::Audio,
        ));

        let resolved =
            KeyPolicyEngine::resolve(&plan, &renditions, EncryptionScheme::Cenc, fetched).unwrap();

        assert_eq!(resolved.len(), 4);
        assert_eq!(
            resolved
                .get_key(TrackType::Video, &QualityTier::hd())
                .unwrap()
                .kid,
            kid_v
        );
        assert_eq!(
            resolved
                .get_key(TrackType::Video, &QualityTier::sd())
                .unwrap()
                .kid,
            kid_v
        );
        assert_eq!(
            resolved
                .get_key(TrackType::Audio, &QualityTier::sd())
                .unwrap()
                .kid,
            kid_a
        );
        assert_eq!(
            resolved
                .get_key(TrackType::Audio, &QualityTier::hd())
                .unwrap()
                .kid,
            kid_a
        );
    }

    #[test]
    fn test_plan_per_tier_and_track() {
        let renditions = vec![rendition_hd(), rendition_sd()];
        let plan = KeyPolicyEngine::plan(
            "content4",
            &renditions,
            KeyMappingPolicy::PerTierAndTrack,
            EncryptionScheme::Cenc,
            &[DrmSystem::Widevine],
        );

        let req = plan.request.as_ref().expect("request should exist");
        assert_eq!(
            req.requested_quality_tiers,
            vec![
                (TrackType::Video, QualityTier::hd()),
                (TrackType::Video, QualityTier::sd())
            ]
        );

        let kid_hd = KeyID::random();
        let kid_sd = KeyID::random();
        let mut fetched = KeySet::new();
        fetched.insert_key(ContentKey::new(
            kid_hd,
            [0x11; 16],
            QualityTier::hd(),
            TrackType::Video,
        ));
        fetched.insert_key(ContentKey::new(
            kid_sd,
            [0x22; 16],
            QualityTier::sd(),
            TrackType::Video,
        ));

        let resolved =
            KeyPolicyEngine::resolve(&plan, &renditions, EncryptionScheme::Cenc, fetched).unwrap();

        assert_eq!(resolved.len(), 2);
        assert_eq!(
            resolved
                .get_key(TrackType::Video, &QualityTier::hd())
                .unwrap()
                .kid,
            kid_hd
        );
        assert_eq!(
            resolved
                .get_key(TrackType::Video, &QualityTier::sd())
                .unwrap()
                .kid,
            kid_sd
        );
    }

    #[test]
    fn test_selective_encryption_mixed_renditions() {
        let renditions = vec![
            rendition_hd(),            // encrypted
            rendition_audio().clear(), // clear
        ];

        let plan = KeyPolicyEngine::plan(
            "mixed",
            &renditions,
            KeyMappingPolicy::SharedAll,
            EncryptionScheme::Cenc,
            &[DrmSystem::Widevine],
        );

        let req = plan.request.as_ref().expect("request should exist");
        assert_eq!(
            req.requested_quality_tiers,
            vec![(TrackType::Video, QualityTier::hd())]
        );

        let kid = KeyID::random();
        let mut fetched = KeySet::new();
        fetched.insert_key(ContentKey::new(
            kid,
            [0x42; 16],
            QualityTier::hd(),
            TrackType::Video,
        ));

        let resolved =
            KeyPolicyEngine::resolve(&plan, &renditions, EncryptionScheme::Cenc, fetched).unwrap();

        // Only the encrypted rendition should receive a key
        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved
                .get_key(TrackType::Video, &QualityTier::hd())
                .unwrap()
                .kid,
            kid
        );
        assert!(resolved
            .get_key(TrackType::Audio, &QualityTier::sd())
            .is_none());
    }

    #[test]
    fn test_resolve_missing_key_returns_error() {
        let renditions = vec![rendition_hd()];
        let plan = KeyPolicyEngine::plan(
            "missing",
            &renditions,
            KeyMappingPolicy::PerTierAndTrack,
            EncryptionScheme::Cenc,
            &[DrmSystem::Widevine],
        );

        // Empty fetched keys
        let empty_fetched = KeySet::new();
        let result =
            KeyPolicyEngine::resolve(&plan, &renditions, EncryptionScheme::Cenc, empty_fetched);

        assert!(matches!(result, Err(DrmpackError::KeyProvider(_))));
    }

    #[test]
    fn test_resolve_preserves_pssh_data() {
        let renditions = vec![rendition_hd()];
        let plan = KeyPolicyEngine::plan(
            "pssh-test",
            &renditions,
            KeyMappingPolicy::SharedAll,
            EncryptionScheme::Cenc,
            &[DrmSystem::Widevine],
        );

        let mut fetched = KeySet::new();
        fetched.insert_key(ContentKey::new(
            KeyID::random(),
            [0x11; 16],
            QualityTier::hd(),
            TrackType::Video,
        ));
        let pssh = PsshData::new(
            DrmSystem::Widevine,
            DrmSystem::Widevine.system_id(),
            Bytes::from_static(b"pssh-bytes"),
        );
        fetched.add_pssh(pssh.clone());

        let resolved =
            KeyPolicyEngine::resolve(&plan, &renditions, EncryptionScheme::Cenc, fetched).unwrap();

        assert_eq!(resolved.pssh.len(), 1);
        assert_eq!(resolved.pssh[0], pssh);
    }

    #[test]
    fn test_resolve_shared_all_dual_missing_scheme_key_fails() {
        let renditions = vec![rendition_hd()];
        let plan = KeyPolicyEngine::plan(
            "dual-missing",
            &renditions,
            KeyMappingPolicy::SharedAll,
            EncryptionScheme::Dual,
            &[DrmSystem::Widevine],
        );

        let mut fetched = KeySet::new();
        // Only CENC key is fetched, no CBCS key
        fetched.insert_key(ContentKey::new_with_scheme(
            KeyID::random(),
            [0x11; 16],
            QualityTier::hd(),
            TrackType::Video,
            EncryptionScheme::Cenc,
        ));

        let result = KeyPolicyEngine::resolve(&plan, &renditions, EncryptionScheme::Dual, fetched);
        assert!(
            result.is_err(),
            "Dual mode with missing CBCS key must return an error"
        );
    }
}
