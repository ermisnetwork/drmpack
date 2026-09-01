# 06: Live streaming session

**What to build:** The live workflow for PackagingSession. Media-server pushes segments continuously via push_segment(). drmpack maintains manifest state internally (sliding window of recent segments). Each push triggers an updated manifest emitted via OutputSink. Session ends via explicit close() (which finalizes manifests with EXT-X-ENDLIST for HLS) or via a configurable timeout when no segments arrive. Segment processing errors are returned as Result to the caller.

**Blocked by:** 05 (Multi-rendition)

**Status:** ready-for-agent

- [ ] push_segment() accepts segments one at a time, encrypts, emits via OutputSink, updates internal manifest state
- [ ] Sliding window: manifest contains only the most recent N segments (configurable window size)
- [ ] Manifest re-emitted via OutputSink after each segment push
- [ ] session.close() finalizes: HLS adds EXT-X-ENDLIST, DASH sets minimumUpdatePeriod to 0
- [ ] Configurable timeout: if no segment arrives within the duration, session auto-closes and frees resources
- [ ] Segment processing errors returned as Result — caller decides skip/retry/kill
- [ ] Test: push 10 segments, verify sliding window manifest (only last N present), close and verify EXT-X-ENDLIST
- [ ] Test: verify timeout triggers auto-close after configured duration with no input
- [ ] Test: push invalid segment data, verify Result::Err returned without crashing the session
