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
    pub widevine_license_url: String,
    pub fairplay_license_url: String,
    pub playready_license_url: String,
    /// Optional FairPlay Application Certificate URL.
    ///
    /// The FairPlay certificate is issued by Apple (not by DRM vendors), and is typically
    /// self-hosted on the customer's CDN in production. This field is `None` when:
    /// - The deployment only serves Widevine/PlayReady (no Apple devices)
    /// - The certificate is loaded from a local file via `LicenseProxy::set_fairplay_certificate()`
    pub fairplay_cert_url: Option<String>,
    pub timeout: Duration,
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
