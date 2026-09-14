# Per-track and per-quality-tier encryption keys

**Status: Amended by ADR-0007**

Each combination of track type (video, audio) and QualityTier (SD, HD, 4K, AUDIO) gets its own ContentKey. This is the most granular key strategy — more complex than single-key or per-track-only — chosen to enable per-tier access policies (e.g. SD free, HD paid, 4K premium). Keys are requested in a single batch CPIX call and cached for the PackagingSession's lifetime. The trade-off is increased key management complexity in both drmpack (mapping renditions to tiers to keys) and the license server (issuing subset licenses per tier).

## Considered options

- **Single key**: simplest, one key for all tracks and qualities. No per-tier policies possible.
- **Per-track only**: separate video/audio keys. Allows audio-free policies but not quality-based differentiation.

## Implementation Reality / Amendments

The granular per-tier keying strategy established in ADR-0003 is no longer the sole or default mechanism:
- **Default Policy Normalized to SharedAll**: [ADR-0007](./0007-selective-encryption-and-key-mapping-policy.md) standardized `KeyMappingPolicy::SharedAll` (a single ContentKey across all encrypted renditions) as the default in `PackagingSessionConfig::new` to simplify OTT workflows and eliminate unnecessary license server roundtrips.
- **Granular Keying as Configurable Option**: `KeyMappingPolicy::PerTierAndTrack` remains fully supported as an opt-in policy for workflows requiring Hollywood/MovieLabs studio compliance, alongside `KeyMappingPolicy::SharedVideoSingleAudio`.
- **Selective Encryption**: ADR-0007 also introduced Selective Encryption (`Rendition::encrypted: bool`), allowing individual renditions (such as clear audio for legacy AV receivers or SD preview video) to bypass DRM encryption entirely regardless of the selected key mapping policy.
