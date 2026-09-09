# Specification: Production-Ready Hybrid ArtifactHarvester

**Status:** Ready for Implementation  
**Triage Label:** `ready-for-agent`  
**ADR References:** ADR-0015, ADR-0016  
**Vocabulary Reference:** CONTEXT.md (`ArtifactHarvester`, `PackagedArtifact`, `ArtifactKind`, `Storage Staging Directory`, `Manifest-Driven Readiness`, `Ephemeral Staging`, `HarvesterCadence`, `Metadata Guarding`)

---

## Problem Statement

As a media platform operator and library consumer integrating `drmpack` into a production media server (such as `ermis-media-server`), I observe that the current segment and manifest harvesting mechanism relies on a fixed periodic polling timer (hardcoded at 200ms) combined with an uncontrolled `harvester.ping()` invocation on every incoming media frame write in `push_data()`.

This implementation exhibits severe production flaws:
1. **CPU & Syscall Thrashing**: Under continuous live ingest (30–60 fps), calling `ping()` on every raw chunk triggers 30–60 immediate directory traversals, filesystem stat calls, and manifest file reads per second per stream. When serving tens or hundreds of concurrent packaging streams on a single node, this causes thousands of redundant syscalls per second, pinning CPU cores and generating substantial memory allocation churn.
2. **Sub-optimal Latency Trade-off**: A fixed 200ms interval is an awkward compromise heuristic. For standard HLS (2s–6s segments), 90% of polling checks find no new segments and waste I/O. For Low-Latency HLS (LL-HLS) with 200ms–333ms chunks, a 200ms polling jitter delays segment discovery by up to 60%–100% of the chunk's entire lifespan, risking player buffer underruns and playback stalls.
3. **Redundant Manifest I/O**: On every polling cycle, the harvester reads and decodes the entire manifest file from disk even when its content has not changed, continuously allocating heap memory and parsing strings.

I want `drmpack` to provide a true production-grade harvesting architecture that detects finalized media segments with sub-millisecond latency (<1ms) via OS kernel filesystem events, eliminates wasted idle CPU usage, prevents input-push ping storms, caches manifest metadata to avoid redundant disk reads, and maintains a resilient watchdog safety net to guard against dropped kernel events.

---

## Solution

1. **Kernel Event-Driven Fast-Path**: Integrate native OS filesystem event notifications via the `notify` crate (v8, default features — automatically selects `inotify` on Linux, `FSEvents` on macOS, `ReadDirectoryChangesW` on Windows). Any filesystem event on the staging directory triggers the existing `harvest_target()` reconciliation function. Events serve exclusively as wake-up triggers — the harvester does not process individual file events directly.
2. **Non-blocking Tokio Async Bridge**: Bridge synchronous OS file notifications to the Tokio asynchronous event loop using an unbounded MPSC channel (`tokio::sync::mpsc::unbounded_channel()`), ensuring the OS file watcher thread never blocks and preventing kernel event queue overflow (`IN_Q_OVERFLOW`).
3. **Decoupled Ingest Pipeline**: Completely eliminate `harvester.ping()` invocations from `push_data()`. The input media feeding rate is entirely decoupled from segment packaging completion.
4. **Manifest Metadata Guarding**: Stat manifest files (`tokio::fs::metadata`) before reading. Only read bytes and allocate strings when `modified()` timestamp or file `len()` has changed, reducing manifest I/O overhead to a single stat check per cycle.
5. **Relaxed Watchdog Safety Net**: Replace the aggressive 200ms polling timer with a relaxed 1500ms watchdog heartbeat. The watchdog serves exclusively as a fallback reconciliation pass for edge-case GPAC buffering or dropped kernel events, consuming near-zero CPU while guaranteeing zero orphaned segments.
6. **Ephemeral Page Cache Ingestion**: Retain immediate unlinking (`tokio::fs::remove_file`) upon artifact dispatch. Staging files are ingested while still in the OS Page Cache and deleted before physical disk sync occurs, preserving zero disk wear and zero physical I/O overhead.

---

## User Stories

1. As a media server engineer, I want segment completion to be detected immediately via kernel file events when GPAC closes the output file, so that live segments are emitted to downstream CDN origins with sub-millisecond packaging latency.
2. As a platform operator running high-density packaging instances, I want the harvester to consume zero CPU and zero syscalls while waiting for segments to complete, so that server CPU utilization scales efficiently with stream count.
3. As a media ingest developer, I want `session.push(...)` to simply forward media samples without triggering immediate staging directory scans, so that high-frequency frame ingestion does not thrash the harvester.
4. As a DevOps engineer running containerized workloads, I want the file watcher to use an unbounded Tokio channel, so that sudden write bursts never block the kernel event thread or cause `IN_Q_OVERFLOW` event drops.
5. As a CDN origin server, I want manifest updates to be emitted only when manifest content has actually changed, so that redundant manifest cache invalidations and CDN re-fetches are eliminated.
6. As a system architect, I want manifest files to be checked for size and modification timestamp before reading bytes into memory, so that unnecessary memory allocations and string parsing are eliminated when files remain static.
7. As a broadcaster deploying Low-Latency HLS (LL-HLS), I want segment and chunk availability signals to incur less than 5ms of internal packager overhead, so that end-to-end glass-to-glass latency remains below 2 seconds.
8. As a reliability engineer, I want a periodic watchdog heartbeat (1500ms) to run in the background, so that any segment or manifest missed by a dropped or delayed OS event is reliably harvested.
9. As an operations engineer, I want all intermediate staging files to be unlinked immediately upon emission, so that the staging directory footprint remains near zero and files do not accumulate on disk.
10. As a developer running on macOS Darwin, I want the hybrid harvester to function reliably across Darwin file event backends (`FSEvents`/`kqueue`) and Linux (`inotify`), so that local developer machines and CI pipelines behave identically to production servers.
11. As a downstream consumer of `PackagingSession`, I want the public API (`session.take_output_receiver()`) and the emitted `PackagedArtifact` data types to remain completely unchanged, so that my existing integration code requires zero breaking refactors.
12. As a quality assurance engineer, I want automated end-to-end integration tests to verify that multi-rendition live sessions emit complete, uncorrupted segments and manifests under the hybrid harvester without dropped artifacts or hangs.
13. As a library consumer, I want the harvester to silently fall back to watchdog-only mode if kernel file event registration fails (e.g. inotify watch limit exhausted), so that packaging never crashes due to OS resource limits.
14. As a library consumer, I want the harvester to handle staging directories that do not yet exist at session start (GPAC creates them on first data), so that watcher registration adapts lazily without errors.

---

## Implementation Decisions

### 1. Hybrid Harvester Architecture (Trigger + Reconcile + Guard)
The harvester operates on a Triad model with a critical invariant: **events are triggers only, not direct file handlers**.
- **Event-Driven Trigger Path**: Uses `notify::RecommendedWatcher` configured with non-recursive directory watches. When any filesystem event fires on the staging directory, the harvester calls the existing `harvest_target()` function — the same function used by the watchdog timer. The harvester does NOT inspect or process individual event types (`IN_CLOSE_WRITE`, `Create`, etc.) because:
  1. **Manifest-Driven Readiness must be preserved**: A media segment is only emitted after GPAC has flushed it AND referenced it in the HLS manifest. Processing a segment file event in isolation would bypass this invariant and risk emitting segments before they are declared playable.
  2. **Cross-platform portability**: macOS `FSEvents` does not provide `IN_CLOSE_WRITE` equivalents, so distinguishing event types would require platform-specific branches. Treating all events as generic wake-ups eliminates this complexity.
- **Watchdog Pull Path**: A relaxed 1500ms ticker (`tokio::time::interval`) that invokes a full directory reconciliation sweep to catch any anomalies.
- **Guard Layer**:
  - `HarvesterState` maintains an in-memory deduplication set (`emitted_segments`) with bounded FIFO pruning (`MAX_EMITTED_HISTORY = 5000`).
  - `ManifestMeta` stores `mtime`, `size`, and cached `Bytes` per manifest. A manifest is read from disk only if its filesystem metadata indicates a change.

### 2. Tokio Runtime Bridge
- Synchronization between the OS watcher thread and Tokio runtime uses `tokio::sync::mpsc::unbounded_channel`.
- The synchronous event handler in `RecommendedWatcher` only executes `tx.send(res)`. It performs zero locks, zero file I/O, and zero blocking operations.

### 3. Graceful Degradation (No Breaking API Changes)
- `Harvester::spawn()` retains its existing `-> Self` return type. It does NOT return `Result`.
- If `RecommendedWatcher::new()` or `watcher.watch()` fails (e.g. inotify watch limit exhausted in dense container environments), the harvester logs a `tracing::warn!` and falls back to watchdog-only mode (1500ms polling). Packaging continues without interruption.
- All downstream call sites (`take_output_receiver()`, `close()`, `Drop`) remain unchanged.

### 4. Lazy Watch Registration
- GPAC creates the staging directory only after receiving its first input data via stdin. At the time `take_output_receiver()` is called, the staging directory may not yet exist.
- The harvester tracks which directories have been registered with the watcher via `watched_dirs: HashSet<PathBuf>` in `HarvesterState`.
- On each `harvest_target()` invocation, if `dir.exists() && !state.watched_dirs.contains(dir)`, the watcher registers the directory and records it. This deferred registration avoids `NotFound` errors on session startup.

### 5. Decoupled Ingest Pipeline
- The `push_data()` method in `PackagingSession` ceases calling `harvester.ping()`.
- The `ping()` mechanism is preserved on the `Harvester` handle exclusively for explicit lifecycle synchronization (such as during final teardown and flush in `session.close()`).

### 6. Ephemeral Staging & Integrity Validation
- Media segments undergo ISOBMFF header validation (`is_complete_isobmff_media_segment` for `.m4s` and `is_complete_isobmff_init_segment` for `init.mp4`) prior to emission.
- Once validated, files are read into `Bytes` and immediately unlinked via `tokio::fs::remove_file`.

---

## Testing Decisions

### 1. Testing Philosophy
- Tests must verify external behavioral contracts, not internal polling mechanics:
  - Artifacts must arrive in correct sequential order (`InitSegment` → `MediaSegment` 1, 2, 3... → `Manifest` updates).
  - Emitted segments must be valid ISOBMFF fMP4 containers containing complete `moof` + `mdat` boxes.
  - Manifests must contain updated `#EXTINF` entries referencing emitted segments.
  - No segment is emitted before it appears in a manifest (Manifest-Driven Readiness invariant).
  - No orphaned staging files must remain after session teardown.

### 2. Seam Selection
- **Primary Seam**: The highest existing public interface seam:
  - `PackagingSession::create(config, &provider)`
  - `session.take_output_receiver()`
  - `session.push(bytes)`
  - `session.close()`
- No new mock hooks or internal test seams are introduced.

### 3. Prior Art & Test Suites
- `tests/direct_output_channel_e2e.rs`: Exercises single-scheme and dual-scheme (CENC + CBCS) live streaming sessions through the direct output channel.
- `tests/multi_track_abr_e2e.rs`: Exercises multi-track ABR packaging and verification.
- `examples/08_in_memory_live_stream.rs`: Interactive demonstration verifying live real-time emission with embedded HTTP server.

---

## Out of Scope

1. **In-Memory Pipe Multiplexing (Eliminating Staging Directory Entirely)**: Directly piping GPAC segment output over Unix anonymous pipes without staging files on the filesystem is documented in ADR-0015 as a future investigation. This specification focuses on optimizing the filesystem harvester for production staging.
2. **LL-HLS Partial Segment Chunks (`_HLS_part`)**: Full implementation of byte-range partial segment generation and blocking playlist reload handlers (`_HLS_msn`) belongs to a dedicated low-latency feature track.
3. **Dynamic Transcoding / Encoding**: Encoding media streams into ABR ladders is upstream responsibility (e.g. FFmpeg); `drmpack` remains strictly an orchestration and DRM packaging engine.
4. **Per-file event type handling**: The harvester treats all filesystem events as generic wake-up triggers. Implementing event-type-specific fast paths (e.g. `IN_CLOSE_WRITE` → direct segment harvest bypassing manifest check) is a potential future optimization but is explicitly out of scope to preserve the Manifest-Driven Readiness safety invariant.

---

## Further Notes

- Adding `notify = "8"` with default features introduces zero C-library dependencies on Linux and macOS, compiling cleanly against standard system APIs (`libc` / `CoreServices`). Default features auto-select `inotify` backend on Linux and `macos_fsevent` on macOS — no platform-specific feature flags needed.
- On Linux deployments using Docker/Kubernetes, the host system's `/proc/sys/fs/inotify/max_user_watches` should remain above default levels (typically 65,536), which is standard across all cloud container environments.
- If the watcher fails to initialize or register directories, the harvester degrades gracefully to 1500ms polling with zero API breakage. This guarantees packaging availability even in constrained container environments.
