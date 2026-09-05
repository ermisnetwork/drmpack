pub mod auth;
pub mod client;
pub mod config;

pub use auth::{SpekeAuth, SpekeSigner};
pub use client::{SpekeClient, SpekeExchangeResponse};
pub use config::SpekeConfig;

/// Type alias for `SpekeClient` to satisfy User Story 10 and AWS SPEKE v2 naming.
pub type SpekeV2Provider = SpekeClient;

/// Type alias for `SpekeConfig` to satisfy User Story 10 and AWS SPEKE v2 naming.
pub type SpekeV2Config = SpekeConfig;
