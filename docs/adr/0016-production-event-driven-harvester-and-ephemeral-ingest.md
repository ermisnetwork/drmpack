# Production Event-Driven Harvester with Watchdog Fallback

**Status: Amended**

The ArtifactHarvester uses kernel filesystem events (`notify` crate — `inotify` on Linux, `FSEvents` on macOS) as the primary mechanism to detect completed segments and manifests, with a relaxed 1500ms watchdog timer as a safety net. Events are treated as generic wake-up triggers that invoke the existing `harvest_target()` reconciliation function, preserving the Manifest-Driven Readiness invariant.

We chose this hybrid over pure polling (wastes CPU scanning empty directories 5×/sec, adds up to 200ms harvesting jitter) and pure event-driven (kernel events can be dropped under `IN_Q_OVERFLOW`, `FSEvents` lacks `CLOSE_WRITE` semantics, and watcher creation can fail in dense container environments hitting inotify watch limits). The hybrid gives sub-millisecond detection in the common case while guaranteeing no segment is ever orphaned.

Key constraints: `Harvester::spawn()` retains its `-> Self` signature — watcher failure silently degrades to watchdog-only. Staging directories are watched lazily since GPAC creates them after receiving first input data. `push_data()` does not ping the harvester; the ingest rate is fully decoupled from the harvest rate.

## Implementation Reality / Amendments

- **Watchdog Interval Calibrated to 50ms**: In production code (`src/session/harvester.rs:13`), the watchdog timer heartbeat interval was calibrated down from 1500ms to **50ms** (`WATCHDOG_INTERVAL_MS = 50`). Under macOS, `FSEvents` event coalescing by `fseventsd` combined with the flush ordering between media segments and manifest updates could delay harvest wakeups until the next watchdog tick. Tying the safety net to a 1500ms cycle produced unacceptable latency (1.5s–6.0s); tightening the interval to 50ms ensures segment detection latency remains strictly under 100ms with negligible CPU impact.
- **Alignment with Direct Channel and GPAC Flush Guarantees**: Operates in direct coordination with [ADR-0015](0015-direct-output-channel-and-safe-storage.md) (direct asynchronous output channel and ephemeral staging deletion) and [ADR-0019](0019-seg-sync-auto-for-manifest-driven-readiness.md) (enforcing `seg_sync=auto` in GPAC dasher so segments are only announced in manifests once their bytes are fully flushed).
