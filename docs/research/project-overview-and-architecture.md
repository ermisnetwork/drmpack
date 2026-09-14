# Technical Overview & Architecture Report: `drmpack`

> **Document Scope**: Comprehensive architectural reference, data flow diagrams, DRM wire protocol specifications, and operational semantics for the `drmpack` library.  
> **Primary Source Verification**: Directly grounded in source code (`src/`), manifests (`Cargo.toml`), architectural decision records (`docs/adr/`, `CONTEXT.md`), and end-to-end examples (`examples/`).

---

## 1. Executive Summary & Design Philosophy

`drmpack` is an in-process Rust media packaging orchestrator designed for media servers. Unlike standalone daemons or microservices, `drmpack` integrates directly into host media servers (such as Axum, Actix, or custom Tokio-based engines) to handle **live and low-latency DRM packaging (CENC / CBCS)** and **dynamic manifest synthesis (HLS / MPEG-DASH)**.

### Core Architectural Principles
1. **In-Process Orchestration**: Runs within the media server process space, receiving multiplexed fragmented MP4 (fMP4) chunks directly from memory buffers.
2. **Zero Disk I/O Ingest**: Pipes media streams into underlying GPAC filter graphs over **anonymous Unix pipes**, avoiding physical disk writes on ingestion.
3. **Ephemeral Staging Egress**: Employs an OS ramdisk or temporary filesystem (`/tmp` backed by the Linux kernel page cache per [ADR-0015](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/docs/adr/0015-tmp-staging-directory-standard.md)) for output segments, immediately harvesting them into RAM (`PackagedArtifact` channels) and unlinking them from disk to eliminate write amplification.
4. **Manifest-Driven Readiness**: Eliminates partial-file read races by gating media segment availability strictly on publication within the canonical HLS playlist ([ADR-0014](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/CONTEXT.md#L182-L185)).
5. **Zero Secret Leakage**: The runtime handoff DTO (`DrmStreamMetadata`) purposefully omits raw AES keys ([ADR-0017](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/CONTEXT.md#L116-L123)), and sensitive credentials implement custom `fmt::Debug` with `[REDACTED]` guards.

---

## 2. Data Plane & GPAC Subprocess Engine

```
[ Ingest Stream (fMP4 chunks) ]
             │
             ▼
[ PackagingSession::push / SessionWriter ] ───► Inactivity Watchdog (Heartbeat)
             │
             ▼
[ RepresentationCluster::write_data ]
             ├─────────────────────────────────────────┐ (tokio::join! for Dual)
             ▼                                         ▼
   [ GpacProcess: CENC ]                     [ GpacProcess: CBCS ]
     Stdin: ChildStdin (Unix Pipe)             Stdin: ChildStdin (Unix Pipe)
     Filter: cecrypt (cfile=cenc.xml)          Filter: cecrypt (cfile=cbcs.xml)
     Filter: dasher (live.mpd + live.m3u8)     Filter: dasher (live.mpd + live.m3u8)
     Stderr: Ring Buffer (256 lines)           Stderr: Ring Buffer (256 lines)
     Supervisor: ProcessSupervisor Task        Supervisor: ProcessSupervisor Task
             │                                         │
             ▼                                         ▼
     [ Staging Output: /cenc/ ]                [ Staging Output: /cbcs/ ]
             │                                         │
             └────────────────────┬────────────────────┘
                                  ▼
                    [ ArtifactHarvester Task ]
                    - notify (Inotify/FSEvents) + 50ms Watchdog
                    - Metadata Guard (mtime/size check)
                    - ISOBMFF Box Inspector (ftyp+moov / moof+mdat)
                    - Ephemeral Unlink (fs::remove_file)
                                  │
                                  ▼
                 mpsc::Sender<PackagedArtifact>
                                  │
                                  ▼
                    [ Consumer / media-server ]
```

### 2.1. Ingestion Interfaces
- **`PackagingSession::push(&mut self, bytes)`** ([`src/session/mod.rs:583`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L583)):
  - Fast-paths raw byte ingestion. Scans initial bytes with `MOOF_FINDER` (`memmem::Finder::new(b"moof")`) to track media fragment continuity.
  - Automatically resets the inactivity watchdog timer via `ping_heartbeat()`.
- **`SessionWriter`** ([`src/session/mod.rs:966`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L966)):
  - An owned handle implementing `tokio::io::AsyncWrite`, enabling direct zero-copy piping from FFmpeg stdout or upstream TCP/Unix sockets via `tokio::io::copy`.
  - Bridges asynchronous poll-based I/O to the session via a bounded `mpsc::channel::<Bytes>(64)`.

### 2.2. GPAC Filter Graph Configuration ([`src/gpac/process.rs`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/gpac/process.rs))
The GPAC binary (version >= 2.2) is spawned with fine-tuned command-line arguments designed for low-latency live operations:
- `-logs=ncl`: Disables ANSI color escapes to ensure deterministic stderr parsing via regular expressions.
- `-no-block=all`: Disables blocking regulation between GPAC filters, preventing backpressure deadlocks on the input stdin pipe.
- `-threads=-1`: Allocates thread pools across all available CPU cores. This prevents the `cecrypt` filter from suffering thread starvation caused by blocking stdin reads, shaving up to 2000ms of cumulative packaging latency per segment.
- `seg_sync=auto`: Forces GPAC to flush the final packet of a media segment to disk before publishing the segment entry in the HLS playlist.
- `dmode=dynauto`: Produces a dynamic live manifest while receiving chunks, and automatically transitions into a static (VOD) manifest upon receiving stdin EOF.
- `template=$RepresentationID$_$Init=init$$Number$`: Standardizes predictable file naming (`video_1080p_init.mp4`, `video_1080p_1.m4s`).

### 2.3. Subprocess Supervision & Fail-Fast Resilience ([`src/gpac/process.rs:347`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/gpac/process.rs#L347))
- **`ProcessSupervisor`**: An asynchronous task monitoring `child.wait()`. If the process terminates, it drains remaining stderr output within a 200ms grace period.
- **Stderr Ring Buffer**: Maintains a 256-line circular buffer (`VecDeque`) storing raw stderr output. It parses lines using lossy UTF-8 conversion (`String::from_utf8_lossy`) to eliminate panic risks on non-UTF8 byte sequences.
- **Symmetric Fail-Fast Teardown**: When running in `Dual` mode (CENC + CBCS), if one GPAC process crashes or experiences a broken pipe, the supervisor immediately triggers `cluster.abort_peers(scheme)` ([`src/session/cluster.rs:346`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/cluster.rs#L346)), issuing `SIGKILL` to the surviving peer to prevent memory and CPU leaks.

### 2.4. Manifest-Driven Readiness & ISOBMFF Validation ([`src/session/harvester.rs`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/harvester.rs))
Media packaging engines commonly suffer from partial-file read races where downstream consumers read a segment before kernel write buffers have fully flushed. `drmpack` resolves this through a dual-gate mechanism:
1. **Canonical Readiness Signal**: Only segments referenced by the parsed HLS playlist are candidates for publication. (MPEG-DASH MPD uses a fixed `SegmentTemplate` and cannot signal physical file completion).
2. **Binary ISOBMFF Box Verification**:
   - `InitSegment`: Validates the presence of both `ftyp` and `moov` boxes, asserting `offset == data.len()`.
   - `MediaSegment`: Validates the presence of both `moof` and `mdat` boxes, asserting `offset == data.len()`.
3. **Static MPD Sanitization**: On stream closure, GPAC's wall-clock ceiling calculation may report slightly inflated `mediaPresentationDuration` values, causing players (e.g., Shaka Player) to request an out-of-range segment ($N+1$) and encounter HTTP 404 errors. `sanitize_static_mpd` adjusts duration fields to match exact segment counts.
4. **`#EXT-X-ENDLIST` Verification**: Ensures every media playlist concludes with an endlist tag, preventing players from stalling indefinitely.

---

## 3. DRM Key Management & Wire Protocols

### 3.1. Layered Key Abstraction ([`src/key/`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/key/))
- **`KeyProvider` Trait** ([`src/key/mod.rs:399`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/key/mod.rs#L399)):
  ```rust
  pub trait KeyProvider: Send + Sync {
      fn fetch_keys(&self, request: &KeyRequest) -> impl Future<Output = Result<KeySet>> + Send;
  }
  ```
  Utilizes native Rust 2021 RPITIT (Return Position Impl Trait in Trait), completely avoiding `#[async_trait]` heap allocation overhead.
- **`StaticKeySource`**: An in-memory key double for testing and offline environments.
- **`KeyPolicyEngine`** ([`src/key/policy.rs`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/key/policy.rs)):
  - `SharedAll`: All video and audio tracks share a single Content Key.
  - `SharedVideoSingleAudio`: A unified key for all video tiers, with a dedicated separate key for audio tracks.
  - `PerTierAndTrack`: Allocates distinct Content Keys for individual quality tiers (SD, HD, 4K) and audio, enabling differential subscriber entitlement tiers ([ADR-0003](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/CONTEXT.md#L124-L135)).

### 3.2. DASH-IF CPIX 2.3 Protocol Engine ([`src/cpix/`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/cpix/))
Implemented via `quick-xml` streaming pull-parser:
- **`CpixRequestBuilder`**: Deconstructs requests into concrete `ContentKey` elements (`commonEncryptionScheme="cenc"` or `"cbcs"`). Attaches `DRMSystemList` elements:
  - Widevine: `edef8ba9-79d6-4ace-a3c8-27dcd51d21ed` (with `<cpix:PSSH/>`)
  - PlayReady: `9a04f079-9840-4286-ab92-e65be0885f95` (with `<cpix:PSSH/>`)
  - FairPlay: `94ce86fb-07ff-4f43-adb8-93d2fa968ca2` (with `<cpix:URIExtXKey/>`)
  *(FairPlay is automatically excluded for CENC schemes as Apple prohibits AES-CTR)*.
- **`CpixResponseParser`**: Extracts 16-byte raw keys from `<pskc:PlainValue>`, strips ISOBMFF PSSH box headers (v0/v1) to isolate private data payloads, and extracts FairPlay `skd://` URIs.

### 3.3. AWS SPEKE v2.0 Wire Client ([`src/speke/`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/speke/))
- Transports CPIX 2.3 XML payloads over HTTP POST REST requests with `X-Speke-Version: 2.0`.
- Supports diverse authentication models (`SpekeAuth`): HTTP Basic, Bearer JWT, API Key headers (`x-api-key`), and AWS SigV4 credentials (including STS session tokens).
- Prioritizes error diagnostics from upstream response headers: `x-amzn-errortype` $\rightarrow$ `x-speke-error-message` $\rightarrow$ `x-axdrm-errormessage`.

### 3.4. Encryption Scheme Comparison: CENC vs CBCS

| Feature | CENC (`cenc`) | CBCS (`cbcs`) |
| :--- | :--- | :--- |
| **Cipher Mode** | AES-128 Counter Mode (CTR) | AES-128 Cipher Block Chaining (CBC) |
| **Pattern Encryption** | None (100% encrypted) | **Video**: 1:9 pattern (10% encrypted per Apple FairPlay)<br>**Audio**: 0:0 pattern (100% encrypted, patterns prohibited) |
| **Initialization Vector (IV)** | Dynamic / Counter | Constant 16-byte (derived deterministically from KID bytes) |
| **In-band PSSH Box** | Mandatory (Widevine, PlayReady) | **Prohibited for FairPlay** (signaled exclusively via `#EXT-X-KEY`) |
| **Primary Platforms** | Android, Chrome, Edge, Roku | Apple iOS/macOS Safari, modern Smart TVs |

---

## 4. Vendor DRM Integrations & Licensing Proxy

### 4.1. Axinom DRM Integration ([`src/vendor/axinom/`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/vendor/axinom/))
- **Mandatory Tenant Endpoints ([ADR-0018](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/docs/adr/0018-mandatory-tenant-endpoints.md))**: Disallows generic fallback URLs, enforcing tenant-isolated endpoints to eliminate staging/production cross-talk.
- **Concurrent Dual-Scheme Resolution**: In `Dual` mode, `AxinomProvider` issues two parallel SPEKE v2 requests via `tokio::try_join!` and merges the results into a unified `KeySet`.
- **JWT Entitlement Token Generation (`generate_axinom_jwt`)**: Mints HMAC-SHA256 (`HS256`) tokens using `ring::hmac`, embedding KIDs and standard FairPlay CBCS IVs (`*kid.as_bytes()`).

### 4.2. Playback Authorization Metadata: `DrmStreamMetadata` ([ADR-0017](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/CONTEXT.md#L116-L123))
- Invoking `session.playback_metadata()` yields a serializable `DrmStreamMetadata` DTO.
- Contains KIDs, IVs, schemes, and track mappings for persistence in PostgreSQL/Redis.
- **Safety Invariant**: Excludes raw AES keys to ensure zero cryptographic secret leakage to application layers.

### 4.3. In-Process Licensing Proxy Service ([`src/license/`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/license/))
Proxies CDM challenge payloads from client web players to upstream DRM license servers:
- **Connection Pooling ([ADR-0009](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/docs/adr/0009-license-proxy-caching-and-forwarding.md))**: Reuses a single `reqwest::Client` instance across requests, eliminating recurring TLS handshake latency.
- **Strict Caching Invariants**:
  - **License Responses Are Never Cached**: Client CDM challenges contain ephemeral session keys and single-use nonces; caching license payloads breaks cryptographic integrity on subsequent playbacks.
  - **FairPlay Application Certificates Are Cached**: Caches static Apple `.cer`/`.der` certificates in an in-memory `Arc<Mutex<HashMap>>`. Holds the lock across cold network fetches to **prevent thundering herd effects** when live broadcasts begin.

---

## 5. End-to-End Examples Reference

The repository provides 9 self-contained examples illustrating progression from local prototyping to full production delivery:

| Example | Required Features | Scenario & Implementation |
| :--- | :--- | :--- |
| **`01_basic_live_cenc`** | Base | Offline live CENC + Widevine pipeline using `StaticKeySource` and FFmpeg `testsrc`. |
| **`02_low_latency_dual_cmaf`** | Base | Concurrent packaging of CENC (DASH) and CBCS (HLS) with low-latency CMAF chunking (0.2s chunk, 2.0s segment). |
| **`03_multitrack_abr_tiers`** | Base | Multi-track ABR (4K, HD, SD, Audio) using `PerTierAndTrack` key policies, with cleartext WebVTT subtitle bypass. |
| **`04_axinom_key_provider`** | `axinom` | Direct integration with Axinom Key Service over SPEKE v2 to fetch live keys. |
| **`05_license_proxy_service`** | `license-proxy` | In-process proxy handling Widevine license challenges and injecting `X-AxDRM-Message` headers. |
| **`06_e2e_live_clearkey`** | `license-proxy` | End-to-end ClearKey DRM pipeline designed for headless CI/CD execution without cloud credentials. |
| **`07_e2e_live_axinom`** | `axinom`, `license-proxy` | Full production flow: Axinom Key Service $\rightarrow$ Packaging Session $\rightarrow$ Channel Receiver $\rightarrow$ CDN Publisher. |
| **`08_in_memory_live_stream`** | `axinom` | Pipes raw FFmpeg stdout directly to `session.writer()` (`AsyncWrite`), logging turnaround and dwell latency metrics. |
| **`09_axum_playback_server`** | `axinom`, `license-proxy` | Production-grade web server using **Axum**: user authentication, one-line Axinom JWT minting, license proxy endpoints, and an embedded Shaka Player web client with automatic DRM scheme detection. |

---

## 6. Ubiquitous Language & Invariants Dictionary

Extracted from [`CONTEXT.md`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/CONTEXT.md):

| Canonical Term | Terms to AVOID | Invariant & Architectural Meaning |
| :--- | :--- | :--- |
| **`PackagingSession`** | Job, task, pipeline, worker | Central orchestrator controlling session lifecycle, key acquisition, GPAC subprocesses, and output channels. |
| **`SessionWriter`** | Pipe wrapper, write adapter | Owned handle implementing `tokio::io::AsyncWrite` bridging asynchronous poll I/O to internal ingestion channels. |
| **`ProcessSupervisor`** | Process monitor, child watcher | Dedicated asynchronous task monitoring GPAC exit statuses and capturing real-time stderr logs. |
| **`EncryptionScheme`** | Cipher mode, scheme | `CBCS` (default convergence baseline), `CENC`, or `Dual` (parallel dual-engine orchestration). |
| **`QualityTier`** | Quality level, tier | Key allocation group (`sd`, `hd`, `uhd_4k`, `audio`). |
| **`DrmStreamMetadata`** | Key manifest | Playback authorization DTO; **Invariant**: strictly excludes raw AES encryption keys. |
| **`LicenseProxy`** | License server | Gateway forwarding client challenges; **Invariant**: never caches licenses; caches FairPlay certificates in RAM. |
| **`Manifest-Driven Readiness`** | File polling, stability check | **Invariant**: Media segments are published only when verified complete and referenced in the HLS manifest. |
| **`Semantic Naming Standard`** | `stdin_dash`, `live_1.m3u8` | Strict naming: `live.m3u8`, `live.mpd`, `video_{Height}p.m3u8`, `video_{Height}p_{Number}.m4s`. |
