use crate::axinom::config::AxinomConfig;
use crate::cpix::builder::CpixRequestBuilder;
use crate::cpix::parser::CpixResponseParser;
use crate::error::{DrmpackError, Result};
use crate::key::{KeyProvider, KeyRequest, KeySet};
use async_trait::async_trait;
use base64::prelude::*;
use std::fmt;

/// Axinom KeyProvider implementing SPEKE v2 over CPIX 2.3 protocol.
pub struct AxinomProvider {
    config: AxinomConfig,
    client: reqwest::Client,
}

impl fmt::Debug for AxinomProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AxinomProvider")
            .field("config", &self.config)
            .finish()
    }
}

impl AxinomProvider {
    /// Create a new AxinomProvider with given configuration.
    pub fn new(config: AxinomConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { config, client }
    }

    /// Create a new AxinomProvider with custom reqwest client.
    pub fn with_client(config: AxinomConfig, client: reqwest::Client) -> Self {
        Self { config, client }
    }

    /// Access provider configuration.
    pub fn config(&self) -> &AxinomConfig {
        &self.config
    }
}

#[async_trait]
impl KeyProvider for AxinomProvider {
    async fn fetch_keys(&self, request: &KeyRequest) -> Result<KeySet> {
        let concrete_schemes = request.concrete_schemes();
        let concrete_schemes: Vec<crate::types::EncryptionScheme> =
            concrete_schemes.into_iter().flatten().collect();

        if concrete_schemes.len() > 1 {
            let mut combined_set = KeySet::new();
            for scheme in concrete_schemes {
                let mut single_req = request.clone();
                single_req.encryption_schemes = vec![scheme];
                let sub_set = self.fetch_single_scheme_keys(&single_req).await?;
                for key in sub_set.all_keys() {
                    combined_set.insert_key(key.clone());
                }
                for pssh in sub_set.pssh {
                    combined_set.add_pssh(pssh);
                }
            }
            Ok(combined_set)
        } else {
            self.fetch_single_scheme_keys(request).await
        }
    }
}

impl AxinomProvider {
    async fn fetch_single_scheme_keys(&self, request: &KeyRequest) -> Result<KeySet> {
        let (xml_request, specs) = CpixRequestBuilder::build_with_specs(request)?;

        // Build Basic Auth header: base64(tenant_id:management_key)
        let credentials = format!("{}:{}", self.config.tenant_id, self.config.management_key);
        let auth_value = format!("Basic {}", BASE64_STANDARD.encode(credentials.as_bytes()));

        let mut req = self
            .client
            .post(&self.config.endpoint)
            .header(reqwest::header::AUTHORIZATION, auth_value)
            .header("X-Speke-Version", "2.0")
            .header(
                "X-Speke-User-Agent",
                concat!("drmpack/", env!("CARGO_PKG_VERSION")),
            )
            .header(reqwest::header::CONTENT_TYPE, "application/xml")
            .header(reqwest::header::ACCEPT, "application/xml")
            .body(xml_request);

        if self.config.override_key_ids {
            req = req.query(&[("overrideKeyIds", "true")]);
        }

        // Apply custom headers if any
        for (k, v) in &self.config.headers {
            req = req.header(k, v);
        }

        let resp = req.send().await.map_err(|e| {
            DrmpackError::KeyProvider(format!(
                "Failed to send request to Axinom Key Service at '{}': {}",
                self.config.endpoint, e
            ))
        })?;

        let status = resp.status();
        if !status.is_success() {
            let ax_err_msg = resp
                .headers()
                .get("x-axdrm-errormessage")
                .and_then(|h| h.to_str().ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            let error_body = resp.text().await.unwrap_or_default();
            let detail = match (ax_err_msg, error_body.trim()) {
                (Some(header_msg), body) if !body.is_empty() && !body.contains(&header_msg) => {
                    format!("{header_msg} ({body})")
                }
                (Some(header_msg), _) => header_msg,
                (None, body) if !body.is_empty() => body.to_string(),
                (None, _) => status.canonical_reason().unwrap_or("Unknown").to_string(),
            };

            return Err(DrmpackError::KeyProvider(format!(
                "Axinom Key Service at '{}' returned HTTP {}: {}",
                self.config.endpoint, status, detail
            )));
        }

        let xml_response = resp.text().await.map_err(|e| {
            DrmpackError::KeyProvider(format!(
                "Failed to read response from Axinom Key Service at '{}': {}",
                self.config.endpoint, e
            ))
        })?;

        CpixResponseParser::parse(&xml_response, Some(&specs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_axinom_provider_debug_redaction() {
        let secret = "top-secret-management-key-abcxyz";
        let config = AxinomConfig::new("my-tenant-uuid", secret);
        let provider = AxinomProvider::new(config);

        let debug_str = format!("{:?}", provider);
        assert!(
            debug_str.contains("[REDACTED]"),
            "Provider debug must contain [REDACTED]"
        );
        assert!(
            !debug_str.contains(secret),
            "Provider debug must NOT leak the secret management_key"
        );
    }
}
