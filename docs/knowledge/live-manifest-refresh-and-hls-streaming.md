# Research: HLS Live Manifest Refresh & Real-time Stream Emission

**Date:** 2026-09-09  
**Status:** Completed  
**Sources Consulted:**
- RFC 8216 (HTTP Live Streaming - IETF Specification)
- GPAC Official Documentation (`gpac -h dasher`, `gpac -h mp4mx`)
- `drmpack` Source Code (`src/session/harvester.rs`, `src/gpac/process.rs`, `src/session/mod.rs`)
- Empirical Live Run Instrumentation (`cargo run --example 07_e2e_live_axinom`)

---

## 1. Primary Standards: RFC 8216 Live Playlist Refresh Rules

In HTTP Live Streaming (RFC 8216):

1. **Master Playlist (`live.m3u8`)**:
   - Lists available variants/renditions (`#EXT-X-STREAM-INF`) and audio groups (`#EXT-X-MEDIA:TYPE=AUDIO`).
   - **Static by specification**: The master playlist does **not** change during a live stream. Players download it once on playback initialization.

2. **Media Playlists (`video_720p.m3u8`, `audio.m3u8`)**:
   - Section 6.2.1: During a live broadcast, media playlists represent a **sliding window** of the most recent media segments.
   - Section 6.3.4: The player must reload the media playlist every target duration (e.g. every 2 seconds).
   - The packager must update the playlist file continuously, appending each new segment as it is completed and pruning older segments when the sliding window is exceeded (`#EXT-X-MEDIA-SEQUENCE` increments).
   - `#EXT-X-ENDLIST`: Must **never** be present in a live playlist while the stream is active. It is only appended when the stream permanently terminates.

---

## 2. How `drmpack` Emits Manifests in Real-Time

In `drmpack`, manifest delivery operates in two distinct consumption models:

### Model A: Direct Output Channel (`session.take_output_receiver()`) — Recommended (ADR-0015)
- GPAC writes `.m3u8` playlists into ephemeral staging (`std::env::temp_dir()`).
- `Harvester` runs on an asynchronous 200ms ticker (`src/session/harvester.rs:472`).
- Every 200ms, `Harvester` inspects all manifests in staging:
  - Validates format via `is_valid_hls_manifest`.
  - Checks against in-memory `state.manifest_cache`.
  - If content changed, immediately dispatches `PackagedArtifact { kind: ArtifactKind::Manifest, data: Bytes, ... }` through the channel.
- **Empirical Proof** (Measured during live ingestion of `07_e2e_live_axinom`):
  ```
  [10.0s] cbcs_m3u8=2 cbcs_segs=1: video_720p.m3u8 (1 segs: ['video_720p_1.m4s'])
  [16.0s] cbcs_m3u8=3 cbcs_segs=2: audio.m3u8 (1 segs: ['audio_1.m4s'])
  [18.0s] cbcs_m3u8=3 cbcs_segs=3: video_720p.m3u8 (2 segs: ['video_720p_1.m4s', 'video_720p_2.m4s'])
  [24.1s] cbcs_m3u8=3 cbcs_segs=4: video_720p.m3u8 (3 segs: ['video_720p_1.m4s', ... 'video_720p_3.m4s'])
  [26.1s] cbcs_m3u8=3 cbcs_segs=5: audio.m3u8 (2 segs: ['audio_1.m4s', 'audio_2.m4s'])
  ```
  **Conclusion:** In direct output channel mode, `.m3u8` manifests **are already emitted continuously in real-time** every time a new segment finishes (~every 2 seconds).

### Model B: Legacy Disk-Serving Mode (Without `take_output_receiver()`)
- GPAC writes files directly to `config.output_dir`.
- GPAC updates `video_720p.m3u8` in real-time on disk.
- However, in older examples (`01_basic_live_cenc.rs`, `02_low_latency_dual_cmaf.rs`), the example code pushed all chunks in a tight loop and only printed `session.hls_manifest_path()` *after* `session.close().await`, creating the false impression that manifest files only appear at the end.

---

## 3. Why the Impression "Only Emitted at the End" Arose

1. **Post-Mortem Inspection**: When checking `scratch/cdn_storage/cbcs/video_720p.m3u8` *after* the example process exits, the file contains `#EXT-X-ENDLIST` and all 50 segments. This is the finalized VOD/archive state created by `session.close()`.
2. **Initial Segment Buffering Delay (0-8s)**:
   - GPAC dasher requires 2 full seconds of GOP video to produce segment 1.
   - `is_valid_hls_manifest` requires `#EXTINF:` to be present before declaring a media playlist valid.
   - Therefore, `video_720p.m3u8` is not emitted during seconds 0..8 until segment 1 is finalized on disk.
3. **Old Examples**: Examples 01 and 02 ran `run_to_completion()` or `close().await` before inspecting disk paths.
