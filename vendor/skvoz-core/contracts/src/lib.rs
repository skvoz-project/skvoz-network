//! Source-only Core contracts; no engine, runtime tasks or native loading.
mod diagnostics;
mod manager;
mod types;
pub use diagnostics::*;
pub use manager::*;
pub use types::*;
#[cfg(feature = "runtime")]
pub mod abi;
#[cfg(feature = "runtime")]
pub mod runtime;

#[cfg(all(feature = "codec", feature = "runtime"))]
pub mod codec;
