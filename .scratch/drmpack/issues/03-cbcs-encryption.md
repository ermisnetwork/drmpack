# 03: CBCS encryption mode

**What to build:** Add CBCS (AES-128-CBC with pattern encryption) alongside the existing CENC mode. EncryptionScheme is configurable per-session. CBCS uses 1:9 crypt/skip pattern by default (as required by FairPlay). The encryption pipeline selects the correct mode based on session config.

**Blocked by:** 01 (Tracer)

**Status:** ready-for-agent

- [ ] CBCS encryption implementation: AES-128-CBC with configurable crypt_byte_block and skip_byte_block (default 1:9)
- [ ] Subsample encryption for video NAL units (encrypt only slice data, not headers)
- [ ] EncryptionScheme selection in PackagingSession config (CENC or CBCS)
- [ ] HLS manifest uses correct EXT-X-KEY METHOD for CBCS (`SAMPLE-AES`)
- [ ] Test: create session with CBCS, push segment, verify encryption differs from CENC, verify manifest uses correct METHOD
