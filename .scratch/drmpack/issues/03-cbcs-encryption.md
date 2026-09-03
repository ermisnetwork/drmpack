# 03: Multi-DRM XML Generator (CENC, CBCS & PSSH)

**What to build:** Build the production-grade GPAC DRM XML generator (`cecrypt` configuration). Support CENC (AES-128-CTR) for Widevine and PlayReady, and CBCS (AES-128-CBC with 1:9 pattern) for Apple FairPlay. The generator constructs valid GPAC Common Encryption XML defining `CrypTrack` elements, key values, key IDs, IV sizes, and `DRMInfo` elements with multi-system PSSH data for all target DRM systems.

**Blocked by:** 01 (Tracer)

**Status:** ready-for-agent

- [ ] XML generator support for `EncryptionScheme::Cenc` (`scheme_type="cenc"`, AES-CTR, 16-byte IV)
- [ ] XML generator support for `EncryptionScheme::Cbcs` (`scheme_type="cbcs"`, AES-CBC 1:9 pattern, constant IV)
- [ ] Multi-DRM PSSH injection: embed Widevine (UUID `edef...`), PlayReady (UUID `9a04...`), and FairPlay signaling in the XML
- [ ] Unit tests verifying XML structure matches GPAC `cecrypt` specification
- [ ] Integration test: verify GPAC correctly parses the generated XML and encrypts sample tracks
