use super::*;
use std::os::fd::AsFd;
fn output() -> Arc<Output> {
    Arc::new(Output {
        state: Mutex::new(OutputState {
            queue: VecDeque::new(),
            closed: false,
        }),
        changed: Condvar::new(),
        budget: Budget::new(8192, 16),
        api: Budget::new(8192, 16),
    })
}
#[test]
fn observable_response_releases_command_admission_but_events_do_not() {
    let out = output();
    let busy = Arc::new(AtomicBool::new(true));
    out.push(OwnedMessage {
        json: br#"{"event":"STATS","data":{}}"#.to_vec(),
        fd: None,
    })
    .unwrap();
    out.poll(8192, Duration::ZERO).unwrap();
    assert!(busy.load(Ordering::Acquire));
    let (observed, acknowledged) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let worker_out = out.clone();
        let worker_busy = busy.clone();
        scope.spawn(move || {
            for id in 1..=128 {
                worker_out
                    .push_response(
                        OwnedMessage {
                            json: serde_json::to_vec(
                                &json!({"v":1,"id":id,"error":null,"result":{}}),
                            )
                            .unwrap(),
                            fd: None,
                        },
                        &worker_busy,
                    )
                    .unwrap();
                acknowledged.recv().unwrap();
            }
        });
        for id in 1..=128 {
            let response: Value =
                serde_json::from_slice(&out.poll(8192, Duration::from_secs(1)).unwrap().json)
                    .unwrap();
            assert_eq!(response["id"], id);
            assert!(
                busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            );
            observed.send(()).unwrap();
        }
    });
    assert_eq!(out.budget.usage().bytes, 0);
}
#[test]
fn transient_request_coalescing_preserves_terminal_advertised_records_and_order() {
    let out = output();
    let message = |seq, id, result| {
        OwnedMessage { json: serde_json::to_vec(&json!({"v":1,"seq":seq,"event":"REQUEST","data":{"id":id,"result":result},"fd_count":0})).unwrap(), fd:None }
    };
    out.push(message(1, 1, "opening")).unwrap();
    out.push(message(2, 2, "opening")).unwrap();
    out.push(message(3, 1, "active")).unwrap();
    let first: Value =
        serde_json::from_slice(&out.poll(32768, Duration::ZERO).unwrap().json).unwrap();
    assert_eq!(first["seq"], 2);
    assert!(matches!(
        out.poll(1, Duration::ZERO),
        Err(PollError::InsufficientBuffer { .. })
    ));
    out.push(message(4, 1, "finished")).unwrap();
    out.push(message(5, 1, "cancelled")).unwrap();
    let values: Vec<Value> = (0..3)
        .map(|_| serde_json::from_slice(&out.poll(32768, Duration::ZERO).unwrap().json).unwrap())
        .collect();
    assert_eq!(
        values
            .iter()
            .map(|v| v["seq"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![3, 4, 5]
    );
    assert_eq!(out.budget.usage(), crate::budget::Usage::default());
}
#[test]
fn insufficient_buffer_retains_authoritative_fd_and_credits_until_transfer() {
    let output = output();
    let (sender, mut receiver) = UnixStream::pair().unwrap();
    sender.set_nonblocking(true).unwrap();
    receiver.set_nonblocking(true).unwrap();
    output
        .push(OwnedMessage {
            json: b"owned-message".to_vec(),
            fd: Some(sender.into()),
        })
        .unwrap();
    let usage = output.budget.usage();
    for _ in 0..3 {
        assert!(matches!(
            output.poll(3, Duration::ZERO),
            Err(PollError::InsufficientBuffer { required: 13 })
        ));
        assert_eq!(output.budget.usage(), usage);
    }
    let message = output.poll(13, Duration::ZERO).unwrap();
    assert!(message.fd.is_some());
    assert_eq!(output.budget.usage(), crate::budget::Usage::default());
    assert!(matches!(
        output.poll(13, Duration::ZERO),
        Err(PollError::Timeout)
    ));
    use std::io::Read;
    let mut byte = [0; 1];
    assert_eq!(
        receiver.read(&mut byte).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    drop(message);
    assert_eq!(receiver.read(&mut byte).unwrap(), 0);
}
#[test]
fn queued_fd_closes_when_owner_is_destroyed_and_stats_coalesce() {
    let output = output();
    let (sender, mut receiver) = UnixStream::pair().unwrap();
    receiver.set_nonblocking(true).unwrap();
    output
        .push(OwnedMessage {
            json: b"fd".to_vec(),
            fd: Some(sender.into()),
        })
        .unwrap();
    for seq in 1..=10 {
        output
            .push(OwnedMessage {
                json: serde_json::to_vec(
                    &json!({"v":1,"seq":seq,"event":"STATS","data":{"counters":{}},"fd_count":0}),
                )
                .unwrap(),
                fd: None,
            })
            .unwrap();
    }
    assert_eq!(output.state.lock().unwrap().queue.len(), 2);
    output.close();
    drop(output);
    use std::io::Read;
    assert_eq!(receiver.read(&mut [0]).unwrap(), 0);
}
#[test]
fn client_rejects_helper_before_startup_or_any_connection() {
    let config =
        StartupConfig::parse_json(include_bytes!("../../tests/fixtures/client-startup.json"))
            .unwrap();
    let (a, _b) = UnixStream::pair().unwrap();
    a.set_nonblocking(true).unwrap();
    assert!(matches!(
        RuntimeHandle::start(config, Some(a.into())),
        Err(RuntimeFailure::InvalidArgument)
    ));
}
#[test]
fn handshake_response_is_independent_of_pending_core_startup() {
    let config =
        StartupConfig::parse_json(include_bytes!("../../tests/fixtures/client-startup.json"))
            .unwrap();
    let output = output();
    let (sender, mut receiver) = mpsc::channel(1);
    let busy = Arc::new(AtomicBool::new(true));
    let stop = Arc::new(AtomicBool::new(false));
    let mut actor = Actor::new(
        config,
        None,
        output.clone(),
        busy.clone(),
        stop,
        Arc::new(Notify::new()),
        Arc::new(Mutex::new(None)),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let budget = Budget::new(65536, 4);
    let request = Request::parse_json(
        br#"{"v":1,"id":1,"op":"HELLO","args":{"api":1,"network":6},"fd_count":0}"#,
    )
    .unwrap();
    sender
        .try_send(Command {
            request,
            fd: None,
            _reservation: budget.reserve(1024, 1).unwrap(),
        })
        .unwrap();
    // No NATS future is run: the same actor command path owns the response.
    runtime
        .block_on(actor.command(receiver.try_recv().unwrap()))
        .unwrap();
    assert!(actor.engine.is_none());
    let message = output.poll(32768, Duration::ZERO).unwrap();
    let response: Response = serde_json::from_slice(&message.json).unwrap();
    assert_eq!(response.id, 1);
    assert!(response.error.is_none());
    assert_eq!(response.result.unwrap()["api"], 1);
    assert!(!busy.load(Ordering::Acquire));
}
#[test]
fn output_overflow_does_not_leak_fd() {
    let output = output();
    let (a, mut b) = UnixStream::pair().unwrap();
    b.set_nonblocking(true).unwrap();
    assert_eq!(
        output.push(OwnedMessage {
            json: vec![0; 9000],
            fd: Some(a.into())
        }),
        Err(RuntimeFailure::Overloaded)
    );
    use std::io::Read;
    assert_eq!(b.read(&mut [0]).unwrap(), 0);
    assert_eq!(output.budget.usage(), crate::budget::Usage::default());
}
#[test]
fn fd_mismatch_is_terminal_and_closes_duplicate() {
    let config =
        StartupConfig::parse_json(include_bytes!("../../tests/fixtures/client-startup.json"))
            .unwrap();
    let (mut owner, observer) = UnixStream::pair().unwrap();
    owner.set_nonblocking(true).unwrap();
    observer.set_nonblocking(true).unwrap();
    let output = output();
    let (commands, _receiver) = mpsc::channel(1);
    let (_done_sender, done) = std::sync::mpsc::channel();
    let mut handle = RuntimeHandle {
        diagnostics: Arc::new(diagnostics::Mailbox::new()),
        commands: Some(commands),
        output,
        budget: Budget::new(2_000_000, 64),
        busy: Arc::new(AtomicBool::new(false)),
        stop: Arc::new(AtomicBool::new(false)),
        wake: Arc::new(Notify::new()),
        shutdown_deadline: Arc::new(Mutex::new(None)),
        done,
        thread: None,
        last_id: 0,
    };
    assert!(!handle.diagnostics(false).unwrap().enabled);
    let duplicate = skvoz_network_native::duplicate_cloexec(observer.as_fd()).unwrap();
    drop(observer);
    assert_eq!(
        handle.request_json(
            br#"{"v":1,"id":1,"op":"HELLO","args":{"api":1,"network":6},"fd_count":0}"#,
            Some(duplicate)
        ),
        Err(RuntimeFailure::InvalidArgument)
    );
    assert_eq!(
        handle.request_json(b"{}", None),
        Err(RuntimeFailure::Closed)
    );
    assert!(matches!(
        handle.diagnostics(true),
        Err(RuntimeFailure::Closed)
    ));
    use std::io::Read;
    assert_eq!(owner.read(&mut [0]).unwrap(), 0);
    drop(config);
}
#[test]
fn advertised_stats_are_retained_exactly_until_delivery() {
    let output = output();
    let original =
        serde_json::to_vec(&json!({"event":"STATS","seq":1,"data":{"counters":{"uploaded":1}}}))
            .unwrap();
    output
        .push(OwnedMessage {
            json: original.clone(),
            fd: None,
        })
        .unwrap();
    assert!(matches!(
        output.poll(1, Duration::ZERO),
        Err(PollError::InsufficientBuffer { .. })
    ));
    output
        .push(OwnedMessage {
            json: serde_json::to_vec(
                &json!({"event":"STATS","seq":2,"data":{"counters":{"uploaded":2000}}}),
            )
            .unwrap(),
            fd: None,
        })
        .unwrap();
    assert_eq!(
        output.poll(original.len(), Duration::ZERO).unwrap().json,
        original
    );
    assert!(matches!(
        output.poll(32768, Duration::ZERO),
        Err(PollError::Timeout)
    ));
}
#[test]
fn timed_out_destroy_discards_queued_fd_and_forbids_late_actor_push() {
    let output = output();
    let (sender, mut receiver) = UnixStream::pair().unwrap();
    receiver.set_nonblocking(true).unwrap();
    output
        .push(OwnedMessage {
            json: b"fd".to_vec(),
            fd: Some(sender.into()),
        })
        .unwrap();
    output.discard();
    assert_eq!(
        output.push(OwnedMessage {
            json: b"late".to_vec(),
            fd: None
        }),
        Err(RuntimeFailure::Closed)
    );
    use std::io::Read;
    assert_eq!(receiver.read(&mut [0]).unwrap(), 0);
    assert!(matches!(
        output.poll(32768, Duration::ZERO),
        Err(PollError::Closed)
    ));
}
#[test]
fn canceled_resolver_waiter_keeps_actual_worker_permit_and_reservation() {
    let budget = Budget::new(65536, 1);
    let slots = Arc::new(Semaphore::new(1));
    let permit = slots.clone().try_acquire_owned().unwrap();
    let reservation = budget.reserve(65536, 1).unwrap();
    let alive = Arc::new(AtomicBool::new(false));
    let liveness = ResolverWorker::new(alive.clone());
    let key = RuntimeKey {
        epoch: 1,
        incarnation: 1,
        stream: skvoz_core::StreamKey {
            peer: PeerId(1),
            stream_id: 7,
        },
    };
    let mut owned = Connecting {
        key,
        future: Some(Box::pin(std::future::pending())),
        host: "example.org".into(),
        port: 443,
        _reservation: Budget::new(1024, 4).reserve(1024, 4).unwrap(),
        deadline: Instant::now() + Duration::from_secs(3),
        resolver_alive: alive,
        cancelled: false,
    };
    let (started_send, started) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let (done_send, done) = std::sync::mpsc::channel();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let worker = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _reservation = reservation;
            started_send.send(()).unwrap();
            gate.recv().unwrap();
            drop((_permit, _reservation, liveness));
            done_send.send(()).unwrap();
        });
        assert!(tokio::time::timeout(Duration::ZERO, worker).await.is_err());
    });
    started.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(slots.clone().try_acquire_owned().is_err());
    assert_eq!(budget.usage().bytes, 65536);
    assert!(owned.retain_owned(false));
    assert!(owned.cancelled && owned.future.is_none());
    assert_eq!(owned.key, key); // terminal proof keeps the exact old owner
    release.send(()).unwrap();
    done.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(slots.available_permits(), 1);
    assert_eq!(budget.usage(), crate::budget::Usage::default());
    assert!(!owned.retain_owned(false));
    runtime.shutdown_background();
}
#[test]
fn initial_deadline_is_not_reapplied_to_healthy_runtime_recovery() {
    let config =
        StartupConfig::parse_json(include_bytes!("../../tests/fixtures/client-startup.json"))
            .unwrap();
    let output = output();
    let mut actor = Actor::new(
        config,
        None,
        output,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        Arc::new(Notify::new()),
        Arc::new(Mutex::new(None)),
    );
    actor.ready_deadline = Instant::now() - Duration::from_secs(1);
    assert!(!actor.ever_ready);
    actor.state("ready", None).unwrap();
    actor
        .state("starting", Some(ApiError::NetworkUnavailable))
        .unwrap();
    assert!(actor.ever_ready);
    assert_eq!(actor.lifecycle, "starting");
    assert!(Instant::now() > actor.ready_deadline);
}
#[test]
fn bounded_helper_reserve_rejection_is_nonfatal_but_activation_failure_is_fatal() {
    let config =
        StartupConfig::parse_json(include_bytes!("../../tests/fixtures/client-startup.json"))
            .unwrap();
    let (a, b) = UnixStream::pair().unwrap();
    a.set_nonblocking(true).unwrap();
    b.set_nonblocking(true).unwrap();
    let mut peer = IncrementalUnix::from_owned_fd(b.into()).unwrap();
    let mut actor = Actor::new(
        config,
        Some(IncrementalUnix::from_owned_fd(a.into()).unwrap()),
        output(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        Arc::new(Notify::new()),
        Arc::new(Mutex::new(None)),
    );
    let key = RuntimeKey {
        epoch: 1,
        incarnation: 1,
        stream: skvoz_core::StreamKey {
            peer: PeerId(1),
            stream_id: 1,
        },
    };
    let session = SessionId::random().unwrap();
    actor.helper_id = 1;
    actor.helper_pending = Some(HelperPending {
        request: local_api::HelperRequest::new(
            1,
            local_api::HelperOperation::ReservePeer,
            json!({"peer":"1","session":session,"families":[4],"mtu":1500}),
        )
        .unwrap(),
        kind: HelperKind::Reserve {
            key,
            session: session.clone(),
            families: vec![4],
            mtu: 1500,
            channels: 1,
        },
        _reservation: actor.budget.reserve(4096, 1).unwrap(),
        deadline: Instant::now() + Duration::from_secs(5),
    });
    peer.queue_frame(
        serde_json::to_vec(&local_api::HelperResponse::failure(1, "overloaded")).unwrap(),
        None,
    )
    .unwrap();
    peer.try_flush().unwrap();
    assert_eq!(actor.helper_turn(), Ok(()));
    assert!(actor.helper_pending.is_none());
    actor.helper_pending = Some(HelperPending {
        request: local_api::HelperRequest::new(
            2,
            local_api::HelperOperation::ActivatePeer,
            json!({"peer":"1","session":session}),
        )
        .unwrap(),
        kind: HelperKind::Activate { key, session },
        _reservation: actor.budget.reserve(4096, 1).unwrap(),
        deadline: Instant::now() + Duration::from_secs(5),
    });
    peer.queue_frame(
        serde_json::to_vec(&local_api::HelperResponse::failure(2, "overloaded")).unwrap(),
        None,
    )
    .unwrap();
    peer.try_flush().unwrap();
    assert_eq!(actor.helper_turn(), Err(RuntimeFailure::Internal));
}
