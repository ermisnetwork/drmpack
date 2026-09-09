#![cfg(feature = "axinom")]

use base64::prelude::*;
use drmpack::axinom::{AxinomConfig, AxinomProvider};
use drmpack::error::DrmpackError;
use drmpack::key::{KeyProvider, KeyRequest};
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::{DrmSystem, EncryptionScheme, LatencyMode, QualityTier, Rendition, TrackType};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex};
use uuid::Uuid;

mod common;
use common::find_box;

fn media_tools_available() -> bool {
    ["gpac", "ffmpeg"].into_iter().all(|binary| {
        std::process::Command::new("which")
            .arg(binary)
            .output()
            .is_ok_and(|output| output.status.success())
    })
}

async fn generate_sample_mp4(prefix: &str) -> Vec<u8> {
    let sample_mp4_path = std::env::temp_dir().join(format!("{prefix}_{}.mp4", Uuid::new_v4()));
    let ffmpeg_status = std::process::Command::new("ffmpeg")
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=4:size=640x360:rate=30",
            "-c:v",
            "libx264",
            "-profile:v",
            "baseline",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "empty_moov+default_base_moof+frag_keyframe",
            "-f",
            "mp4",
            sample_mp4_path.to_str().unwrap(),
            "-y",
        ])
        .output()
        .expect("Failed to run ffmpeg to generate test fMP4");
    assert!(ffmpeg_status.status.success(), "ffmpeg generation failed");

    let sample_bytes = tokio::fs::read(&sample_mp4_path).await.unwrap();
    let _ = tokio::fs::remove_file(&sample_mp4_path).await;
    sample_bytes
}

#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: String,
}

struct AxinomMockServer {
    url: String,
    recorded_requests: Arc<Mutex<Vec<RecordedRequest>>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl AxinomMockServer {
    async fn start(status: u16, response_body: String) -> Self {
        Self::start_with_headers(status, response_body, Vec::new()).await
    }

    async fn start_with_headers(
        status: u16,
        response_body: String,
        response_headers: Vec<(&'static str, &'static str)>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let recorded_requests = Arc::new(Mutex::new(Vec::new()));
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let reqs = Arc::clone(&recorded_requests);
        let resp_body = response_body.clone();
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

                                let rec = RecordedRequest {
                                    method,
                                    path,
                                    headers,
                                    body: body_str,
                                };

                                let body = body_data;
                                reqs.lock().await.push(rec);

                                let reason = match status {
                                    200 => "OK",
                                    400 => "Bad Request",
                                    401 => "Unauthorized",
                                    403 => "Forbidden",
                                    500 => "Internal Server Error",
                                    _ => "Unknown",
                                };

                                let mut extra_hdrs = String::new();
                                for (k, v) in extra_headers_list {
                                    extra_hdrs.push_str(&format!("{k}: {v}\r\n"));
                                }

                                let response = format!(
                                    "HTTP/1.1 {} {}\r\nContent-Type: application/xml\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                                    status, reason, extra_hdrs, body.len(), body
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
            url: format!("http://{addr}/api/SpekeV2"),
            recorded_requests,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    async fn requests(&self) -> Vec<RecordedRequest> {
        self.recorded_requests.lock().await.clone()
    }
}

impl Drop for AxinomMockServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

const SAMPLE_AXINOM_RESPONSE_CENC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc" contentId="axinom-content-1">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="33333333-3333-3333-3333-333333333333" commonEncryptionScheme="cenc">
      <cpix:Data>
        <pskc:Secret>
          <pskc:PlainValue>AQEBAQEBAQEBAQEBAQEBAQ==</pskc:PlainValue>
        </pskc:Secret>
      </cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
  <cpix:DRMSystemList>
    <cpix:DRMSystem kid="33333333-3333-3333-3333-333333333333" systemId="edef8ba9-79d6-4ace-a3c8-27dcd51d21ed">
      <cpix:PSSH>AAAAUHBzc2gAAAAA7e+LqXnWSs6jyCfc1R0h7QAAADASEJmZmZmZmZmZMzMwAAAAAAMaBU5hZ3JhIg1UZXN0X0tSX0luZGV4OAFI88aJmwY=</cpix:PSSH>
    </cpix:DRMSystem>
  </cpix:DRMSystemList>
  <cpix:ContentKeyUsageRuleList>
    <cpix:ContentKeyUsageRule kid="33333333-3333-3333-3333-333333333333" intendedTrackType="HD">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
  </cpix:ContentKeyUsageRuleList>
</cpix:CPIX>"#;

const SAMPLE_AXINOM_RESPONSE_DUAL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc" contentId="axinom-dual-content">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="11111111-1111-1111-1111-111111111111" commonEncryptionScheme="cenc">
      <cpix:Data>
        <pskc:Secret>
          <pskc:PlainValue>MTExMTExMTExMTExMTExMQ==</pskc:PlainValue>
        </pskc:Secret>
      </cpix:Data>
    </cpix:ContentKey>
    <cpix:ContentKey kid="22222222-2222-2222-2222-222222222222" commonEncryptionScheme="cbcs">
      <cpix:Data>
        <pskc:Secret>
          <pskc:PlainValue>MjIyMjIyMjIyMjIyMjIyMg==</pskc:PlainValue>
        </pskc:Secret>
      </cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
  <cpix:DRMSystemList>
    <cpix:DRMSystem kid="11111111-1111-1111-1111-111111111111" systemId="edef8ba9-79d6-4ace-a3c8-27dcd51d21ed">
      <cpix:PSSH>AAAAUHBzc2gAAAAA7e+LqXnWSs6jyCfc1R0h7QAAADASEJmZmZmZmZmZMzMwAAAAAAMaBU5hZ3JhIg1UZXN0X0tSX0luZGV4OAFI88aJmwY=</cpix:PSSH>
    </cpix:DRMSystem>
    <cpix:DRMSystem kid="22222222-2222-2222-2222-222222222222" systemId="94ce86fb-07ff-4f43-adb8-93d2fa968ca2">
      <cpix:URIExtXKey>skd://22222222-2222-2222-2222-222222222222</cpix:URIExtXKey>
    </cpix:DRMSystem>
  </cpix:DRMSystemList>
  <cpix:ContentKeyUsageRuleList>
    <cpix:ContentKeyUsageRule kid="11111111-1111-1111-1111-111111111111" intendedTrackType="HD">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
    <cpix:ContentKeyUsageRule kid="22222222-2222-2222-2222-222222222222" intendedTrackType="HD">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
  </cpix:ContentKeyUsageRuleList>
</cpix:CPIX>"#;

const SAMPLE_AXINOM_RESPONSE_OVERRIDDEN_KID: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc" contentId="axinom-overridden">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee" commonEncryptionScheme="cenc">
      <cpix:Data>
        <pskc:Secret>
          <pskc:PlainValue>AQEBAQEBAQEBAQEBAQEBAQ==</pskc:PlainValue>
        </pskc:Secret>
      </cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
  <cpix:DRMSystemList>
    <cpix:DRMSystem kid="aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee" systemId="edef8ba9-79d6-4ace-a3c8-27dcd51d21ed">
      <cpix:PSSH>AAAAUHBzc2gAAAAA7e+LqXnWSs6jyCfc1R0h7QAAADASEJmZmZmZmZmZMzMwAAAAAAMaBU5hZ3JhIg1UZXN0X0tSX0luZGV4OAFI88aJmwY=</cpix:PSSH>
    </cpix:DRMSystem>
  </cpix:DRMSystemList>
  <cpix:ContentKeyUsageRuleList>
    <cpix:ContentKeyUsageRule kid="aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee" intendedTrackType="HD">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
  </cpix:ContentKeyUsageRuleList>
</cpix:CPIX>"#;

#[tokio::test]
async fn test_axinom_provider_headers_and_basic_auth() {
    let server = AxinomMockServer::start(200, SAMPLE_AXINOM_RESPONSE_CENC.into()).await;
    let tenant_id = "test-tenant-uuid-1234";
    let management_key = "test-management-key-abcd";

    let config = AxinomConfig::new(tenant_id, management_key).with_endpoint(&server.url);
    let provider = AxinomProvider::new(config);

    let req = KeyRequest::new("axinom-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_drm_system(DrmSystem::Widevine)
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let key_set = provider.fetch_keys(&req).await.unwrap();

    // Verify HTTP request details received by mock server
    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    let r = &requests[0];

    assert_eq!(r.method, "POST");
    assert_eq!(r.path, "/api/SpekeV2");

    // 1. Authorization: Basic <base64(tenant:key)>
    let auth_header = r
        .headers
        .get("authorization")
        .expect("Authorization header");
    assert!(auth_header.starts_with("Basic "));
    let b64_part = &auth_header["Basic ".len()..];
    let decoded_auth = String::from_utf8(BASE64_STANDARD.decode(b64_part).unwrap()).unwrap();
    assert_eq!(decoded_auth, format!("{tenant_id}:{management_key}"));

    // 2. SPEKE v2 headers
    assert_eq!(
        r.headers.get("x-speke-version").map(|s| s.as_str()),
        Some("2.0")
    );
    assert_eq!(
        r.headers.get("user-agent").map(|s| s.as_str()),
        Some("drmpack/0.1.0")
    );

    // 3. XML Content-Type and Accept headers
    assert_eq!(
        r.headers.get("content-type").map(|s| s.as_str()),
        Some("application/xml")
    );
    assert_eq!(
        r.headers.get("accept").map(|s| s.as_str()),
        Some("application/xml")
    );

    // 4. No query param by default
    assert!(!r.path.contains("overrideKeyIds"));

    // 5. XML body verification
    assert!(r.body.contains(r#"contentId="axinom-content-1""#));

    // 6. Parsed KeySet verification
    assert_eq!(key_set.len(), 1);
    let key = key_set
        .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
        .expect("HD CENC key");
    assert_eq!(
        key.kid.0,
        Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap()
    );
    assert_eq!(key.key, [1u8; 16]);
}

#[tokio::test]
async fn test_axinom_provider_override_key_ids_query_param() {
    let server = AxinomMockServer::start(200, SAMPLE_AXINOM_RESPONSE_OVERRIDDEN_KID.into()).await;

    let config = AxinomConfig::new("tenant", "key")
        .with_endpoint(&server.url)
        .with_override_key_ids(true);
    let provider = AxinomProvider::new(config);

    let req = KeyRequest::new("axinom-overridden")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_drm_system(DrmSystem::Widevine)
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let key_set = provider.fetch_keys(&req).await.unwrap();

    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].path.contains("overrideKeyIds=true"),
        "Request path must include ?overrideKeyIds=true, got: {}",
        requests[0].path
    );

    // Verify overridden KeyID was parsed successfully into the KeySet
    assert_eq!(key_set.len(), 1);
    let key = key_set
        .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
        .expect("HD CENC key with overridden KID");
    assert_eq!(
        key.kid.0,
        Uuid::parse_str("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap()
    );
}

#[tokio::test]
async fn test_axinom_provider_dual_cenc_cbcs() {
    let server = AxinomMockServer::start(200, SAMPLE_AXINOM_RESPONSE_DUAL.into()).await;

    let config = AxinomConfig::new("tenant", "key").with_endpoint(&server.url);
    let provider = AxinomProvider::new(config);

    let req = KeyRequest::new("axinom-dual-content")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_drm_system(DrmSystem::Widevine)
        .with_drm_system(DrmSystem::FairPlay)
        .with_encryption_scheme(EncryptionScheme::Dual);

    let key_set = provider.fetch_keys(&req).await.unwrap();

    assert_eq!(key_set.len(), 2);
    let cenc_key = key_set
        .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
        .expect("CENC key");
    let cbcs_key = key_set
        .get_key_for_scheme(EncryptionScheme::Cbcs, TrackType::Video, &QualityTier::hd())
        .expect("CBCS key");

    assert_ne!(cenc_key.kid, cbcs_key.kid);
    assert_ne!(cenc_key.key, cbcs_key.key);

    let fairplay = key_set
        .pssh
        .iter()
        .find(|p| p.drm_system == DrmSystem::FairPlay)
        .expect("FairPlay signaling");
    assert_eq!(
        fairplay.data.as_ref(),
        b"skd://22222222-2222-2222-2222-222222222222"
    );
}

#[tokio::test]
async fn test_axinom_provider_custom_headers() {
    let server = AxinomMockServer::start(200, SAMPLE_AXINOM_RESPONSE_CENC.into()).await;

    let config = AxinomConfig::new("tenant", "key")
        .with_endpoint(&server.url)
        .with_header(
            reqwest::header::HeaderName::from_static("x-custom-tenant-tag"),
            reqwest::header::HeaderValue::from_static("staging-cluster-1"),
        );
    let provider = AxinomProvider::new(config);

    let req = KeyRequest::new("axinom-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    provider.fetch_keys(&req).await.unwrap();

    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .headers
            .get("x-custom-tenant-tag")
            .map(|s| s.as_str()),
        Some("staging-cluster-1")
    );
}

#[tokio::test]
async fn test_axinom_provider_error_diagnostics_with_axdrm_header() {
    let server = AxinomMockServer::start_with_headers(
        401,
        "Authentication failed".into(),
        vec![("X-AxDRM-ErrorMessage", "Invalid Management Key for Tenant")],
    )
    .await;

    let config = AxinomConfig::new("tenant", "bad-key").with_endpoint(&server.url);
    let provider = AxinomProvider::new(config);

    let req = KeyRequest::new("axinom-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let result = provider.fetch_keys(&req).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, DrmpackError::KeyProvider(_)));
    let err_msg = err.to_string();

    assert!(err_msg.contains("HTTP 401"));
    assert!(
        err_msg.contains("Invalid Management Key for Tenant"),
        "Error message must include X-AxDRM-ErrorMessage, got: {err_msg}"
    );
}

#[tokio::test]
async fn test_axinom_provider_error_diagnostics_bad_request_header() {
    let server = AxinomMockServer::start_with_headers(
        400,
        "XML parsing failed".into(),
        vec![(
            "X-AxDRM-ErrorMessage",
            "Malformed CPIX 2.3 schema structure",
        )],
    )
    .await;

    let config = AxinomConfig::new("tenant", "key").with_endpoint(&server.url);
    let provider = AxinomProvider::new(config);

    let req = KeyRequest::new("axinom-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let result = provider.fetch_keys(&req).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("HTTP 400"));
    assert!(err_msg.contains("Malformed CPIX 2.3 schema structure"));
}

#[tokio::test]
async fn test_axinom_provider_http_500_fallback() {
    let server =
        AxinomMockServer::start(500, "Internal Server Failure in Axinom Cloud".into()).await;

    let config = AxinomConfig::new("tenant", "key").with_endpoint(&server.url);
    let provider = AxinomProvider::new(config);

    let req = KeyRequest::new("axinom-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let result = provider.fetch_keys(&req).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("HTTP 500"));
    assert!(err_msg.contains("Internal Server Failure in Axinom Cloud"));
}

#[tokio::test]
async fn test_axinom_provider_unreachable_endpoint() {
    let config = AxinomConfig::new("tenant", "key").with_endpoint("http://127.0.0.1:1/speke");
    let provider = AxinomProvider::new(config);

    let req = KeyRequest::new("axinom-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let result = provider.fetch_keys(&req).await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("Failed to send request to Axinom Key Service"));
}

#[tokio::test]
async fn test_axinom_provider_packaging_session_lifecycle() {
    let server = AxinomMockServer::start(200, SAMPLE_AXINOM_RESPONSE_CENC.into()).await;
    let config = AxinomConfig::new("tenant", "key").with_endpoint(&server.url);
    let provider = AxinomProvider::new(config);

    let rendition = Rendition::video_hd();

    let out_dir = std::env::temp_dir().join(format!("drmpack_axinom_session_{}", Uuid::new_v4()));

    let session_config = PackagingSessionConfig::new("axinom-session-test")
        .with_rendition(rendition)
        .with_encryption_scheme(EncryptionScheme::Cenc)
        .with_output_dir(&out_dir)
        .with_gpac_bin("non_existent_gpac_binary_so_spawn_fails");

    // Creating session fetches keys from AxinomProvider, then fails at GPAC spawn
    let result = PackagingSession::create(session_config, &provider).await;
    assert!(result.is_err());
    let err = result.unwrap_err();

    let DrmpackError::PackagingSession(failure) = err else {
        panic!("Expected PackagingSession failure, got: {err}");
    };
    assert_eq!(
        failure.cenc[0].operation,
        drmpack::PackagingOperation::Create
    );

    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");

    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_axinom_provider_live_key_acquisition() {
    // Load .env if present
    let _ = dotenvy::dotenv();

    let config = match AxinomConfig::from_env() {
        Ok(cfg) => cfg,
        Err(_) => {
            println!("SKIPPING test_axinom_provider_live_key_acquisition: AXINOM credentials not configured in environment/.env");
            return;
        }
    };

    // Skip if credentials are placeholders
    if config.tenant_id.contains("00000000-0000")
        || config.tenant_id.to_lowercase().contains("your-")
        || config.management_key.to_lowercase().contains("your-")
    {
        println!("SKIPPING test_axinom_provider_live_key_acquisition: AXINOM credentials appear to be placeholders");
        return;
    }

    println!(
        "Executing live Axinom key acquisition against endpoint: {}",
        config.endpoint
    );
    let provider = AxinomProvider::new(config);

    let content_id = format!("live-dual-{}", Uuid::new_v4());
    let req = KeyRequest::new(&content_id)
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_drm_system(DrmSystem::Widevine)
        .with_drm_system(DrmSystem::FairPlay)
        .with_drm_system(DrmSystem::PlayReady)
        .with_encryption_scheme(EncryptionScheme::Dual);

    let key_set_res = provider.fetch_keys(&req).await;
    match key_set_res {
        Ok(key_set) => {
            assert_eq!(
                key_set.len(),
                2,
                "Live Dual acquisition must return exactly 2 keys (1 CENC, 1 CBCS)"
            );

            // Verify CENC key is present
            let cenc = key_set.get_key_for_scheme(
                EncryptionScheme::Cenc,
                TrackType::Video,
                &QualityTier::hd(),
            );
            assert!(cenc.is_some(), "Live acquisition must return CENC key");
            let cenc_key = cenc.unwrap();
            assert_ne!(
                cenc_key.key, [0u8; 16],
                "CENC key material must not be all zeroes"
            );

            // Verify CBCS key is present
            let cbcs = key_set.get_key_for_scheme(
                EncryptionScheme::Cbcs,
                TrackType::Video,
                &QualityTier::hd(),
            );
            assert!(cbcs.is_some(), "Live acquisition must return CBCS key");
            let cbcs_key = cbcs.unwrap();
            assert_ne!(
                cbcs_key.key, [0u8; 16],
                "CBCS key material must not be all zeroes"
            );

            // Verify distinct key material and KIDs per ADR-0006
            assert_ne!(cenc_key.kid, cbcs_key.kid, "CENC and CBCS KIDs must differ");
            assert_ne!(cenc_key.key, cbcs_key.key, "CENC and CBCS keys must differ");

            // Verify PSSH data
            assert!(
                !key_set.pssh.is_empty(),
                "Live acquisition must return DRM signaling / PSSH"
            );

            println!(
                "Successfully acquired live Axinom Dual keys for content '{}': {} keys, {} PSSH entries",
                content_id,
                key_set.len(),
                key_set.pssh.len()
            );
        }
        Err(e) => {
            panic!("Live Axinom key acquisition failed: {e}");
        }
    }
}

#[tokio::test]
async fn test_axinom_provider_live_cbcs_acquisition() {
    let _ = dotenvy::dotenv();

    let config = match AxinomConfig::from_env() {
        Ok(cfg) => cfg,
        Err(_) => {
            println!("SKIPPING test_axinom_provider_live_cbcs_acquisition: AXINOM credentials not configured");
            return;
        }
    };

    if config.tenant_id.contains("00000000-0000")
        || config.tenant_id.to_lowercase().contains("your-")
        || config.management_key.to_lowercase().contains("your-")
    {
        println!("SKIPPING test_axinom_provider_live_cbcs_acquisition: AXINOM credentials appear to be placeholders");
        return;
    }

    let provider = AxinomProvider::new(config);
    let content_id = format!("live-cbcs-{}", Uuid::new_v4());
    let req = KeyRequest::new(&content_id)
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_drm_system(DrmSystem::FairPlay)
        .with_drm_system(DrmSystem::Widevine)
        .with_encryption_scheme(EncryptionScheme::Cbcs);

    let key_set = provider
        .fetch_keys(&req)
        .await
        .expect("Live CBCS key acquisition must succeed");

    assert_eq!(key_set.len(), 1);
    let cbcs_key = key_set
        .get_key_for_scheme(EncryptionScheme::Cbcs, TrackType::Video, &QualityTier::hd())
        .expect("CBCS key");
    assert_ne!(cbcs_key.key, [0u8; 16]);

    // Check DRM signaling
    let fairplay = key_set
        .pssh
        .iter()
        .find(|p| p.drm_system == DrmSystem::FairPlay);
    assert!(fairplay.is_some(), "FairPlay signaling must be present");
    println!(
        "Successfully acquired live CBCS keys: {} keys, {} PSSH/signaling entries",
        key_set.len(),
        key_set.pssh.len()
    );
}

#[tokio::test]
async fn test_axinom_provider_live_override_key_ids() {
    let _ = dotenvy::dotenv();

    let config = match AxinomConfig::from_env() {
        Ok(cfg) => cfg.with_override_key_ids(true),
        Err(_) => {
            println!("SKIPPING: AXINOM credentials not configured");
            return;
        }
    };

    if config.tenant_id.contains("00000000-0000")
        || config.tenant_id.to_lowercase().contains("your-")
        || config.management_key.to_lowercase().contains("your-")
    {
        println!("SKIPPING: AXINOM credentials appear to be placeholders");
        return;
    }

    let provider = AxinomProvider::new(config);
    let content_id = format!("live-ov-{}", Uuid::new_v4());
    let req = KeyRequest::new(&content_id)
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_tier(TrackType::Video, QualityTier::sd())
        .with_tier(TrackType::Audio, QualityTier::sd())
        .with_drm_system(DrmSystem::Widevine)
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let key_set = provider
        .fetch_keys(&req)
        .await
        .expect("Live key acquisition with overrideKeyIds=true and multiple tiers must succeed");

    assert_eq!(
        key_set.len(),
        3,
        "Expected 3 keys for HD video, SD video, SD audio"
    );
    let hd_key = key_set
        .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
        .expect("CENC HD key");
    let sd_video_key = key_set
        .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::sd())
        .expect("CENC SD video key");
    let sd_audio_key = key_set
        .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Audio, &QualityTier::sd())
        .expect("CENC SD audio key");

    assert_ne!(hd_key.kid, sd_video_key.kid);
    assert_ne!(hd_key.kid, sd_audio_key.kid);
    assert_ne!(sd_video_key.kid, sd_audio_key.kid);
    println!(
        "Successfully acquired live keys with overrideKeyIds=true: HD kid={:?}, SD video kid={:?}, SD audio kid={:?}",
        hd_key.kid, sd_video_key.kid, sd_audio_key.kid
    );
}

#[tokio::test]
async fn test_axinom_provider_live_dual_override_key_ids() {
    let _ = dotenvy::dotenv();

    let config = match AxinomConfig::from_env() {
        Ok(cfg) => cfg.with_override_key_ids(true),
        Err(_) => return,
    };

    if config.tenant_id.contains("00000000-0000")
        || config.tenant_id.to_lowercase().contains("your-")
        || config.management_key.to_lowercase().contains("your-")
    {
        return;
    }

    let provider = AxinomProvider::new(config);
    let content_id = format!("live-dual-ov-{}", Uuid::new_v4());
    let req = KeyRequest::new(&content_id)
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_drm_system(DrmSystem::Widevine)
        .with_drm_system(DrmSystem::FairPlay)
        .with_encryption_scheme(EncryptionScheme::Dual);

    let key_set = provider
        .fetch_keys(&req)
        .await
        .expect("Live Dual acquisition with overrideKeyIds=true must succeed");

    assert_eq!(key_set.len(), 2, "Expected 2 keys (1 CENC, 1 CBCS)");
    let cenc_key = key_set
        .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
        .expect("CENC key");
    let cbcs_key = key_set
        .get_key_for_scheme(EncryptionScheme::Cbcs, TrackType::Video, &QualityTier::hd())
        .expect("CBCS key");

    println!(
        "Dual overrideKeyIds: CENC kid={:?} key={:02x?}, CBCS kid={:?} key={:02x?}",
        cenc_key.kid, cenc_key.key, cbcs_key.kid, cbcs_key.key
    );
    assert_ne!(cenc_key.kid, cbcs_key.kid, "CENC and CBCS KIDs must differ");
    assert_ne!(cenc_key.key, cbcs_key.key, "CENC and CBCS keys must differ");
}

#[tokio::test]
async fn test_axinom_provider_explicit_iv_parsing() {
    let xml_with_iv = r#"<?xml version="1.0" encoding="utf-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc" version="2.3" contentId="test-iv">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="4e53532b-9810-486c-af99-8fc312358e4e" explicitIV="TxvPy617rWTVA/sSYfqPKw==" commonEncryptionScheme="cenc">
      <cpix:Data><pskc:Secret><pskc:PlainValue>Ceblmod1ZS0aOzOV86EGIw==</pskc:PlainValue></pskc:Secret></cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
  <cpix:DRMSystemList>
    <cpix:DRMSystem systemId="edef8ba9-79d6-4ace-a3c8-27dcd51d21ed" kid="4e53532b-9810-486c-af99-8fc312358e4e">
      <cpix:PSSH>AAAAUHBzc2gAAAAA7e+LqXnWSs6jyCfc1R0h7QAAADASEJmZmZmZmZmZMzMwAAAAAAMaBU5hZ3JhIg1UZXN0X0tSX0luZGV4OAFI88aJmwY=</cpix:PSSH>
    </cpix:DRMSystem>
  </cpix:DRMSystemList>
  <cpix:ContentKeyUsageRuleList>
    <cpix:ContentKeyUsageRule kid="4e53532b-9810-486c-af99-8fc312358e4e" intendedTrackType="HD"><cpix:VideoFilter /></cpix:ContentKeyUsageRule>
  </cpix:ContentKeyUsageRuleList>
</cpix:CPIX>"#;

    let server = AxinomMockServer::start(200, xml_with_iv.into()).await;
    let config = AxinomConfig::new("tenant", "key").with_endpoint(&server.url);
    let provider = AxinomProvider::new(config);

    let req = KeyRequest::new("test-iv")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let key_set = provider.fetch_keys(&req).await.unwrap();
    let key = key_set
        .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
        .unwrap();

    assert!(key.iv.is_some(), "Explicit IV must be parsed");
    let expected_iv = BASE64_STANDARD.decode("TxvPy617rWTVA/sSYfqPKw==").unwrap();
    assert_eq!(key.iv.unwrap().as_slice(), expected_iv.as_slice());
}

#[tokio::test]
async fn test_axinom_provider_e2e_real_packaging_cenc() {
    if !media_tools_available() {
        println!(
            "SKIPPING test_axinom_provider_e2e_real_packaging_cenc: 'gpac' or 'ffmpeg' not found"
        );
        return;
    }

    let server = AxinomMockServer::start(200, SAMPLE_AXINOM_RESPONSE_CENC.into()).await;
    let config = AxinomConfig::new("tenant", "key").with_endpoint(&server.url);
    let provider = AxinomProvider::new(config);

    let rendition = Rendition::video_hd();

    let out_dir = std::env::temp_dir().join(format!("drmpack_axinom_e2e_cenc_{}", Uuid::new_v4()));

    let session_config = PackagingSessionConfig::new("axinom-e2e-cenc")
        .with_rendition(rendition)
        .with_latency_mode(LatencyMode::LowLatency)
        .with_segment_duration(1.0)
        .with_chunk_duration(0.2)
        .with_output_dir(&out_dir)
        .with_encryption_scheme(EncryptionScheme::Cenc)
        .with_drm_system(DrmSystem::Widevine);

    let mut session = PackagingSession::create(session_config, &provider)
        .await
        .expect("Failed to create PackagingSession with AxinomProvider");

    let sample_bytes = generate_sample_mp4("sample_axinom_cenc").await;

    session
        .push(sample_bytes)
        .await
        .expect("Failed to push chunk into session");

    session
        .close()
        .await
        .expect("Failed to close session cleanly");

    let live_mpd = out_dir.join("live.mpd");
    assert!(live_mpd.exists(), "live.mpd must exist");
    let mpd_content = tokio::fs::read_to_string(&live_mpd).await.unwrap();
    assert!(
        mpd_content.contains("33333333-3333-3333-3333-333333333333"),
        "DASH MPD must contain Axinom KID"
    );

    let init_mp4 = out_dir.join("video_360p_init.mp4");
    assert!(init_mp4.exists(), "Init segment must exist");
    let init_bytes = tokio::fs::read(&init_mp4).await.unwrap();
    let tenc = find_box(&init_bytes, b"tenc").expect("Init segment must contain tenc box");
    let expected_kid = Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap();
    assert!(
        tenc.windows(16)
            .any(|bytes| bytes == expected_kid.as_bytes()),
        "tenc must carry the Axinom KID"
    );

    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_axinom_provider_e2e_real_packaging_dual() {
    if !media_tools_available() {
        println!(
            "SKIPPING test_axinom_provider_e2e_real_packaging_dual: 'gpac' or 'ffmpeg' not found"
        );
        return;
    }

    let server = AxinomMockServer::start(200, SAMPLE_AXINOM_RESPONSE_DUAL.into()).await;
    let config = AxinomConfig::new("tenant", "key").with_endpoint(&server.url);
    let provider = AxinomProvider::new(config);

    let rendition = Rendition::video_hd();

    let out_dir = std::env::temp_dir().join(format!("drmpack_axinom_e2e_dual_{}", Uuid::new_v4()));

    let session_config = PackagingSessionConfig::new("axinom-e2e-dual")
        .with_rendition(rendition)
        .with_latency_mode(LatencyMode::LowLatency)
        .with_segment_duration(1.0)
        .with_chunk_duration(0.2)
        .with_output_dir(&out_dir)
        .with_encryption_scheme(EncryptionScheme::Dual)
        .with_drm_system(DrmSystem::Widevine)
        .with_drm_system(DrmSystem::FairPlay);

    let mut session = PackagingSession::create(session_config, &provider)
        .await
        .expect("Failed to create Dual PackagingSession with AxinomProvider");

    let sample_bytes = generate_sample_mp4("sample_axinom_dual").await;

    session
        .push(sample_bytes)
        .await
        .expect("Failed to push chunk into Dual session");

    session
        .close()
        .await
        .expect("Failed to close session cleanly");

    // Verify CENC Representation has CENC Axinom KID
    let cenc_init = out_dir.join("cenc/video_360p_init.mp4");
    assert!(cenc_init.exists());
    let cenc_init_bytes = tokio::fs::read(&cenc_init).await.unwrap();
    let cenc_tenc = find_box(&cenc_init_bytes, b"tenc").expect("cenc must have tenc");
    let cenc_kid = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
    assert!(
        cenc_tenc.windows(16).any(|b| b == cenc_kid.as_bytes()),
        "CENC tenc must carry CENC Axinom KID"
    );

    // Verify CBCS Representation has distinct CBCS Axinom KID
    let cbcs_init = out_dir.join("cbcs/video_360p_init.mp4");
    assert!(cbcs_init.exists());
    let cbcs_init_bytes = tokio::fs::read(&cbcs_init).await.unwrap();
    let cbcs_tenc = find_box(&cbcs_init_bytes, b"tenc").expect("cbcs must have tenc");
    let cbcs_kid = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
    assert!(
        cbcs_tenc.windows(16).any(|b| b == cbcs_kid.as_bytes()),
        "CBCS tenc must carry CBCS Axinom KID"
    );

    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}
