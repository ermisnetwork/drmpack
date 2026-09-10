//! GPAC engine integration module.
//!
//! Provides DRM XML configuration generation and subprocess pipe orchestration
//! using the industrial GPAC multimedia framework (`gpac` filter graph).

/// Subprocess execution, pipe management, and supervisor tasks.
pub mod process;
/// GPAC cecrypt DRM XML synthesis and track configuration.
pub mod xml;

pub use process::{GpacProcess, GpacProcessConfig};
pub use xml::{GpacDrmConfig, GpacDrmXmlGenerator};
