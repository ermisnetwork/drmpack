use std::ops::Deref;

/// Structured response container returned by license proxy handlers.
///
/// Encapsulates the raw DRM license payload (`bytes::Bytes`), optional upstream Content-Type,
/// and all upstream response headers (such as Axinom's `X-AxDRM-Message` used for device tracking).
#[derive(Clone, Debug, PartialEq)]
pub struct LicenseResponse {
    /// Raw DRM license response payload bytes.
    pub data: bytes::Bytes,
    /// MIME type returned by the upstream license service.
    pub content_type: Option<String>,
    /// Response headers returned by the upstream license service.
    pub headers: reqwest::header::HeaderMap,
}

impl LicenseResponse {
    /// Create a new LicenseResponse.
    pub fn new(
        data: bytes::Bytes,
        content_type: Option<String>,
        headers: reqwest::header::HeaderMap,
    ) -> Self {
        Self {
            data,
            content_type,
            headers,
        }
    }

    /// Extract the `X-AxDRM-Message` header value if present.
    ///
    /// Axinom DRM returns this header with device identification and tracking information.
    pub fn axdrm_message(&self) -> Option<&str> {
        self.headers
            .get("x-axdrm-message")
            .and_then(|v| v.to_str().ok())
    }

    /// Consume the response and return the underlying byte payload.
    pub fn into_bytes(self) -> bytes::Bytes {
        self.data
    }

    /// Borrow the underlying byte payload as a slice.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Borrow the underlying byte payload.
    pub fn data(&self) -> &bytes::Bytes {
        &self.data
    }

    /// Get the upstream Content-Type header if present.
    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    /// Get a reference to all upstream headers.
    pub fn headers(&self) -> &reqwest::header::HeaderMap {
        &self.headers
    }
}

impl Deref for LicenseResponse {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl AsRef<[u8]> for LicenseResponse {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl std::borrow::Borrow<[u8]> for LicenseResponse {
    fn borrow(&self) -> &[u8] {
        &self.data
    }
}

impl From<LicenseResponse> for bytes::Bytes {
    fn from(resp: LicenseResponse) -> Self {
        resp.data
    }
}

impl From<LicenseResponse> for Vec<u8> {
    fn from(resp: LicenseResponse) -> Self {
        resp.data.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

    #[test]
    fn test_license_response_deref_and_as_ref() {
        let payload = b"encrypted-license-content";
        let resp = LicenseResponse::new(
            bytes::Bytes::from_static(payload),
            Some("application/octet-stream".to_string()),
            HeaderMap::new(),
        );

        assert_eq!(&resp[..], payload);
        assert_eq!(resp.as_bytes(), payload);
        assert_eq!(resp.as_ref(), payload);
        assert_eq!(std::borrow::Borrow::<[u8]>::borrow(&resp), payload);
        assert_eq!(resp.len(), payload.len());
        assert!(!resp.is_empty());
        assert_eq!(resp.content_type(), Some("application/octet-stream"));
        assert_eq!(resp.data(), &bytes::Bytes::from_static(payload));
        assert!(resp.headers().is_empty());
    }

    #[test]
    fn test_license_response_into_bytes_and_from() {
        let payload = b"sample-bytes";
        let resp = LicenseResponse::new(bytes::Bytes::from_static(payload), None, HeaderMap::new());

        let resp_clone = resp.clone();
        assert_eq!(resp, resp_clone);

        let data: bytes::Bytes = resp.into();
        assert_eq!(data.as_ref(), payload);

        let vec_data: Vec<u8> = resp_clone.into();
        assert_eq!(vec_data.as_slice(), payload);
    }

    #[test]
    fn test_license_response_axdrm_message() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-axdrm-message"),
            HeaderValue::from_static("device-tracking-jwt-token-12345"),
        );

        let resp = LicenseResponse::new(
            bytes::Bytes::from_static(b"data"),
            Some("application/octet-stream".to_string()),
            headers,
        );

        assert_eq!(
            resp.axdrm_message(),
            Some("device-tracking-jwt-token-12345")
        );
    }

    #[test]
    fn test_license_response_axdrm_message_missing() {
        let resp = LicenseResponse::new(bytes::Bytes::from_static(b"data"), None, HeaderMap::new());

        assert_eq!(resp.axdrm_message(), None);
    }
}
