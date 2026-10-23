#![cfg(feature = "real-nats")]
//! Raw peer bypasses the client validator; the receiving engine owns enforcement.
#[path = "support/adversary.rs"]
mod adversary;
#[path = "support/nats.rs"]
mod nats;
use skvoz_core::{PeerId, runtime::NatsRuntime};
use skvoz_network::{EngineConfig, EngineRole, FamilyPolicy, NetworkEngine, SessionState};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

#[tokio::test]
async fn raw_peer_cannot_cross_session_or_source_grants_and_valid_packet_still_progresses() {
    let fixture = skvoz_network_qualification::TlsFixture::new();
    let config = EngineConfig::default();
    let mut server = NatsRuntime::connect(
        nats::config(&fixture, 0, "ip-isolation").unwrap(),
        config.core_limits(true),
    )
    .await
    .unwrap();
    let mut client = NatsRuntime::connect(
        nats::config(&fixture, 1, "ip-isolation").unwrap(),
        config.core_limits(false),
    )
    .await
    .unwrap();
    let mut other_config = nats::config(&fixture, 1, "ip-isolation").unwrap();
    other_config.id = PeerId(2);
    other_config.authentication.username = "peer2".into();
    let mut other = NatsRuntime::connect(other_config, config.core_limits(false))
        .await
        .unwrap();
    let until = Instant::now() + Duration::from_secs(8);
    while !client.peer_ready(PeerId(0))
        || !other.peer_ready(PeerId(0))
        || !server.peer_ready(PeerId(1))
        || !server.peer_ready(PeerId(2))
    {
        assert!(Instant::now() < until, "three-peer JOIN deadline");
        client.turn(Duration::ZERO).await.unwrap();
        other.turn(Duration::ZERO).await.unwrap();
        server.turn(Duration::from_millis(1)).await.unwrap();
    }
    let grant = |peer| {
        serde_json::from_value(serde_json::json!({
        "session":format!("{peer:032x}"),"families":[4],
        "source_grants":[format!("192.0.2.{}/32", peer + 9)],"routes":["0.0.0.0/0"],"dns_servers":["192.0.2.53"],
        "mtu":1500,"channels":1,"packet_queue_bytes":262144,"packet_queue_records":256,
        "setup_timeout_ms":15000,"egress":{"ipv4":"nat44","ipv6":"none"}
    })).unwrap()
    };
    let mut server = NetworkEngine::new(
        server,
        EngineRole::Server {
            grants: BTreeMap::from([(PeerId(1), grant(1)), (PeerId(2), grant(2))]),
        },
        config,
    )
    .unwrap();
    let mut other = NetworkEngine::new(other, EngineRole::Client, config).unwrap();
    other
        .open_ip(PeerId(0), vec![4], FamilyPolicy::RequireAll, 1500, 1)
        .unwrap();
    let until = Instant::now() + Duration::from_secs(8);
    let mut local_ready = false;
    let foreign = loop {
        assert!(Instant::now() < until, "foreign live session deadline");
        client.turn(Duration::ZERO).await.unwrap();
        server.drive(Duration::ZERO).await.unwrap();
        other.drive(Duration::from_millis(1)).await.unwrap();
        if let Some(session) = other.sessions().first() {
            if session.state == SessionState::Preparing && session.config.is_some() && !local_ready
            {
                other.local_ready(&session.session).unwrap();
                local_ready = true;
            } else if session.state == SessionState::Active {
                break session.session.clone();
            }
        }
    };
    adversary::run(client, server, other, foreign)
        .await
        .unwrap();
}
