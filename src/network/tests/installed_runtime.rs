//! Exercise installed runtime bytes through API1, verified TLS and owned TCP FDs.
use serde_json::{Value, json};
use skvoz_network_native::{IncrementalUnix, inherit_control};
use std::{
    io::{self, Read, Write},
    os::{
        fd::{AsFd, OwnedFd},
        unix::net::UnixStream,
    },
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Runtime {
    child: Child,
    channel: Option<IncrementalUnix>,
    id: u32,
}
impl Runtime {
    fn start(binary: &Path, config: &Path) -> Self {
        let (owner, child) = UnixStream::pair().unwrap();
        owner.set_nonblocking(true).unwrap();
        child.set_nonblocking(true).unwrap();
        let mut command = Command::new(binary);
        command
            .env_remove("LD_LIBRARY_PATH")
            .arg("--config")
            .arg(config)
            .args(["--control-fd", "3"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        inherit_control(&mut command, child.as_fd(), 3).unwrap();
        let child_process = command.spawn().unwrap();
        drop(command);
        drop(child);
        let mut runtime = Self {
            child: child_process,
            channel: Some(IncrementalUnix::from_owned_fd(owner.into()).unwrap()),
            id: 0,
        };
        assert!(runtime.call("HELLO", json!({"api":1,"network":6})).0["error"].is_null());
        let until = Instant::now() + Duration::from_secs(15);
        while runtime.call("STATUS", json!({})).0["result"]["lifecycle"] != "ready" {
            assert!(
                Instant::now() < until,
                "installed runtime readiness deadline"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        runtime
    }
    fn call(&mut self, op: &str, args: Value) -> (Value, Option<OwnedFd>) {
        self.id += 1;
        let id = self.id;
        let channel = self.channel.as_mut().unwrap();
        channel
            .queue_frame(
                serde_json::to_vec(&json!({"v":1,"id":id,"op":op,"args":args,"fd_count":0}))
                    .unwrap(),
                None,
            )
            .unwrap();
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            assert!(
                Instant::now() < until,
                "installed runtime response deadline: {op}"
            );
            if channel.write_pending() {
                match channel.try_flush() {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => panic!("runtime owner write: {e}"),
                }
            }
            match channel.try_receive_frame() {
                Ok(frame) => {
                    let value = skvoz_network::local_api::parse_strict_json(&frame.body).unwrap();
                    if value["id"] == id {
                        return (value, frame.fd);
                    }
                    assert!(frame.fd.is_none(), "unsolicited FD");
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("runtime owner read: {e}"),
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn owner_eof(&mut self) {
        self.channel = None;
        let until = Instant::now() + Duration::from_secs(6);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "installed runtime cleanup failed");
                return;
            }
            assert!(
                Instant::now() < until,
                "installed runtime owner EOF cleanup deadline"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[test]
#[ignore = "requires the ready installed Linux candidate"]
fn installed_runtime_preserves_tls_api_tcp_half_close_and_owner_eof() {
    let fixture = skvoz_network_qualification::TlsFixture::new();
    let binary =
        std::env::var_os("SKVOZ_RUNTIME_EXECUTABLE").expect("installed runtime path missing");
    let mut server = Runtime::start(Path::new(&binary), &fixture.root.join("server.json"));
    let mut client = Runtime::start(Path::new(&binary), &fixture.root.join("client.json"));
    assert!(
        client
            .call("START_PROXY", json!({"http_bind":null,"socks_bind":null}))
            .0["error"]
            .is_null()
    );
    let host = "127.0.0.1";
    let port = fixture.echo_port;
    let until = Instant::now() + Duration::from_secs(10);
    let fd = loop {
        let (reply, fd) = client.call("OPEN_TCP", json!({"host":host,"port":port}));
        if reply["error"].is_null() {
            break fd.expect("OPEN_TCP lacks owned FD");
        }
        assert_eq!(reply["error"], "network_unavailable");
        assert!(
            Instant::now() < until,
            "installed runtime peer JOIN deadline"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut stream = UnixStream::from(fd);
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let expected: Vec<u8> = (0..32771).map(|i| (i * 31 + i / 255) as u8).collect();
    stream.write_all(&expected).unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, expected);
    drop(stream);
    client.owner_eof();
    server.owner_eof();
    println!("Installed runtime TLS/API1/TCP/half-close/owner EOF passed");
}
