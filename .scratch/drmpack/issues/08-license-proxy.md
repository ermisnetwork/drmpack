# 08: License Proxy Handlers

**What to build:** Async handler functions and a pooled `LicenseProxy` that `media-server` mounts on its HTTP routes to proxy client player license challenges to external DRM providers (Axinom Widevine, FairPlay, PlayReady). Each handler receives the player's raw challenge bytes, injects required authentication headers (`X-AxDRM-Message` JWT), forwards to the provider license server URL over a pooled connection, and returns a structured `LicenseResponse` (payload bytes, upstream Content-Type, and headers). Also supports in-memory caching and proxying of the Apple FairPlay Application Certificate.

**Blocked by:** 01 (Tracer), 06 (Axinom KeyProvider)

**Status:** done

- [x] `AxinomLicenseConfig` with builder, defaults, and `from_env()` loading endpoints (`AXINOM_WIDEVINE_LICENSE_URL`, `AXINOM_FAIRPLAY_LICENSE_URL`, `AXINOM_PLAYREADY_LICENSE_URL`, `AXINOM_FAIRPLAY_CERT_URL`)
- [x] `LicenseResponse` carrying `data: bytes::Bytes`, `content_type: Option<String>`, `headers: reqwest::header::HeaderMap`, helper `axdrm_message()`, and `Deref<Target = [u8]>`
- [x] `LicenseProxy` managing connection pooling (`reqwest::Client`) and in-memory FairPlay cert cache
- [x] `handle_widevine_license(proxy, challenge, auth_token) -> Result<LicenseResponse>`
- [x] `handle_fairplay_license(proxy, spc, auth_token) -> Result<LicenseResponse>`
- [x] `handle_playready_license(proxy, challenge, auth_token) -> Result<LicenseResponse>`
- [x] `handle_fairplay_certificate(proxy, cert_url) -> Result<bytes::Bytes>`
- [x] `DrmpackError::LicenseProxy` capturing HTTP status and diagnostic message from `X-AxDRM-ErrorMessage`
- [x] `license-proxy` feature flag in `Cargo.toml` included in `default`
- [x] Comprehensive unit and integration tests against mock HTTP DRM servers (replaying 200 success, 400/403 error headers, cert caching, and device tracking headers)

