# 10: CPIX KeyProvider

**What to build:** A KeyProvider implementation that fetches encryption keys via the CPIX (Content Protection Information Exchange) protocol. Sends a CPIX XML document via HTTP POST to a provider endpoint, parses the multi-key response containing ContentKeys, KIDs, and PSSH data for each DRM system. Supports batch requests for multiple (track_type, QualityTier) combinations in a single call. Keys are cached for the PackagingSession lifetime.

**Blocked by:** 01 (Tracer)

**Status:** ready-for-agent

- [ ] CPIX request XML builder: construct a valid CPIX 2.3 document with ContentKeyUsageRule elements for each requested key
- [ ] HTTP POST to provider endpoint with CPIX XML body
- [ ] CPIX response parser: extract ContentKey (encrypted or clear), KeyID, DRMSystem elements (PSSH, content protection data) for Widevine, PlayReady, FairPlay
- [ ] Multi-key support: one request returns keys for all tiers/tracks
- [ ] Per-session caching: keys fetched once, stored in session, not re-requested
- [ ] Provider config: endpoint URL, authentication credentials
- [ ] Behind `cpix` feature flag
- [ ] Test: mock HTTP server returns CPIX response, verify correct key/KID/PSSH extraction
- [ ] Test: verify batch request for multiple tiers returns distinct keys per tier
