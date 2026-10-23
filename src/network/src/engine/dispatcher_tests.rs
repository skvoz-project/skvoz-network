use super::*;
use skvoz_core::StreamKey;
fn key(peer: u64, stream_id: u64) -> RuntimeKey {
    RuntimeKey {
        epoch: 77,
        incarnation: 2,
        stream: StreamKey {
            peer: PeerId(peer),
            stream_id,
        },
    }
}
fn id() -> SessionId {
    "0123456789abcdef0123456789abcdef"
        .to_owned()
        .try_into()
        .unwrap()
}
#[test]
fn busy_peer_does_not_delay_single_packet_from_healthy_peer() {
    let mut dispatcher = PacketDispatch::default();
    for end in 1..=256 {
        dispatcher.push(ReceivedPacket {
            session: id(),
            key: key(1, 4),
            end_offset: end,
            packet: vec![0; 20],
        });
    }
    dispatcher.push(ReceivedPacket {
        session: id(),
        key: key(2, 4),
        end_offset: 28,
        packet: vec![0; 20],
    });
    assert_eq!(dispatcher.pop().unwrap().key.stream.peer, PeerId(1));
    assert_eq!(dispatcher.pop().unwrap().key.stream.peer, PeerId(2));
    dispatcher.remove(PeerId(1));
    assert!(dispatcher.pop().is_none());
    assert!(dispatcher.queues.is_empty());
}
#[test]
fn server_accept_local_opened_validates_without_client_transition() {
    let control = key(1, 0);
    let data = key(1, 2);
    let channels = BTreeMap::from([(0, (data, true))]);
    assert!(server_accept_matches(
        control,
        control,
        &id(),
        &channels,
        &Accept::IpSession {
            v: crate::NETWORK_VERSION,
            session: id()
        }
    ));
    assert!(server_accept_matches(
        data,
        control,
        &id(),
        &channels,
        &Accept::IpData {
            v: crate::NETWORK_VERSION,
            session: id(),
            channel: 0
        }
    ));
    assert!(!server_accept_matches(
        key(2, 2),
        control,
        &id(),
        &channels,
        &Accept::IpData {
            v: crate::NETWORK_VERSION,
            session: id(),
            channel: 0
        }
    ));
    assert!(!server_accept_matches(
        data,
        control,
        &id(),
        &channels,
        &Accept::IpData {
            v: crate::NETWORK_VERSION,
            session: id(),
            channel: 1
        }
    ));
}
