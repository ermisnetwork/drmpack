# 08: Unmuxed input — H.264 muxer

**What to build:** Accept raw H.264 NALUs (unmuxed input) as an alternative to muxed fMP4 segments. drmpack parses SPS and PPS parameter sets to extract codec configuration, generates an fMP4 init segment (moov box), and muxes NALUs into fMP4 fragments (moof/mdat). The muxed fragments then feed into the existing encryption pipeline. Media-server provides raw NALUs tagged with timestamps; drmpack handles all container formatting.

**Blocked by:** 01 (Tracer)

**Status:** ready-for-agent

- [ ] H.264 NALU parser: identify NAL unit types, extract SPS and PPS
- [ ] SPS parser: extract resolution, profile, level, timing info for codec config
- [ ] Init segment (moov box) generator: ftyp, moov with trak/mdia/minf/stbl based on SPS/PPS data
- [ ] fMP4 fragment muxer: package NALUs into moof + mdat boxes with correct sample tables and decode/composition timestamps
- [ ] PackagingSession detects input type (muxed vs unmuxed) and routes through muxer when needed
- [ ] Test: push raw H.264 NALUs, verify output is valid encrypted fMP4 with correct init segment
- [ ] Test: verify muxed and unmuxed paths for the same content produce structurally equivalent encrypted output
