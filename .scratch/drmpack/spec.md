Status: ready-for-agent

# drmpack — Low-Latency DRM Packaging Orchestrator

## Problem Statement

The `ermis-stream` media-server needs to deliver DRM-protected live and VOD content across Web, Android, iOS, and SmartTV platforms. Key requirements include **Very Low Latency** (LL-HLS with `EXT-X-PART`, LL-DASH with CMAF chunking) as well as **Standard Latency** (regular 2-6s segments), multi-DRM support (Google Widevine, Apple FairPlay, Microsoft PlayReady), and per-quality-tier key policies (SD/HD/4K).

Previous approaches either lacked LL-HLS support (Shaka Packager does not support `EXT-X-PART` or chunked CMAF) or posed catastrophic CDM compatibility risks and massive development overhead (writing an ISOBMFF/CENC parser from scratch in Rust). Media-server requires a high-performance Rust orchestrator that leverages the battle-tested, CMAF-native GPAC filter engine via isolated long-running subprocess pipes, eliminating disk I/O bottlenecks via Ramdisk/tmpfs while maintaining strict memory safety in the host server.

## Solution

`drmpack` is a Rust crate imported directly by `media-server`. It provides:

- A **PackagingSession** orchestrator that manages the packaging lifecycle, key acquisition, and GPAC subprocess execution.
- A streaming data plane: accepts muxed fMP4 streams from `media-server` and pipes them directly into GPAC's `stdin` via anonymous kernel Unix pipes (zero disk I/O on the input path).
- A Ramdisk output manager: GPAC writes manifests (`.m3u8`, `.mpd`) and CMAF chunks/segments directly into shared memory (`/dev/shm` or `tmpfs`) for instant sub-second serving to CDN/HTTP edge players without physical disk wear.
- Multi-DRM configuration: automatically generates GPAC `cecrypt` XML configs with correct PSSH boxes for Widevine, FairPlay, and PlayReady.
- A pluggable **KeyProvider** trait supporting CPIX 2.3, AWS SPEKE v2, Axinom Key Service, and RawKey (for testing).
- **LatencyMode** selection: `LowLatency` (CMAF chunk duration 200-500ms, LL-HLS byte-range/parts, LL-DASH availability time offset) or `Standard` (regular segment packaging).
- Fail-fast process watchdog: monitors the GPAC child process, capturing unexpected exits or stderr failures and reporting structured errors immediately.
- Async **License Proxy handler functions** that `media-server` mounts on its HTTP routes, proxying player license challenges to DRM providers.

## User Stories

1. As a media-server developer, I want to create a `PackagingSession` with rendition declarations, latency mode (`LowLatency` or `Standard`), and DRM configuration in a single API call.
2. As a media-server developer, I want to stream muxed fMP4 segments into `session.push_segment()`, having bytes piped directly into GPAC's stdin without touching disk.
3. As a media-server developer, I want manifests (`.m3u8`, `.mpd`) and CMAF chunks written to a designated Ramdisk path (`/dev/shm`), so that HTTP edge handlers serve them with zero I/O latency.
4. As a media-server developer, I want to configure encryption schemes: `Cenc` (AES-CTR for Widevine/PlayReady), `Cbcs` (AES-CBC 1:9 for FairPlay), or `Dual` (simultaneous outputs for full cross-device reach).
5. As a media-server developer, I want per-quality-tier keys (SD, HD, 4K), so that each tier is encrypted with a distinct ContentKey fetched in a single batch request.
6. As a media-server developer, I want keys cached for the lifetime of the `PackagingSession`, avoiding redundant key fetches.
7. As a media-server developer, I want a `RawKeyProvider` for development and automated integration tests without external DRM servers.
8. As a media-server developer, I want a `CpixProvider` to fetch multi-tier keys via standardized CPIX 2.3 XML documents.
9. As a media-server developer, I want an `AxinomProvider` to integrate with Axinom's Key Service API according to official documentation.
10. As a media-server developer, I want a `SpekeV2Provider` for AWS DRM key exchange workflows.
11. As a media-server developer, I want async license proxy handlers (`handle_widevine_license`, `handle_fairplay_license`, `handle_playready_license`) to mount directly on Axum/Actix web routes.
12. As a media-server developer, I want graceful session termination via `session.close()`, which closes GPAC's stdin pipe, awaits clean manifest finalization (e.g. `#EXT-X-ENDLIST`), and frees Ramdisk resources.
13. As a media-server developer, I want a fail-fast watchdog that detects GPAC crashes and returns structured `Err(ProcessCrashed)` to trigger failover or stream restart.
14. As a media-server developer, I want structured logging via the `tracing` crate to observe GPAC stderr, pipe throughput, and key acquisition.

## Implementation Architecture

### Control Plane (`drmpack` in Rust)
- **Key acquisition**: Calls `KeyProvider::fetch_keys()` at session start.
- **DRM XML generation**: Constructs temporary `drm.xml` defining `CrypTrack` elements, key IDs, keys, and PSSH metadata for GPAC's `cecrypt` filter.
- **Process spawning**: Launches `gpac` using `tokio::process::Command` configured with:
  - Input: `pipe://stdin:fmt=mp4` (reading continuous fMP4 stream).
  - Cryptor: `cecrypt:cfile=<path_to_drm_xml>`.
  - Dasher: `dasher:profile=live:dmode=dynamic:segdur=<duration>` + (if LowLatency: `:cdur=0.2:asto=1.8:llhls=br`).
  - Output: `live.mpd:dual` (generating both DASH MPD and HLS m3u8 in Ramdisk).
- **Process management**: Manages `ChildStdin` streaming, background stderr logging, and watchdog timeout/termination.

### Data Plane (Kernel Pipes & Ramdisk)
- Input path: `media-server` -> `PackagingSession::push_segment(&[u8])` -> `tokio::io::AsyncWriteExt::write_all(&mut child_stdin)`.
- Output path: GPAC writes manifests and media chunks directly to `/dev/shm/<session_id>/`.
- Cleanup: `PackagingSession` removes `/dev/shm/<session_id>/` upon session termination or drop.

## Testing Strategy

- **Unit tests**: DRM XML generator correctness, configuration validation, CPIX XML parser, RawKeyProvider.
- **Integration tests (E2E)**:
  - Spawn real `gpac` subprocess (or mocked binary in CI environments where gpac is absent).
  - Pipe real fMP4 video fragments into `PackagingSession`.
  - Assert that `.m3u8` and `.mpd` appear in the output directory with valid DRM tags (`#EXT-X-KEY`, PSSH).
  - Assert that CMAF chunks are generated and encrypted.
  - Verify fail-fast watchdog on simulated process crash.

## Out of Scope

- Transcoding / encoding video (media-server responsibility).
- Direct CDN upload over network (media-server / edge reverse proxy responsibility).
- Key rotation during a live session (deferred to later milestone).
- Native in-Rust MP4 box rewriting.
