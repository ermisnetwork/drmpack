//! # License Proxy
//!
//! In-process DRM license proxy module for forwarding player license challenges to
//! commercial DRM license services (Axinom Widevine, FairPlay, PlayReady) and caching
//! FairPlay Application Certificates in memory.

pub mod proxy;
pub mod response;

pub use proxy::{
    handle_fairplay_certificate, handle_fairplay_license, handle_playready_license,
    handle_widevine_license, IntoCertUrl, LicenseProxy,
};
pub use response::LicenseResponse;

#[cfg(any(feature = "axinom", feature = "license-proxy"))]
pub use crate::vendor::axinom::config::AxinomLicenseConfig;
