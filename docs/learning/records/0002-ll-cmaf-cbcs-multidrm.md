# Milestone 0002: Low-Latency CMAF Chunking & Multi-DRM CBCS Orchestration

Documented the critical completion of Ticket 02 (Low-Latency CMAF Chunking) and Ticket 03 (Multi-DRM XML Generator for CENC, CBCS, and Apple FairPlay) within `drmpack`.

## Key Architectural Achievements

- **GPAC Pipe Ingest Latency Fix**: Discovered and resolved a 50-frame / 50KB memory buffering stall in GPAC's pipe demuxer by passing `:mstore_samples=0:mstore_purge=0` to the `stdin:ext=mp4` filter. This guarantees immediate sub-second flushing of CMAF chunks and manifests into Ramdisk during active live streaming.
- **Multi-DRM Common Encryption XML Engine**: Upgraded `GpacDrmXmlGenerator` to dynamically switch root encryption schemes (`<GPACDRM type="cenc">` vs `<GPACDRM type="cbcs">`) and construct compliant HLS `#EXT-X-KEY` definitions using GPAC's `,URI=` delimiter syntax.
- **Apple FairPlay CBCS HLS Signaling**: Integrated native FairPlay `skd://<KID>` URI formatting with `KEYFORMAT="com.apple.streamingkeydelivery"`, satisfying Apple iOS/Safari hardware CDM requirements while preserving parallel Widevine and PlayReady PSSH signaling.
- **End-to-End Validation**: Established automated verification running real `gpac` binaries to validate both AES-CTR (CENC) and AES-CBC 1:9 pattern (CBCS) encryption down to ISO-BMFF box structures (`tenc`, `schm`, `senc`, `saiz`, `saio`).

## Deep-Dive Technical Lesson
For an exhaustive line-by-line code dissection with architectural data-flow diagrams and binary box trees, refer to:
- [`docs/learning/lessons/0002-ll-cmaf-cbcs-multidrm-deep-dive.html`](../lessons/0002-ll-cmaf-cbcs-multidrm-deep-dive.html)
