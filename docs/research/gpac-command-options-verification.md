# Kiểm chứng GPAC Command-Line Options Optimization Audit

Ngày kiểm chứng: 2026-09-10.

## Kết luận

**Tài liệu `gpac-command-options-optimization.md` đúng một phần, không đủ căn cứ để áp dụng nguyên bộ patch.** Có vấn đề signaling DASH-LL (`asto`) và rủi ro publication ordering (`seg_sync`) thực sự. Nhưng tài liệu biến nhiều giả thuyết hiệu năng thành kết luận chắc chắn, nhầm source-duration checking với segment synchronization, và bỏ qua điều kiện Representation ID khi kết luận CMAF/multiplexing.

**Phát hiện bổ sung quan trọng từ kiểm chứng pipe/session:** upstream đổi `stdin:timeout=0` thành 10000ms, nên patch đề xuất không vô hiệu hóa stdin timeout; buffer lớn không đồng nghĩa syscall lớn vì code giới hạn `read_block_size` ở 8192. `-no-block=all` bỏ điều tiết queue, không ngăn queue tăng khi downstream chậm. Chi tiết, điều kiện và nguồn ghim commit nằm trong [báo cáo pipe/session](gpac-pipe-session-options-verification.md).

Đây là kiểm chứng tài liệu và đọc mã, không phải benchmark hay reproduction đầy đủ pipeline. Không sửa Rust/config runtime.

## Phạm vi và nguồn

- Binary tại máy: `gpac -version` báo **26.07-revrelease**; đối chiếu `gpac -hx dasher`.
- Mã upstream được cố định tại commit **181a657b4200de4b10949736c7c7748338044fe2**, commit ngày 2026-09-09. Không mặc định binary local được build từ commit này.
- Công cụ web không trả nội dung trong phiên; nguồn Internet bên dưới được tải trực tiếp bằng HTTPS từ GPAC GitHub và DASH-IF.
- [S1: GPAC dasher.c](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filters/dasher.c): definitions, embedded documentation, implementation.
- [S2: GPAC mux_isom.c](https://github.com/gpac/gpac/blob/181a657b4200de4b10949736c7c7748338044fe2/src/filters/mux_isom.c): CMAF enforcement.
- [S3: DASH-IF dash.js live streaming](https://dashif.org/dash.js/pages/usage/live-streaming.html): player delay precedence.
- Kiểm chứng riêng pipe/session: [gpac-pipe-session-options-verification.md](gpac-pipe-session-options-verification.md).
- Mã ứng dụng: `src/gpac/process.rs`, `src/session/harvester.rs`, `src/session/mod.rs`, `src/session/cluster.rs`; quyết định container: `docs/adr/0010-cmaf-only-over-legacy-hls-ts.md`.

## 1. `asto=0`: đúng về signaling, quá mạnh về chunk dispatch

`GpacProcessConfig::build_args()` thực sự dùng `availability_time_offset.unwrap_or(0.0)` trong LowLatency. Tài liệu GPAC hướng dẫn đặt ATO bằng hoặc hơi lớn hơn `segdur - cdur`; ví dụ chính thức là segment 2s, fragment 0.2s, ATO 1.8s. Trong source, positive ATO được gán vào segment template. **Đề xuất default 1.8s cho cấu hình 2s/0.2s có cơ sở.** [S1, documentation Low Latency và các nhánh `ctx->asto > 0`]

Nhưng ATO là thời điểm client có thể request sớm, không phải công tắc duy nhất tạo/flush fragment. Fragmentation do `cdur`, HLS LL do `llhls`; không nên viết rằng ATO=0 tắt toàn bộ early chunk dispatch. Tăng ATO cũng không tự làm origin phục vụ được file đang ghi. [S1, Low Latency]

**Đặc biệt trong repo này:** harvester chỉ lấy URI media từ dòng không bắt đầu bằng `#` và URI init từ `EXT-X-MAP`; không xử lý `EXT-X-PART` như các artifact riêng. Nó đợi tên media xuất hiện trong HLS rồi đọc cả file. Vì vậy nếu delivery chỉ đi qua artifact harvester, không thể kết luận sửa ATO sẽ tạo LL end-to-end. Đây là suy luận từ `extract_hls_segment_refs()` và `harvest_target()`, cần kiểm tra origin/CDN thực tế.

Ưu tiên: kiểm chứng đường chunk tới client trước, rồi đổi default ATO trong LowLatency. Không advertise availability sớm hơn khả năng phục vụ thực tế.

## 2. `spd=4000`: có thể làm tăng latency, không ép mọi player

Code đặt SPD bằng `2 * segment_duration * 1000`; 4000ms chỉ đúng khi segment=2s. GPAC định nghĩa đây là **suggested** presentation delay, không phải bắt buộc mọi player buffer 4s. dash.js cho application `liveDelay`/`liveDelayFragmentCount` ưu tiên cao hơn suggested delay. [S1, `OFFS(spd)`; S3]

Tách cấu hình delay theo mode là hợp lý, nhưng công thức `max(3 * cdur, 0.6)` là policy đề xuất, không phải công thức DASH-LL bắt buộc trong những nguồn đã kiểm tra. GPAC có `ll_part_hb` mặc định âm để chọn ba lần max part duration cho **HLS PART-HOLD-BACK**; không được đồng nhất cơ chế HLS này với DASH SPD. [S1, `OFFS(ll_part_hb)`]

Chưa nên hardcode 600ms khi chưa đo encoder/GOP, đường publish, RTT và hành vi player. Đây là khuyến nghị kiểm thử, không phải kết quả benchmark.

## 3. Thiếu `cmaf=cmfc` không tự gây `muxed_base`

GPAC mặc định `cmaf=no`, nghĩa là không enforce CMAF. Bật `cmaf` bỏ qua nhánh multiplexing đang xét, nhưng khi tắt CMAF, nhánh đó còn kiểm tra **hai PID có cùng Representation ID**. `strcmp(a_ds->rep_id, ds->rep_id)` khác nhau thì bỏ qua; còn các điều kiện mux/template/source khác. [S1, đoạn khoảng dòng 7788–7812]

Repo đã đặt ID phân biệt video/audio/subtitle (`video_...`, `audio_...`, `sub_...`). Vì vậy không có cơ sở để kết luận chỉ thiếu `cmaf` ở Standard là GPAC sẽ ghép audio/video và gây stall. Có thể điều tra collision ID giữa những track cùng loại, nhưng đó là vấn đề khác cần input cụ thể.

ADR-0010 ủng hộ CMAF; bật enforcement ở cả hai mode có thể hợp lý về tính tuân thủ. Tuy nhiên đây không chỉ là thay brand: muxer có kiểm tra nội dung và chỉnh timing, ví dụ xử lý edit/delay và cấm trộn protected/unprotected samples trong cùng fragment. Cần regression test DRM, timestamp và subtitle trước. [S2, các nhánh `ctx->cmaf`, khoảng dòng 4362–4380 và 4799–4804]

**Phán quyết:** đề xuất compliance có cơ sở; diễn giải root cause stall chưa được chứng minh. Không dùng cụm “constrained media format clump” làm định nghĩa `cmfc`; nguồn GPAC chỉ mô tả enforcement theo CMAF `cmfc` guidelines. [S1]

## 4. `seg_sync=no`: rủi ro thật, nhưng harvester đã có guard

GPAC ghi rõ khi không sync, manifest có thể announce segment/part trước khi ghi/gửi xong. `auto` đợi last packet nếu có HLS; repo dùng `dual`, nên đây là cấu hình đáng ưu tiên thử cho invariant HLS là readiness signal. Không nên gọi đây là guarantee `fsync`/durability tới ổ đĩa: source nói về last packet/byte được đẩy qua output. [S1, embedded documentation và `OFFS(seg_sync)`]

Audit thiếu chi tiết quan trọng: `harvest_target()` đã yêu cầu HLS reference và kiểm tra `is_complete_isobmff_media_segment()`. Nó không đọc file một cách hoàn toàn mù theo manifest event.

Tuy nhiên guard vẫn **không chứng minh writer đã kết thúc segment**: một file đang lớn dần chứa một cặp `moof` + `mdat` hoàn chỉnh có thể pass tại ranh giới fragment, dù sau đó GPAC sẽ nối thêm fragment. Đây là suy luận trực tiếp từ predicate `has_moof && has_mdat && offset == data.len()`, không phải reproduction lỗi đã quan sát.

**Phán quyết:** nên thử `seg_sync=auto` với pipeline disk/harvester hiện tại. Khuyến nghị trong audit rằng chỉ kiểm tra box termination là đủ để giữ `seg_sync=no` chưa chặt chẽ.

## 5. `check_dur=false`: sai khi giải thích là chữa fractional-frame stalls

GPAC định nghĩa `check_dur` là kiểm tra duration của sources trong period, cố giữ chúng gần bằng nhau; có thể bị enforce khi dùng period start times. Không phải check từng segment để AAC 21.33ms bằng video 33.33ms. [S1, `OFFS(check_dur)`; cùng mô tả trong local help]

Trong code, khi có representation đã done và representation chưa done, nhánh `ctx->check_dur` đặt `force_rep_end` cho stream còn lại. Điều này cho thấy đây là xử lý kết thúc/duration period, không chứng minh nguyên nhân steady-state fractional-frame stall. [S1, khoảng dòng 8930–8939]

Có thể giữ setting vì yêu cầu duration của ứng dụng, nhưng phải bỏ kết luận “optimal vì chữa drift AAC/video” nếu không có reproduction riêng.

## 6. `sbound=out`: đúng hướng boundary, không phải zero latency

GPAC định nghĩa `out` là cắt sau khi theoretical bound đạt/vượt; `closest` chọn SAP gần nhất, `in` chọn phía còn lại. Điều đó không guarantee không phải đợi SAP/GOP, input hay muxer. [S1, `OFFS(sbound)` và segmentation documentation]

Nên viết: “chọn boundary ở/sau target, tránh nhu cầu chọn nearest-boundary như `closest`”; không khẳng định loại bỏ đúng một GOP latency hay zero buffering trong mọi input khi chưa đo.

## 7. Các nhận xét khác

- `strict_sap=off` đúng là default và bỏ qua SAP types của PID không phải video, đồng thời ép signaling `startsWithSAP=1`; mô tả “enforces SAP alignment on all tracks” không chính xác. Explicit default là lựa chọn pin behavior, không phải tối ưu đã đo. [S1, `OFFS(strict_sap)`]
- `utcs=inband` đúng là UTC timing trực tiếp cùng giá trị publishTime; đây không phải bằng chứng clock synchronization tuyệt đối chính xác hay miễn nhiễm sai clock. [S1, `OFFS(utcs)`]
- `llhls=br` đúng là byte-range part tham chiếu full segment. Tránh file-descriptor exhaustion là suy đoán phụ thuộc workload, không có bảo đảm trong docs. [S1, `OFFS(llhls)`]
- 200ms là ví dụ GPAC chính thức; từ ví dụ đó không suy ra 200ms luôn tối ưu hoặc luôn tuân thủ mọi điều kiện triển khai Apple/DASH-IF. [S1, Low Latency]
- `dmode=dynauto` đúng là dynamic rồi static khi kết thúc; không biến mọi nguồn lỗi, disconnect hay termination thành graceful finalize. [S1, `OFFS(dmode)`]
- Default application hiện tại là Standard/CBCS trong `PackagingSessionConfig::new()`. Ví dụ sizing 2 subprocess/session phải ghi là dual-scheme deployment, không phải mặc định mọi session.
- Những nhãn “optimal”, “eliminates thrashing”, “critical cecrypt starvation” cần benchmark/trace của deployment cụ thể; đọc definition option không chứng minh chúng.

## Thứ tự hành động đề xuất

1. Sửa nội dung audit trước: phân biệt fact/spec, suy luận từ source và giả thuyết performance.
2. Reproduce ordering segment/manifest với writer chậm; A/B `seg_sync=no` và `auto`, đặc biệt file có nhiều fragment.
3. Xác minh delivery thực sự phát chunk trước full-segment completion. Sau đó mới A/B ATO `0` và `segdur-cdur` cùng player delay.
4. Test CMAF enforcement cả Standard/LowLatency vì mục tiêu container compliance, không gọi đây là fix muxed-track stall nếu chưa thấy trùng Representation ID.
5. Benchmark thread pool/block size theo concurrency, CPU, RSS và p95/p99 latency; không hứa ngưỡng 4 threads hoặc 64KiB là tối ưu.

Không chạy streaming benchmark trong đợt kiểm chứng này. Những thay đổi runtime ở trên là đề xuất kiểm thử, chưa được áp dụng.
