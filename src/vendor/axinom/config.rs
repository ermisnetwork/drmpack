use crate::error::{DrmpackError, Result};
use std::fmt;
use std::time::Duration;

/// Configuration for connecting to the Axinom Key Service (SPEKE v2 over CPIX 2.3).
#[derive(Clone)]
pub struct AxinomConfig {
    pub tenant_id: String,
    pub management_key: String,
    pub endpoint: String,
    pub override_key_ids: bool,
    pub timeout: Duration,
    pub headers: reqwest::header::HeaderMap,
}

impl AxinomConfig {
    /// Create a new Axinom configuration with required tenant ID, management key, and SPEKE v2 endpoint URL.
    ///
    /// `override_key_ids` defaults to `false`, and `timeout` defaults to 10 seconds.
    pub fn new(
        tenant_id: impl Into<String>,
        management_key: impl Into<String>,
        endpoint: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            management_key: management_key.into(),
            endpoint: endpoint.into(),
            override_key_ids: false,
            timeout: Duration::from_secs(10),
            headers: reqwest::header::HeaderMap::new(),
        }
    }

    /// Set a custom endpoint URL.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// Configure whether Axinom should override Key IDs (`overrideKeyIds` query param).
    ///
    /// When `true`, Axinom generates deterministic Key IDs instead of using the
    /// Key IDs generated in the CPIX request.
    pub fn with_override_key_ids(mut self, override_key_ids: bool) -> Self {
        self.override_key_ids = override_key_ids;
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

    /// Load Axinom configuration from environment variables:
    /// - `AXINOM_TENANT_ID` (required)
    /// - `AXINOM_MANAGEMENT_KEY` (required)
    /// - `AXINOM_ENDPOINT` or `AXINOM_SPEKE_ENDPOINT` (required)
    /// - `AXINOM_OVERRIDE_KEY_IDS` (optional, boolean "true"/"false"/"1"/"0")
    pub fn from_env() -> Result<Self> {
        let tenant_id = std::env::var("AXINOM_TENANT_ID").map_err(|_| {
            DrmpackError::InvalidConfig(
                "Missing required environment variable 'AXINOM_TENANT_ID'".into(),
            )
        })?;
        let tenant_id = tenant_id.trim();
        if tenant_id.is_empty() {
            return Err(DrmpackError::InvalidConfig(
                "Environment variable 'AXINOM_TENANT_ID' cannot be empty".into(),
            ));
        }

        let management_key = std::env::var("AXINOM_MANAGEMENT_KEY")
            .or_else(|_| std::env::var("AXINOM_KEY_SERVICE_MANAGEMENT_KEY"))
            .map_err(|_| {
                DrmpackError::InvalidConfig(
                    "Missing required environment variable 'AXINOM_MANAGEMENT_KEY'".into(),
                )
            })?;
        let management_key = management_key.trim();
        if management_key.is_empty() {
            return Err(DrmpackError::InvalidConfig(
                "Environment variable 'AXINOM_MANAGEMENT_KEY' cannot be empty".into(),
            ));
        }

        let endpoint = std::env::var("AXINOM_ENDPOINT")
            .or_else(|_| std::env::var("AXINOM_SPEKE_ENDPOINT"))
            .map_err(|_| {
                DrmpackError::InvalidConfig(
                    "Missing required environment variable 'AXINOM_ENDPOINT' (or 'AXINOM_SPEKE_ENDPOINT')".into(),
                )
            })?;
        let endpoint = endpoint.trim();
        if endpoint.is_empty() {
            return Err(DrmpackError::InvalidConfig(
                "Environment variable 'AXINOM_ENDPOINT' cannot be empty".into(),
            ));
        }

        let mut config = Self::new(tenant_id, management_key, endpoint);

        if let Ok(override_str) = std::env::var("AXINOM_OVERRIDE_KEY_IDS") {
            let trimmed = override_str.trim();
            if !trimmed.is_empty() {
                match trimmed.to_lowercase().as_str() {
                    "true" | "1" | "t" | "yes" => config = config.with_override_key_ids(true),
                    "false" | "0" | "f" | "no" => config = config.with_override_key_ids(false),
                    _ => {
                        return Err(DrmpackError::InvalidConfig(format!(
                            "Invalid boolean value for AXINOM_OVERRIDE_KEY_IDS: '{trimmed}'"
                        )));
                    }
                }
            }
        }

        Ok(config)
    }
}

impl fmt::Debug for AxinomConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AxinomConfig")
            .field("tenant_id", &self.tenant_id)
            .field("management_key", &"[REDACTED]")
            .field("endpoint", &self.endpoint)
            .field("override_key_ids", &self.override_key_ids)
            .field("timeout", &self.timeout)
            .field("headers", &self.headers)
            .finish()
    }
}

/// Configuration for the Axinom License Proxy endpoints and connection parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxinomLicenseConfig {
    pub widevine_license_url: String,
    pub fairplay_license_url: String,
    pub playready_license_url: String,
    pub fairplay_cert_url: String,
    pub timeout: Duration,
    pub headers: reqwest::header::HeaderMap,
}

impl AxinomLicenseConfig {
    /// Create a new AxinomLicenseConfig with required license acquisition and certificate URLs.
    pub fn new(
        widevine_license_url: impl Into<String>,
        fairplay_license_url: impl Into<String>,
        playready_license_url: impl Into<String>,
        fairplay_cert_url: impl Into<String>,
    ) -> Self {
        Self {
            widevine_license_url: widevine_license_url.into(),
            fairplay_license_url: fairplay_license_url.into(),
            playready_license_url: playready_license_url.into(),
            fairplay_cert_url: fairplay_cert_url.into(),
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
        self.fairplay_cert_url = url.into();
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
        Self::validate_url("fairplay_cert_url", &self.fairplay_cert_url)?;
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

    /// Load Axinom License Proxy configuration from environment variables:
    /// - `AXINOM_WIDEVINE_LICENSE_URL` (required)
    /// - `AXINOM_FAIRPLAY_LICENSE_URL` (required)
    /// - `AXINOM_PLAYREADY_LICENSE_URL` (required)
    /// - `AXINOM_FAIRPLAY_CERT_URL` (optional — Apple-issued, not an Axinom tenant endpoint)
    pub fn from_env() -> Result<Self> {
        let wv = std::env::var("AXINOM_WIDEVINE_LICENSE_URL").map_err(|_| {
            DrmpackError::InvalidConfig(
                "Missing required environment variable 'AXINOM_WIDEVINE_LICENSE_URL'".into(),
            )
        })?;
        let wv = wv.trim();
        if wv.is_empty() {
            return Err(DrmpackError::InvalidConfig(
                "Environment variable 'AXINOM_WIDEVINE_LICENSE_URL' cannot be empty".into(),
            ));
        }

        let fp = std::env::var("AXINOM_FAIRPLAY_LICENSE_URL").map_err(|_| {
            DrmpackError::InvalidConfig(
                "Missing required environment variable 'AXINOM_FAIRPLAY_LICENSE_URL'".into(),
            )
        })?;
        let fp = fp.trim();
        if fp.is_empty() {
            return Err(DrmpackError::InvalidConfig(
                "Environment variable 'AXINOM_FAIRPLAY_LICENSE_URL' cannot be empty".into(),
            ));
        }

        let pr = std::env::var("AXINOM_PLAYREADY_LICENSE_URL").map_err(|_| {
            DrmpackError::InvalidConfig(
                "Missing required environment variable 'AXINOM_PLAYREADY_LICENSE_URL'".into(),
            )
        })?;
        let pr = pr.trim();
        if pr.is_empty() {
            return Err(DrmpackError::InvalidConfig(
                "Environment variable 'AXINOM_PLAYREADY_LICENSE_URL' cannot be empty".into(),
            ));
        }

        let cert = std::env::var("AXINOM_FAIRPLAY_CERT_URL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let config = Self::new(wv, fp, pr, cert.as_deref().unwrap_or(""));
        if let Some(ref cert_url) = cert {
            Self::validate_url("fairplay_cert_url", cert_url)?;
        }
        // Validate license URLs (always required) and timeout
        Self::validate_url("widevine_license_url", &config.widevine_license_url)?;
        Self::validate_url("fairplay_license_url", &config.fairplay_license_url)?;
        Self::validate_url("playready_license_url", &config.playready_license_url)?;
        Ok(config)
    }
}

impl From<AxinomLicenseConfig> for crate::license::config::LicenseProxyConfig {
    fn from(c: AxinomLicenseConfig) -> Self {
        let cert_url = if c.fairplay_cert_url.trim().is_empty() {
            None
        } else {
            Some(c.fairplay_cert_url)
        };
        Self {
            widevine_license_url: c.widevine_license_url,
            fairplay_license_url: c.fairplay_license_url,
            playready_license_url: c.playready_license_url,
            fairplay_cert_url: cert_url,
            timeout: c.timeout,
            headers: c.headers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_axinom_config_constructor() {
        let config = AxinomConfig::new(
            "my-tenant-id",
            "my-secret-key",
            "https://custom.axprod.net/speke",
        );
        assert_eq!(config.tenant_id, "my-tenant-id");
        assert_eq!(config.management_key, "my-secret-key");
        assert_eq!(config.endpoint, "https://custom.axprod.net/speke");
        assert!(!config.override_key_ids);
        assert_eq!(config.timeout, Duration::from_secs(10));
        assert!(config.headers.is_empty());
    }

    #[test]
    fn test_axinom_config_builder_methods() {
        let config = AxinomConfig::new("tenant", "key", "https://initial.endpoint.com/speke")
            .with_endpoint("https://custom.endpoint.com/speke")
            .with_override_key_ids(true)
            .with_timeout(Duration::from_secs(30))
            .with_header(
                reqwest::header::HeaderName::from_static("x-custom"),
                reqwest::header::HeaderValue::from_static("custom-value"),
            );

        assert_eq!(config.endpoint, "https://custom.endpoint.com/speke");
        assert!(config.override_key_ids);
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(
            config.headers.get("x-custom").and_then(|v| v.to_str().ok()),
            Some("custom-value")
        );
    }

    #[test]
    fn test_axinom_config_debug_redaction() {
        let secret = "super-confidential-management-key-987654";
        let config = AxinomConfig::new("tenant-uuid", secret, "https://endpoint.axprod.net");
        let debug_repr = format!("{:?}", config);

        assert!(
            debug_repr.contains("[REDACTED]"),
            "Debug output must contain [REDACTED]"
        );
        assert!(
            !debug_repr.contains(secret),
            "Debug output must NEVER leak the raw management_key secret"
        );
        assert!(
            debug_repr.contains("tenant-uuid"),
            "Debug output should include non-secret tenant_id"
        );
    }

    #[test]
    fn test_axinom_config_from_env_missing_tenant_id() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let prev_tenant = std::env::var("AXINOM_TENANT_ID").ok();
        let prev_key = std::env::var("AXINOM_MANAGEMENT_KEY").ok();
        let prev_ep = std::env::var("AXINOM_ENDPOINT").ok();

        std::env::remove_var("AXINOM_TENANT_ID");
        std::env::set_var("AXINOM_MANAGEMENT_KEY", "some-key");
        std::env::set_var("AXINOM_ENDPOINT", "https://custom.endpoint");

        let res = AxinomConfig::from_env();
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("AXINOM_TENANT_ID"));

        // Restore
        if let Some(t) = prev_tenant {
            std::env::set_var("AXINOM_TENANT_ID", t);
        } else {
            std::env::remove_var("AXINOM_TENANT_ID");
        }
        if let Some(k) = prev_key {
            std::env::set_var("AXINOM_MANAGEMENT_KEY", k);
        } else {
            std::env::remove_var("AXINOM_MANAGEMENT_KEY");
        }
        if let Some(e) = prev_ep {
            std::env::set_var("AXINOM_ENDPOINT", e);
        } else {
            std::env::remove_var("AXINOM_ENDPOINT");
        }
    }

    #[test]
    fn test_axinom_config_from_env_missing_management_key() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let prev_tenant = std::env::var("AXINOM_TENANT_ID").ok();
        let prev_key = std::env::var("AXINOM_MANAGEMENT_KEY").ok();
        let prev_ks_key = std::env::var("AXINOM_KEY_SERVICE_MANAGEMENT_KEY").ok();
        let prev_ep = std::env::var("AXINOM_ENDPOINT").ok();

        std::env::set_var("AXINOM_TENANT_ID", "some-tenant");
        std::env::remove_var("AXINOM_MANAGEMENT_KEY");
        std::env::remove_var("AXINOM_KEY_SERVICE_MANAGEMENT_KEY");
        std::env::set_var("AXINOM_ENDPOINT", "https://custom.endpoint");

        let res = AxinomConfig::from_env();
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .to_string()
            .contains("AXINOM_MANAGEMENT_KEY"));

        // Restore
        if let Some(t) = prev_tenant {
            std::env::set_var("AXINOM_TENANT_ID", t);
        } else {
            std::env::remove_var("AXINOM_TENANT_ID");
        }
        if let Some(k) = prev_key {
            std::env::set_var("AXINOM_MANAGEMENT_KEY", k);
        } else {
            std::env::remove_var("AXINOM_MANAGEMENT_KEY");
        }
        if let Some(k) = prev_ks_key {
            std::env::set_var("AXINOM_KEY_SERVICE_MANAGEMENT_KEY", k);
        } else {
            std::env::remove_var("AXINOM_KEY_SERVICE_MANAGEMENT_KEY");
        }
        if let Some(e) = prev_ep {
            std::env::set_var("AXINOM_ENDPOINT", e);
        } else {
            std::env::remove_var("AXINOM_ENDPOINT");
        }
    }

    #[test]
    fn test_axinom_config_from_env_missing_endpoint() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let prev_tenant = std::env::var("AXINOM_TENANT_ID").ok();
        let prev_key = std::env::var("AXINOM_MANAGEMENT_KEY").ok();
        let prev_ep = std::env::var("AXINOM_ENDPOINT").ok();
        let prev_speke_ep = std::env::var("AXINOM_SPEKE_ENDPOINT").ok();

        std::env::set_var("AXINOM_TENANT_ID", "some-tenant");
        std::env::set_var("AXINOM_MANAGEMENT_KEY", "some-key");
        std::env::remove_var("AXINOM_ENDPOINT");
        std::env::remove_var("AXINOM_SPEKE_ENDPOINT");

        let res = AxinomConfig::from_env();
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("AXINOM_ENDPOINT"));

        // Restore
        if let Some(t) = prev_tenant {
            std::env::set_var("AXINOM_TENANT_ID", t);
        } else {
            std::env::remove_var("AXINOM_TENANT_ID");
        }
        if let Some(k) = prev_key {
            std::env::set_var("AXINOM_MANAGEMENT_KEY", k);
        } else {
            std::env::remove_var("AXINOM_MANAGEMENT_KEY");
        }
        if let Some(e) = prev_ep {
            std::env::set_var("AXINOM_ENDPOINT", e);
        } else {
            std::env::remove_var("AXINOM_ENDPOINT");
        }
        if let Some(e) = prev_speke_ep {
            std::env::set_var("AXINOM_SPEKE_ENDPOINT", e);
        } else {
            std::env::remove_var("AXINOM_SPEKE_ENDPOINT");
        }
    }

    #[test]
    fn test_axinom_config_from_env_fallback_variables() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let prev_tenant = std::env::var("AXINOM_TENANT_ID").ok();
        let prev_key = std::env::var("AXINOM_MANAGEMENT_KEY").ok();
        let prev_ks_key = std::env::var("AXINOM_KEY_SERVICE_MANAGEMENT_KEY").ok();
        let prev_ep = std::env::var("AXINOM_ENDPOINT").ok();
        let prev_speke_ep = std::env::var("AXINOM_SPEKE_ENDPOINT").ok();

        std::env::set_var("AXINOM_TENANT_ID", "fallback-tenant");
        std::env::remove_var("AXINOM_MANAGEMENT_KEY");
        std::env::set_var("AXINOM_KEY_SERVICE_MANAGEMENT_KEY", "fallback-key");
        std::env::remove_var("AXINOM_ENDPOINT");
        std::env::set_var("AXINOM_SPEKE_ENDPOINT", "https://fallback.endpoint/speke");

        let cfg = AxinomConfig::from_env().expect("fallback env vars must succeed");
        assert_eq!(cfg.tenant_id, "fallback-tenant");
        assert_eq!(cfg.management_key, "fallback-key");
        assert_eq!(cfg.endpoint, "https://fallback.endpoint/speke");

        // Restore
        std::env::remove_var("AXINOM_KEY_SERVICE_MANAGEMENT_KEY");
        std::env::remove_var("AXINOM_SPEKE_ENDPOINT");
        if let Some(t) = prev_tenant {
            std::env::set_var("AXINOM_TENANT_ID", t);
        } else {
            std::env::remove_var("AXINOM_TENANT_ID");
        }
        if let Some(k) = prev_key {
            std::env::set_var("AXINOM_MANAGEMENT_KEY", k);
        } else {
            std::env::remove_var("AXINOM_MANAGEMENT_KEY");
        }
        if let Some(k) = prev_ks_key {
            std::env::set_var("AXINOM_KEY_SERVICE_MANAGEMENT_KEY", k);
        }
        if let Some(e) = prev_ep {
            std::env::set_var("AXINOM_ENDPOINT", e);
        }
        if let Some(e) = prev_speke_ep {
            std::env::set_var("AXINOM_SPEKE_ENDPOINT", e);
        }
    }

    #[test]
    fn test_axinom_config_from_env_valid() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let prev_tenant = std::env::var("AXINOM_TENANT_ID").ok();
        let prev_key = std::env::var("AXINOM_MANAGEMENT_KEY").ok();
        let prev_ep = std::env::var("AXINOM_ENDPOINT").ok();
        let prev_ov = std::env::var("AXINOM_OVERRIDE_KEY_IDS").ok();

        std::env::set_var("AXINOM_TENANT_ID", "env-tenant-123");
        std::env::set_var("AXINOM_MANAGEMENT_KEY", "env-key-456");
        std::env::set_var("AXINOM_ENDPOINT", "https://custom.axprod.net/api");
        std::env::set_var("AXINOM_OVERRIDE_KEY_IDS", "true");

        let cfg = AxinomConfig::from_env().expect("from_env must succeed");
        assert_eq!(cfg.tenant_id, "env-tenant-123");
        assert_eq!(cfg.management_key, "env-key-456");
        assert_eq!(cfg.endpoint, "https://custom.axprod.net/api");
        assert!(cfg.override_key_ids);

        // Restore
        std::env::remove_var("AXINOM_ENDPOINT");
        std::env::remove_var("AXINOM_OVERRIDE_KEY_IDS");
        if let Some(t) = prev_tenant {
            std::env::set_var("AXINOM_TENANT_ID", t);
        } else {
            std::env::remove_var("AXINOM_TENANT_ID");
        }
        if let Some(k) = prev_key {
            std::env::set_var("AXINOM_MANAGEMENT_KEY", k);
        } else {
            std::env::remove_var("AXINOM_MANAGEMENT_KEY");
        }
        if let Some(e) = prev_ep {
            std::env::set_var("AXINOM_ENDPOINT", e);
        }
        if let Some(o) = prev_ov {
            std::env::set_var("AXINOM_OVERRIDE_KEY_IDS", o);
        }
    }

    #[test]
    fn test_axinom_config_from_env_invalid_override_bool() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let prev_tenant = std::env::var("AXINOM_TENANT_ID").ok();
        let prev_key = std::env::var("AXINOM_MANAGEMENT_KEY").ok();
        let prev_ep = std::env::var("AXINOM_ENDPOINT").ok();
        let prev_ov = std::env::var("AXINOM_OVERRIDE_KEY_IDS").ok();

        std::env::set_var("AXINOM_TENANT_ID", "env-tenant");
        std::env::set_var("AXINOM_MANAGEMENT_KEY", "env-key");
        std::env::set_var("AXINOM_ENDPOINT", "https://custom.endpoint");
        std::env::set_var("AXINOM_OVERRIDE_KEY_IDS", "not_a_bool");

        let res = AxinomConfig::from_env();
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .to_string()
            .contains("Invalid boolean value"));

        // Restore
        if let Some(o) = prev_ov {
            std::env::set_var("AXINOM_OVERRIDE_KEY_IDS", o);
        } else {
            std::env::remove_var("AXINOM_OVERRIDE_KEY_IDS");
        }
        if let Some(t) = prev_tenant {
            std::env::set_var("AXINOM_TENANT_ID", t);
        } else {
            std::env::remove_var("AXINOM_TENANT_ID");
        }
        if let Some(k) = prev_key {
            std::env::set_var("AXINOM_MANAGEMENT_KEY", k);
        } else {
            std::env::remove_var("AXINOM_MANAGEMENT_KEY");
        }
        if let Some(e) = prev_ep {
            std::env::set_var("AXINOM_ENDPOINT", e);
        } else {
            std::env::remove_var("AXINOM_ENDPOINT");
        }
    }

    #[test]
    fn test_axinom_config_from_env_empty_strings() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let prev_tenant = std::env::var("AXINOM_TENANT_ID").ok();
        let prev_key = std::env::var("AXINOM_MANAGEMENT_KEY").ok();
        let prev_ep = std::env::var("AXINOM_ENDPOINT").ok();

        // Empty tenant ID
        std::env::set_var("AXINOM_TENANT_ID", "   ");
        std::env::set_var("AXINOM_MANAGEMENT_KEY", "valid-key");
        std::env::set_var("AXINOM_ENDPOINT", "https://valid.endpoint");
        assert!(AxinomConfig::from_env().is_err());

        // Empty management key
        std::env::set_var("AXINOM_TENANT_ID", "valid-tenant");
        std::env::set_var("AXINOM_MANAGEMENT_KEY", "   ");
        std::env::set_var("AXINOM_ENDPOINT", "https://valid.endpoint");
        assert!(AxinomConfig::from_env().is_err());

        // Empty endpoint
        std::env::set_var("AXINOM_TENANT_ID", "valid-tenant");
        std::env::set_var("AXINOM_MANAGEMENT_KEY", "valid-key");
        std::env::set_var("AXINOM_ENDPOINT", "   ");
        assert!(AxinomConfig::from_env().is_err());

        // Restore
        if let Some(t) = prev_tenant {
            std::env::set_var("AXINOM_TENANT_ID", t);
        } else {
            std::env::remove_var("AXINOM_TENANT_ID");
        }
        if let Some(k) = prev_key {
            std::env::set_var("AXINOM_MANAGEMENT_KEY", k);
        } else {
            std::env::remove_var("AXINOM_MANAGEMENT_KEY");
        }
        if let Some(e) = prev_ep {
            std::env::set_var("AXINOM_ENDPOINT", e);
        } else {
            std::env::remove_var("AXINOM_ENDPOINT");
        }
    }

    #[test]
    fn test_axinom_license_config_constructor() {
        let config = AxinomLicenseConfig::new(
            "https://custom.axprod.net/widevine",
            "https://custom.axprod.net/fairplay",
            "https://custom.axprod.net/playready",
            "https://custom.axprod.net/fairplay.cer",
        );
        assert_eq!(
            config.widevine_license_url,
            "https://custom.axprod.net/widevine"
        );
        assert_eq!(
            config.fairplay_license_url,
            "https://custom.axprod.net/fairplay"
        );
        assert_eq!(
            config.playready_license_url,
            "https://custom.axprod.net/playready"
        );
        assert_eq!(
            config.fairplay_cert_url,
            "https://custom.axprod.net/fairplay.cer"
        );
        assert_eq!(config.timeout, Duration::from_secs(10));
        assert!(config.headers.is_empty());
    }

    #[test]
    fn test_axinom_license_config_builder() {
        let config = AxinomLicenseConfig::new(
            "https://initial/wv",
            "https://initial/fp",
            "https://initial/pr",
            "https://initial/cert",
        )
        .with_widevine_license_url("https://custom.axprod.net/widevine")
        .with_fairplay_license_url("https://custom.axprod.net/fairplay")
        .with_playready_license_url("https://custom.axprod.net/playready")
        .with_fairplay_cert_url("https://custom.axprod.net/cert.cer")
        .with_timeout(Duration::from_secs(25))
        .with_header(
            reqwest::header::HeaderName::from_static("x-custom-tracking"),
            reqwest::header::HeaderValue::from_static("test-track-id"),
        );

        assert_eq!(
            config.widevine_license_url,
            "https://custom.axprod.net/widevine"
        );
        assert_eq!(
            config.fairplay_license_url,
            "https://custom.axprod.net/fairplay"
        );
        assert_eq!(
            config.playready_license_url,
            "https://custom.axprod.net/playready"
        );
        assert_eq!(
            config.fairplay_cert_url,
            "https://custom.axprod.net/cert.cer"
        );
        assert_eq!(config.timeout, Duration::from_secs(25));
        assert_eq!(
            config
                .headers
                .get("x-custom-tracking")
                .and_then(|v| v.to_str().ok()),
            Some("test-track-id")
        );
    }

    #[test]
    fn test_axinom_license_config_from_env_all() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let prev_wv = std::env::var("AXINOM_WIDEVINE_LICENSE_URL").ok();
        let prev_fp = std::env::var("AXINOM_FAIRPLAY_LICENSE_URL").ok();
        let prev_pr = std::env::var("AXINOM_PLAYREADY_LICENSE_URL").ok();
        let prev_cert = std::env::var("AXINOM_FAIRPLAY_CERT_URL").ok();

        std::env::set_var("AXINOM_WIDEVINE_LICENSE_URL", "https://env.axprod.net/wv");
        std::env::set_var("AXINOM_FAIRPLAY_LICENSE_URL", "https://env.axprod.net/fp");
        std::env::set_var("AXINOM_PLAYREADY_LICENSE_URL", "https://env.axprod.net/pr");
        std::env::set_var(
            "AXINOM_FAIRPLAY_CERT_URL",
            "https://env.axprod.net/cert.der",
        );

        let cfg = AxinomLicenseConfig::from_env().expect("from_env must succeed");
        assert_eq!(cfg.widevine_license_url, "https://env.axprod.net/wv");
        assert_eq!(cfg.fairplay_license_url, "https://env.axprod.net/fp");
        assert_eq!(cfg.playready_license_url, "https://env.axprod.net/pr");
        assert_eq!(cfg.fairplay_cert_url, "https://env.axprod.net/cert.der");

        // Restore
        if let Some(v) = prev_wv {
            std::env::set_var("AXINOM_WIDEVINE_LICENSE_URL", v);
        } else {
            std::env::remove_var("AXINOM_WIDEVINE_LICENSE_URL");
        }
        if let Some(v) = prev_fp {
            std::env::set_var("AXINOM_FAIRPLAY_LICENSE_URL", v);
        } else {
            std::env::remove_var("AXINOM_FAIRPLAY_LICENSE_URL");
        }
        if let Some(v) = prev_pr {
            std::env::set_var("AXINOM_PLAYREADY_LICENSE_URL", v);
        } else {
            std::env::remove_var("AXINOM_PLAYREADY_LICENSE_URL");
        }
        if let Some(v) = prev_cert {
            std::env::set_var("AXINOM_FAIRPLAY_CERT_URL", v);
        } else {
            std::env::remove_var("AXINOM_FAIRPLAY_CERT_URL");
        }
    }

    #[test]
    fn test_axinom_license_config_from_env_cert_url_optional() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let prev_wv = std::env::var("AXINOM_WIDEVINE_LICENSE_URL").ok();
        let prev_fp = std::env::var("AXINOM_FAIRPLAY_LICENSE_URL").ok();
        let prev_pr = std::env::var("AXINOM_PLAYREADY_LICENSE_URL").ok();
        let prev_cert = std::env::var("AXINOM_FAIRPLAY_CERT_URL").ok();

        std::env::set_var("AXINOM_WIDEVINE_LICENSE_URL", "https://env.axprod.net/wv");
        std::env::set_var("AXINOM_FAIRPLAY_LICENSE_URL", "https://env.axprod.net/fp");
        std::env::set_var("AXINOM_PLAYREADY_LICENSE_URL", "https://env.axprod.net/pr");
        std::env::remove_var("AXINOM_FAIRPLAY_CERT_URL");

        // Should succeed: cert_url is optional
        let res = AxinomLicenseConfig::from_env();
        assert!(res.is_ok());
        let cfg = res.unwrap();
        assert!(cfg.fairplay_cert_url.is_empty());

        // Restore
        if let Some(v) = prev_wv {
            std::env::set_var("AXINOM_WIDEVINE_LICENSE_URL", v);
        } else {
            std::env::remove_var("AXINOM_WIDEVINE_LICENSE_URL");
        }
        if let Some(v) = prev_fp {
            std::env::set_var("AXINOM_FAIRPLAY_LICENSE_URL", v);
        } else {
            std::env::remove_var("AXINOM_FAIRPLAY_LICENSE_URL");
        }
        if let Some(v) = prev_pr {
            std::env::set_var("AXINOM_PLAYREADY_LICENSE_URL", v);
        } else {
            std::env::remove_var("AXINOM_PLAYREADY_LICENSE_URL");
        }
        if let Some(v) = prev_cert {
            std::env::set_var("AXINOM_FAIRPLAY_CERT_URL", v);
        } else {
            std::env::remove_var("AXINOM_FAIRPLAY_CERT_URL");
        }
    }

    #[test]
    fn test_axinom_license_config_from_env_missing_license_url_fails() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let prev_wv = std::env::var("AXINOM_WIDEVINE_LICENSE_URL").ok();
        let prev_fp = std::env::var("AXINOM_FAIRPLAY_LICENSE_URL").ok();
        let prev_pr = std::env::var("AXINOM_PLAYREADY_LICENSE_URL").ok();
        let prev_cert = std::env::var("AXINOM_FAIRPLAY_CERT_URL").ok();

        // Missing WIDEVINE_LICENSE_URL should fail
        std::env::remove_var("AXINOM_WIDEVINE_LICENSE_URL");
        std::env::set_var("AXINOM_FAIRPLAY_LICENSE_URL", "https://env.axprod.net/fp");
        std::env::set_var("AXINOM_PLAYREADY_LICENSE_URL", "https://env.axprod.net/pr");
        std::env::remove_var("AXINOM_FAIRPLAY_CERT_URL");

        let res = AxinomLicenseConfig::from_env();
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .to_string()
            .contains("AXINOM_WIDEVINE_LICENSE_URL"));

        // Restore
        if let Some(v) = prev_wv {
            std::env::set_var("AXINOM_WIDEVINE_LICENSE_URL", v);
        } else {
            std::env::remove_var("AXINOM_WIDEVINE_LICENSE_URL");
        }
        if let Some(v) = prev_fp {
            std::env::set_var("AXINOM_FAIRPLAY_LICENSE_URL", v);
        } else {
            std::env::remove_var("AXINOM_FAIRPLAY_LICENSE_URL");
        }
        if let Some(v) = prev_pr {
            std::env::set_var("AXINOM_PLAYREADY_LICENSE_URL", v);
        } else {
            std::env::remove_var("AXINOM_PLAYREADY_LICENSE_URL");
        }
        if let Some(v) = prev_cert {
            std::env::set_var("AXINOM_FAIRPLAY_CERT_URL", v);
        } else {
            std::env::remove_var("AXINOM_FAIRPLAY_CERT_URL");
        }
    }

    #[test]
    fn test_axinom_license_config_from_env_empty() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let prev_wv = std::env::var("AXINOM_WIDEVINE_LICENSE_URL").ok();
        let prev_fp = std::env::var("AXINOM_FAIRPLAY_LICENSE_URL").ok();
        let prev_pr = std::env::var("AXINOM_PLAYREADY_LICENSE_URL").ok();
        let prev_cert = std::env::var("AXINOM_FAIRPLAY_CERT_URL").ok();

        std::env::set_var("AXINOM_WIDEVINE_LICENSE_URL", "   ");
        std::env::set_var("AXINOM_FAIRPLAY_LICENSE_URL", "https://env.axprod.net/fp");
        std::env::set_var("AXINOM_PLAYREADY_LICENSE_URL", "https://env.axprod.net/pr");
        std::env::set_var(
            "AXINOM_FAIRPLAY_CERT_URL",
            "https://env.axprod.net/cert.der",
        );

        let res = AxinomLicenseConfig::from_env();
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .to_string()
            .contains("AXINOM_WIDEVINE_LICENSE_URL"));

        // Restore
        if let Some(v) = prev_wv {
            std::env::set_var("AXINOM_WIDEVINE_LICENSE_URL", v);
        } else {
            std::env::remove_var("AXINOM_WIDEVINE_LICENSE_URL");
        }
        if let Some(v) = prev_fp {
            std::env::set_var("AXINOM_FAIRPLAY_LICENSE_URL", v);
        } else {
            std::env::remove_var("AXINOM_FAIRPLAY_LICENSE_URL");
        }
        if let Some(v) = prev_pr {
            std::env::set_var("AXINOM_PLAYREADY_LICENSE_URL", v);
        } else {
            std::env::remove_var("AXINOM_PLAYREADY_LICENSE_URL");
        }
        if let Some(v) = prev_cert {
            std::env::set_var("AXINOM_FAIRPLAY_CERT_URL", v);
        } else {
            std::env::remove_var("AXINOM_FAIRPLAY_CERT_URL");
        }
    }

    #[test]
    fn test_axinom_license_config_from_env_invalid_url() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let prev_wv = std::env::var("AXINOM_WIDEVINE_LICENSE_URL").ok();
        let prev_fp = std::env::var("AXINOM_FAIRPLAY_LICENSE_URL").ok();
        let prev_pr = std::env::var("AXINOM_PLAYREADY_LICENSE_URL").ok();
        let prev_cert = std::env::var("AXINOM_FAIRPLAY_CERT_URL").ok();

        std::env::set_var("AXINOM_WIDEVINE_LICENSE_URL", "ftp://invalid-url.net");
        std::env::set_var("AXINOM_FAIRPLAY_LICENSE_URL", "https://env.axprod.net/fp");
        std::env::set_var("AXINOM_PLAYREADY_LICENSE_URL", "https://env.axprod.net/pr");
        std::env::set_var(
            "AXINOM_FAIRPLAY_CERT_URL",
            "https://env.axprod.net/cert.der",
        );

        let res = AxinomLicenseConfig::from_env();
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("http:// or https://"));

        // Restore
        if let Some(v) = prev_wv {
            std::env::set_var("AXINOM_WIDEVINE_LICENSE_URL", v);
        } else {
            std::env::remove_var("AXINOM_WIDEVINE_LICENSE_URL");
        }
        if let Some(v) = prev_fp {
            std::env::set_var("AXINOM_FAIRPLAY_LICENSE_URL", v);
        } else {
            std::env::remove_var("AXINOM_FAIRPLAY_LICENSE_URL");
        }
        if let Some(v) = prev_pr {
            std::env::set_var("AXINOM_PLAYREADY_LICENSE_URL", v);
        } else {
            std::env::remove_var("AXINOM_PLAYREADY_LICENSE_URL");
        }
        if let Some(v) = prev_cert {
            std::env::set_var("AXINOM_FAIRPLAY_CERT_URL", v);
        } else {
            std::env::remove_var("AXINOM_FAIRPLAY_CERT_URL");
        }
    }

    #[test]
    fn test_axinom_license_config_validation() {
        let valid = AxinomLicenseConfig::new(
            "https://valid/wv",
            "https://valid/fp",
            "https://valid/pr",
            "https://valid/cert",
        );
        assert!(valid.validate().is_ok());
        assert_eq!(valid.timeout, Duration::from_secs(10));
        assert!(valid.headers.is_empty());

        let invalid_url = valid.clone().with_widevine_license_url("not-a-valid-url");
        assert!(invalid_url.validate().is_err());

        let empty_fp = valid.clone().with_fairplay_license_url("");
        assert!(empty_fp.validate().is_err());

        let zero_timeout = valid.clone().with_timeout(Duration::ZERO);
        assert!(zero_timeout.validate().is_err());
    }

    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
