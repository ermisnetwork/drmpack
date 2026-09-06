# Streamlined API Ergonomics, Concrete PackagingSession, and Safe Ramdisk Lifecycle

**Status: Accepted**

`drmpack` eliminates phantom configuration fields, replaces generic type pollution on `PackagingSession` with a non-generic concrete struct, introduces collision-free standardized track identifiers (`track_{type}_{uuid}`), streamlines `Rendition` constructors, adds flexible ingestion adapters (`push`, `ingest_stream`, `run_to_completion`), and decouples stream finalization from Ramdisk resource cleanup with RAII safety.

## Context & Problem Statement

An exhaustive audit of library usability and developer experience revealed several ergonomic bottlenecks and design traps:
1. **Generic Type Pollution**: `PackagingSession<P: KeyProvider>` stored `_key_provider: P` solely to satisfy generic constraints, but never called the provider after initial `create()`. This forced callers (`media-server`) to propagate generic parameters or use dynamic dispatch across all parent structs.
2. **Phantom Configuration in Rendition**: `Rendition::video` mandated 6 positional arguments (`id`, `quality_tier`, `width`, `height`, `bitrate`, `codecs`). GPAC demuxes the input fMP4 directly from its stdin pipe, automatically extracting codec, resolution, and timing from container headers (`moov`/`stsd`). Only `QualityTier` and `track_id` are technically required for GPAC DRM XML generation.
3. **ID Collisions on Shared Tiers**: In ABR ladders, multiple video renditions (e.g. 1080p and 720p) frequently share the same `QualityTier` ("HD") to reuse ContentKeys per ADR-0003. Naive string-based constructors led to name ambiguity or validation failures due to duplicate IDs.
4. **Ingestion Ceremony**: Callers were forced to construct a 5-field synthetic `Segment` struct (`sequence_number`, `duration_seconds`, `rendition_id`, `is_init`, `data`) even though GPAC only reads continuous byte streams from stdin and calculates timing internally. Streaming channel integration required 20-30 lines of boilerplate receiver loops.
5. **Premature Ramdisk Deletion Trap**: In previous releases, `auto_cleanup: true` deleted the entire Ramdisk output folder inside `session.close().await`, destroying generated playlists before edge CDN workers could read them. Conversely, disabling `auto_cleanup` risked unbounded Ramdisk leakage if `cleanup()` was omitted.
6. **Phantom Configuration Fields**: `PackagingSessionConfig.is_live` had no effect on GPAC, which was hard-coded to live mode. Common session setups required 7-8 lines of repetitive builder calls.

## Considered Options

- **Option 1: Retain Generic `PackagingSession<P>` with `Arc<dyn KeyProvider>`**: Deferred; key rotation during live sessions is out of scope for Milestone 1. Storing an unused generic provider adds zero value while polluting caller signatures.
- **Option 2: Zero-Rendition Default Mode**: Considered during initial grilling, but rejected in favor of explicit `Rendition` declarations to maintain clear DRM key-to-tier mappings and deterministic multi-track packaging.
- **Option 3: Standardized Track Identifiers and Decoupled Lifecycle**: Accepted. Eliminates phantom data, generates collision-free `track_{type}_{uuid}` IDs, streamlines `Rendition` construction, adds high-level stream ingestion, and secures Ramdisk cleanup via RAII.

## Architecture & Decisions

1. **Concrete Non-Generic `PackagingSession`**:
   - `PackagingSession` is now a concrete struct without generic type parameters.
   - `PackagingSession::create(config, &provider).await` borrows `&impl KeyProvider` during startup, fetches keys once, and retains only resolved `KeySet`.
2. **Collision-Free Track Identifiers (`track_{type}_{uuid}`)**:
   - `Rendition::video(tier)`, `Rendition::video_hd()`, `Rendition::video_4k()`, and `Rendition::audio()` automatically assign a collision-free identifier with a standard prefix: `track_video_{uuid_short}` or `track_audio_{uuid_short}`.
   - Multiple renditions sharing the same `QualityTier` (e.g. 1080p and 720p sharing "HD") receive distinct unique IDs with zero collision risk.
   - Human-readable labels for debugging and manifests are decoupled into `.with_label(name)`.
   - Technical metadata (`width`, `height`, `bitrate`, `codecs`, `frame_rate`) is completely purged from `Rendition`; GPAC auto-detects all stream parameters directly from fMP4 container headers (`moov`/`stsd`).
3. **Streamlined Ingestion Data Plane**:
   - `session.push(bytes: impl Into<Bytes>).await` is the primary ingestion method for raw fMP4 chunks, automatically detecting `moof` boundaries for watchdog media tracking.
   - `session.ingest_stream(mut rx: mpsc::Receiver<Bytes>).await` streams from an async channel to stdin until EOF, returning total bytes/chunks ingested.
   - `session.run_to_completion(mut rx: mpsc::Receiver<Bytes>).await` runs ingestion and graceful finalization in a single end-to-end call, returning manifest paths.
4. **Decoupled Manifest Finalization & RAII Ramdisk Cleanup**:
   - `session.close().await` strictly finalizes GPAC subprocesses, flushes manifests, and verifies `#EXT-X-ENDLIST`. Media files on Ramdisk remain intact for edge serving.
   - `session.cleanup().await` explicitly deletes the output directory when distribution is finished.
   - If `output_dir` was automatically allocated by the library in `/dev/shm` (or temp dir), the `Drop` implementation automatically deletes it when the session leaves scope, preventing memory leaks unless `.preserve_output()` is set. Custom caller directories configured via `.with_output_dir()` are never deleted by `Drop`.
5. **One-Line Session Presets**:
   - `PackagingSessionConfig::cenc("stream-id")` preconfigures Widevine + PlayReady CENC, LowLatency mode, 2.0s segments, 0.2s chunks.
   - `PackagingSessionConfig::low_latency_dual("stream-id")` preconfigures Dual CENC+CBCS with Widevine, FairPlay, and PlayReady.
   - `.with_all_drm()` adds all target DRM systems in a single call.
   - `.with_renditions(iter)` accepts arrays, slices, or iterators of `Rendition`.
   - The unused `is_live` field is purged.
6. **Test Provider Ergonomics**:
   - `StaticKeySource::shared_key([u8; 16])` constructs an in-memory key provider with preconfigured ContentKey and Widevine PSSH for frictionless unit tests and examples.

## Consequences

- Media-server code size for live DRM session setup is reduced by ~75%.
- Type signatures across `media-server` no longer require generic propagation for `PackagingSession`.
- Multiple renditions sharing the same QualityTier work seamlessly without ID collisions.
- Ramdisk memory leaks are prevented by RAII guards while avoiding premature manifest deletion during active edge distribution.
