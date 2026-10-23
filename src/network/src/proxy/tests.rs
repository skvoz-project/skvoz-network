use super::*;

fn open(progress: Progress) -> (String, u16, Vec<u8>, Vec<u8>) {
    match progress {
        Progress::Open {
            host,
            port,
            initial,
            success,
            ..
        } => (host, port, initial, success),
        _ => panic!("expected open"),
    }
}

#[test]
fn incremental_http_rewrites_and_keeps_binary_prefix() {
    let request = b"POST http://example.org:8080/a?q=1 HTTP/1.1\r\nHost: ignored\r\nContent-Length: 3\r\nProxy-Connection: keep-alive\r\nTE: trailers\r\nConnection: x-secret\r\nX-Secret: remove\r\nX-Keep: yes\r\n\r\n";
    for split in 0..request.len() {
        let mut parser = Handshake::new(Protocol::Http);
        assert_eq!(parser.feed(&request[..split]), Ok(Progress::Read));
        let mut rest = request[split..].to_vec();
        rest.extend_from_slice(&[0, 255, 1]);
        let (host, port, initial, success) = open(parser.feed(&rest).unwrap());
        assert_eq!((host.as_str(), port), ("example.org", 8080));
        assert_eq!(initial, b"POST /a?q=1 HTTP/1.1\r\nContent-Length: 3\r\nX-Keep: yes\r\nHost: example.org:8080\r\nConnection: close\r\n\r\n\0\xff\x01");
        assert!(success.is_empty());
        assert!(parser.feed(&[]).is_err());
    }
}

#[test]
fn connect_preserves_pipelined_tls_prefix() {
    let mut parser = Handshake::new(Protocol::Http);
    let (host, port, initial, success) = open(
        parser
            .feed(b"CONNECT [2001:db8::1]:443 HTTP/1.1\r\n\r\n\x16\x03\x01")
            .unwrap(),
    );
    assert_eq!((host.as_str(), port), ("2001:db8::1", 443));
    assert_eq!(initial, [22, 3, 1]);
    assert_eq!(success, HTTP_CONNECTED);
}

#[test]
fn http_empty_path_query_and_ipv6_host() {
    for (uri, first_line, host_header) in [
        ("http://example.org", "GET / HTTP/1.0", "Host: example.org"),
        (
            "http://[2001:db8::1]:81?x",
            "GET /?x HTTP/1.0",
            "Host: [2001:db8::1]:81",
        ),
    ] {
        let mut parser = Handshake::new(Protocol::Http);
        let (_, _, initial, _) = open(
            parser
                .feed(format!("GET {uri} HTTP/1.0\r\n\r\n").as_bytes())
                .unwrap(),
        );
        let text = String::from_utf8(initial).unwrap();
        assert!(text.starts_with(first_line));
        assert!(text.contains(host_header));
    }
}

#[test]
fn http_rejects_ambiguous_framing_and_invalid_targets() {
    for header in [
        "Content-Length: 1\r\nContent-Length: 1",
        "Content-Length: 1\r\nTransfer-Encoding: chunked",
        "Transfer-Encoding: gzip, chunked",
        "Host: a\r\nHost: b",
        "Connection: host",
        "Bad : value",
        " folded: value",
        "X: value\nInjected: yes",
    ] {
        let mut parser = Handshake::new(Protocol::Http);
        assert!(
            parser
                .feed(format!("GET http://example.org/ HTTP/1.1\r\n{header}\r\n\r\n").as_bytes())
                .is_err(),
            "{header}"
        );
        assert_eq!(parser.bad_request(), HTTP_BAD_REQUEST);
    }
    for target in [
        "https://example.org/",
        "http://user@example.org/",
        "http://example.org/#f",
        "http://127.1/",
        "http://[::ffff:127.0.0.1]/",
        "http://[fe80::1%eth0]/",
        "/origin",
        "http://example.org:0/",
        "http://example.org\\evil/",
    ] {
        assert!(
            Handshake::new(Protocol::Http)
                .feed(format!("GET {target} HTTP/1.1\r\n\r\n").as_bytes())
                .is_err(),
            "{target}"
        );
    }
}

#[test]
fn socks_coalesced_handshake_never_loses_payload() {
    let mut parser = Handshake::new(Protocol::Socks);
    assert_eq!(
        parser.feed(b"\x05\x02\x02\x00\x05\x01\x00\x03\x0bexample.org\x01\xbbpayload"),
        Ok(Progress::Write(vec![5, 0]))
    );
    let (host, port, initial, success) = open(parser.feed(&[]).unwrap());
    assert_eq!((host.as_str(), port), ("example.org", 443));
    assert_eq!(initial, b"payload");
    assert_eq!(success, SOCKS_CONNECTED);
}

#[test]
fn socks_incremental_greeting_and_ipv4_ipv6_request() {
    for address in [
        Ipv4Addr::new(127, 0, 0, 1).octets().to_vec(),
        "2001:db8::1".parse::<Ipv6Addr>().unwrap().octets().to_vec(),
    ] {
        let mut wire = vec![5, 1, 0, if address.len() == 4 { 1 } else { 4 }];
        wire.extend_from_slice(&address);
        wire.extend_from_slice(&80u16.to_be_bytes());
        for split in 0..wire.len() {
            let mut parser = Handshake::new(Protocol::Socks);
            assert_eq!(parser.feed(&[5]), Ok(Progress::Read));
            assert_eq!(parser.feed(&[1]), Ok(Progress::Read));
            assert_eq!(parser.feed(&[0]), Ok(Progress::Write(vec![5, 0])));
            assert_eq!(parser.feed(&wire[..split]), Ok(Progress::Read));
            let (_, port, initial, _) = open(parser.feed(&wire[split..]).unwrap());
            assert_eq!(port, 80);
            assert!(initial.is_empty());
        }
    }
}

#[test]
fn socks_rejects_auth_commands_and_noncanonical_addresses() {
    for greeting in [vec![4, 1, 0], vec![5, 0], vec![5, 1, 2]] {
        let mut parser = Handshake::new(Protocol::Socks);
        assert!(parser.feed(&greeting).is_err());
        assert_eq!(parser.bad_request(), [5, 255]);
    }
    for request in [
        vec![5, 2, 0, 1],
        vec![5, 1, 1, 1],
        vec![5, 1, 0, 9],
        vec![5, 1, 0, 3, 0],
        vec![5, 1, 0, 1, 127, 0, 0, 1, 0, 0],
        b"\x05\x01\x00\x03\x05bad%z\x00\x50".to_vec(),
    ] {
        let mut parser = Handshake::new(Protocol::Socks);
        parser.feed(&[5, 1, 0]).unwrap();
        assert!(parser.feed(&request).is_err());
        assert_eq!(parser.bad_request(), SOCKS_FAILED);
    }
    let mut parser = Handshake::new(Protocol::Socks);
    parser.feed(&[5, 1, 0]).unwrap();
    let mut mapped = vec![5, 1, 0, 4];
    mapped.extend_from_slice(&"::ffff:192.0.2.1".parse::<Ipv6Addr>().unwrap().octets());
    mapped.extend_from_slice(&80u16.to_be_bytes());
    assert!(parser.feed(&mapped).is_err());
}

#[test]
fn cap_is_cumulative_across_socks_greeting() {
    let mut parser = Handshake::new(Protocol::Http);
    assert_eq!(
        parser.feed(&vec![b'x'; HANDSHAKE_MAX - 1]),
        Ok(Progress::Read)
    );
    assert!(parser.feed(b"x").is_err());
    assert!(parser.feed(b"").is_err());
    let mut parser = Handshake::new(Protocol::Socks);
    parser.feed(&[5, 1, 0]).unwrap();
    assert!(parser.feed(&vec![0; HANDSHAKE_MAX - 2]).is_err());
    let mut parser = Handshake::new(Protocol::Http);
    let mut wire = b"CONNECT example.org:80 HTTP/1.1\r\n\r\n".to_vec();
    wire.resize(HANDSHAKE_MAX, 255);
    let (_, _, initial, _) = open(parser.feed(&wire).unwrap());
    assert_eq!(
        initial.len(),
        HANDSHAKE_MAX - b"CONNECT example.org:80 HTTP/1.1\r\n\r\n".len()
    );
}
#[test]
fn header_and_connection_nomination_counts_are_bounded() {
    let mut headers = b"GET http://example.com/ HTTP/1.1\r\n".to_vec();
    for _ in 0..257 {
        headers.extend_from_slice(b"X: 1\r\n");
    }
    headers.extend_from_slice(b"\r\n");
    assert!(Handshake::new(Protocol::Http).feed(&headers).is_err());
    let names = (0..257)
        .map(|n| format!("x{n}"))
        .collect::<Vec<_>>()
        .join(",");
    let request = format!("GET http://example.com/ HTTP/1.1\r\nConnection: {names}\r\n\r\n");
    assert!(
        Handshake::new(Protocol::Http)
            .feed(request.as_bytes())
            .is_err()
    );
}
#[test]
fn overload_and_post_parse_socks_failures_preserve_the_protocol_phase() {
    let fresh = Handshake::new(Protocol::Socks);
    assert_eq!(
        fresh.setup_failure(crate::local_api::ApiError::Overloaded),
        vec![5, 255]
    );
    assert_eq!(
        fresh.open_failure(
            crate::local_api::ApiError::Overloaded,
            SOCKS_FAILED.to_vec()
        ),
        SOCKS_FAILED
    );
    let http = Handshake::new(Protocol::Http);
    assert_eq!(
        http.open_failure(crate::local_api::ApiError::Overloaded, HTTP_FAILED.to_vec()),
        HTTP_OVERLOADED
    );
    assert_eq!(
        http.open_failure(
            crate::local_api::ApiError::NetworkUnavailable,
            HTTP_FAILED.to_vec()
        ),
        HTTP_FAILED
    );
}
