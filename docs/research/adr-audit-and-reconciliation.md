# Architectural Decision Records (ADR) Comprehensive Audit & Reconciliation

> **Document Type**: Architecture Decision Record (ADR) Drift Audit & Reconciliation Matrix  
> **Repository**: `drmpack` (`ermis-stream/drmpack`)  
> **Methodology**: `domain-modeling` codebase cross-referencing and source-level verification against `src/`  
> **Date**: 2026-09-14  

---

## 1. Executive Summary

This audit cross-references all 19 Architecture Decision Records (`docs/adr/0001` through `0019`) against the actual implementation in `src/`, the system configuration in `Cargo.toml`, and the living ubiquitous language in `CONTEXT.md`.

### Core Findings:
1. **Critical Status Drift in ADR-0005**: `docs/adr/0005-ramdisk-tmpfs-manifest-distribution.md` still declares `Status: Accepted` asserting `/dev/shm` as the default storage target. However, it was **completely superseded by ADR-0015**, which moved the default to OS temporary storage (`/tmp` backed by kernel Page Cache) to prevent 64MB container crashes on Docker/K8s.
2. **Reversed Default in ADR-0003**: ADR-0003 declared granular `PerTierAndTrack` keying as mandatory. **ADR-0007 reversed this decision**, establishing `KeyMappingPolicy::SharedAll` as the system default while retaining `PerTierAndTrack` as an opt-in policy. ADR-0003 lacks any indication of this amendment.
3. **Parameter Drift in ADR-0016**: ADR-0016 specifies a "relaxed 1500ms watchdog timer". In production code (`src/session/harvester.rs:13`), this was calibrated down to **50ms (`WATCHDOG_INTERVAL_MS = 50`)** to eliminate macOS `FSEvents` coalescing lag and manifest synchronization latency.
4. **Decoupled Lifecycle in ADR-0011 / ADR-0012**: ADR-0011 claimed `session.close()` verifies `#EXT-X-ENDLIST` and executes storage cleanup. ADR-0012 subsequently decoupled finalization from cleanup, leaving `close()` to flush streams while delegating storage deletion to explicit `cleanup()` or RAII `Drop`.
5. **Missing Standard Status Headers**: ADR-0001, ADR-0003, ADR-0009, ADR-0016, ADR-0017, ADR-0018, and ADR-0019 lack standard status headers (`**Status: ...**`).

---

## 2. ADR Status & Reconciliation Matrix

| ADR | Title | Recorded Status | Code Reality & Successors | Recommended Status | Impact / Severity |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **0001** | In-process library over standalone service | *(Missing)* | 100% Compliant. Rust library crate sharing Tokio runtime; zero network hop hot path. | **`Accepted`** | Low (Formatting) |
| **0002** | Native Rust implementation over wrapping external packager | `Superseded by ADR-0004` | 100% Compliant. No native ISOBMFF/CENC parser; GPAC handles muxing. | **`Superseded by ADR-0004`** | None (Accurate) |
| **0003** | Per-track and per-quality-tier encryption keys | *(Missing)* | **Reversed by ADR-0007**: `SharedAll` is default. `PerTierAndTrack` is opt-in. | **`Amended by ADR-0007`** | **High** (Reversed default) |
| **0004** | GPAC subprocess orchestration over native rewrite | `Accepted` | 100% Compliant. Anonymous pipes for stdin/stderr; staging filesystem for output. | **`Accepted`** | Low (Clarify output boundary) |
| **0005** | Ramdisk tmpfs for manifest and chunk distribution | `Accepted` | **Superseded by ADR-0015**: Default output is `/tmp`, consumed via async channel. | **`Superseded by ADR-0015`** | **CRITICAL** (Severe misguidance) |
| **0006** | Dual CENC and CBCS representations for device compatibility | `Accepted` | Scheme-aware key material is production-ready; ADR-0014 made CBCS canonical default. | **`Accepted (Amended by ADR-0014)`** | Medium (Topology note obsolete) |
| **0007** | Selective encryption and key mapping policy | `Accepted` | 100% Compliant. Subtitle defaults to clear (`encrypted: false`) per ADR-0013. | **`Accepted (Amended by ADR-0013)`** | Low (Clarify subtitle behavior) |
| **0008** | Pluggable SpekeSigner over heavyweight AWS SDK | `Accepted` | 100% Compliant. Zero AWS SDK dependencies in `Cargo.toml`. | **`Accepted`** | None (Accurate) |
| **0009** | License Proxy connection pooling & cert caching | *(Missing)* | 100% Compliant. Decoupled vendor config; static defaults removed by ADR-0018. | **`Accepted (Amended by ADR-0018)`** | Low (Missing header) |
| **0010** | CMAF only over legacy HLS TS | `Accepted` | 100% Compliant on CMAF-only; baseline amended from Dual to Single CBCS by ADR-0014. | **`Accepted (Amended by ADR-0014)`** | Medium (Baseline updated) |
| **0011** | Async ProcessSupervisor and fail-fast lifecycle | `Accepted` | 100% Compliant on supervisor/fail-fast; storage cleanup decoupled by ADR-0012. | **`Accepted (Amended by ADR-0012)`** | Medium (Cleanup decoupling) |
| **0012** | Streamlined API ergonomics & concrete PackagingSession | *(Missing)* | 100% Compliant on concrete struct; `.with_label` removed by ADR-0013; tempdir by ADR-0015. | **`Accepted (Amended by 0013, 0014, 0015)`** | Medium (Evolving modifiers) |
| **0013** | Lean Rendition & track ID disambiguation | `Accepted` | 100% Compliant. Clear distinction between `track_id` (UUID) and `container_track_id`. | **`Accepted`** | None (Accurate) |
| **0014** | CBCS and Standard Latency as canonical baseline | `Accepted` | 100% Compliant. `new()` defaults to CBCS + Standard Latency (2.0s segments, 4000ms SPD). | **`Accepted`** | None (Accurate) |
| **0015** | Direct output channel and safe storage staging | `Accepted (Supersedes 0005)` | 100% Compliant. `take_output_receiver()`, OS tempdir `/tmp`, 60s TSB, immediate unlink. | **`Accepted`** | None (Accurate) |
| **0016** | Production event-driven harvester with watchdog | *(Missing)* | **Parameter Drift**: Watchdog in code is **50ms** (`WATCHDOG_INTERVAL_MS`), not 1500ms. | **`Amended`** | **High** (30x timing discrepancy) |
| **0017** | DRM playback metadata handoff and credentials | *(Missing)* | 100% Compliant. `DrmStreamMetadata` strictly excludes raw AES keys; JWT minting gated. | **`Accepted`** | Low (Missing header) |
| **0018** | Mandatory tenant endpoints for Axinom DRM | *(Missing)* | 100% Compliant. Purged static URLs; removed `Default` implementations. | **`Accepted`** | Low (Missing header) |
| **0019** | seg_sync=auto for manifest-driven readiness | *(Missing)* | 100% Compliant. GPAC configured with `seg_sync=auto` ensuring complete flushes. | **`Accepted`** | Low (Missing header) |

---

## 3. Detailed Audit Notes & Concrete Amendments

### 3.1 ADR-0003: Per-track and per-quality-tier encryption keys
- **Discrepancy**: ADR-0003 presented `PerTierAndTrack` as the mandatory strategy and explicitly rejected single-key configurations. ADR-0007 reversed this, making `SharedAll` the system default in `PackagingSessionConfig::new` (`src/types.rs:223-232`) to optimize common OTT workflows, while preserving `PerTierAndTrack` as an opt-in policy.
- **Action**: Update status to `**Status: Amended by ADR-0007**` and add an `## Implementation Reality` section documenting the relationship with ADR-0007.

### 3.2 ADR-0005: Ramdisk tmpfs for manifest and chunk distribution
- **Discrepancy**: ADR-0005 still reads `Status: Accepted` and mandates `/dev/shm`. ADR-0015 explicitly superseded this default because standard Docker/Kubernetes container runtimes allocate only 64MB to `/dev/shm`, causing catastrophic `ENOSPC` and OOM killer (`SIGKILL 137`) aborts. In code (`src/session/mod.rs:58-60`), `default_output_dir` uses `std::env::temp_dir()` (`/tmp` backed by kernel Page Cache), and outputs are consumed via in-memory channels.
- **Action**: Update status to `**Status: Superseded by ADR-0015 (regarding default storage target and output distribution)**`.

### 3.3 ADR-0006: Dual CENC and CBCS representations
- **Discrepancy**: The caveat in ADR-0006 stating that shared ContentKey/KID is merely a "topology proof" is obsolete. Production-ready scheme-aware keying is fully implemented in `src/key/mod.rs:250` and `src/cpix/`. Furthermore, ADR-0014 changed the default session configuration from `Dual` to single-scheme `CBCS` to save 50% CPU/memory, retaining `Dual` as an opt-in preset.
- **Action**: Update status to `**Status: Accepted (Amended by ADR-0014)**` and document production scheme-aware readiness.

### 3.4 ADR-0011: Asynchronous ProcessSupervisor and Fail-Fast Lifecycle
- **Discrepancy**: Line 34 states that `session.close()` verifies `#EXT-X-ENDLIST` and executes storage cleanup. ADR-0012 subsequently decoupled finalization (`close()`) from cleanup (`cleanup()`) so that CDN edge servers have time to fetch finalized playlists.
- **Action**: Update status to `**Status: Accepted (Amended by ADR-0012 and ADR-0015)**`.

### 3.5 ADR-0012: Streamlined API Ergonomics, Concrete PackagingSession
- **Discrepancy**: Mentioned `.with_label(name)` which was purged by ADR-0013. Stated `push(impl Into<Bytes>)` whereas the code uses `push(impl AsRef<[u8]>)` to avoid forced allocations. Stated `/dev/shm` default storage which was superseded by ADR-0015.
- **Action**: Update status to `**Status: Accepted (Amended by ADR-0013, ADR-0014, and ADR-0015)**`.

### 3.6 ADR-0016: Production Event-Driven Harvester with Watchdog Fallback
- **Discrepancy**: ADR-0016 specifies a 1500ms watchdog interval. In `src/session/harvester.rs:13`, `WATCHDOG_INTERVAL_MS = 50`. The 1500ms timer introduced unacceptable latency (up to 1.5–6.0 seconds) due to macOS `fseventsd` coalescing and manifest flushing order.
- **Action**: Update status to `**Status: Amended**` and document the 50ms latency tuning. Also update line 191 of `CONTEXT.md`.

---

## 4. Proposed Next Steps
1. Apply amendments and standard status headers to all 19 ADR files in `docs/adr/`.
2. Update `CONTEXT.md` line 191 to reflect the actual 50ms harvester watchdog interval.
3. Commit the reconciled documentation state to git.
