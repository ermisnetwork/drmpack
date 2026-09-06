Status: implemented

# Streamlined API Ergonomics, Concrete PackagingSession & Safe Ramdisk Lifecycle

## Problem Statement

Developers integrating `drmpack` into `media-server` encounter excessive boilerplate, ergonomic friction, and dangerous lifecycle traps:
1. **Generic Type Pollution**: `PackagingSession<P: KeyProvider>` holds an unused generic parameter `_key_provider: P` after startup key acquisition. This forces `media-server` to propagate generic type bounds across all higher-level session management structs, increasing code complexity with zero runtime benefit.
2. **Phantom Configuration in Renditions**: Declaring a `Rendition::video` requires passing 6 positional arguments (`id`, `quality_tier`, `width`, `height`, `bitrate`, `codecs`). However, GPAC demuxes the input fMP4 container directly from its stdin pipe, automatically extracting codec, resolution, and framerate from container headers (`moov`/`stsd`). Only the `QualityTier` (for DRM key mapping) and container track sequence are needed by `cecrypt`.
3. **ID Collision Vulnerability in ABR Ladders**: In multi-bitrate ABR streaming, multiple video renditions (e.g. 1080p and 720p) frequently share the same `QualityTier` ("HD") to reuse ContentKeys per ADR-0003. Naive string-based constructors cause naming ambiguity or fatal duplicate ID validation errors.
4. **Ingestion Ceremony**: Feeding media into the session requires constructing an artificial 5-field `Segment` struct (`rendition_id`, `sequence_number`, `duration_seconds`, `is_init`, `data`) even though GPAC only consumes raw byte streams from stdin and handles fragment timing internally. Connecting an async stream or mpsc channel requires writing 20-30 lines of repetitive receiver loops in caller code.
5. **Premature Ramdisk Deletion Trap**: When `auto_cleanup: true` is enabled, `session.close().await` deletes the entire Ramdisk directory immediately upon GPAC termination, destroying generated manifests before edge HTTP servers or CDNs can read and distribute them. Disabling it leaves Ramdisk memory prone to permanent leaks if `cleanup()` is neglected.
6. **Dead Configuration & Repetitive Setups**: `PackagingSessionConfig.is_live` has no effect on GPAC, and common live streaming setups require 7-8 lines of repetitive builder calls.

## Solution

A comprehensive architectural refinement (formalized in ADR-0012) that delivers:
1. **Concrete Non-Generic `PackagingSession`**: Eliminates the `<P>` generic parameter. `PackagingSession::create(config, &provider)` borrows `&impl KeyProvider` during creation, extracts resolved `KeySet`, and frees the provider reference.
2. **Collision-Free Standardized Track Identifiers**: `Rendition::video(tier)`, `Rendition::video_hd()`, `Rendition::video_4k()`, and `Rendition::audio()` automatically assign unique identifiers prefixed with `track_{type}_{uuid_short}`. Multiple renditions sharing the same `QualityTier` never collide. Human-readable labels are optional via `.with_label(name)`.
3. **High-Level Ingestion Adapters**: Provides `session.push(bytes: impl Into<Bytes>)`, `session.ingest_stream(mpsc::Receiver<Bytes>)`, and `session.run_to_completion(mpsc::Receiver<Bytes>)` to streamline both manual and channeled streaming workflows.
4. **Decoupled Finalization & RAII Ramdisk Cleanup**: `session.close().await` strictly closes stdin, flushes manifests, and verifies `#EXT-X-ENDLIST`, leaving Ramdisk files intact for distribution. RAII `Drop` automatically cleans up auto-generated temporary directories in `/dev/shm` unless `.preserve_output()` is set. Custom output directories are never deleted by `Drop`.
5. **One-Line Presets & Test Helpers**: Adds `PackagingSessionConfig::cenc("id")`, `PackagingSessionConfig::low_latency_dual("id")`, `.with_all_drm()`, and `StaticKeySource::shared_key([u8; 16])` to enable full end-to-end setup in under 5 lines of code.

## User Stories

1. As a media-server developer, I want `PackagingSession` to be a concrete struct without generic type parameters, so that I can store sessions in application state without generic pollution or dynamic trait objects.
2. As a media-server developer, I want `PackagingSession::create(config, &provider)` to take a reference to a KeyProvider, so that I do not have to transfer ownership of the provider to the session.
3. As a media-server developer, I want to declare a video rendition by simply specifying its `QualityTier` (e.g. `Rendition::video(QualityTier::hd())` or `Rendition::video_hd()`), so that I do not have to provide dummy width, height, bitrate, and codec values.
4. As a media-server developer, I want renditions to automatically generate unique track identifiers with standard prefixes (e.g. `track_video_{uuid_short}`), so that I never have to invent string IDs.
5. As a media-server developer, I want to declare multiple video renditions sharing the same `QualityTier` (e.g. 1080p and 720p both in HD tier), so that both tracks share the same ContentKey without triggering duplicate ID errors.
6. As a media-server developer, I want to optionally attach a human-readable label to a rendition via `.with_label("name")`, so that I can identify tracks in application logs.
7. As a media-server developer, I want container track IDs to default automatically to 1-based sequential indices (1, 2, 3...) matching declaration order, so that I do not have to manually number tracks in normal fMP4 streams.
8. As a media-server developer, I want an optional `.with_container_track_id(u32)` builder method, so that I can explicitly map out-of-order tracks in complex multiplexes when necessary.
9. As a media-server developer, I want `session.push(bytes)` to accept raw slices, vectors, or `Bytes` directly, so that I do not have to construct a synthetic `Segment` struct for each push.
10. As a media-server developer, I want `session.ingest_stream(rx)` to consume an async channel of media chunks until EOF, so that I do not have to write custom polling or receive loops.
11. As a media-server developer, I want `session.run_to_completion(rx)` to ingest an entire channel and finalize the session in a single call, returning manifest paths upon clean completion.
12. As a media-server developer, I want `session.close()` to finalize manifests and verify `#EXT-X-ENDLIST` without deleting output files, so that edge CDN handlers can safely read and serve the final playlist.
13. As a media-server developer, I want auto-generated Ramdisk directories (`/dev/shm/drmpack_*`) to be automatically deleted when the `PackagingSession` is dropped, so that crashed or abandoned sessions do not leak server memory.
14. As a media-server developer, I want custom output directories passed to `.with_output_dir()` to never be deleted by `Drop`, so that custom persistent paths are preserved.
15. As a media-server developer, I want `.preserve_output()` on `PackagingSessionConfig`, so that I can prevent automatic cleanup even for auto-generated Ramdisk directories when debugging.
16. As a media-server developer, I want `PackagingSessionConfig::cenc("id")` and `PackagingSessionConfig::low_latency_dual("id")` presets, so that I can configure standard live packaging pipelines with a single function call.
17. As a media-server developer, I want `.with_all_drm()` on `PackagingSessionConfig`, so that I can enable Widevine, FairPlay, and PlayReady in one call without repeating method chains.
18. As a media-server developer, I want `StaticKeySource::shared_key([u8; 16])` to generate test keys and PSSH metadata automatically, so that I can write integration tests without configuring complex key tuples.
19. As a media-server developer, I want the phantom field `is_live` removed from `PackagingSessionConfig`, so that the configuration API contains zero dead options.
20. As a media-server developer, I want `PackagingSessionConfig.renditions` to require at least one declared rendition, so that DRM key mapping policies are explicitly and unambiguously bound to tracks.

## Implementation Decisions

1. **Concrete Non-Generic `PackagingSession`**:
   - Redefine `PackagingSession` as a non-generic struct.
   - Remove `_key_provider: P` from `PackagingSession` struct fields.
   - Update constructor signature to `PackagingSession::create<P: KeyProvider>(config: PackagingSessionConfig, key_provider: &P) -> Result<Self>`.
   - The resolved `KeySet` remains the sole key artifact retained by the session.

2. **Standardized Track Identifiers & Simplified `Rendition`**:
   - `Rendition.id` is populated on construction via `format!("track_{}_{}", track_type, uuid::Uuid::new_v4().simple())`.
   - Provide dedicated constructors:
     - `Rendition::video(tier: QualityTier) -> Self`
     - `Rendition::video_hd() -> Self` (preconfigured with `QualityTier::hd()`)
     - `Rendition::video_4k() -> Self` (preconfigured with `QualityTier::uhd_4k()`)
     - `Rendition::audio() -> Self` (preconfigured with `TrackType::Audio`, `QualityTier::sd()`)
     - `Rendition::subtitle() -> Self` (preconfigured with `TrackType::Subtitle`, `encrypted: false`)
   - Add `pub label: Option<String>` with builder method `.with_label(impl Into<String>)`.
   - Rename or alias `track_id` to container track mapping with default fallback `(index + 1) as u32`.
   - Optional builder methods: `.with_container_track_id(u32)`, `.with_bitrate(u64)`, `.with_resolution(u32, u32)`, `.with_codecs(impl Into<String>)`, `.with_language(impl Into<String>)`.
   - Remove duplicate ID validation based on `id` in `validate_config`; validate only `container_track_id` uniqueness.

3. **High-Level Ingestion API**:
   - Add `pub async fn push(&mut self, data: impl Into<Bytes>) -> Result<()>` on `PackagingSession`.
   - Internal `push_data` detects media fragments via `moof` search in payload bytes.
   - Add `pub async fn ingest_stream<S>(&mut self, mut stream: S) -> Result<u64>` where `S` yields `Bytes` (or `tokio::sync::mpsc::Receiver<Bytes>`), returning chunk count.
   - Add `pub async fn run_to_completion<S>(mut self, stream: S) -> Result<PackagingResult>` which executes `ingest_stream`, invokes `close()`, and returns typed manifest paths.

4. **Decoupled Ramdisk Lifecycle & RAII Guard**:
   - Remove `auto_cleanup` boolean flag from `PackagingSessionConfig`.
   - `PackagingSession::close(&mut self)` executes process termination and `#EXT-X-ENDLIST` verification, but never invokes `cleanup_output_dir()`.
   - Implement `Drop` for `PackagingSession`: if `is_custom_output_dir` is false and `preserve_output` is false, synchronously or asynchronously purges the auto-allocated directory.
   - Provide `pub async fn cleanup(&self) -> Result<()>` for callers wishing to explicitly purge Ramdisk resources.

5. **Configuration Presets**:
   - `PackagingSessionConfig::cenc(content_id: impl Into<String>) -> Self`:
     - Sets `encryption_scheme = EncryptionScheme::Cenc`
     - Sets `drm_systems = [Widevine, PlayReady]`
     - Sets `latency_mode = LatencyMode::LowLatency`
     - Sets `segment_duration = 2.0`, `chunk_duration = 0.2`
   - `PackagingSessionConfig::low_latency_dual(content_id: impl Into<String>) -> Self`:
     - Sets `encryption_scheme = EncryptionScheme::Dual`
     - Sets `drm_systems = [Widevine, FairPlay, PlayReady]`
     - Sets `latency_mode = LatencyMode::LowLatency`
     - Sets `segment_duration = 2.0`, `chunk_duration = 0.2`
   - Add `.with_all_drm(mut self) -> Self`.
   - Add `.with_renditions(mut self, iter: impl IntoIterator<Item = Rendition>) -> Self`.
   - Purge `pub is_live: bool` from `PackagingSessionConfig`.

6. **StaticKeySource Ergonomics**:
   - Add `StaticKeySource::shared_key(key_bytes: [u8; 16]) -> Self`:
     - Generates deterministic or random KeyID.
     - Adds ContentKey for `TrackType::Video` and `TrackType::Audio`.
     - Adds default Widevine PSSH.
     - Fulfills requests for all common tiers under `SharedAll` policy without requiring manual tuple registration.

## Testing Decisions

- **Testing Seam**: The highest possible seam — the public crate API of `drmpack` (`PackagingSession`, `PackagingSessionConfig`, `Rendition`, `StaticKeySource`). Tests interact solely with public methods and examine outputs in Ramdisk.
- **Test Criteria**:
  - Behavior-focused verification: verify that valid playlists (`.m3u8` with `#EXT-X-ENDLIST`, `.mpd`) and CMAF chunks are emitted.
  - Multi-tier collision test: declare two `Rendition::video(QualityTier::hd())` and ensure both encrypt cleanly without duplicate ID panics.
  - Ingestion stream test: feed chunks via `mpsc::channel` into `ingest_stream` and `run_to_completion`, verifying clean completion.
  - Ramdisk lifecycle test: verify files exist after `close()`, and verify directory is removed upon `Drop` for auto-generated paths.
- **Prior Art**: Existing integration tests in `tests/tracer_e2e.rs` and `tests/multi_track_abr_e2e.rs`.

## Out of Scope

- Live key rotation (deferred to Milestone 2).
- Native in-Rust MP4 demuxing or transcoding.
- Offline static VOD packaging (Ticket 12 backlog).
- Legacy MPEG-2 TS output (closed as WONTFIX per ADR-0010).

## Further Notes

- References ADR-0012: `docs/adr/0012-streamlined-api-ergonomics-and-lifecycle.md`.
- References `CONTEXT.md` for domain terminology.
- Backward compatibility: As an unreleased internal crate (`0.1.0`), breaking changes are accepted cleanly without deprecation shims.
