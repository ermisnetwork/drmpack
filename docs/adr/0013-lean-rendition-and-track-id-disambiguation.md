# Lean Rendition, Track ID Disambiguation, and Direct Byte Ingestion

**Status: Accepted**

`drmpack` sharpens track terminology by distinguishing the logical `track_id` string from the physical `container_track_id` integer, purges phantom metadata fields (`label`, `language`, `resolution`, `bitrate`, `codecs`) from `Rendition`, and deletes the synthetic `Segment` struct in favor of direct streaming byte ingestion.

## Context & Problem Statement

Prior versions had subtle ambiguities and lingering ceremony:
1. **Track ID Ambiguity**: The term "track id" was overloaded between the session-scoped logical identifier (`rendition.id`, format `track_{type}_{uuid}`) and the ISO-BMFF container elementary stream integer (`rendition.track_id: Option<u32>`). This caused confusion in method naming (`with_track_id` vs `with_container_track_id`).
2. **Phantom Metadata in Rendition**: `Rendition` carried optional `label` and `language` fields alongside deleted resolution/bitrate fields. However, GPAC automatically inspects container metadata (`moov`/`stsd`/`mdhd`) directly from the fMP4 stream, rendering manual metadata configuration in `Rendition` redundant and prone to configuration drift.
3. **Synthetic Ingestion Struct**: The synthetic `Segment` struct required callers to construct synthetic parameters (`sequence_number`, `duration_seconds`, `rendition_id`, `is_init`) even though GPAC demuxes continuous byte streams from anonymous pipes and calculates timings natively.

## Decisions

1. **Explicit Terminology**:
   - `rendition.track_id: String`: The unique logical string identifier (`track_{type}_{uuid}`).
   - `rendition.container_track_id: Option<u32>`: The 1-based ISO-BMFF track integer (`u32 >= 1`), falling back to 1-based declaration index via `effective_container_track_id(index)`.
2. **Lean Rendition**:
   - Purged `label` and `language` fields and builder methods.
   - Clean constructors: `Rendition::video(tier)`, `video_hd()`, `video_4k()`, `audio()`, `audio_tier(tier)`, `subtitle()`.
   - Modifiers: `.with_container_track_id(u32)`, `.clear()`, `.with_encrypted(bool)`.
3. **Direct Byte Ingestion**:
   - Deleted `Segment` struct and `session.push_segment(...)`. All ingestion uses `session.push(bytes: impl Into<Bytes>)`.
