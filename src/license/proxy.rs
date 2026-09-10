use crate::error::{DrmpackError, Result};
use crate::license::config::LicenseProxyConfig;
use crate::license::response::LicenseResponse;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Trait converting various types into an optional FairPlay Certificate URL.
pub trait IntoCertUrl {
    fn into_cert_url(self) -> Option<String>;
}

impl IntoCertUrl for &str {
    fn into_cert_url(self) -> Option<String> {
        let trimmed = self.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
}

impl IntoCertUrl for String {
    fn into_cert_url(self) -> Option<String> {
        let trimmed = self.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
}

impl IntoCertUrl for &String {
    fn into_cert_url(self) -> Option<String> {
        self.as_str().into_cert_url()
    }
}

impl<T: IntoCertUrl> IntoCertUrl for Option<T> {
    fn into_cert_url(self) -> Option<String> {
        self.and_then(|v| v.into_cert_url())
    }
}

/// License proxy manager maintaining a pooled HTTP client and FairPlay certificate cache.
#[derive(Clone)]
pub struct LicenseProxy {
    client: reqwest::Client,
    config: LicenseProxyConfig,
    fairplay_cert_cache: Arc<RwLock<HashMap<String, bytes::Bytes>>>,
}

impl fmt::Debug for LicenseProxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LicenseProxy")
            .field("config", &self.config)
            .finish()
    }
}

impl LicenseProxy {
    /// Create a new LicenseProxy with given configuration and default reqwest::Client.
    pub fn new(config: impl Into<LicenseProxyConfig>) -> Self {
        let config = config.into();
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self::with_client(config, client)
    }

    /// Try to create a new LicenseProxy, validating the configuration first.
    pub fn try_new(config: impl Into<LicenseProxyConfig>) -> Result<Self> {
        let config = config.into();
        config.validate()?;
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|e| {
                DrmpackError::InvalidConfig(format!("Failed to build HTTP client: {e}"))
            })?;
        Ok(Self::with_client(config, client))
    }

    /// Create a new LicenseProxy with given configuration and custom reqwest::Client.
    pub fn with_client(config: impl Into<LicenseProxyConfig>, client: reqwest::Client) -> Self {
        Self {
            client,
            config: config.into(),
            fairplay_cert_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Access the underlying pooled reqwest::Client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Access the configuration.
    pub fn config(&self) -> &LicenseProxyConfig {
        &self.config
    }

    /// Return the currently cached FairPlay certificate for the configured endpoint, if any.
    pub async fn cached_fairplay_certificate(&self) -> Option<bytes::Bytes> {
        let cert_url = self.config.fairplay_cert_url.as_ref()?;
        let lock = self.fairplay_cert_cache.read().await;
        lock.get(cert_url).cloned()
    }

    /// Manually populate the FairPlay certificate cache.
    pub async fn set_fairplay_certificate(&self, cert: bytes::Bytes) {
        let key = self
            .config
            .fairplay_cert_url
            .clone()
            .unwrap_or_else(|| "manual".to_string());
        let mut lock = self.fairplay_cert_cache.write().await;
        lock.insert(key, cert);
    }

    /// Clear the FairPlay certificate cache.
    pub async fn clear_fairplay_certificate(&self) {
        let mut lock = self.fairplay_cert_cache.write().await;
        lock.clear();
    }

    /// Preload the FairPlay application certificate into memory from configured endpoint.
    pub async fn preload_fairplay_certificate(&self) -> Result<bytes::Bytes> {
        self.handle_fairplay_certificate(None::<&str>).await
    }

    /// Proxy a Widevine license challenge to the upstream DRM Provider.
    pub async fn handle_widevine_license(
        &self,
        challenge: impl AsRef<[u8]>,
        auth_token: &str,
    ) -> Result<LicenseResponse> {
        self.proxy_license_post(
            &self.config.widevine_license_url,
            challenge.as_ref(),
            auth_token,
            "Widevine",
            "application/octet-stream",
        )
        .await
    }

    /// Proxy an Apple FairPlay Server Playback Context (SPC) to the upstream DRM Provider.
    pub async fn handle_fairplay_license(
        &self,
        spc: impl AsRef<[u8]>,
        auth_token: &str,
    ) -> Result<LicenseResponse> {
        self.proxy_license_post(
            &self.config.fairplay_license_url,
            spc.as_ref(),
            auth_token,
            "FairPlay",
            "application/octet-stream",
        )
        .await
    }

    /// Proxy a Microsoft PlayReady license challenge to the upstream DRM Provider.
    pub async fn handle_playready_license(
        &self,
        challenge: impl AsRef<[u8]>,
        auth_token: &str,
    ) -> Result<LicenseResponse> {
        self.proxy_license_post(
            &self.config.playready_license_url,
            challenge.as_ref(),
            auth_token,
            "PlayReady",
            "text/xml; charset=utf-8",
        )
        .await
    }

    /// Retrieve the Apple FairPlay application certificate, returning cached bytes if available.
    pub async fn handle_fairplay_certificate(
        &self,
        cert_url: impl IntoCertUrl,
    ) -> Result<bytes::Bytes> {
        let url = cert_url
            .into_cert_url()
            .filter(|s| !s.is_empty())
            .or_else(|| self.config.fairplay_cert_url.clone())
            .ok_or_else(|| {
                DrmpackError::InvalidConfig(
                    "FairPlay certificate URL not specified or configured. \
                     Set AXINOM_FAIRPLAY_CERT_URL in .env or pass the URL explicitly."
                        .into(),
                )
            })?;

        let trimmed_url = url.trim();
        if trimmed_url.is_empty() {
            return Err(DrmpackError::InvalidConfig(
                "FairPlay certificate URL not specified or configured".into(),
            ));
        }
        if !trimmed_url.starts_with("http://") && !trimmed_url.starts_with("https://") {
            return Err(DrmpackError::InvalidConfig(format!(
                "Invalid FairPlay certificate URL: '{trimmed_url}'"
            )));
        }

        // 1. Check read lock first (fast path)
        {
            let lock = self.fairplay_cert_cache.read().await;
            if let Some(cert) = lock.get(trimmed_url) {
                return Ok(cert.clone());
            }
        }

        // 2. Fetch certificate over network WITHOUT holding any lock to prevent contention
        let mut req = self
            .client
            .get(trimmed_url)
            .header(
                reqwest::header::USER_AGENT,
                concat!("drmpack/", env!("CARGO_PKG_VERSION")),
            )
            .header(
                reqwest::header::ACCEPT,
                "application/x-x509-ca-cert, application/octet-stream, */*",
            )
            .timeout(self.config.timeout);

        for (k, v) in &self.config.headers {
            req = req.header(k, v);
        }

        let resp = req.send().await.map_err(|e| DrmpackError::LicenseProxy {
            status: e.status().unwrap_or(reqwest::StatusCode::BAD_GATEWAY),
            message: format!("Failed to fetch FairPlay certificate from '{trimmed_url}': {e}"),
            diagnostic: None,
        })?;

        let status = resp.status();
        if !status.is_success() {
            return Err(parse_license_error_response(resp, "FairPlay certificate").await);
        }

        let data = resp.bytes().await.map_err(|e| DrmpackError::LicenseProxy {
            status: reqwest::StatusCode::BAD_GATEWAY,
            message: format!("Failed to read FairPlay certificate bytes from '{trimmed_url}': {e}"),
            diagnostic: None,
        })?;

        if data.is_empty() {
            return Err(DrmpackError::LicenseProxy {
                status: reqwest::StatusCode::BAD_GATEWAY,
                message: format!(
                    "Received empty FairPlay application certificate from '{trimmed_url}'"
                ),
                diagnostic: None,
            });
        }

        // 3. Briefly acquire write lock solely to insert the fetched certificate
        let mut lock = self.fairplay_cert_cache.write().await;
        lock.insert(trimmed_url.to_string(), data.clone());
        Ok(data)
    }

    async fn proxy_license_post(
        &self,
        url: &str,
        payload: &[u8],
        auth_token: &str,
        system_name: &str,
        default_content_type: &str,
    ) -> Result<LicenseResponse> {
        let trimmed_url = url.trim();
        if trimmed_url.is_empty() {
            return Err(DrmpackError::InvalidConfig(format!(
                "{system_name} license acquisition URL is not configured"
            )));
        }
        if !trimmed_url.starts_with("http://") && !trimmed_url.starts_with("https://") {
            return Err(DrmpackError::InvalidConfig(format!(
                "Invalid {system_name} license acquisition URL: '{trimmed_url}'"
            )));
        }

        let trimmed_token = auth_token.trim();
        if trimmed_token.is_empty() {
            return Err(DrmpackError::LicenseProxy {
                status: reqwest::StatusCode::UNAUTHORIZED,
                message: format!(
                    "Missing or empty authentication token for {system_name} license request"
                ),
                diagnostic: None,
            });
        }

        if payload.is_empty() {
            return Err(DrmpackError::LicenseProxy {
                status: reqwest::StatusCode::BAD_REQUEST,
                message: format!("{system_name} license challenge payload cannot be empty"),
                diagnostic: None,
            });
        }

        let mut req = self
            .client
            .post(trimmed_url)
            .header(
                reqwest::header::USER_AGENT,
                concat!("drmpack/", env!("CARGO_PKG_VERSION")),
            )
            .timeout(self.config.timeout)
            .body(bytes::Bytes::copy_from_slice(payload));

        for (k, v) in &self.config.headers {
            req = req.header(k, v);
        }

        if !self
            .config
            .headers
            .contains_key(reqwest::header::CONTENT_TYPE)
        {
            req = req.header(reqwest::header::CONTENT_TYPE, default_content_type);
        }

        req = req.header("X-AxDRM-Message", trimmed_token);

        let resp = req.send().await.map_err(|e| DrmpackError::LicenseProxy {
            status: e.status().unwrap_or(reqwest::StatusCode::BAD_GATEWAY),
            message: format!(
                "Failed to send {system_name} license request to '{trimmed_url}': {e}"
            ),
            diagnostic: None,
        })?;

        let status = resp.status();
        if !status.is_success() {
            return Err(parse_license_error_response(resp, system_name).await);
        }

        let headers = resp.headers().clone();
        let content_type = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let data = resp.bytes().await.map_err(|e| DrmpackError::LicenseProxy {
            status: reqwest::StatusCode::BAD_GATEWAY,
            message: format!("Failed to read {system_name} license payload: {e}"),
            diagnostic: None,
        })?;

        if data.is_empty() {
            return Err(DrmpackError::LicenseProxy {
                status: reqwest::StatusCode::BAD_GATEWAY,
                message: format!(
                    "Upstream {system_name} Provider returned 200 OK with empty license payload"
                ),
                diagnostic: None,
            });
        }

        Ok(LicenseResponse::new(data, content_type, headers))
    }
}

async fn parse_license_error_response(resp: reqwest::Response, context: &str) -> DrmpackError {
    let status = resp.status();
    let headers = resp.headers().clone();
    let diagnostic = headers
        .get("x-axdrm-errormessage")
        .map(|v| String::from_utf8_lossy(v.as_bytes()).trim().to_string())
        .filter(|s| !s.is_empty());

    let body_bytes = resp.bytes().await.unwrap_or_default();
    let raw_body = String::from_utf8_lossy(&body_bytes);
    let body_trim = raw_body.trim();
    let body_bounded = if body_trim.len() > 2048 {
        let mut end = 2048;
        while !body_trim.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &body_trim[..end])
    } else {
        body_trim.to_string()
    };

    let message = match (&diagnostic, body_bounded.is_empty()) {
        (Some(diag), false) if !body_bounded.eq_ignore_ascii_case(diag) => {
            format!("{diag} ({body_bounded})")
        }
        (Some(diag), _) => diag.clone(),
        (None, false) => body_bounded,
        (None, true) => status
            .canonical_reason()
            .map(|r| format!("{context}: {r}"))
            .unwrap_or_else(|| format!("{context}: upstream returned HTTP {status}")),
    };

    DrmpackError::LicenseProxy {
        status,
        message,
        diagnostic,
    }
}

/// Forward a player's Widevine license challenge to upstream DRM Provider.
pub async fn handle_widevine_license(
    proxy: &LicenseProxy,
    challenge: impl AsRef<[u8]>,
    auth_token: &str,
) -> Result<LicenseResponse> {
    proxy.handle_widevine_license(challenge, auth_token).await
}

/// Forward an Apple FairPlay Server Playback Context (SPC) to upstream DRM Provider.
pub async fn handle_fairplay_license(
    proxy: &LicenseProxy,
    spc: impl AsRef<[u8]>,
    auth_token: &str,
) -> Result<LicenseResponse> {
    proxy.handle_fairplay_license(spc, auth_token).await
}

/// Forward a Microsoft PlayReady license challenge to upstream DRM Provider.
pub async fn handle_playready_license(
    proxy: &LicenseProxy,
    challenge: impl AsRef<[u8]>,
    auth_token: &str,
) -> Result<LicenseResponse> {
    proxy.handle_playready_license(challenge, auth_token).await
}

/// Fetch or retrieve the cached Apple FairPlay Application Certificate.
pub async fn handle_fairplay_certificate(
    proxy: &LicenseProxy,
    cert_url: impl IntoCertUrl,
) -> Result<bytes::Bytes> {
    proxy.handle_fairplay_certificate(cert_url).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_license_proxy_manual_cert_caching() {
        let config = LicenseProxyConfig::new(
            "https://example.com/wv",
            "https://example.com/fp",
            "https://example.com/pr",
            Some("https://example.com/cert".to_string()),
        );
        let proxy = LicenseProxy::new(config);
        assert_eq!(proxy.cached_fairplay_certificate().await, None);

        let cert_data = bytes::Bytes::from_static(b"fairplay-cert-sample");
        proxy.set_fairplay_certificate(cert_data.clone()).await;
        assert_eq!(proxy.cached_fairplay_certificate().await, Some(cert_data));

        proxy.clear_fairplay_certificate().await;
        assert_eq!(proxy.cached_fairplay_certificate().await, None);
    }

    #[test]
    fn test_license_proxy_try_new() {
        let valid = LicenseProxyConfig::new(
            "https://example.com/wv",
            "https://example.com/fp",
            "https://example.com/pr",
            Some("https://example.com/cert".to_string()),
        );
        let proxy = LicenseProxy::try_new(valid.clone());
        assert!(proxy.is_ok());

        let invalid = valid.with_widevine_license_url("invalid-url");
        let err_proxy = LicenseProxy::try_new(invalid);
        assert!(err_proxy.is_err());
    }

    #[test]
    fn test_license_proxy_try_new_without_cert_url() {
        let config = LicenseProxyConfig::new(
            "https://example.com/wv",
            "https://example.com/fp",
            "https://example.com/pr",
            None,
        );
        // Should succeed: cert_url is optional
        let proxy = LicenseProxy::try_new(config);
        assert!(proxy.is_ok());
    }

    #[test]
    fn test_license_proxy_debug_format() {
        let config = LicenseProxyConfig::new(
            "https://example.com/wv",
            "https://example.com/fp",
            "https://example.com/pr",
            Some("https://example.com/cert".to_string()),
        );
        let proxy = LicenseProxy::new(config);
        let debug_str = format!("{proxy:?}");
        assert!(debug_str.contains("LicenseProxy"));
        assert!(debug_str.contains("widevine_license_url"));
    }

    #[test]
    fn test_into_cert_url_implementations() {
        assert_eq!(
            "https://example.com".into_cert_url(),
            Some("https://example.com".to_string())
        );
        assert_eq!("  ".into_cert_url(), None);
        assert_eq!("".into_cert_url(), None);

        let s = "https://example.com".to_string();
        assert_eq!(
            (&s).into_cert_url(),
            Some("https://example.com".to_string())
        );
        assert_eq!(s.into_cert_url(), Some("https://example.com".to_string()));

        let opt_some = Some("https://example.com");
        assert_eq!(
            opt_some.into_cert_url(),
            Some("https://example.com".to_string())
        );

        let opt_none: Option<&str> = None;
        assert_eq!(opt_none.into_cert_url(), None);
    }
}
