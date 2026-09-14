//! # Example 11: HTTP Egress Playback Server
//!
//! A standalone HTTP server that serves content packaged via `EgressMode::HttpPush`
//! (Example 10) with full Axinom or ClearKey DRM license proxying and a Shaka Player web UI.
//!
//! ## Usage
//! ```bash
//! # 1. First, generate encrypted content using HTTP push:
//! cargo run --example 10_http_output_live_stream -- --dual --duration 30
//!
//! # 2. Start this playback server:
//! cargo run --example 11_http_output_playback_server -- --port 8080
//!
//! # 3. Open http://localhost:8080 in your browser
//! ```

mod common;

use common::playback_server::PlaybackServer;
use drmpack::axinom::{AxinomLicenseConfig, AxinomSigningConfig};
use drmpack::license::LicenseProxy;
use drmpack::session::DrmStreamMetadata;
use drmpack::types::LatencyMode;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::Html,
    routing::{get, post},
    Json, Router,
};

fn parse_arg(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}

#[derive(Clone)]
struct AppState {
    signing_config: Option<AxinomSigningConfig>,
    metadata: DrmStreamMetadata,
    stream_dir: PathBuf,
    port: u16,
    demo_token: String,
    is_clearkey: bool,
    static_key_hex: String,
}

fn is_dual_stream(dir: &std::path::Path) -> bool {
    dir.join("cenc").is_dir() && dir.join("cbcs").is_dir()
}

#[derive(serde::Deserialize)]
struct LoginRequest {
    #[serde(default = "default_username")]
    username: String,
    #[serde(default)]
    _password: String,
}

fn default_username() -> String {
    "demo".to_string()
}

#[derive(serde::Serialize)]
struct LoginResponse {
    token: String,
    username: String,
}

async fn handle_login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> Json<LoginResponse> {
    Json(LoginResponse {
        token: state.demo_token.clone(),
        username: req.username,
    })
}

#[derive(serde::Deserialize)]
struct PlaybackInfoQuery {
    #[serde(default = "default_scheme")]
    scheme: String,
}

fn default_scheme() -> String {
    "cenc".to_string()
}

#[derive(serde::Serialize)]
struct PlaybackInfoResponse {
    drm_token: String,
    license_servers: LicenseServers,
    certificate_url: String,
    kids: Vec<String>,
    scheme: String,
    is_dual: bool,
    is_clearkey: bool,
    clear_keys: std::collections::HashMap<String, String>,
}

#[derive(serde::Serialize)]
struct LicenseServers {
    widevine: String,
    fairplay: String,
    playready: String,
}

async fn handle_playback_info(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PlaybackInfoQuery>,
    req: axum::extract::Request,
) -> Result<Json<PlaybackInfoResponse>, StatusCode> {
    let auth = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !auth.ends_with(&state.demo_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let scheme = query.scheme.to_lowercase();
    let dual = is_dual_stream(&state.stream_dir);
    let origin = format!("http://127.0.0.1:{}", state.port);
    let kid_strings: Vec<String> = state.metadata.kid_strings();

    let drm_token = if let Some(ref signing_cfg) = state.signing_config {
        state
            .metadata
            .generate_axinom_jwt(signing_cfg)
            .unwrap_or_default()
    } else {
        "offline-demo-token".to_string()
    };

    let mut clear_keys = std::collections::HashMap::new();
    if state.is_clearkey {
        for kid in &kid_strings {
            let clean_kid = kid.replace('-', "").to_lowercase();
            clear_keys.insert(clean_kid, state.static_key_hex.clone());
        }
    }

    Ok(Json(PlaybackInfoResponse {
        drm_token,
        license_servers: LicenseServers {
            widevine: format!("{origin}/api/license/widevine"),
            fairplay: format!("{origin}/api/license/fairplay"),
            playready: format!("{origin}/api/license/playready"),
        },
        certificate_url: format!("{origin}/api/license/fairplay/cert"),
        kids: kid_strings,
        scheme,
        is_dual: dual,
        is_clearkey: state.is_clearkey,
        clear_keys,
    }))
}

async fn handle_player(State(state): State<Arc<AppState>>) -> Html<String> {
    let dual = is_dual_stream(&state.stream_dir);
    let default_manifest = if dual {
        format!("http://127.0.0.1:{}/cenc/live.mpd", state.port)
    } else {
        format!("http://127.0.0.1:{}/live.mpd", state.port)
    };

    let drm_badge = if state.is_clearkey {
        "<span style=\"color:#10b981;\">ClearKey (Offline / Static)</span>"
    } else {
        "<span style=\"color:#60a5fa;\">Axinom DRM (Widevine + FairPlay)</span>"
    };

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <title>drmpack HTTP Egress Playback</title>
  <script src="https://cdnjs.cloudflare.com/ajax/libs/shaka-player/4.7.11/shaka-player.compiled.js"></script>
  <style>
    body {{
      font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
      background: #111827; color: #f3f4f6; margin: 0; padding: 24px 16px;
      display: flex; flex-direction: column; align-items: center;
    }}
    .container {{ max-width: 860px; width: 100%; }}
    h1 {{ font-size: 22px; margin-bottom: 8px; }}
    .badge {{
      display: inline-block; padding: 4px 8px; border-radius: 4px;
      font-size: 12px; font-weight: 600; background: #374151; margin-bottom: 16px;
    }}
    .card {{
      background: #1f2937; border-radius: 8px; padding: 20px; margin-bottom: 16px;
    }}
    .status-bar {{
      padding: 8px 12px; background: #1f2937; border-radius: 6px;
      font-size: 13px; color: #9ca3af; margin-bottom: 12px;
      display: flex; justify-content: space-between; align-items: center;
    }}
    .status-bar.ok {{ color: #10b981; }}
    .video-wrapper {{
      position: relative; background: #000; border-radius: 8px;
      overflow: hidden; aspect-ratio: 16 / 9;
    }}
    video {{ width: 100%; height: 100%; display: block; }}
    .btn {{
      background: #3b82f6; border: none; color: #fff;
      padding: 8px 20px; border-radius: 6px; cursor: pointer;
      font-size: 14px; font-weight: 600;
    }}
    .btn:hover {{ background: #2563eb; }}
    .controls {{
      margin-top: 12px; display: flex; gap: 8px; align-items: center; font-size: 13px;
    }}
    .info-table {{ width: 100%; border-collapse: collapse; font-size: 13px; }}
    .info-table td {{ padding: 6px 12px; border-bottom: 1px solid #374151; }}
    .info-table td:first-child {{ color: #9ca3af; width: 160px; }}
    .log-box {{
      background: #000; border-radius: 6px; padding: 12px; font-family: monospace;
      font-size: 12px; max-height: 140px; overflow-y: auto; color: #10b981;
    }}
  </style>
</head>
<body>
  <div class="container">
    <h1>🚀 drmpack Zero-Disk HTTP Egress Playback</h1>
    <div class="badge">Egress: HttpPush | DRM: {drm_badge}</div>

    <div class="card">
      <div class="status-bar ok" id="status">Status: Ready to play</div>
      <div class="video-wrapper">
        <video id="video" controls autoplay muted></video>
      </div>
      <div class="controls">
        <button class="btn" id="play-btn">▶ Load & Play Stream</button>
        <span style="color:#9ca3af;" id="manifest-label">Manifest: {default_manifest}</span>
      </div>
    </div>

    <div class="card">
      <h3 style="font-size:15px; margin-top:0;">Stream Metadata (Generated via HTTP Loopback)</h3>
      <table class="info-table">
        <tr><td>Content ID</td><td>{}</td></tr>
        <tr><td>Stream Mode</td><td>{}</td></tr>
        <tr><td>Key IDs</td><td>{}</td></tr>
        <tr><td>Directory</td><td>{}</td></tr>
      </table>
    </div>

    <div class="card">
      <h3 style="font-size:15px; margin-top:0;">Shaka Player Event Log</h3>
      <div class="log-box" id="logs">Player initialized.\n</div>
    </div>
  </div>

  <script>
    const logBox = document.getElementById('logs');
    function log(msg) {{
      const d = new Date().toLocaleTimeString();
      logBox.innerText += `[${{d}}] ${{msg}}\n`;
      logBox.scrollTop = logBox.scrollHeight;
    }}

    async function init() {{
      shaka.polyfill.installAll();
      if (!shaka.Player.isBrowserSupported()) {{
        log('Error: Browser does not support Shaka Player');
        return;
      }}
      const video = document.getElementById('video');
      const player = new shaka.Player(video);
      window.player = player;

      player.addEventListener('error', (e) => {{
        log(`Player error: ${{e.detail.code}} - ${{e.detail.message}}`);
      }});

      document.getElementById('play-btn').onclick = async () => {{
        try {{
          log('Logging in to obtain playback token...');
          const loginRes = await fetch('/api/login', {{
            method: 'POST',
            headers: {{ 'Content-Type': 'application/json' }},
            body: JSON.stringify({{ username: 'demo' }})
          }}).then(r => r.json());

          log('Fetching playback session info...');
          const info = await fetch('/api/playback-info', {{
            headers: {{ 'Authorization': `Bearer ${{loginRes.token}}` }}
          }}).then(r => r.json());

          log(`Configuring DRM (${{info.is_clearkey ? 'ClearKey' : 'Axinom'}})...`);
          if (info.is_clearkey) {{
            player.configure({{ drm: {{ clearKeys: info.clear_keys }} }});
          }} else {{
            player.configure({{
              drm: {{
                servers: {{
                  'com.widevine.alpha': `${{info.license_servers.widevine}}?token=${{encodeURIComponent(info.drm_token)}}`
                }}
              }}
            }});
          }}

          log('Loading manifest: {default_manifest}');
          await player.load('{default_manifest}');
          log('Playback started successfully!');
          document.getElementById('status').innerText = 'Status: Streaming (EgressMode::HttpPush)';
        }} catch (err) {{
          log(`Playback failure: ${{err.message || err}}`);
        }}
      }};
    }}
    document.addEventListener('DOMContentLoaded', init);
  </script>
</body>
</html>"#,
        state.metadata.content_id,
        if dual { "Dual (CENC + CBCS)" } else { "Single" },
        state.metadata.kid_strings().join(", "),
        state.stream_dir.display(),
    );

    Html(html)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage: cargo run --example 11_http_output_playback_server [OPTIONS]");
        println!();
        println!("Options:");
        println!("  --port <PORT>        Playback server port (default: 8080)");
        println!(
            "  --stream-dir <PATH>  Stream dump directory (default: scratch/example10_http_output)"
        );
        println!("  -h, --help           Show this help message");
        return Ok(());
    }

    let port: u16 = parse_arg(&args, "--port")
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let stream_dir = parse_arg(&args, "--stream-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("scratch/example10_http_output"));

    println!("╔══════════════════════════════════════════════════════════════════╗");
    println!("║ drmpack Example 11: HTTP Egress Playback Server                  ║");
    println!("╚══════════════════════════════════════════════════════════════════╝");
    println!("Stream Directory: {}", stream_dir.display());
    println!("Server Port:      {port}");

    if !stream_dir.is_dir() {
        eprintln!(
            "\n❌ ERROR: Stream directory not found: {}",
            stream_dir.display()
        );
        eprintln!("Please run Example 10 first to package content via HTTP push:");
        eprintln!("  cargo run --example 10_http_output_live_stream -- --dual --duration 30");
        std::process::exit(1);
    }

    let meta_file = stream_dir.join("drm_metadata.json");
    if !meta_file.exists() {
        eprintln!(
            "\n❌ ERROR: Missing DRM metadata file at: {}",
            meta_file.display()
        );
        eprintln!("Please run Example 10 first:");
        eprintln!("  cargo run --example 10_http_output_live_stream -- --dual --duration 30");
        std::process::exit(1);
    }

    let meta_json = std::fs::read_to_string(&meta_file)?;
    let metadata = DrmStreamMetadata::from_json(&meta_json)?;
    let dual = is_dual_stream(&stream_dir);

    // Check if Axinom or ClearKey
    let signing_config = AxinomSigningConfig::from_env().ok();
    let is_clearkey = signing_config.is_none();
    let static_key_hex = "55555555555555555555555555555555".to_string();

    let app_state = Arc::new(AppState {
        signing_config: signing_config.clone(),
        metadata: metadata.clone(),
        stream_dir: stream_dir.clone(),
        port,
        demo_token: "drmpack-demo-token-http-push".to_string(),
        is_clearkey,
        static_key_hex: static_key_hex.clone(),
    });

    let manifest_url = if dual {
        format!("http://127.0.0.1:{port}/cenc/live.mpd")
    } else {
        format!("http://127.0.0.1:{port}/live.mpd")
    };

    let mut base_server = PlaybackServer::new(&stream_dir, manifest_url)
        .with_latency_mode(LatencyMode::Standard)
        .with_drm_scheme(if is_clearkey {
            "clearkey"
        } else if dual {
            "dual"
        } else {
            "widevine"
        })
        .with_kids(metadata.kid_strings());

    if is_clearkey {
        for kid in metadata.kid_strings() {
            base_server = base_server.with_clearkey(&kid, &static_key_hex);
        }
    } else if let Ok(license_cfg) = AxinomLicenseConfig::from_env() {
        let auth_token = metadata.generate_axinom_jwt(signing_config.as_ref().unwrap())?;
        let proxy = LicenseProxy::new(license_cfg);
        base_server = base_server.with_license_proxy(proxy, auth_token);
    }

    let base_router = base_server.into_router();

    let app = Router::new()
        .route("/", get(handle_player))
        .route("/api/login", post(handle_login))
        .route("/api/playback-info", get(handle_playback_info))
        .with_state(app_state)
        .fallback_service(base_router);

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    println!();
    println!("🌐 Playback Server listening on http://127.0.0.1:{port}");
    println!("🎬 Shaka Player Web UI:         http://127.0.0.1:{port}/");
    if dual {
        println!("📺 CENC DASH Manifest:          http://127.0.0.1:{port}/cenc/live.mpd");
        println!("📺 CBCS HLS Manifest:           http://127.0.0.1:{port}/cbcs/live.m3u8");
    } else {
        println!("📺 DASH Manifest:               http://127.0.0.1:{port}/live.mpd");
        println!("📺 HLS Manifest:                http://127.0.0.1:{port}/live.m3u8");
    }
    println!();

    axum::serve(listener, app).await?;
    Ok(())
}
