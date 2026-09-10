# Learning Record 0006 — Data Flow End-to-End

**Date:** 2026-09-08
**Lesson:** [0003-data-flow-end-to-end](../lessons/0003-data-flow-end-to-end.html)

## Key Insight

drmpack is an **orchestrator**, not an encryptor. It fetches keys → generates XML config → spawns GPAC → harvests output. GPAC's `cecrypt` filter performs the actual encryption.

## Non-obvious Lessons

1. **Fan-out architecture**: Dual mode spawns 2 GPAC processes in parallel for CENC/CBCS, both consuming the same single media input stream.
2. **Manifest-Driven Readiness**: The Harvester does not read a segment merely upon seeing the file on disk — it only reads when the HLS manifest confirms the segment is complete. DASH MPD cannot be used for this purpose because `SegmentTemplate` is present from the beginning of the session.
3. **Ephemeral Staging**: Files are deleted immediately after being read into memory → conserving disk/ramdisk space.
4. **Init segment bypass**: The init segment is small and GPAC writes it atomically → no need to wait for manifest confirmation.

## Zone of Proximal Development

Topics to explore next:
- Details of the CPIX/SPEKE key exchange protocols
- GPAC filter chain configuration options
- ISO-BMFF box structures (`ftyp`, `moov`, `moof`, `mdat`)
