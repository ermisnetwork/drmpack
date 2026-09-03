# 13: HLS-TS Legacy Output via GPAC

**What to build:** An optional output profile for legacy clients (older SmartTVs, iOS <10) that do not support fMP4-based HLS. Configure GPAC with MPEG-2 Transport Stream segmentation and SAMPLE-AES encryption for CBCS in TS containers. Output is generated as an additional legacy rendition alongside the primary CMAF (fMP4) output.

**Blocked by:** 06 (Process Lifecycle)

**Status:** ready-for-agent

- [ ] Add `enable_hls_ts_legacy` flag to `PackagingSessionConfig` (behind Cargo feature flag `hls-legacy`)
- [ ] GPAC filter configuration to output MPEG-2 TS segments (`.ts`) with SAMPLE-AES
- [ ] Generate corresponding TS-based HLS manifest
