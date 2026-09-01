# Per-track and per-quality-tier encryption keys

Each combination of track type (video, audio) and QualityTier (SD, HD, 4K) gets its own ContentKey. This is the most granular key strategy — more complex than single-key or per-track-only — chosen to enable per-tier access policies (e.g. SD free, HD paid, 4K premium). Keys are requested in a single batch CPIX call and cached for the PackagingSession's lifetime. The trade-off is increased key management complexity in both drmpack (mapping renditions to tiers to keys) and the license server (issuing subset licenses per tier).

## Considered options

- **Single key**: simplest, one key for all tracks and qualities. No per-tier policies possible.
- **Per-track only**: separate video/audio keys. Allows audio-free policies but not quality-based differentiation.
