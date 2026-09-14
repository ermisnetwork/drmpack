# drmpack Roadmap

This document outlines the development roadmap, architectural milestones, and delivery status for `drmpack`. It tracks completed capabilities as well as near-term, medium-term, and long-term targets using actionable checklists.

### Status Legend
- [x] **Completed** — Implemented, validated, and released in current stable versions.
- [ ] **Planned** — Scheduled for implementation in the indicated release milestone.

---

## 1. Milestone v0.1.x: Core Packaging & Baseline (Released)

Initial production-ready baseline for in-process live packaging, DRM encryption, and manifest orchestration.

### Core Engine & Ingress Data Plane
- [x] In-process library architecture sharing Tokio runtime with host media server ([ADR-0001](docs/adr/0001-library-over-service.md))
- [x] Subprocess orchestration with GPAC `cecrypt` and `dasher` filter graphs over anonymous Unix pipes ([ADR-0004](docs/adr/0004-gpac-subprocess-over-native-rewrite.md))
- [x] Zero-disk media ingress via anonymous Unix pipes (`PackagingSession::push`)
- [x] `SessionWriter` adapter implementing `tokio::io::AsyncWrite` for zero-copy stream piping
- [x] Asynchronous `ProcessSupervisor` with 256-line ring buffer and fail-fast teardown ([ADR-0011](docs/adr/0011-async-process-supervisor-and-fail-fast-lifecycle.md))
- [x] Subprocess crash isolation with symmetric peer abort for dual-scheme sessions (`SIGKILL` peer teardown)

### Output & Manifest-Driven Harvesting
- [x] Ephemeral storage staging in OS tempdir (`/tmp`) backed by kernel page cache ([ADR-0015](docs/adr/0015-direct-output-channel-and-safe-storage.md))
- [x] Event-driven `ArtifactHarvester` utilizing kernel notifications (`inotify`/`FSEvents`) with 50ms low-latency watchdog ([ADR-0016](docs/adr/0016-production-event-driven-harvester-and-ephemeral-ingest.md))
- [x] In-memory asynchronous egress channel (`mpsc::Receiver<PackagedArtifact>`)
- [x] Manifest-Driven Readiness gating media segments strictly on canonical HLS playlist references
- [x] Binary ISOBMFF box verification (`ftyp`+`moov` for init segments, `moof`+`mdat` for media segments)
- [x] Static MPD duration sanitization and `#EXT-X-ENDLIST` verification on session finalization
- [x] Standardized semantic manifest and segment naming convention (`video_{Height}p.m3u8`, `video_{Height}p_{Number}.m4s`)

### DRM Schemes, Key Management & Protocols
- [x] Canonical baseline on single-scheme CBCS (AES-CBC 1:9 pattern for video, 0:0 for audio) and Standard Latency ([ADR-0014](docs/adr/0014-cbcs-and-standard-latency-as-default-baseline.md))
- [x] CENC (AES-CTR) and concurrent Dual representation packaging ([ADR-0006](docs/adr/0006-dual-cenc-cbcs-representations.md))
- [x] DASH-IF CPIX 2.3 XML request builder and pull-parser engine
- [x] AWS SPEKE v2.0 REST client supporting Basic, Bearer, API Key, and AWS SigV4 authentication ([ADR-0008](docs/adr/0008-pluggable-speke-signer-over-heavyweight-aws-sdk.md))
- [x] Key mapping policies: `SharedAll`, `SharedVideoSingleAudio`, and `PerTierAndTrack` ([ADR-0007](docs/adr/0007-selective-encryption-and-key-mapping-policy.md))
- [x] Multi-track ABR, QualityTier bindings, and selective clear track bypass (unencrypted WebVTT subtitles per [ADR-0013](docs/adr/0013-lean-rendition-and-track-id-disambiguation.md))
- [x] In-memory `StaticKeySource` for offline test suites and headless CI pipelines

### Vendor Integrations & Licensing
- [x] Axinom Key Service integration with mandatory tenant endpoint enforcement ([ADR-0018](docs/adr/0018-mandatory-tenant-endpoints-for-axinom.md))
- [x] High-performance HS256 JWT entitlement token minting utility
- [x] Safe playback metadata extraction via `DrmStreamMetadata` with zero raw key leakage ([ADR-0017](docs/adr/0017-drm-playback-metadata-handoff-and-credentials.md))
- [x] In-process `LicenseProxy` with persistent connection pooling and thundering-herd protected FairPlay cert caching ([ADR-0009](docs/adr/0009-license-proxy-connection-pooling-and-cert-caching.md))
- [x] 9 comprehensive end-to-end examples including an Axum playback web server

---

## 2. Milestone v0.2.0: Zero-Disk HTTP Egress & VOD Engine (Near-Term)

Focuses on eliminating filesystem staging completely for live streams via in-process loopback HTTP push, providing standalone static VOD packaging, and establishing production observability.

### Code Health & Debt Cleanup (v0.1.1 Review Baseline)
- [x] Concurrent multi-scheme key fetch consolidation: replace manual `pop()` and dead branching with `tokio::try_join!` in `AxinomProvider`
- [x] Eliminate redundant `reqwest::Client` builder fallback in `LicenseProxy::new`
- [x] Apply Ponytail review optimizations: non-allocating static slice `QualityTier` match fallback, clean client builder

### Zero-Disk In-Process HTTP Egress Engine
- [x] Pluggable `EgressMode` abstraction supporting `EgressMode::FileSystemStaging` (canonical default backed by Linux Page Cache) and `EgressMode::HttpPush` (in-process zero-disk mode)
- [x] In-process HTTP loopback sink (`httpout:hmode=push`): GPAC pushes packaged segments and playlists directly via HTTP PUT to an internal lightweight listener in RAM (`127.0.0.1:<ephemeral_port>`) with UUID auth token
- [x] Zero-disk buffer handoff forwarding received HTTP request payloads directly into `output_rx: mpsc::Receiver<PackagedArtifact>`, completely bypassing filesystem staging and kernel file watchers for live streams
- [x] Dual-scheme routing support (`/cenc/*` and `/cbcs/*`) for concurrent Dual live packaging sessions
- [x] Low-latency chunked transfer support (`Transfer-Encoding: chunked`) for CMAF chunk delivery

### VOD Whole-File Batch Packaging (`drmpack::vod`)
- [ ] Standalone batch packaging API `package_vod_file(config, key_provider)` decoupled from real-time live streaming sessions
- [ ] Byte-Range Single-File mode (`profile=onDemand`) generating a single `.mp4` per rendition containing an `sidx` box, `#EXT-X-BYTERANGE` in HLS, and `<SegmentBase>` in DASH (reduces storage file count by 99% per Apple HLS & DASH-IF specifications)
- [ ] Discrete Multi-Segment mode emitting independent `.m4s` segments for traditional segmented storage topologies
- [ ] Flexible input support: Single multiplexed MP4 container or separate rendition track files
- [ ] Static manifest generation with `#EXT-X-ENDLIST`, exact timeline duration, and DRM signaling (Widevine PSSH, PlayReady PSSH, FairPlay `#EXT-X-KEY`)
- [ ] Integration test suite validating static manifests and single-file byte-range playback

### Observability & Telemetry Foundation
- [ ] OpenTelemetry distributed tracing spans across session lifecycle (key acquisition, process spawn, streaming, finalization)
- [ ] Prometheus metrics instrumentation (ingest throughput, segment harvest latency, ring buffer diagnostics, GPAC pipe backpressure)

---

## 3. Milestone v0.3.0: Enterprise DRM & Next-Gen Formats (Medium-Term)

Focuses on enterprise 24/7 live operations, key rotation, additional commercial DRM providers, and modern video/audio codec signaling.

### 24/7 Live Enterprise DRM & Key Rotation
- [ ] Dynamic Key Rotation and cryptoperiod sequencing over SPEKE v2 / CPIX for continuous live packaging
- [ ] Multi-vendor DRM KeyProvider adapters: BuyDRM KeyOS, EZDRM, and Irdeto
- [ ] Automatic key pre-fetching and synchronization ahead of cryptoperiod transition boundaries

### Next-Gen Codecs & Signaling
- [ ] HEVC / H.265 packaging with HDR10, HLG, and Dolby Vision manifest signaling
- [ ] AV1 video packaging support across DASH and HLS
- [ ] Multi-audio and spatial audio tracks (Dolby Atmos, AC-4)

### Production Hardening
- [ ] End-to-end distributed trace propagation from host media server through packaging pipelines
- [ ] Fine-grained memory and CPU profiling benchmarks under 100+ concurrent packaging sessions

---

## 4. Milestone v1.0.0: General Availability & Enterprise Baseline (Long-Term)

Marks production stability, API freeze, multi-platform certification, and memory safety assurance.

### Stability & API Freeze
- [ ] Public API freeze and SemVer 1.0 stability guarantee for core traits and structs (`PackagingSession`, `KeyProvider`, `PackagedArtifact`, `DrmStreamMetadata`)
- [ ] Formal Minimum Supported Rust Version (MSRV) policy and automated CI enforcement

### Multi-Platform & Quality Assurance
- [ ] Tier-1 multi-platform CI matrix: Linux x86_64 (glibc/musl), aarch64 / ARM64, and macOS
- [ ] Automated continuous fuzz testing suite for DASH-IF CPIX XML parsing and ISOBMFF box verification
- [ ] Enterprise production deployment, operational runbooks, and performance tuning guide

---

## 5. Explicit Non-Goals

The following areas are intentionally out of scope to preserve `drmpack`'s role as a focused, high-performance packaging orchestrator:

- [x] **No Media Transcoding / Decoding**: Video/audio decoding, filtering, and encoding belong upstream (in FFmpeg or `media-server`). `drmpack` strictly accepts muxed fMP4 elementary streams.
- [x] **No Direct External CDN Uploads / Publishing**: `drmpack` delivers packaged artifacts exclusively via in-process memory channels (`output_rx: mpsc::Receiver<PackagedArtifact>`). Uploading or publishing to external CDN origins (AWS S3, Akamai, Cloudflare) is strictly the responsibility of the host `media-server`.
- [x] **No Legacy MPEG-2 TS SAMPLE-AES**: Rejecting legacy TS packaging per [ADR-0010](docs/adr/0010-cmaf-only-over-legacy-hls-ts.md). Standardized 100% on CMAF fMP4 for CBCS and CENC.
- [x] **No Client Video Player Hosting**: `drmpack` is an in-process library for backend services. Embedded player examples (`09_axum_playback_server`) exist solely for integration testing.
- [x] **No Interleaved Single-Pipe DASH/HLS**: Adaptive streaming generates multi-resource document trees that cannot be multiplexed into an unstructured scalar pipe without corrupted framing. Zero-disk egress is achieved via In-Process HTTP Sink.
