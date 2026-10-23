//! Bounded endpoint payloads and authoritative DNS; no load generator.
use super::{E, NATS_SHA, Process, ROOT, U, V, command, private, route};
use serde_json::Value;
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    fs,
    io::{self, Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket},
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};
const NAME: &str = "payload.skvoz.test";
fn payload() -> Vec<u8> {
    (0..8192).map(|i| (i * 31 + i / 255) as u8).collect()
}
fn tcp(address: SocketAddr, interface: Option<&str>) -> io::Result<TcpStream> {
    let socket = Socket::new(
        Domain::for_address(address),
        Type::STREAM,
        Some(Protocol::TCP),
    )?;
    if let Some(device) = interface {
        socket.bind_device(Some(device.as_bytes()))?;
    }
    socket.connect_timeout(&address.into(), Duration::from_millis(800))?;
    let stream: TcpStream = socket.into();
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    Ok(stream)
}
fn udp(address: SocketAddr, interface: Option<&str>) -> io::Result<UdpSocket> {
    let socket = Socket::new(
        Domain::for_address(address),
        Type::DGRAM,
        Some(Protocol::UDP),
    )?;
    if let Some(device) = interface {
        socket.bind_device(Some(device.as_bytes()))?;
    }
    let bind: SocketAddr = if address.is_ipv4() {
        "0.0.0.0:0".parse().unwrap()
    } else {
        "[::]:0".parse().unwrap()
    };
    socket.bind(&bind.into())?;
    let socket: UdpSocket = socket.into();
    socket.set_read_timeout(Some(Duration::from_millis(800)))?;
    socket.set_write_timeout(Some(Duration::from_millis(800)))?;
    socket.connect(address)?;
    Ok(socket)
}
fn query(kind: u16) -> Vec<u8> {
    let mut bytes = vec![0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in "physical.skvoz.test".split('.') {
        bytes.push(label.len() as u8);
        bytes.extend_from_slice(label.as_bytes());
    }
    bytes.push(0);
    bytes.extend_from_slice(&kind.to_be_bytes());
    bytes.extend_from_slice(&1_u16.to_be_bytes());
    bytes
}
fn dns(packet: &[u8], peer: IpAddr) -> Vec<u8> {
    assert!((13..=4096).contains(&packet.len()));
    assert_eq!(&packet[4..6], &[0, 1]);
    let mut pos = 12;
    let mut labels = Vec::new();
    while packet[pos] != 0 {
        let size = usize::from(packet[pos]);
        assert!((1..=63).contains(&size));
        pos += 1;
        assert!(pos + size < packet.len());
        labels.push(
            std::str::from_utf8(&packet[pos..pos + size])
                .unwrap()
                .to_ascii_lowercase(),
        );
        pos += size;
    }
    pos += 1;
    assert!(pos + 4 <= packet.len());
    let kind = u16::from_be_bytes(packet[pos..pos + 2].try_into().unwrap());
    assert_eq!(&packet[pos + 2..pos + 4], &[0, 1]);
    pos += 4;
    let name = labels.join(".");
    let accepted = matches!(name.as_str(), NAME | "physical.skvoz.test") && matches!(kind, 1 | 28);
    if name == NAME {
        assert_eq!(
            peer,
            format!("{E}20").parse::<IpAddr>().unwrap(),
            "resolved DNS must arrive through NAT44"
        );
    }
    let mut reply = vec![
        packet[0],
        packet[1],
        0x85,
        if accepted { 0x80 } else { 0x83 },
        0,
        1,
        0,
        u8::from(accepted),
        0,
        0,
        0,
        0,
    ];
    reply.extend_from_slice(&packet[12..pos]);
    if accepted {
        let ip: IpAddr = if kind == 1 {
            format!("{E}30").parse().unwrap()
        } else {
            format!("{V}30").parse().unwrap()
        };
        let octets = match ip {
            IpAddr::V4(ip) => ip.octets().to_vec(),
            IpAddr::V6(ip) => ip.octets().to_vec(),
        };
        reply.extend_from_slice(&[0xc0, 0x0c]);
        reply.extend_from_slice(&kind.to_be_bytes());
        reply.extend_from_slice(&[0, 1, 0, 0, 0, 60]);
        reply.extend_from_slice(&(octets.len() as u16).to_be_bytes());
        reply.extend_from_slice(&octets);
    }
    println!("DNS type={kind} accepted={accepted}");
    reply
}
fn direct_dns(address: SocketAddr, interface: &str) -> io::Result<()> {
    let socket = udp(address, Some(interface))?;
    socket.send(&query(1))?;
    let mut response = [0; 4096];
    let n = socket.recv(&mut response)?;
    if n < 12 || response[..2] != [0x12, 0x34] || response[7] != 1 {
        return Err(io::Error::other("invalid fixture DNS answer"));
    }
    Ok(())
}
pub(super) fn physical(interface: &str, allowed: bool) {
    for address in [format!("{E}30:9000"), format!("[{V}30]:9000")] {
        let result = tcp(address.parse().unwrap(), Some(interface));
        assert_eq!(
            result.is_ok(),
            allowed,
            "physical TCP {address}: {result:?}"
        );
    }
    for address in [format!("{E}30:53"), format!("[{V}30]:53")] {
        assert_eq!(
            direct_dns(address.parse().unwrap(), interface).is_ok(),
            allowed,
            "physical DNS {address}"
        );
    }
}
pub(super) fn broker_exception(interface: &str) {
    let mut stream = tcp(format!("{U}2:4222").parse().unwrap(), Some(interface)).unwrap();
    let mut header = [0; 5];
    stream.read_exact(&mut header).unwrap();
    assert_eq!(&header, b"INFO ");
}
fn echo(address: SocketAddr, expected: &str) {
    let mut stream = tcp(address, None).unwrap();
    let bytes = payload();
    stream.write_all(&bytes).unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut response = Vec::new();
    stream.take(8400).read_to_end(&mut response).unwrap();
    let split = response.iter().position(|b| *b == b'\n').unwrap();
    let source = std::str::from_utf8(&response[..split]).unwrap();
    assert_eq!(
        source.parse::<IpAddr>().unwrap(),
        expected.parse::<IpAddr>().unwrap(),
        "TCP observed source"
    );
    assert_eq!(&response[split + 1..], bytes);
}
pub(super) fn through_tunnel(grant6: &str) {
    echo(format!("{E}30:9000").parse().unwrap(), &format!("{E}20"));
    echo(format!("[{V}30]:9000").parse().unwrap(), grant6);
    for (address, expected) in [
        (format!("{E}30:9001"), format!("{E}20")),
        (format!("[{V}30]:9001"), grant6.to_owned()),
    ] {
        let socket = udp(address.parse().unwrap(), None).unwrap();
        let bytes: Vec<u8> = (0..257).map(|n| n as u8).collect();
        socket.send(&bytes).unwrap();
        let mut response = [0; 512];
        let n = socket.recv(&mut response).unwrap();
        let split = response[..n].iter().position(|b| *b == b'\n').unwrap();
        assert_eq!(
            std::str::from_utf8(&response[..split])
                .unwrap()
                .parse::<IpAddr>()
                .unwrap(),
            expected.parse::<IpAddr>().unwrap()
        );
        assert_eq!(&response[split + 1..n], bytes);
    }
    for (kind, expected) in [("A", format!("{E}30")), ("AAAA", format!("{V}30"))] {
        assert!(
            command(
                "/usr/bin/resolvectl",
                &["query", "--cache=no", &format!("--type={kind}"), NAME]
            )
            .contains(&expected)
        );
    }
}
pub(super) fn endpoint(master: Value) {
    route("-6", "2001:db8:203::/64", &format!("{V}20"));
    private(
        &Path::new(ROOT).join("leaf.pem"),
        master["leaf_pem"].as_str().unwrap().as_bytes(),
    );
    private(
        &Path::new(ROOT).join("leaf.key"),
        master["leaf_key"].as_str().unwrap().as_bytes(),
    );
    private(
        &Path::new(ROOT).join("nats.json"),
        &serde_json::to_vec(&master["nats"]).unwrap(),
    );
    assert_eq!(
        command("/usr/bin/sha256sum", &["/usr/local/bin/nats-server"])
            .split_whitespace()
            .next()
            .unwrap(),
        NATS_SHA
    );
    assert!(command("/usr/local/bin/nats-server", &["--version"]).contains("2.15.1"));
    let mut broker = Process(
        Command::new("/usr/bin/setpriv")
            .args([
                "--no-new-privs",
                "--bounding-set=-all",
                "--inh-caps=-all",
                "--ambient-caps=-all",
                "/usr/local/bin/nats-server",
                "-c",
                "/run/topology/nats.json",
            ])
            .spawn()
            .unwrap(),
    );
    let until = Instant::now() + Duration::from_secs(210);
    let ready = Instant::now() + Duration::from_secs(5);
    while TcpStream::connect(format!("{U}2:4222")).is_err() {
        assert!(Instant::now() < ready);
        thread::sleep(Duration::from_millis(20));
    }
    let mut threads = Vec::new();
    for host in [format!("{E}30"), format!("{V}30")] {
        let ip: IpAddr = host.parse().unwrap();
        for port in [53, 9000] {
            let listener = TcpListener::bind(SocketAddr::new(ip, port)).unwrap();
            listener.set_nonblocking(true).unwrap();
            threads.push(thread::spawn(move || {
                while Instant::now() < until {
                    match listener.accept() {
                        Ok((mut stream, peer)) => {
                            stream
                                .set_read_timeout(Some(Duration::from_secs(3)))
                                .unwrap();
                            stream
                                .set_write_timeout(Some(Duration::from_secs(3)))
                                .unwrap();
                            if port == 53 {
                                let mut length = [0; 2];
                                if stream.read_exact(&mut length).is_err() {
                                    continue;
                                }
                                let n = usize::from(u16::from_be_bytes(length));
                                assert!((13..=4096).contains(&n));
                                let mut bytes = vec![0; n];
                                stream.read_exact(&mut bytes).unwrap();
                                let response = dns(&bytes, peer.ip());
                                stream
                                    .write_all(&(response.len() as u16).to_be_bytes())
                                    .unwrap();
                                stream.write_all(&response).unwrap();
                            } else {
                                let mut bytes = Vec::new();
                                if Read::by_ref(&mut stream)
                                    .take(8193)
                                    .read_to_end(&mut bytes)
                                    .is_ok()
                                    && bytes.len() == 8192
                                {
                                    let _ = stream.write_all(format!("{}\n", peer.ip()).as_bytes());
                                    let _ = stream.write_all(&bytes);
                                    println!("TCP observed source={}", peer.ip());
                                }
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(e) => panic!("endpoint TCP: {e}"),
                    }
                }
            }));
        }
        for port in [53, 9001] {
            let socket = UdpSocket::bind(SocketAddr::new(ip, port)).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_millis(200)))
                .unwrap();
            threads.push(thread::spawn(move || {
                let mut bytes = [0; 4096];
                while Instant::now() < until {
                    match socket.recv_from(&mut bytes) {
                        Ok((n, peer)) => {
                            let response = if port == 53 {
                                dns(&bytes[..n], peer.ip())
                            } else {
                                assert!(n <= 257);
                                let mut response = format!("{}\n", peer.ip()).into_bytes();
                                response.extend_from_slice(&bytes[..n]);
                                response
                            };
                            socket.send_to(&response, peer).unwrap();
                        }
                        Err(e)
                            if matches!(
                                e.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                            ) => {}
                        Err(e) => panic!("endpoint UDP: {e}"),
                    }
                }
            }));
        }
    }
    fs::write(Path::new(ROOT).join("ready"), b"ready").unwrap();
    for thread in threads {
        thread.join().unwrap();
    }
    broker.0.kill().unwrap();
    broker.exited(false);
}
