use crate::error::{DrmpackError, Result};
use crate::key::{KeyID, KeyRequest};
use crate::types::{DrmSystem, EncryptionScheme, QualityTier, TrackType};
use std::fmt::Write;
use uuid::Uuid;

/// Description of a key specification generated for a CPIX request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpixKeySpec {
    pub kid: KeyID,
    pub scheme: EncryptionScheme,
    pub track_type: TrackType,
    pub quality_tier: QualityTier,
}

/// Builder for constructing DASH-IF CPIX 2.3 XML request documents.
pub struct CpixRequestBuilder;

impl CpixRequestBuilder {
    /// Build a CPIX 2.3 XML request document from a KeyRequest.
    pub fn build(request: &KeyRequest) -> Result<String> {
        Self::build_with_specs(request).map(|(xml, _)| xml)
    }

    /// Build a CPIX 2.3 XML request document and return the associated key specifications.
    pub fn build_with_specs(request: &KeyRequest) -> Result<(String, Vec<CpixKeySpec>)> {
        if request.content_id.trim().is_empty() {
            return Err(DrmpackError::InvalidConfig(
                "KeyRequest content_id cannot be empty".into(),
            ));
        }

        if request.requested_quality_tiers.is_empty() {
            return Err(DrmpackError::InvalidConfig(
                "KeyRequest requires at least one requested track and QualityTier".into(),
            ));
        }

        let schemes: Vec<EncryptionScheme> = if request.encryption_schemes.is_empty() {
            vec![EncryptionScheme::Cenc]
        } else {
            let mut unique_schemes = Vec::new();
            for concrete in request.concrete_schemes().into_iter().flatten() {
                if !unique_schemes.contains(&concrete) {
                    unique_schemes.push(concrete);
                }
            }
            unique_schemes
        };

        let mut unique_tiers = Vec::new();
        for tier in &request.requested_quality_tiers {
            if !unique_tiers.contains(tier) {
                unique_tiers.push(tier.clone());
            }
        }

        let mut specs = Vec::new();
        for &scheme in &schemes {
            for (track_type, tier) in &unique_tiers {
                specs.push(CpixKeySpec {
                    kid: KeyID::random(),
                    scheme,
                    track_type: *track_type,
                    quality_tier: tier.clone(),
                });
            }
        }

        let drm_systems = if request.drm_systems.is_empty() {
            vec![DrmSystem::Widevine]
        } else {
            let mut unique_drm = Vec::new();
            for drm in &request.drm_systems {
                if !unique_drm.contains(drm) {
                    unique_drm.push(*drm);
                }
            }
            unique_drm
        };

        let mut xml = String::with_capacity(2048);
        writeln!(xml, r#"<?xml version="1.0" encoding="UTF-8"?>"#).unwrap();
        writeln!(
            xml,
            r#"<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc" version="2.3" contentId="{}">"#,
            quick_xml::escape::escape(&request.content_id)
        )
        .unwrap();

        // 1. ContentKeyList
        writeln!(xml, "  <cpix:ContentKeyList>").unwrap();
        for spec in &specs {
            writeln!(
                xml,
                r#"    <cpix:ContentKey kid="{}" commonEncryptionScheme="{}"/>"#,
                spec.kid.0.hyphenated(),
                spec.scheme
            )
            .unwrap();
        }
        writeln!(xml, "  </cpix:ContentKeyList>").unwrap();

        // 2. DRMSystemList
        writeln!(xml, "  <cpix:DRMSystemList>").unwrap();
        for spec in &specs {
            for drm in &drm_systems {
                // FairPlay does not support CENC mode
                if spec.scheme == EncryptionScheme::Cenc && *drm == DrmSystem::FairPlay {
                    continue;
                }
                let system_id = Uuid::from_bytes(drm.system_id()).hyphenated().to_string();
                writeln!(
                    xml,
                    r#"    <cpix:DRMSystem kid="{}" systemId="{}">"#,
                    spec.kid.0.hyphenated(),
                    system_id
                )
                .unwrap();
                if *drm == DrmSystem::FairPlay {
                    writeln!(xml, "      <cpix:URIExtXKey/>").unwrap();
                } else {
                    writeln!(xml, "      <cpix:PSSH/>").unwrap();
                }
                writeln!(xml, "    </cpix:DRMSystem>").unwrap();
            }
        }
        writeln!(xml, "  </cpix:DRMSystemList>").unwrap();

        // 3. ContentKeyUsageRuleList
        writeln!(xml, "  <cpix:ContentKeyUsageRuleList>").unwrap();
        for spec in &specs {
            let intended_track_type = match spec.track_type {
                TrackType::Video => spec.quality_tier.0.clone(),
                TrackType::Audio => format!("AUDIO_{}", spec.quality_tier),
                TrackType::Subtitle => "SUBTITLE".to_string(),
            };

            writeln!(
                xml,
                r#"    <cpix:ContentKeyUsageRule kid="{}" intendedTrackType="{}">"#,
                spec.kid.0.hyphenated(),
                quick_xml::escape::escape(&intended_track_type)
            )
            .unwrap();

            match spec.track_type {
                TrackType::Video => {
                    writeln!(xml, "      <cpix:VideoFilter/>").unwrap();
                }
                TrackType::Audio => {
                    writeln!(xml, "      <cpix:AudioFilter/>").unwrap();
                }
                TrackType::Subtitle => {}
            }

            writeln!(xml, "    </cpix:ContentKeyUsageRule>").unwrap();
        }
        writeln!(xml, "  </cpix:ContentKeyUsageRuleList>").unwrap();

        writeln!(xml, "</cpix:CPIX>").unwrap();
        Ok((xml, specs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpix_request_builder_cenc() {
        let req = KeyRequest {
            content_id: "test-asset-1".into(),
            requested_quality_tiers: vec![(TrackType::Video, QualityTier::hd())],
            drm_systems: vec![DrmSystem::Widevine, DrmSystem::FairPlay],
            encryption_schemes: vec![EncryptionScheme::Cenc],
        };

        let (xml, specs) = CpixRequestBuilder::build_with_specs(&req).unwrap();

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].scheme, EncryptionScheme::Cenc);
        assert_eq!(specs[0].track_type, TrackType::Video);
        assert_eq!(specs[0].quality_tier, QualityTier::hd());

        assert!(xml.contains(r#"contentId="test-asset-1""#));
        assert!(xml.contains(r#"xmlns:cpix="urn:dashif:org:cpix""#));
        assert!(xml.contains(r#"xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc""#));
        assert!(xml.contains(r#"commonEncryptionScheme="cenc""#));
        // FairPlay must not be generated for CENC keys
        assert!(!xml.contains("94ce86fb-07ff-4f43-adb8-93d2fa968ca2"));
        // Widevine must be present
        assert!(xml.contains("edef8ba9-79d6-4ace-a3c8-27dcd51d21ed"));
        // Video filter and usage rule
        assert!(xml.contains("<cpix:VideoFilter/>"));
        assert!(xml.contains(r#"intendedTrackType="HD""#));
    }

    #[test]
    fn test_cpix_request_builder_dual() {
        let req = KeyRequest {
            content_id: "dual-asset".into(),
            requested_quality_tiers: vec![
                (TrackType::Video, QualityTier::sd()),
                (TrackType::Video, QualityTier::hd()),
            ],
            drm_systems: vec![DrmSystem::Widevine, DrmSystem::FairPlay],
            encryption_schemes: vec![EncryptionScheme::Cenc, EncryptionScheme::Cbcs],
        };

        let (xml, specs) = CpixRequestBuilder::build_with_specs(&req).unwrap();

        // 2 schemes * 2 tiers = 4 key specs
        assert_eq!(specs.len(), 4);

        assert!(xml.contains(r#"commonEncryptionScheme="cenc""#));
        assert!(xml.contains(r#"commonEncryptionScheme="cbcs""#));
        // FairPlay must be included for CBCS
        assert!(xml.contains("94ce86fb-07ff-4f43-adb8-93d2fa968ca2"));
        // Both SD and HD usage rules present
        assert!(xml.contains(r#"intendedTrackType="SD""#));
        assert!(xml.contains(r#"intendedTrackType="HD""#));
    }

    #[test]
    fn test_cpix_request_builder_dual_scheme_expansion() {
        let req = KeyRequest::new("dual-auto-expand")
            .with_tier(TrackType::Video, QualityTier::hd())
            .with_drm_system(DrmSystem::Widevine)
            .with_encryption_scheme(EncryptionScheme::Dual);

        let (xml, specs) = CpixRequestBuilder::build_with_specs(&req).unwrap();
        // Dual must expand to Cenc and Cbcs (2 specs)
        assert_eq!(specs.len(), 2);
        assert!(specs.iter().any(|s| s.scheme == EncryptionScheme::Cenc));
        assert!(specs.iter().any(|s| s.scheme == EncryptionScheme::Cbcs));

        assert!(xml.contains(r#"commonEncryptionScheme="cenc""#));
        assert!(xml.contains(r#"commonEncryptionScheme="cbcs""#));
        assert!(
            !xml.contains(r#"commonEncryptionScheme="dual""#),
            "commonEncryptionScheme='dual' must never be produced"
        );
    }

    #[test]
    fn test_cpix_request_builder_empty_content_id_rejected() {
        let req = KeyRequest::new("   ").with_tier(TrackType::Video, QualityTier::hd());
        let result = CpixRequestBuilder::build(&req);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("content_id cannot be empty"));
    }

    #[test]
    fn test_cpix_request_builder_deduplicates_tiers_and_drm() {
        let req = KeyRequest::new("dedup-test")
            .with_tier(TrackType::Video, QualityTier::hd())
            .with_tier(TrackType::Video, QualityTier::hd())
            .with_drm_system(DrmSystem::Widevine)
            .with_drm_system(DrmSystem::Widevine)
            .with_encryption_scheme(EncryptionScheme::Cenc);

        let (_, specs) = CpixRequestBuilder::build_with_specs(&req).unwrap();
        assert_eq!(specs.len(), 1, "Duplicate tiers must be deduplicated");
    }
}
