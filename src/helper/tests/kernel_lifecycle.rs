#![cfg(target_os = "linux")]
//! Scoped real-kernel check, admitted only inside the explicitly prepared fixture.
#[path = "support/bundle.rs"]
mod bundle;
#[allow(dead_code)]
mod support;
use serde_json::{Value, json};
use skvoz_network_contracts::{
    SessionId,
    config::{BackendConfig, HelperConfig, HelperPolicy, Role},
    local_api::parse_strict_json,
};
use skvoz_network_native::{IncrementalUnix, TunDevice, inherit_control};
use std::{
    io,
    os::{
        fd::{AsFd, AsRawFd, OwnedFd},
        unix::{fs::PermissionsExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        assert_eq!(std::env::var("SKVOZ_KERNEL_FIXTURE").as_deref(), Ok("1"));
        assert!(
            Path::new("/.dockerenv").exists(),
            "fixture must be an isolated container"
        );
        assert!(
            !Path::new("/var/run/docker.sock").exists(),
            "Docker socket must not enter fixture"
        );
        bundle::verify(&["bin/skvoz-network-helper"]);
        let path = PathBuf::from(format!(
            "/run/skvoz-kernel-{}",
            SessionId::random().unwrap().as_str()
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
    fn config(&self) -> (PathBuf, HelperPolicy) {
        let (mut server, mut network) = support::policy();
        network.families = vec![4, 6];
        server.ipv6 = Some(BackendConfig {
            pool: "2001:db8:203::/64".parse().unwrap(),
            egress: "routed".into(),
            interface: "eth0".into(),
        });
        server.lease_store = self.0.join("state/leases.json");
        let policy = HelperPolicy { server, network };
        let state = self.0.join("state");
        std::fs::create_dir(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        let config = HelperConfig {
            v: 1,
            role: Role::Server,
            state_dir: state,
            policy: Some(policy.clone()),
        };
        let path = self.0.join("helper.json");
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        (path, policy)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Helper(Child);
impl Helper {
    fn start(config: &Path) -> (Self, IncrementalUnix) {
        let (owner, child) = UnixStream::pair().unwrap();
        owner.set_nonblocking(true).unwrap();
        child.set_nonblocking(true).unwrap();
        let binary = std::env::var_os("SKVOZ_HELPER_EXECUTABLE")
            .map(PathBuf::from)
            .expect("explicit frozen helper path required");
        let mut command = Command::new("/usr/bin/setpriv");
        command
            .args([
                "--no-new-privs",
                "--bounding-set=-all,+net_admin",
                "--inh-caps=-all",
                "--ambient-caps=-all",
            ])
            .arg(binary)
            .arg("--config")
            .arg(config)
            .args(["--control-fd", "3"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        inherit_control(&mut command, child.as_fd(), 3).unwrap();
        let helper = Self(command.spawn().unwrap());
        drop(command);
        drop(child);
        (
            helper,
            IncrementalUnix::from_owned_fd(owner.into()).unwrap(),
        )
    }
    fn exited(&mut self, success: bool) {
        let until = Instant::now() + Duration::from_secs(6);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert_eq!(status.success(), success);
                return;
            }
            assert!(Instant::now() < until, "helper cleanup deadline");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn call(owner: &mut IncrementalUnix, id: u32, op: &str, args: Value) -> (Value, Option<OwnedFd>) {
    owner
        .queue_frame(
            serde_json::to_vec(&json!({"v":1,"id":id,"op":op,"args":args,"fd_count":0})).unwrap(),
            None,
        )
        .unwrap();
    let until = Instant::now() + Duration::from_secs(6);
    loop {
        assert!(Instant::now() < until, "helper operation deadline: {op}");
        if owner.write_pending() {
            match owner.try_flush() {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("helper write: {e}"),
            }
        }
        match owner.try_receive_frame() {
            Ok(frame) => {
                let value = parse_strict_json(&frame.body).unwrap();
                assert_eq!(value["id"], id);
                assert!(value["error"].is_null(), "{op}: {value}");
                return (value["result"].clone(), frame.fd);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => panic!("helper read: {e}"),
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn output(program: &str, args: &[&str]) -> String {
    let result = Command::new(program).args(args).output().unwrap();
    assert!(
        result.status.success(),
        "fixture command failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}
fn clean_kernel() {
    let links: Value =
        serde_json::from_str(&output("/usr/sbin/ip", &["-j", "link", "show"])).unwrap();
    assert!(
        !links
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["ifname"] == "skvoz0")
    );
    assert!(!output("/usr/sbin/nft", &["list", "tables"]).contains("skvoz_network"));
    for family in ["-4", "-6"] {
        let routes: Value = serde_json::from_str(&output(
            "/usr/sbin/ip",
            &["-N", family, "-j", "route", "show", "table", "all"],
        ))
        .unwrap();
        assert!(
            !routes
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["protocol"] == 186 || v["protocol"] == "186")
        );
    }
}
fn assert_helper_caps(helper: &Helper) {
    let status = std::fs::read_to_string(format!("/proc/{}/status", helper.0.id())).unwrap();
    for name in ["CapPrm", "CapEff", "CapBnd"] {
        assert!(
            status
                .lines()
                .any(|line| line == format!("{name}:\t0000000000001000"))
        );
        println!("{name}:\t0000000000001000");
    }
    assert!(status.lines().any(|line| line == "NoNewPrivs:\t1"));
}
fn fd_count() -> usize {
    std::fs::read_dir("/proc/self/fd").unwrap().count()
}
fn grant_routes(grants: &[String], present: bool) {
    for grant in grants {
        let family = if grant.contains(':') { "-6" } else { "-4" };
        let routes: Value = serde_json::from_str(&output(
            "/usr/sbin/ip",
            &["-N", family, "-j", "route", "show", "table", "all"],
        ))
        .unwrap();
        let address = grant.split('/').next().unwrap();
        assert_eq!(
            routes.as_array().unwrap().iter().any(|route| {
                route["dev"] == "skvoz0" && (route["dst"] == *grant || route["dst"] == address)
            }),
            present,
            "route for {grant}"
        );
    }
}
fn reserve(owner: &mut IncrementalUnix) -> (Value, Vec<String>) {
    let session = SessionId::random().unwrap();
    let (reserved, fd) = call(
        owner,
        3,
        "RESERVE_PEER",
        json!({"peer":"1","session":session,"families":[4,6],"mtu":1500}),
    );
    assert!(fd.is_none());
    assert_eq!(reserved["source_grants"].as_array().unwrap().len(), 2);
    let grants: Vec<String> = serde_json::from_value(reserved["source_grants"].clone()).unwrap();
    let args = json!({"peer":"1","session":session});
    call(owner, 4, "ACTIVATE_PEER", args.clone());
    grant_routes(&grants, true);
    (args, grants)
}
#[test]
#[ignore = "requires disposable NET_ADMIN/TUN fixture with namespace-local forwarding/fragment limits"]
fn real_helper_transfers_tun_rejects_second_owner_retires_grant_and_cleans_on_eof() {
    let fixture = Fixture::new();
    let (config, policy) = fixture.config();
    let descriptors = fd_count();
    clean_kernel();
    let (mut helper, mut owner) = Helper::start(&config);
    call(&mut owner, 1, "HELLO", json!({"api":1,"network":6}));
    assert_helper_caps(&helper);
    let (prepared, fd) = call(
        &mut owner,
        2,
        "PREPARE_SERVER",
        serde_json::to_value(&policy).unwrap(),
    );
    assert_eq!(prepared["interface"], "skvoz0");
    let tun = TunDevice::from_owned_fd(
        fd.expect("PREPARE_SERVER must transfer one owned TUN"),
        1500,
    )
    .unwrap();
    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", tun.as_raw_fd())).unwrap();
    let flags = info
        .lines()
        .find_map(|line| line.strip_prefix("flags:\t"))
        .unwrap();
    assert_ne!(u32::from_str_radix(flags, 8).unwrap() & 0o2000000, 0);
    let (mut duplicate, other) = Helper::start(&config);
    duplicate.exited(false);
    drop(other);
    let (args, grants) = reserve(&mut owner);
    call(&mut owner, 5, "RETIRE_PEER", args.clone());
    grant_routes(&grants, false);
    call(&mut owner, 6, "RETIRE_PEER", args);
    drop(tun);
    drop(owner);
    helper.exited(true);
    clean_kernel();
    let (mut recovered, mut owner) = Helper::start(&config);
    call(&mut owner, 1, "HELLO", json!({"api":1,"network":6}));
    call(&mut owner, 2, "STOP_SERVER", json!({}));
    recovered.exited(true);
    drop(owner);
    clean_kernel();
    assert_eq!(fd_count(), descriptors, "owned control/TUN FD leak");

    // SIGKILL leaves the durable journal for the next real helper to recover.
    let (mut crashed, mut owner) = Helper::start(&config);
    call(&mut owner, 1, "HELLO", json!({"api":1,"network":6}));
    let (_, tun) = call(
        &mut owner,
        2,
        "PREPARE_SERVER",
        serde_json::to_value(&policy).unwrap(),
    );
    reserve(&mut owner);
    crashed.0.kill().unwrap();
    crashed.exited(false);
    drop(tun);
    drop(owner);
    let (mut recovered, mut owner) = Helper::start(&config);
    call(&mut owner, 1, "HELLO", json!({"api":1,"network":6}));
    clean_kernel();
    call(&mut owner, 2, "STOP_SERVER", json!({}));
    recovered.exited(true);
    drop(owner);
    assert_eq!(fd_count(), descriptors, "crash/recovery FD leak");
}
