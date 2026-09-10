# GPAC Direct Output & Pure Streaming Sinks: Deep-Dive Research & Feasibility Assessment

> **Deep-Dive Technical Research Document**  
> **Applies to:** `drmpack` Core Engine, `media-server` Ingestion/Egress Plane, GPAC Subprocess Orchestration.  
> **Primary Sources:** GPAC Official Wiki (`wiki.gpac.io`), GPAC Source Tree (`gpac/gpac` filters), CTA-5003 / CTA-WAVE, DASH-IF Live Media Ingest Specification.

---

## 1. Executive Summary

Core question: **"Does GPAC support direct output / pure streaming sink without writing files to disk/filesystem?"**

### Brief Answer:
1. **For single raw streams (Single Track / Elementary Stream / MPEG-TS):** **YES**. GPAC supports direct output to `stdout`, POSIX pipes (`pipe://`), TCP/UDP/Unix Domain Sockets (`sockout`), completely without touching disk.
2. **For adaptive multi-segment HLS/DASH packaging (`dasher`):**
   - **Via raw `stdout` or POSIX anonymous pipe:** **NOT POSSIBLE**. Emitting HLS/DASH to a single pipe faces an insurmountable physical obstacle: HLS/DASH is a **multi-resource document tree** comprising continuously updating dynamic manifests (`live.mpd`, `.m3u8`), init segments, and a series of independent media fragments (`.m4s`) across multiple tracks (audio, video). A pipe is an unstructured, scalar byte stream with no packet framing or channel multiplexing metadata. Forcing `dasher` to a pipe via the `--template=pipe://...` flag per official GPAC docs will **"trash the manifest"** and emit only a raw mux stream.
   - **Via Network Sink without filesystem:** **YES**. GPAC provides the **`httpout:hmode=push`** filter acting as an HTTP Client Sink (compliant with the **DASH-IF Ingest / CMAF Ingest** model), pushing individual segments and manifests directly via HTTP `PUT`/`POST` to a target endpoint without requiring any local storage directory.
   - **Via In-Memory Sink (`gmem://`):** **YES** at the embedded server level: `httpout:rdirs=gmem` allows storing and serving segments directly from RAM (`max_cache_segs`), but this embedded HTTP server runs inside the GPAC process.
   - **Via C API FFI (`libgpac` Custom Filter):** **BLOCKED BY ARCHITECTURAL RESTRICTION**. GPAC developer documentation explicitly mandates: *Custom filters created via `gf_fs_new_filter` CANNOT act as sources or destinations for filters that load graphs dynamically (such as `dasher` or `dashin`)*.

---

## 2. Detailed Technical Analysis per Investigation Vector

---

### Vector 1: Feasibility of Outputting to `stdout` / Anonymous Pipe

#### 1.1. Can `gpac dasher` output to `stdout` or pipe (`pipe://`, `-o stdout`)?
In GPAC, outputting data to file descriptor 1 (`stdout`) is handled by the `fout` (File Output) sink filter when the target flag is `stdout` or `std`.
However, when combined with the `dasher` filter, a severe architectural conflict arises:

* **Actual behavior of `dasher` with `pipe://`:**  
  GPAC provides the forced-template flag `tpl_force` (Forced-Template mode). Exact citation from official GPAC docs (`wiki.gpac.io/Filters/dasher`):
  > *"When `tpl_force` is set, the template string is not analyzed nor modified for missing elements. This is used to trash the manifest and open [pipe] as the destination for the muxer result.  
  > Example: `gpac -i SRC -o null:ext=mpd:tpl_force --template=pipe://mypipe`"*

* **Technical consequence:**  
  This mode **deliberately skips manifest generation** (`live.mpd` / `live.m3u8`) and only redirects muxer payload bytes to a named pipe. This turns the pipeline into a conventional multiplexer (single-stream fMP4), completely losing the adaptive streaming characteristics of DASH/HLS.

#### 1.2. Physical obstacles of outputting HLS/DASH to a single pipe
Why is funneling an entire DASH/HLS session into a single anonymous pipe impossible from a computer science perspective?

1. **Nature of POSIX Anonymous Pipe (`pipe(2)`):**
   - A pipe is a sequential FIFO byte stream with no concept of message boundaries, no hierarchical file tree structure, and no support for random access (`non-seekable`).
2. **Nature of a Live HLS/DASH Session (Dual CENC/CBCS):**
   - A live packaging session concurrently generates at least 5–10 independent file resources:
     - `live.mpd` (XML manifest periodically overwritten with new content).
     - `live.m3u8` (Master playlist).
     - `video_720p.m3u8`, `audio_eng.m3u8` (Variant playlists updated via sliding window).
     - `video_720p_init.mp4`, `audio_eng_init.mp4` (Init boxes `ftyp`+`moov`).
     - `video_720p_1.m4s`, `audio_eng_1.m4s`, `video_720p_2.m4s`... (Media segments).
3. **Interleaved Byte Soup Phenomenon:**
   - Writing everything to one pipe without a demuxing/framing protocol causes manifest XML bytes, video ISO-BMFF bytes, and audio AAC bytes to interleave at the kernel pipe buffer level (64 KB).
   - The consumer (Rust) has no mechanism to:
     - Delineate boundaries: Which bytes belong to which file.
     - Classify data: Whether this is a new video segment or an overwrite of the manifest.
     - Route data: Which chunk goes to a client watching video vs. fetching playlists.

#### 1.3. Does GPAC provide any Framing mechanism (Tar stream, Multipart) over Pipe?
* **Tar Stream:** GPAC does **not** integrate tar or archive creation filters on `dasher`'s pipe output.
* **Multipart/MIME:** GPAC supports Multipart/MIME in the `routeout` filter (sending manifest + S-TSID signaling) or HTTP chunked transfer, but **not** multipart streams over POSIX pipe.
* **GSF (GPAC Serialized Format - `gsfmx`):**  
  GPAC developed its own proprietary binary protocol called **GSF** (`wiki.gpac.io/Filters/gsfmx`).  
  - The `gsfmx` filter can serialize entire PID states, properties (`GF_PROP_PID_FILE_NAME`, `CueStart`), End-Of-Stream events, and payloads into a single binary stream over pipe or socket:  
    `gpac -i source.mp4 gsfmx:dst=manifest.mpd:mixed -o dump.gsf`  
  - **Obstacle:** GSF is an internal proprietary format. Ingesting GSF in Rust via pipe requires writing a complete parser for GSF binary framing (or compiling the C demuxer `gsfdmx`), introducing unjustified maintenance overhead and complexity.

---

## 3. Non-Filesystem Filter Sinks in GPAC

---

### 2.1. `httpout:hmode=push` (HTTP Client Sink - CMAF Ingest Standard)

This is GPAC's standard and most capable mechanism for direct output without touching disk.

#### A. How it works
The `httpout` filter (`wiki.gpac.io/Filters/httpout`) normally operates as a local HTTP Server. However, when configured with `hmode=push`, it transforms into an **HTTP Client**:
* Instead of writing segment and manifest files to disk, `dasher` forwards `FILE`-type PID packets to `httpout`.
* `httpout` initiates outbound HTTP connections using **`PUT`** (default) or **`POST`** (with `post=true`) to push each resource to the destination HTTP server.
* **Official GPAC Docs state:**  
  > *"In push mode, the filter does not need a local read or write directory because it sends data directly to the remote URL."*

#### B. Standard CLI Syntax from Primary Sources
```bash
gpac -i source reframer:rt=on -o http://127.0.0.1:8080/live/live.mpd:gpac:segdur=2:cdur=0.2:profile=live:dmode=dynamic:hmode=push:llhls=br
```
When running this command:
1. `dasher` generates manifest `live.mpd` -> `httpout` issues HTTP request: `PUT /live/live.mpd`.
2. `dasher` generates init segment -> `httpout` issues: `PUT /live/video_init.mp4`.
3. `dasher` generates media segment 1 -> `httpout` issues: `PUT /live/video_1.m4s`.
4. In Low-Latency mode (`cdur=0.2`), `httpout` uses HTTP `Transfer-Encoding: chunked` to stream each CMAF chunk as soon as the muxer finishes it, minimizing latency.

#### C. Industry Standard Alignment
This mechanism adheres directly to the **DASH-IF Live Media Ingest Specification** (also known as **CMAF Ingest**):
* **Interface 1 (CMAF Track Push):** Pushes fragmented CMAF tracks continuously over long-lived HTTP connections.
* **Interface 2 (DASH/HLS Presentation Ingest):** Pushes packaged objects (Manifests + Segments) independently via HTTP PUT/POST to Origin/Packager.

#### D. Performance Evaluation & Practical Trade-offs
* **Pros:** 100% eliminates disk writes; fully compatible with CDN Ingest Origins (such as AWS Elemental MediaStore, Akamai, or custom local Rust servers).
* **Practical Risks & Considerations (from GPAC GitHub Issues #3027, #2923):**
  - **Issue #3027:** HTTPS / linker issues existed in older GPAC versions when handling multiple PIDs (resolved in commit `85083494`). Plain localhost HTTP (`http://127.0.0.1:port`) has been confirmed stable.
  - **Issue #2923:** Note: As documented in `gpac-http-push-feasibility.md`, Issue #2923 was an unrelated TCP/MPEG-TS memory issue. However, maintaining continuous HTTP client connections introduces socket lifecycle and backpressure handling considerations.

---

### 2.2. `sockout` (TCP / UDP / Unix Domain Socket)

The `sockout` filter (`wiki.gpac.io/Filters/sockout`) enables GPAC to open network sockets in blocking mode:
* **Supported protocols:**
  - `tcp://<ip>:<port>` (TCP socket)
  - `udp://<ip>:<port>` (UDP socket)
  - `tcpu://<path>` (TCP Unix Domain Socket on Linux/macOS)
  - `udpu://<path>` (UDP Unix Domain Socket)
* **Capability regarding `dasher`:**  
  `sockout` only accepts single stream PIDs (e.g., elementary video AVC/HEVC, AAC audio, or MPEG-TS via `:ext=ts`). `sockout` **cannot** recognize `FILE`-type PIDs from `dasher` to demux multiple HLS/DASH files. Thus, `sockout` **cannot** be used as a direct sink for adaptive DASH/HLS.

---

### 2.3. `routeout` (ROUTE / FLUTE in ATSC 3.0)

The `routeout` filter (`wiki.gpac.io/Filters/routeout`) implements the ROUTE protocol per ATSC 3.0:
* Receives `FILE`-type PIDs from `dasher` and multicasts them over UDP: `gpac -i DASH_URL -o route://225.1.1.1:1234/manifest.mpd`.
* Not suitable for local unicast microservice server architectures.

---

### 2.4. `gmem://` & GPAC In-Memory Virtual Filesystem (`gfio`)

* When configured with `--rdirs=gmem`, `httpout` activates **Memory Mode**: Segments generated by `dasher` are retained in RAM and served via GPAC's embedded HTTP server.
* The C API layer provides struct `GF_FileIO` with callback functions `gf_fileio_new_mem()`.

---

## 4. Differences Between GPAC CLI (Subprocess) vs `libgpac` (C Library FFI)

### Core Barrier from GPAC Docs (Primary Source Citation)
In GPAC's custom filter tutorial (**"Writing a custom Filter"** - `wiki.gpac.io/Developers/tutorials/Writing-a-custom-Filter/`), the development team highlights immutable restrictions of application filters (`gf_fs_new_filter`):

> **"Limitations:**  
> - *Custom filters cannot have arguments exposed.*  
> - *Custom filters **cannot act as sources or destinations for filters that load graphs dynamically (like `dashin` or `dasher`)**.*  
> - *Custom filters cannot be cloned."*

**Consequence:** You **CANNOT** write a Rust struct, export C callbacks, and attach it as a direct in-memory sink for `dasher`'s output.

---

## 5. Technical Comparison Matrix & Practical Feasibility

| Comparison Criteria | (A) File Staging + Unlink on SSD / tmpfs *(Recommended)* | (B) GPAC HTTP PUSH to Localhost Rust Server (`httpout:hmode=push`) | (C) Custom In-Memory Sink (GSF Pipe / `libgpac` FFI) |
| :--- | :--- | :--- | :--- |
| **I/O Nature** | Writes to Linux Page Cache RAM -> Rust reads -> Unlinks. | Transmits over TCP Loopback Socket in RAM. Zero filesystem. | Transmits over binary Pipe or C-Rust memory pointers. |
| **Latency** | **Ultra-low (< 1ms)**: Syscall `write()` into kernel RAM page. | **Very low (1-3ms)**: Overhead from HTTP headers & loopback TCP stack. | **Ultra-low (< 0.5ms)** (if using FFI pointers). |
| **Filesystem Dependency** | Yes (requires a small temporary directory). | **Completely independent** of filesystem. | Completely independent of filesystem. |
| **Architectural Complexity** | **Very simple**: Leverages standard GPAC CLI flags, minimal code. | **Medium to High**: Rust must host an HTTP server accepting `PUT`, manage ports, handle connection resets. | **Extremely high / Blocked by GPAC architecture**. |
| **Stability & Fault Isolation** | **Absolute**: GPAC crashes are caught by `ProcessSupervisor`. | **Good**: Slight risk of GPAC HTTP client edge cases. Ports can collide. | **Poor / Dangerous**: C FFI crashes crash the Rust process; parsing binary stream prone to desync. |
| **Dual CENC + CBCS Support** | **Seamless**: 2 processes write to separate `cenc/` and `cbcs/` subdirectories. | **Complex**: Needs routing 2 distinct URL paths (`/cenc/..`, `/cbcs/..`) on HTTP server. | Very complex to multiplex 2 concurrent streams. |
| **Standards Compatibility** | Standard POSIX File System & DASH/HLS File Structure. | Standard **CMAF Ingest (DASH-IF Ingest Interface 2)**. | Proprietary GPAC format. |

---

## 6. Architectural Evaluation Under the "/ponytail" Philosophy (Senior Minimalist Perspective)

1. **What is the real goal?**  
   Avoid SSD wear, prevent I/O bottlenecks, and eliminate RAM OOM risks in containers.
2. **Has the operating system already solved this?**  
   **YES.** When writing to a temporary directory on Linux (`/tmp` or NVMe SSD), the write call goes into the kernel's **Page Cache (RAM)** with nanosecond latency.
   - When `drmpack` reads bytes to dispatch them over the channel and then calls `unlink`, temporary files are deleted immediately.
   - The physical SSD NAND rarely undergoes write wear because files are unlinked while still residing in dirty Page Cache memory.
3. **The cost of forcing GPAC to "emit directly without files":**
   - Using **HTTP Client Sink (`hmode=push`)**: Requires embedding a mini HTTP server in Rust, opening loopback ports, handling TCP socket errors, managing connection race conditions — **costing hundreds of lines of boilerplate code** just to achieve the same result that Page Cache handles more efficiently.
   - Using **FFI C / Custom Filter**: **Strictly blocked** by GPAC because `dasher` does not allow custom filters to serve as destination sinks.
