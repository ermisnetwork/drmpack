use crate::cpix::builder::CpixKeySpec;
use crate::error::{DrmpackError, Result};
use crate::key::{ContentKey, KeyID, KeySet, PsshData};
use crate::types::{DrmSystem, EncryptionScheme, QualityTier, TrackType};
use base64::prelude::*;
use bytes::Bytes;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Default)]
struct RawContentKey {
    kid: Option<KeyID>,
    scheme: Option<EncryptionScheme>,
    key_bytes: Option<[u8; 16]>,
    iv: Option<[u8; 16]>,
    has_encrypted_value: bool,
}

#[derive(Debug, Default)]
struct RawDrmSystem {
    kid: Option<KeyID>,
    system_id: Option<[u8; 16]>,
    drm_system: Option<DrmSystem>,
    pssh_bytes: Option<Bytes>,
    hls_signaling: Option<String>,
}

#[derive(Debug, Default)]
struct RawUsageRule {
    kid: Option<KeyID>,
    intended_track_type: Option<String>,
    track_type: Option<TrackType>,
}

#[inline]
fn local_to_str<'a>(name: quick_xml::name::LocalName<'a>) -> &'a str {
    std::str::from_utf8(name.into_inner()).unwrap_or("")
}

/// Parser for DASH-IF CPIX 2.3 XML response documents.
pub struct CpixResponseParser;

impl CpixResponseParser {
    /// Parse a CPIX 2.3 XML response document into a scheme-aware KeySet.
    /// Optionally accepts key specifications from the originating KeyRequest to
    /// resolve track types and quality tiers when usage rules are absent.
    pub fn parse(xml: &str, specs: Option<&[CpixKeySpec]>) -> Result<KeySet> {
        let clean_xml = xml.trim_start_matches('\u{feff}').trim();
        let mut reader = Reader::from_str(clean_xml);
        reader.config_mut().trim_text(true);

        let mut content_keys: Vec<RawContentKey> = Vec::new();
        let mut drm_systems: Vec<RawDrmSystem> = Vec::new();
        let mut usage_rules: Vec<RawUsageRule> = Vec::new();

        let mut current_content_key: Option<RawContentKey> = None;
        let mut current_drm_system: Option<RawDrmSystem> = None;
        let mut current_usage_rule: Option<RawUsageRule> = None;

        let mut reading_plain_value = false;
        let mut reading_pssh = false;
        let mut reading_hls_signaling = false;

        let mut text_buffer = String::new();
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let local = local_to_str(e.local_name());
                    match local {
                        "ContentKey" => {
                            let mut key = RawContentKey::default();
                            for attr in e.attributes().flatten() {
                                let key_name = local_to_str(attr.key.local_name());
                                if key_name.eq_ignore_ascii_case("kid") {
                                    if let Ok(val) = attr.unescape_value() {
                                        key.kid = parse_uuid(&val).map(KeyID::new);
                                    }
                                } else if key_name.eq_ignore_ascii_case("commonEncryptionScheme")
                                    || key_name.eq_ignore_ascii_case("common_encryption_scheme")
                                {
                                    if let Ok(val) = attr.unescape_value() {
                                        key.scheme = parse_encryption_scheme(&val);
                                    }
                                } else if key_name.eq_ignore_ascii_case("explicitIV") {
                                    if let Ok(val) = attr.unescape_value() {
                                        key.iv = parse_iv(&val);
                                    }
                                }
                            }
                            current_content_key = Some(key);
                        }
                        "PlainValue" => {
                            reading_plain_value = true;
                            text_buffer.clear();
                        }
                        "EncryptedValue" => {
                            if let Some(key) = current_content_key.as_mut() {
                                key.has_encrypted_value = true;
                            }
                        }
                        "DRMSystem" => {
                            let mut drm = RawDrmSystem::default();
                            for attr in e.attributes().flatten() {
                                let key_name = local_to_str(attr.key.local_name());
                                if key_name.eq_ignore_ascii_case("kid") {
                                    if let Ok(val) = attr.unescape_value() {
                                        drm.kid = parse_uuid(&val).map(KeyID::new);
                                    }
                                } else if key_name.eq_ignore_ascii_case("systemId")
                                    || key_name.eq_ignore_ascii_case("system_id")
                                {
                                    if let Ok(val) = attr.unescape_value() {
                                        if let Some(uuid) = parse_uuid(&val) {
                                            let bytes = *uuid.as_bytes();
                                            drm.system_id = Some(bytes);
                                            drm.drm_system = identify_drm_system(&bytes);
                                        }
                                    }
                                }
                            }
                            current_drm_system = Some(drm);
                        }
                        "PSSH" => {
                            reading_pssh = true;
                            text_buffer.clear();
                        }
                        "URIExtXKey" | "HLSSignalingData" => {
                            reading_hls_signaling = true;
                            text_buffer.clear();
                        }
                        "ContentKeyUsageRule" => {
                            let mut rule = RawUsageRule::default();
                            for attr in e.attributes().flatten() {
                                let key_name = local_to_str(attr.key.local_name());
                                if key_name.eq_ignore_ascii_case("kid") {
                                    if let Ok(val) = attr.unescape_value() {
                                        rule.kid = parse_uuid(&val).map(KeyID::new);
                                    }
                                } else if key_name.eq_ignore_ascii_case("intendedTrackType") {
                                    if let Ok(val) = attr.unescape_value() {
                                        rule.intended_track_type = Some(val.into_owned());
                                    }
                                }
                            }
                            current_usage_rule = Some(rule);
                        }
                        "VideoFilter" => {
                            if let Some(rule) = current_usage_rule.as_mut() {
                                rule.track_type = Some(TrackType::Video);
                            }
                        }
                        "AudioFilter" => {
                            if let Some(rule) = current_usage_rule.as_mut() {
                                rule.track_type = Some(TrackType::Audio);
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    let local = local_to_str(e.local_name());
                    match local {
                        "ContentKey" => {
                            let mut key = RawContentKey::default();
                            for attr in e.attributes().flatten() {
                                let key_name = local_to_str(attr.key.local_name());
                                if key_name.eq_ignore_ascii_case("kid") {
                                    if let Ok(val) = attr.unescape_value() {
                                        key.kid = parse_uuid(&val).map(KeyID::new);
                                    }
                                } else if key_name.eq_ignore_ascii_case("commonEncryptionScheme")
                                    || key_name.eq_ignore_ascii_case("common_encryption_scheme")
                                {
                                    if let Ok(val) = attr.unescape_value() {
                                        key.scheme = parse_encryption_scheme(&val);
                                    }
                                } else if key_name.eq_ignore_ascii_case("explicitIV") {
                                    if let Ok(val) = attr.unescape_value() {
                                        key.iv = parse_iv(&val);
                                    }
                                }
                            }
                            content_keys.push(key);
                        }
                        "EncryptedValue" => {
                            if let Some(key) = current_content_key.as_mut() {
                                key.has_encrypted_value = true;
                            }
                        }
                        "VideoFilter" => {
                            if let Some(rule) = current_usage_rule.as_mut() {
                                rule.track_type = Some(TrackType::Video);
                            }
                        }
                        "AudioFilter" => {
                            if let Some(rule) = current_usage_rule.as_mut() {
                                rule.track_type = Some(TrackType::Audio);
                            }
                        }
                        "ContentKeyUsageRule" => {
                            let mut rule = RawUsageRule::default();
                            for attr in e.attributes().flatten() {
                                let key_name = local_to_str(attr.key.local_name());
                                if key_name.eq_ignore_ascii_case("kid") {
                                    if let Ok(val) = attr.unescape_value() {
                                        rule.kid = parse_uuid(&val).map(KeyID::new);
                                    }
                                } else if key_name.eq_ignore_ascii_case("intendedTrackType") {
                                    if let Ok(val) = attr.unescape_value() {
                                        rule.intended_track_type = Some(val.into_owned());
                                    }
                                }
                            }
                            usage_rules.push(rule);
                        }
                        "DRMSystem" => {
                            let mut drm = RawDrmSystem::default();
                            for attr in e.attributes().flatten() {
                                let key_name = local_to_str(attr.key.local_name());
                                if key_name.eq_ignore_ascii_case("kid") {
                                    if let Ok(val) = attr.unescape_value() {
                                        drm.kid = parse_uuid(&val).map(KeyID::new);
                                    }
                                } else if key_name.eq_ignore_ascii_case("systemId") {
                                    if let Ok(val) = attr.unescape_value() {
                                        if let Some(uuid) = parse_uuid(&val) {
                                            let bytes = *uuid.as_bytes();
                                            drm.system_id = Some(bytes);
                                            drm.drm_system = identify_drm_system(&bytes);
                                        }
                                    }
                                }
                            }
                            drm_systems.push(drm);
                        }
                        _ => {}
                    }
                }
                Ok(Event::Text(ref e)) => {
                    if reading_plain_value || reading_pssh || reading_hls_signaling {
                        if let Ok(txt) = e.unescape() {
                            text_buffer.push_str(&txt);
                        }
                    }
                }
                Ok(Event::CData(ref e)) => {
                    if reading_plain_value || reading_pssh || reading_hls_signaling {
                        if let Ok(txt) = std::str::from_utf8(e.as_ref()) {
                            text_buffer.push_str(txt);
                        }
                    }
                }
                Ok(Event::End(ref e)) => {
                    let local = local_to_str(e.local_name());
                    match local {
                        "PlainValue" => {
                            reading_plain_value = false;
                            if let Some(key) = current_content_key.as_mut() {
                                let clean: String =
                                    text_buffer.chars().filter(|c| !c.is_whitespace()).collect();
                                let decoded = BASE64_STANDARD.decode(&clean).map_err(|err| {
                                    DrmpackError::KeyProvider(format!(
                                        "Invalid base64 in PlainValue: {err}"
                                    ))
                                })?;
                                let key_16: [u8; 16] =
                                    decoded.try_into().map_err(|v: Vec<u8>| {
                                        DrmpackError::KeyProvider(format!(
                                            "Expected 16-byte AES key in PlainValue, got {} bytes",
                                            v.len()
                                        ))
                                    })?;
                                key.key_bytes = Some(key_16);
                            }
                            text_buffer.clear();
                        }
                        "PSSH" => {
                            reading_pssh = false;
                            if let Some(drm) = current_drm_system.as_mut() {
                                let clean: String =
                                    text_buffer.chars().filter(|c| !c.is_whitespace()).collect();
                                if let Ok(decoded) = BASE64_STANDARD.decode(&clean) {
                                    let payload = extract_pssh_payload(&decoded);
                                    drm.pssh_bytes = Some(payload);
                                }
                            }
                            text_buffer.clear();
                        }
                        "URIExtXKey" | "HLSSignalingData" => {
                            reading_hls_signaling = false;
                            if let Some(drm) = current_drm_system.as_mut() {
                                let val = text_buffer.trim().to_string();
                                let decoded_val = if !val.starts_with("skd://")
                                    && !val.starts_with("#EXT-X-KEY")
                                {
                                    let clean: String =
                                        val.chars().filter(|c| !c.is_whitespace()).collect();
                                    if let Ok(bytes) = BASE64_STANDARD.decode(&clean) {
                                        if let Ok(s) = String::from_utf8(bytes) {
                                            if s.starts_with("skd://")
                                                || s.starts_with("#EXT-X-KEY")
                                            {
                                                s
                                            } else {
                                                val
                                            }
                                        } else {
                                            val
                                        }
                                    } else {
                                        val
                                    }
                                } else {
                                    val
                                };
                                drm.hls_signaling = Some(decoded_val);
                            }
                            text_buffer.clear();
                        }
                        "ContentKey" => {
                            if let Some(key) = current_content_key.take() {
                                content_keys.push(key);
                            }
                        }
                        "DRMSystem" => {
                            if let Some(drm) = current_drm_system.take() {
                                drm_systems.push(drm);
                            }
                        }
                        "ContentKeyUsageRule" => {
                            if let Some(rule) = current_usage_rule.take() {
                                usage_rules.push(rule);
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Eof) => break,
                Err(err) => {
                    return Err(DrmpackError::KeyProvider(format!(
                        "CPIX XML syntax error: {err}"
                    )))
                }
                _ => {}
            }
            buf.clear();
        }

        if content_keys.is_empty() {
            return Err(DrmpackError::KeyProvider(
                "CPIX response contains no ContentKey elements".into(),
            ));
        }

        let mut usage_rule_by_kid: HashMap<KeyID, &RawUsageRule> = HashMap::new();
        for rule in &usage_rules {
            if let Some(kid) = rule.kid {
                usage_rule_by_kid.insert(kid, rule);
            }
        }

        let mut spec_by_kid: HashMap<KeyID, &CpixKeySpec> = HashMap::new();
        if let Some(specs) = specs {
            for spec in specs {
                spec_by_kid.insert(spec.kid, spec);
            }
        }

        let num_content_keys = content_keys.len();
        let mut key_set = KeySet::new();

        for raw_key in content_keys {
            let kid = raw_key.kid.ok_or_else(|| {
                DrmpackError::KeyProvider("ContentKey missing required 'kid' attribute".into())
            })?;
            let key = match (raw_key.key_bytes, raw_key.has_encrypted_value) {
                (Some(k), _) => k,
                (None, true) => {
                    return Err(DrmpackError::KeyProvider(format!(
                        "ContentKey '{}' contains encrypted key material (<pskc:EncryptedValue>); encrypted PSKC envelopes are not supported",
                        kid.0.hyphenated()
                    )));
                }
                (None, false) => {
                    return Err(DrmpackError::KeyProvider(format!(
                        "ContentKey '{}' missing '<pskc:PlainValue>' key material",
                        kid.0.hyphenated()
                    )));
                }
            };

            let matching_rule = usage_rule_by_kid.get(&kid).copied();
            let matching_spec = spec_by_kid.get(&kid).copied().or_else(|| {
                if let Some(specs) = specs {
                    if specs.len() == 1 && num_content_keys == 1 {
                        specs.first()
                    } else if let Some(scheme) = raw_key.scheme {
                        let matches: Vec<_> = specs.iter().filter(|s| s.scheme == scheme).collect();
                        if matches.len() == 1 {
                            Some(matches[0])
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            });

            let scheme = raw_key.scheme.or_else(|| matching_spec.map(|s| s.scheme));

            let track_type = matching_spec
                .map(|s| s.track_type)
                .or_else(|| matching_rule.and_then(|r| r.track_type))
                .or_else(|| {
                    matching_rule
                        .and_then(|r| r.intended_track_type.as_deref())
                        .map(infer_track_type)
                })
                .unwrap_or(TrackType::Video);

            let quality_tier = matching_spec
                .map(|s| s.quality_tier.clone())
                .or_else(|| {
                    matching_rule
                        .and_then(|r| r.intended_track_type.as_deref())
                        .map(infer_quality_tier)
                })
                .unwrap_or_else(QualityTier::hd);

            let content_key = ContentKey {
                kid,
                key,
                quality_tier,
                track_type,
                iv: raw_key.iv,
                encryption_scheme: scheme,
            };

            key_set.insert_key(content_key);
        }

        for drm in drm_systems {
            let Some(system_id) = drm.system_id else {
                continue;
            };
            let Some(drm_system) = drm.drm_system.or_else(|| identify_drm_system(&system_id))
            else {
                // Ignore unknown or unsupported DRM systems
                continue;
            };

            let data = if let Some(pssh) = drm.pssh_bytes {
                pssh
            } else if let Some(hls_sig) = drm.hls_signaling {
                Bytes::copy_from_slice(hls_sig.as_bytes())
            } else if drm_system == DrmSystem::FairPlay {
                Bytes::new()
            } else {
                continue;
            };

            let scheme = drm.kid.and_then(|k| {
                spec_by_kid.get(&k).map(|s| s.scheme).or_else(|| {
                    key_set
                        .keys
                        .values()
                        .find(|ck| ck.kid == k)
                        .and_then(|ck| ck.encryption_scheme)
                })
            });

            key_set.add_pssh(PsshData {
                drm_system,
                system_id,
                data,
                kid: drm.kid,
                encryption_scheme: scheme,
            });
        }

        Ok(key_set)
    }
}

fn parse_uuid(s: &str) -> Option<Uuid> {
    let trimmed = s.trim();
    let clean = if trimmed.len() >= 9 && trimmed[..9].eq_ignore_ascii_case("urn:uuid:") {
        &trimmed[9..]
    } else {
        trimmed
    };
    Uuid::parse_str(clean).ok()
}

fn parse_encryption_scheme(s: &str) -> Option<EncryptionScheme> {
    match s.trim().to_lowercase().as_str() {
        "cenc" => Some(EncryptionScheme::Cenc),
        "cbcs" => Some(EncryptionScheme::Cbcs),
        _ => None,
    }
}

fn parse_iv(s: &str) -> Option<[u8; 16]> {
    let clean = s.trim();
    let clean = if let Some(stripped) = clean
        .strip_prefix("0x")
        .or_else(|| clean.strip_prefix("0X"))
    {
        stripped
    } else {
        clean
    };
    if clean.len() == 32 {
        u128::from_str_radix(clean, 16).ok().map(u128::to_be_bytes)
    } else {
        let base64_clean: String = clean.chars().filter(|c| !c.is_whitespace()).collect();
        BASE64_STANDARD
            .decode(&base64_clean)
            .ok()
            .and_then(|v| v.try_into().ok())
    }
}

fn identify_drm_system(system_id: &[u8; 16]) -> Option<DrmSystem> {
    if *system_id == DrmSystem::Widevine.system_id() {
        Some(DrmSystem::Widevine)
    } else if *system_id == DrmSystem::FairPlay.system_id() {
        Some(DrmSystem::FairPlay)
    } else if *system_id == DrmSystem::PlayReady.system_id() {
        Some(DrmSystem::PlayReady)
    } else {
        None
    }
}

fn infer_track_type(intended: &str) -> TrackType {
    let upper = intended.to_uppercase();
    if upper.contains("AUDIO") {
        TrackType::Audio
    } else if upper.contains("SUBTITLE") || upper.contains("SUBS") || upper.contains("TEXT") {
        TrackType::Subtitle
    } else {
        TrackType::Video
    }
}

fn infer_quality_tier(intended: &str) -> QualityTier {
    let trimmed = intended.trim();
    if trimmed.is_empty() {
        return QualityTier::hd();
    }
    let upper = trimmed.to_uppercase();
    if upper.starts_with("AUDIO_") {
        return infer_quality_tier(&trimmed["AUDIO_".len()..]);
    }
    if upper.starts_with("VIDEO_") {
        return infer_quality_tier(&trimmed["VIDEO_".len()..]);
    }
    match upper.as_str() {
        "4K" | "UHD" => QualityTier::uhd_4k(),
        "HD" => QualityTier::hd(),
        "SD" | "AUDIO" => QualityTier::sd(),
        _ => QualityTier::new(trimmed),
    }
}

fn extract_pssh_payload(bytes: &[u8]) -> Bytes {
    if bytes.len() >= 8 && &bytes[4..8] == b"pssh" {
        let version = bytes.get(8).copied().unwrap_or(0);
        if version == 0 {
            if bytes.len() >= 32 {
                let data_size =
                    u32::from_be_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]) as usize;
                if 32 + data_size <= bytes.len() {
                    return Bytes::copy_from_slice(&bytes[32..32 + data_size]);
                } else if bytes.len() > 32 {
                    return Bytes::copy_from_slice(&bytes[32..]);
                }
            }
        } else if version == 1 && bytes.len() >= 32 {
            let kid_count =
                u32::from_be_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]) as usize;
            if let Some(offset) = 16usize
                .checked_mul(kid_count)
                .and_then(|o| 32usize.checked_add(o))
            {
                if offset + 4 <= bytes.len() {
                    let data_size = u32::from_be_bytes([
                        bytes[offset],
                        bytes[offset + 1],
                        bytes[offset + 2],
                        bytes[offset + 3],
                    ]) as usize;
                    if offset + 4 + data_size <= bytes.len() {
                        return Bytes::copy_from_slice(&bytes[offset + 4..offset + 4 + data_size]);
                    } else if bytes.len() > offset + 4 {
                        return Bytes::copy_from_slice(&bytes[offset + 4..]);
                    }
                }
            }
        }
    }
    Bytes::copy_from_slice(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CPIX_RESPONSE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc" contentId="test-content">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="99999999-9999-9999-3333-300000000003" commonEncryptionScheme="cenc">
      <cpix:Data>
        <pskc:Secret>
          <pskc:PlainValue>NWX+FHyzgWfc6oCmVFR9PQ==</pskc:PlainValue>
        </pskc:Secret>
      </cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
  <cpix:DRMSystemList>
    <cpix:DRMSystem kid="99999999-9999-9999-3333-300000000003" systemId="edef8ba9-79d6-4ace-a3c8-27dcd51d21ed">
      <cpix:PSSH>AAAAUHBzc2gAAAAA7e+LqXnWSs6jyCfc1R0h7QAAADASEJmZmZmZmZmZMzMwAAAAAAMaBU5hZ3JhIg1UZXN0X0tSX0luZGV4OAFI88aJmwY=</cpix:PSSH>
    </cpix:DRMSystem>
    <cpix:DRMSystem kid="99999999-9999-9999-3333-300000000003" systemId="94ce86fb-07ff-4f43-adb8-93d2fa968ca2">
      <cpix:URIExtXKey>skd://custom-key-uri</cpix:URIExtXKey>
    </cpix:DRMSystem>
  </cpix:DRMSystemList>
  <cpix:ContentKeyUsageRuleList>
    <cpix:ContentKeyUsageRule kid="99999999-9999-9999-3333-300000000003" intendedTrackType="HD">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
  </cpix:ContentKeyUsageRuleList>
</cpix:CPIX>"#;

    #[test]
    fn test_parse_sample_cpix_response() {
        let keyset = CpixResponseParser::parse(SAMPLE_CPIX_RESPONSE, None).unwrap();
        assert_eq!(keyset.len(), 1);

        let key = keyset
            .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
            .expect("Expected HD Video CENC key");

        let expected_kid = Uuid::parse_str("99999999-9999-9999-3333-300000000003").unwrap();
        assert_eq!(key.kid.0, expected_kid);
        assert_eq!(
            key.key,
            BASE64_STANDARD
                .decode("NWX+FHyzgWfc6oCmVFR9PQ==")
                .unwrap()
                .as_slice()
        );
        assert_eq!(key.encryption_scheme, Some(EncryptionScheme::Cenc));

        // Widevine and FairPlay PSSH
        assert_eq!(keyset.pssh.len(), 2);
        let widevine = keyset
            .pssh
            .iter()
            .find(|p| p.drm_system == DrmSystem::Widevine)
            .unwrap();
        assert!(!widevine.data.is_empty());

        let fairplay = keyset
            .pssh
            .iter()
            .find(|p| p.drm_system == DrmSystem::FairPlay)
            .unwrap();
        assert_eq!(fairplay.data.as_ref(), b"skd://custom-key-uri");
    }

    #[test]
    fn test_parse_multi_key_dual_response() {
        let multi_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<CPIX xmlns="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc">
  <ContentKeyList>
    <ContentKey kid="11111111-1111-1111-1111-111111111111" commonEncryptionScheme="cenc">
      <Data><pskc:Secret><pskc:PlainValue>AAAAAAAAAAAAAAAAAAAAAA==</pskc:PlainValue></pskc:Secret></Data>
    </ContentKey>
    <ContentKey kid="22222222-2222-2222-2222-222222222222" commonEncryptionScheme="cbcs">
      <Data><pskc:Secret><pskc:PlainValue>BBBBBBBBBBBBBBBBBBBBBA==</pskc:PlainValue></pskc:Secret></Data>
    </ContentKey>
  </ContentKeyList>
  <ContentKeyUsageRuleList>
    <ContentKeyUsageRule kid="11111111-1111-1111-1111-111111111111" intendedTrackType="HD">
      <VideoFilter/>
    </ContentKeyUsageRule>
    <ContentKeyUsageRule kid="22222222-2222-2222-2222-222222222222" intendedTrackType="HD">
      <VideoFilter/>
    </ContentKeyUsageRule>
  </ContentKeyUsageRuleList>
</CPIX>"#;

        let keyset = CpixResponseParser::parse(multi_xml, None).unwrap();
        assert_eq!(keyset.len(), 2);

        let cenc_key = keyset
            .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
            .unwrap();
        let cbcs_key = keyset
            .get_key_for_scheme(EncryptionScheme::Cbcs, TrackType::Video, &QualityTier::hd())
            .unwrap();

        assert_eq!(
            cenc_key.kid.0,
            Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()
        );
        assert_eq!(cenc_key.encryption_scheme, Some(EncryptionScheme::Cenc));

        assert_eq!(
            cbcs_key.kid.0,
            Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap()
        );
        assert_eq!(cbcs_key.encryption_scheme, Some(EncryptionScheme::Cbcs));
    }

    #[test]
    fn test_parse_invalid_xml_error() {
        let result = CpixResponseParser::parse("<CPIX><broken attr=", None);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("CPIX XML syntax error"));
    }

    #[test]
    fn test_parse_missing_plain_value_error() {
        let xml = r#"<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix">
            <cpix:ContentKeyList>
                <cpix:ContentKey kid="11111111-1111-1111-1111-111111111111"/>
            </cpix:ContentKeyList>
        </cpix:CPIX>"#;
        let result = CpixResponseParser::parse(xml, None);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing '<pskc:PlainValue>'"));
    }

    #[test]
    fn test_parse_encrypted_value_error() {
        let xml = r#"<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc">
            <cpix:ContentKeyList>
                <cpix:ContentKey kid="11111111-1111-1111-1111-111111111111">
                    <cpix:Data>
                        <pskc:Secret>
                            <pskc:EncryptedValue>
                                <xenc:CipherData xmlns:xenc="http://www.w3.org/2001/04/xmlenc#">
                                    <xenc:CipherValue>DEADBEEF==</xenc:CipherValue>
                                </xenc:CipherData>
                            </pskc:EncryptedValue>
                        </pskc:Secret>
                    </cpix:Data>
                </cpix:ContentKey>
            </cpix:ContentKeyList>
        </cpix:CPIX>"#;
        let result = CpixResponseParser::parse(xml, None);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("<pskc:EncryptedValue>")
                && err_msg.contains("encrypted PSKC envelopes are not supported"),
            "Error was: {err_msg}"
        );
    }

    #[test]
    fn test_parse_cdata_blocks() {
        let xml_cdata = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="99999999-9999-9999-3333-300000000003" commonEncryptionScheme="cenc">
      <cpix:Data>
        <pskc:Secret>
          <pskc:PlainValue><![CDATA[NWX+FHyzgWfc6oCmVFR9PQ==]]></pskc:PlainValue>
        </pskc:Secret>
      </cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
  <cpix:DRMSystemList>
    <cpix:DRMSystem kid="99999999-9999-9999-3333-300000000003" systemId="edef8ba9-79d6-4ace-a3c8-27dcd51d21ed">
      <cpix:PSSH><![CDATA[AAAAUHBzc2gAAAAA7e+LqXnWSs6jyCfc1R0h7QAAADASEJmZmZmZmZmZMzMwAAAAAAMaBU5hZ3JhIg1UZXN0X0tSX0luZGV4OAFI88aJmwY=]]></cpix:PSSH>
    </cpix:DRMSystem>
    <cpix:DRMSystem kid="99999999-9999-9999-3333-300000000003" systemId="94ce86fb-07ff-4f43-adb8-93d2fa968ca2">
      <cpix:URIExtXKey><![CDATA[skd://cdata-fairplay-uri]]></cpix:URIExtXKey>
    </cpix:DRMSystem>
  </cpix:DRMSystemList>
  <cpix:ContentKeyUsageRuleList>
    <cpix:ContentKeyUsageRule kid="99999999-9999-9999-3333-300000000003" intendedTrackType="HD">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
  </cpix:ContentKeyUsageRuleList>
</cpix:CPIX>"#;

        let keyset = CpixResponseParser::parse(xml_cdata, None).expect("CDATA parse must succeed");
        assert_eq!(keyset.len(), 1);

        let key = keyset
            .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
            .expect("CENC HD key must be present");
        assert_eq!(
            key.key,
            BASE64_STANDARD
                .decode("NWX+FHyzgWfc6oCmVFR9PQ==")
                .unwrap()
                .as_slice()
        );

        let fairplay = keyset
            .pssh
            .iter()
            .find(|p| p.drm_system == DrmSystem::FairPlay)
            .expect("FairPlay must be present");
        assert_eq!(fairplay.data.as_ref(), b"skd://cdata-fairplay-uri");
    }

    #[test]
    fn test_parse_multiline_whitespace_base64() {
        let xml_formatted = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="99999999-9999-9999-3333-300000000003" commonEncryptionScheme="cenc">
      <cpix:Data>
        <pskc:Secret>
          <pskc:PlainValue>
            NWX+
            FHyzgWfc6oCmVFR9
            PQ==
          </pskc:PlainValue>
        </pskc:Secret>
      </cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
</cpix:CPIX>"#;

        let keyset = CpixResponseParser::parse(xml_formatted, None)
            .expect("Multi-line base64 parse must succeed");
        assert_eq!(keyset.len(), 1);
        let key = keyset.all_keys().next().unwrap();
        assert_eq!(
            key.key,
            BASE64_STANDARD
                .decode("NWX+FHyzgWfc6oCmVFR9PQ==")
                .unwrap()
                .as_slice()
        );
    }

    #[test]
    fn test_infer_quality_tier_uhd_vs_hd_coexistence() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="11111111-1111-1111-1111-111111111111" commonEncryptionScheme="cenc">
      <cpix:Data><pskc:Secret><pskc:PlainValue>AQEBAQEBAQEBAQEBAQEBAQ==</pskc:PlainValue></pskc:Secret></cpix:Data>
    </cpix:ContentKey>
    <cpix:ContentKey kid="22222222-2222-2222-2222-222222222222" commonEncryptionScheme="cenc">
      <cpix:Data><pskc:Secret><pskc:PlainValue>AgICAgICAgICAgICAgICAg==</pskc:PlainValue></pskc:Secret></cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
  <cpix:ContentKeyUsageRuleList>
    <cpix:ContentKeyUsageRule kid="11111111-1111-1111-1111-111111111111" intendedTrackType="HD">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
    <cpix:ContentKeyUsageRule kid="22222222-2222-2222-2222-222222222222" intendedTrackType="UHD">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
  </cpix:ContentKeyUsageRuleList>
</cpix:CPIX>"#;

        let keyset = CpixResponseParser::parse(xml, None).unwrap();
        assert_eq!(
            keyset.len(),
            2,
            "Both HD and UHD keys must be present without collision"
        );

        let hd_key = keyset
            .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
            .expect("HD key must be found");
        let uhd_key = keyset
            .get_key_for_scheme(
                EncryptionScheme::Cenc,
                TrackType::Video,
                &QualityTier::uhd_4k(),
            )
            .expect("UHD key must be resolved to uhd_4k tier, not collided with HD");

        assert_eq!(
            hd_key.kid.0,
            Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()
        );
        assert_eq!(
            uhd_key.kid.0,
            Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap()
        );
    }

    #[test]
    fn test_parse_utf8_bom() {
        let xml_with_bom = format!("\u{feff}{}", SAMPLE_CPIX_RESPONSE);
        let keyset =
            CpixResponseParser::parse(&xml_with_bom, None).expect("BOM parse must succeed");
        assert_eq!(keyset.len(), 1);
    }

    #[test]
    fn test_parse_base64_encoded_hls_signaling() {
        // "skd://custom-key-uri" base64 encoded is "c2tkOi8vY3VzdG9tLWtleS11cmk="
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="99999999-9999-9999-3333-300000000003" commonEncryptionScheme="cbcs">
      <cpix:Data><pskc:Secret><pskc:PlainValue>NWX+FHyzgWfc6oCmVFR9PQ==</pskc:PlainValue></pskc:Secret></cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
  <cpix:DRMSystemList>
    <cpix:DRMSystem kid="99999999-9999-9999-3333-300000000003" systemId="94ce86fb-07ff-4f43-adb8-93d2fa968ca2">
      <cpix:HLSSignalingData>c2tkOi8vY3VzdG9tLWtleS11cmk=</cpix:HLSSignalingData>
    </cpix:DRMSystem>
  </cpix:DRMSystemList>
</cpix:CPIX>"#;

        let keyset = CpixResponseParser::parse(xml, None).unwrap();
        let fairplay = keyset
            .pssh
            .iter()
            .find(|p| p.drm_system == DrmSystem::FairPlay)
            .unwrap();
        assert_eq!(fairplay.data.as_ref(), b"skd://custom-key-uri");
    }

    #[test]
    fn test_parse_unsupported_drm_system_ignored() {
        // ClearKey UUID: 1077efec-c0b2-4d02-ace3-3c1e52e2fb4b
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="99999999-9999-9999-3333-300000000003" commonEncryptionScheme="cenc">
      <cpix:Data><pskc:Secret><pskc:PlainValue>NWX+FHyzgWfc6oCmVFR9PQ==</pskc:PlainValue></pskc:Secret></cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
  <cpix:DRMSystemList>
    <cpix:DRMSystem kid="99999999-9999-9999-3333-300000000003" systemId="1077efec-c0b2-4d02-ace3-3c1e52e2fb4b">
      <cpix:PSSH>AAAAUHBzc2g...</cpix:PSSH>
    </cpix:DRMSystem>
  </cpix:DRMSystemList>
</cpix:CPIX>"#;

        let keyset = CpixResponseParser::parse(xml, None).unwrap();
        // ClearKey must be ignored and NOT coerced to PlayReady
        assert!(
            keyset.pssh.is_empty(),
            "Unsupported DRM system must be skipped"
        );
    }

    #[test]
    fn test_infer_quality_tier_sd_and_audio() {
        assert_eq!(infer_quality_tier("SD"), QualityTier::sd());
        assert_eq!(infer_quality_tier("AUDIO"), QualityTier::sd());
        assert_eq!(infer_quality_tier("audio"), QualityTier::sd());
        assert_eq!(infer_quality_tier("video_sd"), QualityTier::sd());
        assert_eq!(infer_quality_tier("AUDIO_SD"), QualityTier::sd());
    }

    #[test]
    fn test_infer_quality_tier_preserves_custom_tier_names() {
        assert_eq!(infer_quality_tier("FHD"), QualityTier::new("FHD"));
        assert_eq!(infer_quality_tier("1080p_HD"), QualityTier::new("1080p_HD"));
        assert_eq!(infer_quality_tier("720p_HD"), QualityTier::new("720p_HD"));
    }

    #[test]
    fn test_infer_quality_tier_edge_cases() {
        // Leading/trailing whitespace
        assert_eq!(infer_quality_tier("  FHD  "), QualityTier::new("FHD"));
        assert_eq!(infer_quality_tier("  HD  "), QualityTier::hd());
        assert_eq!(
            infer_quality_tier("\tVIDEO_1080p_HD\n"),
            QualityTier::new("1080p_HD")
        );
        assert_eq!(infer_quality_tier("  VIDEO_HD  "), QualityTier::hd());
        // Empty or whitespace-only inputs fallback to HD
        assert_eq!(infer_quality_tier(""), QualityTier::hd());
        assert_eq!(infer_quality_tier("   "), QualityTier::hd());
        assert_eq!(infer_quality_tier("VIDEO_"), QualityTier::hd());
        assert_eq!(infer_quality_tier("AUDIO_"), QualityTier::hd());
    }

    #[test]
    fn test_parse_self_closing_usage_rule() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="11111111-1111-1111-1111-111111111111" commonEncryptionScheme="cenc">
      <cpix:Data><pskc:Secret><pskc:PlainValue>AQEBAQEBAQEBAQEBAQEBAQ==</pskc:PlainValue></pskc:Secret></cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
  <cpix:ContentKeyUsageRuleList>
    <cpix:ContentKeyUsageRule kid="11111111-1111-1111-1111-111111111111" intendedTrackType="HD"/>
  </cpix:ContentKeyUsageRuleList>
</cpix:CPIX>"#;

        let keyset = CpixResponseParser::parse(xml, None).unwrap();
        assert_eq!(keyset.len(), 1);
        let key = keyset
            .get_key(TrackType::Video, &QualityTier::hd())
            .unwrap();
        assert_eq!(
            key.kid.0,
            Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()
        );
    }

    #[test]
    fn test_parse_playready_pssh() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="99999999-9999-9999-3333-300000000003" commonEncryptionScheme="cenc">
      <cpix:Data><pskc:Secret><pskc:PlainValue>NWX+FHyzgWfc6oCmVFR9PQ==</pskc:PlainValue></pskc:Secret></cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
  <cpix:DRMSystemList>
    <cpix:DRMSystem kid="99999999-9999-9999-3333-300000000003" systemId="9a04f079-9840-4286-ab92-e65be0885f95">
      <cpix:PSSH>AAAAUHBzc2gAAAAAmgT/eZioQG6rkubN5l/5lQAAAEAAAAAFAFAAUgBPAEYASQBMAEUA</cpix:PSSH>
    </cpix:DRMSystem>
  </cpix:DRMSystemList>
</cpix:CPIX>"#;

        let keyset = CpixResponseParser::parse(xml, None).unwrap();
        let playready = keyset
            .pssh
            .iter()
            .find(|p| p.drm_system == DrmSystem::PlayReady)
            .expect("PlayReady PSSH must be identified");

        assert_eq!(playready.drm_system, DrmSystem::PlayReady);
        assert_eq!(playready.system_id, DrmSystem::PlayReady.system_id());
        assert_eq!(
            playready.kid.unwrap().0,
            Uuid::parse_str("99999999-9999-9999-3333-300000000003").unwrap()
        );
        assert_eq!(playready.encryption_scheme, Some(EncryptionScheme::Cenc));
    }
}
