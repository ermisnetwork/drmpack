# 13: HLS-TS Legacy Output via GPAC

**What to build:** An optional output profile for legacy clients (older SmartTVs, iOS <10) that do not support fMP4-based HLS. Configure GPAC with MPEG-2 Transport Stream segmentation and SAMPLE-AES encryption for CBCS in TS containers. Output is generated as an additional legacy rendition alongside the primary CMAF (fMP4) output.

**Blocked by:** 11 (Process Lifecycle)

**Status:** wontfix (per ADR-0010: Standardize on Dual CMAF over Legacy MPEG-2 TS SAMPLE-AES)

- [x] Declining MPEG-2 TS SAMPLE-AES support; Apple ecosystem standardized on CMAF fMP4 `cbcs` since iOS 10 (2016), and SmartTVs use CENC fMP4 over DASH.
- [x] Removed `hls-legacy` feature flag from `Cargo.toml`.
- [x] Full rationale and industry survey documented in `docs/adr/0010-cmaf-only-over-legacy-hls-ts.md`.
