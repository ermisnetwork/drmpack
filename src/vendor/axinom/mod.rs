//! Axinom DRM integration module (Key Service and Licensing).

/// Configuration containers for Axinom Key Service and licensing.
pub mod config;
#[cfg(feature = "axinom")]
/// Axinom Key Service provider using SPEKE v2 over CPIX 2.3.
pub mod provider;
#[cfg(feature = "axinom")]
/// Axinom DRM JWT entitlement token generator and signing credentials.
pub mod token;

pub use config::*;
#[cfg(feature = "axinom")]
pub use provider::AxinomProvider;
#[cfg(feature = "axinom")]
pub use token::{generate_axinom_jwt, AxinomKeyConfig, AxinomSigningConfig};
