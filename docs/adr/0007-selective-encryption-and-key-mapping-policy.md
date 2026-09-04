# Selective encryption and key mapping policy in PackagingSession

**Status: Accepted**

`drmpack` supports Selective Encryption (encrypting a subset of Renditions while passing others through unencrypted) and configurable `KeyMappingPolicy` with `SharedAll` as the default.

## Context

Streaming platforms have diverse encryption requirements:
1. **Device compatibility**: Legacy Smart TVs, older Chromecast models, and AV receivers using HDMI ARC/eARC often fail when audio streams are DRM-encrypted, requiring unencrypted (clear) audio.
2. **Business models**: Free preview tiers (e.g. unencrypted SD video) allow zero-latency startup without license acquisition, while premium tiers (HD/4K) require DRM. Subtitle tracks (WebVTT) are always unencrypted.
3. **Operational simplicity**: While ADR-0003 established `PerTierAndTrack` for granular Hollywood/MovieLabs compliance, many live/OTT operations prefer a single shared key across all tracks (`SharedAll`) or separate video and audio keys (`SharedVideoSingleAudio`).

## Decision

1. **Selective Encryption**: `Rendition` includes `pub encrypted: bool` (defaulting to `true`). Unencrypted Renditions bypass GPAC's `cecrypt` filter and emit no `#EXT-X-KEY` in HLS or `<ContentProtection>` in DASH manifests. If all Renditions in a session are clear, key acquisition from `KeyProvider` is bypassed entirely.
2. **KeyMappingPolicy**: `PackagingSessionConfig` includes `key_mapping_policy: KeyMappingPolicy` with three modes:
   - `SharedAll` (**Default**): One ContentKey shared across all encrypted Renditions.
   - `SharedVideoSingleAudio`: One ContentKey for all video Renditions, and one ContentKey for all audio Renditions.
   - `PerTierAndTrack`: Granular ContentKey per (TrackType, QualityTier) combination per ADR-0003.

## Considered options

- **Mandatory 100% encryption**: Forces all renditions to have keys. Fails on unencrypted audio/SD and requires dummy key management for non-premium tracks.
- **PerTierAndTrack default**: Retains ADR-0003 as default. Rejected as default because single-key `SharedAll` is significantly simpler for common live streaming while still available via policy selection.
- **Selective encryption via QualityTier**: Overloading `QualityTier` with a `Clear` variant. Rejected because quality tier represents media resolution/bitrate properties, not cryptographic protection state.

## Consequences

- GPAC XML generator omits clear tracks or marks them `IsEncrypted="0"`, eliminating spurious "No ContentKey found" errors when clear tracks are present.
- Manifest generators must strictly omit DRM signaling tags (`#EXT-X-KEY`, `<ContentProtection>`) from playlists/adaptation sets of clear Renditions to prevent player stalls.
