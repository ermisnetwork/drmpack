//! Axinom DRM JWT entitlement token generator.
//!
//! Generates signed Axinom JWT license tokens (HS256) containing inline content key configurations.

use crate::error::{DrmpackError, Result};
use crate::key::ContentKey;
use base64::prelude::*;

/// Configuration of a content key to be authorized within an Axinom entitlement token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxinomKeyConfig {
    /// KeyID as hex string or UUID.
    pub kid: String,
    /// Optional 16-byte initialization vector.
    pub iv: Option<[u8; 16]>,
}

impl From<String> for AxinomKeyConfig {
    fn from(kid: String) -> Self {
        Self { kid, iv: None }
    }
}

impl From<&str> for AxinomKeyConfig {
    fn from(kid: &str) -> Self {
        Self {
            kid: kid.to_string(),
            iv: None,
        }
    }
}

impl From<&ContentKey> for AxinomKeyConfig {
    fn from(k: &ContentKey) -> Self {
        let mut cfg = Self::new(k.kid.0.hyphenated().to_string());
        let iv = k.iv.or_else(|| {
            if k.encryption_scheme == Some(crate::types::EncryptionScheme::Cbcs) {
                Some(*k.kid.as_bytes())
            } else {
                None
            }
        });
        if let Some(iv) = iv {
            cfg = cfg.with_iv(iv);
        }
        cfg
    }
}

impl From<ContentKey> for AxinomKeyConfig {
    fn from(k: ContentKey) -> Self {
        Self::from(&k)
    }
}

impl AxinomKeyConfig {
    /// Create a new AxinomKeyConfig for the given Key ID.
    pub fn new(kid: impl Into<String>) -> Self {
        Self {
            kid: kid.into(),
            iv: None,
        }
    }

    /// Attach a 16-byte initialization vector (IV) for FairPlay or custom key mapping.
    pub fn with_iv(mut self, iv: [u8; 16]) -> Self {
        self.iv = Some(iv);
        self
    }
}

/// Generate an Axinom JWT entitlement token signed with HMAC-SHA256.
///
/// # Arguments
/// * `com_key_id` - Axinom Communication Key ID (UUID format)
/// * `com_key_b64` - Base64-encoded Axinom Communication Key (secret)
/// * `keys` - Slice of content keys to authorize in the entitlement message
pub fn generate_axinom_jwt(
    com_key_id: &str,
    com_key_b64: &str,
    keys: &[AxinomKeyConfig],
) -> Result<String> {
    let key_bytes = BASE64_STANDARD.decode(com_key_b64.trim()).map_err(|e| {
        DrmpackError::InvalidConfig(format!("Invalid communication key base64: {e}"))
    })?;

    let header_json = r#"{"alg":"HS256","typ":"JWT"}"#;
    let inline_entries: Vec<serde_json::Value> = keys
        .iter()
        .map(|k| {
            let mut obj = serde_json::json!({ "id": k.kid });
            if let Some(iv) = k.iv {
                obj["iv"] = serde_json::Value::String(BASE64_STANDARD.encode(iv));
            }
            obj
        })
        .collect();

    let payload = serde_json::json!({
        "version": 1,
        "com_key_id": com_key_id,
        "message": {
            "type": "entitlement_message",
            "version": 2,
            "content_keys_source": {
                "inline": inline_entries
            }
        }
    });

    let payload_json = serde_json::to_string(&payload).map_err(|e| {
        DrmpackError::InvalidConfig(format!("Failed to serialize JWT payload: {e}"))
    })?;

    let h_b64 = BASE64_URL_SAFE_NO_PAD.encode(header_json.as_bytes());
    let p_b64 = BASE64_URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
    let signing_input = format!("{h_b64}.{p_b64}");

    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &key_bytes);
    let tag = ring::hmac::sign(&key, signing_input.as_bytes());
    let sig_b64 = BASE64_URL_SAFE_NO_PAD.encode(tag.as_ref());

    Ok(format!("{signing_input}.{sig_b64}"))
}

use std::fmt;

/// Configuration and credentials for signing Axinom DRM JWT entitlement tokens.
#[derive(Clone, PartialEq, Eq)]
pub struct AxinomSigningConfig {
    /// Communication Key ID UUID.
    pub key_id: String,
    secret: String,
}

impl fmt::Debug for AxinomSigningConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AxinomSigningConfig")
            .field("key_id", &self.key_id)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

impl AxinomSigningConfig {
    /// Create new signing credentials.
    pub fn new(key_id: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            key_id: key_id.into(),
            secret: secret.into(),
        }
    }

    /// Load communication key credentials from environment variables:
    /// - `AXINOM_COMMUNICATION_KEY_ID`
    /// - `AXINOM_COMMUNICATION_KEY`
    pub fn from_env() -> Result<Self> {
        let key_id = std::env::var("AXINOM_COMMUNICATION_KEY_ID").map_err(|_| {
            DrmpackError::InvalidConfig(
                "Missing AXINOM_COMMUNICATION_KEY_ID environment variable".to_string(),
            )
        })?;
        let secret = std::env::var("AXINOM_COMMUNICATION_KEY").map_err(|_| {
            DrmpackError::InvalidConfig(
                "Missing AXINOM_COMMUNICATION_KEY environment variable".to_string(),
            )
        })?;
        Ok(Self::new(key_id, secret))
    }

    /// Access the communication key ID.
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// Access the raw communication key secret bytes.
    pub fn secret(&self) -> &str {
        &self.secret
    }

    /// Generate a signed Axinom JWT entitlement token for the given key configs.
    pub fn generate_jwt(&self, keys: &[AxinomKeyConfig]) -> Result<String> {
        generate_axinom_jwt(&self.key_id, &self.secret, keys)
    }

    /// Generate a signed Axinom JWT entitlement token for a DrmStreamMetadata.
    pub fn generate_jwt_for_metadata(
        &self,
        metadata: &crate::session::DrmStreamMetadata,
    ) -> Result<String> {
        let configs = metadata.to_axinom_key_configs();
        self.generate_jwt(&configs)
    }

    /// Generate a signed Axinom JWT entitlement token for a KeySet.
    pub fn generate_jwt_for_keyset(&self, keyset: &crate::key::KeySet) -> Result<String> {
        let configs = keyset.to_axinom_key_configs();
        self.generate_jwt(&configs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_axinom_jwt_structure() {
        let com_key_id = "00000000-0000-0000-0000-000000000000";
        let com_key_b64 = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        let keys = vec![
            AxinomKeyConfig::new("11111111-1111-1111-1111-111111111111").with_iv([0xaa; 16]),
            AxinomKeyConfig::new("22222222-2222-2222-2222-222222222222"),
        ];

        let token = generate_axinom_jwt(com_key_id, com_key_b64, &keys)
            .expect("jwt generation should succeed");

        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT must consist of 3 parts");

        let header_bytes = BASE64_URL_SAFE_NO_PAD
            .decode(parts[0])
            .expect("header base64 decode failed");
        let header: serde_json::Value =
            serde_json::from_slice(&header_bytes).expect("header json parse failed");
        assert_eq!(header["alg"], "HS256");
        assert_eq!(header["typ"], "JWT");

        let payload_bytes = BASE64_URL_SAFE_NO_PAD
            .decode(parts[1])
            .expect("payload base64 decode failed");
        let payload: serde_json::Value =
            serde_json::from_slice(&payload_bytes).expect("payload json parse failed");
        assert_eq!(payload["version"], 1);
        assert_eq!(payload["com_key_id"], com_key_id);
        assert_eq!(payload["message"]["type"], "entitlement_message");

        let inline = payload["message"]["content_keys_source"]["inline"]
            .as_array()
            .unwrap();
        assert_eq!(inline.len(), 2);
        assert_eq!(inline[0]["id"], "11111111-1111-1111-1111-111111111111");
        assert!(inline[0].get("iv").is_some());
        assert_eq!(inline[1]["id"], "22222222-2222-2222-2222-222222222222");
        assert!(inline[1].get("iv").is_none());
    }

    #[test]
    fn test_invalid_com_key_b64() {
        let res = generate_axinom_jwt("id", "invalid-not-base64!?", &[]);
        assert!(res.is_err());
    }

    #[test]
    fn test_axinom_key_config_from_content_key() {
        use crate::key::KeyID;
        use crate::types::{EncryptionScheme, QualityTier, TrackType};
        use uuid::Uuid;

        let uuid = Uuid::parse_str("12345678-1234-5678-1234-567812345678").unwrap();
        let mut ck = ContentKey::new(
            KeyID::new(uuid),
            [0x01; 16],
            QualityTier::hd(),
            TrackType::Video,
        );
        let cfg = AxinomKeyConfig::from(&ck);
        assert_eq!(cfg.kid, "12345678-1234-5678-1234-567812345678");
        assert_eq!(cfg.iv, None);

        // CBCS content key defaults to KID bytes as IV
        ck.encryption_scheme = Some(EncryptionScheme::Cbcs);
        let cfg_cbcs = AxinomKeyConfig::from(&ck);
        assert_eq!(cfg_cbcs.iv, Some(*uuid.as_bytes()));

        // Explicit IV is preserved regardless of scheme
        let custom_iv = [0x99; 16];
        ck.iv = Some(custom_iv);
        let cfg2 = AxinomKeyConfig::from(ck);
        assert_eq!(cfg2.iv, Some(custom_iv));
    }

    #[test]
    fn test_axinom_signing_config_debug_redaction_and_signing() {
        let signing = AxinomSigningConfig::new(
            "00000000-0000-0000-0000-000000000000",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        );

        let debug_str = format!("{signing:?}");
        assert!(debug_str.contains("00000000-0000-0000-0000-000000000000"));
        assert!(debug_str.contains("[REDACTED]"));
        assert!(!debug_str.contains("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="));

        let keys = vec![AxinomKeyConfig::new("11111111-1111-1111-1111-111111111111")];
        let jwt = signing
            .generate_jwt(&keys)
            .expect("JWT signing must succeed");
        assert_eq!(jwt.split('.').count(), 3);
    }
}
