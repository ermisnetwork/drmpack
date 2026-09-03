# 12: Audio & Subtitle Track Handling

**What to build:** Support multiple audio tracks (AAC, MP3, AC-3) and subtitle tracks (WebVTT) in the GPAC filter graph and DRM XML config. Audio tracks are encrypted with audio-specific ContentKeys (separate from video keys, per the QualityTier policy). Subtitle tracks (WebVTT) pass through unencrypted (cleartext). Manifests include correct language, codec, and accessibility attributes.

**Blocked by:** 05 (Multi-rendition)

**Status:** ready-for-agent

- [ ] Audio track DRM XML configuration (separate audio ContentKey and KID)
- [ ] Subtitle track handling (cleartext WebVTT signaling in HLS and DASH manifests)
- [ ] Multi-audio rendition manifest tags (`EXT-X-MEDIA:TYPE=AUDIO` in HLS, separate Audio AdaptationSet in DASH)
