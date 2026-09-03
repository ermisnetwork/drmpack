# 09: SPEKE v2 KeyProvider

**What to build:** A KeyProvider implementation for AWS SPEKE (Secure Packager and Encoder Key Exchange) v2 protocol. SPEKE v2 wraps CPIX 2.3 as its payload format but adds SPEKE-specific endpoint URL conventions, AWS SigV4 authentication, and custom HTTP headers. Builds on top of the CPIX request/response parser from ticket 08.

**Blocked by:** 08 (CPIX KeyProvider)

**Status:** ready-for-agent

- [ ] SPEKE v2 request builder: wrap CPIX request XML per AWS SPEKE v2 specification
- [ ] Authentication layer: support API key and AWS SigV4 request signing
- [ ] Integration test with mock SPEKE v2 endpoint
