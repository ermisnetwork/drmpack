# 01: Tracer — GPAC Pipe Orchestration (HLS + DASH)

**What to build:** The thinnest end-to-end path through drmpack as a GPAC Orchestrator. Create the `GpacProcess` manager that spawns `gpac` as a long-running subprocess with anonymous Unix pipes (`stdin` for media input). Implement a basic DRM XML generator for GPAC's `cecrypt` filter using keys from `RawKeyProvider`. Stream a muxed fMP4 sample through `PackagingSession::push_segment()`, outputting both HLS (`.m3u8`) and DASH (`.mpd`) into a Ramdisk/temp directory. Verify that output manifests contain correct DRM tags (PSSH, `#EXT-X-KEY`) and that media chunks are encrypted.

**Blocked by:** None (can start immediately)

**Status:** done

- [x] Define core domain types: `PackagingSessionConfig`, `LatencyMode` (Standard/LowLatency), `Rendition`, `QualityTier`, `EncryptionScheme`, `ContentKey`, `KeyID`
- [x] Implement `RawKeyProvider` providing in-memory test keys and PSSH
- [x] Implement GPAC DRM XML generator: construct valid `drm.xml` with `CrypTrack`, `Key`, and `DRMInfo` (PSSH)
- [x] Implement `GpacProcess`: spawn `gpac` using `tokio::process::Command` with `stdin:ext=mp4`, `cecrypt`, and `dasher:dual`
- [x] Implement `PackagingSession::create()` and `PackagingSession::push_segment(&[u8])` writing to `child_stdin`
- [x] Implement `PackagingSession::close()` closing stdin and awaiting process exit
- [x] End-to-end tracer test: push sample fMP4 through session, verify `.m3u8` and `.mpd` generated in output dir with DRM signaling
