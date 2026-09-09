#![cfg(feature = "license-proxy")]

use bytes::Bytes;
use drmpack::error::DrmpackError;
use drmpack::license::{
    handle_fairplay_certificate, handle_fairplay_license, handle_playready_license,
    handle_widevine_license, LicenseProxy, LicenseResponse,
};
use drmpack::vendor::axinom::config::AxinomLicenseConfig;
use reqwest::StatusCode;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex};

fn mock_license_config(server_url: &str) -> AxinomLicenseConfig {
    AxinomLicenseConfig::new(
        format!("{server_url}/widevine"),
        format!("{server_url}/fairplay"),
        format!("{server_url}/playready"),
        format!("{server_url}/fairplay.cer"),
    )
}

#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

struct LicenseMockServer {
    url: String,
    recorded_requests: Arc<Mutex<Vec<RecordedRequest>>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl LicenseMockServer {
    async fn start(
        status: u16,
        response_body: Vec<u8>,
        response_headers: Vec<(&'static str, &'static str)>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let recorded_requests = Arc::new(Mutex::new(Vec::new()));
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let reqs = Arc::clone(&recorded_requests);
        let resp_body = response_body;
        let headers_to_send = response_headers;

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accept_result = listener.accept() => {
                        let Ok((mut stream, _)) = accept_result else { break };
                        let reqs = Arc::clone(&reqs);
                        let body_data = resp_body.clone();
                        let extra_headers_list = headers_to_send.clone();

                        tokio::spawn(async move {
                            let mut buf = Vec::new();
                            let mut chunk = [0u8; 4096];
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
                                let body_slice = &buf[end..end + content_length];

                                let mut lines = header_str.lines();
                                let first_line = lines.next().unwrap_or("");
                                let mut parts = first_line.split_whitespace();
                                let method = parts.next().unwrap_or("").to_string();
                                let path = parts.next().unwrap_or("").to_string();

                                let mut headers = HashMap::new();
                                for line in lines {
                                    if let Some((k, v)) = line.split_once(':') {
                                        headers.insert(
                                            k.trim().to_lowercase(),
                                            v.trim().to_string(),
                                        );
                                    }
                                }

                                reqs.lock().await.push(RecordedRequest {
                                    method,
                                    path,
                                    headers,
                                    body: body_slice.to_vec(),
                                });

                                let status_text = match status {
                                    200 => "OK",
                                    400 => "Bad Request",
                                    401 => "Unauthorized",
                                    403 => "Forbidden",
                                    404 => "Not Found",
                                    500 => "Internal Server Error",
                                    _ => "Unknown",
                                };

                                let mut response = format!(
                                    "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                                    status,
                                    status_text,
                                    body_data.len()
                                );

                                for (k, v) in &extra_headers_list {
                                    response.push_str(&format!("{k}: {v}\r\n"));
                                }
                                response.push_str("\r\n");

                                let _ = stream.write_all(response.as_bytes()).await;
                                let _ = stream.write_all(&body_data).await;
                                let _ = stream.flush().await;
                            }
                        });
                    }
                }
            }
        });

        Self {
            url: format!("http://{}", addr),
            recorded_requests,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    async fn requests(&self) -> Vec<RecordedRequest> {
        self.recorded_requests.lock().await.clone()
    }
}

impl Drop for LicenseMockServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

#[tokio::test]
async fn test_license_proxy_widevine_success() {
    let license_payload = b"\x08\x01\x12\x10test-widevine-license-payload-data";
    let server = LicenseMockServer::start(
        200,
        license_payload.to_vec(),
        vec![
            ("Content-Type", "application/octet-stream"),
            ("X-AxDRM-Message", "device-wv-tracking-jwt-12345"),
        ],
    )
    .await;

    let config = mock_license_config(&server.url);
    let proxy = LicenseProxy::new(config);

    let challenge = b"\x08\x04\x12\x04wv-challenge";
    let auth_token = "eyJhGciOi...test-jwt-token";

    let resp: LicenseResponse = handle_widevine_license(&proxy, challenge, auth_token)
        .await
        .expect("Widevine license request must succeed");

    // Verify response content
    assert_eq!(resp.as_bytes(), license_payload);
    assert_eq!(&resp[..], license_payload);
    assert_eq!(resp.content_type(), Some("application/octet-stream"));
    assert_eq!(resp.axdrm_message(), Some("device-wv-tracking-jwt-12345"));
    assert_eq!(resp.into_bytes().as_ref(), license_payload);

    // Verify request recorded by mock server
    let reqs = server.requests().await;
    assert_eq!(reqs.len(), 1);
    let req = &reqs[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/widevine");
    assert_eq!(req.body, challenge);
    assert_eq!(
        req.headers.get("x-axdrm-message").map(String::as_str),
        Some(auth_token)
    );
    assert_eq!(
        req.headers.get("content-type").map(String::as_str),
        Some("application/octet-stream")
    );
    assert!(
        req.headers
            .get("user-agent")
            .unwrap()
            .starts_with("drmpack/"),
        "User agent should start with drmpack/"
    );
}

#[tokio::test]
async fn test_license_proxy_fairplay_success() {
    let ckc_payload = b"fairplay-ckc-response-payload";
    let server = LicenseMockServer::start(
        200,
        ckc_payload.to_vec(),
        vec![
            ("Content-Type", "application/octet-stream"),
            ("X-AxDRM-Message", "device-fp-tracking-jwt-67890"),
        ],
    )
    .await;

    let config = mock_license_config(&server.url);
    let proxy = LicenseProxy::new(config);

    let spc_bytes = b"fairplay-spc-client-context";
    let auth_token = "fps-auth-token-jwt";

    let resp = handle_fairplay_license(&proxy, spc_bytes, auth_token)
        .await
        .expect("FairPlay license request must succeed");

    assert_eq!(resp.as_bytes(), ckc_payload);
    assert_eq!(resp.content_type(), Some("application/octet-stream"));
    assert_eq!(resp.axdrm_message(), Some("device-fp-tracking-jwt-67890"));

    let reqs = server.requests().await;
    assert_eq!(reqs.len(), 1);
    let req = &reqs[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.body, spc_bytes);
    assert_eq!(
        req.headers.get("x-axdrm-message").map(String::as_str),
        Some(auth_token)
    );
    assert_eq!(
        req.headers.get("content-type").map(String::as_str),
        Some("application/octet-stream")
    );
}

#[tokio::test]
async fn test_license_proxy_playready_success() {
    let playready_payload = b"<PlayReadyLicenseResponse>...</PlayReadyLicenseResponse>";
    let server = LicenseMockServer::start(
        200,
        playready_payload.to_vec(),
        vec![
            ("Content-Type", "text/xml; charset=utf-8"),
            ("X-AxDRM-Message", "device-pr-tracking-jwt-abcde"),
        ],
    )
    .await;

    let config = mock_license_config(&server.url);
    let proxy = LicenseProxy::new(config);

    let challenge = b"playready-raw-challenge";
    let auth_token = "pr-auth-token-jwt";

    let resp = handle_playready_license(&proxy, challenge, auth_token)
        .await
        .expect("PlayReady license request must succeed");

    assert_eq!(resp.as_bytes(), playready_payload);
    assert_eq!(resp.content_type(), Some("text/xml; charset=utf-8"));
    assert_eq!(resp.axdrm_message(), Some("device-pr-tracking-jwt-abcde"));

    let reqs = server.requests().await;
    assert_eq!(reqs.len(), 1);
    let req = &reqs[0];
    assert_eq!(req.method, "POST");
    assert_eq!(req.body, challenge);
    assert_eq!(
        req.headers.get("content-type").map(String::as_str),
        Some("text/xml; charset=utf-8")
    );
    assert_eq!(
        req.headers.get("x-axdrm-message").map(String::as_str),
        Some(auth_token)
    );
}

#[tokio::test]
async fn test_license_proxy_403_forbidden_with_axdrm_errormessage() {
    let server = LicenseMockServer::start(
        403,
        b"Forbidden".to_vec(),
        vec![("X-AxDRM-ErrorMessage", "License token has expired")],
    )
    .await;

    let config = mock_license_config(&server.url);
    let proxy = LicenseProxy::new(config);

    let result = handle_widevine_license(&proxy, b"challenge", "expired-token").await;
    assert!(result.is_err());

    let err = result.unwrap_err();
    match err {
        DrmpackError::LicenseProxy {
            status,
            message,
            diagnostic,
        } => {
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert_eq!(diagnostic.as_deref(), Some("License token has expired"));
            assert!(
                message.contains("License token has expired"),
                "Message must contain diagnostic: {message}"
            );
        }
        other => panic!("Expected DrmpackError::LicenseProxy, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_license_proxy_400_bad_request_with_axdrm_errormessage() {
    let server = LicenseMockServer::start(
        400,
        b"Malformed challenge protobuf".to_vec(),
        vec![("X-AxDRM-ErrorMessage", "Could not parse player challenge")],
    )
    .await;

    let config = mock_license_config(&server.url);
    let proxy = LicenseProxy::new(config);

    let result = handle_fairplay_license(&proxy, b"bad-spc", "token").await;
    assert!(result.is_err());

    let err = result.unwrap_err();
    match err {
        DrmpackError::LicenseProxy {
            status,
            message,
            diagnostic,
        } => {
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(
                diagnostic.as_deref(),
                Some("Could not parse player challenge")
            );
            assert!(
                message.contains("Could not parse player challenge"),
                "Message must contain diagnostic: {message}"
            );
        }
        other => panic!("Expected DrmpackError::LicenseProxy, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_license_proxy_500_server_error_fallback() {
    let server =
        LicenseMockServer::start(500, b"Internal server error occurred".to_vec(), vec![]).await;

    let config = mock_license_config(&server.url);
    let proxy = LicenseProxy::new(config);

    let result = handle_playready_license(&proxy, b"challenge", "token").await;
    assert!(result.is_err());

    let err = result.unwrap_err();
    match err {
        DrmpackError::LicenseProxy {
            status,
            message,
            diagnostic,
        } => {
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(diagnostic, None);
            assert!(
                message.contains("Internal server error occurred"),
                "Message must fall back to body: {message}"
            );
        }
        other => panic!("Expected DrmpackError::LicenseProxy, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_license_proxy_fairplay_cert_caching_single_request() {
    let cert_bytes = b"MIIAppleFairPlayApplicationCertificateDerData123456789";
    let server = LicenseMockServer::start(
        200,
        cert_bytes.to_vec(),
        vec![("Content-Type", "application/x-x509-ca-cert")],
    )
    .await;

    let config = mock_license_config(&server.url);
    let proxy = LicenseProxy::new(config);

    // Call 5 times sequentially
    for _ in 0..5 {
        let cert = handle_fairplay_certificate(&proxy, &server.url)
            .await
            .expect("Certificate fetch must succeed");
        assert_eq!(cert.as_ref(), cert_bytes);
    }

    // Call 5 times concurrently
    let mut tasks = Vec::new();
    for _ in 0..5 {
        let p = proxy.clone();
        let u = server.url.clone();
        tasks.push(tokio::spawn(async move {
            handle_fairplay_certificate(&p, &u).await.unwrap()
        }));
    }
    for t in tasks {
        let res = t.await.unwrap();
        assert_eq!(res.as_ref(), cert_bytes);
    }

    // Crucial check: exactly 1 request was sent to the upstream server!
    let reqs = server.requests().await;
    assert_eq!(
        reqs.len(),
        1,
        "FairPlay certificate must be cached; expected 1 upstream request, got {}",
        reqs.len()
    );
    assert_eq!(reqs[0].method, "GET");
}

#[tokio::test]
async fn test_license_proxy_fairplay_cert_preloading() {
    let cert_bytes = b"PreloadedFairPlayCertificateDER";
    let server = LicenseMockServer::start(
        200,
        cert_bytes.to_vec(),
        vec![("Content-Type", "application/x-x509-ca-cert")],
    )
    .await;

    let config = mock_license_config(&server.url);
    let proxy = LicenseProxy::new(config);

    // Initially not cached
    assert_eq!(proxy.cached_fairplay_certificate().await, None);

    // Preload certificate
    let preloaded = proxy
        .preload_fairplay_certificate()
        .await
        .expect("Preload must succeed");
    assert_eq!(preloaded.as_ref(), cert_bytes);

    // Now cached
    assert_eq!(
        proxy.cached_fairplay_certificate().await,
        Some(Bytes::from_static(cert_bytes))
    );

    // Subsequent call with None takes from cache without network I/O
    let cert = handle_fairplay_certificate(&proxy, None::<&str>)
        .await
        .expect("Fetch from cache must succeed");
    assert_eq!(cert.as_ref(), cert_bytes);

    let cert2 = handle_fairplay_certificate(&proxy, None as Option<&str>)
        .await
        .expect("Fetch with None must succeed");
    assert_eq!(cert2.as_ref(), cert_bytes);

    let reqs = server.requests().await;
    assert_eq!(
        reqs.len(),
        1,
        "Expected exactly 1 request during preload, got {}",
        reqs.len()
    );
}

#[tokio::test]
async fn test_license_proxy_fairplay_cert_error_not_cached() {
    let server = LicenseMockServer::start(
        404,
        b"Not Found".to_vec(),
        vec![("X-AxDRM-ErrorMessage", "Certificate does not exist")],
    )
    .await;

    let config = mock_license_config(&server.url);
    let proxy = LicenseProxy::new(config);

    let res = handle_fairplay_certificate(&proxy, &server.url).await;
    assert!(res.is_err());
    let err = res.unwrap_err();
    match err {
        DrmpackError::LicenseProxy {
            status, diagnostic, ..
        } => {
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert_eq!(diagnostic.as_deref(), Some("Certificate does not exist"));
        }
        other => panic!("Expected LicenseProxy error, got: {:?}", other),
    }

    // Verify cache remains empty after error
    assert_eq!(proxy.cached_fairplay_certificate().await, None);
}

#[tokio::test]
async fn test_license_proxy_custom_headers_propagation() {
    let server = LicenseMockServer::start(
        200,
        b"license-data".to_vec(),
        vec![("Content-Type", "application/octet-stream")],
    )
    .await;

    let config = mock_license_config(&server.url).with_header(
        reqwest::header::HeaderName::from_static("x-tenant-custom-trace"),
        reqwest::header::HeaderValue::from_static("custom-trace-uuid-987"),
    );
    let proxy = LicenseProxy::new(config);

    handle_widevine_license(&proxy, b"challenge", "token")
        .await
        .unwrap();

    let reqs = server.requests().await;
    assert_eq!(reqs.len(), 1);
    assert_eq!(
        reqs[0]
            .headers
            .get("x-tenant-custom-trace")
            .map(String::as_str),
        Some("custom-trace-uuid-987")
    );
}

#[tokio::test]
async fn test_license_proxy_connection_reuse() {
    let server = LicenseMockServer::start(
        200,
        b"pooled-license".to_vec(),
        vec![("Content-Type", "application/octet-stream")],
    )
    .await;

    let config = mock_license_config(&server.url);
    let proxy = LicenseProxy::new(config);

    for i in 0..3 {
        let resp = handle_widevine_license(
            &proxy,
            format!("challenge-{i}").as_bytes(),
            &format!("token-{i}"),
        )
        .await
        .expect("Pooled request should succeed");
        assert_eq!(resp.as_bytes(), b"pooled-license");
    }

    let reqs = server.requests().await;
    assert_eq!(reqs.len(), 3);
}

#[test]
fn test_license_config_mandatory_urls() {
    let config = AxinomLicenseConfig::new(
        "https://custom.widevine/acquire",
        "https://custom.fairplay/acquire",
        "https://custom.playready/acquire",
        "https://custom.fairplay/cert.der",
    );
    assert_eq!(
        config.widevine_license_url,
        "https://custom.widevine/acquire"
    );
    assert_eq!(
        config.fairplay_license_url,
        "https://custom.fairplay/acquire"
    );
    assert_eq!(
        config.playready_license_url,
        "https://custom.playready/acquire"
    );
    assert_eq!(config.fairplay_cert_url, "https://custom.fairplay/cert.der");
}

#[tokio::test]
async fn test_license_proxy_empty_fairplay_cert_rejected() {
    let server = LicenseMockServer::start(200, vec![], vec![]).await;

    let config = mock_license_config(&server.url);
    let proxy = LicenseProxy::new(config);

    let res = handle_fairplay_certificate(&proxy, &server.url).await;
    assert!(res.is_err());
    let err = res.unwrap_err();
    match err {
        DrmpackError::LicenseProxy {
            status, message, ..
        } => {
            assert_eq!(status, StatusCode::BAD_GATEWAY);
            assert!(message.contains("Received empty FairPlay application certificate"));
        }
        other => panic!("Expected LicenseProxy error, got: {:?}", other),
    }

    assert_eq!(proxy.cached_fairplay_certificate().await, None);
}

#[tokio::test]
async fn test_license_proxy_empty_license_payload_rejected() {
    let server = LicenseMockServer::start(200, vec![], vec![]).await;

    let config = mock_license_config(&server.url);
    let proxy = LicenseProxy::new(config);

    let res = handle_widevine_license(&proxy, b"challenge", "token").await;
    assert!(res.is_err());
    let err = res.unwrap_err();
    match err {
        DrmpackError::LicenseProxy {
            status, message, ..
        } => {
            assert_eq!(status, StatusCode::BAD_GATEWAY);
            assert!(message.contains("empty license payload"));
        }
        other => panic!("Expected LicenseProxy error, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_license_proxy_empty_challenge_rejected() {
    let config = mock_license_config("https://mock.axprod.net");
    let proxy = LicenseProxy::new(config);

    let res = handle_widevine_license(&proxy, b"", "token").await;
    assert!(res.is_err());
    match res.unwrap_err() {
        DrmpackError::LicenseProxy { status, .. } => {
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        other => panic!("Expected 400 Bad Request, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_license_proxy_empty_token_rejected() {
    let config = mock_license_config("https://mock.axprod.net");
    let proxy = LicenseProxy::new(config);

    let res = handle_widevine_license(&proxy, b"challenge", "   ").await;
    assert!(res.is_err());
    match res.unwrap_err() {
        DrmpackError::LicenseProxy { status, .. } => {
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        other => panic!("Expected 401 Unauthorized, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_license_proxy_different_cert_urls_cached_separately() {
    let cert1 = b"CertFromOrigin1";
    let server1 = LicenseMockServer::start(200, cert1.to_vec(), vec![]).await;

    let cert2 = b"CertFromOrigin2";
    let server2 = LicenseMockServer::start(200, cert2.to_vec(), vec![]).await;

    let config = mock_license_config(&server1.url);
    let proxy = LicenseProxy::new(config);

    let res1 = handle_fairplay_certificate(&proxy, &server1.url)
        .await
        .unwrap();
    assert_eq!(res1.as_ref(), cert1);

    let res2 = handle_fairplay_certificate(&proxy, &server2.url)
        .await
        .unwrap();
    assert_eq!(res2.as_ref(), cert2);

    assert_eq!(server1.requests().await.len(), 1);
    assert_eq!(server2.requests().await.len(), 1);
}

#[tokio::test]
async fn test_license_proxy_large_error_body_truncated() {
    let large_body = "x".repeat(5000);
    let server = LicenseMockServer::start(500, large_body.into_bytes(), vec![]).await;

    let config = mock_license_config(&server.url);
    let proxy = LicenseProxy::new(config);

    let err = handle_widevine_license(&proxy, b"challenge", "token")
        .await
        .unwrap_err();
    match err {
        DrmpackError::LicenseProxy { message, .. } => {
            assert!(message.ends_with("..."));
            assert!(message.len() <= 2100);
        }
        other => panic!("Expected LicenseProxy error, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_license_proxy_try_new_api() {
    let valid = mock_license_config("https://mock.axprod.net");
    let proxy = LicenseProxy::try_new(valid);
    assert!(proxy.is_ok());

    let invalid =
        mock_license_config("https://mock.axprod.net").with_fairplay_cert_url("ftp://bad");
    let err = LicenseProxy::try_new(invalid);
    assert!(err.is_err());
}

#[test]
fn test_license_response_conversions_and_equality() {
    let payload = b"test-drm-payload";
    let resp = LicenseResponse::new(
        Bytes::from_static(payload),
        Some("application/octet-stream".to_string()),
        reqwest::header::HeaderMap::new(),
    );

    let clone = resp.clone();
    assert_eq!(resp, clone);

    let bytes: Bytes = resp.clone().into();
    assert_eq!(bytes.as_ref(), payload);

    let vec: Vec<u8> = resp.into();
    assert_eq!(vec.as_slice(), payload);
}

fn generate_axinom_test_jwt(com_key_id: &str, com_key_b64: &str, kid: &str) -> Option<String> {
    use base64::prelude::*;
    let key_bytes = BASE64_STANDARD.decode(com_key_b64).ok()?;
    let header_json = r#"{"alg":"HS256","typ":"JWT"}"#;
    let payload_json = format!(
        r#"{{"version":1,"com_key_id":"{com_key_id}","message":{{"type":"entitlement_message","version":2,"content_keys_source":{{"inline":[{{"id":"{kid}"}}]}}}}}}"#
    );
    let h_b64 = BASE64_URL_SAFE_NO_PAD.encode(header_json.as_bytes());
    let p_b64 = BASE64_URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
    let signing_input = format!("{h_b64}.{p_b64}");

    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &key_bytes);
    let tag = ring::hmac::sign(&key, signing_input.as_bytes());
    let sig_b64 = BASE64_URL_SAFE_NO_PAD.encode(tag.as_ref());
    Some(format!("{signing_input}.{sig_b64}"))
}

#[tokio::test]
async fn test_license_proxy_live_endpoints_roundtrip() {
    let _ = dotenvy::dotenv();

    let config = match AxinomLicenseConfig::from_env() {
        Ok(cfg) => cfg,
        Err(_) => return,
    };

    if config.widevine_license_url.contains("your-")
        || config.widevine_license_url.contains("00000000-0000")
    {
        return;
    }

    let proxy = LicenseProxy::new(config);

    // 1. Live Widevine request reaching real Axinom cloud
    let res_wv = proxy
        .handle_widevine_license(b"test-challenge", "test-dummy-token")
        .await;
    assert!(res_wv.is_err());
    match res_wv.unwrap_err() {
        DrmpackError::LicenseProxy {
            status, diagnostic, ..
        } => {
            assert!(status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED);
            assert!(
                diagnostic.is_some(),
                "Expected X-AxDRM-ErrorMessage from real Axinom Widevine server"
            );
            let diag = diagnostic.unwrap();
            assert!(diag.contains("Invalid DRM message") || diag.contains("Invalid JWT"));
        }
        other => panic!(
            "Expected LicenseProxy error from live Axinom, got: {:?}",
            other
        ),
    }

    // 2. Live PlayReady request reaching real Axinom cloud
    let res_pr = proxy
        .handle_playready_license(b"test-challenge", "test-dummy-token")
        .await;
    assert!(res_pr.is_err());
    match res_pr.unwrap_err() {
        DrmpackError::LicenseProxy {
            status, diagnostic, ..
        } => {
            assert!(
                status == StatusCode::INTERNAL_SERVER_ERROR || status == StatusCode::BAD_REQUEST
            );
            assert!(
                diagnostic.is_some(),
                "Expected X-AxDRM-ErrorMessage from real Axinom PlayReady server"
            );
            let diag = diagnostic.unwrap();
            assert!(diag.contains("Invalid DRM message") || diag.contains("Invalid JWT"));
        }
        other => panic!(
            "Expected LicenseProxy error from live Axinom, got: {:?}",
            other
        ),
    }

    // 3. Live FairPlay request reaching real Axinom cloud
    let res_fp = proxy
        .handle_fairplay_license(b"test-spc", "test-dummy-token")
        .await;
    assert!(res_fp.is_err());
    match res_fp.unwrap_err() {
        DrmpackError::LicenseProxy {
            status, diagnostic, ..
        } => {
            assert!(status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED);
            assert!(
                diagnostic.is_some(),
                "Expected X-AxDRM-ErrorMessage from real Axinom FairPlay server"
            );
            let diag = diagnostic.unwrap();
            assert!(diag.contains("Invalid DRM message") || diag.contains("Invalid JWT"));
        }
        other => panic!(
            "Expected LicenseProxy error from live Axinom, got: {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn test_license_proxy_live_signed_jwt_roundtrip() {
    let _ = dotenvy::dotenv();

    let config = match AxinomLicenseConfig::from_env() {
        Ok(cfg) => cfg,
        Err(_) => return,
    };

    let com_key_id = match std::env::var("AXINOM_COMMUNICATION_KEY_ID") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return,
    };
    let com_key = match std::env::var("AXINOM_COMMUNICATION_KEY") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return,
    };

    let jwt = match generate_axinom_test_jwt(
        &com_key_id,
        &com_key,
        "33333333-3333-3333-3333-333333333333",
    ) {
        Some(t) => t,
        None => return,
    };

    let proxy = LicenseProxy::new(config);

    // 1. Live Widevine with valid JWT: Axinom verifies JWT and inspects challenge payload
    let res_wv = proxy.handle_widevine_license(b"test-challenge", &jwt).await;
    assert!(res_wv.is_err());
    match res_wv.unwrap_err() {
        DrmpackError::LicenseProxy {
            status, diagnostic, ..
        } => {
            assert_eq!(status, StatusCode::BAD_REQUEST);
            let diag = diagnostic.expect("Axinom should report challenge format error");
            assert!(
                diag.contains("Widevine request"),
                "Expected Widevine request format error, got: {diag}"
            );
        }
        other => panic!(
            "Expected LicenseProxy error from live Axinom, got: {:?}",
            other
        ),
    }

    // 2. Live PlayReady with valid JWT: Axinom verifies JWT and validates XML
    let res_pr = proxy
        .handle_playready_license(b"test-challenge", &jwt)
        .await;
    assert!(res_pr.is_err());
    match res_pr.unwrap_err() {
        DrmpackError::LicenseProxy {
            status, diagnostic, ..
        } => {
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            let diag = diagnostic.expect("Axinom should report PlayReady XML error");
            assert!(
                diag.contains("XML") || diag.contains("PlayReady"),
                "Expected XML error, got: {diag}"
            );
        }
        other => panic!(
            "Expected LicenseProxy error from live Axinom, got: {:?}",
            other
        ),
    }

    // 3. Live FairPlay with valid JWT: Axinom verifies JWT and checks SPC size
    let res_fp = proxy.handle_fairplay_license(b"test-spc", &jwt).await;
    assert!(res_fp.is_err());
    match res_fp.unwrap_err() {
        DrmpackError::LicenseProxy {
            status, diagnostic, ..
        } => {
            assert_eq!(status, StatusCode::BAD_REQUEST);
            let diag = diagnostic.expect("Axinom should report FairPlay SPC size error");
            assert!(
                diag.contains("header size") || diag.contains("FairPlay"),
                "Expected SPC size error, got: {diag}"
            );
        }
        other => panic!(
            "Expected LicenseProxy error from live Axinom, got: {:?}",
            other
        ),
    }
}
