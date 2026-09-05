# Standardize on Dual CMAF over Legacy MPEG-2 TS SAMPLE-AES

**Status: Accepted**

`drmpack` standardizes exclusively on fragmented MP4 (CMAF) containers for both HLS and DASH delivery, declining support for legacy MPEG-2 Transport Stream (TS) segmentation with SAMPLE-AES encryption (Ticket 13 marked as `WONTFIX`).

## Context & Industry Landscape

Ticket 13 originally proposed an optional legacy output profile generating MPEG-2 TS segments with SAMPLE-AES encryption for older Apple and Smart TV clients. A comprehensive survey of Apple authoring specifications, DRM ecosystem standards, and hardware fleets reveals:

1. **Apple Ecosystem**: Apple introduced CMAF (fMP4) with `cbcs` pattern encryption in iOS 10 and macOS Sierra (2016). In current production environments, iOS 15+ accounts for >99.9% of active Apple devices. The only devices requiring TS SAMPLE-AES are iOS 9 and older (iPhone 4s/5, released before 2013), representing a negligible legacy footprint.
2. **Smart TV Fleets**: Smart TVs (Samsung Tizen, LG webOS, Android TV) do not use Apple FairPlay; they rely on Microsoft PlayReady and Google Widevine. Older Smart TVs that lack `cbcs` support play content via `cenc` (AES-CTR) over DASH/fMP4. `drmpack` already provides native `cenc` representations through its Dual Packaging pipeline (ADR-0006), fully covering this device tier without TS.
3. **Operational Overhead**: Generating TS and CMAF simultaneously during live streaming doubles CPU encoding/packaging load, doubles pipe I/O throughput, and doubles Ramdisk memory consumption.

## Considered options

- **Dual CMAF + TS SAMPLE-AES in Live Session**: rejected because running concurrent TS segmentation doubles compute and memory consumption for obsolete clients.
- **Support TS via separate offline packaging profile**: deferred; if legacy telecom Set-Top Boxes (STBs) require TS in the future, it should exist as a standalone offline tool rather than burdening the live low-latency pipeline.
- **Standardize on Dual CMAF (fMP4 CENC + CBCS)**: accepted; provides 100% device reach across modern Apple, Android, Web, and Smart TV fleets with zero legacy debt.

## Consequences

- Ticket 13 is closed as `WONTFIX` and the `hls-legacy` feature flag is removed from `Cargo.toml`.
- All FairPlay HLS manifests use CMAF fMP4 segments with `#EXT-X-MAP` and `METHOD=SAMPLE-AES` (signaling `cbcs` mode under ISO/IEC 23001-7).
- Media-server maintains a single, modern CMAF packaging architecture.
