# Dual CENC and CBCS representations for broad device compatibility

**Status: Accepted (Amended by ADR-0014)**

`drmpack` will support `Dual` as a PackagingSession orchestration mode that creates independent CENC and CBCS Representations from the same media input. The project retains both representations to cover legacy CENC-only Widevine/PlayReady clients alongside Apple FairPlay and CBCS-capable modern clients; `Dual` is not a concrete encryption scheme passed to GPAC.

## Considered options

- **CENC only**: provides broad legacy compatibility but cannot provide the CBCS/FairPlay packaging path required by Apple devices.
- **CBCS only**: simplifies operations and serves modern compatible clients but excludes CENC-only legacy client populations.
- **Dual representations**: increases packaging and Ramdisk resource consumption, but preserves the intended broad device coverage.

## Consequences

- Each Dual session is one unit of work: a failure in either Representation fails the session and teardown attempts to shut down both subprocesses.
- The current shared ContentKey/KID model is a topology proof only. Production Dual packaging requires scheme-aware, distinct key identity and material for CENC and CBCS before it can be treated as production-safe.

## Implementation Reality / Amendments

The preliminary constraints and default orchestration model identified in ADR-0006 evolved significantly in production:
- **Production Scheme-Aware Keys ("Topology Proof" Resolved)**:
  - The initial caveat noting that shared ContentKey/KID was merely a "topology proof" has been completely resolved in production.
  - `KeySet` natively differentiates keys by cryptographic scheme (`ContentKey::scheme`), and the CPIX builder/parser and Axinom key provider independently request and resolve distinct key identities, key material, and IVs for CENC and CBCS.
  - The GPAC XML generator creates isolated DRM configuration files per representation, ensuring complete cryptographic independence.
- **Transition to Single CBCS Baseline ([ADR-0014](./0014-cbcs-and-standard-latency-as-default-baseline.md))**:
  - Running Dual representations doubles compute and memory consumption (spawning two concurrent GPAC subprocesses per session).
  - Because modern device ecosystems (Apple FairPlay, Android/Chromium Widevine, Windows/Xbox PlayReady) now universally support ISO-BMFF CMAF with `cbcs`, ADR-0014 standardized `EncryptionScheme::Cbcs` with `LatencyMode::Standard` as the canonical default in `PackagingSessionConfig::new`, cutting CPU and RAM utilization by 50%.
- **Dual as Configurable Presets**:
  - Dual packaging remains fully supported for workflows that must support legacy CENC-only devices.
  - It is exposed via dedicated constructors: `PackagingSessionConfig::dual` (Standard Latency) and `PackagingSessionConfig::low_latency_dual` (Low Latency CMAF chunking), complete with symmetric process teardown under `ProcessSupervisor` ([ADR-0011](./0011-async-process-supervisor-and-fail-fast-lifecycle.md)).
