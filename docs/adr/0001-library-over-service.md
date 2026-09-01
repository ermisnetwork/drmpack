# In-process library over standalone service

drmpack is a Rust library (crate) imported directly by media-server, not a standalone service. The primary motivation is eliminating network hop latency on the segment-encryption hot path — every segment in a live stream would pay a round-trip to a service. DRM provider calls (key fetching, license proxying) already incur network I/O, so the library's own overhead should be zero. The trade-off is tighter coupling: drmpack must be Rust, must share media-server's async runtime (tokio), and deploys as part of media-server's binary rather than scaling independently.

## Considered options

- **Standalone service (gRPC/HTTP)**: independent scaling and language freedom, but adds a network hop per segment — unacceptable for live latency targets.
- **FFI / C ABI library**: language-agnostic but adds marshalling complexity and loses Rust's ownership guarantees across the boundary.
