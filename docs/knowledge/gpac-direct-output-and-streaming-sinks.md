# GPAC Direct Output & Pure Streaming Sinks: Nghiên Cứu Chuyên Sâu & Đánh Giá Khả Thi

> **Tài liệu nghiên cứu kỹ thuật chuyên sâu (Deep-Dive Research)**  
> **Áp dụng cho:** `drmpack` Core Engine, `media-server` Ingestion/Egress Plane, GPAC Subprocess Orchestration.  
> **Primary Sources:** GPAC Official Wiki (`wiki.gpac.io`), GPAC Source Tree (`gpac/gpac` filters), CTA-5003 / CTA-WAVE, DASH-IF Live Media Ingest Specification.

---

## 1. Tóm Tắt Cốt Lõi (Executive Summary)

Câu hỏi cốt lõi: **"GPAC có hỗ trợ nhả thẳng dữ liệu (direct output / pure streaming sink) mà không cần ghi file ra đĩa/filesystem hay không?"**

### Câu trả lời ngắn gọn:
1. **Đối với raw stream đơn lẻ (Single Track/Elementary Stream/MPEG-TS):** **CÓ**. GPAC hỗ trợ xuất thẳng ra `stdout`, POSIX pipe (`pipe://`), TCP/UDP/Unix Domain Socket (`sockout`), hoàn toàn không chạm đĩa.
2. **Đối với đóng gói thích ứng đa phân đoạn HLS/DASH (`dasher`):**
   - **Qua raw `stdout` hoặc POSIX anonymous pipe:** **KHÔNG THỂ**. Xuất HLS/DASH ra 1 pipe đơn lẻ gặp trở ngại vật lý không thể vượt qua: HLS/DASH là một **cây tài nguyên đa file (Multi-Resource Document Tree)** gồm manifests động liên tục cập nhật (`live.mpd`, `.m3u8`), init segments và hàng loạt media fragments độc lập (`.m4s`) của nhiều track (audio, video). Pipe là một dòng byte vô hướng (unstructured byte stream), không có siêu dữ liệu biên gói (framing) hay phân luồng kênh. Việc ép `dasher` ra pipe bằng cờ ép buộc `--template=pipe://...` theo tài liệu chính thức GPAC sẽ **hủy bỏ manifest ("trash the manifest")** và chỉ nhả một luồng mux thô.
   - **Qua Network Sink không dùng filesystem:** **CÓ**. GPAC cung cấp bộ lọc **`httpout:hmode=push`** hoạt động như một HTTP Client Sink (tuân thủ mô hình **DASH-IF Ingest / CMAF Ingest**), đẩy thẳng từng segment và manifest qua HTTP `PUT`/`POST` tới endpoint đích mà không cần bất kỳ thư mục lưu trữ cục bộ nào.
   - **Qua In-Memory Sink (`gmem://`):** **CÓ** ở mức độ nhúng máy chủ: `httpout:rdirs=gmem` cho phép lưu trữ và phục vụ segment trực tiếp từ RAM (`max_cache_segs`), nhưng máy chủ HTTP nhúng này nằm bên trong tiến trình GPAC.
   - **Qua C API FFI (`libgpac` Custom Filter):** **BỊ CHẶN BỞI THIẾT KẾ CỐT LÕI (Architectural Restriction)**. Tài liệu nhà phát triển GPAC chính thức quy định: *Custom filters tạo qua `gf_fs_new_filter` KHÔNG THỂ làm nguồn (source) hoặc đích (destination) cho các bộ lọc nạp đồ thị động như `dasher` hay `dashin`*.

---

## 2. Phân Tích Kỹ Thuật Chi Tiết Từng Hướng Điều Tra

---

### Hướng 1: Khả Năng Xuất Ra `stdout` / Anonymous Pipe

#### 1.1. Lệnh `gpac dasher` có thể xuất ra `stdout` hoặc pipe (`pipe://`, `-o stdout`) được không?
Trong GPAC, việc xuất dữ liệu ra file descriptor 1 (`stdout`) được đảm nhiệm bởi bộ lọc sink `fout` (File Output) khi cờ đích là `stdout` hoặc `std`.
Tuy nhiên, khi kết hợp với bộ lọc `dasher`, hành vi xuất hiện sự xung đột kiến trúc nghiêm trọng:

* **Hành vi thực tế của `dasher` với `pipe://`:**  
  GPAC cung cấp cờ mẫu cưỡng bức `tpl_force` (Forced-Template mode). Trích dẫn chính xác từ tài liệu chính thức GPAC (`wiki.gpac.io/Filters/dasher`):
  > *"When `tpl_force` is set, the template string is not analyzed nor modified for missing elements. This is used to trash the manifest and open [pipe] as the destination for the muxer result.  
  > Example: `gpac -i SRC -o null:ext=mpd:tpl_force --template=pipe://mypipe`"*

* **Hậu quả kỹ thuật:**  
  Chế độ này **cố tình loại bỏ việc sinh manifest** (`live.mpd` / `live.m3u8`) và chỉ chuyển hướng các byte payload của muxer ra một named pipe. Điều này biến pipeline thành một bộ multiplexer thông thường (fMP4 đơn luồng), đánh mất hoàn toàn đặc tính đóng gói thích ứng (Adaptive Streaming) của DASH/HLS.

#### 1.2. Trở ngại vật lý của việc xuất HLS/DASH ra 1 pipe duy nhất
Tại sao việc tống toàn bộ một phiên DASH/HLS vào một anonymous pipe lại bất khả thi về mặt khoa học máy tính?

1. **Bản chất của POSIX Anonymous Pipe (`pipe(2)`):**
   - Pipe là một hàng đợi byte tuần tự vào trước ra trước (FIFO), không có khái niệm gói tin (message boundary), không có cấu trúc phân nhánh cây file, và không hỗ trợ truy cập ngẫu nhiên (`non-seekable`).
2. **Bản chất của phiên HLS/DASH Live (Dual CENC/CBCS):**
   - Một phiên đóng gói trực tiếp sinh ra song song ít nhất 5-10 tài nguyên file độc lập đồng thời:
     - `live.mpd` (XML manifest định kỳ ghi đè nội dung mới).
     - `live.m3u8` (Master playlist).
     - `video_720p.m3u8`, `audio_eng.m3u8` (Variant playlists cập nhật trượt sliding-window).
     - `video_720p_init.mp4`, `audio_eng_init.mp4` (Init boxes `ftyp`+`moov`).
     - `video_720p_1.m4s`, `audio_eng_1.m4s`, `video_720p_2.m4s`... (Media segments).
3. **Hiện tượng "Interleaved Byte Soup" (Nhiễu loạn byte xen kẽ):**
   - Nếu ghi tất cả vào 1 pipe mà không có giao thức đóng gói phân kênh (multiplexing/framing protocol), các byte XML của manifest, byte ISO-BMFF của video và byte AAC của audio sẽ bị trộn lẫn vào nhau ở cấp độ kernel pipe buffer (64 KB).
   - Phía consumer (Rust) hoàn toàn không có cách nào:
     - Phân định ranh giới: Byte nào thuộc về file nào.
     - Phân loại: Đây là segment video mới hay là bản cập nhật đè lên manifest cũ.
     - Định tuyến: Đẩy chunk nào cho client đang xem video, chunk nào cho client lấy playlist.

#### 1.3. GPAC có cơ chế Framing nào (Tar stream, Multipart) qua Pipe không?
* **Tar Stream:** GPAC **không** tích hợp bộ lọc tạo file nén hay tar archive trên pipe output của `dasher`.
* **Multipart/MIME:** GPAC hỗ trợ Multipart/MIME trong bộ lọc `routeout` (gửi manifest + S-TSID signaling) hoặc HTTP chunked transfer, nhưng **không** hỗ trợ multipart stream trên POSIX pipe.
* **GSF (GPAC Serialized Format - `gsfmx`):**  
  GPAC phát triển riêng một giao thức nhị phân độc quyền gọi là **GSF** (`wiki.gpac.io/Filters/gsfmx`).  
  - Bộ lọc `gsfmx` có khả năng tuần tự hóa (serialize) toàn bộ trạng thái PID, thuộc tính (`GF_PROP_PID_FILE_NAME`, `CueStart`), sự kiện End-Of-Stream và payload thành một luồng nhị phân duy nhất qua pipe hoặc socket:  
    `gpac -i source.mp4 gsfmx:dst=manifest.mpd:mixed -o dump.gsf`  
  - **Rào cản:** GSF là định dạng nội bộ đặc thù của GPAC. Nếu muốn hứng GSF trong Rust qua pipe, `drmpack` sẽ phải tự viết một parser hoàn chỉnh giải mã binary framing của GSF (hoặc biên dịch C demuxer `gsfdmx`), tạo ra gánh nặng bảo trì và độ phức tạp phi lý.

---

## 3. Các Filter Sinks Không Dùng Filesystem Của GPAC

---

### 2.1. `httpout:hmode=push` (HTTP Client Sink - Chuẩn CMAF Ingest)

Đây là cơ chế chính quy và mạnh mẽ nhất của GPAC để xuất dữ liệu trực tiếp không chạm đĩa.

#### A. Cách thức hoạt động
Bộ lọc `httpout` (`wiki.gpac.io/Filters/httpout`) thông thường hoạt động như một HTTP Server cục bộ. Tuy nhiên, khi được cấu hình `hmode=push`, bộ lọc này biến đổi thành một **HTTP Client**:
* Thay vì ghi các file segment và manifest ra đĩa, `dasher` chuyển tiếp các packet PID kiểu `FILE` sang `httpout`.
* `httpout` khởi tạo các kết nối HTTP ra bên ngoài, sử dụng phương thức **`PUT`** (mặc định) hoặc **`POST`** (khi có cờ `post=true`) để đẩy từng tài nguyên lên máy chủ HTTP đích.
* **Tài liệu GPAC nêu rõ:**  
  > *"In push mode, the filter does not need a local read or write directory because it sends data directly to the remote URL."*

#### B. Cú pháp CLI chuẩn từ Primary Sources
```bash
gpac -i source reframer:rt=on -o http://127.0.0.1:8080/live/live.mpd:gpac:segdur=2:cdur=0.2:profile=live:dmode=dynamic:hmode=push:llhls=br
```
Khi chạy lệnh trên:
1. `dasher` sinh manifest `live.mpd` -> `httpout` phát HTTP request: `PUT /live/live.mpd`.
2. `dasher` sinh init segment -> `httpout` phát: `PUT /live/video_init.mp4`.
3. `dasher` sinh media segment 1 -> `httpout` phát: `PUT /live/video_1.m4s`.
4. Với chế độ Low-Latency (`cdur=0.2`), `httpout` sử dụng HTTP `Transfer-Encoding: chunked` để đẩy từng chunk CMAF ngay khi muxer vừa hoàn thành, giảm thiểu độ trễ tối đa.

#### C. Đối chiếu chuẩn công nghiệp
Cơ chế này tuân thủ trực tiếp chuẩn **DASH-IF Live Media Ingest Specification** (hay còn gọi là **CMAF Ingest**):
* **Interface 1 (CMAF Track Push):** Đẩy từng track CMAF phân mảnh liên tục qua kết nối HTTP dài hạn.
* **Interface 2 (DASH/HLS Presentation Ingest):** Đẩy các đối tượng đóng gói (Manifests + Segments) độc lập qua HTTP PUT/POST lên Origin/Packager.

#### D. Đánh giá hiệu năng & Điểm yếu thực tế
* **Ưu điểm:** Loại bỏ 100% việc ghi đĩa; tương thích hoàn hảo với các CDN Ingest Origin (như AWS Elemental MediaStore, Akamai, hoặc custom local Rust server).
* **Rủi ro thực tế (từ GPAC GitHub Issues #3027, #2923):**
  - **Issue #3027:** Bộ client HTTP của GPAC gặp các lỗi không ổn định khi xử lý SSL/TLS và lỗi `Multiple input PIDs with no file name set` khi xử lý nhiều PID đồng thời. Kết nối chỉ hoạt động ổn định nhất ở chế độ thuần HTTP không mã hóa (ví dụ `http://127.0.0.1:port`).
  - **Issue #2923:** Trong các phiên live kéo dài nhiều ngày, việc duy trì HTTP client push liên tục có hiện tượng tích lũy bộ nhớ nếu máy chủ nhận phản hồi chậm hoặc ngắt kết nối đột ngột giữa chừng.

---

### 2.2. `sockout` (TCP / UDP / Unix Domain Socket)

Bộ lọc `sockout` (`wiki.gpac.io/Filters/sockout`) cho phép GPAC mở socket mạng ở chế độ blocking:
* **Hỗ trợ giao thức:**
  - `tcp://<ip>:<port>` (TCP socket)
  - `udp://<ip>:<port>` (UDP socket)
  - `tcpu://<path>` (TCP Unix Domain Socket trên Linux/macOS)
  - `udpu://<path>` (UDP Unix Domain Socket)
* **Khả năng đối với `dasher`:**  
  `sockout` chỉ chấp nhận các PID dạng dòng dữ liệu đơn lẻ (ví dụ: elementary video AVC/HEVC, AAC audio, hoặc MPEG-TS qua cờ `:ext=ts`). `sockout` **không thể** nhận diện PID kiểu `FILE` từ `dasher` để phân tách nhiều file HLS/DASH. Do đó, `sockout` **không thể** được sử dụng làm sink trực tiếp cho DASH/HLS thích ứng.

---

### 2.3. `routeout` (ROUTE / FLUTE trong ATSC 3.0)

Bộ lọc `routeout` (`wiki.gpac.io/Filters/routeout`) triển khai giao thức ROUTE theo chuẩn ATSC 3.0:
* Nhận các PID kiểu `FILE` từ `dasher` và phát multicast qua UDP: `gpac -i DASH_URL -o route://225.1.1.1:1234/manifest.mpd`.
* Không phù hợp cho kiến trúc microservice server unicast cục bộ.

---

### 2.4. `gmem://` & GPAC In-Memory Virtual Filesystem (`gfio`)

* Khi cấu hình `--rdirs=gmem`, `httpout` kích hoạt chế độ **Memory Mode**: Các phân đoạn sinh ra từ `dasher` được lưu thẳng trong RAM và phục vụ qua HTTP server nhúng của GPAC.
* Tầng C API cung cấp struct `GF_FileIO` với các hàm callback `gf_fileio_new_mem()`.

---

## 4. Khác Biệt Giữa GPAC CLI (Subprocess) vs `libgpac` (C Library FFI)

### Rào cản cốt lõi từ tài liệu GPAC (Primary Source Citation)
Trong tài liệu hướng dẫn viết bộ lọc tùy biến của GPAC (**"Writing a custom Filter"** - `wiki.gpac.io/Developers/tutorials/Writing-a-custom-Filter/`), nhóm phát triển GPAC nêu rõ giới hạn bất di bất dịch của bộ lọc ứng dụng (`gf_fs_new_filter`):

> **"Limitations:**  
> - *Custom filters cannot have arguments exposed.*  
> - *Custom filters **cannot act as sources or destinations for filters that load graphs dynamically (like `dashin` or `dasher`)**.*  
> - *Custom filters cannot be cloned."*

**Hệ quả:** Bạn **KHÔNG THỂ** viết một struct Rust, export hàm callback C và cắm nó làm sink hứng trực tiếp output của `dasher` trong bộ nhớ.

---

## 5. Ma Trận So Sánh Kỹ Thuật & Đánh Giá Khả Thi Thực Tế

| Tiêu Chí So Sánh | (A) File Staging + Unlink trên SSD / tmpfs *(Khuyến nghị)* | (B) GPAC HTTP PUSH vào Localhost Rust Server (`httpout:hmode=push`) | (C) Custom In-Memory Sink (GSF Pipe / `libgpac` FFI) |
| :--- | :--- | :--- | :--- |
| **Bản chất I/O** | Ghi vào Linux Page Cache RAM -> Rust đọc -> Unlink. | Truyền qua Loopback Socket TCP trong RAM. Zero filesystem. | Truyền qua Pipe nhị phân hoặc Con trỏ bộ nhớ C-Rust. |
| **Độ trễ (Latency)** | **Cực thấp (< 1ms)**: Syscall `write()` vào kernel RAM page. | **Rất thấp (1-3ms)**: Thêm chi phí đóng gói HTTP header & TCP stack loopback. | **Cực thấp (< 0.5ms)** (nếu dùng FFI pointer). |
| **Mức độ phụ thuộc FS** | Có (yêu cầu một thư mục tạm nhỏ). | **Hoàn toàn không** phụ thuộc filesystem. | Hoàn toàn không phụ thuộc filesystem. |
| **Độ phức tạp kiến trúc** | **Rất đơn giản**: Tận dụng cờ CLI tiêu chuẩn của GPAC, code ngắn gọn. | **Trung bình - Cao**: Rust phải dựng máy chủ HTTP nhận `PUT`, quản lý port, xử lý connection reset. | **Cực kỳ cao / Bị cấm bởi GPAC architecture**. |
| **Độ ổn định & Cô lập lỗi** | **Tuyệt đối**: GPAC crash có `ProcessSupervisor` bắt lỗi. | **Khá**: Rủi ro lỗi HTTP client của GPAC (Issues #3027, #2923). Port có thể bị chiếm dụng. | **Kém / Nguy hiểm**: FFI C làm sập tiến trình Rust; parse binary stream dễ lỗi đồng bộ. |
| **Hỗ trợ Dual CENC + CBCS** | **Hoàn hảo**: 2 tiến trình ghi vào 2 thư mục con `cenc/` và `cbcs/` tách bạch. | **Phức tạp**: Cần map 2 đường dẫn URL khác nhau (`/cenc/..`, `/cbcs/..`) trên HTTP server. | Rất phức tạp để multiplex đồng thời 2 luồng. |
| **Độ tương thích chuẩn** | Chuẩn POSIX File System & DASH/HLS File Structure. | Chuẩn **CMAF Ingest (DASH-IF Ingest Interface 2)**. | Định dạng độc quyền (Proprietary GPAC format). |

---

## 6. Đánh Giá Kiến Trúc Theo Triết Lý "/ponytail" (Senior Minimalist Perspective)

1. **Mục tiêu thực sự là gì?**  
   Tránh làm chai mòn SSD, tránh nghẽn I/O, và triệt tiêu nguy cơ OOM RAM trong container.
2. **Hệ điều hành đã giải quyết điều này chưa?**  
   **RỒI.** Khi ghi vào thư mục tạm trên Linux (`/tmp` hoặc SSD NVMe), lệnh ghi chỉ ghi vào **Page Cache (RAM)** của kernel với độ trễ nano-giây.
   - Khi `drmpack` đọc byte lên để bắn qua channel rồi gọi `unlink`, các file tạm bị xóa ngay lập tức.
   - Ổ đĩa SSD hầu như không phải quay hay ghi block NAND nào vì file bị xóa khi còn đang nằm trong Page Cache bẩn (dirty pages).
3. **Cái giá phải trả nếu cố ép GPAC "nhả thẳng không qua file":**
   - Dùng **HTTP Client Sink (`hmode=push`)**: Phải dựng một mini HTTP server trong Rust, mở cổng loopback, bắt lỗi TCP socket, xử lý race condition kết nối mạng — **tốn thêm hàng trăm dòng boilerplate code** chỉ để đạt được cùng một kết quả mà Page Cache đã làm tốt hơn.
   - Dùng **FFI C / Custom Filter**: Bị GPAC **chặn hoàn toàn** vì `dasher` không cho phép custom filter làm destination sink.
