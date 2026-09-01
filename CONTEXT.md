# drmpack

A Rust library for DRM-encrypting media segments and generating CMAF/HLS/DASH manifests. Consumed by media-server as an in-process dependency.

## Language

### Packaging & Encryption

**PackagingSession**:
The core unit of work. Created by media-server with rendition declarations, quality tier mappings, DRM configuration, and encryption mode selection. Holds manifest state for the session's lifetime.
_Avoid_: Job, task, pipeline

**Segment**:
A unit of media input — either muxed (fMP4 with init segment) or unmuxed (raw codec NALUs). The atomic input to the packaging pipeline.
_Avoid_: Chunk, fragment, frame

**Rendition**:
A single quality variant of the content, defined by resolution and bitrate (e.g. 720p@2Mbps). Belongs to exactly one QualityTier.
_Avoid_: Variant, profile, level

**QualityTier**:
A named group of Renditions that share a single ContentKey (e.g. SD, HD, 4K). Enables per-tier access policies.
_Avoid_: Key group, tier, quality level

**EncryptionScheme**:
The encryption mode applied to segments — CENC (CTR) or CBCS (CBC pattern). Configurable per-session; a session may produce both schemes simultaneously.
_Avoid_: Protection scheme, cipher mode

### Keys & Licensing

**ContentKey**:
An AES-128 key used to encrypt segments. Bound to a specific QualityTier and track type (video/audio). Identified by a KeyID (KID).
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

### Output

**OutputSink**:
A trait implemented by media-server to receive encrypted segments, init segments, and manifests. Controls where output goes (memory, disk, CDN stream).
_Avoid_: Writer, output handler, destination

**Manifest**:
The playlist/description file served to players — m3u8 (HLS) or MPD (DASH). drmpack owns manifest generation and lifecycle, including DRM signaling (PSSH, EXT-X-KEY). Separate manifests are produced per DRM/encryption-scheme combination.
_Avoid_: Playlist (ambiguous with HLS-specific usage)
