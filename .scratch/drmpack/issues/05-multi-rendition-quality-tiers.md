# 05: Multi-rendition + per-quality-tier keys

**What to build:** Support multiple Renditions in a single PackagingSession, each mapped to a QualityTier (SD/HD/4K). Each (track_type, QualityTier) combination gets a unique ContentKey. The session's key request fetches all keys in a single batch call. Manifests include ABR structure: HLS master playlist with variant playlists per rendition, DASH MPD with multiple Representations per AdaptationSet. Each variant/representation signals its tier's key.

**Blocked by:** 01 (Tracer), 02 (DASH manifest)

**Status:** ready-for-agent

- [ ] PackagingSession config accepts a list of Renditions, each with resolution, bitrate, codec, and QualityTier assignment
- [ ] KeyProvider.fetch_keys() requests keys for all (track_type, QualityTier) combinations in one batch call
- [ ] Each rendition's segments are encrypted with the ContentKey matching its QualityTier
- [ ] HLS: master playlist with EXT-X-STREAM-INF per rendition, each pointing to a variant playlist with the correct EXT-X-KEY for its tier's key
- [ ] DASH: MPD with multiple Representations per AdaptationSet, each Representation's ContentProtection referencing the correct KID
- [ ] Test: session with 3 renditions across 2 tiers, verify different tiers use different keys, same tier uses same key, ABR manifests are structurally correct
