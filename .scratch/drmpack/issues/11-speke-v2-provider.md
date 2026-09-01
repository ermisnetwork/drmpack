# 11: SPEKE v2 KeyProvider

**What to build:** A KeyProvider implementation that wraps CPIX in the AWS SPEKE (Secure Packager and Encoder Key Exchange) v2 protocol. SPEKE v2 uses CPIX as its payload format but adds SPEKE-specific endpoint conventions, authentication (AWS SigV4 or API key), and request/response handling. Builds on the CPIX parsing infrastructure from ticket 10.

**Blocked by:** 10 (CPIX KeyProvider)

**Status:** ready-for-agent

- [ ] SPEKE v2 request construction: CPIX document wrapped per SPEKE v2 spec with required headers
- [ ] SPEKE v2 endpoint URL handling and authentication (API key or AWS SigV4 signing)
- [ ] Reuse CPIX response parser from ticket 10 for key extraction
- [ ] Behind `speke-v2` feature flag
- [ ] Test: mock SPEKE v2 endpoint, verify correct request format and key extraction from response
