# 01: End-to-End Live Ingestion, Axinom Packaging, and Web Browser Playback Harness

**What to build:** A self-contained runnable testing harness (`examples/06_e2e_live_playback.rs`) that demonstrates the complete end-to-end DRM workflow: taking a local test video (`scratch/test.mp4`), automatically generating it with FFmpeg if missing, streaming it as a continuous live fMP4 stream paced in real-time (`-re -stream_loop -1`), acquiring production DRM keys from Axinom Key Service (or falling back to offline ClearKey), packaging with GPAC into encrypted DASH/HLS manifests, running an embedded HTTP server on `http://localhost:8080` with CORS headers and streaming MIME types, and serving an automated Shaka Player web page with pre-signed Axinom JWT tokens for instant browser playback on Google Chrome.

**Blocked by:** None (can start immediately)

**Status:** closed

- [x] `examples/06_e2e_live_playback.rs` is implemented and declared in `Cargo.toml` under `[[example]]` with `axinom` and `license-proxy` features.
- [x] Automatically checks for `scratch/test.mp4` and creates a 10-second H.264/AAC test card with fixed 2.0s GOPs via FFmpeg if not present.
- [x] Spawns a real-time looping FFmpeg process (`-re -stream_loop -1 -movflags empty_moov+default_base_moof+frag_keyframe`) and streams fragmented MP4 chunks directly into `session.push(bytes)`.
- [x] Supports dual-mode DRM key acquisition: uses `AxinomProvider` (SPEKE v2 / CPIX 2.3) when `.env` credentials exist, and falls back gracefully to `StaticKeySource` (ClearKey) when offline or credentials are missing.
- [x] Generates an HMAC-SHA256 signed Axinom Entitlement JWT token embedding all active KIDs (video and audio) for Widevine authorization.
- [x] Embeds an in-process async HTTP server on `http://localhost:8080` serving the GPAC output directory (`live.mpd`, `*.m4s`, `live.m3u8`) with CORS headers and streaming MIME types (`application/dash+xml`, `application/vnd.apple.mpegurl`, `video/mp4`).
- [x] Serves a pre-configured web player page at `http://localhost:8080/` hosting Shaka Player with Widevine CDM configuration and pre-populated Axinom JWT token, enabling one-click playback in Google Chrome.
- [x] Handles graceful shutdown on `Ctrl+C` by terminating the FFmpeg child process, finalizing `session.close().await`, and stopping the HTTP server cleanly.
- [x] Adheres strictly to repo coding standards: zero comments in `examples/06_e2e_live_playback.rs`, zero emojis/icons in console outputs, passes `cargo fmt --check`, and passes `cargo clippy --all-targets --all-features -- -D warnings`.
