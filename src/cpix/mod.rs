//! DASH-IF CPIX (Content Protection Information Exchange) 2.3 XML documents and provider.

/// CPIX 2.3 XML request document builder.
pub mod builder;
/// CPIX 2.3 XML response parser.
pub mod parser;
/// HTTP CPIX key provider implementation.
pub mod provider;

pub use builder::{CpixKeySpec, CpixRequestBuilder};
pub use parser::CpixResponseParser;
pub use provider::{CpixConfig, CpixProvider};
