# Asynchronous ProcessSupervisor and Fail-Fast Lifecycle

**Status: Accepted (Amended by ADR-0012 and ADR-0015)**

`drmpack` implements an asynchronous `ProcessSupervisor` per GPAC subprocess, combining immediate crash detection via dedicated `child.wait()` tasks, clean plaintext stderr severity parsing via `-logs=ncl`, symmetric teardown for Dual representations, and strict `#EXT-X-ENDLIST` verification on session close.

## Context & Problem Statement

Packaging fMP4 live media through external GPAC subprocesses over kernel pipes introduces asynchronous lifecycle risks:
1. **Idle Crash Blindness**: If GPAC crashes during inter-segment intervals (e.g. between live GOP segments), synchronous status checks inside `push_segment()` only discover the failure on the subsequent write. If upstream video pauses, the crash goes undetected until inactivity watchdog expiry.
2. **Poll vs Single-Waiter Contention**: In Tokio, `Child::wait()` requires a mutable reference or ownership. Polling via `try_wait()` wastes CPU and adds detection latency, while multiple tasks polling `Child` causes lock contention.
3. **Stderr Noise & Color Corruption**: GPAC emits ANSI color escape sequences by default, cluttering machine-parsed logs and obscuring root causes. Log severity levels (`Warning`, `Error`) are emitted on stderr without structured tagging.
4. **HLS Playback Stall Risk**: If an HLS master session terminates without appending `#EXT-X-ENDLIST`, downstream players and CDN edge caches treat the stream as stalled and retry indefinitely.

## Considered Options

- **Option 1: Polling timer (`try_wait()` loop)**: Periodic background tick checking child status. Rejected due to unnecessary CPU overhead and delayed crash detection.
- **Option 2: Asymmetric partial degradation in Dual mode**: If CBCS crashes, keep CENC running. Rejected per ADR-0006 and Ticket 04; a Dual session is an atomic unit of work.
- **Option 3: Dedicated `ProcessSupervisor` task with single-waiter ownership**: Accepted. A single task per GPAC process awaits `child.wait()`, broadcasts status changes, and coordinates immediate symmetric teardown.

## Architecture & Decisions

1. **Dedicated `ProcessSupervisor` Task**:
   - Exactly one background task per GPAC process owns `child.wait()`.
   - Unexpected process exit immediately transitions `Lifecycle` to `Failed`, triggers `CancellationToken`, and aborts pending writes.
   - Graceful termination closes `ChildStdin` and awaits the supervisor task's exit status with a configurable finalization timeout.
2. **Plaintext Stderr Severity Parsing**:
   - GPAC is invoked with `-logs=ncl` to strip ANSI escape codes.
   - Stderr stream reader classifies lines into `tracing::error!`, `tracing::warn!`, `tracing::info!`, or `tracing::debug!`.
   - A bounded ring buffer of recent stderr lines is preserved for diagnostic inclusion in `DrmpackError::ProcessCrashed`.
3. **Symmetric Teardown for Dual Mode**:
   - If either the CENC or CBCS representation encounters an unexpected process termination, the cluster immediately terminates the peer process and surfaces a unified `PackagingSessionFailure`.
4. **Manifest Finalization Verification**:
   - For streams with media segments pushed, `session.close()` verifies the presence of `#EXT-X-ENDLIST` in the generated `.m3u8` manifest before declaring finalization successful. Storage cleanup is intentionally decoupled from finalization.

## Consequences

- Media-server receives immediate fail-fast error notifications when GPAC subprocesses terminate unexpectedly.
- Zero CPU spent on polling timers.
- Log output from GPAC is clean, uncolored, and properly tiered across `tracing` log levels.
- HLS clients are protected against manifest stall conditions.

## Implementation Reality / Amendments

1. **Decoupled Session Finalization and Storage Cleanup (ADR-0012)**: The original lifecycle design in Decision 4 linked manifest finalization directly to storage cleanup. ADR-0012 formally decoupled these concerns: `session.close()` finalizes GPAC subprocesses, flushes pipes, and verifies `#EXT-X-ENDLIST`, leaving all media segments and manifests intact on disk for downstream delivery. Storage reclamation is handled separately via explicit `session.cleanup()` or through RAII `Drop` guards.
2. **Safe Disk-Backed Staging by Default (ADR-0015)**: Staging files are allocated by default under standard OS temporary storage (`/tmp` / `std::env::temp_dir()`) backed by the Linux kernel Page Cache instead of shared memory (`/dev/shm`). This prevents catastrophic container terminations under Docker and Kubernetes 64MB `/dev/shm` constraints and avoids kernel OOM killer actions, while Ramdisk `/dev/shm` remains available as an opt-in configuration for hosts with verified memory capacity.

