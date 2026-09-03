# Mission: Mastering Low-Latency DRM Packaging Orchestration (drmpack + GPAC)

## Why
The engineer needs complete mastery and operational control over `drmpack` to confidently build, maintain, and debug high-performance, very low-latency DRM packaging (LL-HLS / LL-DASH via CMAF chunking), subprocess pipe streaming, multi-DRM key orchestration (CPIX, Axinom, SPEKE v2), and license proxying for `media-server`.

## Success looks like
- Deep understanding of the GPAC filter pipeline architecture: `stdin:ext=mp4` -> `cecrypt:cfile` -> `dasher:dual` with dynamic CMAF chunking.
- Mastery of Very Low Latency streaming mechanics: LL-HLS partial segments (`#EXT-X-PART`, `#EXT-X-PRELOAD-HINT`) and LL-DASH availability time offsets (`asto`, `availabilityTimeComplete="false"`).
- Mastery of GPAC Common Encryption XML formatting: configuring `CrypTrack`, `Key` (KID/hex value), and `DRMInfo` (PSSH metadata for Widevine, FairPlay, and PlayReady).
- Deep understanding of `PackagingSession` orchestration: zero-overhead anonymous Unix pipe streaming into GPAC's `stdin`, Ramdisk (`/dev/shm` or `tmpfs`) manifest delivery, and graceful lifecycle shutdown.
- Robust failure handling: implementing fail-fast watchdog supervision to capture subprocess crashes and prevent corrupted live stream state.

## Constraints
- Rust async with Tokio.
- Subprocess isolation with anonymous Unix pipes (zero disk I/O on media ingestion).
- Output distribution via Ramdisk (`/dev/shm` or `tmpfs`) for sub-second manifest refreshes.
- Strict adherence to domain vocabulary (`CONTEXT.md`) and architectural decisions (ADR-0004 & ADR-0005).

## Out of scope
- Transcoding / video codec decoding (unhandled by packager; media-server responsibility).
- Direct CDN network uploads (handled by media-server / reverse proxy edge).
- Native byte-level ISOBMFF rewriting (delegated to GPAC engine per ADR-0004).
