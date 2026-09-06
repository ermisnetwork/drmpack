# 01: Streamline API Ergonomics, Concrete PackagingSession & Safe Ramdisk Lifecycle

**What to build:** Deliver a streamlined, developer-friendly public API for `drmpack` per ADR-0012 and `.scratch/api-ergonomics/spec.md`.

Transform `PackagingSession` into a concrete struct without generic type parameters, borrowing `&key_provider` during `create()`.
Streamline `Rendition` constructors (`Rendition::video(tier)`, `Rendition::video_hd()`, `Rendition::video_4k()`, `Rendition::audio()`, `Rendition::subtitle()`) with auto-generated collision-free track identifiers (`track_{type}_{uuid}`), automatic 1-based container track ID assignment (1, 2, 3...), and optional `.with_label("name")`. Multiple renditions sharing the same `QualityTier` (e.g. 1080p and 720p sharing HD) can be declared without ID collisions.
Provide a high-level ingestion data plane: `session.push(bytes: impl Into<Bytes>)`, `session.ingest_stream(rx)`, and `session.run_to_completion(rx)`.
Decouple Ramdisk lifecycle: `session.close().await` flushes GPAC and verifies `#EXT-X-ENDLIST` without deleting output files; RAII `Drop` automatically purges auto-allocated temporary directories in `/dev/shm` unless `.preserve_output()` is set. Custom caller paths are never deleted automatically.
Provide 1-line presets on `PackagingSessionConfig`: `cenc()`, `low_latency_dual()`, `.with_all_drm()`, `.with_renditions()`, and remove the dead field `is_live`.
Provide `StaticKeySource::shared_key([u8; 16])` for frictionless test provider setup. Update all tests and examples.

**Blocked by:** None (can start immediately)

**Status:** closed

- [x] `PackagingSession` is a concrete struct without generic type parameters, and `PackagingSession::create(config, &provider).await` borrows the KeyProvider reference.
- [x] `Rendition::video(QualityTier::hd())`, `Rendition::video_hd()`, `Rendition::video_4k()`, `Rendition::audio()`, and `Rendition::subtitle()` auto-generate unique identifiers (`track_{type}_{uuid_short}`) with zero collision risk when multiple renditions share a tier.
- [x] `Rendition` supports optional `.with_label(name)` and `.with_container_track_id(u32)`, with container track ID defaulting to 1-based sequential index.
- [x] `session.push(bytes)` accepts `impl Into<Bytes>` (including `&[u8]`, `Vec<u8>`, `Bytes`) and auto-detects `moof` boundaries for watchdog media tracking.
- [x] `session.ingest_stream(rx)` consumes an `mpsc::Receiver` channel to EOF and returns total ingested chunks/bytes.
- [x] `session.run_to_completion(rx)` ingests an entire stream channel and cleanly closes the session, returning manifest paths.
- [x] `session.close().await` flushes GPAC and verifies `#EXT-X-ENDLIST` while preserving output files in Ramdisk for edge serving.
- [x] Dropping a `PackagingSession` automatically cleans up auto-allocated temporary Ramdisk directories unless `.preserve_output()` is set or a custom output directory was provided.
- [x] `PackagingSessionConfig` provides `.cenc()`, `.low_latency_dual()`, `.with_all_drm()`, and `.with_renditions()` presets, with the phantom field `is_live` removed.
- [x] `StaticKeySource::shared_key([u8; 16])` provides a 1-line test provider for CENC/Widevine setups.
- [x] All unit and integration tests compile and pass 100% cleanly.
- [x] All 5 example files in `examples/` are updated to the new streamlined API and execute cleanly with no compilation warnings.
