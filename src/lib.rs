//! # drmpack
//!
//! A Rust DRM packaging orchestrator leveraging GPAC filter graphs for CENC/CBCS and low-latency manifests.

pub mod error;
pub mod gpac;
pub mod key;
pub mod session;
pub mod types;

#[cfg(feature = "cpix")]
pub mod cpix;

#[cfg(feature = "axinom")]
pub mod axinom;

// Re-export primary types
pub use error::{
    DrmpackError, PackagingOperation, PackagingSessionFailure, RepresentationFailure, Result,
};
pub use gpac::{GpacDrmConfig, GpacDrmXmlGenerator, GpacProcess, GpacProcessConfig};
pub use key::{
    ContentKey, KeyID, KeyPlan, KeyPolicyEngine, KeyProvider, KeyRequest, KeySet, RawKeyProvider,
};
pub use session::{
    PackagingSession, PackagingSessionConfig, Representation, RepresentationCluster,
};
pub use types::{
    DrmSystem, EncryptionScheme, KeyMappingPolicy, LatencyMode, ManifestFormat, QualityTier,
    Rendition, Segment, TrackType,
};

#[cfg(feature = "cpix")]
pub use cpix::{CpixConfig, CpixKeySpec, CpixProvider, CpixRequestBuilder, CpixResponseParser};

#[cfg(feature = "axinom")]
pub use axinom::{AxinomConfig, AxinomProvider, DEFAULT_AXINOM_ENDPOINT};
