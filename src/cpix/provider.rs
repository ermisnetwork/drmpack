use crate::cpix::builder::CpixRequestBuilder;
use crate::cpix::parser::CpixResponseParser;
use crate::error::{DrmpackError, Result};
use crate::key::{KeyProvider, KeyRequest, KeySet};
use async_trait::async_trait;
use std::time::Duration;

/// Configuration options for connecting to a CPIX 2.3 key service.
#[derive(Debug, Clone)]
pub struct CpixConfig {
    pub endpoint: String,
    pub timeout: Duration,
    pub headers: reqwest::header::HeaderMap,
}

impl CpixConfig {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            timeout: Duration::from_secs(10),
            headers: reqwest::header::HeaderMap::new(),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_header(
        mut self,
        name: reqwest::header::HeaderName,
        value: reqwest::header::HeaderValue,
    ) -> Self {
        self.headers.insert(name, value);
        self
    }
}

/// A KeyProvider implementation that fetches encryption keys via the DASH-IF CPIX 2.3 protocol.
pub struct CpixProvider {
    config: CpixConfig,
    client: reqwest::Client,
}

impl CpixProvider {
    /// Create a new CPIX provider targeting the given endpoint URL with default configuration.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self::with_config(CpixConfig::new(endpoint))
    }

    /// Create a new CPIX provider with custom configuration.
    pub fn with_config(config: CpixConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { config, client }
    }

    /// Create a new CPIX provider with a specific reqwest Client.
    pub fn with_client(config: CpixConfig, client: reqwest::Client) -> Self {
        Self { config, client }
    }

    /// Access the provider configuration.
    pub fn config(&self) -> &CpixConfig {
        &self.config
    }
}

#[async_trait]
impl KeyProvider for CpixProvider {
    async fn fetch_keys(&self, request: &KeyRequest) -> Result<KeySet> {
        let (xml_request, specs) = CpixRequestBuilder::build_with_specs(request)?;

        let mut req = self
            .client
            .post(&self.config.endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/xml")
            .header(reqwest::header::ACCEPT, "application/xml")
            .body(xml_request);

        for (k, v) in &self.config.headers {
            req = req.header(k, v);
        }

        let resp = req.send().await.map_err(|e| {
            DrmpackError::KeyProvider(format!(
                "Failed to send CPIX request to '{}': {}",
                self.config.endpoint, e
            ))
        })?;

        let status = resp.status();
        if !status.is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            return Err(DrmpackError::KeyProvider(format!(
                "CPIX provider at '{}' returned HTTP {}: {}",
                self.config.endpoint, status, error_text
            )));
        }

        let xml_response = resp.text().await.map_err(|e| {
            DrmpackError::KeyProvider(format!(
                "Failed to read CPIX response from '{}': {}",
                self.config.endpoint, e
            ))
        })?;

        CpixResponseParser::parse(&xml_response, Some(&specs))
    }
}
