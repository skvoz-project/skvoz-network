//! Canonical strict Network records, configuration and validators, without I/O.
pub mod access;
mod codec;
pub mod config;
mod engine_config;
pub mod enrollment;
pub mod local_api;
mod packet;
pub mod policy;
pub use codec::*;
pub use engine_config::EngineConfig;
pub use packet::*;

pub const BACKEND_QUEUE_CAPACITY: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkError {
    InvalidMetadata,
    UnsupportedVersion,
    UnsupportedType,
    UnsupportedFamily,
    InvalidRecord,
    InvalidPacket,
    InvalidConfiguration,
    InvalidState,
    Forbidden,
    Overloaded,
    Timeout,
    Runtime(skvoz_core::runtime::RuntimeError),
}
impl std::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "network error: {self:?}")
    }
}
impl std::error::Error for NetworkError {}
impl From<skvoz_core::runtime::RuntimeError> for NetworkError {
    fn from(value: skvoz_core::runtime::RuntimeError) -> Self {
        Self::Runtime(value)
    }
}
