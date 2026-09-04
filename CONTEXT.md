# drmpack

A Rust library orchestrating DRM packaging and manifest generation for media-server with zero disk I/O overhead. Consumed by media-server as an in-process dependency.

## Language

### Packaging & Orchestration

**PackagingSession**:
The core controller unit of work. Manages key acquisition, GPAC subprocess lifecycle over anonymous Unix pipes, and manifest delivery into Ramdisk.
_Avoid_: Job, task, pipeline, worker

**LatencyMode**:
The streaming delivery latency profile — `LowLatency` (CMAF chunking, LL-HLS, LL-DASH) or `Standard` (traditional 2-6s segments).
_Avoid_: Stream speed, delay profile

**Segment**:
A unit of media input — muxed fMP4 stream pushed into the packaging session.
_Avoid_: Chunk, fragment, frame

**Rendition**:
A single quality variant of the content, defined by resolution and bitrate (e.g. 720p@2Mbps). Belongs to exactly one QualityTier.
_Avoid_: Variant, profile, level

**QualityTier**:
A named group of Renditions that share a single ContentKey (e.g. SD, HD, 4K). Enables per-tier access policies.
_Avoid_: Key group, tier, quality level

**EncryptionScheme**:
The concrete cipher mode applied to one media representation — CENC (AES-CTR) or CBCS (AES-CBC 1:9 pattern). `Dual` is an orchestration mode that produces one independent representation of each concrete scheme from the same input.
_Avoid_: Protection scheme, cipher mode

### Keys & Licensing

**ContentKey**:
An AES-128 key used to encrypt media samples. Bound to a specific QualityTier and track type (video/audio). Identified by a KeyID (KID).
_Avoid_: Encryption key, media key

**KeyID (KID)**:
UUID identifying a ContentKey. Appears in PSSH boxes and manifest DRM signaling.
_Avoid_: Key identifier

**Provider**:
An external DRM service that supplies ContentKeys and serves player licenses (e.g. Axinom). Accessed through the KeyProvider trait.
_Avoid_: DRM server, key server, license server

**KeyProvider**:
The pluggable trait for key acquisition. Built-in implementations: CPIX (generic), SPEKE v2 (AWS), Axinom, RawKey (testing/development).
_Avoid_: Key source, key fetcher

**License proxy**:
An async handler function that forwards a player's license request to the Provider and returns the response. Media-server mounts it on an HTTP route; auth is media-server's responsibility.
_Avoid_: License server, license endpoint

**DRM Signaling**:
The metadata injected into manifests and initialization segments enabling player license acquisition — PSSH boxes for DASH/CMAF and `#EXT-X-KEY` attributes (`skd://` for FairPlay, inline data URI for Widevine/PlayReady) for HLS.
_Avoid_: Encryption metadata, DRM tags, key header

### Output & Storage

**Representation**:
An independently encrypted and packaged form of the same media, identified by a concrete EncryptionScheme. A Dual PackagingSession produces one CENC Representation and one CBCS Representation.
_Avoid_: Branch, stream

**Control directory**:
Private session-scoped storage for packaging control-plane material, kept separate from the Ramdisk delivery output.
_Avoid_: Output directory, served directory

**Ramdisk**:
A memory-backed filesystem directory (`/dev/shm` or `tmpfs`) where manifests and CMAF chunks are written and served with zero disk I/O.
_Avoid_: Cache, tempdir, disk buffer

**Manifest**:
The playlist or description file served to players — HLS (`.m3u8`) or DASH (`.mpd`). Managed in Ramdisk with correct DRM signaling (PSSH, EXT-X-KEY).
_Avoid_: Playlist (ambiguous with HLS-specific usage)

**Manifest format**:
The delivery protocol of a Manifest: DASH or HLS. It is independent of the concrete EncryptionScheme of a Representation.
_Avoid_: Output type, playlist type
