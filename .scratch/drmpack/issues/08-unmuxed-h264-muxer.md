# 08: CPIX KeyProvider

**What to build:** A KeyProvider implementation that fetches encryption keys via the standardized CPIX 2.3 (Content Protection Information Exchange) protocol. Constructs a valid CPIX XML request document with `ContentKeyUsageRule` elements for each requested (track_type, QualityTier), sends via HTTP POST to a provider endpoint, and parses the multi-key XML response containing `ContentKeys`, `KIDs`, and multi-DRM `PSSH` data.

**Blocked by:** 01 (Tracer)

**Status:** ready-for-agent

- [ ] CPIX request XML builder: construct CPIX 2.3 document requesting keys for specified quality tiers
- [ ] Async HTTP client (reqwest) executing POST request to provider endpoint
- [ ] CPIX response XML parser: extract ContentKeys, KIDs, and PSSH elements for Widevine, FairPlay, and PlayReady
- [ ] Integration test with mock CPIX HTTP server
