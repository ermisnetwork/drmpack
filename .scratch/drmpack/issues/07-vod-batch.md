# 07: VOD Batch Packaging API

**What to build:** A batch packaging API for VOD assets. `PackagingSession::package_file()` accepts a complete MP4 or MKV file path on disk, sets up GPAC with static manifest profiles (`profile=onDemand` for DASH and static HLS), generates the DRM XML config, runs GPAC to completion, and outputs complete static manifests (with `#EXT-X-ENDLIST` and final SegmentList/SegmentTimeline) into the target output directory.

**Blocked by:** 05 (Multi-rendition)

**Status:** ready-for-agent

- [ ] `package_file(file_path: &Path, output_dir: &Path)` API method
- [ ] GPAC command generation for static VOD packaging (non-live dasher profile)
- [ ] Multi-rendition VOD packaging support
- [ ] Verify static `.m3u8` and `.mpd` generation with full duration and complete segment list
