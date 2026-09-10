# Xác minh GPAC pipe input và session options

Ngày kiểm tra: **2026-09-10**. Phạm vi: các claim về `stdin`/`pin`, `timeout`, `block_size`, `-no-block`, `-threads`, `-noprog` trong `gpac-command-options-optimization.md`; không audit dasher, encryption hoặc thay đổi cấu hình chạy.

## 1. Nguồn và giới hạn bằng chứng

- **Local:** `/opt/homebrew/bin/gpac` tự báo `26.07-revrelease`. Đã đọc `gpac -h pin`, `gpac -hx pin`, và help `no-block`, `threads`, `noprog`. Help có cảnh báo RTI trong sandbox; không dùng cảnh báo này để suy ra hành vi production. Chưa xác định commit build local, chưa chạy benchmark hoặc thử pause/reconnect.
- **Upstream:** ghim toàn bộ source vào **`181a657b4200de4b10949736c7c7748338044fe2`**, commit ngày **2026-09-09** theo [GitHub API chính thức](https://api.github.com/repos/gpac/gpac/commits/181a657b4200de4b10949736c7c7748338044fe2). Không coi implementation này là bằng chứng trực tiếp cho binary local.
- **Web docs:** [pin](https://wiki.gpac.io/Filters/pin/), [core options](https://wiki.gpac.io/Filters/core_options/), [core logs](https://wiki.gpac.io/Filters/core_logs/), truy cập ngày kiểm tra; đây là trang cập nhật động, không phải tài liệu ghim riêng cho bản local.
- `web.run` trả rỗng; nội dung web/source được tải bằng `curl` với quyền mạng đã được duyệt. Đã đọc research skill và kiểm tra hướng dẫn repository/ancestor cùng `docs`; không thấy file `AGENTS.md` bổ sung hoặc `.codegraph/`. Phạm vi thay đổi của nghiên cứu pipe/session chỉ là báo cáo này.

## 2. Timeout: phải tách stdin khỏi named pipe

**Claim “stdin mặc định 10 giây” đúng ở mức docs/help; đề xuất `stdin:timeout=0` để vô hiệu hóa timeout không đúng với upstream đã ghim.**

- Help local và [web pin](https://wiki.gpac.io/Filters/pin/#stdin-pipe) đều nói stdin mặc định 10 giây; bảng option lại khai báo `timeout=0` nghĩa là không timeout. Source giải thích ngoại lệ: khi `src` là `stdin` hoặc `-`, initializer chạy `if (!ctx->timeout) ctx->timeout = 10000`. Vì vậy cả giá trị mặc định lẫn **explicit `timeout=0`** đều trở thành 10000 ms. [Source: `in_pipe.c`, L109–115](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filters/in_pipe.c#L109).
- **Named pipe** không đi qua nhánh ghi đè này: `timeout=0` giữ nghĩa không timeout theo option. Trên POSIX, named pipe mặc định mở `O_NONBLOCK`; `blk=true` đổi sang mở blocking. Đây không phải cách nhánh stdin được mở. [Source: L219–229](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filters/in_pipe.c#L219), [khai báo option L666–675](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filters/in_pipe.c#L666).
- **Không thể khẳng định mọi pause >10 giây đều tự ngắt đúng hạn.** Kiểm tra thời gian nằm trong `pipein_process`, trước bước đọc, không phải timer ngắt I/O. Nhánh stdin dùng `fread`; nếu descriptor stdin đang blocking và lời gọi chưa trả về, kiểm tra này không chạy lại để ngắt nó. Ngoài ra `pck_out` có thể làm hàm return trước kiểm tra timeout. Đây là suy luận có điều kiện từ control flow, chưa phải kết quả thử binary local. [Source: L333–407](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filters/in_pipe.c#L333).
- Khi nhánh timeout thật sự chạy, source đặt EOS nếu đã có PID, hoặc setup failure nếu chưa có PID. Điều đó **không tự chứng minh** manifest chuyển static thành công: cần kiểm tra downstream/dasher riêng. Đồng thời **không timeout không có nghĩa bỏ qua writer close/EOF**: named pipe có xử lý riêng `ka`/`bpcnt`, mặc định `ka=false`; stdin cũng kiểm tra `feof`. [Source: timeout/EOF L368–407](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filters/in_pipe.c#L368), [POSIX read/EOF L493–541](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filters/in_pipe.c#L493).

**Kết luận áp dụng:** bỏ claim “`timeout=0` sửa stdin timeout”. Nếu cần tăng khoảng chờ, một giá trị dương lớn hơn thay đổi ngưỡng upstream, nhưng không biến nó thành watchdog đáng tin cậy. Nếu cần named-pipe reconnect, đánh giá riêng `ka`, EOF, framing và lifecycle; không mặc định các lần nối lại là một stream MP4 liên tục hợp lệ.

## 3. `block_size`: dung lượng buffer không phải kích thước syscall

**Default 5000 byte được xác nhận**, nhưng “5000 là bottleneck, đặt 65536 sẽ đọc 64 KiB mỗi syscall” chưa được chứng minh.

- [Web pin](https://wiki.gpac.io/Filters/pin/#block_size) và help local mô tả đây là buffer đọc. Source cấp `block_size + 1` byte nhưng đặt **`read_block_size = MIN(8192, block_size)`**. [Source: L132](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filters/in_pipe.c#L132), [L247](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filters/in_pipe.c#L247).
- Nhánh stdin gọi `fread`, named pipe POSIX gọi `read`, với count `read_block_size-total_read`. Có vòng refill, nhưng tăng allocation lên 65536 **không tạo request đọc 65536 byte** trong các nhánh đó. Không đồng nhất số lần `fread` với số syscall của libc. [Source: L402](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filters/in_pipe.c#L402), [L493–554](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filters/in_pipe.c#L493).
- Tăng 5000 → 65536 tăng allocation trực tiếp **60.536 byte mỗi pin buffer**; đây không phải mức tăng RSS tổng. `pin` gửi shared packet và chờ destructor giải phóng packet trước lần xử lý tiếp, nên không được tính rằng riêng pin luôn tích lũy một buffer lớn cho mỗi packet chờ. [Source: destructor/guard L324–349](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filters/in_pipe.c#L324), [shared packet L639–658](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filters/in_pipe.c#L639).

**Kết luận áp dụng:** 65536 chỉ là ứng viên benchmark, không phải tối ưu mặc định đã xác minh. Đo latency, throughput, read calls và RSS trên đúng binary/workload; không suy từ bitrate rằng 5000 chắc chắn gây nghẽn.

## 4. Session: queue memory, threads và progress

| Claim trong audit | Kết quả xác minh |
| --- | --- |
| `-no-block=all` ngăn backpressure tích lũy samples trong RAM | **Sai hướng tác động.** Nó bỏ điều tiết blocking của filter/PID, không xóa queue, không đặt memory cap và không bật non-blocking OS pipe. |
| `-threads=-1` tạo số worker bằng số core | **Cần sửa:** upstream tính số **extra workers = số core GPAC phát hiện − 1**, cộng main session thread; có fallback và scheduler exception. |
| `-threads=-1` bắt buộc để tránh starvation ở `cecrypt` | **Chưa đủ bằng chứng.** Docs không cam kết điều này; cần trace/reproduction trên graph thực tế. |
| `-noprog` tắt progress trên stderr | **Đúng cho standard progress callback**; không đồng nghĩa tắt logs hoặc giải quyết toàn bộ stderr backpressure. |

### Queue và bộ nhớ

[Core docs](https://wiki.gpac.io/Filters/core_options/#no-block) đặt default `no` (bật blocking), `fanout` và `all` là các mức bỏ blocking. [Session source L378–386](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filter_core/filter_session.c#L378) ánh xạ `all` sang `GF_FS_NOBLOCK`; [PID source L7591–7622](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filter_core/filter_pid.c#L7591) return false **trước** kiểm tra ngưỡng packet count/buffer duration.

**Suy luận:** nếu producer tiếp tục tạo packet nhanh hơn consumer xử lý, bỏ cơ chế điều tiết này có thể làm queue downstream tăng và giữ packet memory lâu hơn. Không kết luận RAM chắc chắn tăng vô hạn: còn phụ thuộc shared-packet lifetime, backpressure riêng của filter, copy và downstream. Guard `pck_out` của pin vẫn tồn tại; nó không chứng minh mọi queue sau demux/encrypt đều bị chặn tương tự. Ngược lại, blocking mặc định cũng không phải hard byte cap cho RSS toàn tiến trình. Nguồn control flow: PID và pin đã dẫn ở trên.

Phân biệt queue đang giữ packet với **reservoir tái sử dụng allocation**: [docs `no-reservoir`](https://wiki.gpac.io/Filters/core_options/#no-reservoir) nói tắt recycling giảm memory nhưng tăng tải allocator; [source option L1647](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/utils/os_config_init.c#L1647) xác nhận trade-off. Không gọi giữ reservoir là “optimal” cho mọi mật độ session, hoặc coi nó là giới hạn queue.

### Threads

[Docs](https://wiki.gpac.io/Filters/core_options/#threads) định nghĩa `N` là **extra threads**. [Source L239–254](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filter_core/filter_session.c#L239) dùng `rti.nb_cores - 1`, fallback về 0 nếu không lấy được số core; `sched=direct` cũng đặt 0. [L343–355](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filter_core/filter_session.c#L343) tạo extra worker.

Ví dụ có điều kiện: 20 session độc lập, GPAC phát hiện 32 core, scheduler thông thường → **620 extra workers + 20 main session threads = 640 session threads**, không phải 640 extra workers; chưa tính threads của codec/library. `threads=4` nghĩa là 4 extra workers, không phải tổng 4. Cho phép cấu hình pool là hướng hợp lý, nhưng `min(cores,4)` không phải default tối ưu được GPAC bảo đảm, cũng không bảo đảm loại bỏ contention.

### Progress

[Docs `noprog`](https://wiki.gpac.io/Filters/core_logs/#noprog) và help local chỉ cam kết tắt progress messages. [Parser L1831–1834](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/utils/os_config_init.c#L1831) gọi `gpac_disable_progress`; [implementation L876–880](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/utils/os_divers.c#L876) thay callback bằng hàm rỗng. [Standard callback L132–168](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/utils/error.c#L132) dùng `fprintf(stderr, ...)` và flush.

Do đó có thể dùng `-noprog` để giảm progress output không cần thiết, nhưng claim “ngăn stderr ring buffer đầy trong live session” cần bằng chứng pipeline thật sự phát progress và reader không theo kịp. Không dùng tùy chọn này thay cho việc drain stderr; chưa audit cơ chế ring buffer của drmpack trong báo cáo này.

## 5. Kết luận có giới hạn

1. **Không áp dụng nguyên xi** đề xuất `stdin:block_size=65536:timeout=0` như một fix đã xác minh.
2. **Sửa mô tả `no-block=all`**: bỏ điều tiết queue có thể tăng memory pressure, không ngăn queue tích lũy.
3. **Sửa cách đếm threads** và hạ claim chống starvation xuống giả thuyết cần thử nghiệm.
4. **Giữ `noprog` là tùy chọn giảm output**, không phải giải pháp memory/backpressure tổng thể.
5. Xác minh implementation trên đúng source của binary local trước khi triển khai; các phát hiện control flow ở đây được ghim upstream, không phải kết quả runtime cho `26.07-revrelease`.
