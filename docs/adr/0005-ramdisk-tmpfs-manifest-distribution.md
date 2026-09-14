# Ramdisk tmpfs for manifest and chunk distribution

**Status: Superseded by ADR-0015 (regarding default storage target and output distribution)**

drmpack directs GPAC output (manifests `.m3u8` / `.mpd` and CMAF partial segments) to a shared memory directory (Ramdisk / `/dev/shm` on Linux or `tmpfs`) while streaming media input into GPAC via anonymous Unix pipes (`stdin`). This eliminates disk I/O bottlenecks on sub-second manifest update loops and provides zero-overhead delivery to HTTP/CDN edge servers.

## Considered options

- **Physical disk storage (SSD/NVMe)**: Simple, but introduces write amplification, disk wear, and I/O latency spikes on continuous 200ms manifest update cycles.
- **Pure Unix pipe/socket output**: Streams bytes directly, but complicates HLS/DASH delivery since video players fetch manifests and segments via discrete HTTP range/file requests.

## Consequences

- The host environment must mount `/dev/shm` or `tmpfs` with sufficient size for the sliding window buffer.
- `PackagingSession` cleans up session directories in Ramdisk upon graceful close or timeout teardown.

## Reason for Obsolescence & Implementation Reality

The default use of `/dev/shm` (Ramdisk/tmpfs) and direct filesystem-based distribution was **superseded by [ADR-0015](./0015-direct-output-channel-and-safe-storage.md)** due to severe operational hazards in containerized cloud environments:
- **64MB Docker / Kubernetes CRI-O Limits**: Default container runtimes allocate only 64MB to `/dev/shm`. On multi-bitrate and multi-scheme live streams, this small quota was exhausted within 30–60 seconds, triggering fatal `ENOSPC` errors.
- **Linux Kernel OOM Killer (`SIGKILL 137`)**: In Linux cgroups without swap, `tmpfs` allocations count directly against the container memory ceiling. Memory pressure caused the kernel OOM killer to immediately terminate the process (`SIGKILL 137`), bypassing Rust's `Drop` handlers and leaking orphaned segment files in host RAM.
- **Current Production Default (`src/session/mod.rs:58`)**:
  - `PackagingSessionConfig::new` defaults `output_dir` to `default_output_dir` using `std::env::temp_dir()` (`/tmp`), backed by local NVMe/SSD storage and accelerated sub-millisecond by the Linux kernel Page Cache.
  - Direct output to Ramdisk (`/dev/shm`) remains supported solely as an explicit opt-in configuration (`with_output_dir`) for hosts with verified memory allocation.
- **In-Memory Channel Distribution**: Output distribution via shared filesystem paths was replaced by `PackagingSession::take_output_receiver()`, an in-memory asynchronous channel dispatching `PackagedArtifact` items (`bytes::Bytes`) directly to caller runtimes. Ephemeral staging files are unlinked immediately after channel dispatch, preventing unbounded disk growth.
