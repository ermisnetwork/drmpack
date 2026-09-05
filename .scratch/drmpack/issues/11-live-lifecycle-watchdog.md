# 11: Process Lifecycle & Fail-Fast Watchdog

**What to build:** Robust child process management for the GPAC subprocess. Implement a fail-fast supervisor that monitors the `gpac` child process via async tasks: captures and logs `stderr` output with severity parsing (forwarding warnings and errors to `tracing::warn!` / `tracing::error!`), detects unexpected process exits/crashes immediately, and returns `Err(DrmpackError::ProcessCrashed { exit_code, stderr })` to `media-server`. Handle graceful termination via `PackagingSession::close()`, ensuring GPAC flushes the final manifest (`#EXT-X-ENDLIST`) before exiting, and cleans up the session's Ramdisk directory.

**Blocked by:** 01 (Tracer)

**Status:** closed

- [x] Add `-logs=ncl` to GPAC command args to disable ANSI color codes
- [x] Stderr stream reader with severity parsing: forward `error`/`failed to` -> `tracing::error!`, `warning` -> `tracing::warn!`, `info` -> `tracing::info!`, rest -> `tracing::debug!`, maintaining bounded circular buffer of recent lines
- [x] Background `ProcessSupervisor` task owning `child.wait()` per GPAC subprocess, broadcasting exit notifications across async channel
- [x] Fail-fast crash propagation: premature exit immediately marks session `Failed`, triggers `CancellationToken`, and causes `push_segment()` to return structured failure immediately
- [x] Dual-mode symmetric fail-fast: unexpected exit of either CENC or CBCS immediately triggers teardown of the remaining healthy representation and surfaces unified `PackagingSessionFailure`
- [x] Graceful shutdown with `#EXT-X-ENDLIST` verification: `session.close()` closes stdin, awaits exit with timeout, verifies exit code 0, and validates `#EXT-X-ENDLIST` in HLS manifests for streams with media segments
- [x] Expose `session.is_alive() -> bool` for non-destructive liveness healthchecks
- [x] Add `PackagingOperation::Supervisor` in `src/error.rs` and reflect domain model in `CONTEXT.md`
- [x] Comprehensive unit and integration tests covering premature crash, stderr severity classification, Dual mode abort, and endlist verification
