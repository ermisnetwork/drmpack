# GPAC subprocess orchestration over native rewrite

**Status: Accepted (supersedes ADR-0002)**

drmpack orchestrates the industrial GPAC engine (`gpac` filter graph) as a persistent long-running subprocess over anonymous Unix pipes instead of implementing an MP4 box parser and CENC/CBCS ciphers natively in Rust from scratch. This guarantees 100% compliance with strict hardware CDM decoders (Widevine, FairPlay, PlayReady), isolates C-library memory safety from `media-server`, and provides native support for Low-Latency CMAF chunking (LL-HLS and LL-DASH).

## Considered options

- **Native Rust implementation from scratch**: Pure Rust and in-process, but carries extreme engineering overhead and severe risks of unlogged playback failures on strict hardware CDMs (SmartTVs, iOS Safari) due to subtle NALU/ISOBMFF box discrepancies.
- **Wrap Shaka Packager**: Battle-tested for DRM and standard VOD/Live, but lacks native support for Low-Latency HLS (`EXT-X-PART`) and CMAF chunking (Issue #675 remains in backlog).
- **C/C++ FFI binding (Static linking)**: Zero-copy, but exposes `media-server` to memory safety crashes in C libraries and complicates LGPL v2.1 licensing.

## Consequences

- GPAC binary must be present in the runtime container / host environment.
- `drmpack`'s core value focuses on Key Management (CPIX, Axinom, SPEKE v2), Quality Tier mapping, DRM XML configuration generation, subprocess watchdog, and License Proxy handlers.
- Process crashes are handled with a fail-fast policy (`Err(ProcessCrashed)`), avoiding corrupted timeline states.

## Implementation Reality / Amendments

While the core decision to orchestrate GPAC as an external subprocess remains in production, subsequent ADRs refined the I/O topology, lifecycle supervision, and encryption profiles:
- **I/O Topology Specialization**:
  - **Anonymous Unix Pipes** are strictly utilized for media ingestion (`stdin` fed via `PackagingSession::push` or `SessionWriter`) and diagnostic monitoring (`stderr` parsed in real time).
  - **Staging Directory & In-Memory Harvesting**: Output from GPAC's `dasher` filter is written to an ephemeral staging directory (backed by OS temporary storage / kernel Page Cache per [ADR-0015](./0015-direct-output-channel-and-safe-storage.md)). An asynchronous harvester immediately streams finalized media segments and playlists into RAM through an in-memory channel emitting `PackagedArtifact` structs via `take_output_receiver()`, unlinking files immediately to prevent storage buildup.
- **Process Supervision ([ADR-0011](./0011-async-process-supervisor-and-fail-fast-lifecycle.md))**:
  - Subprocess lifecycle is governed by an asynchronous `ProcessSupervisor` task owning `child.wait()`.
  - It provides instantaneous crash detection without polling, parses uncolored plaintext logs (`-logs=ncl`), maintains a bounded ring buffer of diagnostic stderr lines, and orchestrates symmetric teardown for dual-scheme representations.
- **Canonical Default Baseline ([ADR-0014](./0014-cbcs-and-standard-latency-as-default-baseline.md))**:
  - Although originally prototyped with Low-Latency CENC, production defaults in `PackagingSessionConfig::new` standardized on Single `EncryptionScheme::Cbcs` and `LatencyMode::Standard` (2.0s segments with 4000ms suggested presentation delay) for universal multi-DRM convergence and broadcast playback stability.
