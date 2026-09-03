//! # drmpack
//!
//! A Rust DRM packaging orchestrator leveraging GPAC filter graphs for CENC/CBCS and low-latency manifests.

pub mod error;
pub mod gpac;
pub mod key;
pub mod session;
pub mod types;

// Re-export primary types
pub use error::{DrmpackError, Result};
pub use gpac::{GpacDrmConfig, GpacDrmXmlGenerator, GpacProcess, GpacProcessConfig};
pub use key::{ContentKey, KeyID, KeyProvider, KeyRequest, KeySet, RawKeyProvider};
pub use session::{PackagingSession, PackagingSessionConfig};
pub use types::{
    DrmSystem, EncryptionScheme, LatencyMode, QualityTier, Rendition, Segment, TrackType,
};
