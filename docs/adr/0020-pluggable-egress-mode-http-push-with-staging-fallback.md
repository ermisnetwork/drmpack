# Pluggable EgressMode: In-Process HTTP Push with Ephemeral Staging Fallback

**Status: Accepted**

In Milestone v0.1.x, `drmpack` writes packaged segments and manifests to an ephemeral staging directory (`/tmp` backed by OS page cache per [ADR-0015](0015-tmp-staging-directory-standard.md)) and uses an asynchronous `ArtifactHarvester` with kernel filesystem events (`notify`) and a 50ms watchdog ([ADR-0016](0016-production-event-driven-harvester-and-ephemeral-ingest.md)) to ingest completed artifacts into RAM (`PackagedArtifact` channel).

For Milestone v0.2.0, the core architectural goal is zero physical disk I/O. GPAC dasher filter supports `httpout:hmode=push`, which pushes packaged artifacts directly to an HTTP PUT endpoint. 

We adopt a pluggable `EgressMode` abstraction supporting two modes:
1. `EgressMode::HttpPush` (default for v0.2.0): Each `PackagingSession` runs an internal, lightweight in-process HTTP loopback listener on `127.0.0.1:<ephemeral_port>`. GPAC pushes segments and playlists directly into memory buffers, which are forwarded straight to `mpsc::Sender<PackagedArtifact>`.
2. `EgressMode::FileSystemStaging` (fallback): Retains the v0.1.x staging in `/tmp` observed by `ArtifactHarvester`.

## Considered options

- **Hard cut to HTTP Push only (Breaking Clean)**: Completely delete `ArtifactHarvester` and filesystem staging code. While it achieves the absolute minimal codebase, it eliminates operational fallback if GPAC `httpout` exhibits unexpected filter pipeline quirks under exotic container/network sandboxing environments.
- **Shared global HTTP server**: Run a single HTTP server in the media-server process space routing requests for all active sessions via session ID prefixes (`/session/{uuid}/...`). This introduces shared state, port conflict concerns, and complex lifecycle entanglement across concurrent sessions.
- **Per-session In-Process HTTP Listener with Staging Fallback (Chosen)**:
  - Each `PackagingSession` binds its own loopback listener on `127.0.0.1:0` (ephemeral port assigned by OS kernel), eliminating cross-session port collision.
  - A session-scoped random bearer token is passed in the URL/header to prevent local port snooping.
  - `EgressMode::FileSystemStaging` is preserved as a configurable fallback option via `PackagingSessionConfig::with_egress_mode(...)`.

## Consequences

- **Zero Disk I/O & Zero Kernel Watchers**: Under `HttpPush`, `/tmp` writes and `notify` kernel file watches are 100% eliminated for live packaging streams.
- **Preserved Public API**: The egress interface exposed to callers remains strictly `session.take_output_receiver(): Option<mpsc::Receiver<PackagedArtifact>>`, ensuring zero breaking changes for host applications (such as `media-server`).
- **Resilience**: If an edge-case environment disallows local TCP binding, operators can switch back to `FileSystemStaging` with a single configuration call.
