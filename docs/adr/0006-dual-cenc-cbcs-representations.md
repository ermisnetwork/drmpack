# Dual CENC and CBCS representations for broad device compatibility

**Status: Accepted**

`drmpack` will support `Dual` as a PackagingSession orchestration mode that creates independent CENC and CBCS Representations from the same media input. The project retains both representations to cover legacy CENC-only Widevine/PlayReady clients alongside Apple FairPlay and CBCS-capable modern clients; `Dual` is not a concrete encryption scheme passed to GPAC.

## Considered options

- **CENC only**: provides broad legacy compatibility but cannot provide the CBCS/FairPlay packaging path required by Apple devices.
- **CBCS only**: simplifies operations and serves modern compatible clients but excludes CENC-only legacy client populations.
- **Dual representations**: increases packaging and Ramdisk resource consumption, but preserves the intended broad device coverage.

## Consequences

- Each Dual session is one unit of work: a failure in either Representation fails the session and teardown attempts to shut down both subprocesses.
- The current shared ContentKey/KID model is a topology proof only. Production Dual packaging requires scheme-aware, distinct key identity and material for CENC and CBCS before it can be treated as production-safe.
