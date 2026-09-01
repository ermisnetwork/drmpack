# 01: Tracer — single CENC-encrypted segment with HLS manifest

**What to build:** The thinnest end-to-end path through drmpack. Create the crate scaffold (Cargo.toml with feature flags, module structure), define core domain types (PackagingSession, Rendition, QualityTier, EncryptionScheme, ContentKey, KeyID), the KeyProvider trait, the OutputSink trait, and RawKeyProvider. Then implement the minimal pipeline: accept one muxed fMP4 segment (H.264 video), CENC-encrypt it with a key from RawKeyProvider, generate a basic HLS manifest (m3u8) with EXT-X-KEY DRM signaling, and deliver encrypted segment + manifest through OutputSink. Include a MemoryOutputSink test double that collects output in memory for assertions.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] Crate scaffold: Cargo.toml with feature flags (`widevine`, `fairplay`, `playready`, `hls-legacy`, `cpix`, `speke-v2`, `axinom`), async dependencies (tokio, tracing)
- [ ] Core domain types defined: PackagingSession config struct, Rendition, QualityTier, EncryptionScheme, ContentKey, KeyID
- [ ] KeyProvider trait defined with `async fn fetch_keys()` signature
- [ ] OutputSink trait defined with `write_segment()`, `write_init_segment()`, `write_manifest()` methods
- [ ] RawKeyProvider implementation accepts manually-specified keys
- [ ] MemoryOutputSink collects all output for test assertions
- [ ] CENC (AES-128-CTR) encryption of fMP4 media samples (moof/mdat)
- [ ] PSSH box injection into init segment
- [ ] Basic HLS manifest (m3u8) with EXT-X-KEY tag and CDN base URL in segment URLs
- [ ] PackagingSession wires the pipeline: config → fetch keys → encrypt segment → generate manifest → OutputSink
- [ ] Test: create session with RawKeyProvider + MemoryOutputSink, push one fMP4 segment, verify encrypted output bytes differ from input, verify manifest is valid m3u8 with correct DRM signaling
