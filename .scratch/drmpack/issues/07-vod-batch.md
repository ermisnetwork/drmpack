# 07: VOD batch API

**What to build:** A batch API for VOD content. package_file() accepts either a complete MP4/MKV file path or a list of pre-segmented content. It processes everything in one call: fragments the file (if needed), encrypts all segments, generates final manifests, and emits all output via OutputSink. The manifests are complete (not live/updating) with all segments listed.

**Blocked by:** 05 (Multi-rendition)

**Status:** ready-for-agent

- [ ] package_file() accepts a file path, reads the file, fragments into segments, encrypts, generates manifests
- [ ] Alternative input: accept pre-segmented content (list of segment buffers) for pipelines that already produce segments
- [ ] Manifests are finalized (HLS has EXT-X-ENDLIST, DASH has static type)
- [ ] All segments and manifests emitted via OutputSink
- [ ] Test: package a complete MP4 file, verify all segments encrypted + manifests complete with correct segment count
- [ ] Test: package pre-segmented input, verify equivalent output
