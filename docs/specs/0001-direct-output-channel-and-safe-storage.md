# Specification: Direct Output Channel and Safe Staging

**Status:** Ready for Implementation  
**Triage Label:** `ready-for-agent`  
**ADR References:** ADR-0015 (Supersedes default of ADR-0005)  
**Vocabulary Reference:** CONTEXT.md (`PackagedArtifact`, `ArtifactKind`, `Storage Staging Directory`, `Manifest-Driven Readiness`, `Ephemeral Staging`)

---

## Problem Statement

As a developer integrating `drmpack` into a media server or CDN publisher, I currently have to manually monitor and scan the filesystem output directory to discover when new video segments or updated manifests are written. This requires writing custom file watchers, polling loops, or synchronizers that introduce race conditions (such as reading partially flushed segments) and considerable boilerplate code.

Furthermore, because the library historically directed all packaging output to Ramdisk (`/dev/shm`) by default with an unbounded 30-minute sliding window (`tsb=1800`), my streaming services risk catastrophic failures in containerized production environments:
1. Docker and Kubernetes containers crash with `ENOSPC` (No space left on device) within 30–60 seconds due to the default 64MB `/dev/shm` allocation.
2. In-memory segment accumulation reaches over 4GB per stream under dual encryption (CENC + CBCS), triggering the Linux kernel Out-Of-Memory (OOM) Killer.
3. Unexpected process termination (`SIGKILL`) bypasses cleanup logic, leaving orphan files permanently leaking host RAM.

I want `drmpack` to emit packaged artifacts directly to me in memory through an idiomatic async channel ("data in -> encrypted data out"), while keeping intermediate staging safely isolated and ephemeral without manual directory management.

---

## Solution

1. Provide an asynchronous output channel directly from `PackagingSession` that emits structured `PackagedArtifact` objects (carrying the in-memory binary payload as `Bytes`, filename, artifact classification, and encryption scheme).
2. Employ a Manifest-Driven readiness synchronization mechanism: segments are only harvested and emitted once GPAC has finalized the segment and referenced it in the manifest, eliminating partial-file reads.
3. Replace the default `/dev/shm` destination with a safe disk-backed Storage Staging Directory (`std::env::temp_dir()`), leveraging the Linux Page Cache for sub-millisecond RAM write/read speeds while providing an automatic eviction buffer that eliminates kernel OOM crashes. Ramdisk remains available as an explicit opt-in setting.
4. Implement Ephemeral Staging: intermediate files in the staging directory are immediately unlinked upon ingestion into memory, preventing storage accumulation.
5. Reduce the default Time-Shift Buffer depth (`tsb`) from 1800 seconds to 60 seconds, with an explicit configuration builder for custom buffer windows.

---

## User Stories

1. As a media server developer, I want to call a method to retrieve an async channel receiver from my packaging session, so that I can process packaged data directly in code without watching the filesystem.
2. As a media server developer, I want each emitted artifact to include raw binary payload bytes, so that I can immediately stream or upload it to object storage (e.g. S3) or CDN origins without performing extra disk reads.
3. As a media server developer, I want each emitted artifact to specify its relative filename (e.g. `video_1080p_1.m4s`, `live.m3u8`), so that I know where to store or route the resource on the distribution plane.
4. As a media server developer, I want each emitted artifact to indicate its artifact kind (`InitSegment`, `MediaSegment`, or `Manifest`), so that I can apply distinct caching and forwarding headers (e.g. immutable caching for media segments vs short TTLs for dynamic manifests).
5. As a media server developer, I want each emitted artifact to identify its concrete encryption scheme (`Cenc` or `Cbcs`), so that I can route payloads to scheme-specific storage paths in dual-scheme streaming sessions.
6. As a media server developer, I want the channel receiver method to be callable only once per session, so that single ownership of the stream is preserved and duplicate consumer conflicts are caught immediately.
7. As a media server developer, I want the session to incur zero CPU and memory overhead if I choose not to take the output receiver, so that legacy directory-serving workflows suffer no regression.
8. As a CDN publisher, I want segments to be emitted only after they are completely flushed and valid, so that players never download truncated or corrupted video chunks.
9. As a DevOps engineer deploying to Docker containers, I want the library to use standard temporary storage by default instead of `/dev/shm`, so that my containers do not crash from Docker's default 64MB shared memory quota.
10. As a DevOps engineer deploying to Kubernetes without swap, I want staging writes to be backed by filesystem page cache rather than pinned tmpfs RAM, so that memory spikes do not trigger kernel OOM Killer termination (`SIGKILL`, exit code 137).
11. As a platform engineer, I want the default time-shift buffer to be 60 seconds rather than 30 minutes, so that idle staging space is constrained to less than 100MB per multi-bitrate stream.
12. As an advanced user with verified RAM headroom, I want to explicitly configure a custom output directory (including `/dev/shm`), so that I can run pure Ramdisk packaging when my infrastructure allows it.
13. As an advanced user, I want to configure the time-shift buffer duration via the session builder, so that I can customize the sliding DVR window for specific broadcast needs.
14. As an operator, I want staged files to be cleaned up immediately upon channel dispatch, so that disk usage remains near zero even during multi-hour live broadcasts.
15. As a developer running on macOS, I want the exact same session creation and output channel APIs to work identically on Darwin as on Linux, so that my local development and CI pipelines run without platform-specific workarounds.

---

## Implementation Decisions

### 1. Data Contract
The data emitted over the output channel is defined by a structured container and a discrete classification enum:

```rust
// Contract:
pub enum ArtifactKind {
    InitSegment,
    MediaSegment,
    Manifest,
}

pub struct PackagedArtifact {
    pub filename: String,
    pub data: bytes::Bytes,
    pub kind: ArtifactKind,
    pub scheme: EncryptionScheme,
}
```

### 2. Session API Shape
- A method on the core session controller:
  `pub fn take_output_receiver(&mut self) -> Option<mpsc::Receiver<PackagedArtifact>>`
- Returns `Some(receiver)` on first invocation; returns `None` on all subsequent invocations.
- The session configuration builder gains:
  `pub fn with_time_shift_buffer(mut self, buffer: Duration) -> Self`
- The default storage staging path function selects `std::env::temp_dir()` by default, discarding the previous hardcoded preference for `/dev/shm`.

### 3. Readiness and Harvester Engine
- When `take_output_receiver()` is claimed, an internal asynchronous background task is spawned.
- The task monitors the session staging directory:
  - Detects manifest changes (`live.mpd`, `live.m3u8`).
  - Identifies newly referenced segment files.
  - Reads segment contents into immutable `bytes::Bytes`.
  - Dispatches the `PackagedArtifact` to the channel sender.
  - Immediately unlinks the consumed segment file from the staging directory.
- If the receiver is dropped or not claimed, the background task terminates or never activates (zero-cost abstraction).

### 4. Process Command Arguments
- GPAC process spawning logic dynamically formats the `tsb` parameter from the configured time-shift buffer duration (in whole seconds), replacing the hardcoded `tsb=1800`.

---

## Testing Decisions

### What Makes a Good Test
- Tests must verify external observable behavior (public API contract and channel emission), never internal polling intervals or private implementation fields.
- Tests must confirm that byte payloads emitted to the channel match the expected data format (non-empty fMP4 headers for init segments, valid media boxes for segments, valid XML/M3U8 text for manifests).
- Tests must verify lifecycle guarantees: single-claim semantics, drop cleanup, and zero residual files after ephemeral unlinking.

### Highest Testing Seam
The sole testing seam is the **public `PackagingSession` API**:
- **Input Seam:** `PackagingSession::push()` / `PackagingSession::ingest_stream()`.
- **Output Seam:** `session.take_output_receiver()` channel receiver.
No internal module mocks or low-level hooks are required.

### Prior Art
- `src/session/mod.rs` (`test_run_to_completion`, `test_ingest_stream_and_run_to_completion`): Ingests bytes through channel and verifies session completion.
- `src/session/mod.rs` (`test_presets_and_builder_methods`): Validates configuration builder defaults and mutations.

---

## Out of Scope

- **In-process HTTP server push (`httpout:hmode=push` / CMAF Ingest)**: Explicitly rejected per ADR-0015 and Ponytail minimalism due to port management overhead and GPAC client bugs.
- **Direct C API FFI filter (`libgpac`)**: Prohibited by GPAC architecture for dynamic graph segmenters.
- **Automatic S3 / CDN uploading inside `drmpack`**: Uploading and distribution are the caller's responsibility. `drmpack` provides the in-memory artifacts and lets the caller distribute them.
- **VOD file-to-file batch packaging**: Static file-in to static file-out packaging remains a distinct future capability decoupled from the live streaming session.

---

## Further Notes

- Cross-platform compatibility: The implementation relies exclusively on standard Rust libraries (`tokio::sync::mpsc`, `tokio::fs`, `bytes::Bytes`), ensuring 100% parity across Linux and macOS.
- Ponytail compliance: The implementation adds fewer than 150 lines of total diff across the codebase, avoiding any new crate dependencies.
