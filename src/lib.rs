#![warn(missing_docs)]

//! # drmpack
//!
//! Native Rust DRM packaging and manifest generation library orchestrating GPAC filters
//! for CENC/CBCS fMP4 and HLS/DASH delivery with zero disk I/O.
//!
//! ## Overview
//!
//! `drmpack` is an in-process packaging orchestrator designed for media servers.
//! It accepts multiplexed fragmented MP4 (fMP4) streams in memory, pipes them directly
//! into GPAC filter graphs over anonymous Unix pipes, and emits encrypted CMAF segments
//! and playlists via an asynchronous in-memory channel.
//!
//! ## Architecture
//!
//! - **Control Plane**: Manages key acquisition, GPAC `cecrypt` DRM XML synthesis, and
//!   GPAC subprocess lifecycle supervision with fail-fast crash detection.
//! - **Data Plane**: Ingests fMP4 chunks via [`PackagingSession::push`] or [`SessionWriter`],
//!   and emits encrypted segments and updated playlists via [`PackagedArtifact`] channels
//!   with Manifest-Driven Readiness guarantees.
//!
//! ## Supported DRM Providers
//!
//! - **Axinom DRM**: Production-tested integration supporting Axinom Key Service
//!   (SPEKE v2 over CPIX 2.3), Widevine, FairPlay, and PlayReady licensing proxies,
//!   and JWT entitlement token minting via [`AxinomSigningConfig`].
//! - **DASH-IF CPIX 2.3**: Standardized CPIX client via [`CpixProvider`].
//! - **AWS SPEKE v2**: Generic wire client via [`SpekeClient`] / [`SpekeV2Provider`].
//! - **StaticKeySource**: In-memory test double and pre-shared key store via [`StaticKeySource`].
//!
//! ## Feature Flags
//!
//! | Feature Flag | Default | Description |
//! | :--- | :--- | :--- |
//! | `cpix` | Yes | DASH-IF CPIX 2.3 XML builder, parser, and [`CpixProvider`]. |
//! | `speke-v2` | Yes | AWS SPEKE v2 wire protocol client ([`SpekeClient`]). |
//! | `axinom` | Yes | Axinom Key Service adapter and token generator. |
//! | `license-proxy` | Yes | DRM license proxy client and handlers. |
//!
//! ## Quick Start
//!
//! ```rust
//! use drmpack::session::PackagingSessionConfig;
//! use drmpack::types::{EncryptionScheme, LatencyMode, Rendition};
//!
//! // Configure a live packaging session preset
//! let config = PackagingSessionConfig::new("live_stream_01")
//!     .with_encryption_scheme(EncryptionScheme::Cbcs)
//!     .with_latency_mode(LatencyMode::Standard)
//!     .with_segment_duration(2.0)
//!     .with_rendition(Rendition::video_hd())
//!     .with_rendition(Rendition::audio());
//!
//! assert_eq!(config.segment_duration, 2.0);
//! ```

/// Error types and crash diagnostics.
pub mod error;
/// GPAC multimedia engine integration and process management.
pub mod gpac;
/// DRM key management, key requests, and key provider trait.
pub mod key;
/// Packaging session controller, cluster management, and artifact harvesting.
pub mod session;
/// Core domain types, renditions, quality tiers, and encryption schemes.
pub mod types;
/// Standalone whole-file VOD batch packaging engine.
pub mod vod;

#[cfg(feature = "cpix")]
/// DASH-IF CPIX 2.3 XML protocol support and client.
pub mod cpix;

#[cfg(feature = "speke-v2")]
/// AWS SPEKE v2.0 wire client and SigV4 authentication.
pub mod speke;

#[cfg(feature = "license-proxy")]
/// In-process DRM license proxy client and handlers.
pub mod license;

/// Commercial DRM vendor integrations.
pub mod vendor;

#[cfg(any(feature = "axinom", feature = "license-proxy"))]
pub use vendor::axinom;

// Re-export primary types
pub use error::{
    DrmpackError, PackagingOperation, PackagingSessionFailure, RepresentationFailure, Result,
};
pub use gpac::{GpacDrmConfig, GpacDrmXmlGenerator, GpacProcess, GpacProcessConfig};
pub use key::{
    ContentKey, KeyID, KeyPlan, KeyPolicyEngine, KeyProvider, KeyRequest, KeySet, RawKeyProvider,
    StaticKeySource,
};
pub use session::{
    DrmKeyEntry, DrmStreamMetadata, PackagingResult, PackagingSession, PackagingSessionConfig,
    Representation, RepresentationCluster, SessionWriter,
};
pub use types::{
    ArtifactKind, DrmSystem, EgressMode, EncryptionScheme, KeyMappingPolicy, LatencyMode,
    ManifestFormat, PackagedArtifact, QualityTier, Rendition, TrackType,
};

#[cfg(feature = "cpix")]
pub use cpix::{CpixConfig, CpixKeySpec, CpixProvider, CpixRequestBuilder, CpixResponseParser};

#[cfg(feature = "speke-v2")]
pub use speke::{
    SigV4Credentials, SpekeAuth, SpekeClient, SpekeConfig, SpekeExchangeResponse, SpekeSigner,
    SpekeV2Config, SpekeV2Provider,
};

#[cfg(feature = "axinom")]
pub use vendor::axinom::{
    generate_axinom_jwt, AxinomConfig, AxinomKeyConfig, AxinomLicenseConfig, AxinomProvider,
    AxinomSigningConfig,
};

#[cfg(feature = "license-proxy")]
pub use license::{
    handle_fairplay_certificate, handle_fairplay_license, handle_playready_license,
    handle_widevine_license, IntoCertUrl, LicenseProxy, LicenseProxyConfig, LicenseResponse,
};
