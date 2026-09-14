# Enforce `seg_sync=auto` for Manifest-Driven Readiness Invariant

**Status: Accepted**

In `drmpack`, downstream consumers (specifically `ArtifactHarvester`) rely on the **Manifest-Driven Readiness** invariant: an encrypted media segment (`.m4s`) is considered complete and ready to be read from staging storage only after its filename appears in the active HLS playlist (`.m3u8`).

Previously, GPAC was spawned with `seg_sync=no` in `GpacProcessConfig::build_args()`. Under `seg_sync=no`, GPAC announces segments in manifests immediately upon boundary detection, before the final packets are flushed and written to storage. If a media segment contains multiple fragments (e.g. sub-fragments or CMAF chunks), an early inspection by `Harvester` through `is_complete_isobmff_media_segment` could detect a valid initial `moof`+`mdat` boundary and emit a truncated segment before trailing fragments are written.

We switch from `seg_sync=no` to `seg_sync=auto`. Because `drmpack` runs in `dual` mode generating HLS manifests (`.m3u8`), `seg_sync=auto` instructs GPAC to wait until the final packet of a segment is fully flushed by the multiplexer before announcing it in playlists.

## Considered options

- **`seg_sync=no` (Status quo)**: Announces segments immediately for lowest boundary signaling latency, but introduces a severe race condition where multi-fragment segments can be ingested by the harvester in an incomplete state.
- **`seg_sync=yes`**: Unconditionally forces synchronous packet flushing before manifest announcements for all manifest formats (both DASH and HLS).
- **`seg_sync=auto` (Chosen)**: Automatically enforces synchronous packet flushing whenever HLS playlists are active (which is always true in `dual` mode). Aligns GPAC emission timing exactly with `ArtifactHarvester`'s readiness invariant without adding redundant synchronization barriers for non-HLS pipelines.

## Implementation Reality

- **Verification in GPAC Process Arguments**: Confirmed in `src/gpac/process.rs:228`, the GPAC dasher filter arguments include `seg_sync=auto`.
- **Enforced Flushing in Dual Mode**: Because `drmpack` runs with `:dual` mode to produce synchronized HLS playlists and DASH manifests, `seg_sync=auto` guarantees that GPAC waits until the multiplexer has fully flushed all trailing fragments and media bytes to disk before announcing the segment in the manifest. Downstream consumers receiving segments via `ArtifactHarvester` are guaranteed to receive complete, untruncated media segments.
