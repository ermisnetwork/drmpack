# In-process library over standalone service

**Status: Accepted**

drmpack is a Rust library (crate) imported directly by media-server, not a standalone service. The primary motivation is eliminating network hop latency on the segment-encryption hot path — every segment in a live stream would pay a round-trip to a service. DRM provider calls (key fetching, license proxying) already incur network I/O, so the library's own overhead should be zero. The trade-off is tighter coupling: drmpack must be Rust, must share media-server's async runtime (tokio), and deploys as part of media-server's binary rather than scaling independently.

## Considered options

- **Standalone service (gRPC/HTTP)**: independent scaling and language freedom, but adds a network hop per segment — unacceptable for live latency targets.
- **FFI / C ABI library**: language-agnostic but adds marshalling complexity and loses Rust's ownership guarantees across the boundary.

## Implementation Reality / Amendments

`drmpack` operates strictly as an in-process Rust library crate within `media-server`, exposing a zero-copy, in-memory data plane:
- **Ingest**: Callers feed media data directly via [`PackagingSession::push`] or the asynchronous [`SessionWriter`] (implementing `tokio::io::AsyncWrite`).
- **Harvesting**: Callers consume finalized CMAF segments and manifests through an in-memory asynchronous channel via [`PackagingSession::take_output_receiver`].

Low-level ISOBMFF muxing and cryptographic transforms are delegated to local GPAC subprocesses orchestrated over kernel anonymous Unix pipes per [ADR-0004](./0004-gpac-subprocess-over-native-rewrite.md). This architecture completely eliminates inter-process or network RPC hops on the media hot path while preserving memory safety and crash isolation between the C engine and the host runtime.
