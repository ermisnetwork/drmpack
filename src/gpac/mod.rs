//! GPAC engine integration module.
//!
//! Provides DRM XML configuration generation and subprocess pipe orchestration
//! using the industrial GPAC multimedia framework (`gpac` filter graph).

pub mod process;
pub mod xml;

pub use process::{GpacProcess, GpacProcessConfig};
pub use xml::{GpacDrmConfig, GpacDrmXmlGenerator};
