# License Proxy with Connection Pooling and FairPlay Certificate Caching

media-server mounts HTTP routes to proxy client player license challenges to external DRM providers (Axinom Widevine, FairPlay, PlayReady). We implement an in-process License Proxy module (`drmpack::license`) featuring connection pooling and selective in-memory caching.

DRM license responses (Widevine license, FairPlay CKC, PlayReady license) are never cached: each client CDM generates a unique cryptographic challenge containing session-bound nonces, making cached responses cryptographically invalid for other requests. In contrast, Apple FairPlay Application Certificates are static across sessions and cached in memory to eliminate redundant origin round-trips. Upstream connections reuse a pooled `reqwest::Client` to avoid TCP and TLS handshake latency during player startup.

Proxy handlers return a structured `LicenseResponse` carrying payload bytes, Content-Type, and upstream headers (surfacing Axinom's `X-AxDRM-Message` response header for device identification). Errors are mapped to `DrmpackError::LicenseProxy` with status code and diagnostic message from `X-AxDRM-ErrorMessage`.

## Considered options

- **Stateless functions instantiating `reqwest::Client` per request**: rejected because repeated TLS handshakes add substantial latency to stream playback startup.
- **Caching DRM license responses**: rejected because licenses are cryptographically bound to single-use CDM session nonces and cannot be reused across players or sessions.
- **Returning flat `bytes::Bytes`**: rejected because it discards upstream metadata, particularly Axinom's `X-AxDRM-Message` device tracking header and Content-Type negotiation.
