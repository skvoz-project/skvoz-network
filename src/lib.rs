//! Single-owner Core/NATS Network engine and supplier-side enrollment.
pub mod budget;
mod engine;
pub mod enrollment;
pub use engine::*;
pub use skvoz_network_contracts::*;

#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    feature = "portable-runtime"
))]
mod proxy;
#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    feature = "portable-runtime"
))]
pub mod runtime;
#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    feature = "portable-runtime"
))]
mod tcp;
#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    feature = "portable-runtime"
))]
pub use runtime::{OwnedMessage, PollError, RuntimeDiagnostics, RuntimeFailure, RuntimeHandle};
