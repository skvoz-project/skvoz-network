#![cfg(target_os = "linux")]
//! Real C ABI export calls against a separately provisioned verified TLS fixture.
use skvoz_network::local_api::parse_strict_json;
use skvoz_network_ffi::*;
use std::{
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    time::{Duration, Instant},
};

struct Owner(u64);
impl Owner {
    fn create(variable: &str) -> Self {
        let bytes = std::fs::read(std::env::var(variable).expect("private fixture config missing"))
            .unwrap();
        let mut handle = 0;
        // SAFETY: independent initialized input and aligned writable handle.
        assert_eq!(
            unsafe { skvoz_network_create(bytes.as_ptr(), bytes.len(), -1, &mut handle) },
            SUCCESS
        );
        assert_ne!(handle, 0);
        Self(handle)
    }
    fn request(&self, id: u32, op: &str, args: serde_json::Value) {
        let input = serde_json::to_vec(
            &serde_json::json!({"v":1,"id":id,"op":op,"args":args,"fd_count":0}),
        )
        .unwrap();
        let mut admitted = 0;
        // SAFETY: local disjoint input/output storage; this thread owns the handle.
        assert_eq!(
            unsafe {
                skvoz_network_request(self.0, input.as_ptr(), input.len(), -1, &mut admitted)
            },
            SUCCESS
        );
        assert_eq!(admitted, id);
    }
    fn next(&self) -> Option<(serde_json::Value, Option<OwnedFd>)> {
        let (mut size, mut fd) = (0, 9);
        // SAFETY: null/capacity0 is the documented size query.
        let status = unsafe {
            skvoz_network_next_event(self.0, std::ptr::null_mut(), 0, &mut size, &mut fd, 100)
        };
        if status == TIMEOUT {
            return None;
        }
        assert_eq!(status, INSUFFICIENT_BUFFER);
        assert_eq!(fd, -1);
        assert!((1..=32768).contains(&size));
        let mut repeated = 0;
        // SAFETY: valid aligned outputs and a documented retained query.
        assert_eq!(
            unsafe {
                skvoz_network_next_event(self.0, std::ptr::null_mut(), 0, &mut repeated, &mut fd, 0)
            },
            INSUFFICIENT_BUFFER
        );
        assert_eq!((repeated, fd), (size, -1));
        let mut bytes = vec![0; size];
        // SAFETY: caller-owned disjoint buffer and outputs; size is advertised above.
        assert_eq!(
            unsafe {
                skvoz_network_next_event(
                    self.0,
                    bytes.as_mut_ptr(),
                    bytes.len(),
                    &mut repeated,
                    &mut fd,
                    0,
                )
            },
            SUCCESS
        );
        assert_eq!(repeated, size);
        // SAFETY: successful ABI call transfers one owned descriptor exactly once.
        let fd = (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) });
        Some((parse_strict_json(&bytes).unwrap(), fd))
    }
    fn response(&self, id: u32, until: Instant) -> (serde_json::Value, Option<OwnedFd>) {
        loop {
            assert!(Instant::now() < until, "ABI response deadline");
            if let Some((value, fd)) = self.next() {
                if value["id"] == id {
                    return (value, fd);
                }
                assert!(fd.is_none(), "unsolicited descriptor");
            }
        }
    }
    fn ready(&self) {
        self.request(1, "HELLO", serde_json::json!({"api":1,"network":6}));
        let (mut hello, mut ready) = (false, false);
        let until = Instant::now() + Duration::from_secs(30);
        while !hello || !ready {
            assert!(Instant::now() < until, "ABI readiness deadline");
            if let Some((value, fd)) = self.next() {
                assert!(fd.is_none());
                if value["id"] == 1 {
                    assert!(value["error"].is_null());
                    hello = true;
                }
                if value["event"] == "RUNTIME_STATE" && value["data"]["state"] == "ready" {
                    ready = true;
                }
                assert_ne!(value["data"]["state"], "closed", "ABI startup closed");
            }
        }
    }
    fn open(&self, id: &mut u32, host: &str, port: u16) -> UnixStream {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            *id += 1;
            self.request(
                *id,
                "OPEN_TCP",
                serde_json::json!({"host":host,"port":port}),
            );
            let (reply, fd) = self.response(*id, until);
            if reply["error"].is_null() {
                let stream = UnixStream::from(fd.expect("successful OPEN lacks owned FD"));
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                return stream;
            }
            assert_eq!(reply["error"], "network_unavailable");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    fn destroy(&mut self) {
        let stale = self.0;
        assert_eq!(skvoz_network_destroy(stale), SUCCESS);
        self.0 = 0;
        assert_eq!(skvoz_network_destroy(stale), CLOSED);
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if self.0 != 0 {
            let _ = skvoz_network_destroy(self.0);
        }
    }
}

#[test]
#[ignore = "requires an isolated verified TLS/NATS and echo-target fixture"]
fn exports_preserve_binary_echo_half_close_retained_fd_and_destroy() {
    let server = Owner::create("SKVOZ_NETWORK_SERVER_CONFIG");
    let mut client = Owner::create("SKVOZ_NETWORK_CLIENT_CONFIG");
    server.ready();
    client.ready();
    client.request(
        2,
        "START_PROXY",
        serde_json::json!({"http_bind":null,"socks_bind":null}),
    );
    assert!(
        client
            .response(2, Instant::now() + Duration::from_secs(10))
            .0["error"]
            .is_null()
    );
    let host = std::env::var("SKVOZ_NETWORK_ECHO_HOST").unwrap();
    let port = std::env::var("SKVOZ_NETWORK_ECHO_PORT")
        .unwrap_or_else(|_| "4444".into())
        .parse()
        .unwrap();
    let mut id = 2;
    let mut stream = client.open(&mut id, &host, port);
    // SAFETY: fcntl only reads flags of this live borrowed descriptor.
    assert_ne!(
        unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    let expected: Vec<u8> = (0..32771).map(|i| (i * 31 + i / 255) as u8).collect();
    stream.write_all(&expected).unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut received = Vec::new();
    stream.read_to_end(&mut received).unwrap();
    assert_eq!(received, expected);
    let mut live = client.open(&mut id, &host, port);
    client.destroy();
    let mut byte = [0];
    assert_eq!(
        live.read(&mut byte).unwrap(),
        0,
        "destroy did not retire delivered stream"
    );
}
