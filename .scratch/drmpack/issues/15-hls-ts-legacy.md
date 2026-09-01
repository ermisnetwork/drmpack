# 15: HLS-TS legacy output

**What to build:** An optional output mode (behind the `hls-legacy` feature flag) that produces MPEG-2 Transport Stream segments instead of fMP4, with a corresponding m3u8 manifest using TS-specific DRM signaling. This supports older devices (iOS <10, legacy smart TVs) that don't support fMP4-based HLS. The TS output is an additional output alongside the default CMAF (fMP4) output, not a replacement.

**Blocked by:** 06 (Live streaming)

**Status:** ready-for-agent

- [ ] TS muxer: package encrypted samples into MPEG-2 TS segments (.ts files) with PAT/PMT tables
- [ ] TS-specific encryption: SAMPLE-AES for CBCS mode in TS containers
- [ ] HLS manifest for TS: EXT-X-KEY with METHOD=SAMPLE-AES, correct IV, URI to key/license
- [ ] Emitted as additional output via OutputSink alongside CMAF output (not instead of)
- [ ] Behind `hls-legacy` feature flag — not compiled when not needed
- [ ] Test: enable hls-legacy, push segments, verify valid TS output alongside fMP4 output
- [ ] Test: verify TS manifest uses correct METHOD and segment URLs with .ts extension
