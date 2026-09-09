use axum::{
    extract::{Path as AxumPath, Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::prelude::*;
use drmpack::license::LicenseProxy;
use drmpack::types::LatencyMode;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};

pub fn resolve_mime_type(path: &str) -> &'static str {
    if path.ends_with(".mpd") {
        "application/dash+xml"
    } else if path.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else if path.ends_with(".m4s") || path.ends_with(".mp4") {
        "video/mp4"
    } else if path.ends_with(".vtt") {
        "text/vtt"
    } else if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".cer") || path.ends_with(".der") {
        "application/x-x509-ca-cert"
    } else {
        "application/octet-stream"
    }
}

pub fn decode_percent(s: &str) -> String {
    let mut result = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                result.push(byte);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&result).into_owned()
}

pub fn parse_range_header_str(val: &str, total_len: usize) -> Option<(usize, usize)> {
    let bytes_part = val.trim().strip_prefix("bytes=")?;
    let mut parts = bytes_part.split('-');
    let start_str = parts.next()?.trim();
    let end_str = parts.next()?.trim();
    let start = start_str.parse::<usize>().ok()?;
    let end = if end_str.is_empty() {
        total_len.saturating_sub(1)
    } else {
        end_str
            .parse::<usize>()
            .ok()?
            .min(total_len.saturating_sub(1))
    };
    if start <= end && start < total_len {
        Some((start, end))
    } else {
        None
    }
}

#[derive(Clone)]
pub struct PlaybackServerState {
    pub cdn_dir: PathBuf,
    pub manifest_url: String,
    pub latency_mode: LatencyMode,
    pub drm_scheme: String,
    pub license_proxy: Option<LicenseProxy>,
    pub auth_token: Option<String>,
    pub kids: Vec<String>,
    pub clearkey_keys: HashMap<String, String>, // hex kid (lowercase, no hyphens) -> hex key
}

pub struct PlaybackServer {
    state: PlaybackServerState,
}

impl PlaybackServer {
    pub fn new(cdn_dir: impl Into<PathBuf>, manifest_url: impl Into<String>) -> Self {
        Self {
            state: PlaybackServerState {
                cdn_dir: cdn_dir.into(),
                manifest_url: manifest_url.into(),
                latency_mode: LatencyMode::Standard,
                drm_scheme: "none".to_string(),
                license_proxy: None,
                auth_token: None,
                kids: Vec::new(),
                clearkey_keys: HashMap::new(),
            },
        }
    }

    pub fn with_latency_mode(mut self, mode: LatencyMode) -> Self {
        self.state.latency_mode = mode;
        self
    }

    pub fn with_drm_scheme(mut self, scheme: impl Into<String>) -> Self {
        self.state.drm_scheme = scheme.into();
        self
    }

    pub fn with_license_proxy(
        mut self,
        proxy: LicenseProxy,
        auth_token: impl Into<String>,
    ) -> Self {
        self.state.license_proxy = Some(proxy);
        self.state.auth_token = Some(auth_token.into());
        self
    }

    pub fn with_kids(mut self, kids: Vec<String>) -> Self {
        self.state.kids = kids;
        self
    }

    pub fn with_clearkey(mut self, kid_hex: impl Into<String>, key_hex: impl Into<String>) -> Self {
        let kid = kid_hex.into().replace('-', "").to_lowercase();
        let key = key_hex.into().replace('-', "").to_lowercase();
        self.state.clearkey_keys.insert(kid, key);
        self
    }

    pub fn into_router(self) -> Router {
        let state = Arc::new(self.state);

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
            .expose_headers([
                header::CONTENT_LENGTH,
                header::CONTENT_TYPE,
                header::CONTENT_RANGE,
                header::ACCEPT_RANGES,
                header::DATE,
                axum::http::header::HeaderName::from_static("x-axdrm-errormessage"),
            ]);

        Router::new()
            .route("/", get(handle_player_html))
            .route("/player.html", get(handle_player_html))
            .route("/api/session-info", get(handle_session_info))
            .route("/fairplay.cer", get(handle_fairplay_cert))
            .route("/license/widevine", post(handle_widevine_license))
            .route("/license/fairplay", post(handle_fairplay_license))
            .route("/license/playready", post(handle_playready_license))
            .route("/license/clearkey", post(handle_clearkey_license))
            .route("/{*path}", get(serve_file).head(serve_file))
            .layer(cors)
            .with_state(state)
    }

    pub async fn run(self, listener: TcpListener, mut shutdown_rx: broadcast::Receiver<()>) {
        let app = self.into_router();
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.recv().await;
            })
            .await;
    }
}

#[derive(serde::Serialize)]
struct SessionInfoResponse {
    manifest_url: String,
    latency_mode: String,
    drm_scheme: String,
    drm_system: String,
    kids: Vec<String>,
    clear_keys: HashMap<String, String>,
}

async fn handle_session_info(
    State(state): State<Arc<PlaybackServerState>>,
) -> Json<SessionInfoResponse> {
    let drm_system = if !state.clearkey_keys.is_empty() {
        "clearkey".to_string()
    } else if state.license_proxy.is_some() {
        "axinom".to_string()
    } else {
        "none".to_string()
    };

    let latency_mode = match state.latency_mode {
        LatencyMode::LowLatency => "low-latency",
        LatencyMode::Standard => "standard",
    }
    .to_string();

    let kids = if !state.kids.is_empty() {
        state.kids.clone()
    } else {
        state.clearkey_keys.keys().cloned().collect()
    };

    Json(SessionInfoResponse {
        manifest_url: state.manifest_url.clone(),
        latency_mode,
        drm_scheme: state.drm_scheme.clone(),
        drm_system,
        kids,
        clear_keys: state.clearkey_keys.clone(),
    })
}

async fn handle_player_html(State(state): State<Arc<PlaybackServerState>>) -> Html<String> {
    Html(render_vanilla_player_html(&state.manifest_url))
}

async fn handle_fairplay_cert(
    State(state): State<Arc<PlaybackServerState>>,
) -> Result<Response, StatusCode> {
    let proxy = state.license_proxy.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    match proxy.handle_fairplay_certificate(None::<&str>).await {
        Ok(cert) => {
            let mut resp = (StatusCode::OK, cert).into_response();
            let h = resp.headers_mut();
            h.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-x509-ca-cert"),
            );
            h.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=86400"),
            );
            Ok(resp)
        }
        Err(e) => {
            tracing::error!("Failed to fetch FairPlay certificate via proxy: {e}");
            Err(StatusCode::BAD_GATEWAY)
        }
    }
}

fn license_response_to_axum(res: drmpack::license::LicenseResponse) -> Response {
    let mut resp = (StatusCode::OK, res.data).into_response();
    let h = resp.headers_mut();
    for (k, v) in &res.headers {
        if k != header::CONTENT_LENGTH && k != header::TRANSFER_ENCODING {
            h.insert(k.clone(), v.clone());
        }
    }
    if let Some(ct) = res.content_type {
        if let Ok(hv) = HeaderValue::from_str(&ct) {
            h.insert(header::CONTENT_TYPE, hv);
        }
    }
    resp
}

fn proxy_error_to_response(
    name: &str,
    err: drmpack::error::DrmpackError,
) -> Result<Response, StatusCode> {
    match err {
        drmpack::error::DrmpackError::LicenseProxy {
            status,
            message,
            diagnostic,
        } => {
            eprintln!("{name} proxy error: HTTP {status} - {message}");
            let code = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let mut resp = (code, message).into_response();
            if let Some(diag) = diagnostic {
                if let Ok(hv) = HeaderValue::from_str(&diag) {
                    resp.headers_mut().insert(
                        axum::http::header::HeaderName::from_static("x-axdrm-errormessage"),
                        hv,
                    );
                }
            }
            Ok(resp)
        }
        e => {
            eprintln!("{name} license proxy error: {e}");
            Err(StatusCode::BAD_GATEWAY)
        }
    }
}

async fn handle_widevine_license(
    State(state): State<Arc<PlaybackServerState>>,
    body: bytes::Bytes,
) -> Result<Response, StatusCode> {
    let proxy = state.license_proxy.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    let token = state.auth_token.as_deref().unwrap_or_default();
    match proxy.handle_widevine_license(&body, token).await {
        Ok(res) => Ok(license_response_to_axum(res)),
        Err(e) => proxy_error_to_response("Widevine", e),
    }
}

async fn handle_fairplay_license(
    State(state): State<Arc<PlaybackServerState>>,
    body: bytes::Bytes,
) -> Result<Response, StatusCode> {
    let proxy = state.license_proxy.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    let token = state.auth_token.as_deref().unwrap_or_default();
    match proxy.handle_fairplay_license(&body, token).await {
        Ok(res) => Ok(license_response_to_axum(res)),
        Err(e) => proxy_error_to_response("FairPlay", e),
    }
}

async fn handle_playready_license(
    State(state): State<Arc<PlaybackServerState>>,
    body: bytes::Bytes,
) -> Result<Response, StatusCode> {
    let proxy = state.license_proxy.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    let token = state.auth_token.as_deref().unwrap_or_default();
    match proxy.handle_playready_license(&body, token).await {
        Ok(res) => Ok(license_response_to_axum(res)),
        Err(e) => proxy_error_to_response("PlayReady", e),
    }
}

#[derive(serde::Deserialize)]
struct ClearKeyRequest {
    #[serde(default)]
    kids: Vec<String>,
}

#[derive(serde::Serialize)]
struct ClearKeyResponse {
    keys: Vec<ClearKeyItem>,
    #[serde(rename = "type")]
    msg_type: String,
}

#[derive(serde::Serialize)]
struct ClearKeyItem {
    kty: String,
    k: String,
    kid: String,
}

fn hex_to_bytes(s: &str) -> Option<Vec<u8>> {
    let s = s.replace('-', "");
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn handle_clearkey_license(
    State(state): State<Arc<PlaybackServerState>>,
    Json(payload): Json<ClearKeyRequest>,
) -> Result<Json<ClearKeyResponse>, StatusCode> {
    let mut keys = Vec::new();
    for kid_b64 in payload.kids {
        let kid_bytes = BASE64_URL_SAFE_NO_PAD
            .decode(&kid_b64)
            .or_else(|_| BASE64_STANDARD.decode(&kid_b64))
            .unwrap_or_default();
        let kid_hex = bytes_to_hex(&kid_bytes);

        let key_hex = state.clearkey_keys.get(&kid_hex).or_else(|| {
            state
                .clearkey_keys
                .get(&kid_b64.replace('-', "").to_lowercase())
        });

        if let Some(k_hex) = key_hex {
            if let Some(key_bytes) = hex_to_bytes(k_hex) {
                let k_b64 = BASE64_URL_SAFE_NO_PAD.encode(&key_bytes);
                keys.push(ClearKeyItem {
                    kty: "oct".to_string(),
                    k: k_b64,
                    kid: kid_b64,
                });
            }
        }
    }

    Ok(Json(ClearKeyResponse {
        keys,
        msg_type: "temporary".to_string(),
    }))
}

async fn serve_file(
    State(state): State<Arc<PlaybackServerState>>,
    AxumPath(raw_path): AxumPath<String>,
    req: Request,
) -> Response {
    let method = req.method().clone();
    let decoded = decode_percent(&raw_path);
    let rel_path = decoded.trim_start_matches('/');

    if rel_path.contains("..") || rel_path.contains('\\') {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let file_path = state.cdn_dir.join(rel_path);
    let mut data_opt = tokio::fs::read(&file_path)
        .await
        .ok()
        .filter(|d| !d.is_empty());

    // Live edge grace window for media segments (.m4s):
    if data_opt.is_none() && rel_path.ends_with(".m4s") {
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if let Ok(data) = tokio::fs::read(&file_path).await {
                if !data.is_empty() {
                    data_opt = Some(data);
                    break;
                }
            }
        }
    }

    let Some(data) = data_opt else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mime = resolve_mime_type(rel_path);
    let total_len = data.len();

    // Check Range header
    if method == Method::GET {
        if let Some(range_header) = req
            .headers()
            .get(header::RANGE)
            .and_then(|v| v.to_str().ok())
        {
            if let Some((start, end)) = parse_range_header_str(range_header, total_len) {
                let slice = data[start..=end].to_vec();
                let mut resp = (StatusCode::PARTIAL_CONTENT, slice).into_response();
                let h = resp.headers_mut();
                h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
                h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
                let range_val = format!("bytes {start}-{end}/{total_len}");
                if let Ok(hv) = HeaderValue::from_str(&range_val) {
                    h.insert(header::CONTENT_RANGE, hv);
                }
                h.insert(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("no-cache, no-store, must-revalidate"),
                );
                return resp;
            }
        }
    }

    let body = if method == Method::HEAD {
        bytes::Bytes::new()
    } else {
        bytes::Bytes::from(data)
    };

    let mut resp = (StatusCode::OK, body).into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(total_len));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    );
    resp
}

pub fn render_vanilla_player_html(manifest_path: &str) -> String {
    include_str!("player.html").replace("{manifest_path}", manifest_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_mime_type() {
        assert_eq!(resolve_mime_type("live.mpd"), "application/dash+xml");
        assert_eq!(
            resolve_mime_type("live.m3u8"),
            "application/vnd.apple.mpegurl"
        );
        assert_eq!(resolve_mime_type("segment_1.m4s"), "video/mp4");
        assert_eq!(resolve_mime_type("init.mp4"), "video/mp4");
        assert_eq!(
            resolve_mime_type("fairplay.cer"),
            "application/x-x509-ca-cert"
        );
        assert_eq!(
            resolve_mime_type("fairplay.der"),
            "application/x-x509-ca-cert"
        );
        assert_eq!(resolve_mime_type("subtitles.vtt"), "text/vtt");
        assert_eq!(resolve_mime_type("player.html"), "text/html; charset=utf-8");
        assert_eq!(resolve_mime_type("binary.dat"), "application/octet-stream");
    }

    #[test]
    fn test_decode_percent() {
        assert_eq!(decode_percent("test%20file.mpd"), "test file.mpd");
        assert_eq!(decode_percent("normal_path.m4s"), "normal_path.m4s");
        assert_eq!(decode_percent("%2Fpath%2Fto"), "/path/to");
        assert_eq!(decode_percent("invalid%2"), "invalid%2");
    }

    #[test]
    fn test_parse_range_header_str() {
        assert_eq!(parse_range_header_str("bytes=0-499", 1000), Some((0, 499)));
        assert_eq!(parse_range_header_str("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(parse_range_header_str("bytes=2000-", 1000), None);
        assert_eq!(parse_range_header_str("invalid", 1000), None);
    }

    #[test]
    fn test_render_vanilla_player_html_no_drm_info_fetch() {
        let html = render_vanilla_player_html("http://127.0.0.1:8080/live.mpd");
        assert!(html.contains("http://127.0.0.1:8080/live.mpd"));
        assert!(html.contains("shaka-player"));
        // Confirm drm_info.json is completely absent
        assert!(!html.contains("drm_info.json"));
        // Confirm uses api/session-info and proxy license endpoints
        assert!(html.contains("/api/session-info"));
        assert!(html.contains("/license/widevine"));
        assert!(html.contains("/license/fairplay"));
        assert!(html.contains("/fairplay.cer"));
    }

    #[test]
    fn test_render_vanilla_player_html_supports_latency_modes() {
        let html = render_vanilla_player_html("http://127.0.0.1:8080/live.mpd");
        assert!(html.contains("lowLatencyMode: isLowLatency"));
        assert!(html.contains("setLatencyMode"));
        assert!(html.contains("val-latency"));
        assert!(html.contains("mode-std"));
        assert!(html.contains("mode-ll"));
    }

    #[test]
    fn test_license_response_to_axum() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::HeaderName::from_static("x-custom"),
            HeaderValue::from_static("test-val"),
        );
        let res = drmpack::license::LicenseResponse {
            data: bytes::Bytes::from_static(b"license-payload"),
            content_type: Some("application/octet-stream".to_string()),
            headers,
        };
        let resp = license_response_to_axum(res);
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/octet-stream"
        );
        assert_eq!(resp.headers().get("x-custom").unwrap(), "test-val");
    }

    #[test]
    fn test_proxy_error_to_response() {
        let err = drmpack::error::DrmpackError::LicenseProxy {
            status: reqwest::StatusCode::FORBIDDEN,
            message: "token expired".to_string(),
            diagnostic: Some("Custom AxDRM error".to_string()),
        };
        let resp = proxy_error_to_response("Widevine", err).unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            resp.headers().get("x-axdrm-errormessage").unwrap(),
            "Custom AxDRM error"
        );

        let other_err = drmpack::error::DrmpackError::InvalidConfig("bad config".to_string());
        let res2 = proxy_error_to_response("FairPlay", other_err);
        assert_eq!(res2.unwrap_err(), StatusCode::BAD_GATEWAY);
    }
}
