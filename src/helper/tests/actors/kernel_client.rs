//! Disposable real-helper client fixture. Ordinary test runs do not enter it.
#[path = "../support/bundle.rs"]
pub(crate) mod bundle;
use serde_json::{Value, json};
use skvoz_network_contracts::{SessionId, local_api::parse_strict_json};
use skvoz_network_native::{FramedUnix, adopt_inherited, inherit_control};
use std::{
    fs, io,
    os::{
        fd::{AsFd, BorrowedFd, OwnedFd},
        unix::{
            fs::{MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) struct Process(pub(crate) Child);
impl Process {
    pub(crate) fn exited(&mut self, success: bool) {
        let until = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert_eq!(status.success(), success);
                return;
            }
            assert!(Instant::now() < until, "fixture child exit deadline");
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
fn spawn(mode: &str, path: &Path, app: bool, fd: BorrowedFd<'_>) -> Process {
    let mut command = Command::new("/usr/bin/setpriv");
    command.args(["--no-new-privs", "--inh-caps=-all", "--ambient-caps=-all"]);
    if app {
        command.args([
            "--reuid=10001",
            "--regid=10001",
            "--clear-groups",
            "--bounding-set=-all",
        ]);
    } else {
        command.arg("--bounding-set=-all,+net_admin");
    }
    command
        .arg(std::env::current_exe().unwrap())
        .arg(mode)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    inherit_control(&mut command, fd, 3).unwrap();
    Process(command.spawn().unwrap())
}
fn receive(channel: &mut FramedUnix) -> skvoz_network_native::ControlFrame {
    channel
        .receive_frame_until(Instant::now() + Duration::from_secs(6))
        .unwrap()
}
pub(crate) struct Owner {
    child: Process,
    channel: Option<FramedUnix>,
    id: u32,
}
impl Owner {
    pub(crate) fn request(&mut self, op: &str, args: Value) -> (Value, Option<OwnedFd>) {
        self.id += 1;
        let channel = self.channel.as_mut().unwrap();
        channel
            .send_frame(
                &serde_json::to_vec(&json!({"v":1,"id":self.id,"op":op,"args":args,"fd_count":0}))
                    .unwrap(),
                None,
            )
            .unwrap();
        let frame = receive(channel);
        let reply = parse_strict_json(&frame.body).unwrap();
        assert_eq!(reply["v"], 1);
        assert_eq!(reply["id"], self.id);
        assert_eq!(reply["fd_count"], u8::from(frame.fd.is_some()));
        (reply, frame.fd)
    }
    pub(crate) fn call(&mut self, op: &str, args: Value) -> (Value, Option<OwnedFd>) {
        let (reply, fd) = self.request(op, args);
        assert!(reply["error"].is_null(), "{op}: {reply}");
        (reply["result"].clone(), fd)
    }
    pub(crate) fn close(mut self) {
        self.channel = None;
        self.child.exited(true);
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.channel = None;
    }
}
pub(crate) fn helper(config: &Path, socket: &Path) -> Process {
    helper_with_listener(config, socket, "--listener").0
}
fn helper_with_listener(
    config: &Path,
    socket: &Path,
    listener_mode: &str,
) -> (Process, Option<UnixStream>) {
    if socket.exists() {
        std::fs::remove_file(socket).unwrap();
    }
    let root = socket.parent().unwrap();
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o733)).unwrap();
    let (mut parent, child) = FramedUnix::pair().unwrap();
    let mut maker = spawn(listener_mode, socket, true, child.as_fd());
    drop(child);
    let frame = receive(&mut parent);
    assert_eq!(frame.body, b"listener");
    let foreign = if listener_mode == "--foreign-peer-listener" {
        assert!(
            std::fs::read_to_string("/proc/self/status")
                .unwrap()
                .lines()
                .any(|line| line == "Uid:\t0\t0\t0\t0")
        );
        let stream = UnixStream::connect(socket).unwrap();
        parent.send_frame(b"private", None).unwrap();
        assert_eq!(receive(&mut parent).body, b"private");
        let metadata = std::fs::metadata(socket).unwrap();
        assert_eq!(metadata.uid(), 10001);
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
        Some(stream)
    } else {
        None
    };
    maker.exited(true);
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o711)).unwrap();
    (
        spawn("--helper", config, false, frame.fd.unwrap().as_fd()),
        foreign,
    )
}
pub(crate) fn owner(socket: &Path) -> Owner {
    let (parent, child) = FramedUnix::pair().unwrap();
    let proxy = spawn("--owner", socket, true, child.as_fd());
    drop(child);
    let mut owner = Owner {
        child: proxy,
        channel: Some(parent),
        id: 0,
    };
    owner.call("HELLO", json!({"api":1,"network":6}));
    owner
}
pub(crate) fn command(program: &str, args: &[&str]) -> String {
    let output = Command::new(program).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{program}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
pub(crate) fn capabilities(path: &str, expected: u64) {
    let status = std::fs::read_to_string(path).unwrap();
    for name in ["CapPrm", "CapEff", "CapBnd"] {
        let line = status
            .lines()
            .find(|line| line.starts_with(&format!("{name}:")))
            .unwrap();
        assert_eq!(
            u64::from_str_radix(line.split_whitespace().nth(1).unwrap(), 16).unwrap(),
            expected,
            "{line}"
        );
        println!("{line}");
    }
    for name in ["CapInh", "CapAmb"] {
        assert!(
            status
                .lines()
                .any(|line| line == format!("{name}:\t0000000000000000"))
        );
    }
    assert!(status.lines().any(|line| line == "NoNewPrivs:\t1"));
}
pub(crate) fn app_identity() {
    capabilities("/proc/self/status", 0);
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    assert!(
        status
            .lines()
            .any(|line| line == "Uid:\t10001\t10001\t10001\t10001")
    );
    println!("Uid:\t10001\t10001\t10001\t10001");
}
fn guard(active: bool) {
    let rules = command("/usr/sbin/nft", &["list", "table", "inet", "skvoz_network"]);
    assert!(rules.contains("ip daddr 198.18.0.2 tcp dport 4222 accept"));
    assert!(rules.contains("drop"));
    assert!(
        !rules.contains("dport 53 accept"),
        "direct physical DNS exception"
    );
    assert_eq!(
        rules.contains("oifname \"skvoz0\" meta nfproto ipv4 accept"),
        active
    );
    assert_eq!(
        rules.contains("oifname \"skvoz0\" meta nfproto ipv6 accept"),
        active
    );
}
pub(crate) fn restored() {
    assert!(!command("/usr/sbin/nft", &["list", "tables"]).contains("skvoz_network"));
    assert!(!command("/usr/sbin/ip", &["-j", "link", "show"]).contains("skvoz0"));
    for family in ["-4", "-6"] {
        assert!(
            !command(
                "/usr/sbin/ip",
                &["-N", family, "route", "show", "table", "all"]
            )
            .contains("proto 186")
        );
    }
}
fn prepare(owner: &mut Owner) -> (SessionId, OwnedFd) {
    let handle = SessionId::random().unwrap();
    let config = json!({"session":SessionId::random().unwrap(),"families":[4,6],
        "source_grants":["10.203.0.2/32","2001:db8:203::2/128"],"routes":["0.0.0.0/0","::/0"],
        "dns_servers":["198.18.0.53"],"mtu":1400,"channels":1,"packet_queue_bytes":8192,
        "packet_queue_records":32,"setup_timeout_ms":10000,"egress":{"ipv4":"nat44","ipv6":"routed"}});
    let (result, fd) = owner.call(
        "PREPARE_CLIENT",
        json!({"handle":handle,"config":config,
        "transport_endpoints":[{"ip":"198.18.0.2","port":4222}]}),
    );
    assert_eq!(result["interface"], "skvoz0");
    assert_eq!(result["mtu"], 1400);
    guard(false);
    assert!(command("/usr/bin/resolvectl", &["dns", "skvoz0"]).contains("198.18.0.53"));
    assert!(command("/usr/sbin/ip", &["-j", "route", "get", "198.18.0.2"]).contains("eth0"));
    assert!(command("/usr/sbin/ip", &["-j", "route", "get", "198.18.0.53"]).contains("skvoz0"));
    owner.call("ACTIVATE_CLIENT", json!({"handle":handle}));
    guard(true);
    (handle, fd.expect("owned TUN missing"))
}
fn fixture() {
    assert!(Path::new("/.dockerenv").exists());
    assert!(!Path::new("/var/run/docker.sock").exists());
    assert!(
        Path::new("/run/dbus/system_bus_socket").exists(),
        "prepared private DBus/polkit/resolved fixture required"
    );
    assert!(
        std::env::current_exe().unwrap().starts_with("/fixture/bin"),
        "install fixture executable where UID10001 can traverse"
    );
    // Setup/supervisor only: KILL,SETGID,SETUID,SETPCAP,NET_BIND_SERVICE,
    // NET_ADMIN,NET_RAW. Product children prove their narrower identities.
    capabilities("/proc/self/status", 0x35e0);
    bundle::verify(&["bin/skvoz-network-helper"]);
    let root = Path::new("/run/skvoz-network-helper/10001");
    std::fs::create_dir_all(root).unwrap();
    std::fs::set_permissions(
        root.parent().unwrap(),
        std::fs::Permissions::from_mode(0o711),
    )
    .unwrap();
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o711)).unwrap();
    let state = Path::new("/var/lib/skvoz-network-helper/10001");
    std::fs::create_dir_all(state).unwrap();
    std::fs::set_permissions(state, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = Path::new("/run/skvoz-client-fixture.json");
    std::fs::write(
        config,
        serde_json::to_vec(&json!({"v":1,"role":"client","state_dir":state,"policy":null}))
            .unwrap(),
    )
    .unwrap();
    std::fs::set_permissions(config, std::fs::Permissions::from_mode(0o600)).unwrap();
    let socket = root.join("control.sock");
    restored();

    // Actual SO_PEERCRED admission rejects a root connection to the app listener.
    // Queue one foreign-UID connection while the app permits DAC access, then
    // restore0600 before starting the real helper and its SO_PEERCRED check.
    let (mut rejected, stream) = helper_with_listener(config, &socket, "--foreign-peer-listener");
    let stream = stream.expect("queued foreign peer");
    stream.set_nonblocking(true).unwrap();
    let mut channel = FramedUnix::from_owned_fd(stream.into()).unwrap();
    if channel
        .send_frame(
            br#"{"v":1,"id":1,"op":"HELLO","args":{"api":1,"network":6},"fd_count":0}"#,
            None,
        )
        .is_ok()
    {
        assert!(
            channel
                .receive_frame_until(Instant::now() + Duration::from_secs(5))
                .is_err()
        );
    }
    rejected.exited(false);
    drop(channel);
    println!("Foreign UID0 connection rejected with listener restored to UID10001 mode0600");

    let mut service = helper(config, &socket);
    let mut app = owner(&socket);
    capabilities(&format!("/proc/{}/status", service.0.id()), 1 << 12);
    assert_eq!(app.call("RECOVER", json!({})).0["state"], "idle");
    let (handle, tun) = prepare(&mut app);
    assert_eq!(
        app.request(
            "RESTORE_CLIENT",
            json!({"handle":SessionId::random().unwrap(),"reason":"user_stop"})
        )
        .0["error"],
        "forbidden"
    );
    guard(true);
    app.call("ABORT_CLIENT", json!({"handle":handle}));
    guard(false);
    app.close();
    service.exited(true);
    drop(tun);
    guard(false);

    let mut service = helper(config, &socket);
    let mut app = owner(&socket);
    let recovered = app.call("RECOVER", json!({})).0;
    assert_eq!(recovered["state"], "guarded");
    assert_eq!(recovered["handle"], serde_json::to_value(&handle).unwrap());
    app.call(
        "RESTORE_CLIENT",
        json!({"handle":handle,"reason":"user_stop"}),
    );
    restored();
    let (handle, tun) = prepare(&mut app);
    app.close();
    service.exited(true);
    drop(tun);
    guard(false);

    let mut service = helper(config, &socket);
    let mut app = owner(&socket);
    assert_eq!(
        app.call("RECOVER", json!({})).0["handle"],
        serde_json::to_value(&handle).unwrap()
    );
    app.call(
        "RESTORE_CLIENT",
        json!({"handle":handle,"reason":"user_stop"}),
    );
    restored();
    let (handle, tun) = prepare(&mut app);
    service.0.kill().unwrap();
    service.exited(false);
    drop(app);
    drop(tun);
    let mut service = helper(config, &socket);
    let mut app = owner(&socket);
    guard(false);
    assert_eq!(app.call("RECOVER", json!({})).0["state"], "guarded");
    app.call(
        "RESTORE_CLIENT",
        json!({"handle":handle,"reason":"user_stop"}),
    );
    app.close();
    service.exited(true);
    restored();
    println!(
        "Actual UID/polkit helper guard/DNS/routes/foreign handle/abort/EOF/crash/recovery passed"
    );
}
pub(crate) fn child_mode() -> bool {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 2
        && matches!(
            args[0].to_str(),
            Some("--helper" | "--listener" | "--foreign-peer-listener" | "--owner")
        )
    {
        let path = PathBuf::from(&args[1]);
        match args[0].to_str().unwrap() {
            "--helper" => {
                let binary = std::env::var_os("SKVOZ_HELPER_EXECUTABLE")
                    .expect("explicit frozen helper path required");
                let error = Command::new(binary)
                    .arg("--config")
                    .arg(path)
                    .args(["--listen-fd", "3"])
                    .env("LISTEN_PID", std::process::id().to_string())
                    .env("LISTEN_FDS", "1")
                    .env("LISTEN_FDNAMES", "control")
                    .exec();
                panic!("fixture helper exec: {error}");
            }
            "--listener" | "--foreign-peer-listener" => {
                app_identity();
                let listener = UnixListener::bind(&path).unwrap();
                let mode = if args[0] == "--foreign-peer-listener" {
                    0o622
                } else {
                    0o600
                };
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
                let mut parent = FramedUnix::from_owned_fd(adopt_inherited(3).unwrap()).unwrap();
                parent
                    .send_frame(b"listener", Some(listener.as_fd()))
                    .unwrap();
                if args[0] == "--foreign-peer-listener" {
                    assert_eq!(receive(&mut parent).body, b"private");
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                        .unwrap();
                    parent.send_frame(b"private", None).unwrap();
                }
            }
            "--owner" => {
                app_identity();
                let socket = UnixStream::connect(path).unwrap();
                socket.set_nonblocking(true).unwrap();
                let mut helper = FramedUnix::from_owned_fd(socket.into()).unwrap();
                let mut parent = FramedUnix::from_owned_fd(adopt_inherited(3).unwrap()).unwrap();
                loop {
                    let request = match parent
                        .receive_frame_until(Instant::now() + Duration::from_secs(15))
                    {
                        Ok(frame) => frame,
                        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                        Err(e) => panic!("fixture parent: {e}"),
                    };
                    assert!(request.fd.is_none());
                    helper.send_frame(&request.body, None).unwrap();
                    let reply = receive(&mut helper);
                    parent
                        .send_frame(&reply.body, reply.fd.as_ref().map(AsFd::as_fd))
                        .unwrap();
                }
            }
            _ => panic!("unknown fixture operation"),
        }
        true
    } else {
        false
    }
}
fn main() {
    if child_mode() {
        return;
    }
    if std::env::var("SKVOZ_KERNEL_FIXTURE").as_deref() == Ok("1") {
        let _services = services();
        fixture();
    } else {
        panic!("explicit disposable kernel fixture required");
    }
}

pub(crate) fn services() -> Vec<Process> {
    fs::create_dir_all("/run/dbus").unwrap();
    fs::create_dir_all("/run/systemd").unwrap();
    fs::set_permissions("/run/systemd", fs::Permissions::from_mode(0o733)).unwrap();
    let status = Command::new("/usr/bin/setpriv")
        .args([
            "--reuid=systemd-resolve",
            "--regid=systemd-resolve",
            "--clear-groups",
            "--no-new-privs",
            "--bounding-set=-all",
            "--inh-caps=-all",
            "--ambient-caps=-all",
            "/usr/bin/mkdir",
            "/run/systemd/resolve",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    fs::set_permissions("/run/systemd", fs::Permissions::from_mode(0o755)).unwrap();
    let dbus = Process(
        Command::new("/usr/bin/dbus-daemon")
            .args(["--system", "--nofork", "--nopidfile"])
            .spawn()
            .unwrap(),
    );
    let until = Instant::now() + Duration::from_secs(3);
    while !Path::new("/run/dbus/system_bus_socket").exists() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(20));
    }
    let polkit = Process(
        Command::new("/usr/lib/polkit-1/polkitd")
            .arg("--no-debug")
            .spawn()
            .unwrap(),
    );
    let resolved = Process(
        Command::new("/usr/bin/setpriv")
            .args([
                "--reuid=systemd-resolve",
                "--regid=systemd-resolve",
                "--clear-groups",
                "--no-new-privs",
                "--bounding-set=-all,+net_raw,+net_bind_service,+setpcap",
                "--inh-caps=+net_raw,+net_bind_service,+setpcap",
                "--ambient-caps=+net_raw,+net_bind_service,+setpcap",
                "/usr/lib/systemd/systemd-resolved",
            ])
            .spawn()
            .unwrap(),
    );
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if Command::new("/usr/bin/resolvectl")
            .arg("status")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
        {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(50));
    }
    vec![dbus, polkit, resolved]
}
