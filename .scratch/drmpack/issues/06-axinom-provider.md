# 06: Axinom KeyProvider

**What to build:** A KeyProvider implementation for Axinom's Key Service API based on Axinom's official documentation (SPEKE v2 over CPIX 2.3 at `https://key-server-management.axprod.net/api/SpekeV2`). Implements HTTP Basic Auth (`TenantID` + `ManagementKey`), sends CPIX 2.3 request payload, supports `overrideKeyIds=true` parameter, and parses Axinom's response into scheme-aware `KeySet`.

**Blocked by:** 05 (CPIX KeyProvider)

**Status:** done

- [x] Axinom authentication: HTTP Basic Auth with Tenant ID and Key Service Management Key
- [x] Axinom request configuration (`overrideKeyIds` query param, SPEKE v2 headers)
- [x] Axinom KeyProvider request/response parsing building upon CPIX 2.3 engine
- [x] Integration tests against mock HTTP server replay
