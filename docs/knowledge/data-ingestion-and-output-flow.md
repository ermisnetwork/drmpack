# Comprehensive Data Flow Architecture: Ingestion & Output Pipeline in drmpack

> **In-Depth Reference Document on Data Flow Architecture**  
> **Applies to:** `drmpack` Core Engine, `media-server` Ingestion Plane, E2E CDN Distribution & Web Playback.  
> **Primary Sources:** `src/session/`, `src/gpac/`, `src/key/`, `examples/common/`, `docs/adr/`.

---

## 1. Data Pipeline Architecture Overview

`drmpack` is a high-performance Rust library functioning as an orchestrator that coordinates live DRM media packaging and generates MPEG-DASH / HLS distribution streams. The core system design adheres to four foundational architectural principles:

1. **Zero-Copy Ingestion via Unix Anonymous Pipes:** All ingested fMP4 data is pushed directly from caller process memory into GPAC subprocesses via kernel anonymous pipes (`ChildStdin`), completely eliminating temporary disk writes for ingested media.
2. **Subprocess Isolation & Process Supervisor:** GPAC runs as an independent subprocess monitored by an asynchronous `ProcessSupervisor` (`tokio::spawn`), isolating the Rust runtime from native C/C++ memory leaks or crash vulnerabilities.
3. **Manifest & Chunk Distribution via Ramdisk (`/dev/shm` / `tmpfs`):** Output data (manifests `.mpd` / `.m3u8` and media segments `.m4s`) are written directly to shared memory ramdisk to eliminate disk I/O latency and write amplification under high-frequency micro-segment updates (200ms – 2s).
4. **Decoupled Finalization & RAII Cleanup:** Stream finalization (`close()`) is decoupled from physical storage cleanup (`cleanup()`), protecting CDN edge workers from missing files while actively serving viewers, backed by an RAII guard (`Drop`) to prevent memory leaks.

### End-to-End Pipeline Architecture Diagram

```mermaid
flowchart TD
    subgraph INGESTION["1. INGESTION DATA PLANE (Caller -> drmpack)"]
        Source["Upstream Source / Transcoder<br/>(FFmpeg, GStreamer, Live Encoder)"]
        Feeder["Caller Ingestion Interface<br/>(media_feeder.rs / media-server)"]
        PushAPI["PackagingSession::push(&[u8])<br/>session.ingest_stream(rx)"]
        MoofDetect{"Inspect Box Header<br/>slice.windows(4) == b'moof'"}
        Cluster["RepresentationCluster::write_data()<br/>tokio::join! concurrency"]
        
        Source -->|Raw fMP4 byte stream| Feeder
        Feeder -->|push / ingest_stream| PushAPI
        PushAPI --> MoofDetect
        MoofDetect -->|Mark has_pushed_media| Cluster
    end

    subgraph ENGINE["2. GPAC SUBPROCESS ENGINE (OS Kernel & Filter Graph)"]
        RepCenc["Representation (CENC)<br/>GpacProcess"]
        RepCbcs["Representation (CBCS)<br/>GpacProcess"]
        PipeCenc[("Unix Anonymous Pipe<br/>ChildStdin fd=0")]
        PipeCbcs[("Unix Anonymous Pipe<br/>ChildStdin fd=0")]
        
        GpacCencEngine["GPAC Process (CENC)<br/>Filter 1: stdin:ext=mp4<br/>Filter 2: cecrypt (cenc.xml)<br/>Filter 3: dasher (live.mpd)"]
        GpacCbcsEngine["GPAC Process (CBCS)<br/>Filter 1: stdin:ext=mp4<br/>Filter 2: cecrypt (cbcs.xml)<br/>Filter 3: dasher (live.mpd)"]
        
        Cluster -->|write_data()| RepCenc
        Cluster -->|write_data()| RepCbcs
        RepCenc -->|write_all & flush| PipeCenc
        RepCbcs -->|write_all & flush| PipeCbcs
        PipeCenc --> GpacCencEngine
        PipeCbcs --> GpacCbcsEngine
    end

    subgraph STORAGE["3. STORAGE TOPOLOGY (Ramdisk & Control Plane)"]
        ControlDir["Private Control Directory<br/>/tmp/drmpack-control/... (mode 0700)<br/>- cenc.xml (mode 0600)<br/>- cbcs.xml (mode 0600)"]
        Ramdisk[("Ramdisk Storage (/dev/shm)<br/>output_dir: /dev/shm/drmpack_{id}_{uuid}/")]
        
        SubCenc["output_dir/cenc/<br/>- live.mpd (DASH Manifest)<br/>- live.m3u8 (Master HLS)<br/>- video_720p.m3u8, audio_eng.m3u8<br/>- video_720p_init.mp4, audio_eng_init.mp4<br/>- video_720p_1.m4s, audio_eng_1.m4s"]
        SubCbcs["output_dir/cbcs/<br/>- live.mpd (DASH Manifest)<br/>- live.m3u8 (Master HLS)<br/>- video_720p.m3u8, audio_eng.m3u8<br/>- video_720p_init.mp4, audio_eng_init.mp4<br/>- video_720p_1.m4s, audio_eng_1.m4s"]
        
        ControlDir -.->|Read DRM Keys| GpacCencEngine
        ControlDir -.->|Read DRM Keys| GpacCbcsEngine
        GpacCencEngine -->|Atomic FS Writes| SubCenc
        GpacCbcsEngine -->|Atomic FS Writes| SubCbcs
        SubCenc --- Ramdisk
        SubCbcs --- Ramdisk
    end

    subgraph EGRESS["4. EGRESS & CONSUMER PLANE"]
        Publisher["CDN Publisher (cdn_publisher.rs)<br/>- File watcher / Ticker poll<br/>- Multi-track parity alignment<br/>- Atomic tmp rename<br/>- Manifest sanitization"]
        CdnStorage[("CDN Edge / Target Storage<br/>(scratch/cdn_storage/)")]
        HttpServer["Playback HTTP Server (playback_server.rs)<br/>- Range request handling (206)<br/>- Origin Shield Grace Window (5.0s)<br/>- CORS & Cache headers"]
        ClientPlayer["Consumers / Web Players<br/>- Shaka Player (Chrome/Edge/Firefox -> Widevine)<br/>- Safari Native / FairPlay CDM"]
        
        Ramdisk -->|Read files| Publisher
        Publisher -->|Atomic publish| CdnStorage
        CdnStorage -->|Serve Static / Ranges| HttpServer
        HttpServer -->|HTTP GET / HEAD / Range| ClientPlayer
    end
```

---

## 2. Data Ingestion / Input Pipeline

The data ingestion pipeline ingests continuous byte streams from the caller, inspects baseline ISO-BMFF box structures to monitor stream progress, fans out data concurrently across target representations, and dispatches data safely into Unix pipes.

### 2.1 Caller-Side Ingestion APIs

Based on the implementation in [`src/session/mod.rs`](../../src/session/mod.rs#L420-L487) and architectural decisions [ADR-0012](../adr/0012-streamlined-api-ergonomics-and-lifecycle.md) and [ADR-0013](../adr/0013-lean-rendition-and-track-id-disambiguation.md), the legacy composite `Segment` struct has been completely removed. Callers now interact exclusively through three zero-copy, byte-level APIs:

```rust
// Extracted from src/session/mod.rs:420-487

/// 1. Primary Ingestion API: Push a discrete byte chunk or media chunk directly
pub async fn push(&mut self, bytes: impl AsRef<[u8]>) -> Result<()> {
    let slice = bytes.as_ref();
    let is_media = slice.windows(4).any(|w| w == b"moof");
    self.push_data(slice, is_media).await
}

/// 2. Streaming Channel Ingestion API: Consume a stream continuously from a tokio mpsc channel
pub async fn ingest_stream(&mut self, mut rx: mpsc::Receiver<Bytes>) -> Result<u64> {
    let mut count = 0u64;
    while let Some(chunk) = rx.recv().await {
        self.push(chunk).await?;
        count += 1;
    }
    Ok(count)
}

/// 3. High-Level Batch / Run-to-Completion API: Ingest until EOF and gracefully close
pub async fn run_to_completion(mut self, rx: mpsc::Receiver<Bytes>) -> Result<PackagingResult> {
    let segments_ingested = self.ingest_stream(rx).await?;
    // ... Prepare manifest paths ...
    self.close().await?;
    self.preserve_output = true;
    Ok(PackagingResult { ... })
}
```

#### Comparison of Ingestion Methods:

| Method | Input Parameter | Transmission Mechanism | Typical Use Case |
| :--- | :--- | :--- | :--- |
| `push(impl AsRef<[u8]>)` | `&[u8]`, `Vec<u8>`, `Bytes`, etc. | Direct async invocation, zero-copy, no new heap allocations | Caller receives buffers from socket/transcoder and immediately pushes into session. |
| `ingest_stream(Receiver<Bytes>)` | `tokio::sync::mpsc::Receiver<Bytes>` | Asynchronous pull over bounded channel buffer | Direct coupling between network/demuxer read tasks and DRM packaging tasks. |
| `run_to_completion(Receiver<Bytes>)` | `tokio::sync::mpsc::Receiver<Bytes>` | Executes full lifecycle: Ingest -> Close -> Return Manifests | Processing VoD files or pre-packaged bounded live segments. |

### 2.2 End-to-End Data Journey Across Architectural Layers

When a caller invokes `session.push(bytes)`, data traverses the architectural layers in the following strict order:

```text
Caller (e.g. MediaFeeder / media-server)
  │  push(&[u8])
  ▼
PackagingSession (src/session/mod.rs:489-498)
  │  - ensure_active(): Verify lifecycle state & cancellation_token
  │  - ping_heartbeat(): Signal watchdog to prevent inactivity timeout
  │  - Inspect 'moof' box: Update has_pushed_media = true
  │  - cluster.write_data(bytes).await
  ▼
RepresentationCluster (src/session/cluster.rs:197-223)
  │  - tokio::join!(rep_cenc.write_data(bytes), rep_cbcs.write_data(bytes))
  │  - Concurrent lock-free fan-out across representations
  ▼
Representation (src/session/cluster.rs:54-56)
  │  - Acquire GpacProcess mutex: gpac.lock().await
  │  - gpac.write_data(bytes).await
  ▼
GpacProcess (src/gpac/process.rs:288-305)
  │  - check_status(): Confirm child process is running
  │  - stdin.write_all(data).await: Write to Tokio async pipe buffer
  │  - stdin.flush().await: Flush data into OS kernel buffer
  ▼
OS Kernel Anonymous Pipe (Unix Pipe Buffer: ChildStdin)
  │  - Copy data into kernel pipe ring buffer (Linux/macOS)
  ▼
GPAC Executable Process (Filter Engine)
     - Filter `stdin:ext=mp4` consumes data from file descriptor 0
```

### 2.3 Detailed Ingestion Technical Analysis

#### A. Identifying Init Segment vs. Media Fragment (`moof` Box Inspection)

Under ISO-BMFF (ISO/IEC 14496-12) and CMAF (ISO/IEC 23000-19):
- **Initialization Segment:** Comprises `ftyp` (File Type) and `moov` (Movie Metadata containing `trak` definitions, `stsd` codec parameters, SPS/PPS, timing). This segment contains **no media samples**.
- **Media Fragment:** Comprises a pair of `moof` (Movie Fragment containing `mfhd`, `traf`, `tfhd`, `trun`) and `mdat` (Media Data containing video NALUs or AAC audio frames) boxes.

In [`src/session/mod.rs:425`](../../src/session/mod.rs#L425):
```rust
let is_media = slice.windows(4).any(|w| w == b"moof");
```

**Architectural Purpose:**
1. **Stream Progress Tracking:** When `is_media == true` is detected, the session sets an atomic flag:
   ```rust
   self.has_pushed_media.store(true, Ordering::Release);
   ```
2. **Integrity Validation on Stream Closure (`verify_hls_endlist`):** GPAC running with `dmode=dynauto` generates segment lists and appends `#EXT-X-ENDLIST` to HLS playlists only if at least one complete media segment was pushed through. If a caller only pushes `init.mp4` and immediately calls `close()`, GPAC produces no segments. The `has_pushed_media` flag enables `session.close()` to distinguish between an empty session (where `#EXT-X-ENDLIST` is not required) and an actual stream failure (where media was pushed but `#EXT-X-ENDLIST` is missing), preventing false-positive errors.

#### B. Backpressure Mechanics & Async Write Loop

Unlike unconstrained in-memory buffering architectures that risk OOM crashes, `drmpack` leverages the operating system's native backpressure mechanics:

1. **Kernel Pipe Buffer:** On Linux, the anonymous pipe between `ChildStdin` and the GPAC child process has a default capacity of 64 KB (governed by kernel `F_SETPIPE_SZ`).
2. **Non-blocking Tokio Reactor:** In [`src/gpac/process.rs:292-298`](../../src/gpac/process.rs#L292-L298):
   ```rust
   if let Some(ref mut stdin) = self.stdin {
       if let Err(e) = stdin.write_all(data).await {
           return Err(self.map_stdin_io_error("write to", e).await);
       }
       if let Err(e) = stdin.flush().await {
           return Err(self.map_stdin_io_error("flush", e).await);
       }
       Ok(())
   }
   ```
3. **Backpressure Propagation Chain:**
   - If the GPAC process lags (e.g. under high CPU load from AES encryption or I/O stalls), GPAC ceases reading from file descriptor 0 (`stdin`).
   - The 64 KB kernel pipe buffer fills up immediately.
   - Tokio's `stdin.write_all(data).await` receives `EWOULDBLOCK` / `EAGAIN` from the `write()` syscall. The Tokio reactor suspends this write future and yields execution back to the Tokio event loop (kqueue/epoll).
   - `RepresentationCluster::write_data` and `session.push` are held in an awaiting state.
   - If the caller uses a bounded channel such as `mpsc::channel(16)` (as in `examples/02_low_latency_dual_cmaf.rs:60` or `media-server`), the 16-item channel buffer fills, causing `tx.send(chunk).await` on the upstream encoder task to suspend.
   - **Result:** Upstream live encoding generation rate is naturally throttled to match GPAC's actual throughput, entirely avoiding memory bloat without complex custom queue management.

#### C. Concurrency & Fan-Out in Dual Mode (CENC vs. CBCS)

Per [ADR-0006](../adr/0006-dual-cenc-cbcs-representations.md), `Dual` is not a single encryption scheme, but an orchestration mode running two GPAC subprocesses in parallel: one performing CENC encryption (serving Android/PC/PlayReady) and one performing CBCS encryption (serving iOS/macOS/FairPlay).

In [`src/session/cluster.rs:197-223`](../../src/session/cluster.rs#L197-L223), data is fanned out concurrently to both subprocesses using `tokio::join!`:

```rust
pub async fn write_data(&self, bytes: &[u8]) -> Vec<RepresentationFailure> {
    let write_results = match self.representations.as_slice() {
        [] => Vec::new(),
        [rep] => vec![rep.write_data(bytes).await],
        [first, second] => {
            // Execute both write branches concurrently
            let (first_result, second_result) =
                tokio::join!(first.write_data(bytes), second.write_data(bytes),);
            vec![first_result, second_result]
        }
        reps => { ... }
    };
    // Collect and transform failures if any representation failed
    write_results
        .into_iter()
        .filter_map(|(scheme, result)| {
            result.err().map(|error| {
                RepresentationFailure::new(scheme, PackagingOperation::Write, error)
            })
        })
        .collect()
}
```

**Symmetric Fail-Fast Policy:**
A Dual session functions as an atomic unit of work. If one branch (e.g. GPAC CENC) crashes, the system never allows the surviving branch (CBCS) to continue operating in isolation, avoiding split-brain live streams across client device populations.

In [`src/session/cluster.rs:333-349`](../../src/session/cluster.rs#L333-L349) and [`src/session/mod.rs:1195-1220`](../../src/session/mod.rs#L1195-L1220):
- `ProcessSupervisor` detects abnormal termination of the CENC branch.
- Supervisor invokes `cluster.abort_peers(failed_scheme).await`.
- The CBCS branch immediately receives `kill()` (SIGKILL) and is reaped within 500ms.
- The overall session transitions to `SessionState::Failed` and yields a consolidated `DrmpackError::PackagingSession`.

#### D. Broken Pipe Handling & Crash Diagnostics (`map_stdin_io_error`)

When GPAC suffers an unrecoverable internal error (e.g. malformed fMP4 NALU structure, invalid SPS/PPS parameters, or missing DRM XML keys), GPAC closes its `stdin` and exits.

Consequently, subsequent Rust writes to the pipe trigger an OS I/O error: `Broken pipe (os error 32)`. Reporting only "Broken pipe" provides zero insight into the root cause.

`drmpack` resolves this through `map_stdin_io_error` in [`src/gpac/process.rs:307-318`](../../src/gpac/process.rs#L307-L318):

```rust
async fn map_stdin_io_error(&self, op: &str, err: std::io::Error) -> DrmpackError {
    // 1. Await up to 50ms for ProcessSupervisor to receive the exit status from child.wait()
    if self.status_rx.borrow().is_none() {
        let mut rx = self.status_rx.clone();
        let _ = tokio::time::timeout(Duration::from_millis(50), rx.changed()).await;
    }
    // 2. Extract process exit code
    let exit_code = self.status_rx.borrow().as_ref().and_then(|s| s.code);
    // 3. Read back the in-memory ring buffer of the last 64 stderr lines
    let stderr = self.get_recent_stderr();
    // 4. Return ProcessCrashed error populated with GPAC's actual error message
    DrmpackError::ProcessCrashed {
        exit_code,
        stderr: format!("Failed to {op} GPAC stdin: {err}. Stderr: {stderr}"),
    }
}
```

---

## 3. Packaging & Output Pipeline

When fMP4 bytes enter the `stdin` pipe, the GPAC child process processes data through its configured Filter Graph.

### 3.1 GPAC Filter Graph Structure

In [`src/gpac/process.rs:110-146`](../../src/gpac/process.rs#L110-L146), `GpacProcessConfig::build_args()` constructs GPAC's filter graph consisting of three cascading filters:

```text
[stdin:ext=mp4] ──(demuxed PIDs)──> [cecrypt] ──(encrypted PIDs)──> [dasher] ──> Ramdisk (/dev/shm)
```

#### 1. Ingestion & Demuxing Filter: `stdin:ext=mp4:alltk:...`
```text
stdin:ext=mp4:alltk:#Representation=(video)video_$Height$p,(video)video,(audio)(Language=!und)audio_$Language$,(audio)audio,(text)(Language=!und)sub_$Language$,(text)sub:#HLSPL=(video)video_$Height$p.m3u8,(video)video.m3u8,(audio)(Language=!und)audio_$Language$.m3u8,(audio)audio.m3u8,(text)(Language=!und)sub_$Language$.m3u8,(text)sub.m3u8
```
- `stdin:ext=mp4`: Specifies source data from standard input formatted as fMP4 (fragmented MP4).
- `alltk`: Instructs GPAC to demux and process all tracks present in the stream (video, multiple audio tracks, subtitles).
- `#Representation=...`: Assigns DASH MPD Representation IDs based on actual track properties (e.g., `video_720p`, `video_1080p`, `audio_eng`, `sub_spa`).
- `#HLSPL=...`: Assigns corresponding HLS Variant Playlist filenames (e.g., `video_720p.m3u8`, `audio_eng.m3u8`).
- *Automatic Metadata Extraction (ADR-0012 & ADR-0013):* GPAC inspects container `moov`/`stsd`/`mdhd` boxes to determine codec (AVC1/H.264, MP4A/AAC), sample rate, width, height, and bitrate automatically. Rust does not need redundant metadata parameter configuration.

#### 2. DRM Encryption Filter: `cecrypt:cfile=<drm_xml_path>`
The `cecrypt` filter performs Common Encryption (CENC) on individual media packets per the XML configuration generated by `GpacDrmXmlGenerator` ([`src/gpac/xml.rs:76-170`](../../src/gpac/xml.rs#L76-L170)).

- **CENC Mode (`EncryptionScheme::Cenc`):** Uses AES-128 in Counter (CTR) mode. Target platforms: Widevine (Android/Chrome) and PlayReady (Windows/Edge). The generated XML injects `pssh` boxes into `moov`/`moof` headers.
- **CBCS Mode (`EncryptionScheme::Cbcs`):** Uses AES-128 in Cipher Block Chaining (CBC) mode with 10% Pattern Encryption.
  - Video Tracks: Requires `crypt_byte_block="1" skip_byte_block="9"` (encrypt 1 16-byte block, skip 9 blocks).
  - Audio Tracks: Apple FairPlay and ISO/IEC 23001-7 strictly prohibit pattern encryption on audio; audio must configure `crypt_byte_block="0" skip_byte_block="0"` (full block encryption) ([`src/gpac/xml.rs:207-213`](../../src/gpac/xml.rs#L207-L213)).
  - Constant IV: Uses a fixed 16-byte IV (`first_IV="0x..."`).
  - HLS Signaling: Injects `hlsInfo` tags containing `KEYFORMAT="com.apple.streamingkeydelivery"` and URI `skd://...`. FairPlay streams must never generate PSSH boxes in MP4 containers ([`src/gpac/xml.rs:96-98`](../../src/gpac/xml.rs#L96-L98)).

#### 3. Packaging & Distribution Filter: `dasher:...`
```text
<output_dir>/live.mpd:dual:profile=live:dmode=dynauto:segdur=2:spd=4000:tsb=1800:utcs=inband:pssh=mv:template=$RepresentationID$_$Init=init$$Number$[:cdur=0.2:asto=0.0:llhls=br:cmaf=cmfc]
```

Detailed parameter breakdown for `dasher`:
- `:dual`: Requests GPAC to generate both DASH manifests (`live.mpd`) and Master HLS playlists (`live.m3u8`) concurrently from the same packaging pipeline.
- `profile=live`: DASH-IF live streaming profile.
- `dmode=dynauto`: Dynamic-auto live mode. Manifests remain dynamic during live streaming. When stdin receives EOF, GPAC automatically transitions manifests to static (VoD) and appends `#EXT-X-ENDLIST` to HLS playlists.
- `segdur=2.0`: Sets target segment duration to 2.0 seconds (aligned with upstream live encoder GOP).
- `spd=4000`: `suggestedPresentationDelay` = 4000ms (2.0x `segdur`). Per DASH-IF IOP v4.3, provides a 4-second safety buffer for client players to prevent 404 stalls when chasing the live edge.
- `tsb=1800`: Time-Shift Buffer of 1800 seconds (30 minutes). GPAC maintains a 30-minute DVR sliding window and automatically deletes older segments from disk.
- `utcs=inband`: Embeds synchronized UTC wall-clock timestamps within the media container.
- `pssh=mv`: Embeds `pssh` boxes in both initialization headers (`moov`) and fragment headers (`moof`).
- `template=$RepresentationID$_$Init=init$$Number$`: File naming scheme: init segment is `{rep_id}_init.mp4`, media segment is `{rep_id}_{number}.m4s`.
- *Low-Latency Mode Parameters (`LatencyMode::LowLatency`):*
  - `:cdur=0.2`: CMAF Chunk duration of 200ms.
  - `:asto=0.0`: Availability Time Offset (defaults to 0.0s on file-staged origins to prevent 404 races without chunked HTTP transfer).
  - `:llhls=br`: Activates Apple Low-Latency HLS byte-range partial segments.
  - `:cmaf=cmfc`: Enforces CMAF Chunk packaging profile compliance.

---

### 3.2 Storage Topology

The system maintains a clean separation between public ramdisk storage and private security control files:

```text
/dev/shm/ (or $TMPDIR on macOS)
 ├── drmpack-control/                                  <-- PRIVATE CONTROL DIR (mode 0700)
 │    └── {content_id}_{uuid}/
 │         ├── cenc.xml                                <-- GPAC DRM XML (mode 0600, contains secret Content Keys)
 │         └── cbcs.xml                                <-- GPAC DRM XML (mode 0600, contains secret Content Keys)
 │
 └── drmpack_{content_id}_{uuid}/                      <-- PUBLIC OUTPUT DIR (mode 0755, exposed to Web/CDN)
      │
      ├── [Single Scheme Case: CENC or CBCS]
      │    ├── live.mpd                                <-- DASH Manifest
      │    ├── live.m3u8                               <-- Master HLS Playlist
      │    ├── video_720p.m3u8                         <-- Variant HLS Video Playlist
      │    ├── audio_eng.m3u8                          <-- Variant HLS Audio Playlist
      │    ├── video_720p_init.mp4                     <-- Video Initialization Segment
      │    ├── audio_eng_init.mp4                      <-- Audio Initialization Segment
      │    ├── video_720p_1.m4s                        <-- Video Media Fragment #1 (moof + mdat)
      │    ├── video_720p_2.m4s                        <-- Video Media Fragment #2
      │    ├── audio_eng_1.m4s                         <-- Audio Media Fragment #1
      │    └── audio_eng_2.m4s                         <-- Audio Media Fragment #2
      │
      └── [Dual Scheme Case: Dual CENC & CBCS]
           ├── cenc/                                   <-- CENC Output Tree (Android / Windows / Chrome)
           │    ├── live.mpd
           │    ├── live.m3u8
           │    ├── video_720p.m3u8
           │    ├── video_720p_init.mp4
           │    └── video_720p_1.m4s ...
           │
           └── cbcs/                                   <-- CBCS Output Tree (Apple iOS / Safari / FairPlay)
                ├── live.mpd
                ├── live.m3u8
                ├── video_720p.m3u8
                ├── video_720p_init.mp4
                └── video_720p_1.m4s ...
```

---

## 4. Generated Output File Structure Details

### 4.1 DASH Manifest (`live.mpd`)
The DASH MPD is an XML document describing adaptive bitrate (ABR) presentation structures:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011"
     profiles="urn:mpeg:dash:profile:isoff-live:2011"
     type="dynamic"
     availabilityStartTime="2026-09-08T03:45:00Z"
     suggestedPresentationDelay="PT4.000S"
     minBufferTime="PT2.000S">
  <Period id="P0" start="PT0S">
    <!-- Video Adaptation Set -->
    <AdaptationSet contentType="video" mimeType="video/mp4" segmentAlignment="true">
      <!-- DRM Protection Descriptor (Widevine) -->
      <ContentProtection schemeIdUri="urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed">
        <cenc:pssh>AAAAPnBzc2gBAAAAr...</cenc:pssh>
      </ContentProtection>
      <!-- DRM Protection Descriptor (Common Encryption) -->
      <ContentProtection schemeIdUri="urn:mpeg:dash:mp4protection:2011" value="cenc" cenc:default_KID="..."/>

      <SegmentTemplate timescale="1000"
                       duration="2000"
                       initialization="$RepresentationID$_init.mp4"
                       media="$RepresentationID$_$Number$.m4s"
                       startNumber="1"/>
      <Representation id="video_720p" width="1280" height="720" bandwidth="2500000" codecs="avc1.64001f"/>
      <Representation id="video_1080p" width="1920" height="1080" bandwidth="5000000" codecs="avc1.640028"/>
    </AdaptationSet>

    <!-- Audio Adaptation Set -->
    <AdaptationSet contentType="audio" mimeType="video/mp4" segmentAlignment="true" lang="eng">
      <SegmentTemplate timescale="1000"
                       duration="2000"
                       initialization="$RepresentationID$_init.mp4"
                       media="$RepresentationID$_$Number$.m4s"
                       startNumber="1"/>
      <Representation id="audio_eng" bandwidth="128000" codecs="mp4a.40.2"/>
    </AdaptationSet>
  </Period>
</MPD>
```

### 4.2 Master HLS Playlist (`live.m3u8`)
The master playlist (Multivariant Playlist) declares available bitrate variants and binds audio/subtitle groups. Conforming to RFC 8216 and Apple HLS Authoring Specifications, `#EXT-X-MEDIA` tags precede `#EXT-X-STREAM-INF`:

```m3u8
#EXTM3U
#EXT-X-VERSION:7
#EXT-X-INDEPENDENT-SEGMENTS

# 1. Audio Rendition declaration (Must precede stream inf per sanitization rules)
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="audio",NAME="English",DEFAULT=YES,AUTOSELECT=YES,LANGUAGE="eng",URI="audio_eng.m3u8"

# 2. Video Stream declarations linked to Audio Group
#EXT-X-STREAM-INF:BANDWIDTH=2628000,AVERAGE-BANDWIDTH=2500000,RESOLUTION=1280x720,CODECS="avc1.64001f,mp4a.40.2",AUDIO="audio"
video_720p.m3u8

#EXT-X-STREAM-INF:BANDWIDTH=5128000,AVERAGE-BANDWIDTH=5000000,RESOLUTION=1920x1080,CODECS="avc1.640028,mp4a.40.2",AUDIO="audio"
video_1080p.m3u8
```

### 4.3 Variant HLS Playlists (`video_720p.m3u8`)
The media playlist identifies FairPlay decryption keys and references `.m4s` media segment files:

```m3u8
#EXTM3U
#EXT-X-VERSION:7
#EXT-X-TARGETDURATION:2
#EXT-X-MEDIA-SEQUENCE:1

# Apple FairPlay DRM Key declaration (CBCS mode)
#EXT-X-KEY:METHOD=SAMPLE-AES,KEYFORMAT="com.apple.streamingkeydelivery",KEYFORMATVERSIONS="1",URI="skd://14ccfc47-2b81-4320-b0b3-111111111111"

# CMAF Initialization Segment Map
#EXT-X-MAP:URI="video_720p_init.mp4"

# Media Segment List
#EXTINF:2.000000,
video_720p_1.m4s
#EXTINF:2.000000,
video_720p_2.m4s
#EXTINF:2.000000,
video_720p_3.m4s
```

### 4.4 Binary Structure of CMAF Segments

```text
[Initialization Segment: video_720p_init.mp4]
 ├── ftyp (File Type Box: major_brand='cmfc', compatible_brands=['iso6', 'cmfc'])
 └── moov (Movie Box)
      ├── mvhd (Movie Header)
      └── trak (Track Box)
           ├── tkhd (Track Header: track_id=1, width=1280, height=720)
           └── mdia -> minf -> stbl -> stsd (Sample Table Description)
                └── encv (Encrypted Video Sample Entry: avc1)
                     └── sinf (Protection Scheme Information Box)
                          ├── frma (Original Format: avc1)
                          ├── schm (Scheme Type: 'cenc' or 'cbcs')
                          └── schi (Scheme Information Box)
                               └── tenc (Track Encryption Box: default_IsEncrypted=1, default_IV_size=16, default_KID)

[Media Segment: video_720p_1.m4s]
 ├── styp (Segment Type Box: brands=['msdh', 'msix'])
 ├── moof (Movie Fragment Box - Demarcates new media segment)
 │    ├── mfhd (Movie Fragment Header: sequence_number=1)
 │    └── traf (Track Fragment Box)
 │         ├── tfhd (Track Fragment Header: track_id=1)
 │         ├── tfdt (Track Fragment Base Media Decode Time)
 │         ├── senc (Sample Encryption Box: Per-sample IVs or Subsample mapping)
 │         └── trun (Track Run Box: sample_count, sample_sizes, sample_durations)
 └── mdat (Media Data Box: Encrypted video NALUs using AES-CTR or AES-CBC)
```

---

## 5. Egress & Distribution Pipeline

Once GPAC writes segment files into Ramdisk, downstream consumer modules ingest and serve them to playback clients.

### 5.1 CDN Synchronization: `CdnPublisher` (`examples/common/cdn_publisher.rs`)

`CdnPublisher` acts as an intelligent synchronization engine between the intermediate Ramdisk and web server/CDN distribution directories. As implemented in [`examples/common/cdn_publisher.rs:33-294`](../../examples/common/cdn_publisher.rs#L33-L294), the module addresses three major live packaging challenges:

#### 1. Multi-Track Parity Alignment & Skew Tolerance
In live fMP4 packaging, multiplexing between video and audio always incurs small phase drift (50ms – 200ms) due to differing frame boundaries. If video segment #17 completes before audio segment #17 and the publisher immediately pushes the updated playlist to the CDN, players fetching audio #17 will encounter HTTP 404 errors (live-edge 404 stall).

`CdnPublisher` computes `min_common_seg` across all active tracks and applies a safe skew tolerance window (`skew_tolerance = 2` segments):
```rust
// Extracted from examples/common/cdn_publisher.rs:116-138
let min_common_seg = max_seg_per_track.values().copied().min().unwrap_or(u64::MAX);
let skew_tolerance = 2u64;

for src_path in segments_and_data {
    if let Some((_track_id, seg_num)) = parse_track_and_segment(file_name) {
        if min_common_seg < u64::MAX && seg_num > min_common_seg + skew_tolerance {
            // Defer publishing leading segments until lagging tracks catch up
            continue;
        }
    }
    self.sync_file(&src_path, &cur_dst, min_common_seg).await?;
}
```

#### 2. Atomic Write-Rename Pattern
Prevents web clients from fetching partially written media segments.
In [`examples/common/cdn_publisher.rs:186-282`](../../examples/common/cdn_publisher.rs#L186-L282):
- Segments are initially written to a hidden temporary file: `.{filename}_{pid}_{uuid}.tmp`.
- Once 100% of bytes are written and flushed, `tokio::fs::rename()` atomically moves the file to its destination path.

#### 3. Manifest Integrity Gate
- For DASH (`.mpd`): Validates the presence of the closing `</MPD>` tag before publishing. Incomplete files lacking the closing tag are skipped on the current polling cycle.
- For HLS (`.m3u8`): Ensures playlists start with `#EXTM3U`, strips segment lines exceeding `min_common_seg + 2`, and normalizes `#EXT-X-MEDIA` tags to precede `#EXT-X-STREAM-INF` (`sanitize_master_playlist`).

---

### 5.2 Playback Server: `playback_server.rs` (`examples/common/playback_server.rs`)

The HTTP origin server serving the packaging stream supports standard live media delivery features:

1. **HTTP Range Requests (`Range: bytes=start-end`):** Web browsers (especially Safari playing HLS or reading byte-range CMAF parts) issue Range requests. The server parses these with `parse_range_header()` and returns HTTP `206 Partial Content` with `Content-Range: bytes start-end/total`.
2. **Origin Shield Grace Window (Preventing Live-Edge 404s):** In [`examples/common/playback_server.rs:634-644`](../../examples/common/playback_server.rs#L634-L644):
   ```rust
   // If a client requests an .m4s file newly referenced in the manifest before
   // the publisher finishes renaming, the server holds the request for up to 5.0s (100 x 50ms)
   if data_opt.is_none() && rel_path.ends_with(".m4s") {
       for _ in 0..100 {
           tokio::time::sleep(Duration::from_millis(50)).await;
           if let Ok(data) = tokio::fs::read(&file_path).await {
               if !data.is_empty() {
                   data_opt = Some(data);
                   break;
               }
           }
       }
   }
   ```
   This mechanism acts as an in-process Origin Shield, absorbing I/O scheduling delays and network jitter.
3. **MIME Types & Cache-Control:** Returns `application/dash+xml` for `.mpd`, `application/vnd.apple.mpegurl` for `.m3u8`, `video/mp4` for `.m4s` and `.mp4`. Live manifests are served with `Cache-Control: no-cache, no-store, must-revalidate`.

---

## 6. Ramdisk Protection & Lifecycle Management

Continuous live packaging requires strict resource management to prevent exhausting server memory.

### 6.1 Buffer Limiting via Time-Shift Buffer (`tsb=1800`)

In GPAC's dasher configuration ([`src/gpac/process.rs:128`](../../src/gpac/process.rs#L128)), the parameter `tsb=1800` (Time-Shift Buffer of 1800 seconds = 30 minutes) instructs GPAC to:
1. Maintain playlist segment history corresponding to the most recent 30 minutes of playback.
2. Automatically execute segment rotation: `.m4s` files older than 30 minutes are deleted directly from Ramdisk (`/dev/shm`).
3. **Maximum Ramdisk Footprint Estimation:**
   $$\text{RAM Limit} = \text{Total Bitrate of All Renditions} \times 1800\text{ seconds} \times \text{Safety Factor (1.25)}$$
   *Example:* A stream comprising 1080p (5 Mbps) + 720p (2.5 Mbps) + Audio (128 kbps) $\approx 7.63\text{ Mbps} \approx 0.95\text{ MB/s}$.  
   The maximum Ramdisk consumption after 30 minutes is: $0.95\text{ MB/s} \times 1800\text{s} \approx 1.71\text{ GB}$. Usage reaches steady-state equilibrium and ceases growing.

---

### 6.2 Decoupled Lifecycle Model

Per [ADR-0012](../adr/0012-streamlined-api-ergonomics-and-lifecycle.md), `drmpack` completely decouples stream finalization from physical Ramdisk deletion:

```text
PackagingSession::create() ──> [Active Streaming] ──> PackagingSession::close() ──> [Delivery Window] ──> PackagingSession::cleanup()
        │                                                     │                                                    │
        ▼                                                     ▼                                                    ▼
Create /dev/shm/ output                            Close stdin, await GPAC exit                         Completely delete output_dir
Create private control_dir                         Flush manifests, verify ENDLIST                      when CDN finishes distribution
                                                   IMMEDIATELY DELETE control_dir (security)
                                                   PRESERVE output_dir (protect CDN delivery)
```

#### 1. Stream Finalization Phase: `session.close().await` ([`src/session/mod.rs:519-580`](../../src/session/mod.rs#L519-L580))
- Cancels watchdog token (`cancellation_token.cancel()`).
- Transitions `Lifecycle` to `SessionState::Closing`.
- Closes the `stdin` pipe for each GPAC subprocess (sends EOF).
- Awaits GPAC completion within `finalization_timeout` (default 5s). If timed out, supervisor dispatches `SIGKILL`.
- **`#EXT-X-ENDLIST` Verification:** For streams where media fragments were pushed, `verify_hls_endlist(&output_dir)` inspects all `.m3u8` media playlists to ensure `#EXT-X-ENDLIST` was appended. If missing, raises a `RepresentationFailure` to prevent client players from hanging indefinitely.
- **Private Control Dir Cleanup:** The `control_dir` containing Content Key XML files is deleted immediately (`cleanup_control_dir()`).
- **Output Dir Preservation:** The `preserve_output` flag is set to `true`. The `/dev/shm` directory holding media segments is preserved for CDN Edge caching and late-joining viewers.

#### 2. Physical Storage Cleanup: `session.cleanup().await` ([`src/session/mod.rs:620-628`](../../src/session/mod.rs#L620-L628))
- Explicitly invoked by the caller once broadcasting is completed and CDN edges have cached all content.
- Executes `tokio::fs::remove_dir_all(&self.config.output_dir)`.

#### 3. Automatic Safeguard via RAII: `impl Drop for PackagingSession` ([`src/session/mod.rs:778-802`](../../src/session/mod.rs#L778-L802))
To guard against neglected `cleanup()` invocations or runtime panics causing `/dev/shm` leaks:
- If `output_dir` was automatically generated by the library in `/dev/shm` (not explicitly specified by caller via `.with_output_dir()`), and `preserve_output` is false, `drop()` automatically deletes `output_dir`.
- The private `control_dir` is always purged during `drop()`.
- The subprocess supervisor task receives `abort()` and GPAC receives `kill()`.

---

## 7. Core Pipeline Source Code Reference Table

| Feature / Component | Source Code Path | Line Range | Primary Responsibility |
| :--- | :--- | :--- | :--- |
| **Core Ingestion APIs** | `src/session/mod.rs` | L420–L487 | Exposes `push()`, `ingest_stream()`, `run_to_completion()`. |
| **`moof` Box Detection** | `src/session/mod.rs` | L425, L492 | Delineates Init Segments and Media Fragments, sets `has_pushed_media`. |
| **Cluster Ingestion Fan-Out** | `src/session/cluster.rs` | L197–L223 | Concurrent fan-out via `tokio::join!` across CENC and CBCS representations. |
| **Symmetric Abort Peers** | `src/session/cluster.rs` | L333–L349 | Terminates peer branches immediately if any GPAC subprocess crashes. |
| **GPAC CLI Arguments Builder** | `src/gpac/process.rs` | L110–L146 | Assembles filter graph: `stdin:ext=mp4`, `cecrypt`, `dasher:dual`. |
| **Tokio Stdin Async Write** | `src/gpac/process.rs` | L288–L305 | Non-blocking write to OS pipe, propagating kernel backpressure. |
| **Broken Pipe Diagnostics** | `src/gpac/process.rs` | L307–L318 | `map_stdin_io_error` captures exit code and 64 stderr lines on pipe failure. |
| **DRM XML Generator** | `src/gpac/xml.rs` | L76–L170 | Generates XML encryption configs for CENC (CTR) and CBCS (Pattern 1:9 / Constant IV). |
| **`#EXT-X-ENDLIST` Validation** | `src/session/mod.rs` | L984–L1033 | Validates HLS playlists post-closure to ensure termination tags are present. |
| **Multi-Track CDN Sync** | `examples/common/cdn_publisher.rs` | L88–L150 | Computes `min_common_seg` and `skew_tolerance`, synchronizes atomically to CDN. |
| **Origin Shield Grace Window** | `examples/common/playback_server.rs` | L634–L644 | Provides 5.0s grace window for `.m4s` segment requests to eliminate 404s. |
| **RAII Ramdisk Cleanup** | `src/session/mod.rs` | L778–L802 | `Drop` automatically purges `/dev/shm` Ramdisk and `control_dir`. |

---

## 8. Conclusion

The Data Ingestion & Output Pipeline architecture of `drmpack` balances **ultra-high performance (Zero-Copy Pipe Ingestion, Shared Memory Ramdisk)** with **production-grade resilience (Non-blocking Backpressure, Asynchronous Process Supervisor, Symmetric Fail-Fast, Atomic CDN Publishing, Origin Shield Grace Window)**. The entire lifecycle from the moment the caller pushes the first fMP4 byte until web players decrypt and render video operates autonomously, transparently, and with strict resource safety.
