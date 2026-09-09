# Báo Cáo Nghiên Cứu Chuyên Sâu: Mức Độ An Toàn, Rủi Ro Bộ Nhớ và Kiến Trúc Bảo Vệ Khi Ghi Dữ Liệu Đóng Gói Video Vào Ramdisk / tmpfs Trong Môi Trường Production

**Tài liệu đích:** `docs/knowledge/ramdisk-safety-and-memory-risks.md`  
**Dự án:** `drmpack` (Ermis Stream Multi-DRM Packaging Engine)  
**Trạng thái:** Hoàn thành nghiên cứu từ Primary Sources  
**Ngày thực hiện:** 2026-09-08  

---

## 1. Tóm Tắt Điều Hành (Executive Summary) & Bối Cảnh

Trong kiến trúc ban đầu của `drmpack` ([ADR-0005](../adr/0005-ramdisk-tmpfs-manifest-distribution.md)), Ramdisk (`/dev/shm` trên Linux hoặc `tmpfs`) được lựa chọn làm nơi chứa đầu ra đóng gói (HLS `.m3u8`, DASH `.mpd` và các media segment CMAF `.m4s`). Quyết định này nhằm triệt tiêu độ trễ I/O đĩa và loại trừ hiện tượng bào mòn đĩa cứng (disk wear) do chu kỳ ghi đè manifest liên tục ở tần suất cao (200ms – 2s) trong các luồng phát Low-Latency Live.

Tuy nhiên, việc triển khai giải pháp này vào **môi trường sản xuất quy mô lớn (Linux bare-metal, Docker containers, Kubernetes Pods)** bộc lộ **5 hiểm họa tiềm tàng nghiêm trọng** nếu không được kiểm soát chặt chẽ:

1. **Bẫy sập 64MB mặc định của Container:** Docker và Kubernetes (CRI-O / containerd) mặc định chỉ cấp đúng **64 MiB** cho `/dev/shm`. Với một luồng phát đa bitrate 1080p-720p-360p, dung lượng 64MB bị lấp đầy chỉ sau **28 đến 56 giây**, khiến GPAC crash do lỗi ghi `ENOSPC` (No space left on device) và làm sập toàn bộ phiên live.
2. **Cửa sổ lưu trữ 30 phút (`tsb=1800`) ngốn hàng Gigabyte RAM:** Hiện tại `src/gpac/process.rs:128` đang hardcode cờ `tsb=1800` (Time-Shift Buffer 30 phút). Dù GPAC có cơ chế tự xóa segment cũ khi ra khỏi window (`keep_segs=false`), trong 30 phút đầu tiên GPAC **hoàn toàn không xóa bất kỳ segment nào**. Một luồng đa bitrate CENC + CBCS (Dual Scheme) sẽ tích lũy tới **4.1 GB** dữ liệu trong RAM. Chỉ cần 5 luồng chạy đồng thời trên một node, dung lượng RAM bị chiếm dụng sẽ vượt quá 20 GB!
3. **Cgroup Double-Accounting & OOM Killer (Exit Code 137):** Trong Linux cgroup v2 và Kubernetes, bộ nhớ `tmpfs` được tính trực tiếp vào hạn mức bộ nhớ của container (`memory.current` / `resources.limits.memory`). Nếu kỹ sư cấu hình `emptyDir.medium: Memory` có dung lượng 4GB nhưng đặt container limit là 2GB, Kernel OOM Killer sẽ gửi tín hiệu `SIGKILL` tiêu diệt tiến trình ngay lập tức.
4. **Rò rỉ vĩnh viễn khi tiến trình chết đột ngột (Zombie / Orphan Leakage):** Khác với bộ nhớ ẩn danh (anonymous memory) được giải phóng tự động khi process terminate, `tmpfs` là một VFS mount point do kernel quản lý. Khi tiến trình bị `SIGKILL`, crash, segfault hoặc OOM, cơ chế RAII (`Drop` trong Rust) **hoàn toàn không được kích hoạt**. Hàng Gigabyte media segments bị kẹt vĩnh viễn trong RAM host. Trong kịch bản Kubernetes `CrashLoopBackOff`, RAM của node máy chủ sẽ bị vắt kiệt chỉ sau vài lần khởi động lại.
5. **Hiểu lầm về tốc độ SSD NVMe và Linux Page Cache:** Các phân tích phần cứng hiện đại chỉ ra rằng lệnh `write(2)` ra SSD thực chất ghi vào **Linux Page Cache trên RAM** với độ trễ nano-giây ngang ngửa `tmpfs`. SSD không hề gây nghẽn I/O cho media segments (200KB – 2MB) và quan trọng nhất: **Page Cache trên SSD có thể tự động evict/flush ra đĩa khi bộ nhớ cạn kiệt**, đóng vai trò như một van xả an toàn triệt tiêu nguy cơ OOM Killer, điều mà `tmpfs` không có Swap hoàn toàn bất lực.

---

## 2. Điều Tra Chuyên Sâu 5 Rủi Ro Cốt Lõi Từ Tài Liệu Gốc (Primary Sources)

### 2.1 Cơ chế tmpfs và /dev/shm trên Linux Kernel & Hành vi OOM Killer

#### Tài liệu tham chiếu gốc (Primary Sources)
* Linux Kernel Documentation: `Documentation/filesystems/tmpfs.rst`
* Linux Kernel Source: `mm/shmem.c`, `mm/oom_kill.c`, `mm/memcontrol.c`
* Linux Programmer's Manual: `tmpfs(5)`, `shm_overview(7)`, `cgroups(7)`

#### Cơ chế phân bổ và quản lý trang nhớ
Theo Linux Kernel Documentation (`tmpfs.rst`), `tmpfs` là một hệ thống tệp lưu trữ toàn bộ dữ liệu trực tiếp trong bộ nhớ ảo (Virtual Memory) của kernel, cụ thể là **Page Cache** và **dentry cache**:
* **Không cấp phát trước (Dynamic Allocation):** `tmpfs` không chiếm dụng ngay dung lượng tối đa. Dung lượng thực tế tiêu hao đúng bằng kích thước các tệp đang lưu trữ cộng với metadata (inode).
* **Kích thước mặc định:** Nếu mount mà không truyền tham số `size`, kernel mặc định giới hạn dung lượng `tmpfs` bằng **50% lượng RAM vật lý của host** (`size=50%`).
* **Tương tác với Swap:** Các trang nhớ của `tmpfs` được quản lý dưới dạng bộ nhớ ẩn danh được ánh xạ tệp (`shmem`). Khi hệ thống gặp áp lực bộ nhớ (memory pressure), tiến trình dọn dẹp trang (`kswapd`) **CÓ THỂ hoán chuyển (swap out) các trang `tmpfs` ra phân vùng Swap vật lý**. Tuy nhiên, trong hạ tầng container hiện đại (đặc biệt là Kubernetes), Swap thường bị vô hiệu hóa hoàn toàn (`swapoff -a`) theo khuyến cáo chuẩn của Kubernetes. Khi không có Swap, toàn bộ các trang nhớ của `tmpfs` bị **ghim cứng (pinned) vào RAM vật lý**.

#### Phân biệt: Tràn đĩa (`ENOSPC`) vs Tràn bộ nhớ (`OOM Killer`)
Kernel xử lý hai trường hợp cạn kiệt bộ nhớ theo hai cơ chế hoàn toàn khác nhau:

```
                          ┌──────────────────────────┐
                          │ Lệnh ghi file: write(2)  │
                          └─────────────┬────────────┘
                                        │
                 ┌──────────────────────┴──────────────────────┐
                 ▼                                             ▼
       [tmpfs chạm size limit]                   [RAM / cgroup memory chạm limit]
                 │                                             │
      VFS trả lỗi: -ENOSPC                               Kernel không cấp được trang nhớ
    "No space left on device"                                  │
                 │                               ┌─────────────┴─────────────┐
        Process nhận lỗi I/O                     ▼                           ▼
        (OOM Killer KHÔNG chạy)             [Có Swap]                   [Không Swap]
                 │                               │                           │
   GPAC exit với mã lỗi I/O              Swap out tmpfs             Kernel cgroup kích hoạt
                                                                     mem_cgroup_out_of_memory()
                                                                             │
                                                                    OOM Killer chọn victim
                                                                             │
                                                                   Gửi SIGKILL (Exit 137)
```

1. **Trường hợp A - Vượt quá hạn mức `size` của tmpfs (`ENOSPC`):**
   * Nếu `/dev/shm` được gán hạn mức (ví dụ 64MB trên Docker) và dữ liệu ghi vượt quá 64MB, VFS subsystem từ chối lệnh `write(2)` và trả về mã lỗi **`-ENOSPC` (No space left on device)**.
   * **Hành vi Kernel:** Kernel **KHÔNG** kích hoạt OOM Killer. Lỗi được trả về cho tiến trình gọi. Nếu ứng dụng (GPAC) không bắt lỗi ghi đĩa, nó sẽ ngắt luồng và thoát với lỗi I/O.
2. **Trường hợp B - Cạn kiệt RAM hệ thống hoặc vượt `memory.max` của cgroup (OOM Killer):**
   * Nếu `tmpfs` có hạn mức lớn (hoặc không giới hạn), và dữ liệu ghi vào `tmpfs` làm cạn kiệt RAM vật lý của node hoặc vượt quá ngưỡng `memory.max` (cgroup v2) / `memory.limit_in_bytes` (cgroup v1) của Container:
   * Trong cgroup v2, các trang nhớ `tmpfs` do tiến trình bên trong container tạo ra được tính trực tiếp vào trường `shmem` và `file` của `memory.current`.
   * Khi `memory.current` chạm trần `memory.max`, bộ điều khiển bộ nhớ (`mem_cgroup`) bắt đầu chu kỳ dọn dẹp (reclaim). Vì các trang `tmpfs` là trang file bẩn (dirty file pages) nhưng **không có ổ đĩa vật lý để flush xuống**, và Swap bị tắt, kernel hoàn toàn bất lực trong việc thu hồi bộ nhớ!
   * Hàm `mem_cgroup_out_of_memory()` trong `mm/memcontrol.c` được kích hoạt. Thuật toán `oom_badness()` tính điểm tiến trình chiếm nhiều bộ nhớ nhất (thường chính là GPAC hoặc media server) và gửi tín hiệu **`SIGKILL` (signal 9)** không thể đánh chặn. Container dừng đột ngột với **Exit Code 137 (`128 + 9`)**.

---

### 2.2 Rủi Ro Trong Môi Trường Container (Docker & Kubernetes)

#### Tài liệu tham chiếu gốc (Primary Sources)
* Docker Run Reference: Command-line reference (`--shm-size`, `/etc/docker/daemon.json`)
* Kubernetes Documentation: *Volumes: emptyDir*, *Assign Memory Resources to Containers and Pods*
* OCI Runtime Specification / runc implementation

#### Bẫy 64MB của Docker Engine
* Mặc định, khi khởi tạo bất kỳ container nào bằng `docker run`, Docker Engine gắn kết một mount point `tmpfs` vào `/dev/shm` với kích thước cố định là **67,108,864 bytes (đúng 64 MiB)**.
* Thiết lập 64MB này bắt nguồn từ lý do bảo mật lịch sử (tránh việc một tiến trình unprivileged làm cạn kiệt POSIX shared memory của host).
* **Hậu quả với video streaming:** Một luồng phát 1080p đa bitrate đạt tốc độ trung bình ~1.15 MB/s. Chỉ sau **56 giây** (với 1 scheme) hoặc **28 giây** (với Dual scheme CENC + CBCS), dung lượng 64MB bị lấp đầy 100%. GPAC lập tức dính lỗi:
  ```text
  [dasher] Error writing segment to /dev/shm/drmpack_live_123/cenc/video_1080p_15.m4s: No space left on device
  ```
  Tiến trình GPAC chết, đường ống Unix pipe bị đóng gãy (`BrokenPipe`), và `PackagingSession` sập hoàn toàn.
* **Cách khắc phục thủ công:** Bắt buộc phải truyền cờ `--shm-size=2gb` trong `docker run` hoặc cấu hình `shm_size: 2gb` trong `docker-compose.yml`.

#### Cạm Bẫy Cấu Hình Trên Kubernetes (K8s Pods)
Mặc định, các Pod chạy trên Kubernetes kế thừa cấu hình runtime từ containerd/CRI-O và cũng chỉ có **64 MiB** trong `/dev/shm`. Để mở rộng, kỹ sư bắt buộc phải dùng `emptyDir` với `medium: Memory`:

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: video-packager
spec:
  containers:
  - name: packager
    image: ermis/drmpack-service:latest
    resources:
      limits:
        memory: "2Gi" # CẠM BẪY 1: Giới hạn cgroup container
    volumeMounts:
    - mountPath: /dev/shm
      name: shm-volume
  volumes:
  - name: shm-volume
    emptyDir:
      medium: Memory
      sizeLimit: "4Gi" # CẠM BẪY 2: sizeLimit của volume
```

Có 3 cạm bẫy sống còn trong mô hình này:
1. **Bẫy Cgroup Double-Accounting:**
   * Dữ liệu ghi vào `emptyDir.medium: Memory` được tính trực tiếp vào mức tiêu thụ bộ nhớ của Pod/Container.
   * Ở ví dụ trên, kỹ sư cấp `sizeLimit: 4Gi` cho `/dev/shm`, nhưng `resources.limits.memory` của container lại chỉ đặt `2Gi`. Khi video buffer tích lũy đến ~1.8GB (cộng thêm 200MB RSS của tiến trình), **Pod bị Kernel OOMKilled ngay lập tức**, dù dung lượng `/dev/shm` mới dùng chưa đến 50% `sizeLimit`!
2. **Bẫy Kubelet Eviction:**
   * Kubelet định kỳ chạy tiến trình giám sát dung lượng các volume `emptyDir`. Nếu thư mục `/dev/shm` vượt quá `sizeLimit`, Kubelet sẽ đánh dấu Pod vi phạm và tiến hành **Evict Pod** khỏi Node (`PodTheNodeWasLowOnResource`).
3. **Bẫy Bỏ Trống `sizeLimit` (Host Starvation):**
   * Nếu khai báo `emptyDir: { medium: Memory }` mà không đặt `sizeLimit`, dung lượng của volume này bị giới hạn bởi dung lượng bộ nhớ của toàn bộ Node vật lý. Khi gặp sự cố rò rỉ hoặc phiên live kéo dài, Pod có thể nuốt chửng hàng chục GB RAM của Node, kích hoạt `NodeMemoryPressure` và làm sập các dịch vụ đồng cấp (co-located pods).

---

### 2.3 Hành Vi Xóa Segment Của GPAC dasher & Phân Tích Định Lượng Dung Lượng

#### Tài liệu tham chiếu gốc (Primary Sources)
* GPAC Documentation: Filter `dasher` parameters (`gpac -h dasher`)
* GPAC Source Code: `src/filters/dasher.c` (Logic `dasher_del_segment`, `keep_segs`, `tsb`)

#### Cơ chế của cờ `tsb` và `keep_segs`
Trong GPAC dasher, vòng đời của các segment file được điều khiển bởi hai tham số:
1. **`tsb` (Time-Shift Buffer - kiểu số thực, mặc định của GPAC là 30 giây):** Quy định độ sâu thời gian tối đa của cửa sổ trượt DVR trong manifest DASH/HLS.
2. **`keep_segs` (boolean, mặc định: `false`):**
   * Khi `keep_segs=false` (mặc định trong GPAC và trong `drmpack`): GPAC **TỰ ĐỘNG XÓA** các segment vật lý trên đĩa/RAM khi thời điểm phát sóng của segment đó nằm ngoài cửa sổ:  
     $$\text{segment\_start\_time} < \text{current\_playback\_time} - \text{tsb}$$
   * Khi `keep_segs=true`: GPAC giữ lại toàn bộ các segment từ đầu buổi phát sóng đến khi kết thúc (dùng cho VOD archiving).

#### "Quả Bom Nổ Chậm" 30 Phút Của `tsb=1800`
Trong mã nguồn hiện tại của `drmpack` ([`src/gpac/process.rs:128`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/gpac/process.rs#L128)):
```rust
"{}:dual:profile=live:dmode=dynauto:segdur={}:spd={}:tsb=1800:utcs=inband:pssh=mv:template=$RepresentationID$_$Init=init$$Number$"
```
Tham số `tsb=1800` (1800 giây = 30 phút) dẫn đến hệ quả:
* **Giai đoạn tích lũy tuyến tính (0 đến 30 phút):** Trong suốt 1800 giây đầu tiên kể từ khi bắt đầu stream, **GPAC KHÔNG XÓA BẤT KỲ 1 BYTE NÀO**! Toàn bộ video segments của tất cả các rendition và scheme liên tục dồn ứ vào `/dev/shm`.
* **Giai đoạn bão hòa (Sau phút thứ 30):** Chỉ từ giây thứ 1801 trở đi, GPAC mới bắt đầu xóa segment số 1 khi segment mới được tạo ra, đưa mức tiêu hao RAM vào trạng thái cân bằng động (steady state).

#### Bảng Tính Toán Định Lượng Dung Lượng Ramdisk

Giả định một cấu hình phát sóng thực tế chuẩn công nghiệp (Multi-bitrate ABR Ladder):
* **Rendition 1080p60:** 5,000 kbps (~625 KB/s)
* **Rendition 720p30:** 2,500 kbps (~312.5 KB/s)
* **Rendition 360p30:** 800 kbps (~100 KB/s)
* **Stereo AAC Audio:** 128 kbps (~16 KB/s)
* **Subtitles & Metadata:** 32 kbps (~4 KB/s)
* **Hệ số đóng gói CMAF & Filesystem Inode Overhead:** 8% (chứa box `moof`, `traf`, `tfhd`, `trun`, `sidx`, `pssh`, metadata `senc`/`saiz`/`saio` và block allocation overhead).

*Tổng băng thông thực tế (Single Scheme):* $8,460\text{ kbps} \times 1.08 \approx 9,136\text{ kbps} \approx 1.142\text{ MB/s}$  
*Tổng băng thông thực tế (Dual Scheme CENC + CBCS):* $1.142\text{ MB/s} \times 2 \approx 2.284\text{ MB/s}$

| Cửa sổ Buffer (`tsb`) | Single Scheme (`MB` / `GiB`) | Dual Scheme CENC+CBCS (`MB` / `GiB`) | 5 Luồng Đồng Thời (Dual) | 10 Luồng Đồng Thời (Dual) | Đánh giá an toàn |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **30 giây** (`tsb=30`) | 34.3 MB (0.03 GiB) | **68.5 MB (0.07 GiB)** | 342.5 MB | 685 MB | **Rất an toàn**, vừa vặn cho RAM nhỏ |
| **60 giây** (`tsb=60`) | 68.5 MB (0.07 GiB) | **137.0 MB (0.13 GiB)** | 685 MB | 1.37 GiB | **Tối ưu cho Live Edge / Low Latency** |
| **120 giây** (2 phút) | 137.0 MB (0.13 GiB) | **274.1 MB (0.26 GiB)** | 1.37 GiB | 2.74 GiB | An toàn cho DVR ngắn |
| **300 giây** (5 phút) | 342.6 MB (0.32 GiB) | **685.2 MB (0.64 GiB)** | 3.43 GiB | 6.85 GiB | Cần cấp phép tối thiểu 1GB/stream |
| **1800 giây** (30 phút - Mặc định hiện tại) | **2.05 GB (1.91 GiB)** | **4.11 GB (3.83 GiB)** | **20.55 GB (19.1 GiB)** | **41.1 GB (38.3 GiB)** | **CỰC KỲ NGUY HIỂM: Dễ OOM Crash** |
| **Hạn mức 64MB Docker** | **Tràn đĩa sau 56 giây!** | **Tràn đĩa sau 28 giây!** | Crash lập tức | Crash lập tức | **Chắc chắn sập 100%** |

---

### 2.4 Rò Rỉ Tài Nguyên (Resource Leakage) & Zombie/Orphan Files

#### Tài liệu tham chiếu gốc (Primary Sources)
* Linux Programmer's Manual: `shm_overview(7)`
* POSIX Standard IEEE Std 1003.1 (Shared Memory Objects Lifecycle)
* Rust Standard Library Documentation: `std::ops::Drop` guarantees and caveats

#### Bản chất vòng đời của tmpfs vs Anonymous Memory
* Khi một tiến trình cấp phát bộ nhớ RAM thông thường thông qua `malloc` hoặc `mmap(MAP_ANONYMOUS)`, kernel theo dõi trang nhớ đó thông qua cấu trúc bảng trang `mm_struct` của tiến trình. Khi tiến trình chết (dù chết êm đẹp hay bị giết bởi `SIGKILL`), kernel **luôn luôn thu hồi 100% bộ nhớ ẩn danh** trong hàm giải phóng `exit_mmap()`.
* **Tuy nhiên, `tmpfs` và `/dev/shm` KHÔNG PHẢI là bộ nhớ của tiến trình!** `tmpfs` là một cấu trúc hệ thống tệp gắn kết (Virtual Filesystem Mount). Mỗi file tạo ra trong `/dev/shm` tồn tại độc lập với tiến trình đã ghi ra nó.
* **Quy tắc POSIX & Linux VFS:** Các tệp trong `tmpfs` chỉ bị giải phóng khi:
  1. Một tiến trình chủ động gọi `unlink(2)` / `remove(3)` để xóa file.
  2. Toàn bộ filesystem `tmpfs` bị `umount`.
  3. Máy chủ khởi động lại (Reboot).

#### Giới Hạn Nghiêm Trọng Của Cơ Chế RAII (`Drop` Trong Rust)
Trong `drmpack`, ta có hàm hủy dọn dẹp thư mục:
```rust
// src/session/mod.rs:778
impl Drop for PackagingSession {
    fn drop(&mut self) {
        // Xóa output_dir và control_dir
    }
}
```
**Hạn chế chết người của Rust `Drop`:**
* Rust runtime chỉ thực thi `Drop::drop` khi một struct ra khỏi scope bình thường hoặc khi đang thực hiện **stack unwinding do `panic!`**.
* `Drop` **HOÀN TOÀN KHÔNG ĐƯỢC CHẠY** trong các trường hợp:
  * Tiến trình nhận tín hiệu `SIGKILL` (signal 9) từ Kernel OOM Killer, Docker daemon (`docker stop` sau timeout), hoặc lệnh `kill -9`.
  * Lỗi phân đoạn bộ nhớ (Segmentation Fault - `SIGSEGV`), Bus Error (`SIGBUS`).
  * Gọi trực tiếp `std::process::exit()` hoặc `libc::_exit()`.
  * Tiến trình GPAC con bị crash làm treo hoặc crash supervisor.

#### Kịch Bản Khủng Hoảng CrashLoopBackOff
Khi một container chạy `drmpack` bị OOMKilled hoặc panic:
1. Thư mục `/dev/shm/drmpack_{content_id}_{uuid}` chứa **~4GB** video segments bị **bỏ rơi hoàn toàn (orphaned/zombie)** trong RAM.
2. Container runtime khởi động lại Pod mới.
3. Pod mới sinh ra một UUID ngẫu nhiên mới: `/dev/shm/drmpack_{content_id}_{uuid_moi}` và tiếp tục ghi thêm 4GB nữa vào RAM.
4. Sau vài chu kỳ khởi động lại trong vòng vài phút, toàn bộ RAM vật lý của server bị ngập rác, biến máy chủ thành "cục gạch" không thể tiếp nhận thêm bất kỳ kết nối nào!

---

### 2.5 So Sánh Thực Chiến: NVMe SSD + Linux Page Cache vs RAM (/dev/shm)

#### Tài liệu tham chiếu gốc (Primary Sources)
* Linux Kernel Documentation: `Documentation/admin-guide/sysctl/vm.rst` (`dirty_background_ratio`, `dirty_ratio`, `dirty_expire_centisecs`)
* Brendan Gregg: *Systems Performance: Enterprise and the Cloud* (2nd Edition, Chapter 8: File Systems)
* Enterprise NVMe SSD Datasheets (Samsung PM9A3, Solidigm D7-P5520, Intel Optane)

#### 1. Cơ Chế Linux Page Cache (Độ trễ ghi là tương đương)
Khi một ứng dụng gọi `write(2)` vào một file nằm trên ổ đĩa SSD thông thường (ext4, xfs):
* Lệnh ghi **KHÔNG HỀ chờ dữ liệu nạp vào chip flash của SSD**! Lệnh ghi chỉ đơn thuần là một thao tác copy bộ nhớ từ user-space buffer vào **Page Cache (RAM)** của kernel.
* Hệ điều hành trả về thành công cho ứng dụng ngay lập tức trong vòng **vài trăm nano-giây**, hoàn toàn ngang ngửa tốc độ ghi vào `/dev/shm`.
* Việc đẩy dữ liệu từ Page Cache xuống đĩa cứng vật lý do các luồng ngầm của kernel (`wb_workfn` / `kworker`) thực hiện bất đồng bộ.
* Khi HTTP server (Origin / CDN Edge) đọc file segment vừa ghi để phân phối cho người xem, lệnh `read(2)` sẽ chạm ngay vào **Page Cache đang nằm sẵn trong RAM** (Page Cache Hit Rate $\approx 100\%$). Đĩa SSD vật lý hầu như không phải thực hiện thao tác đọc nào!

#### 2. Bài Toán Hao Mòn Đĩa (SSD Flash Endurance)
* Mối lo ngại lớn nhất khi dùng SSD là hiện tượng hao mòn flash (Write Amplification) do chu kỳ ghi liên tục.
* **Đối với Media Segments (`.m4s` dung lượng 200KB – 2MB):** Đây là các khối dữ liệu tuần tự (sequential writes) kích thước lớn, hoàn toàn trùng khớp với kích thước block của chip flash NAND. Hệ số Write Amplification Factor (WAF) xấp xỉ 1.0.
  * Một luồng 10 Mbps sinh ra: $1.25\text{ MB/s} = 108\text{ GB/ngày}$.
  * Một ổ SSD NVMe Enterprise 1.92TB (chuẩn 1 DWPD - Drive Writes Per Day) cho phép ghi tối đa **1,920 GB/ngày liên tục trong 5 năm**.
  * Một luồng live chỉ chiếm **5.6%** độ bền cho phép hàng ngày của ổ đĩa. Ngay cả khi chạy 10 luồng liên tục 24/7, ổ đĩa vẫn hoạt động bền bỉ nhiều năm.
* **Đối với Manifest (`.m3u8`, `.mpd` dung lượng vài KB):** Đây là các file nhỏ bị ghi đè (overwrite) 5 đến 10 lần mỗi giây trong Low Latency mode. Các lệnh ghi ngẫu nhiên nhỏ này gây hiện tượng phân mảnh và ép SSD phải chạy Garbage Collection liên tục, làm tăng độ trễ I/O spike.

#### 3. NVMe SSD Là "Van Xả An Toàn" Triệt Tiêu Nguy Cơ OOM
* Khi hệ thống bị nghẽn bộ nhớ, các trang nhớ bẩn của SSD **có thể flush xuống đĩa cứng và giải phóng khỏi RAM ngay lập tức**!
* Bộ nhớ Page Cache của SSD là bộ nhớ có thể thu hồi (Reclaimable Memory). Ngược lại, bộ nhớ của `tmpfs` (khi không có swap) là bộ nhớ bất khả thu hồi (Unreclaimable Memory).
* **Kết luận:** Dùng SSD làm bộ đệm segment biến nguy cơ **OOM Crash (sập hệ thống hoàn toàn)** thành **I/O Latency Tạm Thời (vẫn duy trì hoạt động)**.

---

## 3. Đề Xuất Kiến Trúc An Toàn Cho `drmpack`

1. **Giảm mặc định `tsb` từ 1800s xuống 60s (hoặc 30s):**
   * Giảm tức thì 97% lượng RAM tiêu thụ (từ 4.1 GB xuống ~137 MB cho luồng Dual scheme).
   * Thêm hàm cấu hình `.with_time_shift_buffer(Duration)`.
2. **Cơ chế Pre-flight Check:**
   * Phát hiện nếu thư mục đầu ra là `tmpfs` có dung lượng $\le 64\text{MB}$ (Docker trap) và cảnh báo/từ chối ngay từ đầu trước khi GPAC crash.
3. **Chủ động Reaper dọn dẹp thư mục Zombie:**
   * Cung cấp hàm `PackagingSession::reap_orphaned_sessions()` để dọn sạch rác RAM khi process/container khởi động lại.
4. **Hỗ trợ Storage linh hoạt:**
   * Cho phép trỏ `output_dir` ra SSD NVMe cho các hệ thống cần buffer dài mà không lo OOM.
