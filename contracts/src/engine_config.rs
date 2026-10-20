use crate::NetworkError;
use skvoz_core::ManagerConfig;

#[derive(Clone, Copy, Debug)]
pub struct EngineConfig {
    pub ip_sessions: usize,
    pub packet_queue_bytes: usize,
    pub packet_queue_records: usize,
    pub control_queue_bytes: usize,
    pub control_queue_records: usize,
}
impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            ip_sessions: 128,
            packet_queue_bytes: 262144,
            packet_queue_records: 256,
            control_queue_bytes: 32768,
            control_queue_records: 8,
        }
    }
}
impl EngineConfig {
    pub fn validate(self) -> Result<(), NetworkError> {
        if !(1..=128).contains(&self.ip_sessions)
            || !(1508..=262144).contains(&self.packet_queue_bytes)
            || !(1..=256).contains(&self.packet_queue_records)
            || !(16392..=32768).contains(&self.control_queue_bytes)
            || !(1..=8).contains(&self.control_queue_records)
        {
            return Err(NetworkError::InvalidConfiguration);
        }
        Ok(())
    }
    /// Canonical network receive/send profile; the same Core serves all profiles.
    pub fn core_limits(self, server: bool) -> ManagerConfig {
        crate::config::Limits::canonical(if server {
            crate::config::Role::Server
        } else {
            crate::config::Role::Client
        })
        .manager(if server {
            crate::config::Role::Server
        } else {
            crate::config::Role::Client
        })
    }
}
