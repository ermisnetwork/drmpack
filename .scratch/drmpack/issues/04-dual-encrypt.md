# 04: Dual encrypt + separate manifests

**What to build:** When a session is configured with both CENC and CBCS encryption schemes, drmpack produces two sets of encrypted segments and three separate manifests: HLS-FairPlay (CBCS segments), HLS-Widevine (CENC segments), and DASH (CENC segments with Widevine + PlayReady ContentProtection). Each manifest references only its corresponding segment set. OutputSink receives all artifacts with distinct paths.

**Blocked by:** 02 (DASH manifest), 03 (CBCS encryption)

**Status:** ready-for-agent

- [ ] Session config accepts both CENC + CBCS simultaneously
- [ ] Encryption pipeline produces two segment sets (CENC-encrypted and CBCS-encrypted) from each input segment
- [ ] Three manifests emitted: `master_fairplay.m3u8` (CBCS), `master_widevine.m3u8` (CENC), `manifest.mpd` (CENC)
- [ ] Each manifest's segment URLs reference the correct segment set
- [ ] OutputSink paths clearly distinguish CENC vs CBCS segments
- [ ] Test: dual-encrypt session, verify three manifests, verify each manifest's segments use the correct encryption scheme
