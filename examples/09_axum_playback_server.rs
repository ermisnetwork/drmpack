//! # Example 09: Standalone Axum Playback Server
//!
//! A standalone HTTP server that serves pre-packaged DRM content from disk
//! with full Axinom DRM license proxying, simple auth, and a Shaka Player web UI.
//!
//! ## Usage
//! ```bash
//! # First, generate encrypted content with Example 08:
//! cargo run --example 08_in_memory_live_stream -- --dual --axinom --duration 30
//!
//! # Then start this playback server:
//! cargo run --example 09_axum_playback_server -- --port 8080
//!
//! # Open http://localhost:8080 in your browser
//! ```

mod common;

use common::playback_server::PlaybackServer;
use drmpack::axinom::{generate_axinom_jwt, AxinomKeyConfig};
use drmpack::license::{AxinomLicenseConfig, LicenseProxy};
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

// ── CLI ──────────────────────────────────────────────────────────────

fn parse_arg(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}

// ── App State ────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    com_key_id: String,
    com_key: String,
    kids: Vec<String>, // KIDs extracted from MPD on disk
    stream_dir: PathBuf,
    port: u16,
    // ponytail: hardcoded demo token instead of a real session store
    demo_token: String,
}

// ── KID extraction from MPD ──────────────────────────────────────────

/// Parse default_KID values from MPD XML on disk.
/// ponytail: regex over XML parse — good enough for extracting UUIDs from ContentProtection.
fn extract_kids_from_dir(dir: &std::path::Path) -> Vec<String> {
    let mut kids = Vec::new();
    // Try cenc/ and cbcs/ subdirs, then root
    let search_dirs: Vec<PathBuf> = ["cenc", "cbcs", ""]
        .iter()
        .map(|sub| dir.join(sub))
        .filter(|p| p.is_dir())
        .collect();

    for search_dir in search_dirs {
        if let Ok(entries) = std::fs::read_dir(&search_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("mpd") {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        // Extract default_KID="uuid" from ContentProtection elements
                        for segment in content.split("default_KID=\"") {
                            if let Some(end) = segment.find('"') {
                                let kid = &segment[..end];
                                // Validate UUID-like format
                                if kid.len() >= 32 && !kids.contains(&kid.to_string()) {
                                    kids.push(kid.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    kids
}

// ── Detect dual mode ─────────────────────────────────────────────────

fn is_dual_stream(dir: &std::path::Path) -> bool {
    dir.join("cenc").is_dir() && dir.join("cbcs").is_dir()
}

// ── Auth endpoints ───────────────────────────────────────────────────

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
    // ponytail: no real auth, just return the hardcoded token
    Json(LoginResponse {
        token: state.demo_token.clone(),
        username: req.username,
    })
}

// ── Playback Info endpoint ───────────────────────────────────────────

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
    manifest_url: String,
    drm_token: String,
    license_servers: LicenseServers,
    certificate_url: String,
    kids: Vec<String>,
    scheme: String,
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
    // Check bearer token
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

    // Build manifest URL based on scheme and dual mode
    let manifest_url = if dual {
        match scheme.as_str() {
            "cbcs" => "/cbcs/live.m3u8".to_string(),
            _ => "/cenc/live.mpd".to_string(),
        }
    } else {
        "/live.mpd".to_string()
    };

    // Generate Axinom JWT for the requested KIDs
    let key_configs: Vec<AxinomKeyConfig> = state
        .kids
        .iter()
        .map(|kid| AxinomKeyConfig::new(kid.clone()))
        .collect();

    let drm_token =
        generate_axinom_jwt(&state.com_key_id, &state.com_key, &key_configs).map_err(|e| {
            eprintln!("Failed to generate Axinom JWT: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let origin = format!("http://127.0.0.1:{}", state.port);
    Ok(Json(PlaybackInfoResponse {
        manifest_url,
        drm_token,
        license_servers: LicenseServers {
            widevine: format!("{origin}/license/widevine"),
            fairplay: format!("{origin}/license/fairplay"),
            playready: format!("{origin}/license/playready"),
        },
        certificate_url: format!("{origin}/fairplay.cer"),
        kids: state.kids.clone(),
        scheme,
    }))
}

// ── Player HTML with login flow ──────────────────────────────────────

fn render_player_html(port: u16) -> String {
    let origin = format!("http://127.0.0.1:{port}");
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>drmpack - Playback Server</title>
  <script src="https://ajax.googleapis.com/ajax/libs/shaka-player/4.12.5/shaka-player.compiled.js"></script>
  <style>
    * {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
      background: #111827; color: #f3f4f6;
      display: flex; flex-direction: column; align-items: center;
      padding: 24px 16px;
    }}
    .container {{ max-width: 860px; width: 100%; }}
    h1 {{ font-size: 22px; margin-bottom: 12px; }}
    .card {{
      background: #1f2937; border-radius: 8px; padding: 20px;
      margin-bottom: 16px;
    }}
    .status-bar {{
      padding: 8px 12px; background: #1f2937; border-radius: 6px;
      font-size: 13px; color: #9ca3af; margin-bottom: 12px;
      display: flex; justify-content: space-between; align-items: center;
    }}
    .status-bar.error {{ background: #7f1d1d; color: #fca5a5; }}
    .status-bar.ok {{ color: #10b981; }}
    .video-wrapper {{
      position: relative; background: #000; border-radius: 8px;
      overflow: hidden; aspect-ratio: 16 / 9;
      box-shadow: 0 4px 6px -1px rgba(0,0,0,0.5);
    }}
    video {{ width: 100%; height: 100%; display: block; }}
    .btn {{
      background: #3b82f6; border: none; color: #fff;
      padding: 8px 20px; border-radius: 6px; cursor: pointer;
      font-size: 14px; font-weight: 600;
    }}
    .btn:hover {{ background: #2563eb; }}
    .btn-sm {{
      background: #374151; padding: 4px 12px; font-size: 12px;
      border-radius: 4px;
    }}
    .btn-sm:hover {{ background: #4b5563; }}
    .btn-sm.active {{ background: #3b82f6; }}
    .controls {{
      margin-top: 12px; display: flex; gap: 8px;
      align-items: center; font-size: 13px; flex-wrap: wrap;
    }}
    .info-table {{
      margin-top: 16px; width: 100%; border-collapse: collapse; font-size: 13px;
    }}
    .info-table td {{
      padding: 6px 12px; border-bottom: 1px solid #374151;
    }}
    .info-table td:first-child {{
      color: #9ca3af; width: 140px; font-weight: 500;
    }}
    input {{
      background: #374151; border: 1px solid #4b5563; color: #f3f4f6;
      padding: 8px 12px; border-radius: 6px; font-size: 14px; width: 100%;
    }}
    label {{ font-size: 13px; color: #9ca3af; margin-bottom: 4px; display: block; }}
    .form-group {{ margin-bottom: 12px; }}
    .hidden {{ display: none !important; }}
  </style>
</head>
<body>
  <div class="container">
    <h1>🔐 drmpack Playback Server</h1>

    <!-- Login Section -->
    <div id="login-section" class="card">
      <h2 style="font-size:16px; margin-bottom:12px;">Login</h2>
      <div class="form-group">
        <label>Username</label>
        <input id="username" type="text" value="demo" />
      </div>
      <div class="form-group">
        <label>Password</label>
        <input id="password" type="password" value="demo" />
      </div>
      <button class="btn" onclick="doLogin()">Login</button>
      <div id="login-error" style="color:#fca5a5; font-size:13px; margin-top:8px;"></div>
    </div>

    <!-- Player Section (hidden until login) -->
    <div id="player-section" class="hidden">
      <div id="status" class="status-bar">Initializing...</div>
      <div class="video-wrapper">
        <video id="video" controls autoplay playsinline muted></video>
      </div>
      <div class="controls">
        <span style="color:#9ca3af; font-weight:600;">Scheme:</span>
        <button class="btn btn-sm" id="btn-cenc" onclick="switchScheme('cenc')">CENC (DASH)</button>
        <button class="btn btn-sm" id="btn-cbcs" onclick="switchScheme('cbcs')">CBCS (HLS)</button>
        <span style="color:#4b5563;">|</span>
        <button class="btn btn-sm" onclick="window.location.reload()">Reload</button>
      </div>
      <table class="info-table">
        <tr><td>Manifest URL</td><td id="val-manifest">-</td></tr>
        <tr><td>DRM Scheme</td><td id="val-scheme">-</td></tr>
        <tr><td>DRM System</td><td id="val-drm">-</td></tr>
        <tr><td>Active KIDs</td><td id="val-kids">-</td></tr>
        <tr><td>License Server</td><td id="val-license">-</td></tr>
      </table>
    </div>
  </div>

  <script>
    const ORIGIN = '{origin}';
    const isSafari = /^((?!chrome|android).)*safari/i.test(navigator.userAgent);
    let authToken = null;
    let currentPlayer = null;

    async function doLogin() {{
      const username = document.getElementById('username').value;
      const password = document.getElementById('password').value;
      try {{
        const res = await fetch('/api/login', {{
          method: 'POST',
          headers: {{ 'Content-Type': 'application/json' }},
          body: JSON.stringify({{ username, _password: password }})
        }});
        if (!res.ok) throw new Error('Login failed');
        const data = await res.json();
        authToken = data.token;
        document.getElementById('login-section').classList.add('hidden');
        document.getElementById('player-section').classList.remove('hidden');
        // Auto-detect: Safari→cbcs, others→cenc
        const defaultScheme = isSafari ? 'cbcs' : 'cenc';
        await startPlayback(defaultScheme);
      }} catch (e) {{
        document.getElementById('login-error').textContent = 'Login failed: ' + e.message;
      }}
    }}

    async function switchScheme(scheme) {{
      if (currentPlayer) {{
        try {{ await currentPlayer.destroy(); }} catch(_) {{}}
        currentPlayer = null;
      }}
      await startPlayback(scheme);
    }}

    async function startPlayback(scheme) {{
      const statusEl = document.getElementById('status');
      statusEl.textContent = 'Fetching playback info...';
      statusEl.className = 'status-bar';

      // Highlight active scheme button
      document.getElementById('btn-cenc').classList.toggle('active', scheme === 'cenc');
      document.getElementById('btn-cbcs').classList.toggle('active', scheme === 'cbcs');

      try {{
        const infoRes = await fetch('/api/playback-info?scheme=' + scheme, {{
          headers: {{ 'Authorization': 'Bearer ' + authToken }}
        }});
        if (!infoRes.ok) throw new Error('Failed to get playback info (HTTP ' + infoRes.status + ')');
        const info = await infoRes.json();

        document.getElementById('val-manifest').textContent = info.manifest_url;
        document.getElementById('val-scheme').textContent = info.scheme.toUpperCase();
        document.getElementById('val-kids').textContent = info.kids.join(', ') || '-';

        // Wait for manifest to be available
        const manifestUrl = window.location.origin + info.manifest_url;
        let ready = false;
        statusEl.textContent = 'Waiting for live stream...';
        for (let i = 0; i < 30; i++) {{
          try {{
            const r = await fetch(manifestUrl, {{ method: 'HEAD' }});
            if (r.ok) {{ ready = true; break; }}
          }} catch(_) {{}}
          await new Promise(r => setTimeout(r, 1000));
        }}
        if (!ready) {{
          statusEl.textContent = 'Stream not available. Start example 08 first.';
          statusEl.className = 'status-bar error';
          return;
        }}

        // Init Shaka Player
        shaka.polyfill.installAll();
        if (!shaka.Player.isBrowserSupported()) {{
          statusEl.textContent = 'Browser not supported!';
          statusEl.className = 'status-bar error';
          return;
        }}

        const video = document.getElementById('video');
        const player = new shaka.Player();
        await player.attach(video);
        currentPlayer = player;

        // DRM Configuration
        const wvUrl = info.license_servers.widevine;
        const fpUrl = info.license_servers.fairplay;
        const fpCert = info.certificate_url;

        const drmSystem = isSafari ? 'FairPlay' : 'Widevine';
        document.getElementById('val-drm').textContent = 'Axinom ' + drmSystem;
        document.getElementById('val-license').textContent = isSafari ? fpUrl : wvUrl;

        let certBytes = null;
        if (isSafari || scheme === 'cbcs') {{
          try {{
            const certRes = await fetch(fpCert);
            if (certRes.ok) {{
              certBytes = new Uint8Array(await certRes.arrayBuffer());
              console.log('FairPlay cert loaded:', certBytes.length, 'bytes');
            }}
          }} catch(e) {{ console.warn('FairPlay cert fetch error:', e); }}
        }}

        const fpsAdvanced = {{ serverCertificateUri: fpCert }};
        if (certBytes) fpsAdvanced.serverCertificate = certBytes;

        const fpUtils = (shaka.drm && shaka.drm.FairPlay) || (shaka.util && shaka.util.FairPlayUtils);

        player.configure({{
          streaming: {{
            lowLatencyMode: false,
            preferNativeHls: false,
            useNativeHlsForFairPlay: false
          }},
          drm: {{
            servers: {{
              'com.widevine.alpha': wvUrl,
              'com.apple.fps': fpUrl,
              'com.apple.fps.1_0': fpUrl
            }},
            advanced: {{
              'com.apple.fps': fpsAdvanced,
              'com.apple.fps.1_0': fpsAdvanced
            }},
            initDataTransform: (initData, initDataType, drmInfo) => {{
              if (initDataType !== 'skd') return initData;
              const rawId = (fpUtils && fpUtils.defaultGetContentId)
                ? fpUtils.defaultGetContentId(initData)
                : shaka.util.StringUtils.fromBytesAutoDetect(initData).replace(/^skd:\/\//, '');
              const contentId = String(rawId).split(':')[0].replace(/^skd:\/\//, '').trim();
              const cert = (drmInfo && drmInfo.serverCertificate) || certBytes;
              if (fpUtils && fpUtils.initDataTransform) return fpUtils.initDataTransform(initData, contentId, cert);
              return initData;
            }}
          }}
        }});

        // Set auth token on license requests
        player.getNetworkingEngine().registerRequestFilter((type, request) => {{
          if (type === shaka.net.NetworkingEngine.RequestType.LICENSE) {{
            request.headers['Content-Type'] = 'application/octet-stream';
          }}
        }});

        player.getNetworkingEngine().registerResponseFilter((type, response) => {{
          if (type === shaka.net.NetworkingEngine.RequestType.LICENSE) {{
            const ct = ((response.headers && (response.headers['content-type'] || response.headers['Content-Type'])) || '').toLowerCase();
            let isText = ct.includes('text') || ct.includes('json') || ct.includes('xml');
            if (!isText && response.data) {{
              const u8 = response.data instanceof Uint8Array ? response.data : new Uint8Array(response.data);
              if (u8.length > 0 && (u8[0] === 0x3c || u8[0] === 0x7b)) isText = true;
            }}
            if (isText && fpUtils && fpUtils.commonFairPlayResponse) {{
              fpUtils.commonFairPlayResponse(type, response);
            }}
          }}
        }});

        player.addEventListener('error', (event) => {{
          const d = event.detail;
          let msg = d && d.code ? 'Error ' + d.code : 'Playback Error';
          statusEl.textContent = msg;
          statusEl.className = 'status-bar error';
        }});

        video.addEventListener('playing', () => {{
          statusEl.textContent = '🔓 Decrypted & Streaming Live (' + info.scheme.toUpperCase() + ')';
          statusEl.className = 'status-bar ok';
        }});

        statusEl.textContent = 'Loading manifest...';
        await player.load(manifestUrl);
        statusEl.textContent = 'Live Manifest Loaded';
      }} catch (e) {{
        statusEl.textContent = 'Error: ' + e.message;
        statusEl.className = 'status-bar error';
      }}
    }}
  </script>
</body>
</html>"##
    )
}

async fn handle_player(State(state): State<Arc<AppState>>) -> Html<String> {
    Html(render_player_html(state.port))
}

// ── Main ─────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    let args: Vec<String> = env::args().collect();
    let port = parse_arg(&args, "--port")
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(8080);
    let stream_dir = PathBuf::from(
        parse_arg(&args, "--stream-dir").unwrap_or_else(|| "scratch/example08_live".to_string()),
    );

    println!("====================================================");
    println!("drmpack Playback Server (Example 09)");
    println!("====================================================");

    // Validate stream directory
    if !stream_dir.is_dir() {
        eprintln!(
            "ERROR: Stream directory not found: {}",
            stream_dir.display()
        );
        eprintln!("Run example 08 first to generate content:");
        eprintln!(
            "  cargo run --example 08_in_memory_live_stream -- --dual --axinom --duration 30"
        );
        std::process::exit(1);
    }

    // Load Axinom credentials
    let com_key_id = env::var("AXINOM_COMMUNICATION_KEY_ID").map_err(|_| {
        eprintln!("FATAL: Missing AXINOM_COMMUNICATION_KEY_ID in .env");
        "Missing AXINOM_COMMUNICATION_KEY_ID"
    })?;
    let com_key = env::var("AXINOM_COMMUNICATION_KEY").map_err(|_| {
        eprintln!("FATAL: Missing AXINOM_COMMUNICATION_KEY in .env");
        "Missing AXINOM_COMMUNICATION_KEY"
    })?;

    // Extract KIDs from MPD files on disk
    let kids = extract_kids_from_dir(&stream_dir);
    if kids.is_empty() {
        eprintln!("WARNING: No KIDs found in MPD files. JWT token will have no keys.");
    } else {
        println!("Detected KIDs from content:");
        for kid in &kids {
            println!("  - {kid}");
        }
    }

    let dual = is_dual_stream(&stream_dir);
    let scheme_desc = if dual { "Dual (CENC + CBCS)" } else { "Single" };
    println!("Stream Directory: {}", stream_dir.display());
    println!("Stream Mode:     {scheme_desc}");
    println!("Server Port:     {port}");

    // ponytail: hardcoded demo token, no JWT session management needed
    let demo_token = "drmpack-demo-token-2024".to_string();

    let app_state = Arc::new(AppState {
        com_key_id: com_key_id.clone(),
        com_key: com_key.clone(),
        kids: kids.clone(),
        stream_dir: stream_dir.clone(),
        port,
        demo_token,
    });

    // Generate the Axinom JWT for the PlaybackServer's license proxy
    let key_configs: Vec<AxinomKeyConfig> = kids
        .iter()
        .map(|kid| AxinomKeyConfig::new(kid.clone()))
        .collect();
    let auth_token = generate_axinom_jwt(&com_key_id, &com_key, &key_configs)?;

    // Set up license proxy
    let license_config = AxinomLicenseConfig::from_env().unwrap_or_default();
    let license_proxy = LicenseProxy::new(license_config);

    // Preload FairPlay cert
    if dual || env::var("AXINOM_FAIRPLAY_CERT_PATH").is_ok() {
        if let Ok(path) = env::var("AXINOM_FAIRPLAY_CERT_PATH") {
            println!("Loading FairPlay cert from: {path}");
            let cert = tokio::fs::read(&path).await?;
            license_proxy
                .set_fairplay_certificate(bytes::Bytes::from(cert))
                .await;
        } else {
            println!("Preloading FairPlay cert from Axinom...");
            match license_proxy.preload_fairplay_certificate().await {
                Ok(cert) => println!("FairPlay cert loaded: {} bytes", cert.len()),
                Err(e) => eprintln!("Warning: FairPlay cert preload failed: {e}"),
            }
        }
    }

    // Build the PlaybackServer router (handles file serving, license proxy, etc.)
    let manifest_url = if dual {
        format!("http://127.0.0.1:{port}/cenc/live.mpd")
    } else {
        format!("http://127.0.0.1:{port}/live.mpd")
    };

    let drm_scheme = if dual { "dual" } else { "widevine" };
    let kids_strings: Vec<String> = kids.clone();

    let base_router = PlaybackServer::new(stream_dir, manifest_url)
        .with_latency_mode(LatencyMode::Standard)
        .with_drm_scheme(drm_scheme)
        .with_kids(kids_strings)
        .with_license_proxy(license_proxy, auth_token)
        .into_router();

    // Our custom routes take priority; unmatched requests fall through to PlaybackServer
    let app = Router::new()
        .route("/", get(handle_player))
        .route("/api/login", post(handle_login))
        .route("/api/playback-info", get(handle_playback_info))
        .with_state(app_state)
        .fallback_service(base_router);

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    println!();
    println!("🌐 Server listening on http://127.0.0.1:{port}");
    println!("🎬 Player UI:           http://127.0.0.1:{port}/");
    if dual {
        println!("📺 CENC DASH Manifest:  http://127.0.0.1:{port}/cenc/live.mpd");
        println!("📺 CBCS HLS Manifest:   http://127.0.0.1:{port}/cbcs/live.m3u8");
    }
    println!();

    axum::serve(listener, app).await?;
    Ok(())
}
