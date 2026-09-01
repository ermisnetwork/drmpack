Status: ready-for-agent

# drmpack — Native Rust DRM Packaging Library

## Problem Statement

The ermis-stream media-server needs to deliver DRM-protected content for both live and VOD workflows. The existing solution wraps Shaka Packager as an external binary, which introduces process management overhead, constrains the API to a CLI interface, and prevents streaming output or per-quality-tier key policies. Media-server needs an in-process Rust library that encrypts segments, generates manifests, and proxies license requests — with zero network hop on the hot path.

## Solution

drmpack is a Rust library that media-server imports as a crate dependency. It provides:

- A **PackagingSession** abstraction that accepts media segments (muxed or unmuxed), encrypts them with configurable DRM (Widevine, FairPlay, PlayReady), and emits encrypted segments + complete manifests (m3u8/MPD) through an **OutputSink** trait.
- A pluggable **KeyProvider** trait for fetching encryption keys from DRM providers (Axinom, CPIX, SPEKE v2) or using raw keys for testing.
- Async **license proxy handler functions** that media-server mounts on its HTTP routes, forwarding player license requests to the DRM provider.

## User Stories

1. As a media-server developer, I want to create a PackagingSession with rendition declarations and DRM config, so that I can start packaging content with a single API call.
2. As a media-server developer, I want to push muxed fMP4 segments into a session, so that drmpack encrypts them without me needing to handle encryption details.
3. As a media-server developer, I want to push unmuxed raw NALUs (H.264/H.265) into a session, so that drmpack muxes them into fMP4 and encrypts them in one step.
4. As a media-server developer, I want to receive encrypted output through an OutputSink trait I implement, so that I control where output goes (memory, disk, or direct CDN upload).
5. As a media-server developer, I want drmpack to generate complete HLS manifests (m3u8) and DASH manifests (MPD) with correct DRM signaling, so that I don't have to manually construct PSSH boxes or EXT-X-KEY tags.
6. As a media-server developer, I want separate manifests per DRM/encryption-scheme combination (e.g. `master_fairplay.m3u8`, `master_widevine.m3u8`, `manifest.mpd`), so that I can serve the correct manifest based on client capability.
7. As a media-server developer, I want to configure encryption mode per-session (CENC only, CBCS only, or both), so that I can balance device compatibility against storage costs.
8. As a media-server developer, I want to declare QualityTiers (SD/HD/4K) and map renditions to them at session creation, so that each tier gets its own ContentKey for per-tier access policies.
9. As a media-server developer, I want drmpack to fetch all keys for all tiers in a single batch CPIX request, so that key acquisition adds minimal latency.
10. As a media-server developer, I want keys cached for the PackagingSession's lifetime, so that drmpack doesn't re-fetch keys for every segment.
11. As a media-server developer, I want to use a RawKeyProvider for testing and development, so that I can run the full pipeline without a real DRM provider.
12. As a media-server developer, I want to implement custom KeyProvider adapters by implementing a trait, so that I can integrate with any DRM provider.
13. As a media-server developer, I want async license proxy handler functions, so that I can mount them on my axum routes and forward player license requests to the DRM provider.
14. As a media-server developer, I want to use the streaming API for live packaging, pushing segments as they arrive and having drmpack maintain manifest state (sliding window), so that live latency stays minimal.
15. As a media-server developer, I want to explicitly close a live PackagingSession, so that drmpack finalizes the manifest (e.g. adds `#EXT-X-ENDLIST`).
16. As a media-server developer, I want a timeout fallback that auto-closes sessions when no segments arrive for a configurable duration, so that crashed streams don't leak resources.
17. As a media-server developer, I want drmpack to return `Result` errors for segment processing failures (not silently skip or retry), so that I decide the error policy in my application.
18. As a media-server developer, I want to use the batch API for VOD, giving drmpack a complete file or pre-segmented content, so that the entire asset is packaged in one call.
19. As a media-server developer, I want to provide a CDN base URL in the session config, so that manifests contain correct absolute URLs for segments.
20. As a media-server developer, I want to enable/disable features via Cargo feature flags (e.g. `fairplay`, `widevine`, `playready`, `hls-legacy`), so that I only compile what I need.
21. As a media-server developer, I want drmpack to emit structured logs via the `tracing` crate, so that logs integrate with my existing tracing subscriber without extra setup.
22. As a media-server developer, I want drmpack to handle init segment generation for unmuxed input (from SPS/PPS/VPS), so that I don't need to construct moov boxes myself.
23. As a media-server developer, I want drmpack to accept existing init segments for muxed input and only add encryption metadata (PSSH), so that the original codec config is preserved.
24. As a media-server developer, I want subtitles (WebVTT) included in manifests as cleartext tracks (not encrypted), so that players can render them without a license.
25. As a media-server developer, I want CMAF as the default packaging format (fMP4 segments usable by both HLS and DASH), so that I store one set of segments with dual manifests.
26. As a media-server developer, I want optional HLS-TS legacy output (behind a feature flag), so that I can serve older devices that don't support fMP4-based HLS.
27. As a media-server developer, I want the PackagingSession to manage multi-bitrate ABR manifests (master playlist + variant playlists for HLS, multi-AdaptationSet MPD for DASH), so that adaptive streaming works out of the box.

## Implementation Decisions

### Architecture

- **In-process Rust library** (crate), not a standalone service. Shares media-server's tokio async runtime. See ADR-0001.
- **Native Rust implementation** — no external binary dependency (Shaka Packager or otherwise). See ADR-0002.
- **Single crate with Cargo feature flags**: `widevine`, `fairplay`, `playready`, `hls-legacy`, `cpix`, `speke-v2`, `axinom`. Default features enable all three DRM systems and CPIX.

### Core abstraction: PackagingSession

- Created by media-server with a config struct containing: content ID, rendition list (resolution, bitrate, codec, QualityTier mapping), encryption schemes (CENC/CBCS/both), DRM system selection, KeyProvider, CDN base URL, and session timeout.
- Maintains internal state: manifest data, sliding window for live, key cache.
- Exposes methods: `push_segment()` (streaming), `package_file()` (batch VOD), `close()`.
- Session is `Send` but not necessarily `Sync` — owned by one task, not shared.

### Input pipeline

- **Muxed input** (fMP4): parse init segment, extract codec config, encrypt media samples in each moof/mdat, add PSSH to init segment.
- **Unmuxed input** (raw NALUs): parse codec parameter sets (SPS/PPS for H.264, VPS/SPS/PPS for H.265), generate init segment (moov box), mux NALUs into fMP4 fragments, then encrypt.
- **Streaming API**: `push_segment()` accepts one segment at a time, returns immediately. Output delivered asynchronously via OutputSink.
- **Batch API**: `package_file()` accepts a file path or pre-segmented content, processes everything, calls OutputSink for each output artifact.

### Encryption

- **CENC** (AES-128-CTR): for Widevine and PlayReady.
- **CBCS** (AES-128-CBC with pattern encryption): for FairPlay (required) and modern Widevine/PlayReady devices.
- **Configurable per-session**: media-server chooses CENC only, CBCS only, or dual. Dual produces two segment sets.
- **Per-track + per-quality-tier keys**: each (track_type, QualityTier) pair gets a unique ContentKey. See ADR-0003.
- Subtitles: always cleartext, not encrypted.

### Key management

- **KeyProvider trait**: `async fn fetch_keys(content_id, key_request) -> Result<KeySet>`. KeyRequest describes needed keys (track types × quality tiers). KeySet contains the ContentKeys, KIDs, and PSSH data.
- **Built-in implementations**: `CpixProvider` (generic CPIX over HTTP POST), `SpekeV2Provider` (AWS SPEKE v2), `AxinomProvider` (Axinom-specific), `RawKeyProvider` (manual keys for testing).
- Each provider implementation must follow the provider's official API documentation.
- **Batch key request**: one call fetches all keys for all tiers. CPIX natively supports multi-key responses.
- **Per-session cache**: keys fetched once at session creation, reused for all segments in that session.

### Output

- **OutputSink trait**: `async fn write_segment(path, data)`, `async fn write_init_segment(path, data)`, `async fn write_manifest(path, data)`. Media-server implements this to control output destination.
- **Static dispatch** (generics) on the hot path for zero-cost.
- **Manifests**: drmpack generates complete, ready-to-serve manifests. For live, manifests are re-emitted via OutputSink each time the sliding window updates.
- **Separate manifests per DRM**: e.g. `master_fairplay.m3u8` (CBCS), `master_widevine.m3u8` (CENC), `manifest.mpd` (CENC with Widevine + PlayReady ContentProtection elements).
- **CDN base URL**: injected into all segment URLs in manifests.

### Player license proxy

- Async handler functions: `async fn handle_widevine_license(req, provider_config) -> Response`, similarly for FairPlay and PlayReady.
- Media-server mounts these on its HTTP routes. Auth/validation is media-server's responsibility (middleware).
- Handler proxies the player's license challenge to the DRM provider and returns the license response.

### Live session lifecycle

- **Explicit close**: `session.close()` finalizes manifests (adds `#EXT-X-ENDLIST` for HLS).
- **Timeout fallback**: configurable duration; if no segment arrives within the timeout, session auto-closes and resources are freed.
- **Error handling**: segment processing errors returned as `Result` — media-server decides whether to skip, retry, or kill the stream.

### Codec support (phased)

- **MVP**: H.264 (AVC), H.265 (HEVC), AAC, MP3, WebVTT
- **Phase 2**: VP9, AV1, Opus, AC-3, E-AC-3, TTML
- **Phase 3**: VP8, Vorbis, AC-4, DTS, FLAC, MPEG-H Audio, DVB-SUB

## Testing Decisions

### Test seam

One seam: **PackagingSession public API**. All tests go through: create session → push segments → assert output via OutputSink.

### Test doubles (built into the design)

- **RawKeyProvider**: supplies known keys without calling any external service.
- **MemoryOutputSink**: collects all output (segments, init segments, manifests) in memory for assertions.

### What makes a good test

- Test **external behavior** through the PackagingSession API, not internal module implementation.
- Assert on **output correctness**: encrypted segment bytes are valid fMP4, manifests are well-formed, DRM signaling is correct, segment URLs are correct.
- Do NOT test muxer, encryptor, or manifest generator in isolation — they are implementation details behind the seam.
- Each test exercises one user story or one edge case of a user story.

### Test categories

- **Encryption correctness**: push known plaintext segment with RawKeyProvider, verify output is correctly encrypted (decrypt with known key and compare).
- **Manifest correctness**: verify m3u8/MPD structure, DRM signaling (PSSH, EXT-X-KEY, ContentProtection), segment URLs with CDN base.
- **Multi-DRM**: verify dual-encrypt produces correct separate manifests and segment sets.
- **Per-quality-tier keys**: verify different renditions in different tiers use different keys.
- **Live lifecycle**: verify sliding window manifest updates, explicit close, timeout auto-close.
- **VOD batch**: verify complete file and pre-segmented input both produce correct output.
- **Muxed vs unmuxed input**: verify both input types produce equivalent encrypted output.
- **Error cases**: invalid segment data, provider failure, session timeout.

## Out of Scope

- **Key rotation for live** — deferred to a future iteration. One key per session for now.
- **Offline playback / persistent license** — deferred. Streaming-only for MVP.
- **Forensic watermarking (A/B)** — deferred. Can be added as a pipeline stage later.
- **License server implementation** — drmpack proxies to providers, it does not issue licenses itself.
- **CDN upload** — media-server's responsibility. drmpack only produces bytes.
- **Transcoding / encoding** — drmpack receives already-encoded segments. It muxes and encrypts, but does not encode.
- **Authentication / authorization** — media-server handles auth for both its own API and the license proxy routes.
- **Phase 2 and Phase 3 codecs** — MVP supports H.264, H.265, AAC, MP3, WebVTT only.

## Further Notes

- The `CONTEXT.md` glossary defines all domain terms used in this spec. Any future issues or specs should use the same vocabulary.
- ADRs 0001–0003 in `docs/adr/` record the three key architectural decisions: library over service, native implementation over wrapping, and per-quality-tier keys.
- Each KeyProvider implementation must be based on the provider's official API documentation — no fabricated API contracts.
- The `tracing` crate integration means drmpack automatically participates in media-server's existing observability pipeline without additional configuration.
