use crate::speke::auth::{SigV4Credentials, SpekeAuth, SpekeSigner};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// Configuration options for AWS SPEKE v2.0 REST protocol client.
#[derive(Clone)]
pub struct SpekeConfig {
    pub endpoint: String,
    pub timeout: Duration,
    pub auth: Option<SpekeAuth>,
    pub headers: reqwest::header::HeaderMap,
    pub signer: Option<Arc<dyn SpekeSigner>>,
}

impl SpekeConfig {
    /// Create new configuration for given endpoint URL with 10s default timeout.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            timeout: Duration::from_secs(10),
            auth: None,
            headers: reqwest::header::HeaderMap::new(),
            signer: None,
        }
    }

    /// Set endpoint URL.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// Set HTTP request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set authentication mechanism.
    pub fn with_auth(mut self, auth: SpekeAuth) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Set standard AWS API Key (`x-api-key`) authentication shorthand.
    pub fn with_x_api_key(self, api_key: impl Into<String>) -> Self {
        self.with_auth(SpekeAuth::x_api_key(api_key))
    }

    /// Set standard AWS SigV4 authentication shorthand.
    pub fn with_sigv4(
        self,
        authorization: impl Into<String>,
        security_token: Option<&str>,
        date: impl Into<String>,
    ) -> Self {
        self.with_auth(SpekeAuth::sigv4(authorization, security_token, date))
    }

    /// Set AWS SigV4 authentication using pre-built [`SigV4Credentials`].
    pub fn with_sigv4_credentials(self, credentials: SigV4Credentials) -> Self {
        self.with_auth(SpekeAuth::SigV4(credentials))
    }

    /// Add a custom HTTP header.
    pub fn with_header(
        mut self,
        name: reqwest::header::HeaderName,
        value: reqwest::header::HeaderValue,
    ) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Append multiple HTTP headers.
    pub fn with_headers(mut self, headers: reqwest::header::HeaderMap) -> Self {
        self.headers.extend(headers);
        self
    }

    /// Set a custom request signer (e.g. dynamic AWS SigV4 signer).
    pub fn with_signer(mut self, signer: impl SpekeSigner + 'static) -> Self {
        self.signer = Some(Arc::new(signer));
        self
    }
}

impl fmt::Debug for SpekeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpekeConfig")
            .field("endpoint", &self.endpoint)
            .field("timeout", &self.timeout)
            .field("auth", &self.auth)
            .field("headers", &self.headers)
            .field("has_signer", &self.signer.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speke_config_defaults() {
        let config = SpekeConfig::new("https://speke.example.com/api");
        assert_eq!(config.endpoint, "https://speke.example.com/api");
        assert_eq!(config.timeout, Duration::from_secs(10));
        assert!(config.auth.is_none());
        assert!(config.headers.is_empty());
        assert!(config.signer.is_none());
    }

    #[test]
    fn test_speke_config_builder() {
        let config = SpekeConfig::new("https://speke.example.com/api")
            .with_timeout(Duration::from_secs(30))
            .with_auth(SpekeAuth::bearer("my-token"))
            .with_header(
                reqwest::header::HeaderName::from_static("x-custom"),
                reqwest::header::HeaderValue::from_static("custom-val"),
            );

        assert_eq!(config.timeout, Duration::from_secs(30));
        assert!(config.auth.is_some());
        assert_eq!(
            config.headers.get("x-custom").and_then(|v| v.to_str().ok()),
            Some("custom-val")
        );
    }

    #[test]
    fn test_speke_config_auth_shorthands() {
        let config_api =
            SpekeConfig::new("https://speke.example.com/api").with_x_api_key("my-api-key");
        assert_eq!(config_api.auth, Some(SpekeAuth::x_api_key("my-api-key")));

        let config_sigv4 = SpekeConfig::new("https://speke.example.com/api").with_sigv4(
            "AWS4-HMAC...",
            Some("token"),
            "20260905T120000Z",
        );
        assert_eq!(
            config_sigv4.auth,
            Some(SpekeAuth::sigv4(
                "AWS4-HMAC...",
                Some("token"),
                "20260905T120000Z"
            ))
        );

        let creds = SigV4Credentials::new("AWS4-HMAC...", "20260905T120000Z")
            .with_security_token("token")
            .with_content_sha256("sha256hash");
        let config_creds =
            SpekeConfig::new("https://speke.example.com/api").with_sigv4_credentials(creds.clone());
        assert_eq!(config_creds.auth, Some(SpekeAuth::SigV4(creds)));
    }

    #[test]
    fn test_speke_config_signer() {
        let config = SpekeConfig::new("https://speke.example.com/api").with_signer(
            |req: reqwest::RequestBuilder, _endpoint: &str, _body: &str| {
                req.header("x-signed-by", "custom-signer")
            },
        );

        assert!(config.signer.is_some());
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("has_signer: true"));
    }
}
