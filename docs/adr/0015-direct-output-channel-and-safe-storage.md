# Direct Output Channel and Safe Storage Staging

**Status: Accepted (Supersedes ADR-0005 default storage target)**

## Context

Callers previously had to manually poll or watch the packaging output directory to discover and serve newly produced media segments and playlists. Furthermore, directing all output to `/dev/shm` by default (ADR-0005) introduced severe operational hazards in containerized production environments:
1. Docker and Kubernetes CRI-O defaults allocate only 64MB to `/dev/shm`, causing catastrophic `ENOSPC` crashes within 28–56 seconds on multi-bitrate streams.
2. A 30-minute default Time-Shift Buffer (`tsb=1800`) accumulates up to 4.1GB of unpruned media segments in RAM per dual-scheme stream.
3. Linux cgroups count `tmpfs` directly toward container memory limits; without swap, memory pressure triggers the kernel OOM Killer (`SIGKILL`, exit code 137).
4. Process termination via `SIGKILL` bypasses Rust `Drop`, leaving orphaned files locked in host RAM indefinitely.

## Decision

1. **Direct Output Channel (`PackagedArtifact`)**: `PackagingSession` provides an asynchronous channel receiver via `session.take_output_receiver()`. Each emitted artifact encapsulates the raw binary payload (`bytes::Bytes`), relative filename, classification (`ArtifactKind::InitSegment`, `MediaSegment`, or `Manifest`), and concrete `EncryptionScheme`.
2. **Manifest-Driven Readiness**: Segments staged by GPAC are verified against manifest emission and sequence progression before dispatching to the channel, guaranteeing that callers never receive partially written files.
3. **Safe Disk-Backed Staging by Default**: The default output directory shifts from `/dev/shm` to standard temporary disk storage (`/tmp` / OS tempdir) backed by local NVMe SSD. This eliminates kernel OOM risks while preserving sub-millisecond I/O through the Linux kernel Page Cache. Ramdisk (`/dev/shm`) remains available as an explicit opt-in configuration for environments with verified memory headroom.
4. **Buffer Window Tuning**: Default `tsb` is reduced from 1800s to 60s (with a `.with_time_shift_buffer(Duration)` configuration builder), cutting peak storage accumulation by over 95%.

## Considered Options

- **Raw POSIX anonymous pipe output (`dasher -o pipe://` or `stdout`)**: Rejected because HLS/DASH is a multi-resource document tree (manifests, init segments, rolling media fragments across tracks). A single pipe cannot demux interleaved files without a framing protocol; GPAC explicitly discards manifests when forced to a pipe.
- **In-process HTTP Push (`httpout:hmode=push` / CMAF Ingest)**: Rejected per Ponytail minimalism. Requires hosting an in-process HTTP server, managing ephemeral TCP loopback ports, and exposes the system to upstream GPAC HTTP client bugs (memory leaks on long live runs in issue #2923 and multi-PID failures in issue #3027) while offering zero performance advantage over Linux kernel Page Cache.
- **Direct C API FFI filter (`libgpac`)**: Rejected because GPAC's filter engine explicitly prohibits application-defined custom filters from acting as destination sinks for dynamic graph loaders like `dasher`. It also eliminates process crash isolation.

## Consequences

- Callers consume live encrypted media segments and playlists directly in-memory via `while let Some(artifact) = rx.recv().await`, eliminating external directory polling or watcher daemons.
- Applications deployed to standard Docker containers and Kubernetes Pods run reliably out-of-the-box without container crashes from 64MB `/dev/shm` limits.
- The channel is zero-cost: if `take_output_receiver()` is not invoked, background harvesting is disabled.
- Ephemeral staging files are deleted immediately upon channel dispatch, preventing storage build-up.
