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

#[cfg(feature = "speke-v2")]
pub mod speke;

pub mod vendor;

#[cfg(feature = "axinom")]
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
    PackagingSession, PackagingSessionConfig, Representation, RepresentationCluster,
};
pub use types::{
    DrmSystem, EncryptionScheme, KeyMappingPolicy, LatencyMode, ManifestFormat, QualityTier,
    Rendition, Segment, TrackType,
};

#[cfg(feature = "cpix")]
pub use cpix::{CpixConfig, CpixKeySpec, CpixProvider, CpixRequestBuilder, CpixResponseParser};

#[cfg(feature = "speke-v2")]
pub use speke::{
    SpekeAuth, SpekeClient, SpekeConfig, SpekeExchangeResponse, SpekeSigner, SpekeV2Config,
    SpekeV2Provider,
};

#[cfg(feature = "axinom")]
pub use vendor::axinom::{AxinomConfig, AxinomProvider, DEFAULT_AXINOM_ENDPOINT};
