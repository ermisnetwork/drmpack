# Nghiên Cứu và Phân Tích Nguyên Nhân Thất Bại Của GitHub Actions CI & Phương Án Khắc Phục

**Mã tài liệu:** `DRMPACK-RES-0004`  
**Ngày thực hiện:** 10/09/2026  
**Mục tiêu:** Điều tra độc lập các nguồn sơ cấp (primary sources) từ logs GitHub Actions, mã nguồn GPAC, và cấu hình workflow trong `drmpack` để giải thích chính xác tại sao 3 workflow CI vừa chạy trên branch `main` bị thất bại và cung cấp giải pháp khắc phục triệt để.

---

## 1. Tổng Quan Sự Cố (Executive Summary)

Sau khi merge code lên branch `main`, hệ thống GitHub Actions đã kích hoạt 3 workflows song song. Cả 3 workflows đều kết thúc với trạng thái **Failure**:

| Tên Workflow | File Cấu Hình | Run ID | Thời Lượng | Kết Quả | Nguyên Nhân Gốc Rễ |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **CI** | `.github/workflows/ci.yml` | `34453044124` | 43s | ❌ FAILED (21/179 tests) | Runner `ubuntu-latest` chưa cài binary `gpac`. Các unit test trong `src/session/` panic khi spawn GPAC subprocess. |
| **DRM end-to-end** | `.github/workflows/drm-e2e.yml` | `34453044117` | 1m 17s | ❌ FAILED (Build GPAC) | Workflow gọi lệnh `cmake`, trong khi mã nguồn upstream GPAC dùng GNU Autotools (`./configure && make`). |
| **Documentation** | `.github/workflows/docs.yml` | `34453044068` | 29s | ❌ FAILED (Deploy Pages) | `cargo doc` thành công nhưng API GitHub Pages trả về `404 Not Found` do chưa bật GitHub Pages trong Settings repo. |

---

## 2. Sự Cố 1: Workflow `CI` (`ci.yml`)

### 2.1. Nguồn Sơ Cấp (Primary Evidence)
Trích xuất từ log GitHub Actions Run `34453044124` (Job `Unit Tests (cargo test --lib)`):

```text
Unit Tests (cargo test --lib)	Run library unit tests	2026-09-10T08:03:20.5659671Z thread 'session::tests::test_session_is_alive_healthcheck' (4388) panicked at src/session/mod.rs:2191:79:
Unit Tests (cargo test --lib)	Run library unit tests	2026-09-10T08:03:20.5661119Z called `Result::unwrap()` on an `Err` value: PackagingSession(PackagingSessionFailure { cenc: [], cbcs: [RepresentationFailure { scheme: Cbcs, operation: Create, error: Gpac("Failed to spawn GPAC binary 'gpac': No such file or directory (os error 2). Please ensure GPAC is installed and in PATH.") }], output_cleanup: None, control_cleanup: None })
...
Unit Tests (cargo test --lib)	Run library unit tests	2026-09-10T08:03:20.5682849Z test result: FAILED. 158 passed; 21 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.06s
Unit Tests (cargo test --lib)	Run library unit tests	2026-09-10T08:03:20.5703947Z ##[error]Process completed with exit code 101.
```

### 2.2. Danh Sách 21 Unit Tests Bị Fail
Cả 21 tests bị fail đều thuộc module `session`:
1. `session::cluster::tests::test_cluster_spawn_rollback_on_partial_failure`
2. `session::tests::all_clear_renditions_bypasses_key_provider`
3. `session::tests::cleanup_rejects_an_active_session`
4. `session::tests::dual_session_resolves_isolated_manifest_paths`
5. `session::tests::mixed_renditions_selective_encryption_gpac_xml`
6. `session::tests::status_after_creation_failure_returns_structured_representation_failure`
7. `session::tests::test_dual_mode_symmetric_fail_fast`
8. `session::tests::test_ingest_stream_and_run_to_completion`
9. `session::tests::test_multi_rendition_same_tier_packaging_session`
10. `session::tests::test_multiple_renditions_same_tier_no_id_collision`
11. `session::tests::test_push_accepts_into_bytes`
12. `session::tests::test_push_init_only_does_not_require_endlist`
13. `session::tests::test_ramdisk_lifecycle_custom_output_dir_never_dropped`
14. `session::tests::test_ramdisk_lifecycle_drop_without_close_deletes`
15. `session::tests::test_ramdisk_lifecycle_preserve_output`
16. `session::tests::test_run_to_completion`
17. `session::tests::test_session_fail_fast_on_premature_exit`
18. `session::tests::test_session_is_alive_healthcheck`
19. `session::tests::test_watchdog_cancellation_on_close_prevents_race`
20. `session::tests::test_watchdog_inactivity_timeout_triggers_failure`
21. `session::tests::test_watchdog_timeout_concurrent_with_close`

### 2.3. Cơ Chế Lỗi Kỹ Thuật (Root Cause)
- Các công việc định dạng code (`cargo fmt`) và phân tích tĩnh (`cargo clippy`) chạy độc lập trong các container `ubuntu-latest` và đều vượt qua 100% (không có cảnh báo hoặc lỗi format).
- Tuy nhiên, trong kiến trúc của `drmpack`, `PackagingSession::create()` tương tác trực tiếp với tiến trình hệ thống `gpac` thông qua `tokio::process::Command::new("gpac")` (xem [`src/gpac/process.rs:34`](file:///Users/trungdt/Workspace/work/ermis/ermis-stream/drmpack/src/gpac/process.rs#L34)).
- Các unit tests trong `src/session/mod.rs` kiểm tra hành vi khởi tạo tiến trình, watchdog liveness, dọn dẹp thư mục Ramdisk/staging khi tiến trình kết thúc.
- Môi trường GitHub Actions runner tiêu chuẩn (`ubuntu-latest` / `ubuntu-24.04`) **không cài sẵn gói `gpac`**. Vì vậy, hàm `tokio::process::Command::spawn` trả về mã lỗi hệ điều hành `ENOENT` (os error 2 - No such file or directory).

---

## 3. Sự Cố 2: Workflow `DRM end-to-end` (`drm-e2e.yml`)

### 3.1. Nguồn Sơ Cấp (Primary Evidence)
Trích xuất từ log GitHub Actions Run `34453044117` (Step `Build pinned GPAC`):

```text
GPAC 26.07 CENC/CBCS validation	UNKNOWN STEP	2026-09-10T08:03:51.4110692Z ##[group]Run git clone --depth 1 --branch "v${GPAC_VERSION}" https://github.com/gpac/gpac.git /tmp/gpac
...
GPAC 26.07 CENC/CBCS validation	UNKNOWN STEP	2026-09-10T08:03:53.4680395Z warning: refs/tags/v26.07.0 492699f42b0eb159f43b55b523616d82d72ed6c1 is not a commit!
GPAC 26.07 CENC/CBCS validation	UNKNOWN STEP	2026-09-10T08:03:53.4685071Z Note: switching to 'a07cbfff238a331233e11e916f9fb185d5da8604'.
GPAC 26.07 CENC/CBCS validation	UNKNOWN STEP	2026-09-10T08:03:53.6770931Z CMake Error: The source directory "/tmp/gpac" does not appear to contain CMakeLists.txt.
GPAC 26.07 CENC/CBCS validation	UNKNOWN STEP	2026-09-10T08:03:53.6772170Z Specify --help for usage, or press the help button on the CMake GUI.
GPAC 26.07 CENC/CBCS validation	UNKNOWN STEP	2026-09-10T08:03:53.6822287Z ##[error]Process completed with exit code 1.
```

### 3.2. Cơ Chế Lỗi Kỹ Thuật (Root Cause)
1. **Kiểm tra cấu trúc mã nguồn upstream của GPAC:**
   Dự án GPAC (`https://github.com/gpac/gpac`) là một dự án C truyền thống hơn 20 năm tuổi, sử dụng hệ thống cấu hình và biên dịch bằng **GNU Autotools** (`./configure` sinh `config.mak` và `Makefile`), **hoàn toàn không hỗ trợ CMake** và **không có file `CMakeLists.txt`** ở thư mục gốc.
2. **Sai lệch trong script CI:**
   File `.github/workflows/drm-e2e.yml` tại dòng 29–31 đã giả định sai rằng GPAC sử dụng CMake:
   ```yaml
   # ĐOẠN CODE SAI TRONG drm-e2e.yml:
   cmake -S /tmp/gpac -B /tmp/gpac/build -DCMAKE_BUILD_TYPE=Release
   cmake --build /tmp/gpac/build --parallel
   sudo cmake --install /tmp/gpac/build
   ```
   Do không tìm thấy `CMakeLists.txt`, lệnh `cmake` lập tức dừng với exit code 1.

### 3.3. So Sánh Hai Phương Án Khắc Phục
- **Phương án 1: Biên dịch bằng Autotools (`./configure && make`)**:
  - Lệnh chuẩn:
    ```bash
    cd /tmp/gpac
    ./configure --prefix=/usr/local --use-ffmpeg=no
    make -j$(nproc)
    sudo make install
    sudo ldconfig
    ```
  - *Nhược điểm:* Quá trình biên dịch mã nguồn C của GPAC từ git tag tốn từ 2 đến 4 phút mỗi lần chạy CI workflow.
- **Phương án 2 (Khuyến nghị cao): Cài đặt từ Official GPAC APT Repository (`dist.gpac.io`)**:
  - Qua kiểm tra trực tiếp kho gói `https://dist.gpac.io/gpac/linux/ubuntu/dists/noble/main/binary-amd64/Packages.gz`, upstream GPAC cung cấp sẵn gói pre-compiled:
    ```text
    Package: gpac
    Version: 26.07-rev0-ga07cbfff2-HEAD
    ```
    *Ghi chú quan trọng:* Mã commit `ga07cbfff2` trùng khớp tuyệt đối với git tag `v26.07.0` (`a07cbfff238a331233e11e916f9fb185d5da8604`).
  - Cài đặt qua APT chỉ tốn khoảng **5 - 10 giây**, giúp CI chạy cực nhanh và ổn định.

---

## 4. Sự Cố 3: Workflow `Documentation` (`docs.yml`)

### 4.1. Nguồn Sơ Cấp (Primary Evidence)
Trích xuất từ log GitHub Actions Run `34453044068` (Step `Deploy to GitHub Pages`):

```text
Build & Deploy rustdoc to GitHub Pages	Deploy to GitHub Pages	2026-09-10T08:03:07.4851198Z Creating Pages deployment with payload:
...
Build & Deploy rustdoc to GitHub Pages	Deploy to GitHub Pages	2026-09-10T08:03:07.6325747Z ##[error]Creating Pages deployment failed
Build & Deploy rustdoc to GitHub Pages	Deploy to GitHub Pages	2026-09-10T08:03:07.7909600Z ##[error]HttpError: Not Found
...
Build & Deploy rustdoc to GitHub Pages	Deploy to GitHub Pages	2026-09-10T08:03:07.7916310Z ##[error]Error: Failed to create deployment (status: 404) with build version f982e54f55bae6006b75b650b83ab14c3d62aac9. Request ID DC00:3CAB8:55E6F00:114443F8:6AA2643B Ensure GitHub Pages has been enabled: https://github.com/ermisnetwork/drmpack/settings/pages
```

### 4.2. Cơ Chế Lỗi Kỹ Thuật (Root Cause)
- Workflow `docs.yml` đã thực hiện hoàn hảo các bước:
  1. `cargo doc --all-features --no-deps` sinh HTML trong `target/doc/`
  2. Tạo file `index.html` chuyển hướng tự động sang `drmpack/index.html`
  3. Action `actions/upload-pages-artifact@v3` đóng gói artifact thành công.
- Tuy nhiên, khi gọi `actions/deploy-pages@v4` để gọi GitHub API tạo Deployment, GitHub trả về `HTTP 404 Not Found` vì kho lưu trữ `ermisnetwork/drmpack` **chưa bật tính năng GitHub Pages**.
- Theo đặc tả của GitHub REST API, endpoint `POST /repos/{owner}/{repo}/pages/deployments` sẽ trả về `404` trừ khi người quản trị repo đã vào Settings và cấu hình Build source là `GitHub Actions`.

---

## 5. Hướng Dẫn Sửa Lỗi Chi Tiết (Actionable Fixes)

### 5.1. Sửa `.github/workflows/ci.yml`
Thêm bước cài đặt GPAC vào job `test`:

```yaml
  test:
    name: Unit Tests (cargo test --lib)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Install GPAC
        run: |
          sudo apt-get update
          sudo apt-get install -y gpac
      - name: Run library unit tests
        run: cargo test --lib --all-features
```

### 5.2. Sửa `.github/workflows/drm-e2e.yml`
Thay thế đoạn `cmake` bằng việc cài đặt GPAC 26.07 chính thức từ kho APT của GPAC:

```yaml
      - name: Install GPAC 26.07 and FFmpeg
        run: |
          sudo apt-get update && sudo apt-get install -y ca-certificates curl
          sudo install -m 0755 -d /etc/apt/keyrings
          sudo curl -fsSL https://dist.gpac.io/gpac/linux/gpg.asc -o /etc/apt/keyrings/gpac.asc
          sudo chmod a+r /etc/apt/keyrings/gpac.asc

          sudo tee /etc/apt/sources.list.d/gpac.sources <<EOF
          Types: deb
          URIs: https://dist.gpac.io/gpac/linux/ubuntu
          Suites: noble
          Components: main
          Signed-By: /etc/apt/keyrings/gpac.asc
          EOF

          sudo apt-get update
          sudo apt-get install -y gpac ffmpeg
```

### 5.3. Kích Hoạt GitHub Pages Trên Web GitHub
Thao tác thủ công bằng tài khoản có quyền admin repo:
1. Mở đường dẫn: `https://github.com/ermisnetwork/drmpack/settings/pages`
2. Tại mục **Build and deployment**:
   - **Source**: Chọn **GitHub Actions** (thay vì "Deploy from a branch").
3. Lưu cài đặt. Sau khi cấu hình, bất kỳ lần push nào lên `main` sẽ tự động publish documentation lên `https://ermisnetwork.github.io/drmpack/`.
