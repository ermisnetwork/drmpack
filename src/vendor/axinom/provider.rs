use crate::cpix::builder::CpixRequestBuilder;
use crate::cpix::parser::CpixResponseParser;
use crate::error::{DrmpackError, Result};
use crate::key::{KeyProvider, KeyRequest, KeySet};
use crate::speke::{SpekeAuth, SpekeClient, SpekeConfig};
use crate::vendor::axinom::config::AxinomConfig;
use async_trait::async_trait;
use std::fmt;

/// Axinom KeyProvider implementing SPEKE v2 over CPIX 2.3 protocol.
pub struct AxinomProvider {
    config: AxinomConfig,
    speke_client: SpekeClient,
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
        let speke_config = SpekeConfig::new(&config.endpoint)
            .with_timeout(config.timeout)
            .with_auth(SpekeAuth::basic(&config.tenant_id, &config.management_key))
            .with_headers(config.headers.clone());
        let speke_client = SpekeClient::with_config(speke_config);
        Self {
            config,
            speke_client,
        }
    }

    /// Create a new AxinomProvider with custom reqwest client.
    pub fn with_client(config: AxinomConfig, client: reqwest::Client) -> Self {
        let speke_config = SpekeConfig::new(&config.endpoint)
            .with_timeout(config.timeout)
            .with_auth(SpekeAuth::basic(&config.tenant_id, &config.management_key))
            .with_headers(config.headers.clone());
        let speke_client = SpekeClient::with_client(speke_config, client);
        Self {
            config,
            speke_client,
        }
    }

    /// Access provider configuration.
    pub fn config(&self) -> &AxinomConfig {
        &self.config
    }

    /// Access underlying SPEKE protocol client.
    pub fn speke_client(&self) -> &SpekeClient {
        &self.speke_client
    }

    /// Access underlying reqwest Client.
    pub fn client(&self) -> &reqwest::Client {
        self.speke_client.client()
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

        let query_params: &[(&str, &str)] = if self.config.override_key_ids {
            &[("overrideKeyIds", "true")]
        } else {
            &[]
        };

        let resp = self
            .speke_client
            .raw_exchange(&xml_request, query_params, None)
            .await
            .map_err(|e| {
                DrmpackError::KeyProvider(format!(
                    "Failed to send request to Axinom Key Service at '{}': {}",
                    self.config.endpoint, e
                ))
            })?;

        if !resp.status.is_success() {
            let ax_err_msg = resp
                .header("x-axdrm-errormessage")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            let error_body = resp.body;
            let detail = match (ax_err_msg, error_body.trim()) {
                (Some(header_msg), body) if !body.is_empty() && !body.contains(&header_msg) => {
                    format!("{header_msg} ({body})")
                }
                (Some(header_msg), _) => header_msg,
                (None, body) if !body.is_empty() => body.to_string(),
                (None, _) => resp
                    .status
                    .canonical_reason()
                    .unwrap_or("Unknown")
                    .to_string(),
            };

            return Err(DrmpackError::KeyProvider(format!(
                "Axinom Key Service at '{}' returned HTTP {}: {}",
                self.config.endpoint, resp.status, detail
            )));
        }

        CpixResponseParser::parse(&resp.body, Some(&specs))
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
