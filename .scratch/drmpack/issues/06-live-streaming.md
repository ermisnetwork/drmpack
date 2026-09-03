# 06: Process Lifecycle & Fail-Fast Watchdog

**What to build:** Robust child process management for the GPAC subprocess. Implement a fail-fast supervisor that monitors the `gpac` child process via async tasks: captures and logs `stderr` output using the `tracing` crate, detects unexpected process exits/crashes immediately, and returns `Err(DrmPackError::ProcessCrashed { exit_code, stderr })` to `media-server`. Handle graceful termination via `PackagingSession::close()`, ensuring GPAC flushes the final manifest (`#EXT-X-ENDLIST`) before exiting, and cleans up the session's Ramdisk directory.

**Blocked by:** 01 (Tracer)

**Status:** ready-for-agent

- [ ] Background async task monitoring child process exit status
- [ ] Stderr stream reader forwarding GPAC log lines to `tracing::warn!` / `tracing::error!`
- [ ] Fail-fast crash propagation: `push_segment()` returns `Err(ProcessCrashed)` immediately if process exited prematurely
- [ ] Graceful shutdown: `session.close()` closes stdin, awaits process exit with configurable timeout, and verifies exit code 0
- [ ] Ramdisk cleanup: delete `/dev/shm/<session_id>/` upon session close or drop
- [ ] Inactivity watchdog: auto-close session if no segments pushed within configurable timeout
