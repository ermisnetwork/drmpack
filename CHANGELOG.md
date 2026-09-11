# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] - 2025-09-11

### Fixed

- **[Critical]** SIMD moof box scanning — replaced O(N) `bytes.windows(4)` with `memchr::memmem` + 4KB scan limit + state-latch fast path, preventing Tokio worker thread blocking on large media chunks
- SessionWriter forward_task now properly transitions session to terminal state on write failure, preventing silent hangs
- Removed useless `BufWriter` + `flush()` from GPAC stdin pipe — zero benefit for KB-MB chunks, eliminated redundant memcpy
- Standardized CPIX `intendedTrackType` from compound values (`AUDIO_HD`) to SPEKE v2 standard `AUDIO`
- FairPlay certificate cache thundering herd — replaced `RwLock` with `tokio::sync::Mutex` to serialize cold-cache fetches
- `reqwest` fallback client now preserves configured timeout instead of silently dropping it
- `LatencyMode::default()` returns `Standard` instead of `LowLatency`, aligning with ADR-0014 canonical baseline
- `cenc()` preset no longer silently overrides latency mode to `LowLatency`

### Changed

- PSSH data detection uses byte-slice operations instead of `from_utf8_lossy` heap allocations
- String truncation uses `floor_char_boundary()` instead of manual `is_char_boundary` loop
- Tier ranking functions refactored from if/else chains to idiomatic `match` patterns
- Dual-scheme key fetch (CENC + CBCS) runs concurrently via `tokio::try_join!` instead of sequentially
- Added `memchr` as direct dependency (already transitive via quick-xml, zero new crates)

### Added

- Documentation footnote for `cbcs-1-9` encryption pattern equivalence

[0.1.1]: https://github.com/ermisnetwork/drmpack/compare/v0.1.0...v0.1.1
