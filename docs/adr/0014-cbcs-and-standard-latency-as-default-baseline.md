# Standardize on CBCS and Standard Latency as Canonical Production Baseline

**Status: Accepted**

`drmpack` establishes `EncryptionScheme::Cbcs` as its canonical default encryption scheme and `LatencyMode::Standard` as its canonical default delivery latency profile in `PackagingSessionConfig::new`. Dedicated presets `PackagingSessionConfig::cenc` and `PackagingSessionConfig::low_latency_dual` remain available for legacy client fallback and specialized low-latency deployments.

## Context & Industry Landscape

The original `PackagingSessionConfig` constructor defaulted to `EncryptionScheme::Cenc` and `LatencyMode::LowLatency`. In modern live production workflows targeting media-server, two major architectural requirements drove this update:

1. **Universal CMAF Multi-DRM Convergence (`cbcs`)**:
   - Apple FairPlay explicitly mandates `cbcs` (AES-CBC with pattern protection) and strictly rejects `cenc` (AES-CTR).
   - Modern Google Widevine (Android 7.0+, Chromium, Android TV) and Microsoft PlayReady (Windows 10/11, Xbox) have native support for `cbcs` over ISO-BMFF CMAF.
   - Operating in `cbcs` as the primary baseline allows a single stream of encrypted CMAF segments (`.m4s`) to serve Apple FairPlay, Google Widevine, and Microsoft PlayReady simultaneously, eliminating the dual storage and encoding overhead required by legacy CENC-only architectures.

2. **Playback Buffer Margin & Rollover Stability (`Standard` Latency + SPD)**:
   - Ultra-low latency chunking (`LatencyMode::LowLatency` with 200ms chunks) is highly sensitive to network jitter and player edge-chasing stalls.
   - For general broadcast and high-reliability live distribution, `LatencyMode::Standard` (2.0s segments) combined with DASH-IF `suggestedPresentationDelay` (`spd = 2.0x` segment duration, 4000ms) eliminates segment rollover stalls (HTTP 404) and delivers resilient playback across web players (Shaka Player, dash.js).

## Considered options

- **Retain CENC as default**: rejected because it cannot serve Apple FairPlay without running an expensive Dual representation session, creating a false assumption that `new()` generates an Apple-compatible stream.
- **Mandate Dual representation as default**: rejected because Dual doubles CPU encoding/packaging load and requires maintaining two concurrent GPAC subprocesses per session (ADR-0006), which is wasteful when modern client fleets support CBCS natively.
- **Adopt CBCS and Standard Latency as default baseline**: accepted; aligns `drmpack` with the global CMAF Multi-DRM convergence standard while preserving explicit `cenc()` and `low_latency_dual()` presets for legacy systems.

## Consequences

- `PackagingSessionConfig::new` initializes with `EncryptionScheme::Cbcs` and `LatencyMode::Standard`.
- `PackagingSessionConfig::cbcs` provides an explicit preset configured with `[FairPlay, Widevine, PlayReady]` DRM systems.
- Existing tests and documentation reflect `cbcs.xml` and `failure.cbcs` as the default packaging representation.
- Legacy CENC and Dual modes remain fully functional via `PackagingSessionConfig::cenc` and `PackagingSessionConfig::low_latency_dual`.
