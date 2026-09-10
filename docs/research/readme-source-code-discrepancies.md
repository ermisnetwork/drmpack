# Báo cáo Điều tra và Kiểm chứng Kỹ thuật: Đối chiếu README.md với Mã nguồn drmpack

Ngày kiểm chứng: 2026-09-10  
Người thực hiện: Chuyên viên Điều tra & Kiểm chứng Kỹ thuật drmpack  
Phương pháp kiểm chứng: Đọc mã nguồn tĩnh, phân tích cú pháp/kiểu dữ liệu (type system), kiểm chứng biên dịch thực tế bằng Rust compiler (`cargo check`), và đối chiếu với các Quyết định Kiến trúc (ADR).

---

## Tóm tắt Điều hành (Executive Summary)

Đợt kiểm chứng toàn diện này đối chiếu từng câu chữ, sơ đồ, bảng biểu và đoạn mã ví dụ (code snippets) trong tài liệu [`README.md`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/README.md) với mã nguồn thực tế của thư viện `drmpack` (bao gồm `src/`, `examples/`, `Cargo.toml`, và `docs/adr/`).

**Kết luận chung:** File `README.md` chứa một số thông tin tổng quan tốt về mục tiêu của dự án, nhưng tồn tại **nhiều sai lệch kỹ thuật nghiêm trọng**, bao gồm:
1. **Lỗi biên dịch chắc chắn (Compile Failures):** Các code snippet chính (Snippet 4 và Snippet 5) **không thể biên dịch được** với Rust compiler hiện tại do gọi các trường và phương thức không hề tồn tại, hoặc truyền ngược thứ tự đối số.
2. **Sai lệch Vòng đời (Lifecycle Inconsistency):** Sơ đồ tuần tự và bảng mô tả kiến trúc tuyên bố `PackagingSession::create()` tự spawn background task `ArtifactHarvester`, trong khi thực tế tác vụ này được khởi tạo trễ (lazy) chỉ khi gọi `session.take_output_receiver()`.
3. **Tuyên bố Quá mức về Hiệu năng (Marketing/Hallucinated Claims):** Tuyên bố *"Zero disk I/O on egress"* và *"Eliminates disk wear"* mâu thuẫn trực tiếp với Quyết định Kiến trúc [ADR-0015](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/docs/adr/0015-direct-output-channel-and-safe-storage.md) và cơ chế ghi staging files ra đĩa (`/tmp`) của GPAC `dasher`.
4. **Tham số GPAC và Tên tệp không chính xác:** Cú pháp stdin pipe bị ghi nhầm thành `pipe://stdin:fmt=mp4` (thực tế là GPAC filter `-i stdin:ext=mp4:alltk:...`), và tệp init segment bị ghi là `init.mp4` (thực tế luôn có tiền tố Representation ID như `video_1080p_init.mp4`).
5. **Thông tin bịa đặt (Hallucination) về Token Claims:** Tuyên bố Axinom JWT entitlement token chứa `expiration timestamps`, trong khi hàm `generate_axinom_jwt` trong mã nguồn không hề tạo trường `exp` hay bất kỳ timestamp hết hạn nào.
6. **Sai lệch Chữ ký API & Định danh kiểu:** Ghi sai số lượng đối số của `handle_fairplay_certificate`, và dùng tên `ArtifactHarvester` như một public struct trong khi mã nguồn chỉ có private struct `Harvester`.

Dưới đây là chi tiết điều tra từng hạng mục cùng bằng chứng mã nguồn cụ thể và đề xuất khắc phục.

---

## 1. Kiểm chứng Code Snippets trong README

Khi trích xuất nguyên văn các đoạn mã mẫu trong `README.md` vào một tệp kiểm thử độc lập và chạy `cargo check`, trình biên dịch Rust (`rustc 1.80+`) báo **6 lỗi biên dịch nghiêm trọng**.

### 1.1 Snippet 4 (Consuming Packaged Artifacts) — Lỗi trường không tồn tại

- **Vị trí trong README:** Dòng 500–523 (`### 4. Consuming Packaged Artifacts`).
- **Mã nguồn trong README:**
  ```rust
  while let Some(artifact) = rx.recv().await {
      match artifact.kind {
          ArtifactKind::Manifest => {
              println!("Updated playlist: {} ({} bytes)", artifact.relative_path, artifact.data.len());
          }
          ArtifactKind::MediaSegment => {
              println!("New segment ready: {} (seq: {:?})", artifact.relative_path, artifact.sequence_number);
          }
          ArtifactKind::InitSegment => {
              println!("Init segment ready: {}", artifact.relative_path);
          }
      }
  }
  ```
- **Mã nguồn thực tế ([`src/types.rs:324-333`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/types.rs#L324-L333)):**
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  pub struct PackagedArtifact {
      /// Relative filename of the artifact (e.g. `video_1080p_1.m4s`, `live.m3u8`).
      pub filename: String,
      /// In-memory binary payload.
      pub data: bytes::Bytes,
      /// Structural classification of the artifact.
      pub kind: ArtifactKind,
      /// Concrete encryption scheme of the artifact.
      pub scheme: EncryptionScheme,
  }
  ```
- **Bằng chứng lỗi biên dịch từ `cargo check`:**
  ```text
  error[E0609]: no field `relative_path` on type `PackagedArtifact`
     |
     | println!("Updated playlist: {} ({} bytes)", artifact.relative_path, artifact.data.len());
     |                                                      ^^^^^^^^^^^^^ unknown field
     = note: available fields are: `filename`, `data`, `kind`, `scheme`

  error[E0609]: no field `sequence_number` on type `PackagedArtifact`
     |
     | println!("New segment ready: {} (seq: {:?})", artifact.relative_path, artifact.sequence_number);
     |                                                                                ^^^^^^^^^^^^^^^ unknown field
     = note: available fields are: `filename`, `data`, `kind`, `scheme`
  ```
- **Nguyên nhân gốc rễ:**
  1. Trường lưu tên file trong struct `PackagedArtifact` là `pub filename: String`, không phải `relative_path`.
  2. Struct `PackagedArtifact` **hoàn toàn không có trường `sequence_number`**. Thông tin sequence number chỉ có thể được trích xuất bằng hàm phụ trợ nội bộ [`parse_segment_number(&artifact.filename)`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/harvester.rs#L61-L69) hoặc do caller tự xử lý.
- **Đề xuất sửa chữa trong README:**
  ```rust
  while let Some(artifact) = rx.recv().await {
      match artifact.kind {
          ArtifactKind::Manifest => {
              println!("Updated playlist: {} ({} bytes)", artifact.filename, artifact.data.len());
          }
          ArtifactKind::MediaSegment => {
              println!("New segment ready: {}", artifact.filename);
          }
          ArtifactKind::InitSegment => {
              println!("Init segment ready: {}", artifact.filename);
          }
      }
  }
  ```

---

### 1.2 Snippet 5 (Mounting DRM License Proxy Routes - Axum) — Ngược đối số và phương thức không tồn tại

- **Vị trí trong README:** Dòng 529–559 (`### 5. Mounting DRM License Proxy Routes (Axum Example)`).
- **Mã nguồn trong README:**
  ```rust
  async fn widevine_handler(
      State(state): State<Arc<AppState>>,
      headers: HeaderMap,
      body: Bytes,
  ) -> impl IntoResponse {
      let auth_token = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
      match handle_widevine_license(&state.proxy, auth_token, body).await {
          Ok(res) => (res.status_code(), res.data).into_response(),
          Err(err) => (axum::http::StatusCode::BAD_REQUEST, err.to_string()).into_response(),
      }
  }
  ```
- **Mã nguồn thực tế ([`src/license/proxy.rs:425-431`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/license/proxy.rs#L425-L431)):**
  ```rust
  pub async fn handle_widevine_license(
      proxy: &LicenseProxy,
      challenge: impl AsRef<[u8]>,
      auth_token: &str,
  ) -> Result<LicenseResponse> {
      proxy.handle_widevine_license(challenge, auth_token).await
  }
  ```
- **Mã nguồn thực tế của `LicenseResponse` ([`src/license/response.rs:8-15`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/license/response.rs#L8-L15)):**
  ```rust
  #[derive(Clone, Debug, PartialEq)]
  pub struct LicenseResponse {
      pub data: bytes::Bytes,
      pub content_type: Option<String>,
      pub headers: reqwest::header::HeaderMap,
  }
  ```
- **Bằng chứng lỗi biên dịch từ `cargo check`:**
  ```text
  error[E0308]: mismatched types
     --> tests/readme_snippets_verify.rs:42:61
      |
   42 |     match handle_widevine_license(&state.proxy, auth_token, body).await {
      |           -----------------------                           ^^^^ expected `&str`, found `Bytes`
      |           |
      |           arguments to this function are incorrect

  error[E0599]: no method named `status_code` found for struct `LicenseResponse` in the current scope
     --> tests/readme_snippets_verify.rs:43:25
      |
   43 |         Ok(res) => (res.status_code(), res.data).into_response(),
      |                         ^^^^^^^^^^^ method not found in `LicenseResponse`
  ```
- **Nguyên nhân gốc rễ:**
  1. **Nghịch đảo thứ tự tham số:** Chữ ký hàm thực tế yêu cầu tham số thứ hai là `challenge` (kiểu `impl AsRef<[u8]>`) và tham số thứ ba là `auth_token` (kiểu `&str`). README truyền `auth_token` trước rồi mới truyền `body`. Do kiểu chuỗi `&str` cũng thoả mãn `AsRef<[u8]>`, trình biên dịch gán `challenge = auth_token` và sau đó báo lỗi type mismatch khi thấy `body: Bytes` được truyền vào vị trí của `auth_token: &str`.
  2. **Phương thức `status_code()` không tồn tại:** Struct `LicenseResponse` chỉ đóng gói `data`, `content_type`, và `headers`. Khi license proxy hoàn tất thành công, trạng thái HTTP luôn là thành công (200 OK); nếu upstream trả lỗi, hàm sẽ trả về `Err(DrmpackError::LicenseProxy { status, .. })`. Do đó `LicenseResponse` không có và không cần phương thức `status_code()`.
  3. **Xử lý header `authorization` sơ sài:** Token lấy từ header `authorization` thường có dạng `"Bearer <jwt>"`. Nếu không bóc tiền tố `"Bearer "`, chuỗi token thô bị đẩy vào header `X-AxDRM-Message` của Axinom có thể gây lỗi xác thực 400/401 tại upstream Axinom.
- **Đề xuất sửa chữa trong README:**
  ```rust
  async fn widevine_handler(
      State(state): State<Arc<AppState>>,
      headers: HeaderMap,
      body: Bytes,
  ) -> impl IntoResponse {
      let auth_token = headers
          .get("authorization")
          .and_then(|v| v.to_str().ok())
          .and_then(|h| h.strip_prefix("Bearer "))
          .unwrap_or("");

      match handle_widevine_license(&state.proxy, body, auth_token).await {
          Ok(res) => (axum::http::StatusCode::OK, res.data).into_response(),
          Err(err) => (axum::http::StatusCode::BAD_REQUEST, err.to_string()).into_response(),
      }
  }
  ```

---

### 1.3 Snippet 3b (`SessionWriter`) — `writer.shutdown().await?` vs `writer.close().await`

- **Vị trí trong README:** Dòng 485–494 (`### 3. Ingesting Media via push() or SessionWriter`).
- **Mã nguồn trong README:**
  ```rust
  async fn ingest_with_writer(
      session: &mut PackagingSession,
      mut source_socket: tokio::net::TcpStream,
  ) -> Result<(), Box<dyn std::error::Error>> {
      let mut writer = session.writer();
      tokio::io::copy(&mut source_socket, &mut writer).await?;
      writer.shutdown().await?;
      Ok(())
  }
  ```
- **Mã nguồn thực tế ([`src/session/mod.rs:956-964, 1001-1004`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L956-L964)):**
  ```rust
  impl SessionWriter {
      /// Shut down the writer and drain any buffered bytes to the session.
      pub async fn close(mut self) {
          self.sender.close();
          if let Some(task) = self.forward_task.take() {
              let _ = task.await;
          }
      }
  }

  impl tokio::io::AsyncWrite for SessionWriter {
      // ...
      fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
          self.get_mut().sender.close();
          Poll::Ready(Ok(()))
      }
  }
  ```
- **Phân tích rủi ro runtime:**
  - Về mặt biên dịch: Do `SessionWriter` cài đặt `tokio::io::AsyncWrite` và scope có `use tokio::io::AsyncWriteExt;`, lời gọi `writer.shutdown().await?` có thể biên dịch được.
  - Về mặt ngữ nghĩa và an toàn dữ liệu: `poll_shutdown` chỉ thực hiện đóng channel sender (`self.get_mut().sender.close()`) mà **hoàn toàn không chờ `forward_task` hoàn thành việc drain dữ liệu**.
  - `SessionWriter` sử dụng một background forwarding task để lấy byte từ channel và ghi vào GPAC cluster qua `write_data`. Nếu caller chỉ gọi `writer.shutdown().await?` rồi thoát hàm, `writer` bị drop và `forward_task` có thể bị hủy giữa chừng hoặc kết thúc muộn hơn, dẫn đến mất các fragment cuối cùng của media stream (data truncation / silent data loss).
  - Ngay tại docstring của `SessionWriter` ([`src/session/mod.rs:522`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L522)) và trong example thực tế ([`examples/08_in_memory_live_stream.rs:249`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/examples/08_in_memory_live_stream.rs#L249)), phương thức chuẩn mực duy nhất được thiết kế để kết thúc phiên ghi là:
    ```rust
    writer.close().await;
    ```
- **Đề xuất sửa chữa trong README:**
  ```rust
  async fn ingest_with_writer(
      session: &mut PackagingSession,
      mut source_socket: tokio::net::TcpStream,
  ) -> Result<(), Box<dyn std::error::Error>> {
      let mut writer = session.writer();
      tokio::io::copy(&mut source_socket, &mut writer).await?;
      writer.close().await;
      Ok(())
  }
  ```

---

### 1.4 Snippet 1 & Snippet 2 — Xác minh tính hợp lệ

- **Snippet 1 (Setting Up an In-Memory Session with Static Keys):**
  - `StaticKeySource::shared_key(key)`: Hợp lệ ([`src/key/raw.rs:42`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/key/raw.rs#L42)).
  - `PackagingSessionConfig::new("live_stream_01")`: Hợp lệ ([`src/session/mod.rs:131`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L131)).
  - `.with_encryption_scheme(...)`, `.with_latency_mode(...)`, `.with_segment_duration(...)`, `.with_rendition(...)`: Đều tồn tại và hoạt động đúng.
  - `session.close().await?`: Hợp lệ vì `PackagingSession::close(&mut self)` trả về `Result<()>` ([`src/session/mod.rs:669`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L669)).
- **Snippet 2 (Live Packaging with Axinom DRM):**
  - `use drmpack::axinom::{AxinomConfig, AxinomProvider};`: Hợp lệ vì `src/lib.rs:85` có `pub use vendor::axinom;`.
  - `.with_all_drm()`: Hợp lệ ([`src/session/mod.rs:178`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L178)).
  - `let metadata = session.playback_metadata();`: Trả về `DrmStreamMetadata` ([`src/session/mod.rs:815`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L815)), và struct này có trường `pub keys: Vec<DrmKeyEntry>` ([`src/session/metadata.rs:41`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/metadata.rs#L41)), nên `metadata.keys.len()` biên dịch thành công.

---

## 2. Kiểm chứng Kiến trúc và Vòng đời (Architecture & Lifecycle)

### 2.1 Thời điểm Spawn Harvester: Sequence Diagram & Table vs Mã nguồn thực tế

- **Mô tả trong README:**
  - Sequence Diagram (Dòng 87–89):
    ```mermaid
    DP->>GP: Spawn gpac filter graph (cecrypt -> dasher) with stdin pipe
    DP->>AH: Spawn ArtifactHarvester background task watching staging
    DP-->>MS: PackagingSession handle ready
    ```
  - Bảng Packaging Flow Breakdown (Dòng 118, Step 5–6):
    > *"Spawns the gpac child process with an anonymous Unix pipe... and spawns the ArtifactHarvester background task to watch ephemeral staging (/tmp)."*
- **Mã nguồn thực tế ([`src/session/mod.rs:463-477`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L463-L477) và [`494-517`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L494-L517)):**
  - Trong `PackagingSession::create()`:
    ```rust
    Ok(Self {
        config,
        key_set,
        cluster,
        control_dir,
        lifecycle,
        is_terminal,
        has_pushed_media,
        heartbeat_tx,
        cancellation_token,
        watchdog_handle,
        preserve_output,
        harvester: None, // <--- HARVESTER LÀ NONE KHI CREATE!
        output_receiver_claimed: false,
    })
    ```
  - Trong `PackagingSession::take_output_receiver()`:
    ```rust
    pub fn take_output_receiver(&mut self) -> Option<mpsc::Receiver<PackagedArtifact>> {
        if self.output_receiver_claimed || self.is_closed() {
            return None;
        }
        self.output_receiver_claimed = true;
        // ...
        let harvester = Harvester::spawn(targets, tx);
        self.harvester = Some(harvester);
        Some(rx)
    }
    ```
  - Docstring tại dòng 490–493 giải thích rõ:
    > *"When claimed, activates an asynchronous background harvester that monitors the session staging directory ... Zero overhead if not claimed."*
- **Đánh giá sai lệch:**
  README mô tả sai hoàn toàn nguyên lý thiết kế của hệ thống. Harvester được khởi tạo trễ (**lazy initialization**) nhằm đảm bảo nguyên tắc *"Zero overhead if not claimed"*. Nếu caller không sử dụng output channel mà tự xử lý output hoặc kiểm thử độc lập, không có tác vụ background nào theo dõi filesystem. Việc sơ đồ tuần tự và bảng mô tả đặt bước spawn Harvester vào trong hàm `create()` là không đúng với thực tế mã nguồn.
- **Đề xuất sửa chữa trong README:**
  - Tách bước kích hoạt Harvester ra khỏi Phase 1 của sơ đồ tuần tự. Sơ đồ cần thể hiện rõ `MS->>DP: session.take_output_receiver()`, sau đó mới đến `DP->>AH: Harvester::spawn()`.
  - Cập nhật Step 5–6 trong bảng mô tả: `create()` chỉ khởi chạy GPAC child process; Harvester chỉ được kích hoạt khi caller lấy receiver.

---

### 2.2 Tuyên bố "Zero disk I/O on egress" vs [ADR-0015](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/docs/adr/0015-direct-output-channel-and-safe-storage.md)

- **Mô tả trong README:**
  - Tiêu đề phụ (Dòng 3):
    > *"Native Rust DRM packaging and manifest generation library orchestrating GPAC filters for CENC/CBCS fMP4 and HLS/DASH delivery with **zero disk I/O**."*
  - Đoạn Overview (Dòng 9):
    > *"**By bypassing physical disk writes entirely on both ingress and egress**, `drmpack` **eliminates disk wear**, reduces segment-to-manifest latency to sub-second ranges..."*
- **Mã nguồn thực tế và [ADR-0015](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/docs/adr/0015-direct-output-channel-and-safe-storage.md):**
  - **Ingress:** Hoàn toàn không qua đĩa, dữ liệu fMP4 được đẩy trực tiếp qua anonymous Unix pipe vào GPAC stdin (`src/session/mod.rs:573`, `src/gpac/process.rs:387`).
  - **Egress:** GPAC filter `dasher` bắt buộc phải xuất manifest và segment ra thư mục staging (`self.output_dir`, mặc định là `std::env::temp_dir()`, thường là `/tmp` trên Linux).
  - **Quyết định tại ADR-0015 ([`docs/adr/0015-direct-output-channel-and-safe-storage.md:17, 22-25`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/docs/adr/0015-direct-output-channel-and-safe-storage.md#L17)):**
    > *"3. Safe Disk-Backed Staging by Default: The default output directory shifts from `/dev/shm` to standard temporary disk storage (`/tmp` / OS tempdir) backed by local NVMe SSD. This eliminates kernel OOM risks while preserving sub-millisecond I/O through the Linux kernel Page Cache. Ramdisk (`/dev/shm`) remains available as an explicit opt-in configuration..."*  
    > *"Considered Options: Raw POSIX anonymous pipe output (`dasher -o pipe://` or `stdout`): Rejected because HLS/DASH is a multi-resource document tree ... GPAC explicitly discards manifests when forced to a pipe."*
  - **Cơ chế thu hoạch ([`src/session/harvester.rs:433, 478`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/harvester.rs#L433)):** `Harvester` đọc file từ đường dẫn `/tmp/...` vào RAM thông qua `std::fs::read` hoặc `tokio::fs::read`, sau đó gọi `std::fs::remove_file(&path)` để xóa file tạm.
- **Đánh giá sai lệch:**
  - Tuyên bố *"bypassing physical disk writes entirely on both ingress and egress"* và *"zero disk I/O"* là **tuyên bố phóng đại không đúng kỹ thuật**.
  - Việc lưu trữ mặc định là disk-backed staging (`/tmp`) trên ổ cứng cục bộ/SSD NVMe. Mặc dù Linux kernel tận dụng Page Cache cho các file tồn tại ngắn, nhưng kernel pdflush daemon vẫn có thể đồng bộ dirty pages xuống đĩa cứng vật lý và các thao tác cập nhật inode/directory entries vẫn ghi log trên filesystem journal.
  - Đích đến chỉ trở thành 100% RAM (thực sự không ghi đĩa) khi người dùng cấu hình thủ công ramdisk (`/dev/shm` hoặc tmpfs) thông qua `.with_output_dir()`.
- **Đề xuất sửa chữa trong README:**
  - Sửa dòng 3 và 9: Làm rõ rằng **ingress là pipe không qua đĩa (zero disk I/O)**, còn **egress sử dụng ephemeral staging** (mặc định là disk-backed `/tmp` tận dụng kernel page cache, hoặc opt-in ramdisk `/dev/shm` để đạt zero physical disk I/O) kết hợp tự động dọn dẹp (unlink immediately).

---

## 3. Kiểm chứng Tham số và Command GPAC

### 3.1 Cú pháp Stdin Pipe: `pipe://stdin:fmt=mp4` vs Mã nguồn thực tế

- **Mô tả trong README:** Dòng 118 ghi:
  > *"Spawns the `gpac` child process with an anonymous Unix pipe (`pipe://stdin:fmt=mp4`) connected to `stdin`"*
- **Mã nguồn thực tế ([`src/gpac/process.rs:172-193`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/gpac/process.rs#L172-L193)):**
  ```rust
  const STDIN_INPUT_FILTER: &str = concat!(
      "stdin:ext=mp4:alltk",
      ":#Representation=",
      "(video)video_$Height$p,",
      "(video)video,",
      "(audio)(Language=!und)audio_$Language$,",
      "(audio)audio,",
      "(text)(Language=!und)sub_$Language$,",
      "(text)sub",
      ":#HLSPL=",
      "(video)video_$Height$p.m3u8,",
      "(video)video.m3u8,",
      "(audio)(Language=!und)audio_$Language$.m3u8,",
      "(audio)audio.m3u8,",
      "(text)(Language=!und)sub_$Language$.m3u8,",
      "(text)sub.m3u8",
  );
  args.push("-i".into());
  args.push(STDIN_INPUT_FILTER.into());
  ```
- **Đánh giá sai lệch:**
  Chuỗi `pipe://stdin:fmt=mp4` là cú pháp bịa đặt/nhầm lẫn với FFmpeg hoặc GStreamer. Trong GPAC filter graph, stdin filter được chỉ định bằng cú pháp chuẩn: `-i stdin:ext=mp4:alltk:...`.
- **Đề xuất sửa chữa trong README:** Cập nhật lại mô tả chính xác đối số truyền cho GPAC stdin trong phần breakdown.

---

### 3.2 Tên tệp Init Segment: `init.mp4` vs `$RepresentationID$_init.mp4`

- **Mô tả trong README:**
  - Dòng 498: *"Consume encrypted segments (`.m4s`), initialization files (`init.mp4`), and playlists (`.m3u8`, `.mpd`)..."*
  - Dòng 515–517:
    ```rust
    ArtifactKind::InitSegment => {
        println!("Init segment ready: {}", artifact.relative_path);
    }
    ```
- **Mã nguồn thực tế ([`src/gpac/process.rs:230`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/gpac/process.rs#L230) và [`src/session/harvester.rs:336, 401, 1082`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/harvester.rs#L336)):**
  - GPAC dasher template:
    ```rust
    "template=$RepresentationID$_$Init=init$$Number$".into()
    ```
  - Harvester kiểm tra điều kiện nhận diện:
    ```rust
    let is_init = file_name.ends_with("init.mp4");
    ```
  - Tên tệp thực tế luôn có tiền tố Representation ID, ví dụ: `video_1080p_init.mp4`, `video_720p_init.mp4`, `video_init.mp4`, `audio_en_init.mp4`. Hoàn toàn không bao giờ tồn tại tệp mang tên đơn lẻ là `init.mp4`.
- **Đánh giá sai lệch:** Ghi `init.mp4` làm người đọc hiểu nhầm rằng chỉ có một tệp khởi tạo duy nhất cho toàn bộ luồng, trong khi mỗi Representation trong CMAF đều sở hữu init segment riêng biệt.
- **Đề xuất sửa chữa trong README:** Sửa thành `<rep_id>_init.mp4` (ví dụ: `video_1080p_init.mp4`).

---

## 4. Kiểm chứng Token Claims và Expiration Timestamps

- **Mô tả trong README:**
  - Sơ đồ tuần tự Playback (Dòng 141):
    > *"Note right of MS: JWT signed with Communication Key, containing KIDs, IVs, expiration"*
  - Bảng Playback Flow Breakdown (Dòng 187):
    > *"mints an Axinom DRM entitlement JWT signed with HMAC-SHA256 containing authorized KeyIDs, derived IVs, and expiration timestamps."*
- **Mã nguồn thực tế ([`src/vendor/axinom/token.rs:78-124`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/vendor/axinom/token.rs#L78-L124)):**
  ```rust
  let payload = serde_json::json!({
      "version": 1,
      "com_key_id": com_key_id,
      "message": {
          "type": "entitlement_message",
          "version": 2,
          "content_keys_source": {
              "inline": inline_entries
          }
      }
  });
  ```
  Trong đó `inline_entries` được tạo bởi:
  ```rust
  let inline_entries: Vec<serde_json::Value> = keys
      .iter()
      .map(|k| {
          let mut obj = serde_json::json!({ "id": k.kid });
          if let Some(iv) = k.iv {
              obj["iv"] = serde_json::Value::String(BASE64_STANDARD.encode(iv));
          }
          obj
      })
      .collect();
  ```
- **Đánh giá sai lệch:**
  - Hàm `generate_axinom_jwt` **hoàn toàn không có tham số `expiration`** và payload JSON **không hề chứa trường `exp` hay bất kỳ timestamp hết hạn nào**.
  - Đây là một chi tiết **hoàn toàn bịa đặt (hallucinated)** trong tài liệu `README.md`.
  - Nếu hệ thống phía client hoặc media-server kỳ vọng token tự động hết hạn dựa trên claim `exp`, họ sẽ bị hiểu lầm về tính an toàn của token hiện tại.
- **Đề xuất sửa chữa trong README:**
  - Xóa bỏ từ khóa "expiration" và "expiration timestamps" khỏi sơ đồ và bảng mô tả trong README.
  - (Tùy chọn) Nếu muốn hỗ trợ expiration trong tương lai, cần mở rộng hàm `generate_axinom_jwt` để nhận thêm `Option<SystemTime>` hoặc `valid_duration` và ghi vào payload `message.valid_until` theo đặc tả của Axinom.

---

## 5. Kiểm chứng API Signatures và Định danh Kiểu (Type Names)

### 5.1 `handle_fairplay_certificate` trong Sequence Diagram

- **Mô tả trong README:** Dòng 149 ghi:
  ```mermaid
  MS->>LP: handle_fairplay_certificate(&proxy)
  ```
- **Mã nguồn thực tế ([`src/license/proxy.rs:452-457`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/license/proxy.rs#L452-L457)):**
  ```rust
  pub async fn handle_fairplay_certificate(
      proxy: &LicenseProxy,
      cert_url: impl IntoCertUrl,
  ) -> Result<bytes::Bytes> {
      proxy.handle_fairplay_certificate(cert_url).await
  }
  ```
- **Đánh giá sai lệch:** Hàm độc lập `handle_fairplay_certificate` nhận **2 đối số**: `proxy: &LicenseProxy` và `cert_url: impl IntoCertUrl`. Khi muốn sử dụng URL chứng chỉ đã được cấu hình trong proxy, caller phải truyền `None::<&str>` hoặc chuỗi rỗng `""` (như đã triển khai tại [`examples/05_license_proxy_service.rs:39`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/examples/05_license_proxy_service.rs#L39)). Việc vẽ `handle_fairplay_certificate(&proxy)` tạo ấn tượng sai rằng hàm chỉ nhận 1 đối số.
- **Đề xuất sửa chữa trong README:** Đổi thành `handle_fairplay_certificate(&proxy, cert_url)` hoặc ghi chú rõ tham số thứ hai là `None::<&str>`.

---

### 5.2 Kiểu `ArtifactHarvester` vs Mã nguồn thực tế

- **Mô tả trong README:**
  README liên tục nhắc đến `ArtifactHarvester` tại các dòng 24, 60, 77, 88, 118, 120, 308, 572:
  > *"ArtifactHarvester detects finished segments..."*  
  > *"DP->>AH: Spawn ArtifactHarvester background task watching staging"*  
  > *"ArtifactHarvester: Background subsystem monitoring the staging directory..."*
- **Mã nguồn thực tế ([`src/session/harvester.rs:545`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/harvester.rs#L545) và [`src/lib.rs`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/lib.rs)):**
  - Struct trong mã nguồn có tên là `Harvester` (không có tiền tố `Artifact`).
  - Struct `Harvester` là kiểu nội bộ (**internal private struct**), không hề được export ra public API tại `src/lib.rs` hay `src/session/mod.rs`.
  - Caller hoàn toàn không thể import hay gọi bất kỳ phương thức nào của struct này; API công khai duy nhất để tương tác là phương thức `session.take_output_receiver()`.
- **Đánh giá sai lệch:**
  Việc viết `ArtifactHarvester` như một type độc lập trong phần Architecture và Glossary khiến người dùng thư viện cố gắng tìm kiếm struct `drmpack::session::ArtifactHarvester` trong docs hoặc code nhưng không thể tìm thấy.
- **Đề xuất sửa chữa trong README:**
  Làm rõ trong phần Glossary và Architecture: "Artifact Harvester" là tên khái niệm / subsystem nội bộ, được điều khiển thông qua phương thức `session.take_output_receiver()`, không phải là một kiểu dữ liệu công khai (public type).

---

## 6. Kiểm chứng Các Thành phần Khác

### 6.1 Bảng Feature Flags trong Cargo.toml vs README

- **Bảng trong README (Dòng 363–368):**
  | Feature Flag | Default | Description |
  | :--- | :--- | :--- |
  | `cpix` | Yes | DASH-IF CPIX 2.3 request builder, response parser, and `CpixProvider`. |
  | `speke-v2` | Yes | AWS SPEKE v2 wire client (`SpekeClient`) with SigV4 and token authentication. |
  | `axinom` | Yes | Axinom Key Service provider, signing utilities, and token generator. |
  | `license-proxy` | Yes | In-process DRM license proxy client, handlers, and FairPlay cert cache. |
- **Đối chiếu [`Cargo.toml:10-15`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/Cargo.toml#L10-L15):**
  ```toml
  [features]
  default = ["cpix", "speke-v2", "axinom", "license-proxy"]
  cpix = ["dep:reqwest", "dep:quick-xml"]
  speke-v2 = ["cpix", "dep:reqwest"]
  axinom = ["speke-v2", "dep:ring", "dep:serde_json"]
  license-proxy = ["dep:reqwest"]
  ```
- **Nhận xét:**
  Các feature flag và trạng thái mặc định (Default = Yes) hoàn toàn trùng khớp.
  *Lưu ý nhỏ về `speke-v2`:* Mô tả nói "with SigV4 and token authentication". Cần lưu ý rằng mã nguồn `speke/auth.rs` cung cấp struct [`SigV4Credentials`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/speke/auth.rs#L6) để chứa các header SigV4 đã được tính toán sẵn và trait [`SpekeSigner`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/speke/auth.rs#L85), chứ crate không tự động tính toán mã hóa AWS SigV4 từ access key/secret key (không có phụ thuộc `aws-sigv4`).

---

### 6.2 Biến Môi Trường (Environment Variables)

- **Các biến trong README (Dòng 318–341):**
  - `AXINOM_TENANT_ID`
  - `AXINOM_MANAGEMENT_KEY`
  - `AXINOM_ENDPOINT`
  - `AXINOM_OVERRIDE_KEY_IDS`
  - `AXINOM_COMMUNICATION_KEY_ID`
  - `AXINOM_COMMUNICATION_KEY`
  - `AXINOM_WIDEVINE_LICENSE_URL`
  - `AXINOM_FAIRPLAY_LICENSE_URL`
  - `AXINOM_PLAYREADY_LICENSE_URL`
  - `AXINOM_FAIRPLAY_CERT_URL`
- **Đối chiếu mã nguồn:**
  - `AxinomConfig::from_env` ([`src/vendor/axinom/config.rs:77-135`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/vendor/axinom/config.rs#L77-L135)): Đọc đúng các biến trên (hỗ trợ alias `AXINOM_KEY_SERVICE_MANAGEMENT_KEY` và `AXINOM_SPEKE_ENDPOINT`).
  - `AxinomSigningConfig::from_env` ([`src/vendor/axinom/token.rs:157-169`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/vendor/axinom/token.rs#L157-L169)): Đọc đúng `AXINOM_COMMUNICATION_KEY_ID` và `AXINOM_COMMUNICATION_KEY`.
  - `AxinomLicenseConfig::from_env` ([`src/vendor/axinom/config.rs:269-314`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/vendor/axinom/config.rs#L269-L314)): Đọc đúng `AXINOM_WIDEVINE_LICENSE_URL`, `AXINOM_FAIRPLAY_LICENSE_URL`, `AXINOM_PLAYREADY_LICENSE_URL`, và `AXINOM_FAIRPLAY_CERT_URL`.
- **Nhận xét:** Danh sách biến môi trường trong README hoàn toàn chính xác và khớp với logic đọc biến môi trường trong code. Tuy nhiên, repo hiện chưa có tệp `.env.example` vật lý tại thư mục gốc. Nên tạo tệp `.env.example` từ danh sách trong README để tiện cho người dùng clone dự án.

---

### 6.3 Hướng dẫn Cài đặt GPAC và Repo APT

- **Nội dung README (Dòng 222–238):**
  README hướng dẫn thêm repo APT của GPAC bằng deb822 source file trỏ đến `https://dist.gpac.io/gpac/linux/...` và tải GPG key từ `https://dist.gpac.io/gpac/linux/gpg.asc`.
- **Kiểm chứng mạng thực tế:**
  - URL `https://dist.gpac.io/gpac/linux/gpg.asc` tồn tại và trả về đúng PGP PUBLIC KEY BLOCK chính thức của "Team GPAC".
  - GPAC đã chuyển đổi quy tắc đặt phiên bản từ `v2.4.0` sang quy tắc năm/tháng `vYY.MM.0` (ví dụ `v26.07.0` phát hành tháng 7 năm 2026), do đó câu lệnh `git checkout v26.07.0` (dòng 258) là chính xác theo mô hình phát hành mới của GPAC.

---

## 7. Bảng Tổng Hợp Ma Trận Sai Lệch & Kế Hoạch Khắc Phục

| STT | Vị trí trong README | Mô tả trong README | Thực tế Mã nguồn | Mức độ Nghiêm trọng | Đề xuất Khắc phục |
| :---: | :--- | :--- | :--- | :---: | :--- |
| **1** | Dòng 510, 513, 516 (Snippet 4) | `artifact.relative_path` | Struct `PackagedArtifact` dùng trường `pub filename: String` ([`src/types.rs:326`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/types.rs#L326)). | **Nghiêm trọng (Compile Error)** | Đổi thành `artifact.filename`. |
| **2** | Dòng 513 (Snippet 4) | `artifact.sequence_number` | Struct `PackagedArtifact` **không có** trường này ([`src/types.rs:324`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/types.rs#L324)). | **Nghiêm trọng (Compile Error)** | Bỏ `artifact.sequence_number` khỏi snippet. |
| **3** | Dòng 544 (Snippet 5) | `handle_widevine_license(&state.proxy, auth_token, body)` | Thứ tự đối số là `(&proxy, challenge, auth_token)` ([`src/license/proxy.rs:425`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/license/proxy.rs#L425)). | **Nghiêm trọng (Compile Error)** | Đổi thành `handle_widevine_license(&state.proxy, body, auth_token)`. |
| **4** | Dòng 545 (Snippet 5) | `res.status_code()` | Struct `LicenseResponse` **không có** method này ([`src/license/response.rs:8`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/license/response.rs#L8)). | **Nghiêm trọng (Compile Error)** | Đổi thành `(axum::http::StatusCode::OK, res.data).into_response()`. |
| **5** | Dòng 491 (Snippet 3b) | `writer.shutdown().await?` | `poll_shutdown` không await `forward_task`. Phương thức chuẩn để drain là `writer.close().await` ([`src/session/mod.rs:958`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L958)). | **Đáng kể (Data Loss Risk)** | Đổi thành `writer.close().await;`. |
| **6** | Dòng 88, 118 (Diagram & Table) | `PackagingSession::create()` tự spawn `ArtifactHarvester`. | Harvester được lazy spawn chỉ khi gọi `take_output_receiver()` ([`src/session/mod.rs:494`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/mod.rs#L494)). | **Đáng kể (Lifecycle Inconsistency)** | Tách bước gọi `take_output_receiver()` ra khỏi Phase 1 trong Sequence Diagram. |
| **7** | Dòng 3, 9 (Title & Overview) | "zero disk I/O on egress", "bypassing physical disk writes entirely... eliminates disk wear". | Mặc định sử dụng Disk-backed staging (`/tmp`) theo ADR-0015; GPAC bắt buộc ghi file ra disk staging trước khi Harvester đọc và unlink. | **Đáng kể (Misleading Claim)** | Điều chỉnh: Ingress zero disk I/O qua pipe; Egress ephemeral staging (mặc định disk-backed `/tmp`, opt-in ramdisk `/dev/shm`). |
| **8** | Dòng 141, 187 (Playback Diagram & Text) | Axinom JWT chứa `expiration timestamps`. | `generate_axinom_jwt` không chứa claim `exp` hay bất kỳ timestamp nào ([`src/vendor/axinom/token.rs:99`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/vendor/axinom/token.rs#L99)). | **Trung bình (Hallucination)** | Xóa bỏ chữ "expiration" khỏi sơ đồ và mô tả tính năng. |
| **9** | Dòng 118 (Step 5 Breakdown) | GPAC stdin pipe cú pháp `pipe://stdin:fmt=mp4`. | Cú pháp GPAC filter thực tế là `-i stdin:ext=mp4:alltk:...` ([`src/gpac/process.rs:172`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/gpac/process.rs#L172)). | **Trung bình (Command Syntax)** | Cập nhật lại đúng cú pháp GPAC filter options. |
| **10** | Dòng 498, 516 | Tên file init segment là `init.mp4`. | GPAC dasher template sinh tên `<RepresentationID>_init.mp4` ([`src/gpac/process.rs:230`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/gpac/process.rs#L230)). | **Nhẹ (Naming Accuracy)** | Sửa thành `<rep_id>_init.mp4` (ví dụ `video_1080p_init.mp4`). |
| **11** | Dòng 149 (Playback Diagram) | `handle_fairplay_certificate(&proxy)` (1 tham số). | Signature nhận 2 tham số: `(&proxy, cert_url)` ([`src/license/proxy.rs:452`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/license/proxy.rs#L452)). | **Nhẹ (Signature Incomplete)** | Bổ sung tham số thứ hai (ví dụ `None::<&str>` hoặc `cert_url`). |
| **12** | Dòng 24, 60, 572 (Toàn bộ README) | Dùng tên `ArtifactHarvester` như public struct. | Struct tên là `Harvester` và là private struct bên trong `crate::session::harvester` ([`src/session/harvester.rs:545`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/session/harvester.rs#L545)). | **Nhẹ (Type Nomenclature)** | Làm rõ Artifact Harvester là tên subsystem nội bộ, không phải public struct. |

