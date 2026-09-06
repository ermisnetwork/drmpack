# Milestone 0004: End-to-End Live Packaging, Ingestion & Web Browser Playback Guide

This guide establishes the definitive, step-by-step engineering architecture and runnable recipe for performing a complete End-to-End (E2E) live DRM packaging test using `drmpack`. It traces the complete lifecycle: taking an arbitrary test MP4 file in a scratch folder, simulating a continuous live stream ingest via FFmpeg, acquiring production DRM keys from Axinom Key Service (SPEKE v2 / CPIX 2.3), packaging into encrypted HLS/DASH with GPAC, signing an Axinom Entitlement JWT token, and verifying real-time playback in a web browser (Google Chrome Widevine CDM) using Shaka Player.

---

## 1. High-Level Architectural Topology

```
+----------------------------------------------------------------------------------------------------+
|                                    LIVE INGEST & PACKAGING PIPELINE                                |
+----------------------------------------------------------------------------------------------------+
                                                                                                      
 [ scratch/test.mp4 ]                                                                                 
         |                                                                                            
         v                                                                                            
 [ FFmpeg Process ]  -- (-re: real-time clock, -movflags empty_moov+default_base_moof+frag_keyframe) 
         |                                                                                            
         v (anonymous pipe: stdout -> stdin)                                                          
 [ drmpack Supervisor ] <======== (SPEKE v2 / CPIX 2.3) ========> [ Axinom Key Service ]             
         |                                                       (Key Acquisition: KID, Key, PSSH)   
         v (stdin pipe)                                                                               
 [ GPAC Subprocess ] -- (cecrypt:cfile=drm.xml -> dasher:profile=live:pssh=mv:cmaf=cmfc)             
         |                                                                                            
         v (atomic disk / ramdisk writes)                                                             
 [ Output Directory ] --------------------+                                                           
   - live.mpd   (DASH Manifest)           |                                                           
   - live.m3u8  (HLS Master Manifest)     |                                                           
   - live_1.m3u8 (HLS Variant Playlist)   |                                                           
   - live_1_init.mp4 (Init Segment)       |                                                           
   - live_1_*.m4s (CMAF Media Segments)   |                                                           
                                          |                                                           
+-----------------------------------------|----------------------------------------------------------+
|                                    HTTP DELIVERY & BROWSER PLAYBACK                                |
+-----------------------------------------|----------------------------------------------------------+
                                          |                                                           
                                          v                                                           
                                [ Local HTTP Server ] (CORS Enabled: Access-Control-Allow-Origin: *)  
                                (http://localhost:8080)                                               
                                          |                                                           
                    +---------------------+---------------------+                                     
                    | (GET live.mpd, *.m4s)                     | (Serves player.html)                
                    v                                           v                                     
      [ Google Chrome (Secure Context) ]           [ Axinom Entitlement Token ]                       
      [ Shaka Player / dash.js (EME API) ]         (Signed JWT: com_key_id + inline KID)             
                    |                                           |                                     
                    +---------------------+---------------------+                                     
                                          |                                                           
                                          | (POST Widevine Challenge + Header: X-AxDRM-Message)       
                                          v                                                           
                            [ Axinom License Service ]                                                
                            (https://<tenant>.drm-widevine-licensing.axprod.net/AcquireLicense)       
                                          |                                                           
                                          v (Widevine License Response)                               
                            [ Chrome Widevine CDM ]                                                   
                            (Hardware/Software Decryptor -> Audio/Video Screen)                       
```

---

## 2. Component Deep Dive & Primary Source Specifications

### 2.1 Live fMP4 Ingestion Requirements (GPAC `stdin` Demuxer)

A standard VOD MP4 file cannot be piped directly into a live packager:
1. **Box Structure Mismatch**: Standard MP4 files store media in a single continuous `mdat` box referenced by sample tables (`stbl`/`stts`/`stsc`/`stco`) inside a top-level `moov` box. A live packaging engine cannot read an entire file into memory to index sample tables.
2. **ISO-BMFF Fragmentation**: Live streaming requires ISO/IEC 14496-12 Movie Fragments. Media is partitioned into independent chunks: an **Initialization Segment** (`ftyp` + `moov` with `mvex` defaults) followed by continuous sequence of **Media Segments** (`moof` + `mdat`).
3. **Pacing**: Live streaming engines require media to arrive paced to real time. Dumping an entire MP4 file into GPAC's `stdin` in 50 milliseconds causes GPAC to encounter EOF, write `#EXT-X-ENDLIST`, and exit immediately before any client browser can connect to the live edge.

#### The Authoritative FFmpeg Live Emulation Command
To convert any standard MP4 (`scratch/test.mp4`) into a continuous live fMP4 stream, FFmpeg must be invoked with:

```bash
ffmpeg -re -stream_loop -1 -i scratch/test.mp4 \
  -c:v libx264 -g 60 -keyint_min 60 -sc_threshold 0 \
  -c:a aac -b:a 128k -ar 48000 \
  -movflags empty_moov+default_base_moof+frag_keyframe \
  -f mp4 pipe:1
```

*Key Parameter Rationale:*
- **`-re`**: Reads the input at its native frame rate. This meters the pipe to exact wall-clock speed (e.g. exactly 1 second of video pushed per second of real time).
- **`-stream_loop -1`**: Loops the input MP4 infinitely. A short 10-second test video will continuously loop as a perpetual live TV broadcast.
- **`-movflags empty_moov+default_base_moof+frag_keyframe`**:
  * `empty_moov`: Emits an initial `moov` box containing no sample records (`duration=0`), instructing demuxers that track defaults are in the `mvex` box.
  * `default_base_moof`: Sets the base data offset in `tfhd` to the start of the `moof` box, required for low-latency CMAF.
  * `frag_keyframe`: Forces an ISO-BMFF fragment boundary (`moof` + `mdat`) at every video keyframe (IDR).
- **`-g 60 -keyint_min 60 -sc_threshold 0`**: Enforces a strictly constant Group of Pictures (GOP) size (e.g. 60 frames @ 30 fps = exactly 2.000 seconds). This aligns with GPAC's `segdur=2.0`.
- **`-f mp4 pipe:1`**: Directs the formatted fMP4 stream to standard output (`stdout`).

*(Note: If `scratch/test.mp4` already has constant 2.0s GOPs and compatible H.264/AAC codecs, `-c copy` can replace the transcoding flags to eliminate CPU overhead).*

#### GPAC Latency Elimination Flags
As documented in Milestone 0002, GPAC's pipe demuxer will buffer up to 50 frames or 50 KB by default. `drmpack` automatically spawns GPAC with:
```
stdin:ext=mp4:alltk:mstore_samples=0:mstore_purge=0
```
This guarantees sub-second chunk flushing to Ramdisk during continuous live ingest.

---

### 2.2 Axinom DRM Key Service (SPEKE v2 / CPIX 2.3)

`drmpack` integrates with the Axinom Key Service using the industry-standard SPEKE v2 (Secure Packager and Encoder Key Exchange) protocol over CPIX 2.3 (Content Protection Information Exchange).

- **Authentication**: HTTP Basic Authentication formatted as:
  `Authorization: Basic base64(AXINOM_TENANT_ID + ":" + AXINOM_MANAGEMENT_KEY)`
- **Header**: `X-Speke-Version: 2.0`
- **CPIX Request Construction**: Generated via [`CpixRequestBuilder`](../../src/cpix/builder.rs). The request contains:
  * `<cpix:ContentKey kid="..." commonEncryptionScheme="cenc"/>`
  * `<cpix:DRMSystem kid="..." systemId="edef8ba9-79d6-4ace-a3c8-27dcd51d21ed"><cpix:PSSH/></cpix:DRMSystem>` (Widevine UUID)
  * `<cpix:DRMSystem kid="..." systemId="9a04f079-9840-4286-ab92-e65be0885f95"><cpix:PSSH/></cpix:DRMSystem>` (PlayReady UUID)
  * `<cpix:ContentKeyUsageRule kid="..." intendedTrackType="HD"><cpix:VideoFilter/></cpix:ContentKeyUsageRule>`
- **CPIX Response Handling**: Parsed via [`CpixResponseParser`](../../src/cpix/parser.rs). Axinom returns the raw 16-byte Content Key (`<pskc:PlainValue>`) and the binary `pssh` box payloads for each DRM system.
- **GPAC DRM XML**: Generated via [`GpacDrmXmlGenerator`](../../src/gpac/xml.rs), mapping Widevine PSSH boxes into `<DRMInfo type="pssh">` and keys into `<CrypTrack>` definitions for GPAC's `cecrypt` filter.

---

### 2.3 Axinom Entitlement Message & JWT Generation (`X-AxDRM-Message`)

When the web browser plays the encrypted stream, the Widevine CDM generates an Encrypted Media Extensions (EME) license challenge. Axinom License Service requires this challenge to be accompanied by an **Axinom Entitlement JWT** passed via the `X-AxDRM-Message` HTTP header.

#### JWT Token Anatomy
The token is a standard RFC 7519 JSON Web Token signed with HMAC-SHA256 (`HS256`):

1. **Header**:
   ```json
   {
     "alg": "HS256",
     "typ": "JWT"
   }
   ```
2. **Payload Claims**:
   ```json
   {
     "version": 1,
     "com_key_id": "<AXINOM_COMMUNICATION_KEY_ID>",
     "message": {
       "type": "entitlement_message",
       "version": 2,
       "content_keys_source": {
         "inline": [
           {
             "id": "<CONTENT_KEY_ID>"
           }
         ]
       }
     }
   }
   ```
   *Fields:*
   - `com_key_id`: The UUID of the Communication Key configured in Axinom DRM Portal.
   - `message.type`: Must be `"entitlement_message"`.
   - `content_keys_source.inline[].id`: The exact Key ID (KID) UUID acquired during packaging.
3. **Signature**:
   The token is signed with HMAC-SHA256 using the raw binary bytes of `AXINOM_COMMUNICATION_KEY` (decoded from Base64).

*Code Verification:* See [`tests/license_proxy_test.rs#L767-L782`](../../tests/license_proxy_test.rs#L767-L782) which validates this exact token against Axinom's live production servers.

---

### 2.4 Browser Security Rules: W3C EME & The `localhost` Exception

The W3C Encrypted Media Extensions (EME) specification mandates that `navigator.requestMediaKeySystemAccess()` can only execute within a **Secure Context**.
- **Production Requirement**: Delivery over `https://` with a valid TLS certificate.
- **Development / Localhost Exception**: Under W3C Secure Contexts Level 1, `http://localhost`, `http://127.0.0.1`, and `http://[::1]` are explicitly designated as *potentially trustworthy origins*.
- **Critical Pitfall**: If you open `http://192.168.1.100:8080` (a local LAN IP) or a custom local domain without HTTPS, Chrome will immediately block Widevine with:
  `DOMException: Only secure origins are allowed.`
  Always access the test player via `http://localhost:8080` or `http://127.0.0.1:8080`.

---

### 2.5 HTTP Delivery & CORS Headers

The web server serving the manifests (`live.mpd`, `live.m3u8`) and media segments (`*.m4s`) must support Cross-Origin Resource Sharing (CORS) because browser players fetch them via JavaScript `fetch()`:

1. **Mandatory Headers**:
   ```http
   Access-Control-Allow-Origin: *
   Access-Control-Allow-Methods: GET, HEAD, OPTIONS
   Access-Control-Allow-Headers: *
   Access-Control-Expose-Headers: Content-Length, Content-Type, Date, Server
   ```
2. **Preflight Support**: `OPTIONS` requests must immediately return `200 OK` or `204 No Content`.
3. **MIME Types**:
   - `.mpd` -> `application/dash+xml`
   - `.m3u8` -> `application/vnd.apple.mpegurl`
   - `.m4s` / `.mp4` -> `video/mp4`
4. **Cache Control for Live Streams**:
   - Manifests must specify: `Cache-Control: no-cache, no-store, must-revalidate`
   - Failure to disable caching causes browsers to serve stale playlists, freezing live playback.

---

## 3. Complete Step-by-Step Runnable Recipe

### Step 1: Prepare the Input Video in `scratch/test.mp4`

If you don't already have an MP4, generate a test clip containing synchronized audio and video with 2.0s GOP keyframe intervals using FFmpeg:

```bash
mkdir -p scratch
ffmpeg -y \
  -f lavfi -i "testsrc=duration=10:size=1280x720:rate=30" \
  -f lavfi -i "sine=frequency=1000:duration=10:sample_rate=48000" \
  -c:v libx264 -g 60 -keyint_min 60 -sc_threshold 0 -profile:v baseline -pix_fmt yuv420p \
  -c:a aac -b:a 128k -ar 48000 \
  -f mp4 scratch/test.mp4
```

---

### Step 2: Verify `.env` Credentials

Ensure your repository's `.env` file contains your Axinom credentials:

```dotenv
# Axinom Tenant & Key Service (SPEKE v2)
AXINOM_TENANT_ID=14ccfc47-be26-45b6-9b6c-9b830d04f47b
AXINOM_MANAGEMENT_KEY=c32b2b58-f550-4d6c-bedf-d1886f115dc9
AXINOM_SPEKE_ENDPOINT=https://14ccfc47.key-service-management.axprod.net/api/SpekeV2

# Axinom Licensing Service & Token Signing
AXINOM_COMMUNICATION_KEY_ID=51fb2115-0c73-471e-89a9-b4b300b71d12
AXINOM_COMMUNICATION_KEY=3fYX3GHOMuy4Cklgo2zXdzeMLqdEHgdBueRxQ+Uw/Ao=
AXINOM_WIDEVINE_LICENSE_URL=https://14ccfc47.drm-widevine-licensing.axprod.net/AcquireLicense
```

---

### Step 3: Run the Live Packaging & Ingest Harness

Here is the complete Rust test program that orchestrates the entire live session. It spawns FFmpeg in real-time, packages via `PackagingSession`, derives the Axinom Entitlement JWT from the acquired Key IDs, and keeps the live stream running:

```rust
//! Complete E2E Live DRM Ingest, Packaging and License Server integration.
//! Run with: cargo run --example e2e_live_test

use base64::prelude::*;
use bytes::Bytes;
use drmpack::axinom::{AxinomConfig, AxinomProvider};
use drmpack::session::{PackagingSession, PackagingSessionConfig};
use drmpack::types::Rendition;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::AsyncReadExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::dotenv();

    // 1. Output directory where GPAC writes manifests and media segments
    let output_dir = PathBuf::from("./live_out");
    let _ = tokio::fs::create_dir_all(&output_dir).await;

    println!("============================================================");
    println!("          DRMPACK E2E LIVE STREAM PACKAGING RUNNER          ");
    println!("============================================================");
    println!("Output directory: {}", output_dir.canonicalize()?.display());

    // 2. Initialize Axinom SPEKE v2 Key Provider
    let axinom_config = AxinomConfig::from_env()?;
    let provider = AxinomProvider::new(axinom_config);

    // 3. Configure PackagingSession for CENC (Widevine + PlayReady)
    let content_id = format!("live-test-{}", uuid::Uuid::new_v4());
    let session_config = PackagingSessionConfig::cenc(&content_id)
        .with_rendition(Rendition::video_hd())
        .with_output_dir(&output_dir)
        .with_segment_duration(2.0)
        .with_chunk_duration(0.2)
        .preserve_output(); // Keep segments on disk for playback

    println!("[1] Contacting Axinom Key Service (SPEKE v2) for live keys...");
    let mut session = PackagingSession::create(session_config, &provider).await?;
    println!("    -> Active KeySet acquired. Session supervisor running.");

    // 4. Generate the Axinom Entitlement JWT for client playback
    let com_key_id = std::env::var("AXINOM_COMMUNICATION_KEY_ID")?;
    let com_key = std::env::var("AXINOM_COMMUNICATION_KEY")?;
    
    // Extract the primary Key ID from the session's KeySet
    let primary_key = session
        .key_set()
        .all_keys()
        .next()
        .expect("No keys found in session KeySet");
    let kid_str = primary_key.kid.0.hyphenated().to_string();

    let jwt_token = generate_axinom_jwt(&com_key_id, &com_key, &kid_str)
        .expect("Failed to sign Axinom JWT");

    let wv_license_url = std::env::var("AXINOM_WIDEVINE_LICENSE_URL")?;

    println!("\n============================================================");
    println!("                 PLAYER LAUNCH CONFIGURATION                ");
    println!("============================================================");
    println!("Stream URL:       http://localhost:8080/live.mpd");
    println!("Widevine License: {}", wv_license_url);
    println!("Key ID (KID):     {}", kid_str);
    println!("Axinom JWT:       {}", jwt_token);
    println!("============================================================\n");

    // 5. Spawn FFmpeg to stream scratch/test.mp4 as live fMP4
    let input_file = "scratch/test.mp4";
    println!("[2] Launching FFmpeg real-time pacer for '{input_file}'...");
    let mut ffmpeg = tokio::process::Command::new("ffmpeg")
        .args([
            "-re",
            "-stream_loop", "-1", // loop indefinitely
            "-i", input_file,
            "-c:v", "libx264",
            "-g", "60",
            "-keyint_min", "60",
            "-sc_threshold", "0",
            "-profile:v", "baseline",
            "-pix_fmt", "yuv420p",
            "-c:a", "aac",
            "-b:a", "128k",
            "-ar", "48000",
            "-movflags", "empty_moov+default_base_moof+frag_keyframe",
            "-f", "mp4",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let mut stdout = ffmpeg.stdout.take().expect("Failed to open FFmpeg stdout");

    // 6. Ingest loop: pump FFmpeg stdout into drmpack PackagingSession
    println!("[3] Ingesting fMP4 chunks into GPAC. Ready for browser playback!");
    println!("    (Press Ctrl+C to terminate the live stream)\n");

    let mut buf = vec![0u8; 65536];
    loop {
        match stdout.read(&mut buf).await {
            Ok(0) => {
                println!("FFmpeg reached EOF");
                break;
            }
            Ok(n) => {
                session.push(Bytes::copy_from_slice(&buf[..n])).await?;
                if !session.is_alive() {
                    eprintln!("GPAC process exited unexpectedly!");
                    break;
                }
            }
            Err(e) => {
                eprintln!("Error reading from FFmpeg pipe: {e}");
                break;
            }
        }
    }

    session.close().await?;
    let _ = ffmpeg.kill().await;
    Ok(())
}

fn generate_axinom_jwt(com_key_id: &str, com_key_b64: &str, kid: &str) -> Option<String> {
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
```

---

### Step 4: Host with a CORS-Enabled HTTP Server

In a new terminal, serve the output directory (`./live_out`) and your player files on `http://localhost:8080`.

#### Lightweight Python CORS Server
Save this as `serve.py` and run `python3 serve.py 8080 ./live_out`:

```python
#!/usr/bin/env python3
import sys
from http.server import HTTPServer, SimpleHTTPRequestHandler

class CORSRequestHandler(SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header('Access-Control-Allow-Origin', '*')
        self.send_header('Access-Control-Allow-Methods', 'GET, HEAD, OPTIONS')
        self.send_header('Access-Control-Allow-Headers', '*')
        self.send_header('Access-Control-Expose-Headers', 'Content-Length, Content-Type, Date')
        self.send_header('Cache-Control', 'no-cache, no-store, must-revalidate')
        super().end_headers()

    def do_OPTIONS(self):
        self.send_response(204)
        self.end_headers()

if __name__ == '__main__':
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
    dir_path = sys.argv[2] if len(sys.argv) > 2 else '.'
    server = HTTPServer(('127.0.0.1', port), lambda *args: CORSRequestHandler(*args, directory=dir_path))
    print(f"Serving '{dir_path}' at http://localhost:{port} with CORS enabled...")
    server.serve_forever()
```

---

### Step 5: Web Browser Playback (`player.html`)

Place `player.html` inside the directory served by the HTTP server (or open it via `http://localhost:8080/player.html`).

#### Shaka Player Implementation

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <title>drmpack - Live DRM Playback Test (Widevine)</title>
  <!-- Load Shaka Player Compiled Library -->
  <script src="https://ajax.googleapis.com/ajax/libs/shaka-player/4.12.5/shaka-player.compiled.js"></script>
  <style>
    body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; background: #121212; color: #fff; text-align: center; padding: 20px; }
    video { width: 854px; height: 480px; background: #000; border-radius: 8px; box-shadow: 0 4px 20px rgba(0,0,0,0.5); }
    .status { margin-top: 15px; font-size: 14px; color: #4caf50; }
  </style>
</head>
<body>

  <h2>drmpack Live Widevine Stream Verification</h2>
  <video id="video" controls autoplay muted></video>
  <div id="status" class="status">Initializing Shaka Player...</div>

  <script>
    // 1. CONFIGURATION (Substitute with values emitted by the runner)
    const MANIFEST_URL = 'http://localhost:8080/live.mpd';
    const WIDEVINE_LICENSE_URL = 'https://14ccfc47.drm-widevine-licensing.axprod.net/AcquireLicense';
    const AXINOM_JWT_TOKEN = '<PASTE_JWT_TOKEN_HERE>';

    async function initPlayer() {
      // Install built-in polyfills
      shaka.polyfill.installAll();

      if (!shaka.Player.isBrowserSupported()) {
        document.getElementById('status').innerText = 'Browser does not support EME / Shaka Player!';
        document.getElementById('status').style.color = '#f44336';
        return;
      }

      const video = document.getElementById('video');
      const player = new shaka.Player(video);

      // Listen for error events
      player.addEventListener('error', (event) => {
        console.error('Shaka Player Error:', event.detail);
        document.getElementById('status').innerText = 'Playback Error: ' + event.detail.message + ' (Code ' + event.detail.code + ')';
        document.getElementById('status').style.color = '#f44336';
      });

      // 2. CONFIGURE WIDEVINE DRM SERVER
      player.configure({
        drm: {
          servers: {
            'com.widevine.alpha': WIDEVINE_LICENSE_URL
          }
        },
        streaming: {
          lowLatencyMode: true // Optional: enables low-latency DASH sync
        }
      });

      // 3. INJECT THE AXINOM JWT AUTHENTICATION HEADER
      // Axinom License Service mandates the token in the 'X-AxDRM-Message' header
      player.getNetworkingEngine().registerRequestFilter((type, request) => {
        if (type === shaka.net.NetworkingEngine.RequestType.LICENSE) {
          request.headers['X-AxDRM-Message'] = AXINOM_JWT_TOKEN;
          console.log('[DRM] Injecting X-AxDRM-Message header into Widevine challenge request');
        }
      });

      // 4. LOAD THE LIVE DASH STREAM
      try {
        document.getElementById('status').innerText = 'Loading live DASH manifest...';
        await player.load(MANIFEST_URL);
        document.getElementById('status').innerText = 'Decrypted & Streaming Live via Widevine L3!';
      } catch (e) {
        console.error('Failed to load stream:', e);
      }
    }

    document.addEventListener('DOMContentLoaded', initPlayer);
  </script>
</body>
</html>
```

#### Alternative: dash.js Implementation

```html
<script src="https://cdn.dashjs.org/v4.7.4/dash.all.min.js"></script>
<video id="videoPlayer" controls autoplay muted width="854" height="480"></video>
<script>
  const player = dashjs.MediaPlayer().create();
  player.initialize(document.querySelector("#videoPlayer"), "http://localhost:8080/live.mpd", true);
  player.setProtectionData({
    "com.widevine.alpha": {
      "serverURL": "https://14ccfc47.drm-widevine-licensing.axprod.net/AcquireLicense",
      "httpRequestHeaders": {
        "X-AxDRM-Message": "<PASTE_JWT_TOKEN_HERE>"
      }
    }
  });
</script>
```

---

## 4. In-Process License Proxy Architecture (`LicenseProxy`)

As an alternative to having the web browser communicate directly with Axinom's public license endpoints, `drmpack` provides an in-process license proxy module ([`drmpack::license::LicenseProxy`](../../src/license/proxy.rs)).

### Architecture Comparison

| Dimension | Direct Client-to-Axinom | Backend `LicenseProxy` |
| :--- | :--- | :--- |
| **Token Exposure** | JWT exposed in client-side JavaScript | Token kept strictly server-side |
| **License Acquisition URL** | Direct Axinom cloud endpoint | Local `/api/license/widevine` endpoint |
| **FairPlay Application Cert** | Fetched over network by each player | Cached in memory via `handle_fairplay_certificate` |
| **CORS Configuration** | Managed by Axinom CDN | Managed by your local media server |

### Integrating `LicenseProxy` in Rust

```rust
use drmpack::license::{handle_widevine_license, LicenseProxy};
use drmpack::vendor::axinom::AxinomLicenseConfig;

// Initialize once at server startup
let config = AxinomLicenseConfig::from_env()?;
let proxy = LicenseProxy::new(config);

// Inside your HTTP framework handler (e.g. Axum / Actix):
async fn handle_widevine_request(
    proxy: LicenseProxy,
    jwt_token: String,
    challenge_payload: bytes::Bytes,
) -> Result<impl IntoResponse, StatusCode> {
    match handle_widevine_license(&proxy, &challenge_payload, &jwt_token).await {
        Ok(license_resp) => Ok((
            [(reqwest::header::CONTENT_TYPE, "application/octet-stream")],
            license_resp.into_bytes(),
        )),
        Err(err) => {
            eprintln!("License error: {err}");
            Err(StatusCode::BAD_REQUEST)
        }
    }
}
```

---

## 5. Verification & Troubleshooting Playbook

### 5.1 Verification Checklist in Google Chrome

1. **Verify CDM Initialization**:
   - Open `chrome://media-internals` in a separate tab.
   - Look up the active player instance.
   - Confirm: `kIsDrm=true`, `kCdmType=Widevine`, `kDecryptorType=CdmDecryptor`.
2. **Inspect License Exchange in DevTools Network Tab**:
   - Filter by `AcquireLicense`.
   - Method: `POST`
   - Request Headers: Confirm `X-AxDRM-Message` contains your signed JWT string.
   - Status: `200 OK`
   - Response Payload: Binary octet stream (approx. 2KB - 4KB).

### 5.2 Common Error Codes & Resolutions

| Error / Symptom | Root Cause | Exact Resolution |
| :--- | :--- | :--- |
| **Shaka Error 6001** (`UNSUPPORTED_KEY_SYSTEM`) | Player accessed over an insecure HTTP context (e.g. `http://192.168.x.x`). | Access player strictly via `http://localhost:8080` or `http://127.0.0.1:8080`, or configure HTTPS with TLS certificates. |
| **Shaka Error 6007** (`LICENSE_REQUEST_FAILED`) with HTTP 401 | Missing or corrupted `X-AxDRM-Message` header. | Verify the request filter in Shaka: `request.headers['X-AxDRM-Message'] = token`. Ensure token is non-empty. |
| **Shaka Error 6007** with HTTP 400 (`Invalid DRM message`) | JWT signature verification failed on Axinom's servers. | Verify that `AXINOM_COMMUNICATION_KEY` in `.env` matches the key configured in the Axinom portal for `AXINOM_COMMUNICATION_KEY_ID`. |
| **Shaka Error 6007** with HTTP 400 (`Widevine request format error`) | Challenge bytes were truncated or modified before reaching Axinom. | Ensure no reverse proxy or middleware is decoding or modifying the raw binary `application/octet-stream` body. |
| **Playback freezes after 2 seconds** | Stale manifest cached by browser or FFmpeg pipeline exited on EOF. | Add `Cache-Control: no-cache` header in HTTP server; add `-stream_loop -1` to FFmpeg so it never exits on EOF. |
| **CORS Preflight Error (`Blocked by CORS policy`)** | HTTP server does not respond to `OPTIONS` requests or lacks `Access-Control-Allow-Headers`. | Configure HTTP server with `Access-Control-Allow-Origin: *` and `Access-Control-Allow-Headers: *` on all routes. |
| **GPAC Mux Error: `Missing CENC Key config`** | Track in source MP4 (e.g. subtitle/audio) was marked for encryption without a matching key. | Use `drmpack::QualityTier::sd()` for audio or omit subtitle tracks from encryption configuration. |

---

## 6. References & Primary Sources

- [Axinom DRM Documentation: Key Service (SPEKE v2)](https://portal.axinom.com/mosaic/documentation/drm)
- [Axinom DRM Documentation: Entitlement Message Specification](https://portal.axinom.com/mosaic/documentation/drm/entitlement-message)
- [GPAC Licensing & Filters: `cecrypt` and `dasher`](https://wiki.gpac.io/Filters/cecrypt/)
- [W3C Encrypted Media Extensions (EME) Specification](https://www.w3.org/TR/encrypted-media/)
- [W3C Secure Contexts Specification](https://www.w3.org/TR/secure-contexts/)
- [Shaka Player DRM Configuration Documentation](https://shaka-player-demo.appspot.com/docs/api/shaka.Player.html#configure)
- [DASH Industry Forum: CPIX 2.3 Specification](https://dashif.org/guidelines/cpix/)
