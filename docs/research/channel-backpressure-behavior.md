# Investigation: Channel Backpressure & Memory/Disk Management in `PackagingSession`

**Date:** 2026-09-10
**Scope:** `src/session/mod.rs`, `src/session/harvester.rs`, `src/session/cluster.rs`, `src/gpac/process.rs`, `src/types.rs`

---

## Executive Summary

1. **Channel Type & Buffer Size:** Bounded `tokio::sync::mpsc::channel(1024)` with a fixed **1024-element** buffer of `PackagedArtifact`, created at [`mod.rs:455`](../../src/session/mod.rs#L455).
2. **When Receiver is NEVER taken (`take_output_receiver()` not called):**
   - **RAM:** Data does **NOT** accumulate in memory (0 bytes overhead). `Harvester` is lazy and **never spawned**.
   - **Disk:** All segments (`.m4s`, `init.mp4`) and manifests (`.m3u8`, `.mpd`) produced by GPAC **accumulate entirely on disk**.
   - **Cleanup:** On `session.close()`, the session detects `!self.output_receiver_claimed` and sets `self.preserve_output = true` ([`mod.rs:674-676`](../../src/session/mod.rs#L674-L676)). On drop, the output directory is **preserved on disk** for legacy disk-serving mode.
3. **When Receiver IS taken but NEVER polled/consumed:**
   - **RAM & Internal Backpressure:** Channel accepts up to 1024 artifacts then fills. On artifact #1025, Harvester's `tx.send(artifact).await` **suspends indefinitely** ([`harvester.rs:456`](../../src/session/harvester.rs#L456)). RAM is **bounded**, not unbounded.
   - **No Backpressure to GPAC / Push:** `push()` writes directly to GPAC's stdin pipe (`cluster.write_data()`). GPAC continues writing segments to disk. Since Harvester is suspended, it stops reading and stops deleting segments, causing **unbounded disk accumulation** until `ENOSPC`.
4. **Disk Cleanup:**
   - *Receiver dropped:* Harvester detects `tx.send()` error and **exits immediately** ([`harvester.rs:578`](../../src/session/harvester.rs#L578)). Files created after that remain on disk. On session drop, the output directory is cleaned up if using a temp directory ([`mod.rs:981-988`](../../src/session/mod.rs#L981-L988)).
   - *Receiver never claimed:* Files preserved on disk after close and drop (due to `preserve_output = true`).
   - *Receiver held but not consumed:* Segments 1..1025 are deleted from disk and loaded into RAM; segments 1026+ accumulate on disk.
5. **`PackagingSession` Fate — DEADLOCK RISK:**
   - During streaming: `session.push()` **continues normally**, no blocking, no errors.
   - On `session.close().await`: **DEADLOCKS INDEFINITELY!** At [`mod.rs:667`](../../src/session/mod.rs#L667), session calls `harvester.finish_and_flush().await`, which awaits the Harvester task's join handle. But the Harvester is blocked on `tx.send(artifact).await` with no cancellation token in the select branch. Session hangs forever at `close()`.

---

## Detailed Analysis

---

### Q1: What channel type is used (bounded/unbounded)? What's the buffer size?

The channel between `Harvester` and caller is created inside `PackagingSession::take_output_receiver()`:

- **Location:** [`mod.rs:449-456`](../../src/session/mod.rs#L449-L456)
- **Type:** **Bounded MPSC Channel** (`tokio::sync::mpsc::channel`)
- **Buffer size:** Fixed at **1024 elements** (`PackagedArtifact`)
- **Note:** Inside `Harvester::spawn` ([`harvester.rs:543`](../../src/session/harvester.rs#L543)), there is a separate `mpsc::unbounded_channel()`, but that is used internally to bridge filesystem watcher events (`notify::RecommendedWatcher`) into the Tokio runtime — it is not the artifact output channel.

---

### Q2: If receiver is never taken, does data accumulate in RAM indefinitely?

**Answer: NO**, data does **NOT** accumulate in RAM at all.

- **Lazy Harvester (Zero Overhead if not claimed):**
  - In `PackagingSession::create()` ([`mod.rs:430-431`](../../src/session/mod.rs#L430-L431)): `harvester: None, output_receiver_claimed: false`.
  - Harvester is **only spawned** when caller explicitly calls `session.take_output_receiver()` ([`mod.rs:469-470`](../../src/session/mod.rs#L469-L470)).
- **No Harvester = No Channel = No disk-to-RAM reads:**
  - Since `harvester` is `None`, no channel is created, no background task reads media segments into `PackagedArtifact` (which contains `bytes::Bytes`).
  - RAM consumption for artifacts is exactly **0 bytes**.
- **Disk Accumulation:**
  - GPAC still receives raw media from `session.push()` and continuously writes `.m4s`, `init.mp4`, `.m3u8`, `.mpd` files to `config.output_dir`.
  - Since Harvester is not running, no task unlinks temporary `.m4s` files (normally deleted at [`harvester.rs:448`](../../src/session/harvester.rs#L448)).
  - Result: **Data accumulates entirely on disk**, potentially causing `ENOSPC` on constrained volumes.

---

### Q3: If receiver IS taken but never polled/consumed, does backpressure apply or does memory grow?

**Answer: Backpressure kicks in for Harvester (RAM is bounded), but there is NO backpressure to GPAC or `session.push()` (disk grows unboundedly).**

#### Harvester Backpressure (RAM bounded):
- Harvester spawns with `tx` capacity 1024.
- Once 1024 artifacts fill the channel buffer, `tx.send(artifact).await` on artifact #1025 **suspends indefinitely**.
- RAM is **bounded** at ~1024 segments + state history capped at 5000 filenames (`MAX_EMITTED_HISTORY`, [`harvester.rs:12`](../../src/session/harvester.rs#L12)).

#### Backpressure Gap with GPAC:
- `PackagingSession::push()` ([`mod.rs:526-601`](../../src/session/mod.rs#L526-L601)) writes directly to GPAC's stdin pipe via `RepresentationCluster::write_data` ([`cluster.rs:199`](../../src/session/cluster.rs#L199)).
- GPAC is completely independent — it continues segmenting and writing to disk.
- **Consequence:** RAM stays at ~1024 artifacts, but **disk grows without bound**.

---

### Q4: Are artifact files on disk cleaned up if receiver is dropped or not consumed?

| Scenario | During session | After `session.close()` | After `drop(session)` |
|:---|:---|:---|:---|
| Never call `take_output_receiver()` | Files remain on disk | Sets `preserve_output = true` | **Preserved** (legacy disk-serving) |
| Call then `drop(rx)` | Files remain after Harvester exits | `preserve_output` stays `false` | **Cleaned up** (if temp dir) |
| Hold `rx` without consuming | Segments 1..1025 deleted from disk; 1026+ remain | **DEADLOCK** at `finish_and_flush` | Abort Harvester + **cleaned up** (if skipping close) |

---

### Q5: What happens to `PackagingSession` itself — does it block, error, or silently drop artifacts?

#### Ingest Phase: `session.push(bytes)`
- **Does NOT block, does NOT error, does NOT drop input media.** GPAC continues receiving media via stdin.

#### Close Phase: `session.close().await` — DEADLOCK VULNERABILITY

> [!CAUTION]
> If caller has called `take_output_receiver()`, holds `rx` but never consumes from it, then `session.close().await` will **hang forever** (deadlock).

**Deadlock Mechanism:**
1. Harvester task is suspended at `tx.send(artifact).await` ([`harvester.rs:456`](../../src/session/harvester.rs#L456)) — this call is **NOT** inside a `tokio::select!` with the cancellation token.
2. `close()` calls `harvester.finish_and_flush().await` ([`mod.rs:667`](../../src/session/mod.rs#L667)), which awaits `handle.await` ([`harvester.rs:604`](../../src/session/harvester.rs#L604)).
3. `shutdown_token.cancel()` fires, but Harvester is not at the top of the `select!` loop — it's blocked inside `harvest_target()`. The cancel token cannot wake `tx.send().await`.
4. Harvester task **never exits** → `finish_and_flush` hangs forever → `close()` hangs forever.

---

## Scenario Matrix

| Criterion | Never call `take_output_receiver()` | Call then `drop(rx)` | Hold `rx` without polling |
| :--- | :--- | :--- | :--- |
| **Harvester Task** | Never spawned (`None`) | Spawns → terminates on `tx.send()` error | Spawns → suspends at artifact #1025 |
| **RAM** | **0 bytes** | A few segments before drop | **Bounded** (~1024 artifacts) |
| **Backpressure to GPAC** | None | None | None |
| **Disk Data** | Accumulates entirely | Accumulates after rx drop | 1..1025 deleted; 1026+ accumulates |
| **`session.push()`** | OK | OK | OK |
| **`session.close()`** | OK, sets `preserve_output` | OK (task already exited) | **DEADLOCK** |
| **`drop(session)`** | Preserves files | Cleans up (temp dir) | Aborts Harvester + cleans up |

---

## Recommendations

1. **Fix Deadlock in `harvester.finish_and_flush()`:**
   - Wrap `tx.send(artifact)` with `tokio::select!` against `loop_shutdown.cancelled()` in [`harvester.rs`](../../src/session/harvester.rs). If cancelled while channel is full, Harvester should break/return instead of hanging.
   - Wrap `tokio::time::timeout()` around `harvester.finish_and_flush()` in [`mod.rs:666-668`](../../src/session/mod.rs#L666-L668). On timeout, call `harvester.cancel()` to abort the task.

2. **Document Caller Obligation:**
   - If `take_output_receiver()` is called, the caller **must** either consume continuously (`while let Some(..) = rx.recv().await`) or explicitly `drop(rx)` when artifacts are no longer needed. Holding `rx` without consuming is a misuse that leads to deadlock on `close()`.
