use crate::error::{DrmpackError, Result};
use std::time::Duration;

/// Vendor-agnostic configuration for the DRM license proxy endpoints and connection parameters.
///
/// This struct holds the URLs for DRM license acquisition services (Widevine, FairPlay, PlayReady)
/// and an optional FairPlay Application Certificate URL. It is intentionally decoupled from any
/// specific DRM vendor to allow `LicenseProxy` to operate without vendor-specific dependencies.
///
/// For Axinom users, `AxinomLicenseConfig` implements `Into<LicenseProxyConfig>` for seamless conversion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LicenseProxyConfig {
    /// URL for Google Widevine license acquisition requests.
    pub widevine_license_url: String,
    /// URL for Apple FairPlay license acquisition requests.
    pub fairplay_license_url: String,
    /// URL for Microsoft PlayReady license acquisition requests.
    pub playready_license_url: String,
    /// Optional FairPlay Application Certificate URL.
    ///
    /// The FairPlay certificate is issued by Apple (not by DRM vendors), and is typically
    /// self-hosted on the customer's CDN in production. This field is `None` when:
    /// - The deployment only serves Widevine/PlayReady (no Apple devices)
    /// - The certificate is loaded from a local file via `LicenseProxy::set_fairplay_certificate()`
    pub fairplay_cert_url: Option<String>,
    /// Request timeout for upstream license proxying.
    pub timeout: Duration,
    /// Custom HTTP headers forwarded to the license server.
    pub headers: reqwest::header::HeaderMap,
}

impl LicenseProxyConfig {
    /// Create a new `LicenseProxyConfig` with required license URLs and optional cert URL.
    pub fn new(
        widevine_license_url: impl Into<String>,
        fairplay_license_url: impl Into<String>,
        playready_license_url: impl Into<String>,
        fairplay_cert_url: Option<String>,
    ) -> Self {
        Self {
            widevine_license_url: widevine_license_url.into(),
            fairplay_license_url: fairplay_license_url.into(),
            playready_license_url: playready_license_url.into(),
            fairplay_cert_url,
            timeout: Duration::from_secs(10),
            headers: reqwest::header::HeaderMap::new(),
        }
    }

    /// Set a custom Widevine license acquisition URL.
    pub fn with_widevine_license_url(mut self, url: impl Into<String>) -> Self {
        self.widevine_license_url = url.into();
        self
    }

    /// Set a custom FairPlay license acquisition URL.
    pub fn with_fairplay_license_url(mut self, url: impl Into<String>) -> Self {
        self.fairplay_license_url = url.into();
        self
    }

    /// Set a custom PlayReady license acquisition URL.
    pub fn with_playready_license_url(mut self, url: impl Into<String>) -> Self {
        self.playready_license_url = url.into();
        self
    }

    /// Set a custom FairPlay Application Certificate URL.
    pub fn with_fairplay_cert_url(mut self, url: impl Into<String>) -> Self {
        self.fairplay_cert_url = Some(url.into());
        self
    }

    /// Set request timeout duration.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Add a custom HTTP header to outgoing requests.
    pub fn with_header(
        mut self,
        name: reqwest::header::HeaderName,
        value: reqwest::header::HeaderValue,
    ) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Set multiple custom HTTP headers.
    pub fn with_headers(mut self, headers: reqwest::header::HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    /// Validate the configuration endpoints and parameters.
    pub fn validate(&self) -> Result<()> {
        Self::validate_url("widevine_license_url", &self.widevine_license_url)?;
        Self::validate_url("fairplay_license_url", &self.fairplay_license_url)?;
        Self::validate_url("playready_license_url", &self.playready_license_url)?;
        if let Some(ref cert_url) = self.fairplay_cert_url {
            Self::validate_url("fairplay_cert_url", cert_url)?;
        }
        if self.timeout.is_zero() {
            return Err(DrmpackError::InvalidConfig(
                "License proxy timeout duration cannot be zero".into(),
            ));
        }
        Ok(())
    }

    fn validate_url(name: &str, url: &str) -> Result<()> {
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return Err(DrmpackError::InvalidConfig(format!(
                "License proxy '{name}' cannot be empty"
            )));
        }
        if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
            return Err(DrmpackError::InvalidConfig(format!(
                "License proxy '{name}' must start with http:// or https://, got: '{trimmed}'"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderName, HeaderValue};

    #[test]
    fn test_license_proxy_config_new_and_builders() {
        let config = LicenseProxyConfig::new(
            "https://wv.example.com",
            "https://fp.example.com",
            "https://pr.example.com",
            None,
        );
        assert_eq!(config.widevine_license_url, "https://wv.example.com");
        assert_eq!(config.fairplay_license_url, "https://fp.example.com");
        assert_eq!(config.playready_license_url, "https://pr.example.com");
        assert_eq!(config.fairplay_cert_url, None);
        assert_eq!(config.timeout, Duration::from_secs(10));
        assert!(config.headers.is_empty());

        let mut custom_headers = reqwest::header::HeaderMap::new();
        custom_headers.insert(
            HeaderName::from_static("x-tenant-id"),
            HeaderValue::from_static("tenant-123"),
        );

        let updated = config
            .with_widevine_license_url("https://wv2.example.com")
            .with_fairplay_license_url("https://fp2.example.com")
            .with_playready_license_url("https://pr2.example.com")
            .with_fairplay_cert_url("https://cert.example.com/fp.cer")
            .with_timeout(Duration::from_secs(20))
            .with_header(
                HeaderName::from_static("authorization"),
                HeaderValue::from_static("Bearer token"),
            )
            .with_headers(custom_headers);

        assert_eq!(updated.widevine_license_url, "https://wv2.example.com");
        assert_eq!(updated.fairplay_license_url, "https://fp2.example.com");
        assert_eq!(updated.playready_license_url, "https://pr2.example.com");
        assert_eq!(
            updated.fairplay_cert_url,
            Some("https://cert.example.com/fp.cer".to_string())
        );
        assert_eq!(updated.timeout, Duration::from_secs(20));
        assert_eq!(
            updated.headers.get("x-tenant-id"),
            Some(&HeaderValue::from_static("tenant-123"))
        );
    }

    #[test]
    fn test_license_proxy_config_validate_success() {
        let config_no_cert = LicenseProxyConfig::new(
            "https://wv.example.com",
            "https://fp.example.com",
            "https://pr.example.com",
            None,
        );
        assert!(config_no_cert.validate().is_ok());

        let config_with_cert = config_no_cert
            .clone()
            .with_fairplay_cert_url("https://cert.example.com/fairplay.cer");
        assert!(config_with_cert.validate().is_ok());
    }

    #[test]
    fn test_license_proxy_config_validate_errors() {
        let valid = LicenseProxyConfig::new(
            "https://wv.example.com",
            "https://fp.example.com",
            "https://pr.example.com",
            Some("https://cert.example.com".to_string()),
        );

        // Empty URLs
        assert!(valid
            .clone()
            .with_widevine_license_url("  ")
            .validate()
            .is_err());
        assert!(valid
            .clone()
            .with_fairplay_license_url("")
            .validate()
            .is_err());
        assert!(valid
            .clone()
            .with_playready_license_url("")
            .validate()
            .is_err());

        // Invalid URL schemes
        assert!(valid
            .clone()
            .with_widevine_license_url("ftp://example.com")
            .validate()
            .is_err());
        assert!(valid
            .clone()
            .with_fairplay_license_url("custom://example.com")
            .validate()
            .is_err());
        assert!(valid
            .clone()
            .with_fairplay_cert_url("file:///etc/cert.der")
            .validate()
            .is_err());
        assert!(valid
            .clone()
            .with_fairplay_cert_url("  ")
            .validate()
            .is_err());

        // Zero timeout
        assert!(valid
            .clone()
            .with_timeout(Duration::ZERO)
            .validate()
            .is_err());
    }
}
