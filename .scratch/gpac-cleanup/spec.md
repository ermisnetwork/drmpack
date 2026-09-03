Status: ready-for-agent

# Spec: GPAC Architecture Alignment & Legacy Code Pruning

## Problem Statement

Following the architectural decision documented in ADR-0004 (orchestrating GPAC filter graphs over anonymous Unix pipes instead of a native Rust rewrite) and ADR-0005 (distributing manifests and CMAF chunks through Ramdisk), `drmpack` contains substantial legacy code that is now completely dead and out of sync with the runtime architecture:

1. **Dead native crypto & container parsing (`src/mp4/`)**: `drmpack` still contains ~38KB of native MP4 box parsers, box writers, and AES-CTR CENC encryption engines (`boxes.rs`, `cenc.rs`, `parser.rs`, `writer.rs`). These were built for the initial native spike (ADR-0002) but are bypassed by GPAC (`cecrypt` filter). Maintaining them bloats build times, carries 5 unused crypto crate dependencies (`aes`, `ctr`, `cbc`, `cipher`, `byteorder`), and risks confusing developers about the encryption path.
2. **Redundant manifest generation (`src/manifest/`)**: `HlsManifestGenerator` constructs `.m3u8` playlists in-memory, but GPAC's `dasher:dual` filter already automatically generates both DASH (`live.mpd`) and HLS (`live.m3u8` + variant playlists) directly into Ramdisk (`/dev/shm`).
3. **Unused sink abstraction (`src/sink.rs`)**: The `OutputSink` and `MemoryOutputSink` traits are disconnected from `PackagingSession`, which delegates file writing directly to GPAC into Ramdisk.
4. **Outdated educational docs (`docs/learning/`)**: Lesson 0001 and Record 0001 detail recursive ISO-BMFF AST transformations and AES-CTR keystreams from the discarded native spike rather than the production GPAC filter pipeline.
5. **Incomplete session lifecycle**: `PackagingSession` lacks explicit manifest path accessors (`hls_manifest_path`, `dash_manifest_path`) and lacks a managed cleanup policy for freeing Ramdisk memory after live draining.

## Solution

Prune all dead code, purge unused dependencies, and solidify the GPAC-first architecture:

1. **Delete obsolete modules**: Remove `src/mp4/`, `src/manifest/`, and `src/sink.rs`.
2. **Purge dependencies**: Remove `aes`, `ctr`, `cbc`, `cipher`, and `byteorder` from `Cargo.toml`.
3. **Expose typed manifest accessors on `PackagingSession`**: Provide `hls_manifest_path(&self) -> PathBuf` and `dash_manifest_path(&self) -> PathBuf` pointing to `<output_dir>/live.m3u8` and `<output_dir>/live.mpd`.
4. **Implement Ramdisk lifecycle management**: Add `auto_cleanup: bool` to `PackagingSessionConfig` and an async `cleanup(&self) -> Result<()>` method on `PackagingSession` that safely removes the session Ramdisk directory when media-server finishes draining player buffers.
5. **Consolidate test helpers**: Move MP4 box inspection helpers (`find_box`) to `tests/common/mod.rs` for integration tests only, without polluting production crate exports.
6. **Update learning records**: Refresh `docs/learning/records/0001-tracer-cenc-hls.md` and `docs/learning/lessons/0001-tracer-cenc-hls-deep-dive.html` to document the architectural pivot from native crypto to GPAC filter graph orchestration.

## User Stories

1. As a media-server developer, I want `drmpack` to compile quickly without compiling unused native cryptographic ciphers (AES, CTR, CBC, block padding).
2. As a media-server developer, I want `drmpack` to have a clear, focused dependency graph containing only crates necessary for key management, process supervision, and IPC.
3. As a media-server developer, I want `PackagingSession` to provide a `hls_manifest_path()` method returning the exact path to `live.m3u8` in Ramdisk, so that I can serve it via HTTP without guessing or hardcoding filenames.
4. As a media-server developer, I want `PackagingSession` to provide a `dash_manifest_path()` method returning the exact path to `live.mpd` in Ramdisk, so that I can serve DASH players reliably.
5. As a media-server developer, I want `PackagingSession` to manage session directories in Ramdisk (`/dev/shm` or `tmpfs`), so that disk I/O bottlenecks are eliminated.
6. As a media-server developer, I want to call `session.cleanup().await` after a stream finishes its draining phase, so that Ramdisk memory is promptly freed.
7. As a media-server developer, I want to configure `auto_cleanup = true` in `PackagingSessionConfig` for automated cleanup upon `session.close()`.
8. As a media-server developer, I want `session.close()` to cleanly terminate the GPAC process and flush final chunks without immediately deleting segments that live-edge players are still downloading.
9. As an engineer exploring the codebase, I want `src/lib.rs` to expose only active GPAC orchestration, key management, and session types, so that there is no ambiguity about how DRM packaging is implemented.
10. As an engineer writing integration tests, I want shared MP4/PSSH inspection helpers isolated under `tests/common/`, so that production library code remains free of testing assertions.
11. As an engineer onboarding to `drmpack`, I want the learning documents to reflect the actual GPAC pipeline (`stdin:ext=mp4` -> `cecrypt:cfile` -> `dasher:dual`), so that I understand why the project uses subprocess orchestration instead of native ciphers.
12. As a CI engineer, I want `cargo test` and `cargo check` to run cleanly without warnings or dead-code annotations after pruning.

## Implementation Decisions

- **Removal of `src/mp4/`**: Delete `src/mp4/boxes.rs`, `src/mp4/cenc.rs`, `src/mp4/parser.rs`, `src/mp4/writer.rs`, and `src/mp4/mod.rs`. Per ADR-0004, GPAC `cecrypt` handles 100% of ISOBMFF parsing, sample encryption, subsample map creation, and box rewriting.
- **Removal of `src/manifest/`**: Delete `src/manifest/hls.rs` and `src/manifest/mod.rs`. Per ADR-0005, GPAC `dasher:dual` generates both HLS (`.m3u8`) and DASH (`.mpd`) manifests directly to Ramdisk.
- **Removal of `src/sink.rs`**: Delete `src/sink.rs` containing `OutputSink` and `MemoryOutputSink`. GPAC outputs directly to the filesystem (Ramdisk), making the sink abstraction dead code.
- **Dependency cleanup in `Cargo.toml`**: Remove `aes`, `ctr`, `cbc`, `cipher`, and `byteorder`.
- **Public API refinements in `src/session.rs`**:
  - Add `pub fn hls_manifest_path(&self) -> PathBuf { self.config.output_dir.join("live.m3u8") }`.
  - Add `pub fn dash_manifest_path(&self) -> PathBuf { self.config.output_dir.join("live.mpd") }`.
  - Add `pub auto_cleanup: bool` to `PackagingSessionConfig` (default: `false`).
  - Add builder method `pub fn with_auto_cleanup(mut self, auto_cleanup: bool) -> Self`.
  - Add `pub async fn cleanup(&self) -> Result<()>` on `PackagingSession` to delete `self.config.output_dir` if it exists.
  - In `PackagingSession::close()`, if `self.config.auto_cleanup` is true, invoke `self.cleanup().await`.
- **Update `src/lib.rs`**:
  - Remove `pub mod mp4;`, `pub mod manifest;`, `pub mod sink;`.
  - Remove re-exports of deleted types (`HlsManifestGenerator`, `MediaManifestContext`, `OutputSink`, `MemoryOutputSink`).
- **Test helper consolidation**:
  - Create `tests/common/mod.rs` containing `find_box(data: &[u8], box_type: &[u8; 4]) -> Option<&[u8]>`.
  - Update `tests/tracer_e2e.rs` to import `find_box` from `common`.
- **Documentation refresh**:
  - Update `docs/learning/records/0001-tracer-cenc-hls.md` to summarize the pivot from native spike to GPAC subprocess.
  - Update `docs/learning/lessons/0001-tracer-cenc-hls-deep-dive.html` to reflect GPAC filter graph orchestration.

## Testing Decisions

- **Single High-Level Seam**: Testing occurs exclusively at the highest possible architectural seam — `PackagingSession`'s public API (`PackagingSession::create`, `push_segment`, `close`, `cleanup`, `hls_manifest_path`, `dash_manifest_path`).
- **No low-level crypto unit tests**: Because internal ciphers are removed, tests focus on observable external behavior:
  - Process lifecycle and error reporting on missing GPAC binary.
  - Correct command-line argument construction for GPAC filter graphs.
  - Valid `drm.xml` generation for CENC and CBCS schemes.
  - End-to-end live packaging: pushing fMP4 segments and verifying that GPAC outputs `live.mpd`, `live.m3u8`, initialization segment (`stdin_dashinit.mp4`), and encrypted media segments containing valid Widevine `pssh` boxes.
  - Ramdisk cleanup verification: asserting that `cleanup()` removes the session directory from Ramdisk.
- **Prior art**: `tests/tracer_e2e.rs` and `tests/tracer_gpac_e2e.rs` already exercise this exact seam.

## Out of Scope

- Implementing Low-Latency CMAF chunking parameters (covered in Issue 02).
- Dynamic multi-system DRM XML generation for FairPlay CBCS (covered in Issue 03).
- Dual parallel encryption pipelines (covered in Issue 04).
- CPIX 2.3 or Axinom remote key provider implementations (covered in Issues 10 & 12).
- License proxy HTTP handlers (covered in Issue 13).

## Further Notes

- This cleanup reduces compilation time by avoiding five crypto and bit-manipulation dependencies.
- Eliminating native MP4 parsing eliminates any lingering risk of unintentional divergence between native code and GPAC's industrial packaging logic.
