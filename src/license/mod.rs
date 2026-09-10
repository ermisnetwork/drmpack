//! # License Proxy
//!
//! In-process DRM license proxy module for forwarding player license challenges to
//! commercial DRM license services (Widevine, FairPlay, PlayReady) and caching
//! FairPlay Application Certificates in memory.

pub mod config;
pub mod proxy;
pub mod response;

pub use config::LicenseProxyConfig;
pub use proxy::{
    handle_fairplay_certificate, handle_fairplay_license, handle_playready_license,
    handle_widevine_license, IntoCertUrl, LicenseProxy,
};
pub use response::LicenseResponse;
