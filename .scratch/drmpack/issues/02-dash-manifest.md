# 02: Low-Latency CMAF Chunking (LL-HLS + LL-DASH)

**What to build:** Implement `LatencyMode::LowLatency` in `PackagingSession`. Configure GPAC's `dasher` filter with CMAF chunking parameters: `:cdur=0.2` (200ms fragment duration), `:asto=1.8` (Availability Time Offset for LL-DASH), and `:llhls=br` (Byte-range partial segments for LL-HLS). Verify that the generated `.m3u8` manifest contains `#EXT-X-PART` tags and `#EXT-X-PRELOAD-HINT`, and the `.mpd` contains `availabilityTimeOffset` and `availabilityTimeComplete="false"`.

**Blocked by:** 01 (Tracer)

**Status:** ready-for-agent

- [ ] Add `LatencyMode` enum (`Standard`, `LowLatency`) to `PackagingSessionConfig`
- [ ] Construct GPAC `dasher` filter arguments dynamically based on `LatencyMode`:
  - `LowLatency`: `:cdur=0.2:asto=1.8:llhls=br:cmaf=cmfc`
  - `Standard`: standard segment duration without chunking
- [ ] Test LL-HLS manifest: assert `#EXT-X-PART` entries and `#EXT-X-PRELOAD-HINT` present in variant `.m3u8`
- [ ] Test LL-DASH manifest: assert `availabilityTimeOffset` attribute present in `.mpd`
- [ ] Verify sub-second chunk availability in output directory during active stream push
