# DRM Playback Metadata Handoff and Credential Security

**Status: Accepted**

Playback authorization backends issuing client DRM licenses (e.g. Axinom Entitlement JWTs) require stream DRM signaling metadata (KIDs, Encryption Schemes, and FairPlay IVs) without being colocated with the packaging process or having access to packager local filesystems.

We provide `DrmStreamMetadata` as a first-class, serializable data transfer object produced via `PackagingSession::playback_metadata()`. Packaging applications serialize this metadata into application-level persistence stores (PostgreSQL, Redis) at session initialization. Playback backends deserialize `DrmStreamMetadata` to mint entitlement tokens on demand, completely decoupled from media distribution.

We chose explicit metadata hand-off over runtime manifest inspection (scanning local disk or fetching CDN manifests via HTTP to parse MPD/M3U8) and key re-acquisition (querying key providers twice). Manifest scanning introduces network roundtrips, XML parsing overhead, and filesystem coupling to storage layouts. Re-querying key providers risks key ID desynchronization and adds external API dependencies to latency-critical playback authorization paths.

Furthermore, `DrmStreamMetadata` strictly excludes raw AES keys (`key: [u8; 16]`), ensuring that business-layer playback services cannot inadvertently leak encryption material. To guard communication secrets, `AxinomSigningConfig` provides type-safe environment loading (`from_env()`) with redacted `Debug` formatting and direct JWT generation over `DrmStreamMetadata`.

## Implementation Reality

- **Zero Secret Key Exposure**: As implemented in `src/session/metadata.rs`, `DrmStreamMetadata` and `DrmKeyEntry` strictly omit raw AES encryption keys (`key: [u8; 16]`). They only encapsulate public signaling identifiers (`kid`, `scheme`, `track_type`, `quality_tier`, and derived FairPlay `iv`), guaranteeing that serializing metadata to application persistence stores (PostgreSQL, Redis) or querying it from playback microservices carries zero cryptographic secret leakage risk.
- **Conditional Compilation for Axinom Integration**: Axinom JWT entitlement token generation functions (such as `DrmStreamMetadata::generate_axinom_jwt` and `DrmStreamMetadata::to_axinom_key_configs`) are gated under `#[cfg(feature = "axinom")]`. This ensures core packaging and metadata serialization remain lean and free of unnecessary JWT/cryptographic vendor dependencies when alternative key providers are used.
