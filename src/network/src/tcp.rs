//! Native stream ownership: partial SEND and write prefixes advance independently.
use crate::{
    NetworkEngine, NetworkError,
    budget::{Budget, Reservation},
};
use skvoz_core::runtime::RuntimeKey;
use skvoz_core::{Event, SendOutcome};
use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    net::{Shutdown, TcpStream},
    os::fd::{AsFd, OwnedFd},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

trait CorePort {
    fn send(&mut self, key: RuntimeKey, bytes: &[u8]) -> Result<SendOutcome, NetworkError>;
    fn consume(&mut self, key: RuntimeKey, offset: u64) -> Result<(), NetworkError>;
    fn finish(&mut self, key: RuntimeKey) -> Result<bool, NetworkError>;
    fn allowance(&self, peer: skvoz_core::PeerId) -> (usize, usize, usize);
    fn account(&mut self, peer: skvoz_core::PeerId, send: usize, receive: usize, records: usize);
}
impl CorePort for NetworkEngine {
    fn send(&mut self, key: RuntimeKey, bytes: &[u8]) -> Result<SendOutcome, NetworkError> {
        self.runtime.send(key, bytes).map_err(Into::into)
    }
    fn consume(&mut self, key: RuntimeKey, offset: u64) -> Result<(), NetworkError> {
        self.runtime
            .consume_through(key, offset)
            .map_err(Into::into)
    }
    fn finish(&mut self, key: RuntimeKey) -> Result<bool, NetworkError> {
        self.finish_native_tcp(key)
    }
    fn allowance(&self, peer: skvoz_core::PeerId) -> (usize, usize, usize) {
        self.tcp_allowance(peer)
    }
    fn account(&mut self, peer: skvoz_core::PeerId, send: usize, receive: usize, records: usize) {
        self.account_native(peer, send, receive, records)
    }
}
pub(crate) enum Socket {
    Tcp(TcpStream),
    Unix(UnixStream),
}
impl Socket {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(s) => s.read(b),
            Self::Unix(s) => s.read(b),
        }
    }
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(s) => s.write(b),
            Self::Unix(s) => s.write(b),
        }
    }
    fn shutdown_write(&self) -> io::Result<()> {
        match self {
            Self::Tcp(s) => s.shutdown(Shutdown::Write),
            Self::Unix(s) => s.shutdown(Shutdown::Write),
        }
    }
}
struct WritePending {
    _reservation: Reservation,
    bytes: Box<[u8]>,
    cursor: usize,
    offset: u64,
}
pub(crate) struct SetupFailure {
    pub error: NetworkError,
    pub socket: Socket,
    pub reservation: Reservation,
}
pub(crate) struct TcpConnection {
    pub key: RuntimeKey,
    socket: Socket,
    send: Vec<u8>,
    send_cursor: usize,
    receive: VecDeque<WritePending>,
    receive_bytes: usize,
    receive_limit: usize,
    record_limit: usize,
    local_eof: bool,
    finished: bool,
    remote_eof: bool,
    write_shutdown: bool,
    write_deadline: Option<Instant>,
    pub opened: bool,
    pub reply: Option<(u32, crate::SessionId, OwnedFd)>,
    pub success: Vec<u8>,
    pub failure: Vec<u8>,
    pub uploaded: u64,
    pub downloaded: u64,
    core_closed: bool,
    rejected: bool,
    pub rejection: Option<crate::local_api::ApiError>,
    prefix: Vec<u8>,
    prefix_cursor: usize,
    budget: Budget,
    _reservation: Reservation,
}
pub(crate) struct DataFailure {
    pub event: Event,
    pub error: Option<NetworkError>,
}
impl TcpConnection {
    pub fn new(
        key: RuntimeKey,
        socket: Socket,
        budget: &Budget,
        initial: Vec<u8>,
        limits: skvoz_core::ManagerConfig,
    ) -> Result<Self, NetworkError> {
        let reservation = budget.reserve(4096 + initial.capacity(), 4)?;
        Self::with_reservation(key, socket, budget, initial, limits, reservation)
            .map_err(|failure| failure.error)
    }
    pub fn with_reservation(
        key: RuntimeKey,
        socket: Socket,
        budget: &Budget,
        initial: Vec<u8>,
        limits: skvoz_core::ManagerConfig,
        mut reservation: Reservation,
    ) -> Result<Self, SetupFailure> {
        // Explicit TCP buffer sizes disable Linux autotuning. Local Unix
        // sockets retain a bounded queue; TCP uses the kernel's active policy.
        if let Socket::Unix(stream) = &socket
            && skvoz_network_native::configure_socket_buffers(stream.as_fd(), 131072).is_err()
        {
            return Err(SetupFailure {
                error: NetworkError::InvalidState,
                socket,
                reservation,
            });
        }
        // The setup parser has been dropped; its owned reservation transfers once.
        if let Err(error) = reservation.resize(4096 + initial.capacity(), 4) {
            return Err(SetupFailure {
                error,
                socket,
                reservation,
            });
        }
        if initial.len() > 65536 {
            return Err(SetupFailure {
                error: NetworkError::Overloaded,
                socket,
                reservation,
            });
        }
        Ok(Self {
            key,
            socket,
            send: initial,
            send_cursor: 0,
            receive: VecDeque::new(),
            receive_bytes: 0,
            receive_limit: limits
                .receive_budget_per_peer
                .min(limits.stream.receive_window as usize),
            record_limit: limits.receive_records_per_peer(),
            local_eof: false,
            finished: false,
            remote_eof: false,
            write_shutdown: false,
            write_deadline: None,
            opened: false,
            reply: None,
            success: Vec::new(),
            failure: Vec::new(),
            uploaded: 0,
            downloaded: 0,
            core_closed: false,
            rejected: false,
            rejection: None,
            prefix: Vec::new(),
            prefix_cursor: 0,
            budget: budget.clone(),
            _reservation: reservation,
        })
    }
    pub fn core_finished(&self) -> bool {
        self.core_closed || self.rejected
    }
    pub fn gracefully_finished(&self) -> bool {
        self.core_closed && self.receive.is_empty() && self.write_shutdown
    }
    pub fn event(&mut self, event: Event) -> Result<bool, NetworkError> {
        match event {
            Event::Opened { .. } => {
                self.opened = true;
                self.prefix = std::mem::take(&mut self.success);
                if !self.prefix.is_empty() {
                    self.write_deadline = Some(Instant::now() + Duration::from_secs(30));
                }
            }
            event @ Event::Data { .. } => {
                self.try_data(event)
                    .map_err(|failure| failure.error.unwrap_or(NetworkError::Overloaded))?;
            }
            Event::RemoteFinished => self.remote_eof = true,
            Event::Closed {
                reason: skvoz_core::CloseReason::Finished,
            } => {
                self.core_closed = true;
                self.remote_eof = true;
            }
            Event::Rejected { reason } => {
                self.rejection = Some(reject_error(&reason));
                if self.failure.is_empty() {
                    return Ok(false);
                }
                self.opened = true;
                self.rejected = true;
                self.prefix = if self.rejection == Some(crate::local_api::ApiError::Overloaded)
                    && self.failure.starts_with(b"HTTP/")
                {
                    self.failure = Vec::new();
                    crate::proxy::HTTP_OVERLOADED.to_vec()
                } else {
                    std::mem::take(&mut self.failure)
                };
                self.write_deadline = Some(Instant::now() + Duration::from_secs(1));
            }
            Event::Closed { .. } => return Ok(false),
            Event::Writable => {}
            Event::IncomingOpen { .. } => return Err(NetworkError::InvalidState),
        }
        Ok(true)
    }
    /// Shared-budget exhaustion retains the original event for an ordered retry.
    /// Every reservation is atomic; an availability snapshot is not sufficient.
    pub(crate) fn try_data(&mut self, event: Event) -> Result<(), DataFailure> {
        let Event::Data { offset, bytes } = event else {
            return Err(DataFailure {
                event,
                error: Some(NetworkError::InvalidState),
            });
        };
        let failure = |bytes, error| DataFailure {
            event: Event::Data { offset, bytes },
            error,
        };
        if self.remote_eof
            || self.receive.len() >= self.record_limit
            || self
                .receive_bytes
                .checked_add(bytes.len())
                .is_none_or(|n| n > self.receive_limit)
        {
            return Err(failure(bytes, Some(NetworkError::Overloaded)));
        }
        let reservation = match self.budget.reserve(bytes.len(), 1) {
            Ok(reservation) => reservation,
            Err(_) => return Err(failure(bytes, None)),
        };
        if self.receive.len() == self.receive.capacity() {
            let target = (self.receive.capacity().max(2) * 2).min(self.record_limit);
            let old_capacity = self.receive.capacity();
            let base = 4096 + self.send.capacity();
            let node = std::mem::size_of::<WritePending>();
            // Charge both old and replacement arrays during ownership transfer.
            if self
                ._reservation
                .resize(base + (old_capacity + target) * node, 4)
                .is_err()
            {
                return Err(failure(bytes, None));
            }
            let mut replacement = VecDeque::with_capacity(target);
            if replacement.capacity() > target
                && self
                    ._reservation
                    .resize(base + (old_capacity + replacement.capacity()) * node, 4)
                    .is_err()
            {
                drop(replacement);
                // Keeping a conservative reservation is safe and retryable.
                return Err(failure(bytes, None));
            }
            replacement.append(&mut self.receive);
            self.receive = replacement;
            if let Err(error) = self.account_capacity() {
                return Err(failure(bytes, Some(error)));
            }
        }
        self.receive_bytes += bytes.len();
        self.receive.push_back(WritePending {
            _reservation: reservation,
            bytes,
            cursor: 0,
            offset,
        });
        self.write_deadline
            .get_or_insert_with(|| Instant::now() + Duration::from_secs(30));
        Ok(())
    }
    pub fn turn(&mut self, engine: &mut NetworkEngine) -> Result<(), NetworkError> {
        self.turn_port(engine)
    }
    fn turn_port(&mut self, engine: &mut impl CorePort) -> Result<(), NetworkError> {
        if !self.opened {
            return Ok(());
        }
        if self.write_deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(NetworkError::Timeout);
        }
        // Native preamble is written before any remote payload, and creates no Core credit.
        if self.prefix_cursor < self.prefix.len() {
            match self.socket.write(&self.prefix[self.prefix_cursor..]) {
                Ok(0) => return Err(NetworkError::InvalidState),
                Ok(n) => {
                    self.prefix_cursor += n;
                    if !self.rejected {
                        self.write_deadline = Some(Instant::now() + Duration::from_secs(30));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(_) => return Err(NetworkError::InvalidState),
            }
            if self.prefix_cursor < self.prefix.len() {
                return Ok(());
            }
            self.prefix = Vec::new();
            self.write_deadline = None;
        }
        if self.rejected {
            return Err(NetworkError::InvalidState);
        }
        let (_, mut receive_limit, record_limit) = engine.allowance(self.key.stream.peer);
        for _ in 0..record_limit {
            if receive_limit == 0 {
                break;
            }
            let Some(front) = self.receive.front_mut() else {
                break;
            };
            match self.socket.write(
                &front.bytes[front.cursor..front.bytes.len().min(front.cursor + receive_limit)],
            ) {
                Ok(0) => return Err(NetworkError::InvalidState),
                Ok(n) => {
                    front.cursor += n;
                    self.receive_bytes -= n;
                    receive_limit -= n;
                    self.downloaded = self.downloaded.saturating_add(n as u64);
                    engine.account(self.key.stream.peer, 0, n, 1);
                    self.write_deadline = Some(Instant::now() + Duration::from_secs(30));
                    if front.cursor == front.bytes.len() {
                        let consumed = front
                            .offset
                            .checked_add(front.bytes.len() as u64)
                            .ok_or(NetworkError::InvalidState)?;
                        // Retain byte and record promises until the entire owned chunk is released.
                        if !self.core_closed {
                            engine.consume(self.key, consumed)?;
                        }
                        self.receive.pop_front();
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => return Err(NetworkError::InvalidState),
            }
        }
        if self.receive.is_empty() {
            self.receive = VecDeque::new();
            self.account_capacity()?;
            self.write_deadline = None;
        }
        if self.remote_eof && self.receive.is_empty() && !self.write_shutdown {
            self.socket
                .shutdown_write()
                .map_err(|_| NetworkError::InvalidState)?;
            self.write_shutdown = true;
        }
        if self.core_closed {
            if self.receive.is_empty() && self.write_shutdown {
                return Err(NetworkError::InvalidState);
            }
            return Ok(());
        }
        if self.send_cursor == self.send.len() {
            self.send.clear();
            self.send_cursor = 0;
            if !self.local_eof {
                // Do not remove bytes from the kernel until their worst-case
                // read capacity is backed. Exhaustion applies backpressure.
                if self
                    ._reservation
                    .resize(
                        4096 + self.send.capacity().max(16384)
                            + self.receive.capacity() * std::mem::size_of::<WritePending>(),
                        4,
                    )
                    .is_err()
                {
                    return Ok(());
                }
                let mut buffer = [0u8; 16384];
                match self.socket.read(&mut buffer) {
                    Ok(0) => {
                        self.local_eof = true;
                        self.send = Vec::new();
                        self.account_capacity()?;
                    }
                    Ok(n) => {
                        // Reserve before growth; reuse already charged capacity during active I/O.
                        let target = self.send.capacity().max(n);
                        self._reservation.resize(
                            4096 + target
                                + self.receive.capacity() * std::mem::size_of::<WritePending>(),
                            4,
                        )?;
                        self.send.reserve_exact(n.saturating_sub(self.send.len()));
                        self.send.extend_from_slice(&buffer[..n]);
                        self.account_capacity()?;
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        self.send = Vec::new();
                        self.account_capacity()?;
                    }
                    Err(_) => return Err(NetworkError::InvalidState),
                }
            }
        }
        let (send_limit, _, record_limit) = engine.allowance(self.key.stream.peer);
        if self.send_cursor < self.send.len() && send_limit > 0 && record_limit > 0 {
            match engine.send(
                self.key,
                &self.send[self.send_cursor..self.send.len().min(self.send_cursor + send_limit)],
            )? {
                SendOutcome::Accepted(n) => {
                    self.send_cursor += n;
                    self.uploaded = self.uploaded.saturating_add(n as u64);
                    engine.account(self.key.stream.peer, n, 0, 1);
                }
                SendOutcome::WouldBlock => {}
            }
        }
        if self.local_eof
            && self.send_cursor == self.send.len()
            && !self.finished
            && engine.allowance(self.key.stream.peer).2 > 0
            && engine.finish(self.key)?
        {
            engine.account(self.key.stream.peer, 0, 0, 1);
            self.finished = true;
        }
        Ok(())
    }
    fn account_capacity(&mut self) -> Result<(), NetworkError> {
        self._reservation.resize(
            4096 + self.send.capacity()
                + self.receive.capacity() * std::mem::size_of::<WritePending>(),
            4,
        )
    }
}

pub(crate) fn reject_error(bytes: &[u8]) -> crate::local_api::ApiError {
    use crate::local_api::ApiError;
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Rejection {
        v: u8,
        #[serde(rename = "type")]
        kind: String,
        error: String,
    }
    let Ok(value) = crate::local_api::parse_strict_json_bounded(bytes, 512) else {
        return ApiError::InvalidRequest;
    };
    let Ok(rejection) = serde_json::from_value::<Rejection>(value) else {
        return ApiError::InvalidRequest;
    };
    if rejection.v != crate::NETWORK_VERSION {
        return ApiError::UnsupportedVersion;
    }
    if rejection.kind != "tcp" {
        return ApiError::InvalidRequest;
    }
    match rejection.error.as_str() {
        "forbidden" => ApiError::Forbidden,
        "overloaded" => ApiError::Overloaded,
        "timeout" => ApiError::Timeout,
        "unsupported_version" => ApiError::UnsupportedVersion,
        "network_unavailable" => ApiError::NetworkUnavailable,
        _ => ApiError::InvalidRequest,
    }
}

#[cfg(test)]
#[path = "tcp/tests.rs"]
mod tests;
