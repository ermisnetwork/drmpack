# 09: H.265 (HEVC) support

**What to build:** Extend the muxer and encryption pipeline for H.265/HEVC content. Parse VPS, SPS, and PPS parameter sets. Generate correct init segments with HEVCDecoderConfigurationRecord. Handle H.265 NAL unit structure for subsample encryption (CENC and CBCS). Support both muxed fMP4 (H.265) and unmuxed (raw H.265 NALUs) input.

**Blocked by:** 08 (H.264 muxer)

**Status:** ready-for-agent

- [ ] H.265 NALU parser: identify NAL unit types (VPS, SPS, PPS, IDR, trail), extract parameter sets
- [ ] H.265 SPS parser: extract resolution, profile, tier, level for HEVCDecoderConfigurationRecord
- [ ] Init segment with hvcC box (HEVCDecoderConfigurationRecord) instead of avcC
- [ ] fMP4 fragment muxer handles H.265 NAL units (different start code and header structure from H.264)
- [ ] Subsample encryption: correct slice header parsing for H.265 (different from H.264)
- [ ] Test: push muxed H.265 fMP4 segment, verify correct encryption
- [ ] Test: push unmuxed H.265 NALUs, verify muxing + encryption produces valid output
