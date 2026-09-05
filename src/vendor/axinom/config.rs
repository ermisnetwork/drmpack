use crate::error::{DrmpackError, Result};
use std::fmt;
use std::time::Duration;

/// Default Axinom SPEKE v2 endpoint URL.
pub const DEFAULT_AXINOM_ENDPOINT: &str = "https://key-server-management.axprod.net/api/SpekeV2";

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
    /// Create a new Axinom configuration with required tenant ID and management key.
    ///
    /// Endpoint defaults to `DEFAULT_AXINOM_ENDPOINT` (`https://key-server-management.axprod.net/api/SpekeV2`),
    /// `override_key_ids` defaults to `false`, and `timeout` defaults to 10 seconds.
    pub fn new(tenant_id: impl Into<String>, management_key: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            management_key: management_key.into(),
            endpoint: DEFAULT_AXINOM_ENDPOINT.to_string(),
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
    /// - `AXINOM_ENDPOINT` (optional, overrides default endpoint)
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

        let mut config = Self::new(tenant_id, management_key);

        if let Ok(endpoint) =
            std::env::var("AXINOM_ENDPOINT").or_else(|_| std::env::var("AXINOM_SPEKE_ENDPOINT"))
        {
            let endpoint = endpoint.trim();
            if !endpoint.is_empty() {
                config = config.with_endpoint(endpoint);
            }
        }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_axinom_config_defaults() {
        let config = AxinomConfig::new("my-tenant-id", "my-secret-key");
        assert_eq!(config.tenant_id, "my-tenant-id");
        assert_eq!(config.management_key, "my-secret-key");
        assert_eq!(config.endpoint, DEFAULT_AXINOM_ENDPOINT);
        assert!(!config.override_key_ids);
        assert_eq!(config.timeout, Duration::from_secs(10));
        assert!(config.headers.is_empty());
    }

    #[test]
    fn test_axinom_config_builder_methods() {
        let config = AxinomConfig::new("tenant", "key")
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
        let config = AxinomConfig::new("tenant-uuid", secret);
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

        std::env::remove_var("AXINOM_TENANT_ID");
        std::env::set_var("AXINOM_MANAGEMENT_KEY", "some-key");

        let res = AxinomConfig::from_env();
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("AXINOM_TENANT_ID"));

        // Restore
        if let Some(t) = prev_tenant {
            std::env::set_var("AXINOM_TENANT_ID", t);
        }
        if let Some(k) = prev_key {
            std::env::set_var("AXINOM_MANAGEMENT_KEY", k);
        }
    }

    #[test]
    fn test_axinom_config_from_env_missing_management_key() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let prev_tenant = std::env::var("AXINOM_TENANT_ID").ok();
        let prev_key = std::env::var("AXINOM_MANAGEMENT_KEY").ok();
        let prev_ks_key = std::env::var("AXINOM_KEY_SERVICE_MANAGEMENT_KEY").ok();

        std::env::set_var("AXINOM_TENANT_ID", "some-tenant");
        std::env::remove_var("AXINOM_MANAGEMENT_KEY");
        std::env::remove_var("AXINOM_KEY_SERVICE_MANAGEMENT_KEY");

        let res = AxinomConfig::from_env();
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .to_string()
            .contains("AXINOM_MANAGEMENT_KEY"));

        // Restore
        if let Some(t) = prev_tenant {
            std::env::set_var("AXINOM_TENANT_ID", t);
        }
        if let Some(k) = prev_key {
            std::env::set_var("AXINOM_MANAGEMENT_KEY", k);
        }
        if let Some(k) = prev_ks_key {
            std::env::set_var("AXINOM_KEY_SERVICE_MANAGEMENT_KEY", k);
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
        let prev_ov = std::env::var("AXINOM_OVERRIDE_KEY_IDS").ok();

        std::env::set_var("AXINOM_TENANT_ID", "env-tenant");
        std::env::set_var("AXINOM_MANAGEMENT_KEY", "env-key");
        std::env::set_var("AXINOM_OVERRIDE_KEY_IDS", "not_a_bool");

        let res = AxinomConfig::from_env();
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .to_string()
            .contains("Invalid boolean value"));

        // Restore
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
        if let Some(o) = prev_ov {
            std::env::set_var("AXINOM_OVERRIDE_KEY_IDS", o);
        }
    }

    #[test]
    fn test_axinom_config_from_env_empty_strings() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let prev_tenant = std::env::var("AXINOM_TENANT_ID").ok();
        let prev_key = std::env::var("AXINOM_MANAGEMENT_KEY").ok();

        // Empty tenant ID
        std::env::set_var("AXINOM_TENANT_ID", "   ");
        std::env::set_var("AXINOM_MANAGEMENT_KEY", "valid-key");
        assert!(AxinomConfig::from_env().is_err());

        // Empty management key
        std::env::set_var("AXINOM_TENANT_ID", "valid-tenant");
        std::env::set_var("AXINOM_MANAGEMENT_KEY", "   ");
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
    }

    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
