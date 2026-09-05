# 10: Audio & Subtitle Track Handling

**What to build:** Support multiple audio tracks (AAC, MP3, AC-3) and subtitle tracks (WebVTT) in the GPAC filter graph and DRM XML config. Audio tracks are encrypted with audio-specific ContentKeys (separate from video keys, per the QualityTier policy). Subtitle tracks (WebVTT) pass through unencrypted (cleartext). Manifests include correct language, codec, and accessibility attributes.

**Blocked by:** 09 (Multi-rendition)

**Status:** consolidated into 09 (Multi-Track ABR Packaging Engine)

- [x] Consolidated into Ticket 09 to share the same multiplexed fMP4 pipe data plane and test harness.
- [x] Clear subtitle track handling (`<CrypTrack IsEncrypted="0"/>`) already implemented in ADR-0007.
- [x] Audio track DRM XML and key mapping policy already implemented in ADR-0007 (`KeyMappingPolicy::SharedVideoSingleAudio`).
- [x] Remaining delivery items merged directly into Ticket 09 checklist.

