use crate::error::{DrmpackError, Result};
use crate::key::{ContentKey, KeySet, PsshData};
use crate::types::{DrmSystem, EncryptionScheme, QualityTier, TrackType};
use base64::prelude::*;
use std::fmt::Write;
use uuid::Uuid;

/// Track encryption configuration for GPAC DRM XML.
#[derive(Debug, Clone)]
pub struct GpacTrackConfig {
    /// 1-based ISO-BMFF track ID.
    pub track_id: u32,
    /// Elementary track type (video or audio).
    pub track_type: TrackType,
    /// Quality tier for ContentKey mapping.
    pub quality_tier: QualityTier,
    /// Whether this track is encrypted.
    pub encrypted: bool,
}

impl GpacTrackConfig {
    /// Construct a new track configuration.
    pub fn new(track_id: u32, track_type: TrackType, quality_tier: QualityTier) -> Self {
        Self {
            track_id,
            track_type,
            quality_tier,
            encrypted: true,
        }
    }

    /// Set explicit encryption flag for this track.
    pub fn with_encrypted(mut self, encrypted: bool) -> Self {
        self.encrypted = encrypted;
        self
    }
}

/// Configuration container for generating GPAC DRM XML.
#[derive(Debug, Clone)]
pub struct GpacDrmConfig {
    /// Concrete encryption scheme (CENC or CBCS).
    pub scheme: EncryptionScheme,
    /// List of track configurations included in the DRM XML.
    pub tracks: Vec<GpacTrackConfig>,
}

impl GpacDrmConfig {
    /// Construct a new DRM configuration for an encryption scheme.
    pub fn new(scheme: EncryptionScheme) -> Self {
        Self {
            scheme,
            tracks: Vec::new(),
        }
    }

    /// Add an encrypted track to the configuration.
    pub fn with_track(
        mut self,
        track_id: u32,
        track_type: TrackType,
        quality_tier: QualityTier,
    ) -> Self {
        self.tracks
            .push(GpacTrackConfig::new(track_id, track_type, quality_tier));
        self
    }

    /// Add a clear (unencrypted) track to the configuration.
    pub fn with_clear_track(
        mut self,
        track_id: u32,
        track_type: TrackType,
        quality_tier: QualityTier,
    ) -> Self {
        self.tracks
            .push(GpacTrackConfig::new(track_id, track_type, quality_tier).with_encrypted(false));
        self
    }
}

/// Generator for GPAC `cecrypt` Common Encryption XML configuration.
#[derive(Debug, Default)]
pub struct GpacDrmXmlGenerator;

impl GpacDrmXmlGenerator {
    /// Generate a valid GPAC Common Encryption XML document string from a KeySet and configuration.
    pub fn generate(key_set: &KeySet, config: &GpacDrmConfig) -> Result<String> {
        let mut xml = String::with_capacity(1024);
        writeln!(xml, r#"<?xml version="1.0" encoding="UTF-8"?>"#).unwrap();

        let scheme_str = match config.scheme {
            EncryptionScheme::Cenc => "cenc",
            EncryptionScheme::Cbcs => "cbcs",
            EncryptionScheme::Dual => {
                return Err(DrmpackError::InvalidConfig(
                    "EncryptionScheme::Dual is an orchestration mode and cannot generate GPAC DRM XML"
                        .into(),
                ));
            }
        };

        writeln!(xml, r#"<GPACDRM type="{}">"#, scheme_str).unwrap();

        for pssh in &key_set.pssh {
            // FairPlay strictly uses HLS manifest signaling and must not emit ISO-BMFF PSSH boxes
            if pssh.drm_system == DrmSystem::FairPlay {
                continue;
            }
            if let Some(s) = pssh.encryption_scheme {
                if s != config.scheme {
                    continue;
                }
            }
            Self::write_drm_info(&mut xml, pssh);
        }

        let is_cbcs = config.scheme == EncryptionScheme::Cbcs;

        // If tracks are specified, generate a CrypTrack for each.
        // Otherwise, generate a default CrypTrack matching all tracks.
        if config.tracks.is_empty() {
            // Find any available key in KeySet for this scheme
            let key = key_set
                .keys
                .iter()
                .find(|((scheme, _, _), _)| scheme.is_none_or(|s| s == config.scheme))
                .map(|(_, k)| k)
                .or_else(|| key_set.keys.values().next());
            if let Some(key) = key {
                let hls_info = build_hls_info(&key_set.pssh, key, config.scheme)?;
                Self::write_cryptrack(
                    &mut xml,
                    None,
                    key,
                    scheme_str,
                    is_cbcs,
                    hls_info.as_deref(),
                )?;
            } else {
                return Err(DrmpackError::KeyProvider("KeySet contains no keys".into()));
            }
        } else {
            for track in &config.tracks {
                if track.track_type == TrackType::Subtitle {
                    // Subtitle / text tracks cannot be encrypted with Common Encryption (CENC/CBCS).
                    // Under GPAC cecrypt, any PID without a CrypTrack entry passes through unencrypted.
                    // Emitting a CrypTrack for text PIDs causes GPAC MP4Mux to abort with
                    // "Missing CENC Key config, cannot mux".
                    continue;
                }

                if !track.encrypted {
                    Self::write_clear_track(&mut xml, track.track_id)?;
                    continue;
                }

                let key = key_set
                    .get_key_for_scheme(config.scheme, track.track_type, &track.quality_tier)
                    .ok_or_else(|| {
                        DrmpackError::Encryption(format!(
                            "No ContentKey found for track_id={} ({:?} / {} / {})",
                            track.track_id, track.track_type, track.quality_tier, config.scheme
                        ))
                    })?;

                let hls_info = build_hls_info(&key_set.pssh, key, config.scheme)?;
                Self::write_cryptrack(
                    &mut xml,
                    Some(track.track_id),
                    key,
                    scheme_str,
                    is_cbcs,
                    hls_info.as_deref(),
                )?;
            }
        }

        writeln!(xml, r#"</GPACDRM>"#).unwrap();
        Ok(xml)
    }

    fn write_clear_track(xml: &mut String, track_id: u32) -> Result<()> {
        writeln!(
            xml,
            r#"  <CrypTrack trackID="{}" IsEncrypted="0"/>"#,
            track_id
        )
        .unwrap();
        Ok(())
    }

    fn write_drm_info(xml: &mut String, pssh: &PsshData) {
        let system_id = hex_encode(&pssh.system_id);
        let data = BASE64_STANDARD.encode(&pssh.data);

        writeln!(xml, r#"  <DRMInfo type="pssh" version="0">"#).unwrap();
        writeln!(xml, r#"    <BS ID128="{}"/>"#, system_id).unwrap();
        writeln!(xml, r#"    <BS data64="{}"/>"#, data).unwrap();
        writeln!(xml, r#"  </DRMInfo>"#).unwrap();
    }

    fn write_cryptrack(
        xml: &mut String,
        track_id: Option<u32>,
        key: &ContentKey,
        scheme_str: &str,
        is_cbcs: bool,
        hls_info: Option<&str>,
    ) -> Result<()> {
        let track_attr = match track_id {
            Some(id) => format!(r#" trackID="{}""#, id),
            None => "".to_string(),
        };

        // Apple FairPlay & ISO/IEC 23001-7: Pattern encryption (1:9) applies ONLY to video tracks.
        // Audio tracks in CBCS must not use pattern encryption (0:0 / whole-block).
        let pattern_attrs = if is_cbcs {
            if key.track_type == TrackType::Video {
                r#" crypt_byte_block="1" skip_byte_block="9""#
            } else {
                r#" crypt_byte_block="0" skip_byte_block="0""#
            }
        } else {
            ""
        };

        let iv_size = 16;
        // In CBCS mode, ensure a deterministic 16-byte constant IV is present.
        // If key.iv is None, derive deterministically from the KID bytes.
        let effective_iv = if is_cbcs {
            Some(key.iv.unwrap_or_else(|| *key.kid.as_bytes()))
        } else {
            key.iv
        };

        let first_iv = if let Some(iv) = effective_iv {
            format!(r#" first_IV="0x{}""#, hex_encode(&iv))
        } else {
            "".to_string()
        };

        writeln!(
            xml,
            r#"  <CrypTrack{} IsEncrypted="1" IV_size="{}" scheme_type="{}"{}{}>"#,
            track_attr, iv_size, scheme_str, pattern_attrs, first_iv
        )
        .unwrap();

        // GPAC's schema names the key element in lowercase.
        let kid_hex = format!("0x{}", key.kid.to_hex());
        let val_hex = format!("0x{}", hex_encode(&key.key));
        if let Some(hls_info) = hls_info {
            writeln!(
                xml,
                r#"    <key KID="{}" value="{}" hlsInfo='{}'/>"#,
                kid_hex, val_hex, hls_info
            )
            .unwrap();
        } else {
            writeln!(xml, r#"    <key KID="{}" value="{}"/>"#, kid_hex, val_hex).unwrap();
        }

        writeln!(xml, r#"  </CrypTrack>"#).unwrap();
        Ok(())
    }
}

fn hex_encode(bytes: &[u8; 16]) -> String {
    Uuid::from_bytes(*bytes).simple().to_string()
}

fn build_hls_info(
    pssh_list: &[PsshData],
    key: &ContentKey,
    scheme: EncryptionScheme,
) -> Result<Option<String>> {
    if pssh_list.is_empty() {
        return Ok(None);
    }

    let mut parts = Vec::new();
    let mut has_fairplay = false;
    for pssh in pssh_list {
        // Only include PSSH matching this key's KID (if PSSH has kid bound)
        if let Some(pssh_kid) = &pssh.kid {
            if pssh_kid != &key.kid {
                continue;
            }
        }
        // Only include PSSH matching this scheme (if PSSH has scheme bound)
        if let Some(pssh_scheme) = pssh.encryption_scheme {
            if pssh_scheme != scheme {
                continue;
            }
        }

        if pssh.drm_system == crate::types::DrmSystem::FairPlay {
            // FairPlay does not support CENC mode
            if scheme == EncryptionScheme::Cenc || has_fairplay {
                continue;
            }
            has_fairplay = true;
            let mut skd_uri = if !pssh.data.is_empty() {
                let s = String::from_utf8_lossy(&pssh.data);
                if s.starts_with("skd://") {
                    s.to_string()
                } else if let Some(start) = s.find("URI=\"skd://") {
                    let rem = &s[start + 5..];
                    if let Some(end) = rem.find('"') {
                        rem[..end].to_string()
                    } else {
                        format!("skd://{}", key.kid.0.hyphenated())
                    }
                } else {
                    format!("skd://{}", key.kid.0.hyphenated())
                }
            } else {
                format!("skd://{}", key.kid.0.hyphenated())
            };
            let uri_suffix = skd_uri.strip_prefix("skd://").unwrap_or(&skd_uri);
            if !uri_suffix.contains(':') {
                let iv = key.iv.unwrap_or_else(|| *key.kid.as_bytes());
                let iv_hex = hex_encode(&iv);
                skd_uri = format!("{skd_uri}:{iv_hex}");
            }
            parts.push(format!(
                r#"URI="{}",KEYFORMAT="com.apple.streamingkeydelivery",KEYFORMATVERSIONS="1""#,
                skd_uri
            ));
        } else {
            let pssh_box = build_pssh_box(pssh)?;
            let uri = BASE64_STANDARD.encode(pssh_box);
            let part = format!(
                r#"URI="data:text/plain;base64,{}",KEYFORMAT="urn:uuid:{}",KEYFORMATVERSIONS="1""#,
                uri,
                Uuid::from_bytes(pssh.system_id)
            );
            if !parts.contains(&part) {
                parts.push(part);
            }
        }
    }

    if parts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parts.join(",")))
    }
}

fn build_pssh_box(pssh: &PsshData) -> Result<Vec<u8>> {
    const PSSH_HEADER_SIZE: usize = 32;
    let box_size = PSSH_HEADER_SIZE
        .checked_add(pssh.data.len())
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| DrmpackError::Encryption("PSSH data is too large".into()))?;
    let data_size = u32::try_from(pssh.data.len())
        .map_err(|_| DrmpackError::Encryption("PSSH data is too large".into()))?;

    let mut bytes = Vec::with_capacity(box_size as usize);
    bytes.extend_from_slice(&box_size.to_be_bytes());
    bytes.extend_from_slice(b"pssh");
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&pssh.system_id);
    bytes.extend_from_slice(&data_size.to_be_bytes());
    bytes.extend_from_slice(&pssh.data);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::KeyID;
    use crate::types::DrmSystem;
    use bytes::Bytes;
    use uuid::Uuid;

    #[test]
    fn test_gpac_xml_cenc_generation() {
        let kid_uuid = Uuid::from_bytes([
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10,
        ]);
        let kid = KeyID::new(kid_uuid);
        let key_bytes = [0x42; 16];
        let content_key = ContentKey::new(kid, key_bytes, QualityTier::hd(), TrackType::Video);

        let mut key_set = KeySet::new();
        key_set.insert_key(content_key);

        let pssh = PsshData::new(
            DrmSystem::Widevine,
            DrmSystem::Widevine.system_id(),
            Bytes::from_static(b"widevine-payload"),
        );
        key_set.add_pssh(pssh);

        let config = GpacDrmConfig::new(EncryptionScheme::Cenc).with_track(
            1,
            TrackType::Video,
            QualityTier::hd(),
        );

        let xml = GpacDrmXmlGenerator::generate(&key_set, &config).expect("XML generation failed");

        assert!(xml.contains(r#"<GPACDRM type="cenc">"#));
        assert!(xml.contains(
            "  <DRMInfo type=\"pssh\" version=\"0\">\n\
             \x20   <BS ID128=\"edef8ba979d64acea3c827dcd51d21ed\"/>\n\
             \x20   <BS data64=\"d2lkZXZpbmUtcGF5bG9hZA==\"/>\n\
             \x20 </DRMInfo>\n\
             \x20 <CrypTrack"
        ));
        assert!(xml.contains(
            r#"<CrypTrack trackID="1" IsEncrypted="1" IV_size="16" scheme_type="cenc">"#
        ));
        assert!(xml.contains(r#"<key KID="0x0102030405060708090a0b0c0d0e0f10" value="0x42424242424242424242424242424242""#));
        assert!(xml.contains(r#"hlsInfo='URI="data:text/plain;base64,AAAAMHBzc2gAAAAA7e+LqXnWSs6jyCfc1R0h7QAAABB3aWRldmluZS1wYXlsb2Fk",KEYFORMAT="urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed",KEYFORMATVERSIONS="1"'/>"#));
        assert!(!xml.contains("<Key "));
        assert!(xml.contains(r#"</GPACDRM>"#));
    }

    #[test]
    fn test_gpac_xml_cbcs_pattern_generation() {
        let kid = KeyID::new(Uuid::from_bytes([0x07; 16]));
        let video_key = ContentKey::new(kid, [0x99; 16], QualityTier::sd(), TrackType::Video);
        let audio_key = ContentKey::new(kid, [0x88; 16], QualityTier::sd(), TrackType::Audio);

        let mut key_set = KeySet::new();
        key_set.insert_key(video_key);
        key_set.insert_key(audio_key);

        let config = GpacDrmConfig::new(EncryptionScheme::Cbcs)
            .with_track(1, TrackType::Video, QualityTier::sd())
            .with_track(2, TrackType::Audio, QualityTier::sd());

        let xml = GpacDrmXmlGenerator::generate(&key_set, &config).expect("XML generation failed");

        assert!(xml.contains(r#"<GPACDRM type="cbcs">"#));
        assert!(xml.contains(r#"trackID="1" IsEncrypted="1" IV_size="16" scheme_type="cbcs" crypt_byte_block="1" skip_byte_block="9" first_IV="0x07070707070707070707070707070707""#));
        assert!(xml.contains(r#"trackID="2" IsEncrypted="1" IV_size="16" scheme_type="cbcs" crypt_byte_block="0" skip_byte_block="0" first_IV="0x07070707070707070707070707070707""#));
    }

    #[test]
    fn test_gpac_xml_rejects_dual_orchestration_mode() {
        let kid = KeyID::new(Uuid::from_bytes([0x01; 16]));
        let mut key_set = KeySet::new();
        key_set.insert_key(ContentKey::new(
            kid,
            [0x42; 16],
            QualityTier::hd(),
            TrackType::Video,
        ));

        let error =
            GpacDrmXmlGenerator::generate(&key_set, &GpacDrmConfig::new(EncryptionScheme::Dual))
                .expect_err("Dual must not be accepted as a GPAC encryption scheme");

        assert!(matches!(error, DrmpackError::InvalidConfig(_)));
        assert!(error.to_string().contains("orchestration mode"));
    }

    #[test]
    fn test_gpac_xml_multi_drm_generation() {
        let kid_uuid = Uuid::from_bytes([
            0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
            0x1e, 0x1f,
        ]);
        let kid = KeyID::new(kid_uuid);
        let content_key = ContentKey::new(kid, [0xaa; 16], QualityTier::hd(), TrackType::Video);

        let mut key_set = KeySet::new();
        key_set.insert_key(content_key);

        // 1. Widevine PSSH
        key_set.add_pssh(PsshData::new(
            DrmSystem::Widevine,
            DrmSystem::Widevine.system_id(),
            Bytes::from_static(b"widevine-data"),
        ));

        // 2. PlayReady PSSH
        key_set.add_pssh(PsshData::new(
            DrmSystem::PlayReady,
            DrmSystem::PlayReady.system_id(),
            Bytes::from_static(b"playready-data"),
        ));

        // 3. FairPlay
        key_set.add_pssh(PsshData::new(
            DrmSystem::FairPlay,
            DrmSystem::FairPlay.system_id(),
            Bytes::from_static(b""),
        ));

        let config = GpacDrmConfig::new(EncryptionScheme::Cbcs).with_track(
            1,
            TrackType::Video,
            QualityTier::hd(),
        );

        let xml = GpacDrmXmlGenerator::generate(&key_set, &config).expect("XML generation failed");

        // Verify Widevine and PlayReady have DRMInfo PSSH tags
        assert!(xml.contains(r#"<BS ID128="edef8ba979d64acea3c827dcd51d21ed"/>"#));
        assert!(xml.contains(r#"<BS ID128="9a04f07998404286ab92e65be0885f95"/>"#));
        // Verify FairPlay does NOT emit an ISO-BMFF PSSH tag
        assert!(!xml.contains(r#"<BS ID128="94ce86fb07ff4f43adb893d2fa968ca2"/>"#));

        // Verify hlsInfo includes FairPlay skd URI with IV as well as Widevine and PlayReady UUIDs
        let expected_fairplay = format!(
            r#"URI="skd://{}:{}",KEYFORMAT="com.apple.streamingkeydelivery",KEYFORMATVERSIONS="1""#,
            kid_uuid.hyphenated(),
            hex_encode(kid.as_bytes())
        );
        let expected_widevine = r#"KEYFORMAT="urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed""#;
        let expected_playready = r#"KEYFORMAT="urn:uuid:9a04f079-9840-4286-ab92-e65be0885f95""#;

        assert!(xml.contains(&expected_fairplay));
        assert!(xml.contains(expected_widevine));
        assert!(xml.contains(expected_playready));

        // Verify delimiter ,URI= is present between DRM formats
        assert!(xml.contains(r#",KEYFORMATVERSIONS="1",URI="#));
    }

    #[test]
    fn test_gpac_xml_cenc_excludes_fairplay() {
        let kid_uuid = Uuid::from_bytes([
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10,
        ]);
        let kid = KeyID::new(kid_uuid);
        let content_key = ContentKey::new(kid, [0xaa; 16], QualityTier::hd(), TrackType::Video);

        let mut key_set = KeySet::new();
        key_set.insert_key(content_key);

        // Widevine PSSH
        key_set.add_pssh(PsshData::new(
            DrmSystem::Widevine,
            DrmSystem::Widevine.system_id(),
            Bytes::from_static(b"widevine-data"),
        ));

        // FairPlay PSSH
        key_set.add_pssh(PsshData::new(
            DrmSystem::FairPlay,
            DrmSystem::FairPlay.system_id(),
            Bytes::from_static(b"skd://custom-key-uri"),
        ));

        let config = GpacDrmConfig::new(EncryptionScheme::Cenc).with_track(
            1,
            TrackType::Video,
            QualityTier::hd(),
        );

        let xml = GpacDrmXmlGenerator::generate(&key_set, &config).expect("XML generation failed");

        // Widevine must be present
        assert!(xml.contains(r#"<BS ID128="edef8ba979d64acea3c827dcd51d21ed"/>"#));
        // FairPlay MUST NOT be present in CENC XML
        assert!(
            !xml.contains(r#"<BS ID128="94ce86fb07ff4f43adb893d2fa968ca2"/>"#),
            "CENC GPAC XML must not contain FairPlay DRMInfo"
        );
        assert!(
            !xml.contains("com.apple.streamingkeydelivery"),
            "CENC GPAC XML must not contain FairPlay HLS signaling"
        );
    }

    #[test]
    fn test_gpac_xml_empty_tracks_scheme_matching() {
        let kid_cenc = KeyID::new(Uuid::from_bytes([1; 16]));
        let mut key_cenc =
            ContentKey::new(kid_cenc, [0xaa; 16], QualityTier::hd(), TrackType::Video);
        key_cenc.encryption_scheme = Some(EncryptionScheme::Cenc);

        let kid_cbcs = KeyID::new(Uuid::from_bytes([2; 16]));
        let mut key_cbcs =
            ContentKey::new(kid_cbcs, [0xbb; 16], QualityTier::hd(), TrackType::Video);
        key_cbcs.encryption_scheme = Some(EncryptionScheme::Cbcs);

        let mut key_set = KeySet::new();
        key_set.insert_key(key_cenc);
        key_set.insert_key(key_cbcs);

        let config_cbcs = GpacDrmConfig::new(EncryptionScheme::Cbcs);
        let xml_cbcs =
            GpacDrmXmlGenerator::generate(&key_set, &config_cbcs).expect("XML generation failed");
        assert!(xml_cbcs.contains("02020202020202020202020202020202"));

        let config_cenc = GpacDrmConfig::new(EncryptionScheme::Cenc);
        let xml_cenc =
            GpacDrmXmlGenerator::generate(&key_set, &config_cenc).expect("XML generation failed");
        assert!(xml_cenc.contains("01010101010101010101010101010101"));
    }

    #[test]
    fn test_gpac_xml_selective_encryption_clear_track() {
        let kid = KeyID::new(Uuid::from_bytes([0x01; 16]));
        let key = ContentKey::new(kid, [0xaa; 16], QualityTier::hd(), TrackType::Video);

        let mut key_set = KeySet::new();
        key_set.insert_key(key);
        // Note: key_set intentionally contains NO Audio key!

        let config = GpacDrmConfig::new(EncryptionScheme::Cenc)
            .with_track(1, TrackType::Video, QualityTier::hd())
            .with_clear_track(2, TrackType::Audio, QualityTier::sd());

        let xml =
            GpacDrmXmlGenerator::generate(&key_set, &config).expect("XML generation must succeed");
        assert!(xml.contains(r#"<CrypTrack trackID="1" IsEncrypted="1""#));
        assert!(xml.contains(r#"<CrypTrack trackID="2" IsEncrypted="0"/>"#));
    }

    #[test]
    fn test_gpac_xml_subtitle_track_omitted() {
        let kid = KeyID::new(Uuid::from_bytes([0x01; 16]));
        let key = ContentKey::new(kid, [0xaa; 16], QualityTier::hd(), TrackType::Video);

        let mut key_set = KeySet::new();
        key_set.insert_key(key);

        let config = GpacDrmConfig::new(EncryptionScheme::Cenc)
            .with_track(1, TrackType::Video, QualityTier::hd())
            .with_clear_track(2, TrackType::Subtitle, QualityTier::new("default"));

        let xml =
            GpacDrmXmlGenerator::generate(&key_set, &config).expect("XML generation must succeed");
        assert!(xml.contains(r#"<CrypTrack trackID="1" IsEncrypted="1""#));
        // Subtitle track 2 must be omitted to prevent GPAC cecrypt failure on text stream
        assert!(!xml.contains(r#"trackID="2""#));
    }
}
