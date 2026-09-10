//! AWS SPEKE v2.0 REST protocol client and authentication utilities.
//!
//! Provides [`SpekeClient`], [`SpekeConfig`], and authentication types including SigV4.

/// Authentication mechanisms and SigV4 credentials.
pub mod auth;
/// HTTP wire client for SPEKE v2 exchanges.
pub mod client;
/// Client configuration and endpoint settings.
pub mod config;

pub use auth::{SigV4Credentials, SpekeAuth, SpekeSigner};
pub use client::{SpekeClient, SpekeExchangeResponse};
pub use config::SpekeConfig;

/// Type alias for `SpekeClient` to satisfy User Story 10 and AWS SPEKE v2 naming.
pub type SpekeV2Provider = SpekeClient;

/// Type alias for `SpekeConfig` to satisfy User Story 10 and AWS SPEKE v2 naming.
pub type SpekeV2Config = SpekeConfig;
