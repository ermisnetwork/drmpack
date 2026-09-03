# 04: Dual encryption pipeline (CENC + CBCS)

**What to build:** When a session is configured with `EncryptionScheme::Dual`, `drmpack` coordinates the generation of two parallel encrypted streams from a single input: one CENC-encrypted (for Widevine & PlayReady) and one CBCS-encrypted (for Apple FairPlay). In GPAC, this can be achieved either by running twin dasher filter branches in a single filter graph or managing twin subprocesses. Output manifests (`master_widevine.m3u8`, `master_fairplay.m3u8`, and `live.mpd`) reference their respective segment sets in Ramdisk.

**Blocked by:** 02 (Low-Latency CMAF), 03 (Multi-DRM XML)

**Status:** ready-for-agent

- [ ] `PackagingSessionConfig` accepts `EncryptionScheme::Dual`
- [ ] GPAC filter graph setup or twin process coordination for dual output
- [ ] Output directory contains separate subtrees for CENC and CBCS segments
- [ ] Separate manifests emitted: `master_fairplay.m3u8` (CBCS), `master_widevine.m3u8` (CENC), and `manifest.mpd` (CENC)
- [ ] Integration test: verify both sets of segments are correctly encrypted with their respective cipher schemes
