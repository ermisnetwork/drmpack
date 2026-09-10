//! Commercial DRM vendor integration modules.

#[cfg(any(feature = "axinom", feature = "license-proxy"))]
/// Axinom DRM Key Service and license proxy integration.
pub mod axinom;
