# 05: CPIX 2.3 KeyProvider & Scheme-Aware Key Architecture

**What to build:** A KeyProvider implementation that fetches encryption keys via the standardized DASH-IF CPIX 2.3 (Content Protection Information Exchange) protocol over HTTP POST. Constructs a valid CPIX XML request document with `ContentKeyUsageRule` elements for requested keys. Upgrades `KeyRequest` and `KeySet` to be scheme-aware (distinguishing `EncryptionScheme::Cenc` and `EncryptionScheme::Cbcs` keys per ADR-0006 and PlayReady/ISO security guidelines). Parses the multi-key XML response containing `ContentKeys` (with `<pskc:PlainValue>`), `KIDs`, and multi-DRM `PSSH` / FairPlay signaling data.

**Blocked by:** 01 (Tracer), 04 (Dual Encryption)

**Status:** done

- [x] Upgrade `KeyRequest` and `KeySet` in `src/key.rs` to support `EncryptionScheme` key differentiation
- [x] CPIX request XML builder: construct CPIX 2.3 document (`urn:dashif:org:cpix`, `urn:ietf:params:xml:ns:keyprov:pskc`)
- [x] Async HTTP client (`reqwest`) executing POST request to provider endpoint
- [x] CPIX response XML parser: extract ContentKeys (`<pskc:PlainValue>`), KIDs, and PSSH elements for Widevine, FairPlay, and PlayReady
- [x] Unit & integration tests with mock CPIX HTTP server
- [x] Code review fixes: redaction of secret keys in `Debug`, self-closing XML tag support, FairPlay PSSH suppression in ISO-BMFF init fragments, PSSH KID/scheme binding
- [x] Selective encryption (`Rendition.clear()`, `<CrypTrack IsEncrypted="0"/>`) & `KeyMappingPolicy` (ADR-0007)

