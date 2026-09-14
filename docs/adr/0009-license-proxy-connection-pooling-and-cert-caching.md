# License Proxy with Connection Pooling and FairPlay Certificate Caching

**Status: Accepted (Amended by ADR-0018)**

media-server mounts HTTP routes to proxy client player license challenges to external DRM providers (Axinom Widevine, FairPlay, PlayReady). We implement an in-process License Proxy module (`drmpack::license`) featuring connection pooling and selective in-memory caching.

DRM license responses (Widevine license, FairPlay CKC, PlayReady license) are never cached: each client CDM generates a unique cryptographic challenge containing session-bound nonces, making cached responses cryptographically invalid for other requests. In contrast, Apple FairPlay Application Certificates are static across sessions and cached in memory to eliminate redundant origin round-trips. Upstream connections reuse a pooled `reqwest::Client` to avoid TCP and TLS handshake latency during player startup.

Proxy handlers return a structured `LicenseResponse` carrying payload bytes, Content-Type, and upstream headers (surfacing Axinom's `X-AxDRM-Message` response header for device identification). Errors are mapped to `DrmpackError::LicenseProxy` with status code and diagnostic message from `X-AxDRM-ErrorMessage`.

## Considered options

- **Stateless functions instantiating `reqwest::Client` per request**: rejected because repeated TLS handshakes add substantial latency to stream playback startup.
- **Caching DRM license responses**: rejected because licenses are cryptographically bound to single-use CDM session nonces and cannot be reused across players or sessions.
- **Returning flat `bytes::Bytes`**: rejected because it discards upstream metadata, particularly Axinom's `X-AxDRM-Message` device tracking header and Content-Type negotiation.

## Implementation Reality / Amendments

1. **Vendor-Agnostic Core Configuration**: The `LicenseProxyConfig` struct is strictly vendor-neutral. It encapsulates generic endpoint URLs (`widevine_license_url`, `fairplay_license_url`, `playready_license_url`, and optional `fairplay_cert_url`), connection timeouts, and headers without coupling the proxy to Axinom or any specific DRM vendor. Vendor-specific configurations (such as `AxinomLicenseConfig`) implement `Into<LicenseProxyConfig>` to bridge vendor credentials and tenant endpoints into the proxy engine.
2. **Purge of Static Defaults and `Default` Trait (ADR-0018)**: Following ADR-0018, all hardcoded fallback URLs (`DEFAULT_AXINOM_*`) and `impl Default` implementations for `LicenseProxyConfig` and `LicenseProxy` have been eliminated. Callers are strictly required to provide explicit, tenant-isolated endpoints during initialization (`LicenseProxyConfig::new(...)` or `from_env()`). This enforces fail-fast startup behavior, preventing silent 401/404 DNS errors or accidental transmission of license requests to deprecated shared staging endpoints.

