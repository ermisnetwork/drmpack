# Ramdisk tmpfs for manifest and chunk distribution

**Status: Accepted**

drmpack directs GPAC output (manifests `.m3u8` / `.mpd` and CMAF partial segments) to a shared memory directory (Ramdisk / `/dev/shm` on Linux or `tmpfs`) while streaming media input into GPAC via anonymous Unix pipes (`stdin`). This eliminates disk I/O bottlenecks on sub-second manifest update loops and provides zero-overhead delivery to HTTP/CDN edge servers.

## Considered options

- **Physical disk storage (SSD/NVMe)**: Simple, but introduces write amplification, disk wear, and I/O latency spikes on continuous 200ms manifest update cycles.
- **Pure Unix pipe/socket output**: Streams bytes directly, but complicates HLS/DASH delivery since video players fetch manifests and segments via discrete HTTP range/file requests.

## Consequences

- The host environment must mount `/dev/shm` or `tmpfs` with sufficient size for the sliding window buffer.
- `PackagingSession` cleans up session directories in Ramdisk upon graceful close or timeout teardown.
