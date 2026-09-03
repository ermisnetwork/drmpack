# GPAC subprocess orchestration over native rewrite

**Status: Accepted** (supersedes ADR-0002)

drmpack orchestrates the industrial GPAC engine (`gpac` filter graph) as a persistent long-running subprocess over anonymous Unix pipes instead of implementing an MP4 box parser and CENC/CBCS ciphers natively in Rust from scratch. This guarantees 100% compliance with strict hardware CDM decoders (Widevine, FairPlay, PlayReady), isolates C-library memory safety from `media-server`, and provides native support for Low-Latency CMAF chunking (LL-HLS and LL-DASH).

## Considered options

- **Native Rust implementation from scratch**: Pure Rust and in-process, but carries extreme engineering overhead and severe risks of unlogged playback failures on strict hardware CDMs (SmartTVs, iOS Safari) due to subtle NALU/ISOBMFF box discrepancies.
- **Wrap Shaka Packager**: Battle-tested for DRM and standard VOD/Live, but lacks native support for Low-Latency HLS (`EXT-X-PART`) and CMAF chunking (Issue #675 remains in backlog).
- **C/C++ FFI binding (Static linking)**: Zero-copy, but exposes `media-server` to memory safety crashes in C libraries and complicates LGPL v2.1 licensing.

## Consequences

- GPAC binary must be present in the runtime container / host environment.
- `drmpack`'s core value focuses on Key Management (CPIX, Axinom, SPEKE v2), Quality Tier mapping, DRM XML configuration generation, subprocess watchdog, and License Proxy handlers.
- Process crashes are handled with a fail-fast policy (`Err(ProcessCrashed)`), avoiding corrupted timeline states.
