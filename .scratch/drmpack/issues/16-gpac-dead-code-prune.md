# 16: GPAC Architecture Alignment, Dead Code Pruning & Ramdisk Lifecycle

**What to build:** Align the codebase with ADR-0004 and ADR-0005 by removing obsolete native MP4 parsing, native crypto engines, redundant in-memory manifest generators, and unused sink abstractions. Purge 5 unused crypto dependencies from `Cargo.toml`. Enhance `PackagingSession` with typed manifest paths (`hls_manifest_path()`, `dash_manifest_path()`) and Ramdisk lifecycle management (`cleanup()`, `auto_cleanup`). Consolidate test helpers into `tests/common/mod.rs` and update educational records.

**Blocked by:** 01 (Tracer)

**Status:** done

- [x] Delete obsolete modules: `src/mp4/`, `src/manifest/`, and `src/sink.rs`
- [x] Remove unused dependencies from `Cargo.toml`: `aes`, `ctr`, `cbc`, `cipher`, `byteorder`
- [x] Update `src/lib.rs` to remove deleted modules and re-exports
- [x] Update `PackagingSession` in `src/session.rs`:
  - Add `hls_manifest_path(&self) -> PathBuf`
  - Add `dash_manifest_path(&self) -> PathBuf`
  - Add `auto_cleanup: bool` and `with_auto_cleanup()` to `PackagingSessionConfig`
  - Add `cleanup(&self) -> Result<()>` to clean up `output_dir` in Ramdisk
  - Call `cleanup()` in `close()` when `auto_cleanup == true`
- [x] Create `tests/common/mod.rs` with `find_box` helper and update integration tests
- [x] Add unit test verifying `cleanup()` and manifest path helpers on `PackagingSession`
- [x] Refresh `docs/learning/records/0001-tracer-cenc-hls.md` and `docs/learning/lessons/0001-tracer-cenc-hls-deep-dive.html` to reflect GPAC filter pipeline architecture
- [x] Verify `cargo test` and `cargo check` compile and pass 100% cleanly
