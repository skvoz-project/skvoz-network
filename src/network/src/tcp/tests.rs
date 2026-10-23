use super::*;
use skvoz_core::{PeerId, StreamKey};
struct Port {
    script: VecDeque<usize>,
    sent: Vec<u8>,
    consumed: Vec<u64>,
    finished: bool,
    terminal: usize,
    used: (usize, usize),
}
impl CorePort for Port {
    fn send(&mut self, _key: RuntimeKey, bytes: &[u8]) -> Result<SendOutcome, NetworkError> {
        let n = self
            .script
            .pop_front()
            .unwrap_or(bytes.len())
            .min(bytes.len());
        if n == 0 {
            return Ok(SendOutcome::WouldBlock);
        }
        self.sent.extend_from_slice(&bytes[..n]);
        Ok(SendOutcome::Accepted(n))
    }
    fn consume(&mut self, _key: RuntimeKey, offset: u64) -> Result<(), NetworkError> {
        self.consumed.push(offset);
        Ok(())
    }
    fn finish(&mut self, _key: RuntimeKey) -> Result<bool, NetworkError> {
        if self.terminal >= 4 {
            return Ok(false);
        }
        self.terminal += 1;
        self.finished = true;
        Ok(true)
    }
    fn allowance(&self, _peer: PeerId) -> (usize, usize, usize) {
        let bytes = 32768usize.saturating_sub(self.used.0);
        (bytes, bytes, 16usize.saturating_sub(self.used.1))
    }
    fn account(&mut self, _peer: PeerId, send: usize, receive: usize, records: usize) {
        self.used.0 += send + receive;
        self.used.1 += records;
    }
}
fn port() -> Port {
    Port {
        script: VecDeque::new(),
        sent: Vec::new(),
        consumed: Vec::new(),
        finished: false,
        terminal: 0,
        used: (0, 0),
    }
}
fn key() -> RuntimeKey {
    RuntimeKey {
        epoch: 1,
        incarnation: 1,
        stream: StreamKey {
            peer: PeerId(0),
            stream_id: 1,
        },
    }
}
fn pair(initial: Vec<u8>) -> (TcpConnection, UnixStream) {
    let (a, b) = UnixStream::pair().unwrap();
    a.set_nonblocking(true).unwrap();
    b.set_nonblocking(true).unwrap();
    let mut c = TcpConnection::new(
        key(),
        Socket::Unix(a),
        &Budget::new(1_000_000, 256),
        initial,
        crate::config::Limits::canonical(crate::config::Role::Client)
            .manager(crate::config::Role::Client),
    )
    .unwrap();
    c.opened = true;
    (c, b)
}
#[test]
fn fin_waits_for_and_consumes_the_shared_peer_record_quantum() {
    let (mut connection, host) = pair(Vec::new());
    host.shutdown(std::net::Shutdown::Write).unwrap();
    let mut port = port();
    port.used.1 = 16;
    connection.turn_port(&mut port).unwrap();
    assert!(connection.local_eof);
    assert!(!port.finished);
    port.used = (0, 15);
    connection.turn_port(&mut port).unwrap();
    assert!(port.finished);
    assert_eq!(port.used.1, 16);
    connection.turn_port(&mut port).unwrap();
    assert_eq!(port.used.1, 16);
}
#[test]
fn tiny_global_send_budget_backpressures_without_reading_or_canceling_other_socket() {
    let budget = Budget::new(8192 + 16384, 32);
    let make = || {
        let (a, b) = UnixStream::pair().unwrap();
        a.set_nonblocking(true).unwrap();
        b.set_nonblocking(true).unwrap();
        let mut c = TcpConnection::new(
            key(),
            Socket::Unix(a),
            &budget,
            Vec::new(),
            crate::config::Limits::canonical(crate::config::Role::Client)
                .manager(crate::config::Role::Client),
        )
        .unwrap();
        c.opened = true;
        (c, b)
    };
    let (mut first, mut a) = make();
    let (mut second, mut b) = make();
    a.write_all(&vec![1; 16384]).unwrap();
    b.write_all(b"other connection").unwrap();
    let mut one = port();
    one.script.push_back(0);
    first.turn_port(&mut one).unwrap();
    assert_eq!(budget.usage().bytes, 8192 + 16384);
    let mut two = port();
    second.turn_port(&mut two).unwrap();
    assert!(two.sent.is_empty());
    first.turn_port(&mut one).unwrap();
    one.used = (0, 0);
    first.turn_port(&mut one).unwrap();
    assert_eq!(budget.usage().bytes, 8192);
    second.turn_port(&mut two).unwrap();
    assert_eq!(two.sent, b"other connection");
    two.used = (0, 0);
    second.turn_port(&mut two).unwrap();
    assert_eq!(budget.usage().bytes, 8192);
    drop((first, second));
    assert_eq!(budget.usage(), crate::budget::Usage::default());
}
#[test]
fn larger_read_after_small_retained_send_is_charged_and_released_when_idle() {
    let (mut c, mut socket) = pair(vec![7; 8]);
    let mut port = port();
    c.turn_port(&mut port).unwrap();
    socket.write_all(&vec![9; 16000]).unwrap();
    port.used = (0, 0);
    c.turn_port(&mut port).unwrap();
    assert_eq!(port.sent.len(), 16008);
    assert_eq!(c._reservation.usage().bytes, 4096 + c.send.capacity());
    port.used = (0, 0);
    c.turn_port(&mut port).unwrap();
    assert_eq!(c.send.capacity(), 0);
    assert_eq!(c._reservation.usage().bytes, 4096);
}
#[test]
fn rejected_stream_drains_native_failure_after_core_retirement() {
    let (mut connection, mut socket) = pair(Vec::new());
    connection.opened = false;
    let failure = vec![5, 1, 0, 1, 0, 0, 0, 0, 0, 0];
    connection.failure = failure.clone();
    assert!(
        connection
            .event(Event::Rejected {
                reason: br#"{"v":6,"type":"tcp","error":"forbidden"}"#
                    .to_vec()
                    .into_boxed_slice(),
            })
            .unwrap()
    );
    assert!(connection.core_finished());
    let mut port = port();
    assert!(connection.turn_port(&mut port).is_err());
    let mut received = [0; 10];
    socket.read_exact(&mut received).unwrap();
    assert_eq!(received.as_slice(), failure);
    assert!(port.sent.is_empty());
    assert!(port.consumed.is_empty());
    assert!(!port.finished);
}
#[test]
fn partial_core_acceptance_retains_exact_suffix_and_fin_waits() {
    let initial: Vec<u8> = (0..241).map(|n| n as u8).collect();
    let (mut c, socket) = pair(initial.clone());
    socket.shutdown(Shutdown::Write).unwrap();
    let mut port = port();
    port.script = VecDeque::from([1, 0, 3, 0, 17, 2, 0, 218]);
    for _ in 0..16 {
        port.used = (0, 0);
        c.turn_port(&mut port).unwrap();
        if port.finished {
            break;
        }
        assert!(port.sent.len() <= initial.len());
    }
    assert_eq!(port.sent, initial);
    assert_eq!(c.uploaded, 241);
    assert!(port.finished);
}
#[test]
fn actual_shared_budget_shortage_preserves_box_for_exact_retry_and_fin() {
    let (mut old, _stalled) = pair(Vec::new());
    let budget = old.budget.clone();
    old.event(Event::Data {
        offset: 0,
        bytes: vec![7; 65536].into(),
    })
    .unwrap();
    let mut old_port = port();
    old.turn_port(&mut old_port).unwrap();
    assert!(old.downloaded > 0 && old.downloaded < 65536);
    let (socket, mut owner) = UnixStream::pair().unwrap();
    socket.set_nonblocking(true).unwrap();
    owner.set_nonblocking(true).unwrap();
    let mut next = TcpConnection::new(
        key(),
        Socket::Unix(socket),
        &budget,
        Vec::new(),
        crate::config::Limits::canonical(crate::config::Role::Client)
            .manager(crate::config::Role::Client),
    )
    .unwrap();
    next.opened = true;
    let expected = (0..4093).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    let payload: Box<[u8]> = expected.clone().into();
    let pointer = payload.as_ptr();
    let mut event = Event::Data {
        offset: 0,
        bytes: payload,
    };
    // An observed free budget can be occupied before the atomic reserve.
    assert!(budget.available().bytes > expected.len());
    let blocker = budget.reserve(budget.available().bytes, 1).unwrap();
    for _ in 0..2 {
        let failure = next
            .try_data(event)
            .expect_err("actual reservation must fail");
        assert!(failure.error.is_none());
        event = failure.event;
        let Event::Data { bytes, .. } = &event else {
            panic!("original DATA required")
        };
        assert_eq!(bytes.as_ptr(), pointer);
        assert_eq!(bytes.as_ref(), expected);
        assert!(next.receive.is_empty());
    }
    assert!(
        !old.event(Event::Closed {
            reason: skvoz_core::CloseReason::Cancelled
        })
        .unwrap()
    );
    drop(old);
    drop(blocker);
    assert!(next.try_data(event).is_ok());
    next.event(Event::RemoteFinished).unwrap();
    owner.shutdown(Shutdown::Write).unwrap();
    let mut out = Vec::new();
    let mut p = port();
    for _ in 0..8 {
        p.used = (0, 0);
        next.turn_port(&mut p).unwrap();
        let mut buffer = [0; 8192];
        match owner.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buffer[..n]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("{error}"),
        }
    }
    assert_eq!(out, expected);
    assert_eq!(p.consumed, [4093]);
    assert!(next.write_shutdown);
    assert!(p.finished);
    drop(next);
    assert_eq!(budget.usage(), crate::budget::Usage::default());
}
#[test]
fn partial_write_and_terminal_host_retention_keep_the_full_payload_charged() {
    let (mut connection, _owner) = pair(Vec::new());
    let budget = connection.budget.clone();
    let baseline = budget.usage();
    connection
        .event(Event::Data {
            offset: 0,
            bytes: vec![7; 65536].into(),
        })
        .unwrap();
    assert_eq!(
        connection
            .receive
            .front()
            .unwrap()
            ._reservation
            .usage()
            .bytes,
        65536
    );
    let mut port = port();
    connection.turn_port(&mut port).unwrap();
    assert!(connection.downloaded > 0 && connection.downloaded < 65536);
    assert!(port.consumed.is_empty());
    assert_eq!(
        connection
            .receive
            .front()
            .unwrap()
            ._reservation
            .usage()
            .bytes,
        65536
    );
    assert!(
        !connection
            .event(Event::Closed {
                reason: skvoz_core::CloseReason::Cancelled
            })
            .unwrap()
    );
    let retained = budget.usage();
    assert!(retained.bytes >= baseline.bytes + 65536);
    let (socket, _new_owner) = UnixStream::pair().unwrap();
    socket.set_nonblocking(true).unwrap();
    let mut next = TcpConnection::new(
        key(),
        Socket::Unix(socket),
        &budget,
        Vec::new(),
        crate::config::Limits::canonical(crate::config::Role::Client)
            .manager(crate::config::Role::Client),
    )
    .unwrap();
    next.event(Event::Data {
        offset: 0,
        bytes: vec![9; 65536].into(),
    })
    .unwrap();
    assert!(budget.usage().bytes >= retained.bytes + 65536);
    drop(connection);
    assert!(budget.usage().bytes >= 65536);
    drop(next);
    assert_eq!(budget.usage(), crate::budget::Usage::default());
}
#[test]
fn copying_remote_data_never_consumes_native_write_grants_exact_prefix() {
    let (mut c, mut socket) = pair(Vec::new());
    let mut port = port();
    let bytes: Vec<u8> = (0..65536).map(|n| (n % 251) as u8).collect();
    c.event(Event::Data {
        offset: 0,
        bytes: bytes.clone().into_boxed_slice(),
    })
    .unwrap();
    assert!(port.consumed.is_empty());
    assert_eq!(c.downloaded, 0);
    let mut received = Vec::new();
    for _ in 0..8 {
        port.used = (0, 0);
        c.turn_port(&mut port).unwrap();
        let mut chunk = [0u8; 32768];
        loop {
            match socket.read(&mut chunk) {
                Ok(n) if n > 0 => received.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                other => panic!("unexpected native read: {other:?}"),
            }
        }
        if received.len() == bytes.len() {
            break;
        }
    }
    assert_eq!(received, bytes);
    assert_eq!(port.consumed.last(), Some(&65536));
    assert_eq!(c.downloaded, 65536);
    assert!(port.consumed.windows(2).all(|p| p[0] < p[1]));
}
#[test]
fn remote_halfclose_waits_for_native_payload_then_preserves_reverse_direction() {
    let (mut c, mut socket) = pair(Vec::new());
    let mut port = port();
    c.event(Event::Data {
        offset: 0,
        bytes: b"payload".to_vec().into_boxed_slice(),
    })
    .unwrap();
    c.event(Event::RemoteFinished).unwrap();
    let mut byte = [0];
    assert_eq!(
        socket.read(&mut byte).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    c.turn_port(&mut port).unwrap();
    let mut data = [0; 7];
    socket.read_exact(&mut data).unwrap();
    assert_eq!(&data, b"payload");
    assert_eq!(socket.read(&mut byte).unwrap(), 0);
    socket.write_all(b"reply").unwrap();
    c.turn_port(&mut port).unwrap();
    assert_eq!(&port.sent, b"reply");
    assert!(!port.finished);
}
#[test]
fn graceful_core_closed_retains_already_received_native_bytes() {
    let (mut c, mut socket) = pair(Vec::new());
    let mut port = port();
    c.event(Event::Data {
        offset: 0,
        bytes: b"tail".to_vec().into_boxed_slice(),
    })
    .unwrap();
    assert!(
        c.event(Event::Closed {
            reason: skvoz_core::CloseReason::Finished
        })
        .unwrap()
    );
    assert_eq!(c.turn_port(&mut port), Err(NetworkError::InvalidState));
    let mut tail = [0; 4];
    socket.read_exact(&mut tail).unwrap();
    assert_eq!(&tail, b"tail");
    assert!(c.gracefully_finished());
    assert!(port.consumed.is_empty());
}
#[test]
fn repeated_or_early_remote_fin_data_is_terminal() {
    let (mut c, _) = pair(Vec::new());
    c.event(Event::RemoteFinished).unwrap();
    assert!(
        c.event(Event::Data {
            offset: 0,
            bytes: b"late".to_vec().into_boxed_slice()
        })
        .is_err()
    );
}
#[test]
fn exhausted_receive_record_quantum_prevents_an_extra_send() {
    let (mut c, _socket) = pair(b"send-after-writes".to_vec());
    let mut port = port();
    for offset in 0..16 {
        c.event(Event::Data {
            offset,
            bytes: vec![1].into_boxed_slice(),
        })
        .unwrap();
    }
    c.turn_port(&mut port).unwrap();
    assert_eq!(port.used.1, 16);
    assert!(port.sent.is_empty());
    assert_eq!(c.send_cursor, 0);
    port.used = (0, 0);
    c.turn_port(&mut port).unwrap();
    assert_eq!(port.sent, b"send-after-writes");
}
#[test]
fn terminal_quantum_defers_fin_without_losing_native_eof() {
    let mut connections = Vec::new();
    let mut owners = Vec::new();
    let mut port = port();
    for _ in 0..5 {
        let (connection, owner) = pair(Vec::new());
        owner.shutdown(Shutdown::Write).unwrap();
        connections.push(connection);
        owners.push(owner);
    }
    for connection in &mut connections {
        connection.turn_port(&mut port).unwrap();
    }
    assert_eq!(port.terminal, 4);
    assert!(connections[..4].iter().all(|c| c.finished));
    assert!(connections[4].local_eof && !connections[4].finished);
    port.terminal = 0;
    port.used = (0, 0);
    connections[4].turn_port(&mut port).unwrap();
    assert!(connections[4].finished);
    assert_eq!(port.terminal, 1);
}
