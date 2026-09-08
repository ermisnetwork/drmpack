# Performance Review — drmpack

_Date: 2026-09-08_

## Executive Summary
The `drmpack` library demonstrates strong foundational architecture with efficient async process supervision (ADR-0011) and connection pooling (ADR-0009). However, the implementation of the Direct Output Channel (ADR-0015) in the `Harvester` introduces severe disk I/O, memory allocation, and CPU bottlenecks. The most critical issue causes O(N*T) file reads of growing media segments, severely undermining the "zero disk I/O overhead" goal. Addressing the harvester loop and reducing system calls in the pipe writer will yield massive performance gains.

## Critical Findings

### 1. Harvester Segment Thrashing (O(N*T) I/O & Memory)
- **Severity**: Critical
- **Location**: `src/session/harvester.rs`, lines 346-368
- **Description**: The `in_manifest` check uses `mpd_haystack.contains(rep_id)`. For DASH manifests, the `rep_id` is defined in the static `SegmentTemplate`, meaning it is *always* present in the MPD from the very beginning of the session. Because of this, the harvester eagerly attempts to read actively growing `.m4s` segments every 200ms using `tokio::fs::read(&path).await`. If `is_complete_isobmff_media_segment` is false, it drops the loaded `Vec<u8>` and repeats this cycle until the segment completes. For a 4MB segment spanning 2 seconds, this allocates, reads, and drops 40MB of data across 10 ticks.
- **Reference**: `tokio::fs::read` allocates entirely in memory.
- **Suggested Fix**: Rely exclusively on HLS manifest segment append events (`hls_haystack.contains(&file_name)`) as a signal for segment completion, or use an `inotify`/`fsevents` watcher (via the `notify` crate) to trigger strictly on file `IN_CLOSE_WRITE` events.

### 2. O(N²) String Haystack Allocation in Harvester Hot Path
- **Severity**: Critical
- **Location**: `src/session/harvester.rs`, lines 290-323
- **Description**: On every 200ms tick, the harvester iterates over all manifests, reads them into memory, and concatenates their contents into giant `hls_haystack` and `mpd_haystack` strings. It then performs substring searches (`.contains(&file_name)`) for every segment present in the directory. This scales quadratically as the live window grows and the number of tiers increases.
- **Suggested Fix**: Parse manifests once and cache a `HashSet<String>` of known complete segment filenames. Only re-read and update the set when the manifest file's modified time or size increases.

## Moderate Findings

### 3. Unbuffered ChildStdin Flushing
- **Severity**: Moderate
- **Location**: `src/gpac/process.rs`, lines 303-311
- **Description**: The `write_data` function directly calls `stdin.write_all(data).await` and `stdin.flush().await`. `ChildStdin` wraps a raw OS pipe. If callers push small byte chunks (e.g., individual NALUs), this results in excessive `write` syscalls.
- **Reference**: `tokio::process::ChildStdin` is unbuffered.
- **Suggested Fix**: Wrap the `ChildStdin` in `tokio::io::BufWriter` to batch writes and amortize syscall overhead.

### 4. Excessive String Cloning in Segment Evaluation Loop
- **Severity**: Moderate
- **Location**: `src/session/harvester.rs`, lines 327-330
- **Description**: The loop creates a tuple `let key = (scheme, file_name.clone());` for every file in the directory on every 200ms tick, just to check if it exists in `state.emitted_segments`.
- **Suggested Fix**: Defer cloning. In Rust, checking a `HashSet<(EncryptionScheme, String)>` with a borrowed `&str` requires custom trait implementations, but an alternative is storing `HashSet<String>` partitioned by scheme, allowing `state.emitted_segments.get(&scheme).unwrap().contains(file_name.as_str())`.

### 5. Deep Copying in Manifest Cache
- **Severity**: Moderate
- **Location**: `src/session/harvester.rs`, lines 420-423
- **Description**: The manifest cache stores `Vec<u8>`. During insertion, the vector is deep-copied: `state.manifest_cache.insert(key, data_bytes.clone());`, and then immediately wrapped into a `Bytes::from(data_bytes)` for emission.
- **Suggested Fix**: Change `manifest_cache` to store `bytes::Bytes`. Cloning a `Bytes` instance is an O(1) atomic reference count increment.

## Minor / Low-Priority

### 6. Stderr Buffer Lock Contention
- **Severity**: Minor
- **Location**: `src/gpac/process.rs`, lines 225-230
- **Description**: Log harvesting relies on `buffer_clone.lock().unwrap()` using `std::sync::Mutex` inside a Tokio async task. While the lock is held briefly without `await` points, it can block the async worker thread under high log volume.
- **Suggested Fix**: Use an `mpsc` channel to forward log strings to a centralized supervisor loop, or swap to `parking_lot::Mutex`.

## Hot Path Analysis

### The Segment Packaging Loop
1. **Input**: Unbuffered writes in `gpac/process.rs` limit throughput on small chunks.
2. **GPAC Processing**: Runs efficiently out-of-process.
3. **Harvesting (Critical Bottleneck)**: The `Harvester` loop represents the most dangerous hot path in the system. Staging segments in `/tmp` (ADR-0015) successfully avoids OOM crashes, but the polling implementation aggressively reads incomplete files back into memory (due to the DASH MPD template flaw) and reconstructs massive string haystacks 5 times per second. This completely negates the performance benefits of zero-copy disk I/O.

### Manifest Updates
Updating manifests relies on `std::str::from_utf8` and direct string replacement (`sanitize_static_mpd`). While string manipulation is generally fast, repeatedly reading the same manifest from disk every 200ms instead of maintaining an in-memory diff causes unnecessary file descriptor churn and page cache invalidations.

## Positive Patterns

- **Async Process Supervisor (ADR-0011)**: Excellent use of `tokio::select!` and `watch::channel` for non-blocking child process monitoring.
- **Zero-Cost Abstractions**: Conversion of vectors to `bytes::Bytes` (`Bytes::from(data)`) for the output channel correctly avoids memory copying during artifact dispatch.
- **Connection Pooling (ADR-0009)**: `LicenseProxy` efficiently reuses the `reqwest::Client`, keeping TLS handshakes to a minimum.
- **Memory Safety Limits**: Enforcing a `MAX_EMITTED_HISTORY` (5000) prevents unbounded memory growth in the harvester state.

## Recommendations

1. **(Immediate)** Fix the `in_manifest` DASH logic in `Harvester::harvest_target` to prevent O(N*T) thrashing of growing `.m4s` segments. Rely exclusively on the HLS manifest for completion signals.
2. **(High)** Replace the 200ms directory polling loop with the `notify` crate to listen for `IN_CLOSE_WRITE` filesystem events.
3. **(High)** Cache parsed manifest segment lists instead of reading and concatenating files into giant strings on every tick.
4. **(Medium)** Wrap `ChildStdin` in `tokio::io::BufWriter` to batch IPC writes.
5. **(Low)** Refactor `HarvesterState` to use `bytes::Bytes` and reduce `String` cloning.
