//! Standalone whole-file VOD batch packaging module.
//!
//! Provides [`VodInputSource`](crate::vod::VodInputSource), [`VodMode`](crate::vod::VodMode),
//! [`VodPackageConfig`](crate::vod::VodPackageConfig), and [`VodPackageResult`](crate::vod::VodPackageResult).

pub mod types;
pub use types::{VodInputSource, VodMode, VodPackageConfig, VodPackageResult};
