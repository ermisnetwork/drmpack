# GPAC Live Packaging: Timeline Synchronization, Latency Accumulation & Segment Emission Mechanics

> **Scope:** GPAC Subprocess Orchestration, `drmpack` Live Pipeline, fMP4 Unix Pipe Ingestion (`mp4dmx` -> `dasher` -> `mux_isom`).  
> **Primary Sources:** GPAC Official Wiki (`wiki.gpac.io/Filters/dasher/`), GitHub Issues (`gpac/gpac#2907`, `#2488`, `#3154`, `#3224`), GPAC C Source Code (`src/filters/dasher.c`, `src/filters/mux_isom.c`).

---

## 1. Why GPAC Dasher Holds Video Segments & Delays Emission with Audio & Video

When ingesting an fMP4 pipe containing both H.264 video (e.g. 30 fps) and AAC audio (e.g. 48 kHz), GPAC `dasher` frequently exhibits behavior where video segments are held back, delayed, or only flushed upon stream closure (EOS). This is driven by four interrelated architectural mechanisms in GPAC:

### 1.1. Incommensurable Frame Durations & Boundary Drift
- **Video (30 fps):** Each video frame lasts exactly $\frac{1}{30}\text{ s} \approx 33.333\text{ ms}$. For a target segment duration of 2.0 seconds (`segdur=2`), 60 video frames equal **exactly 2000.0 ms**.
- **Audio (AAC 48 kHz, 1024 samples/frame):** Each AAC audio packet lasts $\frac{1024}{48000}\text{ s} \approx 21.3333\text{ ms}$.
  $$\frac{2000\text{ ms}}{21.3333\text{ ms}} = 93.75\text{ frames}$$
  Since frames cannot be divided without sample splitting:
  - 93 audio frames = **1984.0 ms** (16 ms shorter than 2.0 s).
  - 94 audio frames = **2005.333 ms** (5.333 ms longer than 2.0 s).
- Because audio and video segment boundaries cannot land on identical mathematical timestamps, segment boundaries drift relative to each other across segments.

### 1.2. Cross-Stream CTS Regulation (`min_segment_start_time`)
In `src/filters/dasher.c` (lines 10667–10675), GPAC implements an internal regulation mechanism to prevent any track from advancing ahead of others:
```c
// perform regulation of inputs to avoid dashing one stream faster than the others
if (!base_ds->segment_started && ctx->min_segment_start_time) {
    orig_cts = cts;
    if (ds->split_dur_next)
        cts += ds->split_dur_next;

    if (gf_timestamp_greater(cts, ds->timescale, ctx->min_segment_start_time, 1000)) {
        nb_seg_waiting++;
        break;
    }
    cts = orig_cts;
}
```
Whenever any stream begins a new segment, `ctx->min_segment_start_time` is updated. If video frames arrive faster or ahead of audio packets (or vice versa), the segmenter executes `break`, suspending packet consumption on the leading stream until the lagging stream catches up.

### 1.3. Multi-Track Coupling & Base Stream Dependency (`muxed_base` & `check_dur`)
In `dasher.c` (lines 10695 & 10986):
```c
// mux rep, wait for a CTS more than our base if base not yet over
if ((base_ds != ds) && !base_ds->seg_done && gf_timestamp_greater(cts, ds->timescale, base_ds->last_cts, base_ds->timescale))
    break;

// we have a base (muxed rep) and it is not yet done, and we exceed estimated next seg start on base
// wait for the base to be done as the next seg estimate may change (cf #2488)
if ((ds != base_ds) && !base_ds->seg_done) {
    break;
}
```
If CMAF separation is not explicitly enabled (`cmaf=cmfc`), GPAC attempts representation multiplexing or base-stream locking (`muxed_base`). Even when streams reside in separate Adaptation Sets, `check_dur=true` (default) causes GPAC to check period durations across adaptation sets and force `force_rep_end` (line 8945), stalling segments if duration parity is not reached.

### 1.4. Pipe Demuxing & Backpressure Lockup
When reading an interleaved fMP4 byte stream from an anonymous Unix pipe:
1. Upstream encoders emit clusters of `moof`/`mdat` boxes (often video chunks followed by audio chunks).
2. If the OS pipe buffer holds unread audio packets while the video packets for segment $N$ have already arrived, `dasher` reaches the segment boundary for video, but refuses to emit the segment because audio has not reached `adjusted_next_seg_start`.
3. If neither stream can advance to trigger the next read, the pipeline deadlocks or lags continuously until stream close (`EOS`), where `dasher_process` executes its terminal flush (`is_eos`), dumping all backlogged segments in a single burst.

### 1.5. Muxer Write Synchronization Barrier (`seg_sync`)
In `src/filters/dasher.c` (lines 4160–4170):
```c
switch (ctx->seg_sync) {
case DASHER_SEGSYNC_AUTO:
    if (!ctx->do_m3u8) break;
    // fallthrough
case DASHER_SEGSYNC_YES:
    gf_filter_pid_set_property(ds->opid, GF_PROP_PID_FORCE_SEG_SYNC, &PROP_BOOL(GF_TRUE));
    break;
case DASHER_SEGSYNC_NO:
    break;
}
```
When HLS output is enabled (`do_m3u8`), `seg_sync` defaults to `auto`, which forces `force_seg_sync=true` into the multiplexer (`mux_isom`). In `mux_isom.c` (lines 8031, 8229):
- The muxer sets `seg_flush_state = 1`.
- The segment completion event is deferred until the operating system/file sink fully destructs and flushes the final byte packet (`mp4_mux_on_packet_destruct` -> `seg_flush_state = 2`).
- Until this callback completes, `mp4_mux_process_fragmented` aborts early (`return GF_OK;`), withholding segment events and delaying manifest emission.

---

## 2. Audio/Video Stream Synchronization Mechanics in `profile=live:dmode=dynauto`

In `profile=live:dmode=dynauto`, GPAC orchestrates dynamic live generation with automatic transition to static VOD manifests upon upstream stream conclusion. The synchronization behavior is controlled by several key parameters:

| Parameter | Type & Default | Mechanism & Impact on Live Synchronization |
| :--- | :--- | :--- |
| **`dmode=dynauto`** | `enum` (default: `static`) | Runs in dynamic live mode (`GF_MPD_TYPE_DYNAMIC`), outputting `<MPD type="dynamic">`. On receiving `GF_EOS`, automatically rewrites the manifest into `<MPD type="static">` with final duration, avoiding broken manifests. |
| **`check_dur`** | `bool` (default: `true`) | Enforces roughly equal durations across all sources/Adaptation Sets in the period. In live streaming with fractional frame duration differences (e.g. 21.33ms AAC vs 33.33ms H.264), **`check_dur=true` causes dasher to hold segments or force clamp boundaries (`force_rep_end`)**. Disabling (`check_dur=false`) decouples track durations and prevents pipeline stalls. |
| **`seg_sync`** | `enum` (`auto`, `yes`, `no`, default: `auto`) | Dictates whether manifest updates wait for the last packet of a segment to be flushed to disk/sink. In `auto`, defaults to `yes` for HLS to prevent 404 read races. In `no`, manifest updates are announced immediately upon boundary detection, lowering packaging latency. |
| **`strict_sap`** | `enum` (`off`, `sig`, `on`, `intra`, default: `off`) | In `off`, GPAC ignores SAP types for non-visual streams (audio), forcing `startsWithSAP=1`. This allows AAC audio to split on arbitrary sample boundaries without requiring keyframe alignment. |
| **`sbound`** | `enum` (`out`, `closest`, `in`, default: `out`) | Controls alignment between segment boundaries and Sync Access Points (SAP):<br>• `out` (default): Splits as soon as theoretical boundary is reached at the next SAP (`TSS <= segment_start`). **Zero buffering delay.**<br>• `closest` & `in`: Compares distances between adjacent SAPs. **Requires buffering at least one full GOP ahead**, introducing significant latency. *GPAC Wiki warning: "These modes will introduce delay in the segmenter (typically buffering of one GOP) and should not be used for low-latency modes."* |
| **`cmaf`** | `enum` (`no`, `cmfc`, `cmf2`, default: `no`) | When set to `cmfc`, GPAC strictly adheres to CMAF: isolates media tracks into separate Adaptation Sets (`if (ctx->cmaf) continue;` skips muxed representations), disables multiplexed audio+video in single fragments, and enforces clean track-independent segment clocks. |
| **`spd`** | `sint` (default: `0`) | Suggested Presentation Delay in milliseconds (`suggestedPresentationDelay` in MPD). Instructs DASH clients how far behind the live edge to buffer and play (typically set to $2\times$ or $3\times$ segment duration, e.g. `4000` for 2s segments). |
| **`buf`** | `sint` (default: `-100`) | Minimum buffer time in ms. Negative values represent percentage of segment duration (`-100` = 100% of `segdur`, `-150` = 150%). |

---

## 3. The `[Dasher] AS-1 Rep video_720p segment X done TOO LATE by Y ms` Warning

### 3.1. Exact Formula & Code Analysis
In `src/filters/dasher.c` (lines 8865–8895):
```c
#ifndef GPAC_DISABLE_LOG
if (ctx->dmode >= GF_DASH_DYNAMIC) {
    u32 asid;
    s64 ast_diff;
    u64 seg_ast = ctx->mpd->availabilityStartTime;
    seg_ast += ctx->current_period->period->start;
    seg_ast += gf_timestamp_rescale(base_ds->adjusted_next_seg_start, base_ds->timescale, 1000);

    // if theoretical AST of the segment is less than the current UTC, we are producing the segment too late.
    ast_diff = (s64) dasher_get_utc(ctx);
    ast_diff -= seg_ast;

    asid = base_ds->set->id;
    if (!asid)
        asid = gf_list_find(ctx->current_period->period->adaptation_sets, base_ds->set) + 1;

    if (ast_diff > 10) {
        GF_LOG(GF_LOG_WARNING, GF_LOG_DASH, ("[Dasher] AS%d Rep %s segment %d done TOO LATE by %d ms\n", asid, base_ds->rep->id, base_ds->seg_number, (s32) ast_diff));
    } else {
        GF_LOG(GF_LOG_INFO, GF_LOG_DASH, ("[Dasher] AS%d Rep %s segment %d done %d ms %s UTC due time\n", asid, base_ds->rep->id, base_ds->seg_number, ABS(ast_diff), (ast_diff<0) ? "before" : "after"));
    }
}
#endif
```

### 3.2. Mathematical Breakdown
1. **Segment Availability End Time ($\text{seg\_ast}$):**
   $$\text{seg\_ast} = \text{MPD.availabilityStartTime} + \text{Period.start} + \left(\frac{\text{adjusted\_next\_seg\_start} \times 1000}{\text{timescale}}\right)$$
   - `MPD.availabilityStartTime`: UTC epoch timestamp (in ms) when the dynamic DASH session was established.
   - `adjusted_next_seg_start`: Media timestamp of the end of the current segment (start of next segment).
   - $\text{seg\_ast}$ represents the **exact UTC wall-clock time at which the media timeline contained in this segment expires in real-world time**.

2. **Availability Time Delta ($\text{ast\_diff}$):**
   $$\text{ast\_diff} = \text{UTC}_{\text{current}} - \text{seg\_ast}$$
   - Where $\text{UTC}_{\text{current}} = \text{dasher\_get\_utc}(\text{ctx})$.

3. **Warning Condition:**
   $$\text{ast\_diff} > 10\text{ ms}$$
   If current UTC wall-clock time has progressed more than 10 ms past the segment's media end time, GPAC logs:
   `[Dasher] AS-1 Rep video_720p segment X done TOO LATE by Y ms`

### 3.3. Primary Causes in Live Pipe Ingest
As clarified by GPAC author Jean Le Feuvre (`jeanlf`) in GitHub Issue #2907:
> *"You cannot disable this warning, it is raised when the end of the segment is detected later than its availability time."*

1. **Initial Timestamp Skew:** If GPAC starts and initializes `availabilityStartTime` at wall-clock time $T_0$, but the first video keyframe arrives at $T_0 + 1500\text{ ms}$ (due to upstream encoder warm-up or pipe negotiation), the media timeline lags behind wall clock by 1500 ms from the start.
2. **GOP Duration Exceeding `segdur`:** If `segdur=2.0` but the encoder sends keyframes every 2.5 seconds, GPAC cannot close the segment until the keyframe arrives at second 2.5. By then, the 2.0s segment is already 500 ms late.
3. **Cross-Stream Sync Hold:** If video is waiting on audio packets stuck in an OS pipe buffer, wall-clock time continues to advance while GPAC waits, causing $\text{ast\_diff}$ to accumulate linearly.

---

## 4. Recommended GPAC Dasher Filter Configuration for Real-Time Live Packaging

To achieve strictly real-time packaging from an fMP4 pipe without segment stalling, buffer accumulation, or stream-close flushes, configure the `dasher` filter with the following parameters:

```bash
dasher:segdur=2:profile=live:dmode=dynauto:cmaf=cmfc:sbound=out:check_dur=false:seg_sync=no:strict_sap=off:spd=4000:buf=-150:asto=0
```

### Breakdown of Flags:
1. **`cmaf=cmfc`**:
   Enforces clean CMAF track separation. Prevents GPAC from attempting representation multiplexing (`muxed_base`) and decouples audio and video segmentation logic.
2. **`sbound=out`**:
   Ensures segments are split immediately when the theoretical boundary is reached at the next SAP without GOP lookahead buffering. Never use `sbound=closest` or `sbound=in` in live pipelines.
3. **`check_dur=false`**:
   Disables cross-representation duration equality validation. Allows audio (21.33ms frames) and video (33.33ms frames) to emit segments independently without waiting to reconcile fractional millisecond differences.
4. **`seg_sync=no`**:
   Bypasses the synchronous file write destruction barrier (`force_seg_sync`), enabling immediate manifest updates. (In disk-backed setups, ensure filesystem write caches or ramdisks prevent client 404 races).
5. **`strict_sap=off`**:
   Permits audio to segment cleanly without demanding video-style SAP 1 keyframe properties.
6. **`profile=live:dmode=dynauto`**:
   Generates live `SegmentTemplate` manifests with dynamic-to-static conversion upon stream end of session (EOS).
7. **`spd=4000:buf=-150`**:
   Signals a safe 4-second suggested presentation delay and 150% segment buffer time to client players to accommodate live network jitter without stalling.
