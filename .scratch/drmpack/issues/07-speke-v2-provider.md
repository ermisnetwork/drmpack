# 07: AWS SPEKE v2 KeyProvider

**What to build:** A KeyProvider implementation for AWS SPEKE (Secure Packager and Encoder Key Exchange) v2 protocol. Wraps CPIX 2.3 request XML with mandatory `X-Speke-Version: 2.0` header and authentication (`x-api-key` header and optional AWS SigV4 request signing). Builds on top of the CPIX 2.3 engine from Ticket 05.

**Blocked by:** 05 (CPIX KeyProvider)

**Status:** done

- [x] SPEKE v2 request builder: wrap CPIX 2.3 request XML per AWS SPEKE v2 specification (`X-Speke-Version: 2.0`)
- [x] Authentication layer: support API key (`x-api-key`) and optional AWS SigV4 request signing
- [x] Integration test with mock SPEKE v2 endpoint
