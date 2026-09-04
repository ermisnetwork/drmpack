# 05: CPIX 2.3 KeyProvider & Scheme-Aware Key Architecture

**What to build:** A KeyProvider implementation that fetches encryption keys via the standardized DASH-IF CPIX 2.3 (Content Protection Information Exchange) protocol over HTTP POST. Constructs a valid CPIX XML request document with `ContentKeyUsageRule` elements for requested keys. Upgrades `KeyRequest` and `KeySet` to be scheme-aware (distinguishing `EncryptionScheme::Cenc` and `EncryptionScheme::Cbcs` keys per ADR-0006 and PlayReady/ISO security guidelines). Parses the multi-key XML response containing `ContentKeys` (with `<pskc:PlainValue>`), `KIDs`, and multi-DRM `PSSH` / FairPlay signaling data.

**Blocked by:** 01 (Tracer), 04 (Dual Encryption)

**Status:** ready-for-agent

- [ ] Upgrade `KeyRequest` and `KeySet` in `src/key.rs` to support `EncryptionScheme` key differentiation
- [ ] CPIX request XML builder: construct CPIX 2.3 document (`urn:dashif:org:cpix`, `urn:ietf:params:xml:ns:keyprov:pskc`)
- [ ] Async HTTP client (`reqwest`) executing POST request to provider endpoint
- [ ] CPIX response XML parser: extract ContentKeys (`<pskc:PlainValue>`), KIDs, and PSSH elements for Widevine, FairPlay, and PlayReady
- [ ] Unit & integration tests with mock CPIX HTTP server
