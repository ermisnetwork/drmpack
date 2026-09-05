use crate::cpix::builder::CpixRequestBuilder;
use crate::cpix::parser::CpixResponseParser;
use crate::error::{DrmpackError, Result};
use crate::key::{KeyProvider, KeyRequest, KeySet};
use crate::speke::config::SpekeConfig;
use async_trait::async_trait;
use std::fmt;

/// Response received from a raw SPEKE exchange.
#[derive(Debug, Clone)]
pub struct SpekeExchangeResponse {
    pub status: reqwest::StatusCode,
    pub headers: reqwest::header::HeaderMap,
    pub body: String,
}

impl SpekeExchangeResponse {
    /// Returns true if the HTTP response status code indicates success (200-299).
    pub fn is_success(&self) -> bool {
        self.status.is_success()
    }

    /// Retrieve a response header value as a string slice if present and valid UTF-8.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// Parse the XML body of the SPEKE exchange into a KeySet.
    pub fn parse_keys(
        &self,
        specs: Option<&[crate::cpix::CpixKeySpec]>,
    ) -> Result<crate::key::KeySet> {
        crate::cpix::CpixResponseParser::parse(&self.body, specs)
    }
}

/// AWS SPEKE v2.0 REST protocol client managing HTTPS POST with CPIX XML exchange.
#[derive(Clone)]
pub struct SpekeClient {
    config: SpekeConfig,
    client: reqwest::Client,
}

impl fmt::Debug for SpekeClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpekeClient")
            .field("config", &self.config)
            .finish()
    }
}

impl SpekeClient {
    /// Create a new SpekeClient targeting the given endpoint URL with default configuration.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self::with_config(SpekeConfig::new(endpoint))
    }

    /// Create a new SpekeClient with custom configuration.
    pub fn with_config(config: SpekeConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { config, client }
    }

    /// Create a new SpekeClient with a specific reqwest Client.
    pub fn with_client(config: SpekeConfig, client: reqwest::Client) -> Self {
        Self { config, client }
    }

    /// Access provider configuration.
    pub fn config(&self) -> &SpekeConfig {
        &self.config
    }

    /// Access underlying reqwest Client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Perform a raw HTTP POST exchange with the SPEKE v2 server.
    ///
    /// Automatically attaches:
    /// - `X-Speke-Version: 2.0`
    /// - `X-Speke-User-Agent: drmpack/<version>`
    /// - `Content-Type: application/xml`
    /// - `Accept: application/xml`
    /// - Authentication headers configured on `SpekeConfig`
    /// - Configured headers on `SpekeConfig`
    /// - Any extra headers and query parameters passed to this call
    /// - Applies configured dynamic `SpekeSigner` last so it can sign over all headers
    ///
    /// Returns `Ok(SpekeExchangeResponse)` regardless of HTTP status code,
    /// allowing caller to inspect status, response headers (e.g. `X-AxDRM-ErrorMessage`), and body.
    pub async fn raw_exchange(
        &self,
        xml_body: &str,
        query_params: &[(&str, &str)],
        extra_headers: Option<&reqwest::header::HeaderMap>,
    ) -> Result<SpekeExchangeResponse> {
        let mut req = self
            .client
            .post(&self.config.endpoint)
            .header("X-Speke-Version", "2.0")
            .header(
                "X-Speke-User-Agent",
                concat!("drmpack/", env!("CARGO_PKG_VERSION")),
            )
            .header(reqwest::header::CONTENT_TYPE, "application/xml")
            .header(reqwest::header::ACCEPT, "application/xml")
            .body(xml_body.to_string());

        if !query_params.is_empty() {
            req = req.query(query_params);
        }

        if let Some(auth) = &self.config.auth {
            req = auth.apply(req);
        }

        for (k, v) in &self.config.headers {
            req = req.header(k, v);
        }

        if let Some(extra) = extra_headers {
            for (k, v) in extra {
                req = req.header(k, v);
            }
        }

        if let Some(signer) = &self.config.signer {
            req = signer.sign(req, &self.config.endpoint, xml_body);
        }

        let resp = req.send().await.map_err(|e| {
            DrmpackError::KeyProvider(format!(
                "Failed to send SPEKE v2 request to '{}': {}",
                self.config.endpoint, e
            ))
        })?;

        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.text().await.map_err(|e| {
            DrmpackError::KeyProvider(format!(
                "Failed to read SPEKE v2 response from '{}': {}",
                self.config.endpoint, e
            ))
        })?;

        Ok(SpekeExchangeResponse {
            status,
            headers,
            body,
        })
    }
}

#[async_trait]
impl KeyProvider for SpekeClient {
    async fn fetch_keys(&self, request: &KeyRequest) -> Result<KeySet> {
        let (xml_request, specs) = CpixRequestBuilder::build_with_specs(request)?;

        let resp = self.raw_exchange(&xml_request, &[], None).await?;

        if !resp.status.is_success() {
            let error_header = resp
                .header("x-amzn-errortype")
                .or_else(|| resp.header("x-speke-error-message"))
                .map(|s| s.trim())
                .filter(|s| !s.is_empty());

            let body_trim = resp.body.trim();
            let detail = match (error_header, body_trim) {
                (Some(hdr), body) if !body.is_empty() && !body.contains(hdr) => {
                    format!("{hdr} ({body})")
                }
                (Some(hdr), _) => hdr.to_string(),
                (None, body) if !body.is_empty() => body.to_string(),
                (None, _) => resp
                    .status
                    .canonical_reason()
                    .unwrap_or("Unknown")
                    .to_string(),
            };

            return Err(DrmpackError::KeyProvider(format!(
                "SPEKE v2 provider at '{}' returned HTTP {}: {}",
                self.config.endpoint, resp.status, detail
            )));
        }

        CpixResponseParser::parse(&resp.body, Some(&specs))
    }
}
