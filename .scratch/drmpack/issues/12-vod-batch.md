# 12: VOD Whole-File Batch Packaging API

**What to build:** A standalone batch packaging API for static VOD assets (`drmpack::vod::package_vod_file`). Unlike live streaming which operates over real-time pipes into Ramdisk, VOD packaging runs as a one-shot process on disk, generating static HLS (`.m3u8` with `#EXT-X-ENDLIST`) and DASH (`.mpd`) manifests with complete duration and index metadata.

**Architecture & Industry Standards (from Web Research):**
1. **Decoupled Standalone API:** Do NOT bloat `PackagingSession` (which is a streaming pipe actor with heartbeat watchdogs and Ramdisk sliding windows). Expose a dedicated helper function:
   ```rust
   pub async fn package_vod_file<P: KeyProvider>(
       config: VodPackagingConfig,
       key_provider: P,
   ) -> Result<VodPackagingResult>;
   ```
2. **Flexible Input Source:**
   - Single-file multiplexed: `VodSource::SingleFile(PathBuf)` -> `gpac -i input.mp4:alltk cecrypt:... -o out/vod.mpd:dual:profile=onDemand`.
   - Multi-file separate renditions: `VodSource::SeparateFiles(Vec<PathBuf>)` -> `gpac -i v1080.mp4 -i v720.mp4 -i audio.mp4 ...`.
3. **Byte-Range Single-File vs Segmented Output:**
   - Default: Byte-Range Single-File (`profile=onDemand`). Emits 1 self-contained `.mp4` per rendition containing an `sidx` box, with `#EXT-X-BYTERANGE` in HLS and `<SegmentBase>` in DASH. (Apple HLS Authoring Spec & DASH-IF standard, reduces storage file count by 99%).
   - Optional: Multi-Segment mode emitting discrete `.m4s` chunks when required.
4. **DRM & Multi-Scheme:** Reuses existing `KeyProvider` (CPIX, Axinom, SPEKE) and `GpacDrmXmlGenerator` for CENC, CBCS, and Dual packaging.

**Blocked by:** 09 (Multi-Track ABR Engine)

**Status:** deferred (backlog — focus on live streaming session first)

- [ ] Define `VodSource` (`SingleFile(PathBuf)`, `SeparateFiles(Vec<PathBuf>)`)
- [ ] Define `VodPackagingConfig` (source, output_dir, renditions, scheme, segment_duration, single_file_byterange)
- [ ] Implement `drmpack::vod::package_vod_file` running one-shot GPAC CLI
- [ ] Support both Byte-Range Single-File (`profile=onDemand`) and Segmented output
- [ ] Support Dual CENC + CBCS static packaging in subdirectories
- [ ] Add integration test verifying static `.m3u8` (`#EXT-X-ENDLIST`, `#EXT-X-BYTERANGE`) and `.mpd` generation
