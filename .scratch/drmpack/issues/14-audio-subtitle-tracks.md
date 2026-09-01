# 14: Audio + subtitle track handling

**What to build:** Support audio and subtitle tracks alongside video. Audio tracks (AAC, MP3) are encrypted with per-track ContentKeys (audio key separate from video key, as per the per-quality-tier key strategy). Subtitle tracks (WebVTT) are included in manifests as cleartext (not encrypted). Manifests include correct track metadata: codec, language, and accessibility attributes.

**Blocked by:** 05 (Multi-rendition)

**Status:** ready-for-agent

- [ ] Audio segment encryption: parse audio-only fMP4 segments, encrypt with audio-specific ContentKey
- [ ] AAC codec support: correct sample entry (mp4a) in init segment, proper sample table handling
- [ ] MP3 codec support: correct sample entry in init segment
- [ ] Audio tracks use a separate ContentKey from video (keyed by track_type=audio + QualityTier)
- [ ] WebVTT subtitle tracks: included in manifests as cleartext (no encryption)
- [ ] HLS: EXT-X-MEDIA tags for audio and subtitle tracks with correct TYPE, GROUP-ID, LANGUAGE
- [ ] DASH: separate AdaptationSet for audio and text, with correct contentType, mimeType, codecs
- [ ] Test: session with video + audio + subtitle, verify audio encrypted with different key than video, subtitle cleartext, manifests reference all tracks correctly
