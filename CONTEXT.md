# drmpack

A Rust library orchestrating DRM packaging and manifest generation for media-server with zero disk I/O overhead. Consumed by media-server as an in-process dependency.

## Language

### Packaging & Orchestration

**PackagingSession**:
The core controller unit of work. Manages key acquisition, GPAC subprocess lifecycle over anonymous Unix pipes, and manifest delivery into Ramdisk.
_Avoid_: Job, task, pipeline, worker

**SessionWriter**:
An owned write handle implementing `tokio::io::AsyncWrite`, created via `PackagingSession::writer()`. Bridges poll-based I/O (e.g. `tokio::io::copy`) with the session's async data ingestion pipeline. Internally backed by a bounded channel and a forwarding task that replicates `push()` semantics (lifecycle checks, heartbeat, cluster fan-out).
_Avoid_: Write adapter, pipe wrapper, session sink


**ProcessSupervisor**:
The asynchronous supervisor task monitoring a GPAC subprocess exit lifecycle and stderr stream, detecting unexpected crashes immediately and distinguishing them from graceful finalization.
_Avoid_: Process monitor, child watcher, process tracker

**LatencyMode**:
The streaming delivery latency profile — `LowLatency` (CMAF chunking, LL-HLS, LL-DASH) or `Standard` (traditional 2-6s segments). `Standard` latency is the canonical production default for robust player buffer margins and segment rollover stability (ADR-0014).
_Avoid_: Stream speed, delay profile

**Segment**:
A packaged media output file (`.m4s`) produced by GPAC for HLS/DASH streaming delivery.
_Avoid_: Chunk, fragment, frame

**Rendition**:
A declared track configuration bound to a QualityTier for DRM key association. Uniquely identified by a system-generated logical `track_id` (`track_{type}_{uuid}`) and bound to an optional `container_track_id`. All stream metadata (codecs, resolution, bitrate, language) is extracted automatically from the container by GPAC.
_Avoid_: Variant, profile, level, stream

**track_id**:
The unique logical identifier string for a track within the packaging session (format: `track_{type}_{uuid}`, collision-free UUID). Distinguishes distinct Renditions sharing the same QualityTier without naming collisions.
_Avoid_: Track name, rendition ID, stream ID

**container_track_id**:
The 1-based ISO-BMFF track integer (`u32 >= 1`) in the `tkhd` box, mapped to GPAC `cecrypt` `<CrypTrack trackID="...">`. Defaults to 1-based declaration index (`1, 2, 3...`).
_Avoid_: Stream index, track number, track index

**Multi-Track ABR**:
An orchestrated packaging configuration combining multiple video Renditions (quality tiers), distinct audio tracks, and cleartext subtitle tracks within a single continuous multiplex.
_Avoid_: Adaptive stream, variant ladder

**QualityTier**:
A named group of Renditions that share a single ContentKey (e.g. SD, HD, 4K). Enables per-tier access policies.
_Avoid_: Key group, tier, quality level

**EncryptionScheme**:
The concrete cipher mode applied to one media representation — CBCS (AES-CBC 1:9 pattern for video, 0:0 for audio) or CENC (AES-CTR). `CBCS` is the canonical production default for universal CMAF Multi-DRM convergence across Apple FairPlay, Google Widevine, and Microsoft PlayReady (ADR-0014). `Dual` is an orchestration mode that produces one independent representation of each concrete scheme from the same input for legacy compatibility.
_Avoid_: Protection scheme, cipher mode

### Keys & Licensing

**ContentKey**:
An AES-128 key used to encrypt media samples. Bound to a specific QualityTier and track type (video/audio). Identified by a KeyID (KID).
_Avoid_: Encryption key, media key

**KeyID (KID)**:
UUID identifying a ContentKey. Appears in PSSH boxes and manifest DRM signaling.
_Avoid_: Key identifier

**Vendor**:
An external commercial DRM service partner (e.g. Axinom, BuyDRM, EZDRM) that supplies ContentKeys and serves player licenses. Managed under `drmpack::vendor::*`.
_Avoid_: DRM server, key server, license server

**CPIX**:
The DASH-IF Content Protection Information Exchange Format (v2.3/v2.4). Pure XML data serialization format for ContentKeys, KIDs, PSSH boxes, and track usage rules, as well as the standardized HTTP POST exchange protocol binding. Implemented in `drmpack::cpix` with request builder, response parser, and the built-in `CpixProvider` KeyProvider.
_Avoid_: CPIX DRM

**SPEKE**:
Secure Packager and Encoder Key Exchange (AWS specification v2.0). The REST API wire protocol client (`SpekeClient`, aliased as `SpekeV2Provider` for AWS DRM workflows) operating over HTTPS using CPIX 2.3 XML documents as message payload. Implemented in `drmpack::speke`. It is a wire protocol client, not a DRM provider.
_Avoid_: SPEKE DRM, SPEKE Provider

**StaticKeySource (RawKeyProvider)**:
The in-memory test double (Fake) and pre-shared key store supplying manually configured ContentKeys and PSSH boxes for unit/E2E testing and offline packaging without network I/O or XML parsing. Implemented in `drmpack::key::raw`.
_Avoid_: Raw DRM, Fake Provider, Raw Provider

**KeyProvider**:
The pluggable trait for key acquisition (`fetch_keys`). Implemented by vendor adapters (`AxinomProvider`), generic protocol clients (`SpekeClient` / `SpekeV2Provider`), standard DASH-IF implementations (`CpixProvider`), and local test doubles (`StaticKeySource`).
_Avoid_: Key source, key fetcher

**Axinom Provider (`AxinomProvider`)**:
The built-in vendor adapter targeting Axinom's Key Service API via SPEKE v2 over CPIX 2.3 (`https://key-server-management.axprod.net/api/SpekeV2`). Managed under `drmpack::vendor::axinom`. Authenticates using HTTP Basic Auth with Tenant ID and Management Key (`Authorization: Basic <base64(tenant_id:management_key)>`), composes `SpekeClient`, supports `overrideKeyIds` configuration, surfaces `X-AxDRM-ErrorMessage` diagnostics on non-200 responses, and maps homogeneous or dual-scheme requests into scheme-aware `KeySet`.
_Avoid_: Axinom client, Axinom adapter

**License proxy**:
An async handler function that forwards a player's license request to the Provider and returns the response. Media-server mounts it on an HTTP route; auth is media-server's responsibility.
_Avoid_: License server, license endpoint

**License challenge**:
A raw DRM challenge payload generated by the client player's CDM (e.g. Widevine challenge protobuf, FairPlay SPC, PlayReady challenge) containing ephemeral session keys and nonce data.
_Avoid_: License request body, DRM query

**License response**:
The structured container carrying the DRM license payload (Widevine license, FairPlay CKC, PlayReady license) and upstream HTTP metadata (Content-Type, diagnostic headers, device identification).
_Avoid_: License body, raw license

**Application certificate**:
Apple FairPlay public certificate (`.cer` / `.der`) required by Safari and iOS players to generate a Server Playback Context (SPC). Cached statically in memory.
_Avoid_: Apple cert, FairPlay key

**DRM Signaling**:
The metadata injected into manifests and initialization segments enabling player license acquisition — PSSH boxes for DASH/CMAF and `#EXT-X-KEY` attributes (`skd://` for FairPlay, inline data URI for Widevine/PlayReady) for HLS.
_Avoid_: Encryption metadata, DRM tags, key header

**KeyMappingPolicy**:
The orchestration policy governing how ContentKeys are assigned across Renditions — `SharedAll` (single key for all tracks, default), `SharedVideoSingleAudio` (one video key, one audio key), or `PerTierAndTrack` (granular key per QualityTier and track type per ADR-0003).
_Avoid_: Key allocation, key strategy, tier mode

**KeyPolicyEngine**:
The two-phase planner and resolver that decouples KeyMappingPolicy evaluation, provider key requests, and key replication across Renditions from session lifecycle.
_Avoid_: Key allocator, key manager

**KeyPlan**:
The intermediate execution plan computed during Phase 1 of KeyPolicyEngine planning, holding the optional KeyRequest and mapping source criteria.
_Avoid_: Key blueprint, key spec

**Selective Encryption**:
The capability to encrypt a subset of Renditions while passing others through unencrypted (Clear Renditions, e.g. unencrypted audio or clear SD preview). Clear Renditions bypass GPAC cecrypt and emit no DRM signaling in manifests.
_Avoid_: Partial encryption, hybrid DRM, split encryption

### Output & Storage

**Representation**:
An independently encrypted and packaged form of the same media, identified by a concrete EncryptionScheme. A Dual PackagingSession produces one CENC Representation and one CBCS Representation.
_Avoid_: Branch, stream

**RepresentationCluster**:
The coordinated group of one or more active Representations managed together for a PackagingSession (e.g. CENC and CBCS in Dual mode), handling joint media fan-out, lifecycle, and teardown.
_Avoid_: Pipeline, worker pool, process group, job

**Control directory**:
Private session-scoped storage for packaging control-plane material, kept separate from the Ramdisk delivery output.
_Avoid_: Output directory, served directory

**Storage Staging Directory**:
The temporary filesystem location (`/tmp` or OS tempdir by default) used as an intermediate workspace by GPAC dasher. Segments and manifests are read from staging into PackagedArtifact buffers and can be immediately pruned.
_Avoid_: Output folder, cache dir

**Ramdisk**:
An optional memory-backed filesystem directory (`/dev/shm` or `tmpfs`) for high-throughput zero-disk I/O deployments with explicitly provisioned memory headroom (opt-in per ADR-0015).
_Avoid_: Cache, tempdir, disk buffer

**Manifest**:
The playlist or description file served to players — HLS (`.m3u8`) or DASH (`.mpd`). Managed in staging with correct DRM signaling (PSSH, EXT-X-KEY).
_Avoid_: Playlist (ambiguous with HLS-specific usage)

**Manifest format**:
The delivery protocol of a Manifest: DASH or HLS. It is independent of the concrete EncryptionScheme of a Representation. In GPAC dasher, `:dual` specifies generating dual manifest formats (both DASH `.mpd` and HLS `.m3u8` simultaneously), which is distinct from `EncryptionScheme::Dual` (which produces dual cipher representations: CENC and CBCS).
_Avoid_: Output type, playlist type

**VOD Packaging**:
File-to-file static packaging for on-demand media assets, reading complete source containers from disk and generating static manifests with fixed durations and `#EXT-X-ENDLIST`, decoupled from the live streaming pipe orchestrator.
_Avoid_: Offline job, batch transcode, file packager

**PackagedArtifact**:
The structured container carrying an encrypted media segment or updated manifest emitted directly to callers via an async output channel. Eliminates manual filesystem polling for consumers.
_Avoid_: Output event, packaging message, segment packet

**ArtifactKind**:
The classification of an emitted PackagedArtifact — `InitSegment` (`init.mp4`), `MediaSegment` (`.m4s`), or `Manifest` (`.m3u8` / `.mpd`).
_Avoid_: File type, asset type, item kind

**Manifest-Driven Readiness**:
The synchronization invariant guaranteeing that a media segment is only emitted to callers after GPAC has fully flushed the segment and referenced it in the manifest. The HLS manifest is the canonical readiness signal; DASH MPD uses `SegmentTemplate` patterns that are present from session start and cannot signal individual segment completion. Eliminates partial-file read races.
_Avoid_: File polling, file stability check, timer delay

**ArtifactHarvester**:
The asynchronous background subsystem that monitors the Storage Staging Directory for completed segments and updated manifests, emitting them as PackagedArtifact values through the direct output channel. Uses kernel filesystem events (`inotify`/`FSEvents`) as the primary detection mechanism with a relaxed watchdog timer as safety net (ADR-0016).
_Avoid_: File watcher, polling loop, segment scanner

**HarvesterCadence**:
The detection and scheduling strategy governing how the ArtifactHarvester observes the staging directory — kernel event-driven (sub-millisecond, primary) or watchdog timer fallback (1500ms, safety net).
_Avoid_: Polling interval, scan rate, timer frequency

**Metadata Guarding**:
The optimization where manifest files are checked via filesystem metadata (`mtime` and `size`) before reading bytes into memory. Unchanged manifests are skipped entirely, eliminating redundant heap allocations and string parsing.
_Avoid_: Content diffing, hash check, file fingerprint

**Ephemeral Staging**:
The lifecycle model where intermediate packaging files in the Storage Staging Directory are unlinked immediately after ingestion into memory buffers, minimizing filesystem footprint and disk write amplification.
_Avoid_: Scratch directory, temporary caching, permanent staging

**Semantic Manifest & Segment Naming Standard**:
The standardized, resolution-aware naming scheme adhering to Apple HLS Authoring Guidelines and DASH-IF guidelines:
- **Master Manifests**: `live.m3u8` (HLS master) and `live.mpd` (DASH description).
- **Video Tracks**: Dynamically bound to track height as `video_{Height}p.m3u8` (e.g. `video_1080p.m3u8`, `video_720p.m3u8`, `video_360p.m3u8`), initialization segments `video_{Height}p_init.mp4`, and media segments `video_{Height}p_{Number}.m4s`.
- **Audio Tracks**: Canonical `audio.m3u8` (or `audio_{lang}.m3u8` for multi-language tracks), initialization segments `audio_init.mp4`, and media segments `audio_{Number}.m4s`.
- **Subtitle Tracks**: `sub.m3u8`, initialization segment `sub_init.mp4`, and media segments `sub_{Number}.m4s`.
- Eliminates raw numeric stream radicals (`live_1.m3u8`, `live_2.m3u8`) and pipe artifacts (`stdin_dash`).
_Avoid_: live_1.m3u8, numeric playlist names, stdin_dash
