use super::*;

fn session(send_bytes: usize, send_records: usize) -> Session {
    let id = SessionId::try_from("01".repeat(16)).unwrap();
    Session {
        id: id.clone(),
        control: RuntimeKey {
            epoch: 1,
            incarnation: 1,
            stream: skvoz_core::StreamKey {
                peer: PeerId(0),
                stream_id: 1,
            },
        },
        config: Some(SessionConfig {
            session: id,
            families: vec![4],
            source_grants: vec!["192.0.2.10/32".parse().unwrap()],
            routes: vec!["0.0.0.0/0".parse().unwrap()],
            dns_servers: vec!["192.0.2.53".parse().unwrap()],
            mtu: 1500,
            channels: 1,
            packet_queue_bytes: 262144,
            packet_queue_records: 256,
            setup_timeout_ms: 15000,
            egress: Egress {
                ipv4: "nat44".into(),
                ipv6: "none".into(),
            },
        }),
        requested: None,
        state: SessionState::Active,
        channels: BTreeMap::new(),
        local_ready: true,
        deadline: Instant::now(),
        seed: 0,
        send_bytes,
        send_records,
        receive_bytes: 0,
        receive_records: 0,
    }
}

#[test]
fn tun_ingress_waits_for_a_complete_mtu_record_and_record_slot() {
    let mut peer = session(262144 - 1508, 255);
    assert!(NetworkEngine::packet_input_ready_for([&peer].into_iter()));
    peer.send_bytes += 1;
    assert!(!NetworkEngine::packet_input_ready_for([&peer].into_iter()));
    peer.send_bytes = 0;
    peer.send_records = 256;
    assert!(!NetworkEngine::packet_input_ready_for([&peer].into_iter()));
}

#[test]
fn stalled_peer_preserves_tun_reads_for_healthy_peer_and_retirement() {
    let blocked = session(262144, 256);
    let mut healthy = session(0, 0);
    assert!(NetworkEngine::packet_input_ready_for(
        [&blocked, &healthy].into_iter()
    ));
    assert!(NetworkEngine::packet_input_ready_for(
        [&healthy, &blocked].into_iter()
    ));
    healthy.state = SessionState::Preparing;
    assert!(!NetworkEngine::packet_input_ready_for(
        [&blocked, &healthy].into_iter()
    ));
    assert!(NetworkEngine::packet_input_ready_for(
        [&healthy].into_iter()
    ));
    assert!(NetworkEngine::packet_input_ready_for(std::iter::empty()));
}
