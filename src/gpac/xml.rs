use crate::error::{DrmpackError, Result};
use crate::key::{ContentKey, KeySet, PsshData};
use crate::types::{EncryptionScheme, QualityTier, TrackType};
use base64::prelude::*;
use std::fmt::Write;

/// Track encryption configuration for GPAC DRM XML.
#[derive(Debug, Clone)]
pub struct GpacTrackConfig {
    pub track_id: u32,
    pub track_type: TrackType,
    pub quality_tier: QualityTier,
}

impl GpacTrackConfig {
    pub fn new(track_id: u32, track_type: TrackType, quality_tier: QualityTier) -> Self {
        Self {
            track_id,
            track_type,
            quality_tier,
        }
    }
}

/// Configuration container for generating GPAC DRM XML.
#[derive(Debug, Clone)]
pub struct GpacDrmConfig {
    pub scheme: EncryptionScheme,
    pub tracks: Vec<GpacTrackConfig>,
}

impl GpacDrmConfig {
    pub fn new(scheme: EncryptionScheme) -> Self {
        Self {
            scheme,
            tracks: Vec::new(),
        }
    }

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
}

/// Generator for GPAC `cecrypt` Common Encryption XML configuration.
#[derive(Debug, Default)]
pub struct GpacDrmXmlGenerator;

impl GpacDrmXmlGenerator {
    /// Generate a valid GPAC Common Encryption XML document string from a KeySet and configuration.
    pub fn generate(key_set: &KeySet, config: &GpacDrmConfig) -> Result<String> {
        let mut xml = String::with_capacity(1024);
        writeln!(xml, r#"<?xml version="1.0" encoding="UTF-8"?>"#).unwrap();
        writeln!(xml, r#"<GPACDRM type="CENC">"#).unwrap();

        for pssh in &key_set.pssh {
            Self::write_drm_info(&mut xml, pssh);
        }
        let hls_info = build_hls_info(&key_set.pssh)?;

        let scheme_str = match config.scheme {
            EncryptionScheme::Cenc => "cenc",
            EncryptionScheme::Cbcs => "cbcs",
            EncryptionScheme::Dual => "cenc", // Default to CENC for the primary branch
        };

        let is_cbcs = config.scheme == EncryptionScheme::Cbcs;

        // If tracks are specified, generate a CrypTrack for each.
        // Otherwise, generate a default CrypTrack matching all tracks.
        if config.tracks.is_empty() {
            // Find any available key in KeySet
            if let Some((_, key)) = key_set.keys.iter().next() {
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
                let key = key_set
                    .get_key(track.track_type, &track.quality_tier)
                    .ok_or_else(|| {
                        DrmpackError::Encryption(format!(
                            "No ContentKey found for track_id={} ({:?} / {})",
                            track.track_id, track.track_type, track.quality_tier
                        ))
                    })?;

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

        let pattern_attrs = if is_cbcs {
            r#" crypt_byte_block="1" skip_byte_block="9""#
        } else {
            ""
        };

        let iv_size = 16;
        let first_iv = if let Some(iv) = key.iv {
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

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        let _ = write!(s, "{:02x}", b);
    }
    s
}

fn build_hls_info(pssh_list: &[PsshData]) -> Result<Option<String>> {
    let Some(pssh) = pssh_list.first() else {
        return Ok(None);
    };

    let pssh_box = build_pssh_box(pssh)?;
    let uri = BASE64_STANDARD.encode(pssh_box);
    Ok(Some(format!(
        r#"URI="data:text/plain;base64,{}",KEYFORMAT="urn:uuid:{}",KEYFORMATVERSIONS="1""#,
        uri,
        format_uuid(&pssh.system_id)
    )))
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

fn format_uuid(bytes: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
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

        let pssh = PsshData {
            drm_system: DrmSystem::Widevine,
            system_id: DrmSystem::Widevine.system_id(),
            data: Bytes::from_static(b"widevine-payload"),
        };
        key_set.add_pssh(pssh);

        let config = GpacDrmConfig::new(EncryptionScheme::Cenc).with_track(
            1,
            TrackType::Video,
            QualityTier::hd(),
        );

        let xml = GpacDrmXmlGenerator::generate(&key_set, &config).expect("XML generation failed");

        assert!(xml.contains(r#"<GPACDRM type="CENC">"#));
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
        let content_key = ContentKey::new(kid, [0x99; 16], QualityTier::sd(), TrackType::Video);

        let mut key_set = KeySet::new();
        key_set.insert_key(content_key);

        let config = GpacDrmConfig::new(EncryptionScheme::Cbcs).with_track(
            1,
            TrackType::Video,
            QualityTier::sd(),
        );

        let xml = GpacDrmXmlGenerator::generate(&key_set, &config).expect("XML generation failed");

        assert!(xml.contains(r#"scheme_type="cbcs""#));
        assert!(xml.contains(r#"crypt_byte_block="1" skip_byte_block="9""#));
    }
}
