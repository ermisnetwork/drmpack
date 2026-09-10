# GPAC Command-Line Options Optimization Audit & Verification

> **Scope:** Subprocess orchestration in `drmpack/src/gpac/process.rs`, GPAC `dasher`, `stdin` (`pin`), `mp4dmx`, `cecrypt`, and session filters.  
> **Primary Sources:**
> - GPAC Official Man Pages: `gpac.1`, `gpac-filters.1` (GPAC 26.07-revrelease on macOS/Linux).
> - GPAC Official Documentation: [gpac.io Filters/dasher](https://wiki.gpac.io/Filters/dasher/), [gpac.io Filters/pin](https://wiki.gpac.io/Filters/pin/).
> - GPAC Core Source Code: `src/filters/dasher.c`, `src/filters/mux_isom.c`, `src/filters/in_pipe.c`, `src/filter_core/filter_session.c`.
> - Project Architecture Documents: `CONTEXT.md`, `ADR-0010`, `ADR-0014`, `ADR-0019`, [gpac-command-options-verification.md](gpac-command-options-verification.md), [gpac-pipe-session-options-verification.md](gpac-pipe-session-options-verification.md).

---

## 1. Executive Summary

The subprocess configuration in `drmpack/src/gpac/process.rs` orchestrates continuous live fMP4 ingestion over anonymous Unix pipes. An investigation against GPAC documentation and upstream source code reveals:

1. **Immediate Correctness Fix Applied (`seg_sync=auto`)**:
   - `seg_sync=no` permitted GPAC to announce segments in the HLS playlist before the trailing bytes/packets were flushed to storage.
   - For multi-fragment segments, `ArtifactHarvester`'s `is_complete_isobmff_media_segment` could match the first complete `moof`+`mdat` boundary and emit a truncated segment.
   - Switching to `seg_sync=auto` (ADR-0019) ensures GPAC waits until the last packet is flushed before updating `.m3u8`, restoring the Manifest-Driven Readiness invariant.

2. **Refuted Assumptions & Non-viable Patches**:
   - **`stdin:timeout=0` does not disable timeout**: In GPAC source (`in_pipe.c` L109–115), if `src` is `stdin` or `-` and `timeout == 0`, GPAC automatically resets `timeout = 10000` ms. Passing `timeout=0` is a no-op.
   - **`block_size=65536` does not enlarge read syscalls**: In `in_pipe.c` L132 and L247, GPAC clamps `read_block_size = MIN(8192, block_size)`. Setting 64 KB merely allocates a larger internal buffer without increasing single `fread()` sizes beyond 8 KB.
   - **`-no-block=all` does not prevent memory accumulation**: It removes PID buffer blocking regulation (`GF_FS_NOBLOCK`) so producers never suspend. If downstream processing is slow, queues grow in RAM rather than being capped. Its true purpose is preventing pipe deadlocks.
   - **Missing `cmaf=cmfc` in Standard mode does not cause `muxed_base` stalls**: GPAC only couples tracks into a `muxed_base` when they share the same Representation ID. `drmpack` assigns distinct IDs (`video_...`, `audio_...`), preventing cross-track multiplexing.

3. **Deferred Items (Pending Benchmarks / E2E Verification)**:
   - **`asto`**: While `segdur - cdur` is standard for DASH-LL, `drmpack` currently emits full segments via `ArtifactHarvester`. Advertising early availability before egress can serve chunks risks player HTTP 404s.
   - **`cmaf=cmfc` in Standard Mode**: Enforcing CMAF activates strict multiplexer checks (sample mixing, edit lists). Deferred to a dedicated compliance PR with regression testing.
   - **`threads`**: `-threads=-1` remains stable and avoids `cecrypt` starvation. Any reduction must be validated under concurrent load benchmarks.

---

## 2. Options Breakdown & Status

| Option | Location | Status | Rationale & Code Action |
| :--- | :--- | :--- | :--- |
| **`seg_sync=auto`** | `dasher_opts` | ✅ **Fixed (ADR-0019)** | Replaces `seg_sync=no`. Waits for the final segment packet to flush before HLS playlist update, eliminating partial segment reads in Harvester. |
| **`-no-block=all`** | Session args | ✅ **Retained** | Disables filter blocking regulation to avoid backpressure deadlocks on the stdin pipe. (Code comments updated to correct the memory claim). |
| **`-threads=-1`** | Session args | ✅ **Retained** | Prevents `cecrypt` starvation during live pipe reads. Sizing reduction deferred until multi-stream benchmarks. |
| **`-logs=ncl`** | Session args | ✅ **Optimal** | Disables ANSI color escapes on stderr for clean Tracing log classification. |
| **`stdin:ext=mp4:alltk`** | Input filter | ✅ **Retained** | Preserves existing filter. `timeout=0` and `block_size=65536` rejected due to GPAC internal clamps (`MIN(8192, ...)` and reset to 10000ms). |
| **`dmode=dynauto`** | `dasher_opts` | ✅ **Optimal** | Dynamic manifest during live ingest; auto-converts to static with `#EXT-X-ENDLIST` on stdin EOF. |
| **`profile=live`** | `dasher_opts` | ✅ **Optimal** | Enforces MPEG-DASH Live profile with SegmentTemplate. |
| **`sbound=out`** | `dasher_opts` | ✅ **Optimal** | Immediate boundary cut at next SAP without GOP lookahead buffering delay. |
| **`check_dur=false`** | `dasher_opts` | ✅ **Optimal** | Relaxes strict cross-track period duration enforcement. |
| **`keep_segs=true`** | `dasher_opts` | ✅ **Optimal** | Preserves segments in staging directory for Harvester consumption. |
| **`pssh=mv`** | `dasher_opts` | ✅ **Optimal** | Injects PSSH in both MPD and initialization `moov`. |
| **`utcs=inband`** | `dasher_opts` | ✅ **Optimal** | Inband UTC timing descriptor using MPD publish time. |
