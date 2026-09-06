Status: proposed

# End-to-End Live Ingestion, Axinom DRM Packaging & Web Browser Playback

## Problem Statement

Developers and integrators working with `drmpack` need a way to verify the complete end-to-end media and DRM lifecycle:
1. **Lack of Runnable Real-World Testing Harness**: While unit tests and mock integration tests verify individual components in isolation, there is currently no single turnkey example that demonstrates the entire live streaming path from an input MP4 file to decrypted playback on a real web browser (Google Chrome Widevine CDM).
2. **Media Ingestion Gap**: Standard MP4 files cannot be directly ingested into GPAC live pipelines because they lack ISO-BMFF movie fragments (`moof` + `mdat`) and wall-clock pacing (`-re`). Developers must manually figure out complex FFmpeg CLI pipelines to stream test media into the packager.
3. **DRM Licensing Complexity & Multi-KID Entitlement**: To play encrypted streams in a browser, Axinom requires an HMAC-SHA256 signed JWT token (`X-AxDRM-Message`) referencing all encrypted Key IDs (KIDs). Manually generating this token, setting up an HTTP server with CORS headers (`Access-Control-Allow-Origin: *`) and MIME types (`application/dash+xml`, `video/mp4`), and wiring Shaka Player requires dozens of error-prone manual steps.
4. **Environment Lock-in**: Developers without active Axinom enterprise credentials cannot easily run the end-to-end flow to test playback functionality locally.

## Solution

A complete, self-contained, turnkey testing executable (`examples/06_e2e_live_playback.rs`) that:
1. **Automates Input Media & Live Pacing**: Automatically detects or generates a valid 10-second test video in `scratch/test.mp4` (H.264 + AAC with fixed 2.0s GOPs), then spawns an FFmpeg child process that loops the file indefinitely at 1x real-time speed (`-re -stream_loop -1`) emitting fragmented MP4 (`moof` + `mdat`) chunks directly into `session.push(bytes)`.
2. **Dual-Mode DRM Provider (Cloud Axinom + Offline Fallback)**:
   - **Axinom Mode**: If `.env` contains valid Axinom credentials, acquires production DRM keys via `AxinomProvider` (SPEKE v2 over CPIX 2.3) and generates a signed Axinom Entitlement JWT token embedding all active KIDs.
   - **Offline Mode**: If `.env` credentials are missing or `--offline` flag is passed, falls back to `StaticKeySource` with ClearKey signaling, allowing anyone to test decrypted browser playback with zero cloud dependencies.
3. **In-Process Embedded HTTP Server with CORS**: Runs a lightweight async HTTP server on `http://localhost:8080` that serves the GPAC live output directory (`live.mpd`, `*.m4s`, `live.m3u8`) with required streaming MIME types and permissive CORS headers.
4. **Instant Browser Playback (`player.html`)**: Serves a pre-configured Shaka Player web page at `http://localhost:8080` with the correct license server URL and authentication tokens pre-injected. The developer opens the browser and immediately watches the live decrypted video.

## User Stories

1. As a developer, I want to run `cargo run --example 06_e2e_live_playback`, so that I can immediately start an end-to-end live DRM packaging session with a single command.
2. As a developer, I want the harness to automatically generate `scratch/test.mp4` using FFmpeg if the file does not exist, so that I do not need to prepare test media manually.
3. As a developer, I want FFmpeg to pace media chunks in real time (`-re`) and loop infinitely (`-stream_loop -1`), so that GPAC receives a continuous live stream and does not trigger premature `#EXT-X-ENDLIST` EOF.
4. As a developer, I want the harness to acquire keys from Axinom Key Service when `.env` credentials are provided, so that I can verify production cloud DRM key exchange.
5. As a developer, I want the harness to fall back to an offline key source when Axinom credentials are not configured, so that the example remains runnable and testable in offline or CI environments.
6. As a developer, I want the harness to generate a valid Axinom Entitlement JWT token containing all active KIDs (video and audio), so that Axinom License Service authorizes playback for all tracks.
7. As a developer, I want an embedded HTTP server running on `http://localhost:8080` serving manifests and segments with CORS and correct MIME types, so that I do not have to run a separate Python script or web server.
8. As a developer, I want to open `http://localhost:8080` in Google Chrome and see the decrypted live stream playing automatically in Shaka Player, so that I have visual confirmation that packaging and DRM decryption succeed.
9. As a developer, I want the example to comply with the repo's documented standards (zero comments, zero emojis, clean formatting, clean clippy), so that code quality is strictly maintained.

## Implementation Decisions

1. **Executable Location & Target Declaration**:
   - Create `examples/06_e2e_live_playback.rs`.
   - Register target `[[example]] name = "06_e2e_live_playback"` in `Cargo.toml` with required features (`axinom`, `license-proxy`).
2. **Automated Input Preparation & Ingestion**:
   - Check if `scratch/test.mp4` exists; if not, invoke `ffmpeg -f lavfi -i testsrc=... -f lavfi -i sine=... -c:v libx264 -g 60 -keyint_min 60 -c:a aac scratch/test.mp4`.
   - Spawn FFmpeg child process:
     `ffmpeg -re -stream_loop -1 -i scratch/test.mp4 -c copy -movflags empty_moov+default_base_moof+frag_keyframe -f mp4 pipe:1`
     (or transcode if input codecs/GOP are incompatible).
   - Read from `ffmpeg.stdout` into a 64 KB buffer in a Tokio loop and call `session.push(chunk_bytes).await`.
3. **Packaging Configuration**:
   - Use `PackagingSessionConfig::new("e2e-live-playback")` with `Rendition::video_hd()` and `Rendition::audio()`.
   - Configure output directory at `scratch/live_out`.
   - Set `.preserve_output()` so files remain available for HTTP serving during and after playback.
4. **Axinom Token & License Proxy**:
   - If Axinom credentials exist, compute HMAC-SHA256 JWT using `ring::hmac` matching Axinom Entitlement Message format v2 with inline KIDs.
   - If offline mode, use ClearKey with a known test KID/Key pair.
5. **Embedded HTTP Server**:
   - Implement a lightweight TCP/HTTP server using Tokio (`tokio::net::TcpListener`).
   - Handle:
     - `GET /` or `GET /player.html`: Returns HTML containing Shaka Player with the pre-populated license URL and token.
     - `GET /live.mpd`, `GET /*.m4s`, `GET /*.mp4`, `GET /*.m3u8`: Reads file from `scratch/live_out` and returns with CORS headers (`Access-Control-Allow-Origin: *`, `Access-Control-Allow-Headers: *`) and content types:
       - `.mpd` -> `application/dash+xml`
       - `.m3u8` -> `application/vnd.apple.mpegurl`
       - `.m4s` / `.mp4` -> `video/mp4`
     - `OPTIONS *`: Returns 204 No Content with CORS preflight headers.
6. **Graceful Shutdown**:
   - Listen for `tokio::signal::ctrl_c()`.
   - On Ctrl+C, terminate FFmpeg child process, finalize `session.close().await`, and stop the HTTP server cleanly.
