# Milestone 0003: Dual CENC and CBCS Concurrent Representations

Documented the completion of Ticket 04 (Dual Encryption Pipeline) within `drmpack`, enabling broad client reach covering legacy CENC devices and Apple FairPlay CBCS devices concurrently from a single input media stream.

## Key Architectural Achievements

- **Dual Orchestration Mode**: Introduced `EncryptionScheme::Dual` as an opt-in orchestration mode in `PackagingSession`. Spawns two isolated GPAC subprocesses, fanning out incoming media `Segment` bytes to both processes over anonymous Unix pipes without multiplexing interference.
- **Ramdisk Output Isolation**: Structured output directories so that Dual sessions write to isolated Ramdisk subtrees (`cenc/` and `cbcs/`), preventing filename collisions while preserving flat Ramdisk layout for single-scheme sessions. Typed manifest resolution (`manifest_path(scheme, format)`) guarantees delivery code receives unambiguous entrypoints.
- **Private Control Directory Hardening**: Stored sensitive GPAC DRM XML files (`cenc.xml`, `cbcs.xml`) inside an owner-restricted control directory (`0700` on Unix) completely separated from served Ramdisk delivery directories, ensuring DRM keys are never accidentally exposed via HTTP.
- **Fail-Close Session Lifecycle & Atomic Teardown**: Structured error reporting using `RepresentationFailure` and `PackagingSessionFailure`. If either Representation fails during spawn, segment push, or watchdog timeout, the session fail-closes atomically, shutting down both subprocesses and cleaning up resources.
- **Continuous Integration for DRM E2E**: Added `.github/workflows/drm-e2e.yml` running pinned GPAC 26.07 and FFmpeg on Ubuntu 24.04 to validate live CENC, CBCS, and Dual packaging e2e pipelines on pull requests and main branch.
