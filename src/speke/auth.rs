use base64::prelude::*;
use std::fmt;

/// Authentication mechanism for AWS SPEKE v2.0 requests.
#[derive(Clone, PartialEq, Eq)]
pub enum SpekeAuth {
    /// HTTP Basic authentication with username and password.
    Basic { username: String, password: String },
    /// HTTP Bearer token authentication.
    Bearer(String),
    /// Custom API key header authentication.
    ApiKey { header_name: String, key: String },
    /// AWS SigV4 static signature or pre-signed authorization header.
    SigV4 {
        /// The `Authorization` header value (e.g. `AWS4-HMAC-SHA256 Credential=...`).
        authorization: String,
        /// Optional `x-amz-security-token` header value for temporary STS credentials.
        security_token: Option<String>,
        /// Optional `x-amz-date` header value (e.g. `20260905T120000Z`).
        date: Option<String>,
        /// Optional `x-amz-content-sha256` payload hash header value.
        content_sha256: Option<String>,
    },
}

/// Trait for custom or dynamic request signers (e.g. AWS SigV4 signer).
pub trait SpekeSigner: Send + Sync {
    /// Sign an outgoing HTTP request before sending.
    /// Receives the request builder, the target endpoint URL, and the XML request body.
    fn sign(
        &self,
        builder: reqwest::RequestBuilder,
        endpoint: &str,
        body: &str,
    ) -> reqwest::RequestBuilder;
}

impl<F> SpekeSigner for F
where
    F: Fn(reqwest::RequestBuilder, &str, &str) -> reqwest::RequestBuilder + Send + Sync,
{
    fn sign(
        &self,
        builder: reqwest::RequestBuilder,
        endpoint: &str,
        body: &str,
    ) -> reqwest::RequestBuilder {
        self(builder, endpoint, body)
    }
}

impl SpekeAuth {
    /// Create HTTP Basic authentication.
    pub fn basic(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self::Basic {
            username: username.into(),
            password: password.into(),
        }
    }

    /// Create Bearer token authentication.
    pub fn bearer(token: impl Into<String>) -> Self {
        Self::Bearer(token.into())
    }

    /// Create API Key header authentication with custom header name.
    pub fn api_key(header_name: impl Into<String>, key: impl Into<String>) -> Self {
        Self::ApiKey {
            header_name: header_name.into(),
            key: key.into(),
        }
    }

    /// Create standard AWS API Key (`x-api-key`) header authentication.
    pub fn x_api_key(key: impl Into<String>) -> Self {
        Self::api_key("x-api-key", key)
    }

    /// Create AWS SigV4 authorization with `Authorization` header, optional session token, and optional x-amz-date.
    pub fn sigv4(
        authorization: impl Into<String>,
        security_token: Option<impl Into<String>>,
        date: Option<impl Into<String>>,
    ) -> Self {
        Self::SigV4 {
            authorization: authorization.into(),
            security_token: security_token.map(|s| s.into()),
            date: date.map(|d| d.into()),
            content_sha256: None,
        }
    }

    /// Create AWS SigV4 authorization with `Authorization` header, session token, x-amz-date, and payload hash.
    pub fn sigv4_with_payload_hash(
        authorization: impl Into<String>,
        security_token: Option<impl Into<String>>,
        date: Option<impl Into<String>>,
        content_sha256: Option<impl Into<String>>,
    ) -> Self {
        Self::SigV4 {
            authorization: authorization.into(),
            security_token: security_token.map(|s| s.into()),
            date: date.map(|d| d.into()),
            content_sha256: content_sha256.map(|s| s.into()),
        }
    }

    /// Apply authentication credentials to an outgoing HTTP request builder.
    pub fn apply(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            SpekeAuth::Basic { username, password } => {
                let credentials = format!("{}:{}", username, password);
                let auth_value =
                    format!("Basic {}", BASE64_STANDARD.encode(credentials.as_bytes()));
                builder.header(reqwest::header::AUTHORIZATION, auth_value)
            }
            SpekeAuth::Bearer(token) => {
                builder.header(reqwest::header::AUTHORIZATION, format!("Bearer {}", token))
            }
            SpekeAuth::ApiKey { header_name, key } => {
                builder.header(header_name.as_str(), key.as_str())
            }
            SpekeAuth::SigV4 {
                authorization,
                security_token,
                date,
                content_sha256,
            } => {
                let mut b = builder.header(reqwest::header::AUTHORIZATION, authorization);
                if let Some(token) = security_token {
                    b = b.header("x-amz-security-token", token);
                }
                if let Some(d) = date {
                    b = b.header("x-amz-date", d);
                }
                if let Some(sha) = content_sha256 {
                    b = b.header("x-amz-content-sha256", sha);
                }
                b
            }
        }
    }
}

impl fmt::Debug for SpekeAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpekeAuth::Basic { username, .. } => f
                .debug_struct("SpekeAuth::Basic")
                .field("username", username)
                .field("password", &"[REDACTED]")
                .finish(),
            SpekeAuth::Bearer(_) => f
                .debug_tuple("SpekeAuth::Bearer")
                .field(&"[REDACTED]")
                .finish(),
            SpekeAuth::ApiKey { header_name, .. } => f
                .debug_struct("SpekeAuth::ApiKey")
                .field("header_name", header_name)
                .field("key", &"[REDACTED]")
                .finish(),
            SpekeAuth::SigV4 {
                authorization: _,
                security_token,
                date,
                content_sha256,
            } => f
                .debug_struct("SpekeAuth::SigV4")
                .field("authorization", &"[REDACTED]")
                .field(
                    "security_token",
                    &security_token.as_ref().map(|_| "[REDACTED]"),
                )
                .field("date", date)
                .field("content_sha256", content_sha256)
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speke_auth_debug_redaction() {
        let auth_basic = SpekeAuth::basic("my-user", "super-secret-password");
        let debug_basic = format!("{:?}", auth_basic);
        assert!(debug_basic.contains("[REDACTED]"));
        assert!(!debug_basic.contains("super-secret-password"));
        assert!(debug_basic.contains("my-user"));

        let auth_bearer = SpekeAuth::bearer("jwt-secret-token-xyz");
        let debug_bearer = format!("{:?}", auth_bearer);
        assert!(debug_bearer.contains("[REDACTED]"));
        assert!(!debug_bearer.contains("jwt-secret-token-xyz"));

        let auth_api_key = SpekeAuth::api_key("x-api-key", "key-secret-1234");
        let debug_api_key = format!("{:?}", auth_api_key);
        assert!(debug_api_key.contains("[REDACTED]"));
        assert!(!debug_api_key.contains("key-secret-1234"));
        assert!(debug_api_key.contains("x-api-key"));

        let auth_sigv4 = SpekeAuth::sigv4_with_payload_hash(
            "AWS4-HMAC-SHA256 Credential=AKIA...",
            Some("session-token-secret"),
            Some("20260905T120000Z"),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        );
        let debug_sigv4 = format!("{:?}", auth_sigv4);
        assert!(debug_sigv4.contains("[REDACTED]"));
        assert!(!debug_sigv4.contains("AKIA"));
        assert!(!debug_sigv4.contains("session-token-secret"));
        assert!(debug_sigv4.contains("20260905T120000Z"));
        assert!(debug_sigv4
            .contains("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"));
    }

    #[test]
    fn test_speke_auth_x_api_key_helper() {
        let auth = SpekeAuth::x_api_key("my-api-key-value");
        assert_eq!(
            auth,
            SpekeAuth::ApiKey {
                header_name: "x-api-key".into(),
                key: "my-api-key-value".into(),
            }
        );
    }
}
