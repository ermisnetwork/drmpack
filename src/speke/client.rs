use crate::cpix::builder::CpixRequestBuilder;
use crate::cpix::parser::CpixResponseParser;
use crate::error::{DrmpackError, Result};
use crate::key::{KeyProvider, KeyRequest, KeySet};
use crate::speke::config::SpekeConfig;
use std::fmt;
use tracing::{debug, warn};

/// Response received from a raw SPEKE exchange.
#[derive(Debug, Clone)]
pub struct SpekeExchangeResponse {
    /// HTTP status code returned by the key server.
    pub status: reqwest::StatusCode,
    /// HTTP response headers.
    pub headers: reqwest::header::HeaderMap,
    /// Response payload body string.
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

    /// Retrieve the `X-Speke-Version` header value from the key server response if present.
    pub fn speke_version(&self) -> Option<&str> {
        self.header("x-speke-version")
    }

    /// Retrieve the `X-Speke-User-Agent` header value from the key server response if present.
    pub fn speke_user_agent(&self) -> Option<&str> {
        self.header("x-speke-user-agent")
    }

    /// Format non-200 error detail from response headers and body.
    ///
    /// Checks for error diagnostic headers in order of precedence:
    /// - `x-amzn-errortype` (AWS API Gateway / Lambda)
    /// - `x-speke-error-message` (AWS SPEKE v2)
    /// - `x-axdrm-errormessage` (Axinom Key Service)
    ///
    /// Combines the header message with the response body if the body contains additional context.
    /// Safely truncates response bodies exceeding 2048 bytes on UTF-8 boundaries to avoid log flooding.
    pub fn format_error_detail(&self) -> String {
        let error_header = self
            .header("x-amzn-errortype")
            .or_else(|| self.header("x-speke-error-message"))
            .or_else(|| self.header("x-axdrm-errormessage"))
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());

        let body_trim = self.body.trim();
        let truncated_storage;
        let body_bounded = if body_trim.len() > 2048 {
            let end = body_trim.floor_char_boundary(2048);
            truncated_storage = format!("{}...", &body_trim[..end]);
            truncated_storage.as_str()
        } else {
            body_trim
        };

        match (error_header, body_bounded) {
            (Some(hdr), body) if !body.is_empty() && !body.eq_ignore_ascii_case(hdr) => {
                format!("{hdr} ({body})")
            }
            (Some(hdr), _) => hdr.to_string(),
            (None, body) if !body.is_empty() => body.to_string(),
            (None, _) => self
                .status
                .canonical_reason()
                .unwrap_or("Unknown")
                .to_string(),
        }
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
    /// - `User-Agent: drmpack/<version>`
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
                reqwest::header::USER_AGENT,
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

        debug!(endpoint = %self.config.endpoint, "Sending SPEKE v2 key exchange request");

        let resp = req.send().await.map_err(|e| {
            DrmpackError::KeyProvider(format!(
                "Failed to send SPEKE v2 request to '{}': {}",
                self.config.endpoint, e
            ))
        })?;

        let status = resp.status();
        let headers = resp.headers().clone();
        debug!(endpoint = %self.config.endpoint, ?status, "Received SPEKE v2 response");

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

impl KeyProvider for SpekeClient {
    async fn fetch_keys(&self, request: &KeyRequest) -> Result<KeySet> {
        let (xml_request, specs) = CpixRequestBuilder::build_with_specs(request)?;

        let resp = self.raw_exchange(&xml_request, &[], None).await?;

        if !resp.status.is_success() {
            let detail = resp.format_error_detail();
            warn!(endpoint = %self.config.endpoint, ?resp.status, %detail, "SPEKE v2 key request rejected by endpoint");
            return Err(DrmpackError::KeyProvider(format!(
                "SPEKE v2 endpoint at '{}' returned HTTP {}: {}",
                self.config.endpoint, resp.status, detail
            )));
        }

        CpixResponseParser::parse(&resp.body, Some(&specs))
    }
}
