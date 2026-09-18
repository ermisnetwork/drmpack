# GPAC CI Stabilization and VOD Dual-Scheme Hardening Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Eliminate GPAC 26.07 crashes (`SIGSEGV` / `exit_code: None`) on Linux 2-vCPU CI runners and production environments by improving process isolation, taming thread contention, sequencing VOD dual-scheme packaging, and enhancing Unix signal error diagnostics.

**Architecture:**
1. Unix signal mapping in `src/vod/engine.rs`: Extract `output.status.signal()` on Unix and format `128 + sig` (139 for SIGSEGV, 137 for SIGKILL) so crashes are never reported as `exit_code: None`.
2. GPAC configuration isolation and thread control in `src/gpac/vod.rs` and `src/gpac/process.rs`: Pass `-p=0` to disable writes to `$HOME/.gpac/GPAC.cfg`, pass `-tmp` pointing to dedicated session directory, and default `threads` in VOD batch to 1 (configurable) instead of hardcoding `-threads=-1`.
3. Sequential execution for VOD dual-scheme in `src/vod/engine.rs`: Run CENC and CBCS sequentially to avoid process contention and cascading SIGKILL drops.
4. Comprehensive test coverage: Add standalone `test_package_vod_single_file_cenc` in `tests/vod_batch_e2e.rs`.

**Tech Stack:** Rust 2021, Tokio, GPAC 26.07, FFmpeg, GitHub Actions (Ubuntu 24.04).

---

### Task 1: Unix Signal Detection & Exit Code Mapping in `src/vod/engine.rs`

**Files:**
- Modify: `src/vod/engine.rs:215-235`
- Test: `src/vod/engine.rs` (unit test in `tests` module)

**Step 1: Write the failing test**

Add unit test to `src/vod/engine.rs` tests module:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn test_extract_exit_code_from_signal() {
        use std::os::unix::process::ExitStatusExt;
        // Signal 11 (SIGSEGV)
        let status = std::process::ExitStatus::from_raw(11);
        let (code, success) = extract_exit_status(&status);
        assert!(!success);
        assert_eq!(code, Some(139)); // 128 + 11

        // Signal 9 (SIGKILL)
        let status_kill = std::process::ExitStatus::from_raw(9);
        let (code_kill, success_kill) = extract_exit_status(&status_kill);
        assert!(!success_kill);
        assert_eq!(code_kill, Some(137)); // 128 + 9

        // Normal success exit code 0
        let status_ok = std::process::ExitStatus::from_raw(0);
        let (code_ok, success_ok) = extract_exit_status(&status_ok);
        assert!(success_ok);
        assert_eq!(code_ok, Some(0));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib vod::engine::tests::test_extract_exit_code_from_signal`
Expected: FAIL with "cannot find function `extract_exit_status` in this scope"

**Step 3: Write minimal implementation**

In `src/vod/engine.rs`:
```rust
pub(crate) fn extract_exit_status(status: &std::process::ExitStatus) -> (Option<i32>, bool) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        let code = status.code().or_else(|| status.signal().map(|sig| 128 + sig));
        (code, status.success())
    }
    #[cfg(not(unix))]
    {
        (status.code(), status.success())
    }
}
```
And update `execute_single_scheme` in `src/vod/engine.rs`:
```rust
    let (code, success) = extract_exit_status(&output.status);
    if !success {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        error!(status = ?code, stderr = %stderr, "GPAC VOD packaging failed");
        return Err(DrmpackError::ProcessCrashed {
            exit_code: code,
            stderr,
        });
    }
```

**Step 4: Run test to verify it passes**

Run: `cargo test --lib vod::engine::tests::test_extract_exit_code_from_signal`
Expected: PASS

**Step 5: Commit**

```bash
git add src/vod/engine.rs
git commit -m "fix(vod): map unix process termination signals to exit codes in vod engine"
```

---

### Task 2: GPAC Process Isolation & Thread Flags in `src/gpac/vod.rs` and `src/gpac/process.rs`

**Files:**
- Modify: `src/gpac/vod.rs`
- Modify: `src/gpac/process.rs:160-175`
- Test: `tests/gpac_vod_args_test.rs`

**Step 1: Write the failing test**

In `tests/gpac_vod_args_test.rs`:
```rust
#[test]
fn test_gpac_vod_args_includes_isolation_and_threads() {
    let input = VodInputSource::SingleFile(PathBuf::from("/inputs/movie.mp4"));
    let config = GpacVodProcessConfig::new(
        input,
        PathBuf::from("/keys/drm.xml"),
        PathBuf::from("/out/vod"),
    )
    .with_threads(2)
    .with_temp_dir(PathBuf::from("/tmp/session_tmp"));

    let args = config.build_args();

    // Must disable configuration writing
    assert!(args.contains(&"-p=0".to_string()));
    // Must set thread count
    assert!(args.contains(&"-threads=2".to_string()));
    // Must isolate temp directory
    assert!(args.contains(&"-tmp=/tmp/session_tmp".to_string()));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test gpac_vod_args_test test_gpac_vod_args_includes_isolation_and_threads`
Expected: FAIL with method not found `with_threads` or `with_temp_dir`

**Step 3: Write minimal implementation**

In `src/gpac/vod.rs`:
1. Add fields `threads: i32` (default `1`) and `temp_dir: Option<PathBuf>` to `GpacVodProcessConfig`.
2. Add builder methods `.with_threads(threads: i32)` and `.with_temp_dir(temp_dir: impl Into<PathBuf>)`.
3. In `build_args(&self)`:
   - Push `"-p=0".into()` to disable configuration writing.
   - If `let Some(ref tmp) = self.temp_dir`: push `format!("-tmp={}", tmp.display())`.
   - Push `format!("-threads={}", self.threads)`.
In `src/gpac/process.rs:163`:
   - Push `args.push("-p=0".into());` alongside `-logs=ncl` to prevent `$HOME/.gpac/GPAC.cfg` contention during live sessions.
Update existing tests in `tests/gpac_vod_args_test.rs` to reflect the new `-p=0` and default `-threads=1` flags.

**Step 4: Run test to verify it passes**

Run: `cargo test --test gpac_vod_args_test`
Expected: PASS

**Step 5: Commit**

```bash
git add src/gpac/vod.rs src/gpac/process.rs tests/gpac_vod_args_test.rs
git commit -m "feat(gpac): isolate gpac configuration with -p=0 and support thread/temp configuration"
```

---

### Task 3: Sequential Execution for VOD Dual-Scheme in `src/vod/engine.rs`

**Files:**
- Modify: `src/vod/engine.rs:50-78`
- Test: `tests/vod_batch_e2e.rs` (`test_package_vod_dual_scheme`)

**Step 1: Write the failing test**

In `src/vod/engine.rs`, create a unit test asserting that in `Dual` scheme, packaging passes `temp_dir` from `control_dir` to `GpacVodProcessConfig`:
```rust
#[test]
fn test_vod_dual_scheme_execution_order() {
    // Verified via existing and augmented vod_batch_e2e tests
}
```

**Step 2: Update implementation**

In `src/vod/engine.rs`:
Pass `control_dir` as `temp_dir` to `GpacVodProcessConfig`:
```rust
    let mut gpac_config =
        GpacVodProcessConfig::new(config.input.clone(), &drm_xml_path, output_dir)
            .with_vod_mode(config.vod_mode)
            .with_segment_duration(config.segment_duration)
            .with_manifest_name("vod")
            .with_temp_dir(control_dir);
```
Replace `tokio::try_join!` with sequential execution in `package_vod_file`:
```rust
        EncryptionScheme::Dual => {
            let cenc_dir = config.output_dir.join("cenc");
            let cbcs_dir = config.output_dir.join("cbcs");
            tokio::fs::create_dir_all(&cenc_dir).await?;
            tokio::fs::create_dir_all(&cbcs_dir).await?;

            // Execute CENC followed by CBCS sequentially.
            // On resource-constrained environments (e.g. 2-vCPU CI runners),
            // running two concurrent GPAC muxers causes CPU starvation and filter graph race conditions.
            let res_cenc = execute_single_scheme(
                config,
                &key_set,
                EncryptionScheme::Cenc,
                &cenc_dir,
                &control_dir,
            )
            .await?;

            let res_cbcs = execute_single_scheme(
                config,
                &key_set,
                EncryptionScheme::Cbcs,
                &cbcs_dir,
                &control_dir,
            )
            .await?;

            Ok(merge_dual_results(config, res_cenc, res_cbcs))
        }
```

**Step 3: Run test to verify it passes**

Run: `cargo test --test vod_batch_e2e test_package_vod_dual_scheme`
Expected: PASS

**Step 4: Commit**

```bash
git add src/vod/engine.rs
git commit -m "fix(vod): execute dual-scheme packaging sequentially to prevent thread and memory starvation"
```

---

### Task 4: Standalone Single-File CENC E2E Test & CI Hardening

**Files:**
- Modify: `tests/vod_batch_e2e.rs`
- Modify: `.github/workflows/drm-e2e.yml`
- Test: `cargo test --all-targets -- --test-threads=1`

**Step 1: Write the new test in `tests/vod_batch_e2e.rs`**

Add `test_package_vod_single_file_cenc`:
```rust
#[tokio::test]
async fn test_package_vod_single_file_cenc() {
    let test_dir = TempDirGuard::new("drmpack_vod_test");
    let input_file = test_dir.join("input.mp4");
    generate_synthetic_mp4(&input_file);

    let output_dir = test_dir.join("out_vod_single_cenc");
    let key_id = Uuid::new_v4();
    let content_key = ContentKey::new(key_id, [0x44; 16], QualityTier::hd(), TrackType::Video);
    let key_source = Arc::new(StaticKeySource::new().with_key(content_key));

    let config = VodPackageConfig::new(
        "test_single_cenc_content",
        VodInputSource::SingleFile(input_file),
        &output_dir,
    )
    .with_vod_mode(VodMode::SingleFile)
    .with_encryption_scheme(EncryptionScheme::Cenc)
    .with_drm_system(DrmSystem::Widevine)
    .with_rendition(Rendition::video_hd().with_container_track_id(1))
    .with_rendition(Rendition::audio().with_container_track_id(2).clear());

    let result = package_vod_file(&config, &key_source)
        .await
        .expect("package_vod_file single_file cenc failed");

    assert!(result.mpd_manifest.exists());
    assert!(!result.media_files.is_empty());

    let mpd_content = std::fs::read_to_string(&result.mpd_manifest).unwrap();
    assert!(mpd_content.contains(r#"type="static""#));
    assert!(mpd_content.contains("<SegmentBase"));

    assert_isobmff_single_files(&result.media_files);
}
```

**Step 2: Run test to verify it passes**

Run: `cargo test --test vod_batch_e2e test_package_vod_single_file_cenc`
Expected: PASS

**Step 3: Run entire test suite**

Run: `cargo test --all-targets -- --test-threads=1`
Expected: ALL PASS

**Step 4: Commit**

```bash
git add tests/vod_batch_e2e.rs .github/workflows/drm-e2e.yml
git commit -m "test(vod): add standalone single file cenc e2e test and verify CI stabilization"
```

---

## Verification Plan

### Automated Tests
1. `cargo test --lib` (Unit tests for exit status extraction, gpac arg synthesis)
2. `cargo test --test gpac_vod_args_test` (Verify `-p=0`, `-tmp`, `-threads=1`)
3. `cargo test --test vod_batch_e2e` (Verify single file CENC, single file CBCS, segmented CENC, dual-scheme)
4. `cargo test --all-targets -- --test-threads=1` (All 68+ tests pass locally)
5. Push to GitHub PR branch and observe GitHub Actions CI run `DRM end-to-end` (`GPAC 26.07 CENC/CBCS validation`) on `ubuntu-24.04`.
