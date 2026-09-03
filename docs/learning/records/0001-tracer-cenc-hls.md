# Architectural Pivot: GPAC Subprocess Filter Graph & Ramdisk Orchestration

Documented the critical engineering pivot from the native Rust MP4/CENC spike (ADR-0002) to industrial GPAC subprocess orchestration (ADR-0004) and Ramdisk manifest distribution (ADR-0005).

## Key Insights
- **Hardware CDM Strictness**: Pure native Rust ISO-BMFF rewriting carries extreme risks of silent decryption failures on strict hardware CDMs (Widevine L1, Apple FairPlay, PlayReady) due to subtle NALU parsing, subsample offset, and slice header discrepancies.
- **GPAC Subprocess Architecture**: By orchestrating the battle-tested GPAC filter engine (`stdin:ext=mp4` -> `cecrypt:cfile=drm.xml` -> `dasher:dual`) over anonymous kernel pipes, `drmpack` guarantees 100% compliance with hardware CDMs while isolating C-library memory safety from `media-server`.
- **Low-Latency CMAF Chunking**: GPAC natively supports CMAF chunking (`:cdur=0.2`), LL-HLS byte-range partial segments (`EXT-X-PART`), and LL-DASH availability time offsets (`asto`), solving latency requirements that older tools like Shaka Packager could not satisfy.
- **Zero Disk I/O via Ramdisk**: Manifests (`.m3u8`, `.mpd`) and partial chunks are written directly to memory-backed `/dev/shm` (or `tmpfs`), eliminating physical disk wear on sub-second manifest update cycles.
- **Lifecycle & Draining**: `PackagingSession` provides explicit manifest paths (`hls_manifest_path`, `dash_manifest_path`) and a clean `cleanup()` lifecycle for reclaiming Ramdisk memory after client playback buffers drain.
