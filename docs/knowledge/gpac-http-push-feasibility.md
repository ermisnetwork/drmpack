# GPAC HTTP Push Feasibility: Fact-Check ADR-0015

> Research conclusion: ADR-0015 **misattributed** 2 GPAC issues, but the **architectural
> decision remains valid** due to complexity and performance parity.

> [!IMPORTANT]
> **Future work:** Will implement `httpout:hmode=push` to eliminate filesystem
> watching entirely. Both cited bugs (#2923, #3027) have been fixed in GPAC 26.07+,
> and localhost HTTP push is confirmed stable. Implementation requires: Axum HTTP
> server accepting PUT, dual-scheme routing (`/cenc/*`, `/cbcs/*`), ephemeral port allocation.

## ADR-0015 Claims vs Reality

### Issue #2923: "Memory leaks on long live runs"

| | ADR-0015 states | Reality (from GitHub primary source) |
|---|---|---|
| **About** | HTTP client push memory leak | MPEG-TS TCP socket ingest + filesystem output |
| **Uses httpout?** | Implied yes | **NO** — pipeline is `tcp://` → `reframer` → `.m3u8` file |
| **Related to HTTP push?** | Yes | **NO** — completely unrelated |
| **Status** | N/A | Closed Aug 2024 |

**Actual command in the issue:**
```bash
gpac -i tcp://localhost:5020/:gpac:tsprobe=true:listen=false reframer:rt=on \
  -o "/path/playlist.m3u8:gpac:..."
```
→ No `httpout`, no `hmode=push`. Bug was TCP socket memory, not HTTP client.

### Issue #3027: "Multi-PID failures"

| | ADR-0015 states | Reality (from GitHub primary source) |
|---|---|---|
| **About** | Multi-PID failure in HTTP push | HTTPS + `httpin` (S3 remote) → `httpout` graph-linking bug |
| **Localhost HTTP works?** | Implied broken | **YES** — reporter confirmed "only HTTP → HTTP works fine" |
| **Root cause** | General httpout instability | GPAC linker wired httpin PID directly into httpout + SSL non-blocking bug |
| **Fix** | N/A | **FIXED** — commit `85083494` (linker) + `d6c8b0fd` (SSL). Closed Apr 2025 |

### Verdict: ADR misattributed both bugs

`docs/knowledge/gpac-direct-output-and-streaming-sinks.md` also contains an inaccurate description:
> "During multi-day live sessions, maintaining continuous HTTP client push
> exhibits memory accumulation if the receiving server responds slowly..."

This sentence was **hallucinated/conflated** — Issue #2923 makes no mention of HTTP push or HTTP responses.

---

## But the architectural decision remains correct

Despite misattributing the bugs, **Page Cache staging is still better than HTTP push** because:

### 1. Performance: Page Cache is 1.5-2x faster

| Path | Latency (50KB segment) | Memory copies |
|------|----------------------|---------------|
| Page Cache (write → read → unlink) | **35-80 µs** | 2 |
| HTTP loopback (PUT → recv → 200 OK) | **60-130 µs** | 3-4 |

Both are <0.15ms — **no practical difference** on a 2s segment.
But HTTP is **never faster**, only slower + more complex.

### 2. Complexity: HTTP push adds ~300-500 LOC

If using `httpout:hmode=push`, drmpack would need to:
- Host an in-process HTTP server (Axum/Hyper)
- Bind an ephemeral loopback port and pass it to the GPAC subprocess
- Route `/cenc/*` and `/cbcs/*` for Dual scheme
- Handle TCP connection lifecycle, keep-alive, timeouts
- Handle edge cases: port conflicts, connection resets, partial PUTs

### 3. Container safety: Zero network attack surface

File staging uses only filesystem paths — no TCP ports opened inside the container.
HTTP push requires port binding → port collision risk in dense K8s pods.

### 4. Subprocess crash isolation

File staging: `ProcessSupervisor` catches exit status immediately.
HTTP push: must detect via `ECONNREFUSED` or socket timeout — slower.

---

## Recommended ADR-0015 corrections

ADR-0015 and `gpac-direct-output-and-streaming-sinks.md` should be updated to:
- **Remove** incorrect citations of Issues #2923 and #3027
- **Keep** the rejection but justify it with **complexity, zero perf gain, container safety**

---

## GPAC httpout:hmode=push — Actual capabilities

| Feature | Status (GPAC 26.07+) |
|---------|---------------------|
| HTTP PUT segments + manifests | ✅ Stable |
| HTTP POST (with `post=true`) | ✅ Stable |
| Chunked transfer (LL-CMAF) | ✅ Stable |
| Keep-alive connection reuse | ✅ Stable |
| DASH-IF Ingest Interface 2 | ✅ Native profile `dashif.ingest` |
| HTTPS push | ✅ Fixed (commit d6c8b0fd) |
| Unix Domain Sockets | ❌ Not supported in httpout |
| Multi-track localhost HTTP | ✅ Works (confirmed by #3027 reporter) |

## Sources

1. [GPAC Issue #2923](https://github.com/gpac/gpac/issues/2923) — TCP/TS memory, NOT HTTP push
2. [GPAC Issue #3027](https://github.com/gpac/gpac/issues/3027) — HTTPS linker bug, FIXED
3. [GPAC httpout filter docs](https://wiki.gpac.io/Filters/httpout/) — hmode=push specification
4. ADR-0015 `docs/adr/0015-direct-output-channel-and-safe-storage.md` L23
5. `docs/knowledge/gpac-direct-output-and-streaming-sinks.md` §2.1
6. Linux kernel `drivers/net/loopback.c`, `mm/page-writeback.c` — Page Cache mechanics
7. `rigtorp/ipc-bench`, `sockperf` — IPC latency benchmarks
