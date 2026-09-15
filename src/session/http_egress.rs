//! In-process HTTP egress server acting as an in-memory loopback sink for GPAC `httpout:hmode=push`.
//!
//! Provides [`HttpEgressServer`], which binds an ephemeral port on `127.0.0.1` and receives
//! packaged artifacts pushed by GPAC via HTTP PUT/POST. Incoming payloads are verified for
//! authorization and ISOBMFF box integrity before being emitted directly to callers as
//! [`PackagedArtifact`]s without filesystem staging.

use crate::error::{DrmpackError, Result};
use crate::session::isobmff::{
    is_complete_isobmff_init_segment, is_complete_isobmff_media_segment,
};
use crate::types::{ArtifactKind, EncryptionScheme, PackagedArtifact};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{debug, warn};

/// Maximum allowable request body payload (64 MB).
const MAX_PAYLOAD_SIZE: usize = 64 * 1024 * 1024;

/// In-process HTTP egress server acting as an in-memory loopback sink for GPAC `httpout:hmode=push`.
pub struct HttpEgressServer {
    local_addr: SocketAddr,
    auth_token: String,
    shutdown_token: CancellationToken,
    server_task: Option<JoinHandle<()>>,
    conn_tracker: TaskTracker,
}

impl std::fmt::Debug for HttpEgressServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpEgressServer")
            .field("local_addr", &self.local_addr)
            .field("endpoint_url", &self.endpoint_url())
            .finish_non_exhaustive()
    }
}

impl Drop for HttpEgressServer {
    fn drop(&mut self) {
        self.shutdown_token.cancel();
        self.conn_tracker.close();
        if let Some(task) = self.server_task.take() {
            task.abort();
        }
    }
}

type BoxBody = Full<Bytes>;

fn full_body(msg: &'static str) -> BoxBody {
    Full::new(Bytes::from_static(msg.as_bytes()))
}

/// Classify an incoming artifact path or filename into its corresponding [`ArtifactKind`].
fn classify_artifact_path(path: &str) -> Option<ArtifactKind> {
    if path.ends_with(".m3u8") || path.ends_with(".mpd") {
        Some(ArtifactKind::Manifest)
    } else if path.ends_with(".m4s") {
        Some(ArtifactKind::MediaSegment)
    } else if path.ends_with("init.mp4") {
        Some(ArtifactKind::InitSegment)
    } else {
        None
    }
}

fn parse_relative_path(
    rel_path: &str,
    default_scheme: EncryptionScheme,
) -> std::result::Result<(String, EncryptionScheme), &'static str> {
    let rel_path = rel_path.trim_start_matches('/');
    if rel_path.is_empty() {
        return Err("Empty artifact path");
    }

    let (subpath, scheme) = if let Some(sub) = rel_path.strip_prefix("cbcs/") {
        (sub, EncryptionScheme::Cbcs)
    } else if let Some(sub) = rel_path.strip_prefix("cenc/") {
        (sub, EncryptionScheme::Cenc)
    } else {
        (rel_path, default_scheme)
    };

    let filename = match std::path::Path::new(subpath)
        .file_name()
        .and_then(|n| n.to_str())
    {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => return Err("Invalid filename in path"),
    };

    Ok((filename, scheme))
}

async fn handle_request(
    req: Request<Incoming>,
    token_prefix: Arc<String>,
    artifact_tx: mpsc::Sender<PackagedArtifact>,
    default_scheme: EncryptionScheme,
) -> std::result::Result<Response<BoxBody>, Infallible> {
    // 1. Verify HTTP method is PUT or POST
    if req.method() != Method::PUT && req.method() != Method::POST {
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .header(hyper::header::ALLOW, "PUT, POST")
            .body(full_body("Method Not Allowed"))
            .unwrap());
    }

    // 2. Verify URI path starts with precomputed token prefix (e.g. /{auth_token}/)
    let req_path = req.uri().path();
    let rel_path = match req_path.strip_prefix(token_prefix.as_str()) {
        Some(p) if !p.is_empty() => p,
        _ => {
            return Ok(Response::builder()
                .status(StatusCode::FORBIDDEN)
                .body(full_body("Forbidden"))
                .unwrap());
        }
    };

    // 3. Parse scheme and filename from relative path
    let (filename, scheme) = match parse_relative_path(rel_path, default_scheme) {
        Ok(parsed) => parsed,
        Err(err) => {
            warn!(path = %req_path, error = %err, "Invalid request path in HttpEgressServer");
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(full_body(err))
                .unwrap());
        }
    };

    // 4. Collect request body with bounded payload size limit (64 MB)
    let body_bytes = match Limited::new(req.into_body(), MAX_PAYLOAD_SIZE)
        .collect()
        .await
    {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            if e.is::<LengthLimitError>() {
                warn!(filename = %filename, "Payload exceeded 64 MB limit in HttpEgressServer");
                return Ok(Response::builder()
                    .status(StatusCode::PAYLOAD_TOO_LARGE)
                    .body(full_body("Payload Too Large"))
                    .unwrap());
            }
            warn!(error = %e, "Failed to read request body in HttpEgressServer");
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(full_body("Failed to read body"))
                .unwrap());
        }
    };

    // 5. Determine ArtifactKind
    let kind = match classify_artifact_path(&filename) {
        Some(k) => k,
        None => {
            warn!(filename = %filename, "Unknown artifact type received in HttpEgressServer");
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(full_body("Unknown artifact type"))
                .unwrap());
        }
    };

    // 6. Validate binary ISOBMFF boxes for InitSegment and MediaSegment
    let is_valid = match kind {
        ArtifactKind::InitSegment => is_complete_isobmff_init_segment(&body_bytes),
        ArtifactKind::MediaSegment => is_complete_isobmff_media_segment(&body_bytes),
        ArtifactKind::Manifest => true,
    };
    if !is_valid {
        warn!(filename = %filename, kind = ?kind, "Invalid ISOBMFF segment received in HttpEgressServer");
        return Ok(Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(full_body("Invalid ISOBMFF segment"))
            .unwrap());
    }

    // 7. Construct PackagedArtifact
    let artifact = PackagedArtifact {
        filename: filename.clone(),
        data: body_bytes,
        kind,
        scheme,
    };

    // 8. Forward to artifact channel respecting backpressure
    if artifact_tx.send(artifact).await.is_err() {
        debug!(filename = %filename, "HttpEgressServer: artifact receiver dropped, discarding artifact");
    }

    // 9. Return HTTP 200 OK
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_LENGTH, "0")
        .body(Full::default())
        .unwrap())
}

impl HttpEgressServer {
    /// Start a new `HttpEgressServer` loopback instance on an ephemeral port.
    ///
    /// The default fallback scheme is [`EncryptionScheme::Cbcs`].
    pub async fn start(
        auth_token: String,
        artifact_tx: mpsc::Sender<PackagedArtifact>,
    ) -> Result<Self> {
        Self::start_with_scheme(auth_token, artifact_tx, EncryptionScheme::Cbcs).await
    }

    /// Start a new `HttpEgressServer` with an explicit fallback scheme for unscoped manifest/init paths.
    pub async fn start_with_scheme(
        auth_token: String,
        artifact_tx: mpsc::Sender<PackagedArtifact>,
        default_scheme: EncryptionScheme,
    ) -> Result<Self> {
        if auth_token.trim().is_empty() {
            return Err(DrmpackError::Validation(
                "auth_token cannot be empty".into(),
            ));
        }

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(DrmpackError::Io)?;
        let local_addr = listener.local_addr().map_err(DrmpackError::Io)?;
        let shutdown_token = CancellationToken::new();
        let loop_token = shutdown_token.clone();

        let token_prefix = Arc::new(format!("/{auth_token}/"));
        let conn_tracker = TaskTracker::new();
        let task_tracker = conn_tracker.clone();

        let server_task = tokio::spawn(async move {
            let auto = auto::Builder::new(TokioExecutor::new());

            loop {
                let (stream, _) = tokio::select! {
                    _ = loop_token.cancelled() => break,
                    res = listener.accept() => {
                        match res {
                            Ok(conn) => conn,
                            Err(e) => {
                                debug!(error = %e, "HttpEgressServer: accept error");
                                continue;
                            }
                        }
                    }
                };

                let io = TokioIo::new(stream);
                let auto = auto.clone();
                let token_prefix = Arc::clone(&token_prefix);
                let tx = artifact_tx.clone();
                let conn_token = loop_token.clone();

                task_tracker.spawn(async move {
                    let service = service_fn(move |req| {
                        handle_request(req, Arc::clone(&token_prefix), tx.clone(), default_scheme)
                    });

                    let conn = auto.serve_connection_with_upgrades(io, service);
                    tokio::pin!(conn);

                    tokio::select! {
                        res = conn.as_mut() => {
                            if let Err(err) = res {
                                debug!(error = %err, "HttpEgressServer: connection error");
                            }
                        }
                        _ = conn_token.cancelled() => {
                            conn.as_mut().graceful_shutdown();
                            let _ = tokio::time::timeout(Duration::from_secs(2), conn.as_mut()).await;
                        }
                    }
                });
            }

            task_tracker.close();
            task_tracker.wait().await;
        });

        Ok(Self {
            local_addr,
            auth_token,
            shutdown_token,
            server_task: Some(server_task),
            conn_tracker,
        })
    }

    /// The local socket address the server is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The base HTTP endpoint URL including the authentication token prefix.
    ///
    /// E.g. `http://127.0.0.1:{port}/{token}`
    pub fn endpoint_url(&self) -> String {
        format!("http://{}/{}", self.local_addr, self.auth_token)
    }

    /// Gracefully shut down the server, draining in-flight requests and waiting for the listener task to terminate.
    pub async fn shutdown(mut self) {
        self.shutdown_token.cancel();
        self.conn_tracker.close();
        if let Some(task) = self.server_task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), task).await;
        }
        let _ = tokio::time::timeout(Duration::from_secs(3), self.conn_tracker.wait()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_box(box_type: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = (8 + payload.len()) as u32;
        let mut buf = Vec::with_capacity(size as usize);
        buf.extend_from_slice(&size.to_be_bytes());
        buf.extend_from_slice(box_type);
        buf.extend_from_slice(payload);
        buf
    }

    fn valid_media_segment() -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&make_box(b"moof", b"moof_sample_data"));
        data.extend_from_slice(&make_box(b"mdat", b"mdat_media_samples"));
        data
    }

    fn valid_init_segment() -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&make_box(b"ftyp", b"iso6mp41"));
        data.extend_from_slice(&make_box(b"moov", b"moov_metadata"));
        data
    }

    #[tokio::test]
    async fn test_http_egress_server_put_manifest_and_segment() {
        let (tx, mut rx) = mpsc::channel(16);
        let token = "test-auth-token-123";
        let server = HttpEgressServer::start(token.to_string(), tx)
            .await
            .expect("server start failed");
        let client = reqwest::Client::new();

        // 1. Valid manifest PUT
        let manifest_url = format!("{}/cbcs/video_720p.m3u8", server.endpoint_url());
        let res = client
            .put(&manifest_url)
            .body("#EXTM3U\n#EXT-X-VERSION:7\n")
            .send()
            .await
            .expect("manifest request failed");
        assert_eq!(res.status(), reqwest::StatusCode::OK);

        let artifact = rx.recv().await.expect("artifact not received");
        assert_eq!(artifact.filename, "video_720p.m3u8");
        assert_eq!(artifact.scheme, EncryptionScheme::Cbcs);
        assert_eq!(artifact.kind, ArtifactKind::Manifest);
        assert_eq!(artifact.data.as_ref(), b"#EXTM3U\n#EXT-X-VERSION:7\n");

        // 2. Valid media segment PUT
        let segment_url = format!("{}/cbcs/video_720p_1.m4s", server.endpoint_url());
        let seg_data = valid_media_segment();
        let res = client
            .put(&segment_url)
            .body(seg_data.clone())
            .send()
            .await
            .expect("segment request failed");
        assert_eq!(res.status(), reqwest::StatusCode::OK);

        let artifact = rx.recv().await.expect("segment artifact not received");
        assert_eq!(artifact.filename, "video_720p_1.m4s");
        assert_eq!(artifact.scheme, EncryptionScheme::Cbcs);
        assert_eq!(artifact.kind, ArtifactKind::MediaSegment);
        assert_eq!(artifact.data.as_ref(), seg_data.as_slice());

        // 3. Valid init segment PUT
        let init_url = format!("{}/cbcs/video_720p_init.mp4", server.endpoint_url());
        let init_data = valid_init_segment();
        let res = client
            .put(&init_url)
            .body(init_data.clone())
            .send()
            .await
            .expect("init segment request failed");
        assert_eq!(res.status(), reqwest::StatusCode::OK);

        let artifact = rx.recv().await.expect("init artifact not received");
        assert_eq!(artifact.filename, "video_720p_init.mp4");
        assert_eq!(artifact.scheme, EncryptionScheme::Cbcs);
        assert_eq!(artifact.kind, ArtifactKind::InitSegment);
        assert_eq!(artifact.data.as_ref(), init_data.as_slice());

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_wrong_auth_token_returns_403_forbidden() {
        let (tx, mut rx) = mpsc::channel(16);
        let token = "correct-token";
        let server = HttpEgressServer::start(token.to_string(), tx)
            .await
            .expect("server start failed");
        let client = reqwest::Client::new();

        let bad_url = format!("http://{}/wrong-token/cbcs/live.mpd", server.local_addr());
        let res = client
            .put(&bad_url)
            .body("<MPD/>")
            .send()
            .await
            .expect("request failed");
        assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);

        // Verify no artifact was emitted
        assert!(rx.try_recv().is_err());

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_empty_auth_token_rejected() {
        let (tx, _rx) = mpsc::channel(16);
        let res_empty = HttpEgressServer::start("".to_string(), tx.clone()).await;
        assert!(
            matches!(res_empty, Err(DrmpackError::Validation(msg)) if msg.contains("auth_token cannot be empty"))
        );

        let res_whitespace = HttpEgressServer::start("   ".to_string(), tx).await;
        assert!(
            matches!(res_whitespace, Err(DrmpackError::Validation(msg)) if msg.contains("auth_token cannot be empty"))
        );
    }

    #[tokio::test]
    async fn test_method_not_allowed() {
        let (tx, mut rx) = mpsc::channel(16);
        let token = "token-method-test";
        let server = HttpEgressServer::start(token.to_string(), tx)
            .await
            .expect("server start failed");
        let client = reqwest::Client::new();

        let url = format!("{}/cbcs/video_720p.m3u8", server.endpoint_url());

        // GET should return 405 Method Not Allowed
        let res = client.get(&url).send().await.expect("GET request failed");
        assert_eq!(res.status(), reqwest::StatusCode::METHOD_NOT_ALLOWED);
        let allow = res
            .headers()
            .get(reqwest::header::ALLOW)
            .and_then(|v| v.to_str().ok());
        assert_eq!(allow, Some("PUT, POST"));

        // DELETE should return 405 Method Not Allowed
        let res = client
            .delete(&url)
            .send()
            .await
            .expect("DELETE request failed");
        assert_eq!(res.status(), reqwest::StatusCode::METHOD_NOT_ALLOWED);

        // Verify no artifact was emitted
        assert!(rx.try_recv().is_err());

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_dual_scheme_routing() {
        let (tx, mut rx) = mpsc::channel(16);
        let token = "dual-routing-token";
        let server = HttpEgressServer::start(token.to_string(), tx)
            .await
            .expect("server start failed");
        let client = reqwest::Client::new();

        let seg_data = valid_media_segment();

        // 1. CBCS routing via /cbcs/ path prefix
        let cbcs_url = format!("{}/cbcs/video_1080p_1.m4s", server.endpoint_url());
        let res = client
            .put(&cbcs_url)
            .body(seg_data.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), reqwest::StatusCode::OK);
        let art_cbcs = rx.recv().await.unwrap();
        assert_eq!(art_cbcs.scheme, EncryptionScheme::Cbcs);
        assert_eq!(art_cbcs.filename, "video_1080p_1.m4s");
        assert_eq!(art_cbcs.kind, ArtifactKind::MediaSegment);

        // 2. CENC routing via /cenc/ path prefix
        let cenc_url = format!("{}/cenc/video_1080p_1.m4s", server.endpoint_url());
        let res = client
            .put(&cenc_url)
            .body(seg_data.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), reqwest::StatusCode::OK);
        let art_cenc = rx.recv().await.unwrap();
        assert_eq!(art_cenc.scheme, EncryptionScheme::Cenc);
        assert_eq!(art_cenc.filename, "video_1080p_1.m4s");
        assert_eq!(art_cenc.kind, ArtifactKind::MediaSegment);

        // 3. Unscoped manifest fallback to Cbcs
        let mpd_url = format!("{}/live.mpd", server.endpoint_url());
        let res = client
            .put(&mpd_url)
            .body("<MPD>manifest</MPD>")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), reqwest::StatusCode::OK);
        let art_mpd = rx.recv().await.unwrap();
        assert_eq!(art_mpd.scheme, EncryptionScheme::Cbcs);
        assert_eq!(art_mpd.filename, "live.mpd");
        assert_eq!(art_mpd.kind, ArtifactKind::Manifest);

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_graceful_shutdown() {
        let (tx, mut rx) = mpsc::channel(16);
        let token = "shutdown-token";
        let mut server = HttpEgressServer::start(token.to_string(), tx)
            .await
            .expect("server start failed");
        let endpoint_url = server.endpoint_url();
        let client = reqwest::Client::new();

        // 1. Verify endpoint is responsive before shutdown
        let test_url = format!("{}/cbcs/video_720p.m3u8", endpoint_url);
        let res = client
            .put(&test_url)
            .body("#EXTM3U\n")
            .send()
            .await
            .expect("PUT before shutdown failed");
        assert_eq!(res.status(), reqwest::StatusCode::OK);
        assert!(rx.recv().await.is_some());

        // 2. Trigger cancellation and verify server_task completes cleanly
        let server_task = server.server_task.take().expect("server_task must exist");
        server.shutdown_token.cancel();
        let task_result = tokio::time::timeout(Duration::from_secs(3), server_task).await;
        assert!(task_result.is_ok(), "server_task did not complete in time");
        assert!(
            task_result.unwrap().is_ok(),
            "server_task panicked or failed"
        );

        // 3. Close and drain active connections
        server.conn_tracker.close();
        let drain_result =
            tokio::time::timeout(Duration::from_secs(3), server.conn_tracker.wait()).await;
        assert!(
            drain_result.is_ok(),
            "connection tasks did not drain in time"
        );

        // 4. Verify request to endpoint_url returns error (connection refused / unreachable)
        let res_after = client.put(&test_url).body("#EXTM3U\n").send().await;
        assert!(
            res_after.is_err(),
            "Server should reject connections to endpoint_url after shutdown: got {res_after:?}"
        );
    }

    #[tokio::test]
    async fn test_server_shutdown_method() {
        let (tx, _rx) = mpsc::channel(16);
        let token = "shutdown-method-token";
        let server = HttpEgressServer::start(token.to_string(), tx)
            .await
            .expect("server start failed");
        let endpoint_url = server.endpoint_url();
        let client = reqwest::Client::new();

        let test_url = format!("{}/cbcs/video_720p.m3u8", endpoint_url);
        let res = client
            .put(&test_url)
            .body("#EXTM3U\n")
            .send()
            .await
            .expect("PUT before shutdown failed");
        assert_eq!(res.status(), reqwest::StatusCode::OK);

        // Call graceful shutdown convenience method
        server.shutdown().await;

        let res_after = client.put(&test_url).body("#EXTM3U\n").send().await;
        assert!(res_after.is_err());
    }

    #[tokio::test]
    async fn test_invalid_isobmff_box_returns_400() {
        let (tx, mut rx) = mpsc::channel(16);
        let token = "validation-token";
        let server = HttpEgressServer::start(token.to_string(), tx)
            .await
            .expect("server start failed");
        let client = reqwest::Client::new();

        // Corrupt media segment (missing moof/mdat)
        let corrupt_url = format!("{}/cbcs/video_corrupt.m4s", server.endpoint_url());
        let res = client
            .put(&corrupt_url)
            .body(b"plain corrupt bytes".to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), reqwest::StatusCode::BAD_REQUEST);

        // Corrupt init segment (missing ftyp/moov)
        let corrupt_init_url = format!("{}/cbcs/video_corrupt_init.mp4", server.endpoint_url());
        let res = client
            .put(&corrupt_init_url)
            .body(b"plain corrupt bytes".to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), reqwest::StatusCode::BAD_REQUEST);

        // Unknown artifact file extension
        let unknown_url = format!("{}/cbcs/unknown.xyz", server.endpoint_url());
        let res = client
            .put(&unknown_url)
            .body(b"data".to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), reqwest::StatusCode::BAD_REQUEST);

        // Verify no artifact was emitted
        assert!(rx.try_recv().is_err());

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_payload_limit_64mb() {
        let (tx, _rx) = mpsc::channel(16);
        let token = "payload-limit-token";
        let server = HttpEgressServer::start(token.to_string(), tx)
            .await
            .expect("server start failed");
        let client = reqwest::Client::new();

        // 64 MB + 1 byte payload exceeds 64 MB limit
        let oversized = vec![0u8; 64 * 1024 * 1024 + 1];
        let url = format!("{}/cbcs/video_720p_1.m4s", server.endpoint_url());
        let res = client
            .put(&url)
            .body(oversized)
            .send()
            .await
            .expect("request failed");
        assert_eq!(res.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);

        server.shutdown().await;
    }

    #[test]
    fn test_classify_artifact_path_order_and_rules() {
        // Manifest: .m3u8 and .mpd
        assert_eq!(
            classify_artifact_path("master.m3u8"),
            Some(ArtifactKind::Manifest)
        );
        assert_eq!(
            classify_artifact_path("stream.mpd"),
            Some(ArtifactKind::Manifest)
        );

        // Media segment: .m4s MUST be checked before init!
        assert_eq!(
            classify_artifact_path("video_init_1.m4s"),
            Some(ArtifactKind::MediaSegment)
        );
        assert_eq!(
            classify_artifact_path("segment_0001.m4s"),
            Some(ArtifactKind::MediaSegment)
        );

        // Init segment: init.mp4 or _init.mp4
        assert_eq!(
            classify_artifact_path("init.mp4"),
            Some(ArtifactKind::InitSegment)
        );
        assert_eq!(
            classify_artifact_path("video_720p_init.mp4"),
            Some(ArtifactKind::InitSegment)
        );

        // Unknown
        assert_eq!(classify_artifact_path("unknown.txt"), None);
        assert_eq!(classify_artifact_path("video.mp4"), None);
    }

    #[tokio::test]
    async fn test_receiver_dropped_discards_gracefully() {
        let (tx, rx) = mpsc::channel(16);
        let token = "drop-token";
        let server = HttpEgressServer::start(token.to_string(), tx)
            .await
            .expect("server start failed");
        let client = reqwest::Client::new();

        // Drop the receiver to simulate downstream crash / teardown
        drop(rx);

        let manifest_url = format!("{}/cbcs/video_720p.m3u8", server.endpoint_url());
        let res = client
            .put(&manifest_url)
            .body("#EXTM3U\n")
            .send()
            .await
            .expect("request failed");
        assert_eq!(res.status(), reqwest::StatusCode::OK);

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_post_method_allowed() {
        let (tx, mut rx) = mpsc::channel(16);
        let token = "post-token";
        let server = HttpEgressServer::start(token.to_string(), tx)
            .await
            .expect("server start failed");
        let client = reqwest::Client::new();

        let manifest_url = format!("{}/cbcs/video_720p.m3u8", server.endpoint_url());
        let res = client
            .post(&manifest_url)
            .body("#EXTM3U\n")
            .send()
            .await
            .expect("POST request failed");
        assert_eq!(res.status(), reqwest::StatusCode::OK);

        let artifact = rx.recv().await.expect("artifact not received");
        assert_eq!(artifact.filename, "video_720p.m3u8");

        server.shutdown().await;
    }
}
