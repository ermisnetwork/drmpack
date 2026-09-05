#![cfg(feature = "cpix")]

use bytes::Bytes;
use drmpack::cpix::{CpixConfig, CpixProvider};
use drmpack::error::DrmpackError;
use drmpack::key::{KeyProvider, KeyRequest};
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::{
    DrmSystem, EncryptionScheme, LatencyMode, QualityTier, Rendition, Segment, TrackType,
};
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

struct MockServer {
    url: String,
    recorded_requests: Arc<Mutex<Vec<RecordedRequest>>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl MockServer {
    async fn start(status: u16, response_body: String) -> Self {
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
                                    403 => "Forbidden",
                                    500 => "Internal Server Error",
                                    _ => "Unknown",
                                };

                                let response = format!(
                                    "HTTP/1.1 {} {}\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                    status, reason, body.len(), body
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
            url: format!("http://{addr}/cpix"),
            recorded_requests,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    async fn requests(&self) -> Vec<RecordedRequest> {
        self.recorded_requests.lock().await.clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

const SAMPLE_CPIX_RESPONSE_CENC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc" contentId="mock-content-1">
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

const SAMPLE_CPIX_RESPONSE_DUAL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc" contentId="mock-dual-content">
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

const SAMPLE_CPIX_RESPONSE_MULTI_TIER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc" contentId="multi-tier-content">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa" commonEncryptionScheme="cenc">
      <cpix:Data><pskc:Secret><pskc:PlainValue>c2Rfa2V5X2J5dGVzXzEyMw==</pskc:PlainValue></pskc:Secret></cpix:Data>
    </cpix:ContentKey>
    <cpix:ContentKey kid="bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb" commonEncryptionScheme="cenc">
      <cpix:Data><pskc:Secret><pskc:PlainValue>aGRfa2V5X2J5dGVzXzEyMw==</pskc:PlainValue></pskc:Secret></cpix:Data>
    </cpix:ContentKey>
    <cpix:ContentKey kid="cccccccc-cccc-cccc-cccc-cccccccccccc" commonEncryptionScheme="cenc">
      <cpix:Data><pskc:Secret><pskc:PlainValue>NGtfa2V5X2J5dGVzXzEyMw==</pskc:PlainValue></pskc:Secret></cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
  <cpix:ContentKeyUsageRuleList>
    <cpix:ContentKeyUsageRule kid="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa" intendedTrackType="SD">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
    <cpix:ContentKeyUsageRule kid="bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb" intendedTrackType="HD">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
    <cpix:ContentKeyUsageRule kid="cccccccc-cccc-cccc-cccc-cccccccccccc" intendedTrackType="4K">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
  </cpix:ContentKeyUsageRuleList>
</cpix:CPIX>"#;

#[tokio::test]
async fn test_cpix_provider_fetch_cenc_keys() {
    let server = MockServer::start(200, SAMPLE_CPIX_RESPONSE_CENC.into()).await;
    let provider = CpixProvider::new(&server.url);

    let req = KeyRequest::new("mock-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_drm_system(DrmSystem::Widevine)
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let key_set = provider.fetch_keys(&req).await.unwrap();

    // Verify HTTP request details
    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/cpix");
    assert_eq!(
        requests[0].headers.get("content-type").map(|s| s.as_str()),
        Some("application/xml")
    );
    assert_eq!(
        requests[0].headers.get("accept").map(|s| s.as_str()),
        Some("application/xml")
    );
    assert!(
        !requests[0].headers.contains_key("x-speke-version"),
        "Generic CPIX provider must not send X-Speke-Version header"
    );
    assert!(requests[0].body.contains(r#"contentId="mock-content-1""#));

    // Verify parsed KeySet
    assert_eq!(key_set.len(), 1);
    let key = key_set
        .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
        .expect("CENC HD key must be resolved");

    assert_eq!(
        key.kid.0,
        Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap()
    );
    assert_eq!(key.key, [1u8; 16]);
    assert_eq!(key.encryption_scheme, Some(EncryptionScheme::Cenc));

    // Verify PSSH data
    assert_eq!(key_set.pssh.len(), 1);
    assert_eq!(key_set.pssh[0].drm_system, DrmSystem::Widevine);
}

#[tokio::test]
async fn test_cpix_provider_scheme_aware_dual_keys() {
    let server = MockServer::start(200, SAMPLE_CPIX_RESPONSE_DUAL.into()).await;
    let provider = CpixProvider::new(&server.url);

    let req = KeyRequest::new("mock-dual-content")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_drm_system(DrmSystem::Widevine)
        .with_drm_system(DrmSystem::FairPlay)
        .with_encryption_scheme(EncryptionScheme::Cenc)
        .with_encryption_scheme(EncryptionScheme::Cbcs);

    let key_set = provider.fetch_keys(&req).await.unwrap();

    // Verify that CENC and CBCS keys are distinct in KID and key material
    let cenc_key = key_set
        .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
        .expect("CENC key must be present");

    let cbcs_key = key_set
        .get_key_for_scheme(EncryptionScheme::Cbcs, TrackType::Video, &QualityTier::hd())
        .expect("CBCS key must be present");

    assert_ne!(
        cenc_key.kid, cbcs_key.kid,
        "CENC and CBCS keys must have distinct KIDs per ADR-0006"
    );
    assert_ne!(
        cenc_key.key, cbcs_key.key,
        "CENC and CBCS keys must have distinct key material per ADR-0006"
    );

    assert_eq!(
        cenc_key.kid.0,
        Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()
    );
    assert_eq!(
        cbcs_key.kid.0,
        Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap()
    );

    // Verify DRM signaling: Widevine for CENC, FairPlay for CBCS
    let fairplay = key_set
        .pssh
        .iter()
        .find(|p| p.drm_system == DrmSystem::FairPlay)
        .expect("FairPlay signaling must be present");
    assert_eq!(
        fairplay.data.as_ref(),
        b"skd://22222222-2222-2222-2222-222222222222"
    );

    let widevine = key_set
        .pssh
        .iter()
        .find(|p| p.drm_system == DrmSystem::Widevine)
        .expect("Widevine PSSH must be present");
    assert!(!widevine.data.is_empty());
}

#[tokio::test]
async fn test_cpix_provider_multi_tier_keys() {
    let server = MockServer::start(200, SAMPLE_CPIX_RESPONSE_MULTI_TIER.into()).await;
    let provider = CpixProvider::new(&server.url);

    let req = KeyRequest::new("multi-tier-content")
        .with_tier(TrackType::Video, QualityTier::sd())
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_tier(TrackType::Video, QualityTier::uhd_4k())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let key_set = provider.fetch_keys(&req).await.unwrap();

    assert_eq!(key_set.len(), 3);

    let sd_key = key_set
        .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::sd())
        .expect("SD key missing");
    let hd_key = key_set
        .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
        .expect("HD key missing");
    let uhd_key = key_set
        .get_key_for_scheme(
            EncryptionScheme::Cenc,
            TrackType::Video,
            &QualityTier::uhd_4k(),
        )
        .expect("4K key missing");

    assert_eq!(
        sd_key.kid.0,
        Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap()
    );
    assert_eq!(
        hd_key.kid.0,
        Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap()
    );
    assert_eq!(
        uhd_key.kid.0,
        Uuid::parse_str("cccccccc-cccc-cccc-cccc-cccccccccccc").unwrap()
    );
}

#[tokio::test]
async fn test_cpix_provider_custom_headers() {
    let server = MockServer::start(200, SAMPLE_CPIX_RESPONSE_CENC.into()).await;

    let config = CpixConfig::new(&server.url)
        .with_header(
            reqwest::header::HeaderName::from_static("x-api-key"),
            reqwest::header::HeaderValue::from_static("secret-key-12345"),
        )
        .with_header(
            reqwest::header::HeaderName::from_static("x-tenant-id"),
            reqwest::header::HeaderValue::from_static("tenant-abc"),
        );

    let provider = CpixProvider::with_config(config);

    let req = KeyRequest::new("mock-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    provider.fetch_keys(&req).await.unwrap();

    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers.get("x-api-key").map(|s| s.as_str()),
        Some("secret-key-12345")
    );
    assert_eq!(
        requests[0].headers.get("x-tenant-id").map(|s| s.as_str()),
        Some("tenant-abc")
    );
}

#[tokio::test]
async fn test_cpix_provider_http_500_error() {
    let server = MockServer::start(500, "Internal Server Error from KMS".into()).await;
    let provider = CpixProvider::new(&server.url);

    let req = KeyRequest::new("mock-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let result = provider.fetch_keys(&req).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, DrmpackError::KeyProvider(_)));
    assert!(err.to_string().contains("HTTP 500"));
    assert!(err.to_string().contains("Internal Server Error from KMS"));
}

#[tokio::test]
async fn test_cpix_provider_http_403_error() {
    let server = MockServer::start(403, "Forbidden: Invalid credentials".into()).await;
    let provider = CpixProvider::new(&server.url);

    let req = KeyRequest::new("mock-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let result = provider.fetch_keys(&req).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, DrmpackError::KeyProvider(_)));
    assert!(err.to_string().contains("HTTP 403"));
}

#[tokio::test]
async fn test_cpix_provider_malformed_xml_response() {
    let server = MockServer::start(200, "<CPIX><broken-xml-syntax".into()).await;
    let provider = CpixProvider::new(&server.url);

    let req = KeyRequest::new("mock-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let result = provider.fetch_keys(&req).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, DrmpackError::KeyProvider(_)));
    assert!(err.to_string().contains("CPIX XML syntax error"));
}

#[tokio::test]
async fn test_cpix_provider_unreachable_endpoint() {
    // Port 1 is reserved and typically unreachable locally
    let provider = CpixProvider::new("http://127.0.0.1:1/cpix");

    let req = KeyRequest::new("mock-content-1")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let result = provider.fetch_keys(&req).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, DrmpackError::KeyProvider(_)));
    assert!(err.to_string().contains("Failed to send CPIX request"));
}

#[tokio::test]
async fn test_cpix_provider_in_packaging_session_lifecycle() {
    let server = MockServer::start(200, SAMPLE_CPIX_RESPONSE_CENC.into()).await;
    let provider = CpixProvider::new(&server.url);

    let rendition = Rendition::video(
        "v720p",
        QualityTier::hd(),
        1280,
        720,
        2_500_000,
        "avc1.4d401f",
    );

    let out_dir = std::env::temp_dir().join(format!("drmpack_cpix_session_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::new("cpix-session-test")
        .with_rendition(rendition)
        .with_encryption_scheme(EncryptionScheme::Cenc)
        .with_output_dir(&out_dir)
        .with_gpac_bin("non_existent_gpac_binary_so_spawn_fails");

    // Attempting to create the session will:
    // 1. Fetch keys from the CPIX provider (succeeds!)
    // 2. Prepare output and attempt to spawn GPAC process (fails due to fake binary)
    // This cleanly proves the PackagingSession <-> CpixProvider interaction seam!
    let result = PackagingSession::create(config, provider).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    // Verify it reached GPAC spawn failure, meaning CpixProvider key fetch succeeded!
    let DrmpackError::PackagingSession(failure) = err else {
        panic!("Expected structured PackagingSession failure, got: {err}");
    };
    assert_eq!(
        failure.cenc[0].operation,
        drmpack::PackagingOperation::Create
    );

    // Verify CPIX server received the expected POST request
    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");

    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_cpix_provider_e2e_real_packaging_cenc() {
    if !media_tools_available() {
        println!(
            "SKIPPING test_cpix_provider_e2e_real_packaging_cenc: 'gpac' or 'ffmpeg' not found"
        );
        return;
    }

    let server = MockServer::start(200, SAMPLE_CPIX_RESPONSE_CENC.into()).await;
    let provider = CpixProvider::new(&server.url);

    let rendition = Rendition::video(
        "v720p",
        QualityTier::hd(),
        1280,
        720,
        2_500_000,
        "avc1.4d401f",
    );

    let out_dir = std::env::temp_dir().join(format!("drmpack_cpix_e2e_cenc_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::new("cpix-e2e-cenc")
        .with_rendition(rendition)
        .with_latency_mode(LatencyMode::LowLatency)
        .with_segment_duration(1.0)
        .with_chunk_duration(0.2)
        .with_output_dir(&out_dir)
        .with_encryption_scheme(EncryptionScheme::Cenc)
        .with_drm_system(DrmSystem::Widevine);

    let mut session = PackagingSession::create(config, provider)
        .await
        .expect("Failed to create PackagingSession with CpixProvider");

    let sample_bytes = generate_sample_mp4("sample_cpix_cenc").await;

    session
        .push_segment(Segment {
            rendition_id: "v720p".into(),
            sequence_number: 0,
            duration_seconds: 4.0,
            data: Bytes::from(sample_bytes),
            is_init: false,
        })
        .await
        .expect("Failed to push Segment into session");

    session
        .close()
        .await
        .expect("Failed to close session cleanly");

    let live_mpd = out_dir.join("live.mpd");
    assert!(live_mpd.exists(), "live.mpd must exist");
    let mpd_content = tokio::fs::read_to_string(&live_mpd).await.unwrap();
    assert!(
        mpd_content.contains("33333333-3333-3333-3333-333333333333"),
        "DASH MPD must contain CPIX KID"
    );

    let init_mp4 = out_dir.join("stdin_dashinit.mp4");
    assert!(init_mp4.exists(), "Init segment must exist");
    let init_bytes = tokio::fs::read(&init_mp4).await.unwrap();
    let tenc = find_box(&init_bytes, b"tenc").expect("Init segment must contain tenc box");
    let expected_kid = Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap();
    assert!(
        tenc.windows(16)
            .any(|bytes| bytes == expected_kid.as_bytes()),
        "tenc must carry the CPIX KID"
    );

    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_cpix_provider_e2e_real_packaging_dual() {
    if !media_tools_available() {
        println!(
            "SKIPPING test_cpix_provider_e2e_real_packaging_dual: 'gpac' or 'ffmpeg' not found"
        );
        return;
    }

    let server = MockServer::start(200, SAMPLE_CPIX_RESPONSE_DUAL.into()).await;
    let provider = CpixProvider::new(&server.url);

    let rendition = Rendition::video(
        "v720p",
        QualityTier::hd(),
        1280,
        720,
        2_500_000,
        "avc1.4d401f",
    );

    let out_dir = std::env::temp_dir().join(format!("drmpack_cpix_e2e_dual_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::new("cpix-e2e-dual")
        .with_rendition(rendition)
        .with_latency_mode(LatencyMode::LowLatency)
        .with_segment_duration(1.0)
        .with_chunk_duration(0.2)
        .with_output_dir(&out_dir)
        .with_encryption_scheme(EncryptionScheme::Dual)
        .with_drm_system(DrmSystem::Widevine)
        .with_drm_system(DrmSystem::FairPlay);

    let mut session = PackagingSession::create(config, provider)
        .await
        .expect("Failed to create Dual PackagingSession with CpixProvider");

    let sample_bytes = generate_sample_mp4("sample_cpix_dual").await;

    session
        .push_segment(Segment {
            rendition_id: "v720p".into(),
            sequence_number: 0,
            duration_seconds: 4.0,
            data: Bytes::from(sample_bytes),
            is_init: false,
        })
        .await
        .expect("Failed to push Segment into Dual session");

    session
        .close()
        .await
        .expect("Failed to close session cleanly");

    // Verify CENC Representation has CENC CPIX KID
    let cenc_init = out_dir.join("cenc/stdin_dashinit.mp4");
    assert!(cenc_init.exists());
    let cenc_init_bytes = tokio::fs::read(&cenc_init).await.unwrap();
    let cenc_tenc = find_box(&cenc_init_bytes, b"tenc").expect("cenc must have tenc");
    let cenc_kid = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
    assert!(
        cenc_tenc.windows(16).any(|b| b == cenc_kid.as_bytes()),
        "CENC tenc must carry CENC CPIX KID"
    );

    // Verify CBCS Representation has distinct CBCS CPIX KID
    let cbcs_init = out_dir.join("cbcs/stdin_dashinit.mp4");
    assert!(cbcs_init.exists());
    let cbcs_init_bytes = tokio::fs::read(&cbcs_init).await.unwrap();
    let cbcs_tenc = find_box(&cbcs_init_bytes, b"tenc").expect("cbcs must have tenc");
    let cbcs_kid = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
    assert!(
        cbcs_tenc.windows(16).any(|b| b == cbcs_kid.as_bytes()),
        "CBCS tenc must carry CBCS CPIX KID"
    );

    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[tokio::test]
async fn test_cpix_provider_cdata_and_whitespace_response() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc" contentId="cdata-content">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="55555555-5555-5555-5555-555555555555" commonEncryptionScheme="cenc">
      <cpix:Data>
        <pskc:Secret>
          <pskc:PlainValue><![CDATA[
            AQEBAQEBAQEBAQEBAQEBAQ==
          ]]></pskc:PlainValue>
        </pskc:Secret>
      </cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
  <cpix:DRMSystemList>
    <cpix:DRMSystem kid="55555555-5555-5555-5555-555555555555" systemId="edef8ba9-79d6-4ace-a3c8-27dcd51d21ed">
      <cpix:PSSH><![CDATA[AAAAUHBzc2gAAAAA7e+LqXnWSs6jyCfc1R0h7QAAADASEJmZmZmZmZmZMzMwAAAAAAMaBU5hZ3JhIg1UZXN0X0tSX0luZGV4OAFI88aJmwY=]]></cpix:PSSH>
    </cpix:DRMSystem>
  </cpix:DRMSystemList>
  <cpix:ContentKeyUsageRuleList>
    <cpix:ContentKeyUsageRule kid="55555555-5555-5555-5555-555555555555" intendedTrackType="HD">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
  </cpix:ContentKeyUsageRuleList>
</cpix:CPIX>"#;

    let server = MockServer::start(200, xml.into()).await;
    let provider = CpixProvider::new(&server.url);

    let req = KeyRequest::new("cdata-content")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let key_set = provider
        .fetch_keys(&req)
        .await
        .expect("Key fetch with CDATA and whitespace must succeed");
    assert_eq!(key_set.len(), 1);
    let key = key_set
        .get_key_for_scheme(EncryptionScheme::Cenc, TrackType::Video, &QualityTier::hd())
        .unwrap();
    assert_eq!(key.key, [1u8; 16]);
    assert_eq!(key_set.pssh.len(), 1);
}

#[tokio::test]
async fn test_cpix_provider_encrypted_value_error() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="55555555-5555-5555-5555-555555555555" commonEncryptionScheme="cenc">
      <cpix:Data>
        <pskc:Secret>
          <pskc:EncryptedValue/>
        </pskc:Secret>
      </cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
</cpix:CPIX>"#;

    let server = MockServer::start(200, xml.into()).await;
    let provider = CpixProvider::new(&server.url);

    let req = KeyRequest::new("enc-val-content")
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let result = provider.fetch_keys(&req).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err
        .to_string()
        .contains("encrypted PSKC envelopes are not supported"));
}

#[tokio::test]
async fn test_cpix_provider_multi_tier_uhd_sd_hd() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<cpix:CPIX xmlns:cpix="urn:dashif:org:cpix" xmlns:pskc="urn:ietf:params:xml:ns:keyprov:pskc" contentId="multi-tier-content">
  <cpix:ContentKeyList>
    <cpix:ContentKey kid="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa" commonEncryptionScheme="cenc">
      <cpix:Data><pskc:Secret><pskc:PlainValue>c2Rfa2V5X2J5dGVzXzEyMw==</pskc:PlainValue></pskc:Secret></cpix:Data>
    </cpix:ContentKey>
    <cpix:ContentKey kid="bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb" commonEncryptionScheme="cenc">
      <cpix:Data><pskc:Secret><pskc:PlainValue>aGRfa2V5X2J5dGVzXzEyMw==</pskc:PlainValue></pskc:Secret></cpix:Data>
    </cpix:ContentKey>
    <cpix:ContentKey kid="cccccccc-cccc-cccc-cccc-cccccccccccc" commonEncryptionScheme="cenc">
      <cpix:Data><pskc:Secret><pskc:PlainValue>NGtfa2V5X2J5dGVzXzEyMw==</pskc:PlainValue></pskc:Secret></cpix:Data>
    </cpix:ContentKey>
  </cpix:ContentKeyList>
  <cpix:ContentKeyUsageRuleList>
    <cpix:ContentKeyUsageRule kid="aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa" intendedTrackType="SD">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
    <cpix:ContentKeyUsageRule kid="bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb" intendedTrackType="HD">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
    <cpix:ContentKeyUsageRule kid="cccccccc-cccc-cccc-cccc-cccccccccccc" intendedTrackType="UHD">
      <cpix:VideoFilter/>
    </cpix:ContentKeyUsageRule>
  </cpix:ContentKeyUsageRuleList>
</cpix:CPIX>"#;

    let server = MockServer::start(200, xml.into()).await;
    let provider = CpixProvider::new(&server.url);

    let req = KeyRequest::new("multi-tier-content")
        .with_tier(TrackType::Video, QualityTier::sd())
        .with_tier(TrackType::Video, QualityTier::hd())
        .with_tier(TrackType::Video, QualityTier::uhd_4k())
        .with_encryption_scheme(EncryptionScheme::Cenc);

    let key_set = provider.fetch_keys(&req).await.unwrap();
    assert_eq!(key_set.len(), 3);

    let uhd_key = key_set
        .get_key_for_scheme(
            EncryptionScheme::Cenc,
            TrackType::Video,
            &QualityTier::uhd_4k(),
        )
        .expect("UHD key must resolve to QualityTier::uhd_4k()");
    assert_eq!(
        uhd_key.kid.0,
        Uuid::parse_str("cccccccc-cccc-cccc-cccc-cccccccccccc").unwrap()
    );
}

#[tokio::test]
async fn test_cpix_provider_selective_encryption_clear_audio() {
    let server = MockServer::start(200, SAMPLE_CPIX_RESPONSE_CENC.into()).await;
    let provider = CpixProvider::new(&server.url);

    let video_rendition = Rendition::video(
        "v720p",
        QualityTier::hd(),
        1280,
        720,
        2_500_000,
        "avc1.4d401f",
    );
    let audio_rendition =
        Rendition::audio("a_clear", QualityTier::sd(), 128_000, "mp4a.40.2").clear();

    let out_dir = std::env::temp_dir().join(format!("drmpack_cpix_selective_{}", Uuid::new_v4()));
    let control_dir =
        std::env::temp_dir().join(format!("drmpack_control_selective_{}", Uuid::new_v4()));

    let config = PackagingSessionConfig::new("cpix-selective-content")
        .with_rendition(video_rendition)
        .with_rendition(audio_rendition)
        .with_encryption_scheme(EncryptionScheme::Cenc)
        .with_output_dir(&out_dir)
        .with_control_dir(&control_dir)
        .with_gpac_bin("gpac");

    let mut session = PackagingSession::create(config, provider)
        .await
        .expect("PackagingSession::create must succeed for selective encryption with CPIX");

    let drm_xml = tokio::fs::read_to_string(session.control_dir_path().join("cenc.xml"))
        .await
        .unwrap();

    // Verify track 1 (Video) is encrypted
    assert!(drm_xml.contains(r#"<CrypTrack trackID="1" IsEncrypted="1""#));
    // Verify track 2 (Audio) is unencrypted
    assert!(drm_xml.contains(r#"<CrypTrack trackID="2" IsEncrypted="0"/>"#));

    // Verify CPIX request body only requested Video, since Audio is unencrypted
    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].body.contains(r#"intendedTrackType="UHD,HD""#)
            || requests[0].body.contains(r#"intendedTrackType="HD""#)
    );
    assert!(!requests[0].body.contains(r#"intendedTrackType="AUDIO""#));

    let _ = session.close().await;
    let _ = session.cleanup().await;
    let _ = tokio::fs::remove_dir_all(&out_dir).await;
}

#[test]
fn test_cpix_provider_exports() {
    use drmpack::key::CpixProvider as KeyCpixProvider;
    use drmpack::CpixProvider as RootCpixProvider;

    let p1 = RootCpixProvider::new("http://example.com/cpix");
    let p2 = KeyCpixProvider::new("http://example.com/cpix");
    assert_eq!(p1.config().endpoint, "http://example.com/cpix");
    assert_eq!(p2.config().endpoint, "http://example.com/cpix");
}
