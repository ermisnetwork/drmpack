#![cfg(feature = "speke-v2")]

use base64::prelude::*;
use drmpack::error::DrmpackError;
use drmpack::key::{ContentKey, KeyID, KeyProvider, KeyRequest, StaticKeySource};
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::speke::{SpekeAuth, SpekeClient, SpekeConfig};
use drmpack::types::{DrmSystem, EncryptionScheme, QualityTier, Rendition, TrackType};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex};
use uuid::Uuid;

#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

struct SpekeMockServer {
    url: String,
    recorded_requests: Arc<Mutex<Vec<RecordedRequest>>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl SpekeMockServer {
    async fn start(status: u16, response_body: String) -> Self {
        Self::start_with_headers(status, response_body, Vec::new()).await
    }

    async fn start_with_headers(
        status: u16,
        response_body: String,
        custom_headers: Vec<(&'static str, &'static str)>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let recorded_requests = Arc::new(Mutex::new(Vec::new()));
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let reqs = Arc::clone(&recorded_requests);
        let resp_body = response_body.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accept_result = listener.accept() => {
                        let Ok((mut stream, _)) = accept_result else { break };
                        let reqs = Arc::clone(&reqs);
                        let body_data = resp_body.clone();
                        let extra_hdrs = custom_headers.clone();

                        tokio::spawn(async move {
                            let mut buf = Vec::new();
                            let mut chunk = [0u8; 2048];
                            let mut content_length = 0;
                            let mut header_end = None;

                            loop {
                                let n = stream.read(&mut chunk).await.unwrap_or(0);
                                if n == 0 { break; }
                                buf.extend_from_slice(&chunk[..n]);

                                if header_end.is_none() {
                                    if let Some(pos) =
                                        buf.windows(4).position(|w| w == b"\r\n\r\n")
                                    {
                                        header_end = Some(pos + 4);
                                        let header_str = String::from_utf8_lossy(&buf[..pos]);
                                        for line in header_str.lines() {
                                            if let Some((k, v)) = line.split_once(':') {
                                                if k.trim().eq_ignore_ascii_case("content-length") {
                                                    content_length =
                                                        v.trim().parse::<usize>().unwrap_or(0);
                                                }
                                            }
                                        }
                                    }
                                }

                                if let Some(end) = header_end {
                                    if buf.len() >= end + content_length {
                                        break;
                                    }
                                }
                            }

                            if let Some(end) = header_end {
                                let header_str = String::from_utf8_lossy(&buf[..end - 4]);
                                let body_str =
                                    String::from_utf8_lossy(&buf[end..end + content_length])
                                        .to_string();

                                let mut lines = header_str.lines();
                                let first_line = lines.next().unwrap_or("");
                                let mut parts = first_line.split_whitespace();
                                let method = parts.next().unwrap_or("").to_string();
                                let path = parts.next().unwrap_or("").to_string();

                                let mut headers = HashMap::new();
                                for line in lines {
                                    if let Some((k, v)) = line.split_once(':') {
                                        headers
                                            .insert(k.trim().to_lowercase(), v.trim().to_string());
                                    }
                                }

                                reqs.lock().await.push(RecordedRequest {
                                    method,
                                    path,
                                    headers,
                                    body: body_str,
                                });

                                let reason = match status {
                                    200 => "OK",
                                    400 => "Bad Request",
                                    401 => "Unauthorized",
                                    403 => "Forbidden",
                                    500 => "Internal Server Error",
                                    _ => "Unknown",
                                };

                                let mut header_lines = String::new();
                                for (hk, hv) in extra_hdrs {
                                    header_lines.push_str(&format!("{hk}: {hv}\r\n"));
                                }

                                let response = format!(
                                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/xml\r\nContent-Length: {}\r\n{header_lines}Connection: close\r\n\r\n{}",
                                    body_data.len(),
                                    body_data
                                );

                                let _ = stream.write_all(response.as_bytes()).await;
                                let _ = stream.shutdown().await;
                            }
                        });
                    }
                }
            }
        });

        Self {
            url: format!("http://{addr}/speke"),
            recorded_requests,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    async fn requests(&self) -> Vec<RecordedRequest> {
        self.recorded_requests.lock().await.clone()
    }
}

impl Drop for SpekeMockServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

const SAMPLE_SPEKE_RESPONSE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc" contentId="speke-content-1">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="55555555-5555-5555-5555-555555555555" commonEncryptionScheme="cenc">
      <cpix:Data>
        <pskc:Secret>
          <pskc:PlainValue>AQEBAQEBAQEBAQEBAQEBAQ==</pskc:PlainValue>
        </pskc:Secret>
      </cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
  <cpix:DRMSystemList>
    <cpix:DRMSystem kid="55555555-5555-5555-5555-555555555555" systemId="edef8ba9-79d6-4ace-a3c8-27dcd51d21ed">
      <cpix:PSSH>AAAAUHBzc2gAAAAA7e+LqXnWSs6jyCfc1R0h7QAAADASEJmZmZmZmZmZMzMwAAAAAAMaBU5hZ3JhIg1UZXN0X0tSX0luZGV4OAFI88aJmwY=</cpix:PSSH>
    </cpix:DRMSystem>
  </cpix:DRMSystemList>
  <cpix:ContentKeyUsageRuleList>
    <cpix:ContentKeyUsageRule kid="55555555-5555-5555-5555-555555555555" intendedTrackType="HD">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
  </cpix:ContentKeyUsageRuleList>
</cpix:CPIX>"#;

#[tokio::test]
async fn test_speke_client_basic_auth_and_headers() {
    let server = SpekeMockServer::start(200, SAMPLE_SPEKE_RESPONSE.into()).await;
    let config = SpekeConfig::new(&server.url)
        .with_auth(SpekeAuth::basic("my_user", "my_pass"))
        .with_header(
            reqwest::header::HeaderName::from_static("x-tenant-id"),
            reqwest::header::HeaderValue::from_static("tenant-999"),
        );

    let client = SpekeClient::with_config(config);

    let req = KeyRequest::new("speke-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_drm_system(DrmSystem::Widevine)
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let keyset = client.fetch_keys(&req).await.unwrap();

    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    let r = &requests[0];

    assert_eq!(r.method, "POST");
    assert_eq!(r.path, "/speke");
    assert_eq!(
        r.headers.get("x-speke-version").map(|s| s.as_str()),
        Some("2.0")
    );
    assert_eq!(
        r.headers.get("x-speke-user-agent").map(|s| s.as_str()),
        Some("drmpack/0.1.0")
    );
    assert_eq!(
        r.headers.get("content-type").map(|s| s.as_str()),
        Some("application/xml")
    );
    assert_eq!(
        r.headers.get("accept").map(|s| s.as_str()),
        Some("application/xml")
    );
    assert_eq!(
        r.headers.get("x-tenant-id").map(|s| s.as_str()),
        Some("tenant-999")
    );

    // Verify Basic Auth
    let auth = r
        .headers
        .get("authorization")
        .expect("Authorization header");
    assert!(auth.starts_with("Basic "));
    let decoded =
        String::from_utf8(BASE64_STANDARD.decode(&auth["Basic ".len()..]).unwrap()).unwrap();
    assert_eq!(decoded, "my_user:my_pass");

    // Verify parsed KeySet
    assert_eq!(keyset.len(), 1);
    let key = keyset
        .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
        .expect("HD key");
    assert_eq!(
        key.kid.0,
        Uuid::parse_str("55555555-5555-5555-5555-555555555555").unwrap()
    );
}

#[tokio::test]
async fn test_speke_client_bearer_auth() {
    let server = SpekeMockServer::start(200, SAMPLE_SPEKE_RESPONSE.into()).await;
    let config = SpekeConfig::new(&server.url).with_auth(SpekeAuth::bearer("jwt.test.token"));
    let client = SpekeClient::with_config(config);

    let req = KeyRequest::new("speke-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    client.fetch_keys(&req).await.unwrap();

    let requests = server.requests().await;
    assert_eq!(
        requests[0].headers.get("authorization").map(|s| s.as_str()),
        Some("Bearer jwt.test.token")
    );
}

#[tokio::test]
async fn test_speke_client_api_key_auth() {
    let server = SpekeMockServer::start(200, SAMPLE_SPEKE_RESPONSE.into()).await;
    let config =
        SpekeConfig::new(&server.url).with_auth(SpekeAuth::api_key("x-api-key", "secret-key-42"));
    let client = SpekeClient::with_config(config);

    let req = KeyRequest::new("speke-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    client.fetch_keys(&req).await.unwrap();

    let requests = server.requests().await;
    assert_eq!(
        requests[0].headers.get("x-api-key").map(|s| s.as_str()),
        Some("secret-key-42")
    );
}

#[tokio::test]
async fn test_speke_client_raw_exchange_features() {
    let server = SpekeMockServer::start_with_headers(
        200,
        "<response>OK</response>".into(),
        vec![("x-custom-response", "test-value")],
    )
    .await;

    let client = SpekeClient::new(&server.url);

    let mut extra_headers = reqwest::header::HeaderMap::new();
    extra_headers.insert(
        reqwest::header::HeaderName::from_static("x-exchange-hdr"),
        reqwest::header::HeaderValue::from_static("extra"),
    );

    let resp = client
        .raw_exchange(
            "<request>payload</request>",
            &[("queryKey", "queryValue")],
            Some(&extra_headers),
        )
        .await
        .unwrap();

    assert!(resp.is_success());
    assert_eq!(resp.status.as_u16(), 200);
    assert_eq!(resp.body, "<response>OK</response>");
    assert_eq!(
        resp.headers
            .get("x-custom-response")
            .and_then(|h| h.to_str().ok()),
        Some("test-value")
    );

    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    assert!(requests[0].path.contains("queryKey=queryValue"));
    assert_eq!(
        requests[0]
            .headers
            .get("x-exchange-hdr")
            .map(|s| s.as_str()),
        Some("extra")
    );
    assert_eq!(requests[0].body, "<request>payload</request>");
}

#[tokio::test]
async fn test_speke_client_raw_exchange_preserves_error_status() {
    let server = SpekeMockServer::start_with_headers(
        400,
        "Bad Request Body".into(),
        vec![("x-error-code", "INVALID_ARGUMENT")],
    )
    .await;

    let client = SpekeClient::new(&server.url);
    let resp = client
        .raw_exchange("<bad-payload/>", &[], None)
        .await
        .unwrap();

    assert!(!resp.is_success());
    assert_eq!(resp.status.as_u16(), 400);
    assert_eq!(resp.body, "Bad Request Body");
    assert_eq!(
        resp.headers
            .get("x-error-code")
            .and_then(|h| h.to_str().ok()),
        Some("INVALID_ARGUMENT")
    );
}

#[tokio::test]
async fn test_speke_client_fetch_keys_server_error() {
    let server = SpekeMockServer::start(500, "Server exploded".into()).await;
    let client = SpekeClient::new(&server.url);

    let req = KeyRequest::new("speke-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let result = client.fetch_keys(&req).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, DrmpackError::KeyProvider(_)));
    assert!(err.to_string().contains("HTTP 500"));
}

#[tokio::test]
async fn test_speke_client_empty_error_body_fallback() {
    let server = SpekeMockServer::start(502, "".into()).await;
    let client = SpekeClient::new(&server.url);

    let req = KeyRequest::new("speke-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let result = client.fetch_keys(&req).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("HTTP 502"));
    assert!(err.to_string().contains("Bad Gateway"));
}

#[tokio::test]
async fn test_speke_client_header_helper_and_auth_eq() {
    let server = SpekeMockServer::start_with_headers(
        200,
        "<ok/>".into(),
        vec![("x-custom-res-header", "custom-header-val")],
    )
    .await;

    let auth1 = SpekeAuth::bearer("token-123");
    let auth2 = SpekeAuth::bearer("token-123");
    let auth3 = SpekeAuth::bearer("other");
    assert_eq!(auth1, auth2);
    assert_ne!(auth1, auth3);

    let config = SpekeConfig::new("http://initial-url")
        .with_endpoint(&server.url)
        .with_auth(auth1);

    assert_eq!(config.endpoint, server.url);

    let client = SpekeClient::with_config(config);
    let resp = client.raw_exchange("<req/>", &[], None).await.unwrap();
    assert_eq!(
        resp.header("x-custom-res-header"),
        Some("custom-header-val")
    );
    assert_eq!(resp.header("non-existent"), None);
}

#[tokio::test]
async fn test_speke_client_packaging_session_lifecycle() {
    let server = SpekeMockServer::start(200, SAMPLE_SPEKE_RESPONSE.into()).await;
    let client = SpekeClient::new(&server.url);

    let rendition = Rendition::video(
        "v720p",
        QualityTier::hd(),
        1280,
        720,
        2_500_000,
        "avc1.4d401f",
    );

    let out_dir = std::env::temp_dir().join(format!("drmpack_speke_session_{}", Uuid::new_v4()));
    let control_dir =
        std::env::temp_dir().join(format!("drmpack_control_speke_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::new("speke-session-content")
        .with_rendition(rendition)
        .with_encryption_scheme(EncryptionScheme::Cenc)
        .with_output_dir(&out_dir)
        .with_control_dir(&control_dir)
        .with_gpac_bin("gpac");

    let mut session = PackagingSession::create(config, client)
        .await
        .expect("PackagingSession::create with SpekeClient must succeed");

    assert!(session.control_dir_path().join("cenc.xml").exists());

    let _ = session.close().await;
    let _ = session.cleanup().await;
    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[test]
fn test_static_key_source_direct_api() {
    let kid = KeyID::random();
    let key = ContentKey::new(kid, [0x99; 16], QualityTier::hd(), TrackType::Video);
    let source = StaticKeySource::new().with_key(key);
    assert!(format!("{:?}", source).contains("StaticKeySource"));
}

#[cfg(feature = "axinom")]
#[tokio::test]
async fn test_vendor_axinom_canonical_namespace_and_client_accessor() {
    use drmpack::vendor::axinom::{AxinomConfig, AxinomProvider};

    let config = AxinomConfig::new("my-tenant", "my-key");
    let provider = AxinomProvider::new(config);
    let _ = provider.client();
    let _ = provider.speke_client();
    assert_eq!(provider.config().tenant_id, "my-tenant");
}

#[tokio::test]
async fn test_speke_v2_provider_alias_and_x_api_key() {
    use drmpack::{SpekeV2Config, SpekeV2Provider};

    let server = SpekeMockServer::start(200, SAMPLE_SPEKE_RESPONSE.into()).await;
    let config =
        SpekeV2Config::new(&server.url).with_auth(SpekeAuth::x_api_key("aws-secret-api-key-99"));
    let provider: SpekeV2Provider = SpekeV2Provider::with_config(config);

    let req = KeyRequest::new("speke-alias-content")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let keyset = provider.fetch_keys(&req).await.unwrap();
    assert_eq!(keyset.len(), 1);

    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    let r = &requests[0];
    assert_eq!(
        r.headers.get("x-speke-version").map(|s| s.as_str()),
        Some("2.0")
    );
    assert_eq!(
        r.headers.get("x-api-key").map(|s| s.as_str()),
        Some("aws-secret-api-key-99")
    );
}

#[tokio::test]
async fn test_speke_v2_provider_sigv4_auth() {
    use drmpack::SpekeV2Provider;

    let server = SpekeMockServer::start(200, SAMPLE_SPEKE_RESPONSE.into()).await;
    let config = SpekeConfig::new(&server.url).with_auth(SpekeAuth::sigv4(
        "AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/20260905/us-east-1/execute-api/aws4_request",
        Some("session-token-xyz"),
        Some("20260905T120000Z"),
    ));
    let provider = SpekeV2Provider::with_config(config);

    let req = KeyRequest::new("speke-sigv4-content")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let keyset = provider.fetch_keys(&req).await.unwrap();
    assert_eq!(keyset.len(), 1);

    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    let r = &requests[0];
    assert_eq!(
        r.headers.get("authorization").map(|s| s.as_str()),
        Some("AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/20260905/us-east-1/execute-api/aws4_request")
    );
    assert_eq!(
        r.headers.get("x-amz-security-token").map(|s| s.as_str()),
        Some("session-token-xyz")
    );
    assert_eq!(
        r.headers.get("x-amz-date").map(|s| s.as_str()),
        Some("20260905T120000Z")
    );
}

#[tokio::test]
async fn test_speke_v2_provider_dynamic_signer() {
    let server = SpekeMockServer::start(200, SAMPLE_SPEKE_RESPONSE.into()).await;
    let config = SpekeConfig::new(&server.url).with_signer(
        |builder: reqwest::RequestBuilder, body: &str| {
            builder.header("x-amz-content-sha256", format!("body-len-{}", body.len()))
        },
    );
    let client = SpekeClient::with_config(config);

    let req = KeyRequest::new("speke-signer-content")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    client.fetch_keys(&req).await.unwrap();

    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    let r = &requests[0];
    assert!(r.headers.contains_key("x-amz-content-sha256"));
    let sha_val = r.headers.get("x-amz-content-sha256").unwrap();
    assert!(sha_val.starts_with("body-len-"));
}

#[test]
fn test_speke_v2_exports_at_crate_and_key_module() {
    use drmpack::key::SpekeV2Provider as KeySpekeV2Provider;
    use drmpack::SpekeV2Provider as RootSpekeV2Provider;

    let p1 = RootSpekeV2Provider::new("http://localhost/speke");
    let p2 = KeySpekeV2Provider::new("http://localhost/speke");
    assert_eq!(p1.config().endpoint, "http://localhost/speke");
    assert_eq!(p2.config().endpoint, "http://localhost/speke");
}
