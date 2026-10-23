#![cfg(all(target_os = "linux", feature = "linux-runtime"))]
//! Executable owner isolation and cleanup without a live broker or host kernel changes.
use skvoz_network::local_api::parse_strict_json;
use skvoz_network_native::{IncrementalUnix, inherit_control};
use std::{
    io,
    os::{
        fd::AsFd,
        unix::{fs::PermissionsExt, net::UnixStream},
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "skvoz-process-{}-{}",
            std::process::id(),
            skvoz_network::SessionId::random().unwrap().as_str()
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn config(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        path
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Process(Child);
impl Process {
    fn exited(&mut self, success: bool) {
        let until = Instant::now() + Duration::from_secs(6);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert_eq!(status.success(), success);
                return;
            }
            assert!(
                Instant::now() < until,
                "runtime executable cleanup deadline"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn start(config: &std::path::Path) -> (Process, IncrementalUnix) {
    let (owner, child) = UnixStream::pair().unwrap();
    owner.set_nonblocking(true).unwrap();
    child.set_nonblocking(true).unwrap();
    let executable = std::env::var_os("SKVOZ_RUNTIME_EXECUTABLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_skvoz-network-runtime").into());
    let mut command = Command::new(executable);
    command
        .arg("--config")
        .arg(config)
        .args(["--control-fd", "3"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    inherit_control(&mut command, child.as_fd(), 3).unwrap();
    let process = Process(command.spawn().unwrap());
    drop(command);
    drop(child);
    (
        process,
        IncrementalUnix::from_owned_fd(owner.into()).unwrap(),
    )
}
fn hello(channel: &mut IncrementalUnix, id: u32) {
    channel.queue_frame(serde_json::to_vec(&serde_json::json!({"v":1,"id":id,"op":"HELLO","args":{"api":1,"network":6},"fd_count":0})).unwrap(), None).unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        assert!(Instant::now() < until, "HELLO waited for broker readiness");
        if channel.write_pending() {
            match channel.try_flush() {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("HELLO write: {e}"),
            }
        }
        match channel.try_receive_frame() {
            Ok(frame) => {
                assert!(frame.fd.is_none());
                let value = parse_strict_json(&frame.body).unwrap();
                if value.get("id").is_some() {
                    assert_eq!(value["id"], id, "owner responses crossed channels");
                    assert!(value["error"].is_null());
                    assert_eq!(value["result"]["api"], 1);
                    assert_eq!(value["result"]["network"], 6);
                    break;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => panic!("HELLO read: {e}"),
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn separate_process_owners_receive_only_their_response_and_eof_reaps_runtime() {
    let scratch = Scratch::new();
    let config = scratch.config(
        "client.json",
        include_bytes!("fixtures/client-startup.json"),
    );
    let (mut first, mut a) = start(&config);
    let (mut second, mut b) = start(&config);
    hello(&mut a, 41);
    hello(&mut b, 42);
    drop(a);
    first.exited(true);
    // Closing the first owner cannot stop the second runtime.
    assert!(second.0.try_wait().unwrap().is_none());
    drop(b);
    second.exited(true);
}
#[test]
fn duplicate_and_wrong_startup_versions_fail_before_runtime_admission() {
    let scratch = Scratch::new();
    let baseline = include_bytes!("fixtures/client-startup.json");
    let mut value: serde_json::Value = serde_json::from_slice(baseline).unwrap();
    value["v"] = 2.into();
    for (name, bytes) in [
        ("wrong.json", serde_json::to_vec(&value).unwrap()),
        ("duplicate.json", b"{\"v\":3,\"v\":3}".to_vec()),
    ] {
        let path = scratch.config(name, &bytes);
        let (mut process, owner) = start(&path);
        process.exited(false);
        drop(owner);
    }
}
