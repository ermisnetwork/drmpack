# Production Event-Driven Harvester with Watchdog Fallback

The ArtifactHarvester uses kernel filesystem events (`notify` crate — `inotify` on Linux, `FSEvents` on macOS) as the primary mechanism to detect completed segments and manifests, with a relaxed 1500ms watchdog timer as a safety net. Events are treated as generic wake-up triggers that invoke the existing `harvest_target()` reconciliation function, preserving the Manifest-Driven Readiness invariant.

We chose this hybrid over pure polling (wastes CPU scanning empty directories 5×/sec, adds up to 200ms harvesting jitter) and pure event-driven (kernel events can be dropped under `IN_Q_OVERFLOW`, `FSEvents` lacks `CLOSE_WRITE` semantics, and watcher creation can fail in dense container environments hitting inotify watch limits). The hybrid gives sub-millisecond detection in the common case while guaranteeing no segment is ever orphaned.

Key constraints: `Harvester::spawn()` retains its `-> Self` signature — watcher failure silently degrades to watchdog-only. Staging directories are watched lazily since GPAC creates them after receiving first input data. `push_data()` does not ping the harvester; the ingest rate is fully decoupled from the harvest rate.
