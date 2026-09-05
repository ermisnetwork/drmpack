# 09: Multi-Track ABR Packaging Engine (Consolidated 09 + 10)

**What to build:** Support multi-track ABR packaging combining multiple video Renditions (quality tiers: SD/HD/4K), distinct audio tracks, and cleartext subtitle tracks (WebVTT) within a single continuous multiplexed fMP4 pipe. Add `:alltk` to GPAC's input filter to prevent dropping secondary renditions. Allow caller to specify explicit `track_id` on `Rendition`. Ensure correct DRM XML mapping, scheme-aware key acquisition, and multi-variant HLS/DASH manifest emission.

**Consolidates:** Ticket 09 (Video quality tiers) and Ticket 10 (Audio & Subtitle tracks).

**Blocked by:** 01 (Tracer), 05 (CPIX KeyProvider)

**Status:** closed

- [x] Add `:alltk` to GPAC input filter (`stdin:ext=mp4:alltk:mstore_samples=0:mstore_purge=0`) in `src/gpac/process.rs`
- [x] Add `pub track_id: Option<u32>` and `.with_track_id(u32)` to `Rendition` in `src/types.rs`
- [x] Add `Rendition::subtitle(id, codecs)` constructor helper in `src/types.rs`
- [x] Bind `track_id` (fallback to 1-based index) in `src/session/cluster.rs` when building `GpacTrackConfig`
- [x] GPAC DRM XML maps each track ID to its tier's `ContentKey` and `KeyID` (and unencrypted for subtitles)
- [x] Multi-rendition HLS master manifest generated with `#EXT-X-STREAM-INF`, `#EXT-X-MEDIA:TYPE=AUDIO`, `#EXT-X-MEDIA:TYPE=SUBTITLES`
- [x] Multi-Representation DASH MPD generated with distinct video AdaptationSet (multiple Representations) and Audio/Subtitle AdaptationSets
- [x] Comprehensive multi-track ABR E2E test in `tests/multi_track_abr_e2e.rs` (feeding synthetic multi-track fMP4)

