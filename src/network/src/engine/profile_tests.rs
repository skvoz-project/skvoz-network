use super::*;

#[test]
fn backend_abort_bypasses_only_cancelled_data_preserving_other_fifo_and_fin() {
    let key = |id| RuntimeKey {
        epoch: 1,
        incarnation: 1,
        stream: skvoz_core::StreamKey {
            peer: PeerId(0),
            stream_id: id,
        },
    };
    let data = |id, offset| BackendEvent::Tcp {
        key: key(id),
        event: Event::Data {
            offset,
            bytes: Box::new([id as u8]),
        },
    };
    let mut queue = VecDeque::from([
        data(2, 0),
        BackendEvent::TcpOpen {
            key: key(1),
            host: "example.org".into(),
            port: 443,
        },
        data(1, 0),
        BackendEvent::Tcp {
            key: key(2),
            event: Event::RemoteFinished,
        },
        BackendEvent::Tcp {
            key: key(2),
            event: Event::Closed {
                reason: skvoz_core::CloseReason::Finished,
            },
        },
        BackendEvent::Tcp {
            key: key(1),
            event: Event::Closed {
                reason: skvoz_core::CloseReason::Cancelled,
            },
        },
        data(3, 0),
        data(3, 1),
    ]);
    assert!(
        matches!(extract_backend_abort(&mut queue), Some(BackendEvent::Tcp { key: k, event: Event::Closed { reason: skvoz_core::CloseReason::Cancelled } }) if k == key(1))
    );
    assert_eq!(queue.len(), 5);
    assert!(extract_backend_abort(&mut queue).is_none());
    assert!(
        matches!(queue.pop_front(), Some(BackendEvent::Tcp { key: k, event: Event::Data { offset: 0, .. } }) if k == key(2))
    );
    assert!(matches!(
        queue.pop_front(),
        Some(BackendEvent::Tcp {
            event: Event::RemoteFinished,
            ..
        })
    ));
    assert!(matches!(
        queue.pop_front(),
        Some(BackendEvent::Tcp {
            event: Event::Closed {
                reason: skvoz_core::CloseReason::Finished
            },
            ..
        })
    ));
    for offset in 0..2 {
        assert!(
            matches!(queue.pop_front(), Some(BackendEvent::Tcp { key: k, event: Event::Data { offset: n, .. } }) if k == key(3) && n == offset)
        );
    }
    assert!(queue.is_empty());
}
#[test]
fn fixed_bookkeeping_backs_backend_and_pending_cancellation_keys() {
    // Both arrays have finite capacities, without tree nodes or a copy.
    let bytes = BACKEND_QUEUE_CAPACITY * (std::mem::size_of::<BackendEvent>() + 512)
        + 2048 * std::mem::size_of::<RuntimeKey>();
    assert!(bytes <= 524288, "bookkeeping requires {bytes} bytes");
}

#[test]
fn canonical_send_pool_and_stream_profile_with_finite_role_bounds() {
    let canonical = EngineConfig::default().core_limits(false);
    assert!(validate_core_profile(canonical, false).is_ok());
    let mut smaller = canonical;
    smaller.max_streams = 2;
    smaller.max_streams_per_peer = 2;
    smaller.receive_budget = 131072;
    smaller.receive_budget_per_peer = 131072;
    assert!(validate_core_profile(smaller, false).is_ok());
    for (case, mut invalid) in [canonical; 9].into_iter().enumerate() {
        match case {
            0 => invalid.stream.receive_window = 16384,
            1 => invalid.stream.max_frame = 1,
            2 => invalid.max_peers = 2,
            3 => invalid.max_streams = canonical.max_streams + 1,
            4 => invalid.send_budget_per_peer = CONTROL_MAX + 7,
            5 => invalid.receive_budget_per_peer = 65536,
            6 => invalid.stream.max_pending_frames = 129,
            7 => invalid.stream.max_metadata = 513,
            8 => invalid.send_budget = CONTROL_MAX + 8,
            _ => unreachable!(),
        }
        assert_eq!(
            validate_core_profile(invalid, false),
            Err(NetworkError::InvalidConfiguration)
        );
    }
    assert!(!compatible_peer_profile(None));
    assert!(!compatible_peer_profile(Some(PeerLimits {
        receive_window: 65536,
        max_frame: 1,
    })));
    assert!(!compatible_peer_profile(Some(PeerLimits {
        receive_window: 16384,
        max_frame: 16384,
    })));
    assert!(compatible_peer_profile(Some(PeerLimits {
        receive_window: 65536,
        max_frame: 16384,
    })));
}
