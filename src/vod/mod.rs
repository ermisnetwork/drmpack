//! Standalone whole-file VOD batch packaging module.
//!
//! Provides [`package_vod_file`](crate::vod::package_vod_file),
//! [`VodInputSource`](crate::vod::VodInputSource), [`VodMode`](crate::vod::VodMode),
//! [`VodPackageConfig`](crate::vod::VodPackageConfig), and [`VodPackageResult`](crate::vod::VodPackageResult).

pub mod engine;
pub mod types;

pub use crate::session::isobmff::is_complete_isobmff_single_file;
pub use engine::package_vod_file;
pub use types::{VodInputSource, VodMode, VodPackageConfig, VodPackageResult};
