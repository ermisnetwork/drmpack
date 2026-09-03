# Engineering & Learning Notes

## User Preferences & Standards
- **No Shortcuts or Abbreviations**: Always provide full, comprehensive, line-by-line explanations with zero skipped steps, hand-waving, or compressed pseudo-summaries.
- **Code & Diagram Formatting**: All code blocks and diagrams must be properly formatted using accurate indentation, syntax highlighting, and monospaced typography.
- **Strict Domain Vocabulary**: Strictly adhere to `CONTEXT.md` (e.g., use `Manifest` over `Playlist`, `PackagingSession` over `Pipeline/Job`, `Segment` over `Fragment/Chunk`, `LatencyMode` over `SpeedProfile`, `Ramdisk` over `TempDir`).
- **Two-Axis Quality Bar**: Every feature must pass both **Standards** (Fowler code smells, domain naming, clippy compliance) and **Spec** (functional correctness, RFC compliance, edge cases).

## Architecture & Code Walkthrough Strategy
- **Data-Flow Driven Exploration**: Explain components in sequential pipeline order:
  1. *Domain Types & Configuration* (`src/types.rs`, `src/key.rs`, `src/error.rs`)
     - Core abstractions: `PackagingSessionConfig`, `LatencyMode` (`Standard` vs `LowLatency`), `EncryptionScheme` (`Cenc`, `Cbcs`, `Dual`), `DrmSystem`.
  2. *GPAC Common Encryption XML Generation* (`src/gpac/xml.rs`)
     - Translating `KeySet` into GPAC's `cecrypt` XML format with `<CrypTrack>`, `<Key>`, and `<DRMInfo type="pssh">`.
  3. *Subprocess & Pipe Orchestration* (`src/gpac/process.rs`)
     - Managing long-running `gpac` child process via anonymous Unix pipes (`stdin:ext=mp4`), background `stderr` capture into `tracing`, and `check_status()`.
  4. *Session Coordination & Fail-Fast Watchdog* (`src/session.rs`)
     - Coordinating key fetching, XML writing, pipe streaming, and timeout watchdog.
  5. *Ramdisk Delivery & Zero Disk I/O*
     - Directing GPAC output into `/dev/shm` or `tmpfs` to eliminate physical disk wear during 200ms manifest refresh loops.
  6. *Public Seam Verification* (`tests/tracer_e2e.rs`)
     - End-to-end testing via the public `PackagingSession` API, validating real DASH `.mpd` and LL-HLS `.m3u8` with DRM tags.
- **Single Public Seam**: Maintain `PackagingSession` as the sole public entrypoint for all test assertions and integrations.
