# 12: Axinom KeyProvider

**What to build:** A KeyProvider implementation for Axinom's DRM key service, based on Axinom's official API documentation. Axinom may use CPIX or their own key service API. The implementation must follow Axinom's documented authentication, request format, and response handling precisely. Research the current Axinom key service API documentation before implementing.

**Blocked by:** 10 (CPIX KeyProvider)

**Status:** ready-for-agent

- [ ] Research Axinom key service API: read official documentation for current endpoints, auth, request/response format
- [ ] Implement AxinomProvider following the documented API contract exactly
- [ ] Axinom-specific authentication (API key, tenant ID, or other credentials per their docs)
- [ ] Multi-key support per Axinom's API capabilities
- [ ] Behind `axinom` feature flag
- [ ] Test: mock Axinom endpoint matching their documented response format, verify correct key extraction
