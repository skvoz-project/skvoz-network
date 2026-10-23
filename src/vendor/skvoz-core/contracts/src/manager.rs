use crate::*;
use std::fmt;
const ACTIVE_KEY_INDEX_BACKING: usize = 384;
/// One synchronous ABI event copy may temporarily retain one extra frame.
pub const EVENT_BRIDGE_BACKING: usize = 16384;
pub const ABI_COLD_BRIDGE_BACKING: usize = 65536;
// Qualified layout constants are checked by supplier tests.
pub const LAYOUT_QUALIFIED: bool = cfg!(all(
    target_arch = "x86_64",
    target_os = "linux",
    target_env = "gnu"
));
pub const ENTRY_BYTES: usize = if LAYOUT_QUALIFIED { 592 } else { 1024 };
pub const PEER_BYTES: usize = if LAYOUT_QUALIFIED { 656 } else { 1024 };
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[cfg_attr(feature = "codec", derive(serde::Serialize, serde::Deserialize))]
pub struct PeerId(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[cfg_attr(feature = "codec", derive(serde::Serialize, serde::Deserialize))]
pub struct StreamKey {
    pub peer: PeerId,
    pub stream_id: u64,
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "codec", derive(serde::Serialize, serde::Deserialize))]
pub struct ManagerConfig {
    pub stream: Config,
    pub max_peers: usize,
    pub max_streams: usize,
    pub max_streams_per_peer: usize,
    pub receive_budget: usize,
    pub receive_budget_per_peer: usize,
    pub send_budget: usize,
    pub send_budget_per_peer: usize,
}
impl Default for ManagerConfig {
    fn default() -> Self {
        Self {
            stream: Config {
                receive_window: 8192,
                max_frame: 1024,
                max_pending_frames: 8,
                max_metadata: 256,
                open_timeout_ms: 5000,
            },
            max_peers: 128,
            max_streams: 8192,
            max_streams_per_peer: 128,
            receive_budget: 64 * 1024 * 1024,
            receive_budget_per_peer: 2 * 1024 * 1024,
            send_budget: 8 * 1024 * 1024,
            send_budget_per_peer: 256 * 1024,
        }
    }
}
impl ManagerConfig {
    pub fn receive_record_budget(self) -> usize {
        (self.receive_budget / self.stream.max_frame as usize)
            .saturating_add(self.max_peers.saturating_mul(64))
    }
    pub fn receive_records_per_peer(self) -> usize {
        (self.receive_budget_per_peer / self.stream.max_frame as usize)
            .saturating_add(64)
            .min(65536)
    }
    pub fn send_record_budget(self) -> usize {
        self.receive_record_budget()
    }
    pub fn send_records_per_peer(self) -> usize {
        (crate::MAX_RECEIVE_WINDOW as usize / self.stream.max_frame as usize)
            .saturating_add(64)
            .min(65536)
    }
    /// Payload is separate. Per record, 160 bytes conservatively cover the
    /// chunk/frame node, both end-offset nodes, alignment and allocator headers.
    /// Per stream, Entry plus 512 bytes cover BTree/deadline/waiter nodes, both
    /// ready indexes and four lifecycle event slots. Peer bookkeeping includes
    /// its four control slots and queue/index nodes. The two new key indexes
    /// have separate backing: 384 bytes/key covers a full 11-key/12-edge BTree
    /// node with 16-byte keys, headers, alignment and allocator overhead, even
    /// when sparsely occupied. This is ledger backing, not preallocation.
    pub fn metadata_backing(self) -> Option<usize> {
        self.receive_record_budget()
            .checked_add(self.send_record_budget())?
            .checked_mul(160)?
            .checked_add(
                self.max_streams.checked_mul(
                    ENTRY_BYTES
                        .checked_add(512)?
                        .checked_add(self.stream.max_metadata.checked_mul(3)?)?,
                )?,
            )?
            .checked_add(self.max_peers.checked_mul(PEER_BYTES.checked_add(512)?)?)
            .and_then(|bytes| {
                bytes.checked_add(
                    self.max_streams
                        .checked_add(self.max_peers)?
                        .checked_mul(ACTIVE_KEY_INDEX_BACKING)?,
                )
            })
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "codec", derive(serde::Serialize, serde::Deserialize))]
pub enum ManagerError {
    Stream(Error),
    InvalidConfig,
    UnknownPeer,
    UnknownStream,
    Admission,
    IdentityExhausted,
    InvalidOrigin,
    TransportLost,
    PeerBusy,
}
impl fmt::Display for ManagerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Core manager error: {self:?}")
    }
}
impl std::error::Error for ManagerError {}
impl From<Error> for ManagerError {
    fn from(e: Error) -> Self {
        Self::Stream(e)
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "codec", derive(serde::Serialize, serde::Deserialize))]
pub struct Resources {
    pub peers: usize,
    pub streams: usize,
    pub reserved_receive_bytes: usize,
    pub pending_send_bytes: usize,
    pub buffered_receive_bytes: usize,
    pub receive_capacity_bytes: usize,
    pub receive_unconsumed_bytes: u64,
    pub ready_output_peers: usize,
    pub ready_event_peers: usize,
    pub pending_rejections: usize,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "codec", derive(serde::Serialize, serde::Deserialize))]
pub struct Aggregate {
    pub peers: usize,
    pub streams: usize,
    pub reserved_receive_bytes: usize,
    pub pending_send_bytes: usize,
    pub ready_output_peers: usize,
    pub ready_event_peers: usize,
}

/// Process-wide executor stack reservation: one worker and at most two blocking threads.
pub const ABI_EXECUTOR_STACK_BACKING: usize = 3 * 512 * 1024;
impl ManagerConfig {
    pub fn metadata_backing_with_abi(self) -> Option<usize> {
        self.metadata_backing()?
            .checked_add(EVENT_BRIDGE_BACKING)?
            .checked_add(ABI_COLD_BRIDGE_BACKING)
    }
    pub fn validate(self) -> Result<(), ManagerError> {
        self.stream.validate()?;
        if self.max_peers == 0
            || self.max_streams == 0
            || self.max_streams_per_peer == 0
            || self.receive_budget == 0
            || self.receive_budget_per_peer == 0
            || self.send_budget == 0
            || self.send_budget_per_peer == 0
            || self.metadata_backing().is_none()
        {
            return Err(ManagerError::InvalidConfig);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "codec", derive(serde::Serialize, serde::Deserialize))]
pub struct FlowObservation {
    pub backlog_age_ms: Option<u64>,
    pub application_credit_blocked: bool,
    pub local_budget_blocked: bool,
    pub queued_for_output: bool,
    pub independent_control_ready: bool,
    pub unread_bytes: u64,
    pub pending_send_bytes: usize,
}
