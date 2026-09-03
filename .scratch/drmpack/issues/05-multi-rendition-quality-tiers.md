# 05: Multi-rendition + per-quality-tier keys

**What to build:** Support multiple Renditions in a single `PackagingSession`, each mapped to a `QualityTier` (SD/HD/4K). Each (track_type, QualityTier) combination gets a unique `ContentKey`. The session's key request fetches all keys in a single batch call. The GPAC DRM XML generator assigns the correct key and KID to each track. Manifests include multi-bitrate ABR structure (HLS master playlist with variant playlists, DASH MPD with multiple Representations per AdaptationSet).

**Blocked by:** 01 (Tracer), 03 (Multi-DRM XML)

**Status:** ready-for-agent

- [ ] `PackagingSessionConfig` accepts a list of `Renditions`, each with resolution, bitrate, codec, and `QualityTier`
- [ ] `KeyProvider::fetch_keys()` requests keys for all (track_type, QualityTier) combinations in one batch call
- [ ] GPAC DRM XML maps each track ID to its tier's `ContentKey` and `KeyID`
- [ ] Multi-rendition HLS master manifest generated with correct bandwidth and codec tags
- [ ] Multi-Representation DASH MPD generated with correct adaptation sets
