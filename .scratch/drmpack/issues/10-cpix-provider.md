# 10: Axinom KeyProvider

**What to build:** A KeyProvider implementation for Axinom's Key Service API based on Axinom's official documentation. Axinom uses a CPIX-based or JSON-based key acquisition API with Axinom communication keys and tenant tokens. Implements authentication, batch key requests for quality tiers, and parses Axinom's response into ContentKeys and PSSH data.

**Blocked by:** 08 (CPIX KeyProvider)

**Status:** ready-for-agent

- [ ] Research and verify Axinom Key Service current API specification
- [ ] Axinom authentication and token management
- [ ] Axinom KeyProvider request/response parsing
- [ ] Integration test with mock Axinom key service
