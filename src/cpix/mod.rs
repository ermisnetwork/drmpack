pub mod builder;
pub mod parser;

pub use builder::{CpixKeySpec, CpixRequestBuilder};
pub use parser::CpixResponseParser;

#[cfg(feature = "speke-v2")]
#[deprecated(note = "CpixProvider is deprecated, use crate::speke::SpekeClient instead")]
pub type CpixProvider = crate::speke::SpekeClient;

#[cfg(feature = "speke-v2")]
#[deprecated(note = "CpixConfig is deprecated, use crate::speke::SpekeConfig instead")]
pub type CpixConfig = crate::speke::SpekeConfig;
