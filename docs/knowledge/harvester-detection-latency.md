# Harvester Segment Detection Latency on macOS

> **Context:** `drmpack` ArtifactHarvester uses `notify::RecommendedWatcher` (FSEvents on macOS)
> with a 1500ms watchdog fallback to detect GPAC output segments.
>
> **Observed:** Example 08 shows ~6s delays between segment outputs during live
> packaging, with most segments flushing at finalization rather than real-time.

## Root Cause Analysis

The delay is **NOT caused by FSEvents latency**. Research confirms:

| Factor | Value | Source |
|--------|-------|--------|
| `notify` 8.2.0 default FSEvents latency | `Duration::ZERO` | [config.rs L248](https://github.com/notify-rs/notify/blob/main/notify/src/config.rs) |
| `kFSEventStreamCreateFlagNoDefer` | Enabled | [fsevent.rs L306](https://github.com/notify-rs/notify/blob/main/notify/src/fsevent.rs) |
| FSEvents physical delivery floor | 10–50ms | Apple `fseventsd` daemon IPC overhead |
| `Config::default()` ignored by `FsEventWatcher` | Yes — `_config: Config` | [fsevent.rs L579](https://github.com/notify-rs/notify/blob/main/notify/src/fsevent.rs) |
| `with_poll_interval()` effect on FSEvents | **None** (PollWatcher only) | [config.rs L90–98](https://github.com/notify-rs/notify/blob/main/notify/src/config.rs) |

### The actual bottleneck: Manifest-Driven Readiness + GPAC timing

The harvester uses a **two-step readiness gate** (ADR-0015):

```rust
// harvester.rs L395
let in_manifest = is_init || hls_segments.contains(&file_name) || is_final;
```

A media segment is only emitted when:
1. The segment `.m4s` file exists on disk **AND**
2. The HLS `.m3u8` manifest has been updated to reference it **AND**
3. The segment passes `is_complete_isobmff_media_segment()` validation

**Timeline of a single segment:**
```
t=0.0s  FFmpeg encodes GOP (2s of video)
t=2.0s  FFmpeg outputs fMP4 fragment to stdout
t=2.0s  SessionWriter forwards to GPAC stdin
t=2.5s  GPAC writes video_720p_1.m4s to staging dir
        → FSEvents fires (~20ms) → Harvester wakes
        → BUT video_720p.m3u8 not yet updated
        → Segment NOT emitted (in_manifest = false)
t=3.0s  GPAC updates video_720p.m3u8
        → FSEvents fires → Harvester wakes
        → Manifest parsed, "video_720p_1.m4s" found
        → Segment read, validated, emitted ✓
```

The ~6s gap in Example 08 output is caused by:
1. **FFmpeg `-re` encoding latency**: First GOP takes ~2–4s to encode + output
2. **GPAC packaging pipeline**: Internal buffering before first segment write
3. **Manifest update delay**: GPAC updates `.m3u8` after segment is fully written
4. **Harvester loop timing**: Select loop may be mid-harvest when event arrives

## ADR Constraints (Non-Negotiable)

Per ADR-0015 and ADR-0016, any fix MUST:
1. **Keep manifest-driven readiness** — segments only emit after manifest references them
2. **Stay filesystem-staged** — HTTP push, pipes, and C FFI are rejected (ADR-0015 L22-24)
3. **Retain hybrid event+watchdog** — graceful degradation on watcher failure (ADR-0016)
4. **Preserve ephemeral unlink** — files deleted immediately after channel dispatch

### Why GPAC HTTP Push Was Rejected (ADR-0015 L23)

> "Rejected per Ponytail minimalism. Requires hosting an in-process HTTP server,
> managing ephemeral TCP loopback ports, and exposes the system to upstream GPAC
> HTTP client bugs (memory leaks on long live runs in issue #2923 and multi-PID
> failures in issue #3027) while offering zero performance advantage over Linux
> kernel Page Cache."

Three concrete reasons:
1. **Complexity**: In-process HTTP server + ephemeral TCP port management
2. **GPAC bugs**: Memory leaks (#2923), multi-PID crashes (#3027)
3. **Zero perf gain**: Linux Page Cache already sub-millisecond for temp files

## Remediation Options (ADR-Compliant)

### Option A: Reduce Watchdog Interval (Low Risk, Easy)

```rust
const WATCHDOG_INTERVAL_MS: u64 = 100; // was 1500
```

- Guarantees ≤100ms worst-case detection even if FSEvents drops an event
- Scanning a small staging dir (5–10 files) every 100ms: <0.2% CPU on Apple Silicon
- Does NOT fix the manifest gate timing — segments still wait for .m3u8 update
- **Best as a complementary fix**

### Option B: Debounce FSEvents + Manifest-Aware Harvest (Medium Risk)

Instead of harvesting on every single FSEvents notification, batch events
across a 10–20ms window so GPAC has time to finish writing BOTH the segment
AND the manifest before the reconciliation pass runs:

```rust
tokio::select! {
    _ = loop_shutdown.cancelled() => break,
    Some(_) = fs_rx.recv() => {
        // Drain all pending events, then wait 15ms for GPAC to finish
        while fs_rx.try_recv().is_ok() {}
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    _ = watchdog.tick() => {}
}
```

- Lets GPAC's segment+manifest writes settle before harvesting
- Reduces wasted harvest passes that find segment but no manifest yet
- Total latency: FSEvents ~20ms + debounce 15ms + harvest = ~40ms
- Preserves manifest-driven readiness invariant

### Option C: Filter on `.m3u8` Events Only (Medium Risk)

Only trigger harvest when a manifest file changes, not on every filesystem event:

```rust
Some(Ok(event)) = fs_rx.recv() => {
    let is_manifest = event.paths.iter().any(|p|
        p.extension().map_or(false, |e| e == "m3u8" || e == "mpd")
    );
    if is_manifest {
        // Harvest — manifest update means segment is ready
    }
}
```

- Directly addresses the root cause: harvest only runs when the readiness
  signal (manifest update) actually fires
- Eliminates wasted harvest passes triggered by .m4s creation events
- Preserves ADR-0015 manifest-driven readiness completely

## Recommendation

**Option B + reduced watchdog (Option A)** is the safest approach:
- 15ms debounce lets segment+manifest writes settle
- 100ms watchdog catches any missed events
- Preserves all ADR invariants
- Expected improvement: segments appear within ~50ms of GPAC finishing,
  instead of waiting up to 1500ms for next watchdog tick

## Sources

1. ADR-0015: Direct Output Channel and Safe Storage (L16, L22-24)
2. ADR-0016: Production Event-Driven Harvester and Ephemeral Ingest (L3)
3. Apple Developer Documentation — `FSEventStreamCreate` latency parameter
4. `notify` 8.2.0 source: `fsevent.rs` L306 (latency: 0.0, NoDefer)
5. `drmpack` `src/session/harvester.rs` L390-395: Manifest-driven readiness
6. GPAC Issues #2923 (HTTP memory leaks), #3027 (multi-PID failures)
