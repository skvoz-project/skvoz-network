pub use skvoz_core_contracts::TurnDiagnostics;
pub use skvoz_core_contracts::runtime::*;
use skvoz_core_contracts::{
    abi::{self, Key, Reply, op},
    *,
};
use std::{
    ffi::c_void,
    future::Future,
    marker::PhantomData,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker},
    time::Duration,
};

unsafe extern "C" {
    fn skvoz_core_v1_version() -> u32;
    fn skvoz_core_v1_capabilities() -> u64;
    fn skvoz_core_v1_buffer_free(buffer: abi::Buffer);
    fn skvoz_core_v1_runtime_new(out: *mut *mut c_void) -> i32;
    fn skvoz_core_v1_runtime_free(handle: *mut c_void) -> i32;
    fn skvoz_core_v1_start(
        handle: *mut c_void,
        kind: u32,
        wait_ms: u64,
        data: *const u8,
        len: usize,
        out: *mut *mut c_void,
        reply: *mut Reply,
    ) -> i32;
    fn skvoz_core_v1_poll(
        handle: *mut c_void,
        wake: abi::Wake,
        host_context: *mut c_void,
        host_poll: abi::HostPoll,
        reply: *mut Reply,
    ) -> i32;
    fn skvoz_core_v1_operation_free(handle: *mut c_void) -> i32;
    fn skvoz_core_v1_call(
        handle: *mut c_void,
        command: u32,
        key: Key,
        argument: u64,
        data: *const u8,
        len: usize,
        reply: *mut Reply,
    ) -> i32;
    fn skvoz_core_v1_events(
        handle: *mut c_void,
        max: usize,
        context: *mut c_void,
        admit: abi::Admit,
        receive: abi::Receive,
    ) -> i32;
}
struct Output(Reply);
impl Output {
    fn new() -> Self {
        Self(Reply::default())
    }
    fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T, RuntimeError> {
        if self.0.buffer.len > abi::MAX_DTO_BYTES || self.0.buffer.data.is_null() {
            return Err(RuntimeError::Protocol);
        }
        let bytes = unsafe { std::slice::from_raw_parts(self.0.buffer.data, self.0.buffer.len) };
        serde_json::from_slice(bytes).map_err(|_| RuntimeError::Protocol)
    }
    fn check(&self, code: i32) -> Result<(), RuntimeError> {
        match code {
            abi::OK | abi::WOULD_BLOCK => Ok(()),
            abi::CORE_ERROR => Err(self.decode()?),
            abi::BUSY => Err(RuntimeError::Admission),
            _ => Err(RuntimeError::Protocol),
        }
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        unsafe {
            skvoz_core_v1_buffer_free(self.0.buffer);
        }
    }
}
/// One exclusive owner; moving between threads is allowed, concurrent calls are not.
pub struct NatsRuntime {
    handle: *mut c_void,
    _exclusive: PhantomData<std::cell::Cell<()>>,
}
// The supplier permits transferring exclusive ownership to another thread.
unsafe impl Send for NatsRuntime {}
impl Drop for NatsRuntime {
    fn drop(&mut self) {
        unsafe {
            skvoz_core_v1_runtime_free(self.handle);
        }
    }
}
unsafe extern "C" fn retain(context: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        Arc::<Waker>::increment_strong_count(context.cast());
    }));
}
unsafe extern "C" fn release(context: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        Arc::<Waker>::decrement_strong_count(context.cast());
    }));
}
unsafe extern "C" fn wake(context: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        (&*context.cast::<Waker>()).wake_by_ref();
    }));
}
struct Host<'a, F> {
    future: Pin<&'a mut F>,
    context: *mut Context<'static>,
    panic: Option<Box<dyn std::any::Any + Send>>,
}
unsafe extern "C" fn host_poll<F: Future<Output = ()>>(context: *mut c_void) -> bool {
    let host = unsafe { &mut *context.cast::<Host<'_, F>>() };
    match catch_unwind(AssertUnwindSafe(|| {
        host.future
            .as_mut()
            .poll(unsafe { &mut *host.context })
            .is_ready()
    })) {
        Ok(ready) => ready,
        Err(panic) => {
            host.panic = Some(panic);
            true
        }
    }
}
struct Operation<'a> {
    handle: *mut c_void,
    _owner: PhantomData<&'a mut NatsRuntime>,
}
// Operation only borrows the exclusive, transferable runtime.
unsafe impl Send for Operation<'_> {}
impl Drop for Operation<'_> {
    fn drop(&mut self) {
        unsafe {
            skvoz_core_v1_operation_free(self.handle);
        }
    }
}
impl Operation<'_> {
    async fn run<F: Future<Output = ()>>(self, wake_future: F) -> Result<usize, RuntimeError> {
        let mut wake_future = std::pin::pin!(wake_future);
        std::future::poll_fn(|context| {
            let waker = Arc::new(context.waker().clone());
            let wake = abi::Wake {
                context: Arc::as_ptr(&waker).cast_mut().cast(),
                retain,
                wake,
                release,
            };
            let mut output = Output::new();
            let mut host = Host {
                future: wake_future.as_mut(),
                // Erase only the raw pointer's lifetime: the supplier invokes
                // host_poll synchronously inside this call and never retains
                // this Context. Host and Context remain live until it returns.
                context: (context as *mut Context<'_>).cast::<Context<'static>>(),
                panic: None,
            };
            let code = unsafe {
                skvoz_core_v1_poll(
                    self.handle,
                    wake,
                    (&mut host as *mut Host<'_, F>).cast(),
                    host_poll::<F>,
                    &mut output.0,
                )
            };
            if let Some(panic) = host.panic.take() {
                resume_unwind(panic);
            }
            if code == abi::PENDING {
                Poll::Pending
            } else {
                Poll::Ready(output.check(code).map(|()| output.0.value as usize))
            }
        })
        .await
    }
}
impl NatsRuntime {
    pub async fn connect(
        config: RuntimeConfig,
        limits: ManagerConfig,
    ) -> Result<Self, RuntimeError> {
        if unsafe { skvoz_core_v1_version() } != abi::ABI_VERSION
            || unsafe { skvoz_core_v1_capabilities() } & abi::CAPABILITIES != abi::CAPABILITIES
        {
            return Err(RuntimeError::UnsupportedVersion);
        }
        let bytes = skvoz_core_contracts::codec::encode(&(config, limits))
            .map_err(|_| RuntimeError::Config)?;
        if bytes.len() > abi::MAX_DTO_BYTES {
            return Err(RuntimeError::Config);
        }
        let mut handle = std::ptr::null_mut();
        let output = Output::new();
        output.check(unsafe { skvoz_core_v1_runtime_new(&mut handle) })?;
        let mut runtime = Self {
            handle,
            _exclusive: PhantomData,
        };
        let operation = runtime.start(0, 0, &bytes)?;
        drop(bytes);
        operation.run(std::future::pending()).await?;
        Ok(runtime)
    }
    fn start(
        &mut self,
        kind: u32,
        wait_ms: u64,
        bytes: &[u8],
    ) -> Result<Operation<'_>, RuntimeError> {
        let mut output = Output::new();
        let mut handle = std::ptr::null_mut();
        let code = unsafe {
            skvoz_core_v1_start(
                self.handle,
                kind,
                wait_ms,
                bytes.as_ptr(),
                bytes.len(),
                &mut handle,
                &mut output.0,
            )
        };
        output.check(code)?;
        Ok(Operation {
            handle,
            _owner: PhantomData,
        })
    }
    fn call(
        &self,
        command: u32,
        key: RuntimeKey,
        argument: u64,
        bytes: &[u8],
    ) -> Result<(i32, Output), RuntimeError> {
        let mut output = Output::new();
        let code = unsafe {
            skvoz_core_v1_call(
                self.handle,
                command,
                key.into(),
                argument,
                bytes.as_ptr(),
                bytes.len(),
                &mut output.0,
            )
        };
        output.check(code)?;
        Ok((code, output))
    }
    fn scalar(&self, command: u32, argument: u64) -> Output {
        self.call(command, empty_key(), argument, &[])
            .expect("Core ABI owner contract failed")
            .1
    }
    fn query<T: serde::de::DeserializeOwned>(
        &self,
        command: u32,
        key: RuntimeKey,
        argument: u64,
    ) -> T {
        self.call(command, key, argument, &[])
            .and_then(|(_, out)| out.decode())
            .expect("Core ABI observation failed")
    }
    fn command(
        &mut self,
        command: u32,
        key: RuntimeKey,
        argument: u64,
        bytes: &[u8],
    ) -> Result<(), RuntimeError> {
        self.call(command, key, argument, bytes).map(|_| ())
    }
    pub fn join_peer(&mut self, peer: PeerId) -> Result<(), RuntimeError> {
        self.command(op::JOIN, empty_key(), peer.0, &[])
    }
    pub fn revoke_peer(&mut self, peer: PeerId) -> Result<(), RuntimeError> {
        self.command(op::REVOKE, empty_key(), peer.0, &[])
    }
    pub fn authorize_peer(
        &mut self,
        peer: PeerId,
        generation: Option<u128>,
    ) -> Result<(), RuntimeError> {
        self.command(
            op::AUTHORIZE,
            RuntimeKey {
                epoch: generation.unwrap_or(0),
                incarnation: u64::from(generation.is_some()),
                ..empty_key()
            },
            peer.0,
            &[],
        )
    }
    pub fn terminate_peer(&mut self, peer: PeerId) {
        self.scalar(op::TERMINATE, peer.0);
    }
    pub fn epoch(&self) -> u128 {
        let out = self.scalar(op::EPOCH, 0);
        (u128::from(out.0.key.epoch_high) << 64) | u128::from(out.0.key.epoch_low)
    }
    pub fn status(&self) -> Status {
        self.query(op::STATUS, empty_key(), 0)
    }
    pub fn resources(&self) -> Resources {
        self.query(op::RESOURCES, empty_key(), 0)
    }
    pub fn peer_status(&self, peer: PeerId) -> Option<PeerStatus> {
        self.query(op::PEER_STATUS, empty_key(), peer.0)
    }
    pub fn peer_ready(&self, peer: PeerId) -> bool {
        self.scalar(op::PEER_READY, peer.0).0.value != 0
    }
    pub fn peer_streams_drained(&self, peer: PeerId) -> bool {
        self.scalar(op::PEER_DRAINED, peer.0).0.value != 0
    }
    pub fn open(&mut self, peer: PeerId, bytes: &[u8]) -> Result<RuntimeKey, RuntimeError> {
        self.call(op::OPEN, empty_key(), peer.0, bytes)
            .map(|(_, out)| out.0.key.into())
    }
    pub fn accept(&mut self, key: RuntimeKey, bytes: &[u8]) -> Result<(), RuntimeError> {
        self.command(op::ACCEPT, key, 0, bytes)
    }
    pub fn reject(&mut self, key: RuntimeKey, bytes: &[u8]) -> Result<(), RuntimeError> {
        self.command(op::REJECT, key, 0, bytes)
    }
    pub fn send(&mut self, key: RuntimeKey, bytes: &[u8]) -> Result<SendOutcome, RuntimeError> {
        self.call(
            op::SEND,
            key,
            0,
            &bytes[..bytes.len().min(RUNTIME_FRAME_BYTES)],
        )
        .map(|(code, out)| {
            if code == abi::WOULD_BLOCK {
                SendOutcome::WouldBlock
            } else {
                SendOutcome::Accepted(out.0.value as usize)
            }
        })
    }
    pub fn finish(&mut self, key: RuntimeKey) -> Result<(), RuntimeError> {
        self.command(op::FINISH, key, 0, &[])
    }
    pub fn consume_through(&mut self, key: RuntimeKey, offset: u64) -> Result<(), RuntimeError> {
        self.command(op::CONSUME, key, offset, &[])
    }
    pub fn close(&mut self, key: RuntimeKey) -> Result<(), RuntimeError> {
        self.command(op::CLOSE, key, 0, &[])
    }
    pub fn snapshot(&self, key: RuntimeKey) -> Option<Snapshot> {
        self.query(op::SNAPSHOT, key, 0)
    }
    pub fn stream_diagnostics(&self, key: RuntimeKey) -> Option<StreamDiagnostics> {
        self.query(op::STREAM_DIAGNOSTICS, key, 0)
    }
    pub fn limits(&self) -> ManagerConfig {
        self.query(op::LIMITS, empty_key(), 0)
    }
    pub fn peer_limits(&self, key: RuntimeKey) -> Option<PeerLimits> {
        self.query(op::PEER_LIMITS, key, 0)
    }
    pub fn transport_lost(&mut self) {
        self.scalar(op::TRANSPORT_LOST, 0);
    }
    pub fn delivery_status(&self) -> Result<DeliveryStatus, RuntimeError> {
        self.call(op::DELIVERY_STATUS, empty_key(), 0, &[])
            .and_then(|(_, out)| out.decode())
    }
    pub fn diagnostics(&self) -> TurnDiagnostics {
        self.query(op::DIAGNOSTICS, empty_key(), 0)
    }
    pub fn set_diagnostics_enabled(&mut self, enabled: bool) {
        self.scalar(op::SET_DIAGNOSTICS, u64::from(enabled));
    }
    pub async fn turn(&mut self, wait: Duration) -> Result<usize, RuntimeError> {
        self.turn_with_wake(wait, std::future::pending()).await
    }
    pub async fn turn_with_wake<F: Future<Output = ()>>(
        &mut self,
        wait: Duration,
        wake: F,
    ) -> Result<usize, RuntimeError> {
        let wait = u64::try_from(wait.as_millis()).map_err(|_| RuntimeError::Config)?;
        self.start(1, wait, &[])?.run(wake).await
    }
    pub async fn shutdown(&mut self) -> Result<(), RuntimeError> {
        self.start(2, 0, &[])?
            .run(std::future::pending())
            .await
            .map(|_| ())
    }
    pub fn poll_events(&mut self, maximum: usize) -> Vec<RuntimeEvent> {
        self.poll_events_with_data_budget(maximum, |_, _| true)
    }
    pub fn poll_events_with_data_budget<F: FnMut(RuntimeKey, usize) -> bool>(
        &mut self,
        maximum: usize,
        admit: F,
    ) -> Vec<RuntimeEvent> {
        let mut receiver = Receiver {
            admit,
            events: Vec::new(),
            panic: None,
        };
        let code = unsafe {
            skvoz_core_v1_events(
                self.handle,
                maximum.min(MAX_BATCH_EVENTS),
                (&mut receiver as *mut Receiver<_>).cast(),
                admission::<F>,
                receive::<F>,
            )
        };
        if let Some(panic) = receiver.panic {
            resume_unwind(panic);
        }
        Output::new()
            .check(code)
            .expect("Core ABI event owner contract failed");
        receiver.events
    }
}
fn empty_key() -> RuntimeKey {
    RuntimeKey {
        epoch: 0,
        incarnation: 0,
        stream: StreamKey {
            peer: PeerId(0),
            stream_id: 0,
        },
    }
}
struct Receiver<F> {
    admit: F,
    events: Vec<RuntimeEvent>,
    panic: Option<Box<dyn std::any::Any + Send>>,
}
unsafe extern "C" fn admission<F: FnMut(RuntimeKey, usize) -> bool>(
    context: *mut c_void,
    key: Key,
    len: usize,
) -> bool {
    let receiver = unsafe { &mut *context.cast::<Receiver<F>>() };
    if receiver.panic.is_some() {
        return false;
    }
    match catch_unwind(AssertUnwindSafe(|| (receiver.admit)(key.into(), len))) {
        Ok(admit) => admit,
        Err(panic) => {
            receiver.panic = Some(panic);
            false
        }
    }
}
unsafe extern "C" fn receive<F>(context: *mut c_void, event: *const abi::Event) {
    let receiver = unsafe { &mut *context.cast::<Receiver<F>>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let raw = unsafe { &*event };
        assert!(
            raw.len <= abi::EVENT_BRIDGE_BYTES,
            "Core event exceeds ABI bound"
        );
        let payload = || {
            if raw.len == 0 {
                Box::<[u8]>::default()
            } else {
                unsafe { std::slice::from_raw_parts(raw.data, raw.len) }.into()
            }
        };
        let event = match raw.kind {
            0 => Event::IncomingOpen {
                metadata: payload(),
            },
            1 => Event::Opened {
                metadata: payload(),
            },
            2 => Event::Rejected { reason: payload() },
            3 => Event::Data {
                offset: raw.offset,
                bytes: payload(),
            },
            4 => Event::Writable,
            5 => Event::RemoteFinished,
            6 => Event::Closed {
                reason: match raw.reason {
                    0 => CloseReason::Finished,
                    1 => CloseReason::Rejected,
                    2 => CloseReason::Cancelled,
                    3 => CloseReason::TransportLost,
                    4 => CloseReason::ProtocolError,
                    5 => CloseReason::OpenTimeout,
                    6 => CloseReason::ResourcePressure,
                    _ => panic!("invalid Core close reason"),
                },
            },
            _ => panic!("invalid Core event kind"),
        };
        receiver.events.push(RuntimeEvent {
            key: raw.key.into(),
            event,
        });
    }));
    if let Err(panic) = result {
        receiver.panic = Some(panic);
    }
}
