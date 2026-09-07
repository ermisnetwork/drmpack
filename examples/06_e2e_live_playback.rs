use base64::prelude::*;
use bytes::Bytes;
use drmpack::axinom::{AxinomConfig, AxinomProvider};
use drmpack::key::StaticKeySource;
use drmpack::license::DEFAULT_AXINOM_WIDEVINE_LICENSE_URL;
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::{LatencyMode, Rendition};
use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = dotenvy::dotenv();

    let args: Vec<String> = env::args().collect();
    let offline_flag = args.iter().any(|a| a == "--offline");
    let port = args
        .windows(2)
        .find(|w| w[0] == "--port")
        .and_then(|w| w[1].parse::<u16>().ok())
        .unwrap_or(8080);
    let max_duration = args
        .windows(2)
        .find(|w| w[0] == "--duration")
        .and_then(|w| w[1].parse::<u64>().ok())
        .map(Duration::from_secs);
    let max_chunks = args
        .windows(2)
        .find(|w| w[0] == "--max-chunks")
        .and_then(|w| w[1].parse::<u64>().ok());

    let test_mp4 = PathBuf::from("scratch/test.mp4");
    ensure_test_video(&test_mp4).await?;

    let output_dir = PathBuf::from("scratch/live_out");
    if output_dir.exists() {
        let _ = tokio::fs::remove_dir_all(&output_dir).await;
    }
    tokio::fs::create_dir_all(&output_dir).await?;

    let has_axinom_creds = env::var("AXINOM_TENANT_ID")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
        && env::var("AXINOM_MANAGEMENT_KEY")
            .or_else(|_| env::var("AXINOM_KEY_SERVICE_MANAGEMENT_KEY"))
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false);

    let use_axinom = has_axinom_creds && !offline_flag;

    let content_id = format!("e2e-live-{}", uuid::Uuid::new_v4());
    let session_config = PackagingSessionConfig::cenc(&content_id)
        .with_rendition(Rendition::video_hd())
        .with_rendition(Rendition::audio())
        .with_output_dir(&output_dir)
        .with_segment_duration(2.0)
        .with_latency_mode(LatencyMode::Standard)
        .preserve_output();

    let (mut session, is_axinom, active_kids, jwt_token, clear_kid_hex, clear_key_hex, license_url) =
        if use_axinom {
            println!("Axinom credentials detected in environment.");
            println!("Requesting live encryption keys from Axinom Key Service (SPEKE v2)...");
            let ax_config = AxinomConfig::from_env()?;
            let provider = AxinomProvider::new(ax_config);
            match PackagingSession::create(session_config.clone(), &provider).await {
                Ok(session) => {
                    let mut kids: Vec<String> = session
                        .key_set()
                        .all_keys()
                        .map(|k| k.kid.0.hyphenated().to_string())
                        .collect();
                    kids.sort();
                    kids.dedup();

                    let com_key_id = env::var("AXINOM_COMMUNICATION_KEY_ID")
                        .unwrap_or_else(|_| "00000000-0000-0000-0000-000000000000".to_string());
                    let com_key = env::var("AXINOM_COMMUNICATION_KEY")
                        .unwrap_or_else(|_| BASE64_STANDARD.encode([0u8; 32]));
                    let token = generate_axinom_jwt(&com_key_id, &com_key, &kids)?;
                    let wv_url = env::var("AXINOM_WIDEVINE_LICENSE_URL")
                        .unwrap_or_else(|_| DEFAULT_AXINOM_WIDEVINE_LICENSE_URL.to_string());

                    (
                        session,
                        true,
                        kids,
                        token,
                        String::new(),
                        String::new(),
                        wv_url,
                    )
                }
                Err(err) => {
                    eprintln!("Axinom key acquisition failed: {err}");
                    println!("Falling back to offline StaticKeySource (ClearKey)...");
                    let key_provider = StaticKeySource::shared_key([0x11; 16]);
                    let session = PackagingSession::create(session_config, &key_provider).await?;
                    let kid_str = "11111111-1111-1111-1111-111111111111".to_string();
                    (
                        session,
                        false,
                        vec![kid_str],
                        String::new(),
                        "11111111111111111111111111111111".to_string(),
                        "11111111111111111111111111111111".to_string(),
                        String::new(),
                    )
                }
            }
        } else {
            println!("Operating in offline mode with StaticKeySource (ClearKey)...");
            let key_provider = StaticKeySource::shared_key([0x11; 16]);
            let session = PackagingSession::create(session_config, &key_provider).await?;
            let kid_str = "11111111-1111-1111-1111-111111111111".to_string();
            (
                session,
                false,
                vec![kid_str],
                String::new(),
                "11111111111111111111111111111111".to_string(),
                "11111111111111111111111111111111".to_string(),
                String::new(),
            )
        };

    println!("PackagingSession initialized successfully.");
    println!("Active Key IDs:");
    for kid in &active_kids {
        println!("  - {kid}");
    }

    let manifest_url = format!("http://127.0.0.1:{port}/live.mpd");
    let player_html = render_player_html(
        is_axinom,
        &manifest_url,
        &license_url,
        &jwt_token,
        &clear_kid_hex,
        &clear_key_hex,
        &active_kids,
    );

    let player_file_path = output_dir.join("player.html");
    tokio::fs::write(&player_file_path, &player_html).await?;

    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    let server_handle = tokio::spawn(run_http_server(
        listener,
        output_dir.clone(),
        Arc::new(player_html),
        shutdown_rx,
    ));

    println!("Embedded HTTP Server listening on http://127.0.0.1:{port}");
    println!("Player Web UI: http://127.0.0.1:{port}/");
    println!("Live DASH Manifest: http://127.0.0.1:{port}/live.mpd");
    println!("Live HLS Manifest:  http://127.0.0.1:{port}/live.m3u8");

    let input_path_str = test_mp4.to_str().unwrap();
    let can_copy = check_stream_copy(input_path_str).await;

    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.arg("-re")
        .arg("-stream_loop")
        .arg("-1")
        .arg("-i")
        .arg(input_path_str);

    if can_copy {
        println!("Spawning FFmpeg pacer with stream copy (-c copy)...");
        cmd.arg("-c").arg("copy");
    } else {
        println!("Spawning FFmpeg pacer with live transcoding (-c:v libx264 -c:a aac)...");
        cmd.args([
            "-c:v",
            "libx264",
            "-g",
            "60",
            "-keyint_min",
            "60",
            "-sc_threshold",
            "0",
            "-profile:v",
            "baseline",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-ar",
            "48000",
        ]);
    }

    cmd.args([
        "-movflags",
        "empty_moov+default_base_moof+frag_keyframe",
        "-f",
        "mp4",
        "pipe:1",
    ]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());

    let mut ffmpeg_child = cmd.spawn()?;
    let mut stdout = ffmpeg_child
        .stdout
        .take()
        .ok_or("Failed to capture FFmpeg stdout pipe")?;

    println!("Piping FFmpeg live fMP4 stream into PackagingSession...");
    println!("Press Ctrl+C to terminate live packaging session.");

    let mut buf = vec![0u8; 65536];
    let mut chunk_count: u64 = 0;
    let mut stream_active = true;

    let duration_timer = max_duration.map(tokio::time::sleep);
    tokio::pin!(duration_timer);

    while stream_active {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("\nShutdown signal received (Ctrl+C). Terminating live stream...");
                stream_active = false;
            }
            _ = async {
                if let Some(ref mut timer) = duration_timer.as_mut().as_pin_mut() {
                    timer.await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                println!("Duration limit reached. Terminating live stream...");
                stream_active = false;
            }
            res = stdout.read(&mut buf) => {
                match res {
                    Ok(0) => {
                        println!("FFmpeg stdout reached EOF.");
                        stream_active = false;
                    }
                    Ok(n) => {
                        if let Err(err) = session.push(Bytes::copy_from_slice(&buf[..n])).await {
                            eprintln!("Error pushing media to GPAC: {err}");
                            stream_active = false;
                        }
                        chunk_count += 1;
                        if chunk_count.is_multiple_of(30) {
                            println!("Ingested {chunk_count} media chunks into live packager...");
                        }
                        if let Some(max) = max_chunks {
                            if chunk_count >= max {
                                println!("Max chunks reached ({chunk_count}). Terminating live stream...");
                                stream_active = false;
                            }
                        }
                        if !session.is_alive() {
                            eprintln!("GPAC session terminated unexpectedly!");
                            stream_active = false;
                        }
                    }
                    Err(err) => {
                        eprintln!("Error reading from FFmpeg stdout: {err}");
                        stream_active = false;
                    }
                }
            }
        }
    }

    println!("Stopping FFmpeg child process...");
    let _ = ffmpeg_child.kill().await;
    let _ = ffmpeg_child.wait().await;

    println!("Stopping embedded HTTP server...");
    let _ = shutdown_tx.send(());
    let _ = server_handle.await;

    println!("Closing GPAC packaging session...");
    session.close().await?;

    println!("Live packaging session closed cleanly.");
    println!(
        "Output manifests and segments preserved at: {}",
        output_dir.display()
    );

    Ok(())
}

fn render_player_html(
    is_axinom: bool,
    manifest_url: &str,
    license_url: &str,
    axinom_token: &str,
    clear_key_kid: &str,
    clear_key_key: &str,
    active_kids: &[String],
) -> String {
    let kids_json = serde_json::to_string(active_kids).unwrap_or_else(|_| "[]".to_string());
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>drmpack - Live DRM Playback Test</title>
  <script src="https://ajax.googleapis.com/ajax/libs/shaka-player/4.12.5/shaka-player.compiled.js"></script>
  <style>
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      background: #111827;
      color: #f3f4f6;
      display: flex;
      flex-direction: column;
      align-items: center;
      padding: 24px 16px;
      margin: 0;
    }}
    h1 {{
      font-size: 22px;
      margin-bottom: 8px;
    }}
    .container {{
      max-width: 860px;
      width: 100%;
    }}
    video {{
      width: 100%;
      height: 480px;
      background: #000;
      border-radius: 8px;
      box-shadow: 0 8px 24px rgba(0,0,0,0.6);
    }}
    .status-bar {{
      margin-top: 12px;
      padding: 10px 14px;
      border-radius: 6px;
      background: #1f2937;
      font-size: 14px;
      font-weight: 500;
      color: #10b981;
    }}
    .status-bar.error {{
      color: #ef4444;
      background: #2b1d1d;
    }}
    .info-card {{
      margin-top: 16px;
      background: #1f2937;
      border-radius: 8px;
      padding: 16px;
      font-size: 13px;
      font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
    }}
    .info-row {{
      margin-bottom: 8px;
      display: flex;
      word-break: break-all;
    }}
    .info-row:last-child {{
      margin-bottom: 0;
    }}
    .info-label {{
      color: #9ca3af;
      width: 140px;
      flex-shrink: 0;
      font-weight: 600;
    }}
    .info-value {{
      color: #e5e7eb;
    }}
  </style>
</head>
<body>
  <div class="container">
    <h1>drmpack Live Ingestion & Playback</h1>
    <video id="video" controls autoplay muted playsinline></video>
    <div id="status" class="status-bar">Initializing Shaka Player...</div>
    <div class="info-card">
      <div class="info-row"><span class="info-label">Stream URL:</span><span class="info-value" id="val-stream"></span></div>
      <div class="info-row"><span class="info-label">DRM Scheme:</span><span class="info-value" id="val-drm"></span></div>
      <div class="info-row"><span class="info-label">Active KIDs:</span><span class="info-value" id="val-kids"></span></div>
      <div class="info-row"><span class="info-label">License Service:</span><span class="info-value" id="val-license"></span></div>
    </div>
  </div>
  <script>
    const isAxinom = {is_axinom};
    const manifestUrl = '{manifest_url}';
    const licenseUrl = '{license_url}';
    const axinomToken = '{axinom_token}';
    const clearKeyKid = '{clear_key_kid}';
    const clearKeyKey = '{clear_key_key}';
    const activeKids = {kids_json};

    document.getElementById('val-stream').textContent = manifestUrl;
    document.getElementById('val-drm').textContent = isAxinom ? 'Axinom (Widevine L3 / SPEKE v2)' : 'Offline (ClearKey)';
    document.getElementById('val-kids').textContent = activeKids.join(', ');
    document.getElementById('val-license').textContent = isAxinom ? licenseUrl : 'ClearKey (In-Browser)';

    async function initPlayer() {{
      shaka.polyfill.installAll();
      if (!shaka.Player.isBrowserSupported()) {{
        const s = document.getElementById('status');
        s.textContent = 'Browser does not support EME / Shaka Player!';
        s.className = 'status-bar error';
        return;
      }}
      const video = document.getElementById('video');
      const player = new shaka.Player();
      await player.attach(video);

      player.addEventListener('error', (event) => {{
        const s = document.getElementById('status');
        const detail = event.detail;
        const msg = (detail && detail.code) ? ('Error ' + detail.code + ' (category ' + detail.category + ')') : 'Playback Error';
        s.textContent = 'Playback Error: ' + msg;
        s.className = 'status-bar error';
      }});

      if (isAxinom) {{
        player.configure({{
          drm: {{
            servers: {{
              'com.widevine.alpha': licenseUrl
            }},
            advanced: {{
              'com.widevine.alpha': {{
                videoRobustness: 'SW_SECURE_CRYPTO',
                audioRobustness: 'SW_SECURE_CRYPTO'
              }}
            }}
          }},
          manifest: {{
            defaultPresentationDelay: 4
          }},
          streaming: {{
            rebufferingGoal: 2,
            bufferingGoal: 4,
            retryParameters: {{
              maxAttempts: 10,
              baseDelay: 500,
              backoffFactor: 1.2
            }}
          }}
        }});
        player.getNetworkingEngine().registerRequestFilter((type, request) => {{
          if (type === shaka.net.NetworkingEngine.RequestType.LICENSE) {{
            request.headers['X-AxDRM-Message'] = axinomToken;
          }}
        }});
      }} else {{
        player.configure({{
          drm: {{
            clearKeys: {{
              [clearKeyKid]: clearKeyKey
            }}
          }},
          manifest: {{
            defaultPresentationDelay: 4
          }},
          streaming: {{
            rebufferingGoal: 2,
            bufferingGoal: 4,
            retryParameters: {{
              maxAttempts: 10,
              baseDelay: 500,
              backoffFactor: 1.2
            }}
          }}
        }});
      }}

      video.addEventListener('playing', () => {{
        const s = document.getElementById('status');
        s.textContent = isAxinom
          ? 'Decrypted & Streaming Live via Widevine L3'
          : 'Decrypted & Streaming Live via ClearKey';
        s.className = 'status-bar';
      }});

      try {{
        document.getElementById('status').textContent = 'Loading live DASH manifest...';
        await player.load(manifestUrl);
        document.getElementById('status').textContent = isAxinom
          ? 'Live DASH Manifest Loaded (Widevine L3)'
          : 'Live DASH Manifest Loaded (ClearKey)';
      }} catch (err) {{
        const s = document.getElementById('status');
        const msg = (err && err.code) ? ('Shaka Error ' + err.code + ' (category ' + err.category + ')') : (err.message || String(err));
        s.textContent = 'Failed to load live stream: ' + msg;
        s.className = 'status-bar error';
      }}
    }}
    document.addEventListener('DOMContentLoaded', initPlayer);
  </script>
</body>
</html>
"#
    )
}

fn resolve_mime_type(path: &str) -> &'static str {
    if path.ends_with(".mpd") {
        "application/dash+xml"
    } else if path.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else if path.ends_with(".m4s") || path.ends_with(".mp4") {
        "video/mp4"
    } else if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    }
}

fn generate_axinom_jwt(
    com_key_id: &str,
    com_key_b64: &str,
    kids: &[String],
) -> Result<String, Box<dyn Error>> {
    let key_bytes = BASE64_STANDARD.decode(com_key_b64.trim())?;
    let header_json = r#"{"alg":"HS256","typ":"JWT"}"#;
    let inline_entries: Vec<serde_json::Value> = kids
        .iter()
        .map(|kid| serde_json::json!({ "id": kid }))
        .collect();
    let payload = serde_json::json!({
        "version": 1,
        "com_key_id": com_key_id,
        "message": {
            "type": "entitlement_message",
            "version": 2,
            "content_keys_source": {
                "inline": inline_entries
            }
        }
    });
    let payload_json = serde_json::to_string(&payload)?;
    let h_b64 = BASE64_URL_SAFE_NO_PAD.encode(header_json.as_bytes());
    let p_b64 = BASE64_URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
    let signing_input = format!("{h_b64}.{p_b64}");

    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &key_bytes);
    let tag = ring::hmac::sign(&key, signing_input.as_bytes());
    let sig_b64 = BASE64_URL_SAFE_NO_PAD.encode(tag.as_ref());
    Ok(format!("{signing_input}.{sig_b64}"))
}

async fn check_stream_copy(path: &str) -> bool {
    let probe = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            path,
            "-t",
            "0.1",
            "-c",
            "copy",
            "-movflags",
            "empty_moov+default_base_moof+frag_keyframe",
            "-f",
            "mp4",
            "-y",
            "/dev/null",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    matches!(probe, Ok(status) if status.success())
}

async fn ensure_test_video(path: &Path) -> Result<(), Box<dyn Error>> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    println!("Creating test video: {}", path.display());
    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=10:size=1280x720:rate=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:duration=10:sample_rate=48000",
            "-c:v",
            "libx264",
            "-g",
            "60",
            "-keyint_min",
            "60",
            "-sc_threshold",
            "0",
            "-profile:v",
            "baseline",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-ar",
            "48000",
            "-f",
            "mp4",
            path.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;

    if !status.success() {
        return Err(format!("ffmpeg failed to create test video at {}", path.display()).into());
    }
    println!("Test video created successfully: {}", path.display());
    Ok(())
}

async fn handle_connection(mut stream: TcpStream, output_dir: PathBuf, player_html: Arc<String>) {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    loop {
        let read_result =
            tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk)).await;
        match read_result {
            Ok(Ok(n)) if n > 0 => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() >= 8192 {
                    break;
                }
            }
            _ => break,
        }
    }
    if buf.is_empty() {
        return;
    }
    let req_str = String::from_utf8_lossy(&buf);
    let first_line = match req_str.lines().next() {
        Some(line) => line,
        None => return,
    };
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }
    let method = parts[0];
    let full_path = parts[1];
    let path = full_path.split('?').next().unwrap_or("/");

    if method == "OPTIONS" {
        let response = "HTTP/1.1 204 No Content\r\n\
Access-Control-Allow-Origin: *\r\n\
Access-Control-Allow-Methods: *\r\n\
Access-Control-Allow-Headers: *\r\n\
Access-Control-Max-Age: 86400\r\n\
Content-Length: 0\r\n\
Connection: close\r\n\r\n";
        let _ = stream.write_all(response.as_bytes()).await;
        return;
    }

    if method != "GET" && method != "HEAD" {
        let response = "HTTP/1.1 405 Method Not Allowed\r\n\
Content-Length: 0\r\n\
Connection: close\r\n\r\n";
        let _ = stream.write_all(response.as_bytes()).await;
        return;
    }

    if path == "/" || path == "/player.html" {
        let html_bytes = player_html.as_bytes();
        let header = format!(
            "HTTP/1.1 200 OK\r\n\
Content-Type: text/html; charset=utf-8\r\n\
Content-Length: {}\r\n\
Access-Control-Allow-Origin: *\r\n\
Access-Control-Allow-Methods: *\r\n\
Access-Control-Allow-Headers: *\r\n\
Cache-Control: no-cache, no-store, must-revalidate\r\n\
Connection: close\r\n\r\n",
            html_bytes.len()
        );
        let _ = stream.write_all(header.as_bytes()).await;
        if method == "GET" {
            let _ = stream.write_all(html_bytes).await;
        }
        return;
    }

    if path == "/favicon.ico" {
        let response = "HTTP/1.1 204 No Content\r\n\
Access-Control-Allow-Origin: *\r\n\
Content-Length: 0\r\n\
Connection: close\r\n\r\n";
        let _ = stream.write_all(response.as_bytes()).await;
        return;
    }

    let rel_path = path.trim_start_matches('/');
    if rel_path.contains("..") || rel_path.contains('\\') {
        let response = "HTTP/1.1 400 Bad Request\r\n\
Content-Length: 0\r\n\
Connection: close\r\n\r\n";
        let _ = stream.write_all(response.as_bytes()).await;
        return;
    }

    let file_path = output_dir.join(rel_path);
    let mut data_opt = tokio::fs::read(&file_path)
        .await
        .ok()
        .filter(|d| !d.is_empty());
    if data_opt.is_none()
        && (rel_path.ends_with(".m4s") || rel_path.ends_with(".mpd") || rel_path.ends_with(".m3u8"))
    {
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if let Ok(data) = tokio::fs::read(&file_path).await {
                if !data.is_empty() {
                    data_opt = Some(data);
                    break;
                }
            }
        }
    }

    if let Some(data) = data_opt {
        let mime = resolve_mime_type(rel_path);
        let header = format!(
            "HTTP/1.1 200 OK\r\n\
Content-Type: {}\r\n\
Content-Length: {}\r\n\
Access-Control-Allow-Origin: *\r\n\
Access-Control-Allow-Methods: *\r\n\
Access-Control-Allow-Headers: *\r\n\
Access-Control-Expose-Headers: Content-Length, Content-Type, Date\r\n\
Cache-Control: no-cache, no-store, must-revalidate\r\n\
Connection: close\r\n\r\n",
            mime,
            data.len()
        );
        let _ = stream.write_all(header.as_bytes()).await;
        if method == "GET" {
            let _ = stream.write_all(&data).await;
        }
    } else {
        let response = "HTTP/1.1 404 Not Found\r\n\
Access-Control-Allow-Origin: *\r\n\
Content-Length: 9\r\n\
Connection: close\r\n\r\n\
Not Found";
        let _ = stream.write_all(response.as_bytes()).await;
    }
}

async fn run_http_server(
    listener: TcpListener,
    output_dir: PathBuf,
    player_html: Arc<String>,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                break;
            }
            res = listener.accept() => {
                match res {
                    Ok((stream, _)) => {
                        let dir = output_dir.clone();
                        let html = Arc::clone(&player_html);
                        tokio::spawn(async move {
                            handle_connection(stream, dir, html).await;
                        });
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_axinom_jwt() {
        let com_key_id = "00000000-0000-0000-0000-000000000000";
        let com_key_b64 = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        let kids = vec![
            "11111111-1111-1111-1111-111111111111".to_string(),
            "22222222-2222-2222-2222-222222222222".to_string(),
        ];
        let token =
            generate_axinom_jwt(com_key_id, com_key_b64, &kids).expect("jwt generation failed");
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);

        let payload_bytes = BASE64_URL_SAFE_NO_PAD
            .decode(parts[1])
            .expect("payload base64 decode failed");
        let payload: serde_json::Value =
            serde_json::from_slice(&payload_bytes).expect("payload json parse failed");
        assert_eq!(payload["version"], 1);
        assert_eq!(payload["com_key_id"], com_key_id);
        assert_eq!(payload["message"]["type"], "entitlement_message");
        assert_eq!(payload["message"]["version"], 2);
        let inline = payload["message"]["content_keys_source"]["inline"]
            .as_array()
            .expect("inline array expected");
        assert_eq!(inline.len(), 2);
        assert_eq!(inline[0]["id"], "11111111-1111-1111-1111-111111111111");
        assert_eq!(inline[1]["id"], "22222222-2222-2222-2222-222222222222");
    }

    #[test]
    fn test_mime_type_resolution() {
        assert_eq!(resolve_mime_type("live.mpd"), "application/dash+xml");
        assert_eq!(
            resolve_mime_type("live.m3u8"),
            "application/vnd.apple.mpegurl"
        );
        assert_eq!(
            resolve_mime_type("live_1.m3u8"),
            "application/vnd.apple.mpegurl"
        );
        assert_eq!(resolve_mime_type("live_1_1.m4s"), "video/mp4");
        assert_eq!(resolve_mime_type("live_1_init.mp4"), "video/mp4");
        assert_eq!(resolve_mime_type("player.html"), "text/html; charset=utf-8");
        assert_eq!(resolve_mime_type("other.bin"), "application/octet-stream");
    }

    #[test]
    fn test_player_html_rendering() {
        let kids = vec!["11111111-1111-1111-1111-111111111111".to_string()];
        let html = render_player_html(
            true,
            "http://127.0.0.1:8080/live.mpd",
            "https://license.example.com",
            "test-token",
            "11111111111111111111111111111111",
            "11111111111111111111111111111111",
            &kids,
        );
        assert!(html.contains("http://127.0.0.1:8080/live.mpd"));
        assert!(html.contains("https://license.example.com"));
        assert!(html.contains("test-token"));
        assert!(html.contains("Shaka Player"));
    }
}
