# 02: DASH manifest generation

**What to build:** Add MPD (DASH) manifest output alongside the existing HLS manifest. The MPD should include ContentProtection elements for Widevine and PlayReady with correct PSSH boxes, period and adaptation set structure, and segment URLs with the CDN base URL. When a session produces output, both m3u8 and MPD manifests are emitted via OutputSink.

**Blocked by:** 01 (Tracer)

**Status:** ready-for-agent

- [ ] MPD manifest generator: valid DASH MPD with Period, AdaptationSet, Representation, SegmentTemplate/SegmentList
- [ ] ContentProtection elements for Widevine (scheme ID `edef8ba9-79d6-4ace-a3c8-27dcd51d21ed`) and PlayReady (scheme ID `9a04f079-9840-4286-ab92-e65be0885f95`)
- [ ] PSSH boxes encoded as base64 in ContentProtection elements
- [ ] CDN base URL in segment URLs (BaseURL or SegmentTemplate)
- [ ] PackagingSession emits both m3u8 and MPD via OutputSink
- [ ] Test: push segment, verify valid MPD with correct ContentProtection elements and segment URLs
