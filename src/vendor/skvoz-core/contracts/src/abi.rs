//! ABI v1 scalar layout and operation numbers, shared with native consumers.
use std::ffi::c_void;

pub const ABI_VERSION: u32 = 1;
pub const CAPABILITIES: u64 = 0b111;
pub const MAX_DTO_BYTES: usize = 65536;
pub const EVENT_BRIDGE_BYTES: usize = 16384;
pub const MAX_HANDLES: usize = 128;
pub const OK: i32 = 0;
pub const PENDING: i32 = 1;
pub const WOULD_BLOCK: i32 = 2;
pub const CORE_ERROR: i32 = -1;
pub const INVALID: i32 = -2;
pub const BUSY: i32 = -3;
pub const PANIC: i32 = -4;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Key {
    pub epoch_high: u64,
    pub epoch_low: u64,
    pub incarnation: u64,
    pub peer: u64,
    pub stream: u64,
}
impl From<crate::runtime::RuntimeKey> for Key {
    fn from(k: crate::runtime::RuntimeKey) -> Self {
        Self {
            epoch_high: (k.epoch >> 64) as u64,
            epoch_low: k.epoch as u64,
            incarnation: k.incarnation,
            peer: k.stream.peer.0,
            stream: k.stream.stream_id,
        }
    }
}
impl From<Key> for crate::runtime::RuntimeKey {
    fn from(k: Key) -> Self {
        Self {
            epoch: (u128::from(k.epoch_high) << 64) | u128::from(k.epoch_low),
            incarnation: k.incarnation,
            stream: crate::StreamKey {
                peer: crate::PeerId(k.peer),
                stream_id: k.stream,
            },
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Buffer {
    pub data: *mut u8,
    pub len: usize,
}
impl Default for Buffer {
    fn default() -> Self {
        Self {
            data: std::ptr::null_mut(),
            len: 0,
        }
    }
}
#[repr(C)]
#[derive(Default)]
pub struct Reply {
    pub value: u64,
    pub key: Key,
    pub buffer: Buffer,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Wake {
    pub context: *mut c_void,
    pub retain: unsafe extern "C" fn(*mut c_void),
    pub wake: unsafe extern "C" fn(*mut c_void),
    pub release: unsafe extern "C" fn(*mut c_void),
}
#[repr(C)]
pub struct Event {
    pub key: Key,
    pub kind: u32,
    pub reason: u32,
    pub offset: u64,
    pub data: *const u8,
    pub len: usize,
}
pub type Admit = unsafe extern "C" fn(*mut c_void, Key, usize) -> bool;
pub type Receive = unsafe extern "C" fn(*mut c_void, *const Event);
pub type HostPoll = unsafe extern "C" fn(*mut c_void) -> bool;
pub mod op {
    pub const JOIN: u32 = 1;
    pub const REVOKE: u32 = 2;
    pub const AUTHORIZE: u32 = 3;
    pub const TERMINATE: u32 = 4;
    pub const EPOCH: u32 = 5;
    pub const STATUS: u32 = 6;
    pub const RESOURCES: u32 = 7;
    pub const PEER_STATUS: u32 = 8;
    pub const PEER_READY: u32 = 9;
    pub const PEER_DRAINED: u32 = 10;
    pub const OPEN: u32 = 11;
    pub const ACCEPT: u32 = 12;
    pub const REJECT: u32 = 13;
    pub const SEND: u32 = 14;
    pub const FINISH: u32 = 15;
    pub const CONSUME: u32 = 16;
    pub const CLOSE: u32 = 17;
    pub const SNAPSHOT: u32 = 18;
    pub const STREAM_DIAGNOSTICS: u32 = 19;
    pub const LIMITS: u32 = 20;
    pub const PEER_LIMITS: u32 = 21;
    pub const TRANSPORT_LOST: u32 = 22;
    pub const DIAGNOSTICS: u32 = 23;
    pub const SET_DIAGNOSTICS: u32 = 24;
    pub const DELIVERY_STATUS: u32 = 25;
}
