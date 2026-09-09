# Kiến Trúc Toàn Diện Luồng Dữ Liệu: Ingestion & Output Pipeline trong drmpack

> **Tài liệu tham chiếu chuyên sâu về kiến trúc luồng dữ liệu (Data Flow Architecture)**  
> **Áp dụng cho:** `drmpack` Core Engine, `media-server` Ingestion Plane, E2E CDN Distribution & Web Playback.  
> **Primary Sources:** `src/session/`, `src/gpac/`, `src/key/`, `examples/common/`, `docs/adr/`.

---

## 1. Tổng Quan Kiến Trúc Data Pipeline

`drmpack` là một thư viện Rust hiệu năng cao đóng vai trò orchestrator điều phối việc đóng gói DRM bảo vệ nội dung media trực tiếp (Live Packaging) và phát sinh luồng phân phối MPEG-DASH / HLS. Trọng tâm thiết kế của `drmpack` tuân thủ các nguyên lý hệ thống cốt lõi:

1. **Zero-Copy Ingestion qua Unix Anonymous Pipes:** Toàn bộ dữ liệu fMP4 đầu vào được đẩy trực tiếp từ bộ nhớ tiến trình gọi vào tiến trình con GPAC thông qua pipe ẩn danh của kernel hệ điều hành (`ChildStdin`), loại bỏ hoàn toàn việc ghi file tạm đầu vào ra ổ đĩa.
2. **Cách ly tiến trình (Subprocess Isolation & Process Supervisor):** GPAC chạy dưới dạng tiến trình độc lập được giám sát bởi một `ProcessSupervisor` không đồng bộ (`tokio::spawn`), giải phóng runtime Rust khỏi rủi ro rò rỉ bộ nhớ hoặc crash của thư viện C/C++.
3. **Phân phối Manifest & Chunks qua Ramdisk (`/dev/shm` / `tmpfs`):** Dữ liệu đầu ra (Manifest `.mpd` / `.m3u8` và các media segment `.m4s`) được ghi trực tiếp vào bộ nhớ chia sẻ (Shared Memory Ramdisk) nhằm triệt tiêu độ trễ I/O và hiện tượng write amplification ở tần suất cập nhật micro-segment (200ms - 2s).
4. **Vòng đời độc lập an toàn (Decoupled Finalization & RAII Cleanup):** Quá trình kết thúc stream (`close()`) được tách rời khỏi quá trình dọn dẹp file vật lý (`cleanup()`), bảo vệ CDN edge worker không bị mất file khi đang phục vụ người xem, đồng thời sử dụng RAII guard (`Drop`) để ngăn chặn rò rỉ RAM.

### Sơ Đồ Tổng Thể End-to-End Pipeline

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

## 2. Luồng Bắn Dữ Liệu Vào (Data Ingestion / Input Pipeline)

Luồng bắn dữ liệu vào chịu trách nhiệm tiếp nhận các byte stream liên tục từ tiến trình gọi (`caller`), kiểm tra cấu trúc hộp ISO-BMFF cơ bản để giám sát tiến trình, điều phối phân luồng đồng thời (fan-out) tới các representation mục tiêu và đẩy vào các Unix pipe một cách an toàn.

### 2.1 Các API Ingestion Phía Caller

Căn cứ vào mã nguồn tại [`src/session/mod.rs`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L420-L487) và quyết định kiến trúc tại [ADR-0012](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/docs/adr/0012-streamlined-api-ergonomics-and-lifecycle.md) cùng [ADR-0013](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/docs/adr/0013-lean-rendition-and-track-id-disambiguation.md), cấu trúc `Segment` tổng hợp trước đây đã bị loại bỏ hoàn toàn. Thay vào đó, caller tương tác thông qua 3 API byte-level zero-copy:

```rust
// Trích từ src/session/mod.rs:420-487

/// 1. Primary Ingestion API: Gửi trực tiếp một khối byte hoặc chunk media
pub async fn push(&mut self, bytes: impl AsRef<[u8]>) -> Result<()> {
    let slice = bytes.as_ref();
    let is_media = slice.windows(4).any(|w| w == b"moof");
    self.push_data(slice, is_media).await
}

/// 2. Streaming Channel Ingestion API: Đọc stream liên tục từ tokio mpsc channel
pub async fn ingest_stream(&mut self, mut rx: mpsc::Receiver<Bytes>) -> Result<u64> {
    let mut count = 0u64;
    while let Some(chunk) = rx.recv().await {
        self.push(chunk).await?;
        count += 1;
    }
    Ok(count)
}

/// 3. High-level Batch / Run-to-Completion API: Ingest đến EOF và graceful close
pub async fn run_to_completion(mut self, rx: mpsc::Receiver<Bytes>) -> Result<PackagingResult> {
    let segments_ingested = self.ingest_stream(rx).await?;
    // ... Chuẩn bị danh sách đường dẫn manifest ...
    self.close().await?;
    self.preserve_output = true;
    Ok(PackagingResult { ... })
}
```

#### Bảng so sánh các phương thức Ingestion:

| Phương thức | Tham số đầu vào | Cơ chế truyền nhận | Trường hợp sử dụng điển hình |
| :--- | :--- | :--- | :--- |
| `push(impl AsRef<[u8]>)` | `&[u8]`, `Vec<u8>`, `Bytes`, v.v. | Đồng bộ gọi hàm async, zero-copy, không cấp phát heap mới | Caller nhận được buffer từ socket/transcoder và đẩy trực tiếp vào session. |
| `ingest_stream(Receiver<Bytes>)` | `tokio::sync::mpsc::Receiver<Bytes>` | Hút dữ liệu bất đồng bộ qua kênh channel có buffer giới hạn | Kết nối trực tiếp giữa task đọc mạng/demuxer và task đóng gói DRM. |
| `run_to_completion(Receiver<Bytes>)` | `tokio::sync::mpsc::Receiver<Bytes>` | Chạy toàn bộ chu kỳ: Ingest -> Close -> Return Manifests | Xử lý file VoD hoặc các live segment có độ dài xác định trước (bounded stream). |

### 2.2 Hành Trình Dữ Liệu Qua Các Tầng Kiến Trúc

Khi caller gọi `session.push(bytes)`, dữ liệu đi xuyên qua các lớp kiến trúc theo thứ tự nghiêm ngặt sau:

```text
Caller (e.g. MediaFeeder / media-server)
  │  push(&[u8])
  ▼
PackagingSession (src/session/mod.rs:489-498)
  │  - ensure_active(): Kiểm tra lifecycle state & cancellation_token
  │  - ping_heartbeat(): Báo hiệu watchdog tránh timeout do inactivity
  │  - Inspect 'moof' box: Cập nhật has_pushed_media = true
  │  - cluster.write_data(bytes).await
  ▼
RepresentationCluster (src/session/cluster.rs:197-223)
  │  - tokio::join!(rep_cenc.write_data(bytes), rep_cbcs.write_data(bytes))
  │  - Fan-out đồng thời không khóa chéo (lock-free fan-out)
  ▼
Representation (src/session/cluster.rs:54-56)
  │  - Lock mutex của GpacProcess: gpac.lock().await
  │  - gpac.write_data(bytes).await
  ▼
GpacProcess (src/gpac/process.rs:288-305)
  │  - check_status(): Xác nhận tiến trình chưa crash
  │  - stdin.write_all(data).await: Ghi vào buffer pipe Tokio
  │  - stdin.flush().await: Đẩy toàn bộ dữ liệu vào kernel buffer
  ▼
OS Kernel Anonymous Pipe (Unix Pipe Buffer: ChildStdin)
  │  - Copy dữ liệu vào pipe ring buffer trong Linux/macOS kernel
  ▼
GPAC Executable Process (Filter Engine)
     - Filter `stdin:ext=mp4` đọc dữ liệu từ file descriptor 0
```

### 2.3 Phân Tích Kỹ Thuật Chi Tiết Trong Ingestion

#### A. Cách nhận diện Init Segment vs Media Fragment (Kiểm tra hộp `moof`)

Trong chuẩn ISO-BMFF (ISO/IEC 14496-12) và CMAF (ISO/IEC 23000-19):
- **Initialization Segment:** Bao gồm các box `ftyp` (File Type) và `moov` (Movie Metadata chứa thông tin các track `trak`, codec parameters `stsd`, SPS/PPS, timing). Khối này **không** chứa dữ liệu mẫu media (samples).
- **Media Fragment:** Bao gồm cặp box `moof` (Movie Fragment chứa `mfhd`, `traf`, `tfhd`, `trun`) và `mdat` (Media Data chứa video NALUs hoặc AAC frames).

Tại [`src/session/mod.rs:425`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L425):
```rust
let is_media = slice.windows(4).any(|w| w == b"moof");
```

**Mục đích kiến trúc:**
1. **Theo dõi tiến trình stream:** Khi nhận thấy `is_media == true`, session cập nhật cờ nguyên tử:
   ```rust
   self.has_pushed_media.store(true, Ordering::Release);
   ```
2. **Bảo vệ tính toàn vẹn khi đóng luồng (`verify_hls_endlist`):** GPAC ở chế độ `dmode=dynauto` chỉ sinh ra danh sách phân đoạn và chèn `#EXT-X-ENDLIST` vào các playlist HLS nếu đã có ít nhất một media segment hoàn chỉnh được đẩy qua. Nếu caller chỉ mới gửi `init.mp4` rồi gọi `close()`, GPAC sẽ không tạo file phân đoạn nào. Cờ `has_pushed_media` giúp hàm `session.close()` phân biệt giữa một session rỗng (không bắt buộc có `#EXT-X-ENDLIST`) và một session live thực sự bị lỗi (đã push media nhưng thiếu `#EXT-X-ENDLIST`), ngăn chặn lỗi false-positive.

#### B. Cơ Chế Backpressure & Async Write Loop

Khác với các hệ thống buffering tự do trên RAM gây nguy cơ OOM (Out Of Memory), `drmpack` tận dụng cơ chế backpressure tự nhiên của hệ điều hành:

1. **Kernel Pipe Buffer:** Trên Linux, anonymous pipe giữa `ChildStdin` và tiến trình con GPAC có dung lượng mặc định là 64 KB (được điều khiển bởi `F_SETPIPE_SZ` của kernel).
2. **Non-blocking Tokio Reactor:** Tại [`src/gpac/process.rs:292-298`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/gpac/process.rs#L292-L298):
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
3. **Chuỗi phản ứng Backpressure (Propagation Chain):**
   - Nếu tiến trình GPAC bị chậm (ví dụ do CPU quá tải khi mã hóa AES hoặc disk IO bị nghẽn), GPAC sẽ ngừng đọc từ file descriptor 0 (`stdin`).
   - Pipe buffer 64 KB trong kernel lập tức bị đầy.
   - Hàm `stdin.write_all(data).await` của Tokio nhận mã lỗi `EWOULDBLOCK` / `EAGAIN` từ syscall `write()`. Tokio reactor tạm thời đình chỉ future ghi này và trả quyền điều khiển về cho Tokio event loop (kqueue/epoll).
   - `RepresentationCluster::write_data` và `session.push` bị giữ lại (awaiting).
   - Nếu caller sử dụng kênh `mpsc::channel(16)` (như trong `examples/02_low_latency_dual_cmaf.rs:60` hoặc `media-server`), buffer kênh 16 phần tử sẽ bị đầy, khiến lệnh `tx.send(chunk).await` phía upstream encoder bị chặn lại.
   - **Kết quả:** Tốc độ tạo dữ liệu của live encoder được tự động hãm lại theo đúng tốc độ xử lý thực tế của GPAC, ngăn chặn tràn bộ nhớ hoàn toàn mà không cần code quản lý hàng đợi phức tạp.

#### C. Concurrency & Fan-out trong Dual Mode (CENC vs CBCS)

Theo [ADR-0006](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/docs/adr/0006-dual-cenc-cbcs-representations.md), `Dual` không phải là một scheme đơn lẻ mà là một chế độ điều phối tạo ra 2 tiến trình GPAC chạy song song: một tiến trình mã hóa CENC (phục vụ Android/PC/PlayReady) và một tiến trình mã hóa CBCS (phục vụ iOS/macOS/FairPlay).

Tại [`src/session/cluster.rs:197-223`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/cluster.rs#L197-L223), dữ liệu được nhân đôi (fan-out) tới cả 2 tiến trình bằng `tokio::join!`:

```rust
pub async fn write_data(&self, bytes: &[u8]) -> Vec<RepresentationFailure> {
    let write_results = match self.representations.as_slice() {
        [] => Vec::new(),
        [rep] => vec![rep.write_data(bytes).await],
        [first, second] => {
            // Thực thi đồng thời cả 2 nhánh ghi dữ liệu
            let (first_result, second_result) =
                tokio::join!(first.write_data(bytes), second.write_data(bytes),);
            vec![first_result, second_result]
        }
        reps => { ... }
    };
    // Thu thập và chuyển đổi lỗi nếu có representation thất bại
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

**Symmetric Fail-Fast Policy (Chính sách sập đối xứng):**
Một session Dual là một đơn vị công việc nguyên tử (atomic unit of work). Nếu một bên (ví dụ GPAC CENC) bị crash, hệ thống không bao giờ cho phép bên còn lại (CBCS) tiếp tục chạy đơn độc (tránh hiện tượng phân mảnh luồng live giữa các nhóm thiết bị người dùng). 

Tại [`src/session/cluster.rs:333-349`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/cluster.rs#L333-L349) và [`src/session/mod.rs:1195-1220`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L1195-L1220):
- `ProcessSupervisor` phát hiện nhánh CENC thoát bất thường.
- Supervisor kích hoạt hàm `cluster.abort_peers(failed_scheme).await`.
- Nhánh CBCS ngay lập tức nhận tín hiệu `kill()` (SIGKILL) và bị thu hồi trong vòng 500ms.
- Toàn bộ session chuyển sang trạng thái `SessionState::Failed` và trả về một lỗi hợp nhất `DrmpackError::PackagingSession`.

#### D. Xử Lý Lỗi Broken Pipe & Cơ Chế Chẩn Đoán Crash (`map_stdin_io_error`)

Khi tiến trình GPAC gặp lỗi nghiêm trọng bên trong (ví dụ: fMP4 stream bị lỗi cấu trúc NALU, sai tham số SPS/PPS, hoặc thiếu Key trong DRM XML), GPAC sẽ tự đóng `stdin` và thoát (`exit`). 

Khi đó, lệnh ghi tiếp theo của Rust vào pipe sẽ gặp lỗi I/O của hệ điều hành: `Broken pipe (os error 32)` (Linux) hoặc `os error 32` (macOS). Nếu chỉ trả về "Broken pipe", lập trình viên sẽ hoàn toàn mù tịt về nguyên nhân gốc rễ.

`drmpack` giải quyết vấn đề này qua hàm `map_stdin_io_error` tại [`src/gpac/process.rs:307-318`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/gpac/process.rs#L307-L318):

```rust
async fn map_stdin_io_error(&self, op: &str, err: std::io::Error) -> DrmpackError {
    // 1. Chờ tối đa 50ms để ProcessSupervisor kịp nhận tín hiệu exit status từ child.wait()
    if self.status_rx.borrow().is_none() {
        let mut rx = self.status_rx.clone();
        let _ = tokio::time::timeout(Duration::from_millis(50), rx.changed()).await;
    }
    // 2. Trích xuất mã thoát (exit code)
    let exit_code = self.status_rx.borrow().as_ref().and_then(|s| s.code);
    // 3. Đọc lại ring buffer 64 dòng stderr gần nhất được lưu trong bộ nhớ
    let stderr = self.get_recent_stderr();
    // 4. Trả về lỗi ProcessCrashed mang đầy đủ thông điệp lỗi thực sự của GPAC
    DrmpackError::ProcessCrashed {
        exit_code,
        stderr: format!("Failed to {op} GPAC stdin: {err}. Stderr: {stderr}"),
    }
}
```

---

## 3. Luồng Xử Lý & Nhận Data Ra (Output / Packaging Pipeline)

Khi các byte fMP4 đi vào pipe `stdin`, tiến trình con GPAC tiếp nhận và xử lý dữ liệu thông qua kiến trúc chuỗi Filter (GPAC Filter Graph).

### 3.1 Cấu Trúc GPAC Filter Graph

Tại [`src/gpac/process.rs:110-146`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/gpac/process.rs#L110-L146), `GpacProcessConfig::build_args()` khởi tạo đồ thị filter của GPAC bao gồm 3 filter nối tiếp:

```text
[stdin:ext=mp4] ──(demuxed PIDs)──> [cecrypt] ──(encrypted PIDs)──> [dasher] ──> Ramdisk (/dev/shm)
```

#### 1. Filter Đọc & Phân Tách Luồng: `stdin:ext=mp4:alltk:...`
```text
stdin:ext=mp4:alltk:#Representation=(video)video_$Height$p,(video)video,(audio)(Language=!und)audio_$Language$,(audio)audio,(text)(Language=!und)sub_$Language$,(text)sub:#HLSPL=(video)video_$Height$p.m3u8,(video)video.m3u8,(audio)(Language=!und)audio_$Language$.m3u8,(audio)audio.m3u8,(text)(Language=!und)sub_$Language$.m3u8,(text)sub.m3u8
```
- `stdin:ext=mp4`: Chỉ định nguồn dữ liệu từ standard input, định dạng stream là fMP4 (fragmented MP4).
- `alltk`: Chỉ thị GPAC demux và xử lý toàn bộ các track có trong stream (video, nhiều audio tracks, subtitles).
- `#Representation=...`: Đặt tên định danh Representation trong DASH MPD theo thuộc tính thực tế của track (ví dụ: `video_720p`, `video_1080p`, `audio_eng`, `sub_spa`).
- `#HLSPL=...`: Đặt tên file danh sách phát phân đoạn (Variant Playlist) tương ứng cho HLS (ví dụ: `video_720p.m3u8`, `audio_eng.m3u8`).
- *Cơ chế tự động trích xuất metadata (ADR-0012 & 0013):* GPAC tự đọc cấu trúc container `moov`/`stsd`/`mdhd` từ input để biết chính xác codec (AVC1/H.264, MP4A/AAC), sample rate, width, height, bitrate. Phía Rust không cần cấu hình dư thừa các trường này.

#### 2. Filter Mã Hóa Bản Quyền: `cecrypt:cfile=<drm_xml_path>`
Filter `cecrypt` thực hiện mã hóa Common Encryption (CENC) trực tiếp trên từng gói tin media dựa theo file cấu hình XML được sinh tự động bởi `GpacDrmXmlGenerator` ([`src/gpac/xml.rs:76-170`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/gpac/xml.rs#L76-L170)).

- **Chế độ CENC (`EncryptionScheme::Cenc`):** Sử dụng thuật toán AES-128 ở chế độ đếm (CTR mode). Áp dụng cho Widevine (Android/Chrome) và PlayReady (Windows/Edge). XML sinh ra các box `pssh` nhúng vào moov/moof headers.
- **Chế độ CBCS (`EncryptionScheme::Cbcs`):** Sử dụng thuật toán AES-128 ở chế độ Cipher Block Chaining kết hợp mã hóa mẫu (Pattern Encryption 10%).
  - Với Track Video: Bắt buộc cấu hình `crypt_byte_block="1" skip_byte_block="9"` (mã hóa 1 block 16-byte, bỏ qua 9 block).
  - Với Track Audio: Chuẩn Apple FairPlay và ISO/IEC 23001-7 nghiêm cấm dùng pattern encryption trên audio; audio phải cấu hình `crypt_byte_block="0" skip_byte_block="0"` (mã hóa toàn khối) ([`src/gpac/xml.rs:207-213`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/gpac/xml.rs#L207-L213)).
  - Constant IV: Sử dụng IV cố định 16-byte (`first_IV="0x..."`).
  - HLS Signaling: Nhúng tag `hlsInfo` chứa `KEYFORMAT="com.apple.streamingkeydelivery"` và URI `skd://...`. Riêng FairPlay không bao giờ được sinh ra PSSH box trong file MP4 ([`src/gpac/xml.rs:96-98`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/gpac/xml.rs#L96-L98)).

#### 3. Filter Đóng Gói Phân Phối: `dasher:...`
```text
<output_dir>/live.mpd:dual:profile=live:dmode=dynauto:segdur=2:spd=4000:tsb=1800:utcs=inband:pssh=mv:template=$RepresentationID$_$Init=init$$Number$[:cdur=0.2:asto=0.0:llhls=br:cmaf=cmfc]
```

Các tham số chi tiết của filter `dasher`:
- `:dual`: Yêu cầu GPAC sinh song song cả DASH manifest (`live.mpd`) và Master HLS playlist (`live.m3u8`) từ cùng một luồng đóng gói.
- `profile=live`: Hồ sơ streaming trực tiếp theo chuẩn DASH-IF.
- `dmode=dynauto`: Chế độ live dynamic tự động. Trong lúc stream, manifest là dynamic. Khi stdin nhận tín hiệu EOF, GPAC tự động chuyển manifest sang static (VoD) và thêm `#EXT-X-ENDLIST` vào các playlist HLS.
- `segdur=2.0`: Thiết lập độ dài phân đoạn mục tiêu là 2.0 giây (khuyến nghị khớp với GOP của live encoder).
- `spd=4000`: `suggestedPresentationDelay` = 4000ms (2.0x `segdur`). Theo chuẩn DASH-IF IOP v4.3, tham số này tạo một vùng đệm an toàn 4 giây cho player để tránh hiện tượng văng 404 khi bám sát live-edge.
- `tsb=1800`: Time-Shift Buffer 1800 giây (30 phút). GPAC duy trì một cửa sổ trượt DVR 30 phút và tự động xóa các segment cũ trên đĩa.
- `utcs=inband`: Nhúng mốc thời gian UTC đồng bộ vào container media.
- `pssh=mv`: Nhúng box `pssh` vào cả header khởi tạo (`moov`) và từng fragment (`moof`).
- `template=$RepresentationID$_$Init=init$$Number$`: Quy tắc đặt tên file: phân đoạn init là `{rep_id}_init.mp4`, phân đoạn media là `{rep_id}_{number}.m4s`.
- *Tham số mở rộng Low-Latency (`LatencyMode::LowLatency`):*
  - `:cdur=0.2`: CMAF Chunk duration 200ms.
  - `:asto=0.0`: Availability Time Offset (mặc định 0.0s trên file-based origin để chống lỗi 404 khi không dùng chunked HTTP transfer).
  - `:llhls=br`: Kích hoạt chế độ Apple Low-Latency HLS byte-range partial segments.
  - `:cmaf=cmfc`: Đảm bảo đóng gói tuân thủ profile CMAF Chunk.

---

### 3.2 Cấu Trúc File & Không Gian Lưu Trữ (Storage Topology)

Hệ thống phân tách ranh giới dữ liệu rất rõ ràng giữa vùng lưu trữ tạm bộ nhớ (Ramdisk) và vùng lưu trữ điều khiển bảo mật (Control Dir):

```text
/dev/shm/ (hoặc $TMPDIR trên macOS)
 ├── drmpack-control/                                  <-- PRIVATE CONTROL DIR (mode 0700)
 │    └── {content_id}_{uuid}/
 │         ├── cenc.xml                                <-- GPAC DRM XML (mode 0600, chứa Content Keys bí mật)
 │         └── cbcs.xml                                <-- GPAC DRM XML (mode 0600, chứa Content Keys bí mật)
 │
 └── drmpack_{content_id}_{uuid}/                      <-- PUBLIC OUTPUT DIR (mode 0755, expose cho Web/CDN)
      │
      ├── [Trường hợp Single Scheme: CENC hoặc CBCS]
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
      └── [Trường hợp Dual Scheme: Dual CENC & CBCS]
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

## 4. Chi Tiết Cấu Trúc Các File Output Được Sinh Ra

### 4.1 DASH Manifest (`live.mpd`)
DASH MPD là tài liệu XML biểu diễn cấu trúc trình chiếu đa độ phân giải thích ứng (ABR):

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
Master playlist (Multivariant Playlist) khai báo các biến thể bitrate và liên kết các nhóm audio/phụ đề. Tuân thủ RFC 8216 và Apple HLS Authoring Spec, các thẻ `#EXT-X-MEDIA` luôn được sắp xếp đứng trước `#EXT-X-STREAM-INF`:

```m3u8
#EXTM3U
#EXT-X-VERSION:7
#EXT-X-INDEPENDENT-SEGMENTS

# 1. Khai báo Rendition Audio (Phải đứng trước theo cdn_publisher sanitize)
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="audio",NAME="English",DEFAULT=YES,AUTOSELECT=YES,LANGUAGE="eng",URI="audio_eng.m3u8"

# 2. Khai báo Stream Video liên kết với Group Audio
#EXT-X-STREAM-INF:BANDWIDTH=2628000,AVERAGE-BANDWIDTH=2500000,RESOLUTION=1280x720,CODECS="avc1.64001f,mp4a.40.2",AUDIO="audio"
video_720p.m3u8

#EXT-X-STREAM-INF:BANDWIDTH=5128000,AVERAGE-BANDWIDTH=5000000,RESOLUTION=1920x1080,CODECS="avc1.640028,mp4a.40.2",AUDIO="audio"
video_1080p.m3u8
```

### 4.3 Variant HLS Playlists (`video_720p.m3u8`)
Danh sách phân đoạn thực tế chỉ định khóa giải mã FairPlay và các file `.m4s`:

```m3u8
#EXTM3U
#EXT-X-VERSION:7
#EXT-X-TARGETDURATION:2
#EXT-X-MEDIA-SEQUENCE:1

# Khai báo khóa DRM Apple FairPlay (Chế độ CBCS)
#EXT-X-KEY:METHOD=SAMPLE-AES,KEYFORMAT="com.apple.streamingkeydelivery",KEYFORMATVERSIONS="1",URI="skd://14ccfc47-2b81-4320-b0b3-111111111111"

# Segment Khởi tạo CMAF
#EXT-X-MAP:URI="video_720p_init.mp4"

# Danh sách các phân đoạn media
#EXTINF:2.000000,
video_720p_1.m4s
#EXTINF:2.000000,
video_720p_2.m4s
#EXTINF:2.000000,
video_720p_3.m4s
```

### 4.4 Cấu Trúc Nhị Phân Các File Phân Đoạn (CMAF Segments)

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
                          ├── schm (Scheme Type: 'cenc' hoặc 'cbcs')
                          └── schi (Scheme Information Box)
                               └── tenc (Track Encryption Box: default_IsEncrypted=1, default_IV_size=16, default_KID)

[Media Segment: video_720p_1.m4s]
 ├── styp (Segment Type Box: brands=['msdh', 'msix'])
 ├── moof (Movie Fragment Box - Đánh dấu bắt đầu phân đoạn media mới)
 │    ├── mfhd (Movie Fragment Header: sequence_number=1)
 │    └── traf (Track Fragment Box)
 │         ├── tfhd (Track Fragment Header: track_id=1)
 │         ├── tfdt (Track Fragment Base Media Decode Time)
 │         ├── senc (Sample Encryption Box: chứa Per-sample IVs hoặc Subsample mapping)
 │         └── trun (Track Run Box: sample_count, sample_sizes, sample_durations)
 └── mdat (Media Data Box: Chứa các khối NALU video đã bị mã hóa AES-CTR hoặc AES-CBC)
```

---

## 5. Luồng Tiêu Thụ Dữ Liệu Ra (Egress & Distribution Pipeline)

Sau khi GPAC ghi các file phân đoạn vào Ramdisk, các module consumer bên ngoài sẽ tiếp nhận và phục vụ tới thiết bị phát.

### 5.1 Đồng Bộ Hóa CDN: `CdnPublisher` (`examples/common/cdn_publisher.rs`)

`CdnPublisher` đóng vai trò là một tiến trình đồng bộ dữ liệu thông minh giữa Ramdisk trung gian và bộ nhớ phân phối của web server/CDN. Căn cứ vào [`examples/common/cdn_publisher.rs:33-294`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/examples/common/cdn_publisher.rs#L33-L294), module giải quyết 3 thách thức lớn trong live packaging:

#### 1. Multi-Track Parity Alignment & Skew Tolerance (Khử lệch pha audio/video)
Trong live fMP4 streaming, việc multiplexing giữa video và audio luôn có độ trễ lệch pha nhỏ (50ms - 200ms) do độ dài khung hình khác nhau. Nếu video segment #17 được ghi xong trước audio segment #17 mà publisher lập tức cập nhật manifest lên CDN, trình phát video sẽ yêu cầu audio #17 và gặp ngay lỗi HTTP 404 (Live-edge 404 stall).

`CdnPublisher` tính toán chỉ số `min_common_seg` trên toàn bộ các active track và cho phép một độ dung thứ lệch pha an toàn (`skew_tolerance = 2` segments):
```rust
// Trích từ examples/common/cdn_publisher.rs:116-138
let min_common_seg = max_seg_per_track.values().copied().min().unwrap_or(u64::MAX);
let skew_tolerance = 2u64;

for src_path in segments_and_data {
    if let Some((_track_id, seg_num)) = parse_track_and_segment(file_name) {
        if min_common_seg < u64::MAX && seg_num > min_common_seg + skew_tolerance {
            // Giữ lại segment bị chạy trước quá xa cho đến khi track chậm hơn bắt kịp
            continue;
        }
    }
    self.sync_file(&src_path, &cur_dst, min_common_seg).await?;
}
```

#### 2. Atomic Write-Rename Pattern (Ghi nguyên tử)
Tránh việc client web fetch phải một file media segment đang được ghi dở dang giữa chừng.
Tại [`examples/common/cdn_publisher.rs:186-282`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/examples/common/cdn_publisher.rs#L186-L282):
- File được ghi vào một file tạm ẩn: `.{filename}_{pid}_{uuid}.tmp`.
- Sau khi ghi xong 100% byte, thực hiện lệnh `tokio::fs::rename()` để thay thế nguyên tử vào đường dẫn chính thức.

#### 3. Kiểm tra tính toàn vẹn Manifest (Manifest Integrity Gate)
- Với DASH (`.mpd`): Bắt buộc kiểm tra phải chứa thẻ đóng `</MPD>` mới được publish. Nếu file đang ghi dở thiếu thẻ đóng, publisher sẽ bỏ qua ở tick hiện tại.
- Với HLS (`.m3u8`): Kiểm tra bắt đầu bằng `#EXTM3U`, loại bỏ các line phân đoạn vượt quá `min_common_seg + 2`, và tự động chuẩn hóa vị trí các thẻ `#EXT-X-MEDIA` lên trước `#EXT-X-STREAM-INF` (`sanitize_master_playlist`).

---

### 5.2 Máy Chủ Phát Sóng: `playback_server.rs` (`examples/common/playback_server.rs`)

HTTP origin server phục vụ luồng dữ liệu hỗ trợ các tính năng tiêu chuẩn:

1. **HTTP Range Requests (`Range: bytes=start-end`):** Trình duyệt web (đặc biệt khi phát HLS trên Safari hoặc đọc byte-range CMAF parts) thường xuyên gửi header Range. Server phân tích cú pháp bằng `parse_range_header()` và trả về HTTP `206 Partial Content` kèm header `Content-Range: bytes start-end/total`.
2. **Origin Shield Grace Window (Chống lỗi 404 live-edge):** Tại [`examples/common/playback_server.rs:634-644`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/examples/common/playback_server.rs#L634-L644):
   ```rust
   // Nếu client yêu cầu một file .m4s vừa mới xuất hiện trong manifest nhưng publisher
   // chưa kịp hoàn tất việc rename, server giữ kết nối chờ tối đa 5.0s (100 lần x 50ms)
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
   Cơ chế này đóng vai trò như một bộ đệm Origin Shield, hấp thụ hoàn toàn độ trễ I/O và rung pha (jitter) của mạng.
3. **MIME Types & Cache-Control:** Trả về `application/dash+xml` cho `.mpd`, `application/vnd.apple.mpegurl` cho `.m3u8`, `video/mp4` cho `.m4s` và `.mp4`. Manifest luôn được gắn header `Cache-Control: no-cache, no-store, must-revalidate`.

---

## 6. Cơ Chế Bảo Vệ Ramdisk & Quản Lý Vòng Đời (Lifecycle & Resource Safety)

Hoạt động live streaming liên tục đòi hỏi cơ chế dọn dẹp tài nguyên chặt chẽ để tránh làm cạn kiệt bộ nhớ máy chủ.

### 6.1 Giới Hạn Bộ Nhớ Đệm Bằng Time-Shift Buffer (`tsb=1800`)

Trong lệnh cấu hình GPAC dasher ([`src/gpac/process.rs:128`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/gpac/process.rs#L128)), tham số `tsb=1800` (Time-Shift Buffer 1800 giây = 30 phút) chỉ thị cho GPAC:
1. Duy trì danh sách phân đoạn trong playlist ứng với 30 phút phát sóng gần nhất.
2. Tự động thực hiện phép quay vòng (segment rotation): Các file `.m4s` có tuổi thọ vượt quá 30 phút sẽ được GPAC tự động xóa trực tiếp khỏi Ramdisk (`/dev/shm`).
3. **Ước tính dung lượng Ramdisk tối đa:**
   $$\text{RAM Limit} = \text{Total Bitrate of All Renditions} \times 1800\text{ seconds} \times \text{Safety Factor (1.25)}$$
   *Ví dụ:* Một luồng gồm 1080p (5 Mbps) + 720p (2.5 Mbps) + Audio (128 kbps) $\approx 7.63\text{ Mbps} \approx 0.95\text{ MB/s}$.  
   Dung lượng Ramdisk tối đa bị chiếm dụng sau 30 phút là: $0.95\text{ MB/s} \times 1800\text{s} \approx 1.71\text{ GB}$. Bộ nhớ này sẽ đạt ngưỡng bão hòa và không bao giờ tăng thêm.

---

### 6.2 Vòng Đời Dọn Dẹp Phân Tầng (Decoupled Lifecycle Model)

Theo [ADR-0012](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/docs/adr/0012-streamlined-api-ergonomics-and-lifecycle.md), `drmpack` tách rời hoàn toàn quá trình finalization luồng phát khỏi quá trình hủy thư mục Ramdisk:

```text
PackagingSession::create() ──> [Active Streaming] ──> PackagingSession::close() ──> [Delivery Window] ──> PackagingSession::cleanup()
        │                                                     │                                                    │
        ▼                                                     ▼                                                    ▼
Tạo /dev/shm/ output                               Đóng stdin, đợi GPAC exit                            Xóa hoàn toàn output_dir
Tạo private control_dir                            Flush manifest, kiểm tra ENDLIST                     khi CDN kết thúc phân phối
                                                   XÓA NGAY control_dir (bảo mật)
                                                   GIỮ NGUYÊN output_dir (bảo vệ CDN)
```

#### 1. Giai đoạn kết thúc stream: `session.close().await` ([`src/session/mod.rs:519-580`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L519-L580))
- Hủy token watchdog (`cancellation_token.cancel()`).
- Chuyển `Lifecycle` sang `SessionState::Closing`.
- Đóng pipe `stdin` của từng GPAC process (gửi EOF).
- Chờ tiến trình GPAC hoàn tất ghi đĩa trong khoảng thời gian `finalization_timeout` (mặc định 5s). Nếu quá thời gian, supervisor sẽ gửi tín hiệu `SIGKILL`.
- **Kiểm tra `#EXT-X-ENDLIST`:** Với những stream đã push media fragment, hàm `verify_hls_endlist(&output_dir)` đọc tất cả các file media playlist `.m3u8` để đảm bảo thẻ `#EXT-X-ENDLIST` đã được ghi vào cuối file. Nếu thiếu, báo lỗi `RepresentationFailure` để tránh hiện tượng player bị treo mãi mãi.
- **Xóa Private Control Dir:** Thư mục `control_dir` chứa các file XML lưu Content Key được xóa ngay lập tức (`cleanup_control_dir()`).
- **Bảo tồn Output Dir:** Cờ `preserve_output` được đặt thành `true`. Thư mục `/dev/shm` chứa video/audio segments được giữ nguyên vẹn để các CDN Edge node hoặc người xem trễ tiếp tục tải nốt nội dung.

#### 2. Giai đoạn dọn dẹp vật lý: `session.cleanup().await` ([`src/session/mod.rs:620-628`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L620-L628))
- Được caller gọi chủ động khi buổi phát sóng đã kết thúc hoàn toàn và CDN edge đã cache xong toàn bộ nội dung.
- Thực hiện `tokio::fs::remove_dir_all(&self.config.output_dir)`.

#### 3. Tự động bảo vệ bằng RAII: `impl Drop for PackagingSession` ([`src/session/mod.rs:778-802`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L778-L802))
Để ngăn ngừa tình trạng lập trình viên quên gọi `cleanup()` hoặc chương trình bị panic giữa chừng gây rò rỉ dung lượng `/dev/shm`:
- Nếu `output_dir` do thư viện tự động cấp phát trong `/dev/shm` (không phải thư mục do caller chỉ định qua `.with_output_dir()`), và cờ `preserve_output` chưa được bật, phương thức `drop()` sẽ tự động xóa sạch thư mục `output_dir`.
- Thư mục riêng tư `control_dir` luôn được xóa trong `drop()`.
- Task giám sát tiến trình con nhận lệnh `abort()` và GPAC process nhận `kill()`.

---

## 7. Bảng Tổng Hợp Tham Chiếu Mã Nguồn Core Pipeline

| Chức năng / Thành phần | Đường dẫn mã nguồn | Vị trí dòng | Trách nhiệm chính |
| :--- | :--- | :--- | :--- |
| **API Ingestion Cơ Bản** | `src/session/mod.rs` | L420–L487 | Cung cấp `push()`, `ingest_stream()`, `run_to_completion()`. |
| **Phát hiện box `moof`** | `src/session/mod.rs` | L425, L492 | Phân biệt Init Segment và Media Fragment, đặt cờ `has_pushed_media`. |
| **Cluster Ingestion Fan-out** | `src/session/cluster.rs` | L197–L223 | Phân phối luồng song song qua `tokio::join!` cho CENC và CBCS. |
| **Symmetric Abort Peers** | `src/session/cluster.rs` | L333–L349 | Huỷ diệt ngay nhánh còn lại nếu 1 tiến trình con GPAC bị crash. |
| **Cấu hình GPAC CLI Args** | `src/gpac/process.rs` | L110–L146 | Thiết lập filter graph: `stdin:ext=mp4`, `cecrypt`, `dasher:dual`. |
| **Tokio Stdin Async Write** | `src/gpac/process.rs` | L288–L305 | Ghi non-blocking vào pipe, kích hoạt cơ chế backpressure của kernel. |
| **Chẩn đoán Broken Pipe** | `src/gpac/process.rs` | L307–L318 | `map_stdin_io_error` trích xuất exit code và 64 dòng stderr khi pipe gãy. |
| **Sinh cấu hình DRM XML** | `src/gpac/xml.rs` | L76–L170 | Tạo file XML cấu hình mã hóa CENC (CTR) và CBCS (Pattern 1:9 / Constant IV). |
| **Xác thực `#EXT-X-ENDLIST`** | `src/session/mod.rs` | L984–L1033 | Đọc playlist HLS sau khi đóng để đảm bảo không bị thiếu thẻ kết thúc. |
| **Multi-Track CDN Sync** | `examples/common/cdn_publisher.rs` | L88–L150 | Tính `min_common_seg` và `skew_tolerance`, đồng bộ nguyên tử lên CDN. |
| **Origin Shield Grace Window** | `examples/common/playback_server.rs` | L634–L644 | Trì hoãn 5.0s cho các request phân đoạn `.m4s` đang được ghi để tránh 404. |
| **Dọn dẹp RAII Ramdisk** | `src/session/mod.rs` | L778–L802 | `Drop` tự động giải phóng Ramdisk `/dev/shm` và `control_dir`. |

---

## 8. Kết Luận

Kiến trúc Data Ingestion & Output Pipeline của `drmpack` đạt được sự cân bằng tối ưu giữa **hiệu năng cực hạn (Zero-Copy Pipe Ingestion, Shared Memory Ramdisk)** và **độ tin cậy tuyệt đối trong môi trường sản xuất (Non-blocking Backpressure, Asynchronous Process Supervisor, Symmetric Fail-Fast, Atomic CDN Publishing, Origin Shield Grace Window)**. Toàn bộ chu trình từ lúc caller bắn byte fMP4 đầu tiên cho đến khi trình phát web giải mã thành công diễn ra hoàn toàn tự động, minh bạch và an toàn tài nguyên.
