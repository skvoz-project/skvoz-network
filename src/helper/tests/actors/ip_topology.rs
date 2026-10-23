//! Three private actors exercise frozen product executables, never engine source.
#[allow(dead_code)]
#[path = "kernel_client.rs"]
mod platform;
#[path = "../support/ip_prepare.rs"]
mod prepare;
#[path = "../support/ip_traffic.rs"]
mod traffic;
use platform::{Process, command};
use serde_json::{Value, json};
use skvoz_network_contracts::local_api::parse_strict_json;
use skvoz_network_native::{IncrementalUnix, inherit_control};
use std::{
    collections::VecDeque,
    fs, io,
    os::{
        fd::{AsFd, BorrowedFd, OwnedFd},
        unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
const U: &str = "198.18.0.";
const E: &str = "198.19.0.";
const V: &str = "2001:db8:900::";
const ROOT: &str = "/run/topology";
const NATS_SHA: &str = "baab2eecb5a8ebdabf6edd3e22756f89805e3184468653ba67185adf5a3b57eb";

fn private(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
fn candidate() {
    platform::bundle::verify(&[
        "bin/skvoz-network-runtime",
        "bin/skvoz-network-helper",
        "lib/libskvoz_core.so",
    ]);
    assert_eq!(
        std::env::var("SKVOZ_HELPER_EXECUTABLE").unwrap(),
        "/opt/skvoz-native/bin/skvoz-network-helper"
    );
    assert!(std::env::var_os("LD_LIBRARY_PATH").is_none());
}
fn envelope(role: &str) {
    assert!(Path::new("/.dockerenv").exists());
    assert!(!Path::new("/var/run/docker.sock").exists());
    let caps = match role {
        "prepare" => 0,
        "server" => 0x11e0,
        "client" | "kernel" => 0x35e0,
        "endpoint" => 0x1500,
        _ => panic!("unknown actor"),
    };
    platform::capabilities("/proc/self/status", caps);
    candidate();
}
fn device(address: &str) -> String {
    let links: Value =
        serde_json::from_str(&command("/usr/sbin/ip", &["-j", "address", "show"])).unwrap();
    links
        .as_array()
        .unwrap()
        .iter()
        .find(|link| {
            link["addr_info"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["local"] == address)
        })
        .unwrap()["ifname"]
        .as_str()
        .unwrap()
        .to_owned()
}
fn route(family: &str, destination: &str, gateway: &str) {
    command(
        "/usr/sbin/ip",
        &[family, "route", "add", destination, "via", gateway],
    );
}
fn app_command(binary: &Path) -> Command {
    let mut command = Command::new("/usr/bin/setpriv");
    command
        .args([
            "--reuid=10001",
            "--regid=10001",
            "--clear-groups",
            "--no-new-privs",
            "--bounding-set=-all",
            "--inh-caps=-all",
            "--ambient-caps=-all",
        ])
        .arg(binary);
    command
}
fn profile(master: &Value) -> PathBuf {
    let root = Path::new(ROOT);
    fs::set_permissions(root, fs::Permissions::from_mode(0o733)).unwrap();
    let (mut parent, child) = skvoz_network_native::FramedUnix::pair().unwrap();
    let mut command = app_command(&std::env::current_exe().unwrap());
    command.arg("--profile").arg(root.join("profile"));
    inherit_control(&mut command, child.as_fd(), 3).unwrap();
    let mut child_process = Process(command.spawn().unwrap());
    drop(command);
    drop(child);
    parent
        .send_frame(&serde_json::to_vec(master).unwrap(), None)
        .unwrap();
    child_process.exited(true);
    fs::set_permissions(root, fs::Permissions::from_mode(0o755)).unwrap();
    root.join("profile/runtime.json")
}
struct Channel {
    socket: IncrementalUnix,
    id: u32,
    events: VecDeque<Value>,
}
impl Channel {
    fn receive(&mut self, until: Instant) -> (Value, Option<OwnedFd>) {
        loop {
            assert!(Instant::now() < until, "runtime response deadline");
            self.socket.check_deadlines().unwrap();
            if self.socket.write_pending() {
                match self.socket.try_flush() {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => panic!("runtime write: {e}"),
                }
            }
            match self.socket.try_receive_frame() {
                Ok(frame) => return (parse_strict_json(&frame.body).unwrap(), frame.fd),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(e) => panic!("runtime read: {e}"),
            }
        }
    }
    fn save(&mut self, value: Value, fd: Option<OwnedFd>) {
        assert!(fd.is_none());
        if value["event"] != "STATS" {
            assert!(self.events.len() < 128);
            self.events.push_back(value);
        }
    }
    fn call(&mut self, op: &str, args: Value, fd: Option<OwnedFd>) -> Value {
        self.id += 1;
        self.socket.queue_frame(serde_json::to_vec(&json!({"v":1,"id":self.id,"op":op,"args":args,"fd_count":u8::from(fd.is_some())})).unwrap(),fd).unwrap();
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            let (value, fd) = self.receive(until);
            if value.get("event").is_some() {
                self.save(value, fd);
                continue;
            }
            assert_eq!(value["id"], self.id);
            assert!(value["error"].is_null(), "{op}: {value}");
            assert!(fd.is_none());
            return value["result"].clone();
        }
    }
    fn event(&mut self, event: &str, handle: &Value) -> Value {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(i) = self
                .events
                .iter()
                .position(|v| v["event"] == event && v["data"]["handle"] == *handle)
            {
                return self.events.remove(i).unwrap()["data"].clone();
            }
            assert!(Instant::now() < until, "runtime event deadline: {event}");
            let (value, fd) = self.receive(until);
            self.save(value, fd);
        }
    }
}
struct Runtime {
    child: Process,
    control: Channel,
}
impl Runtime {
    fn start(profile: &Path, helper: Option<BorrowedFd<'_>>) -> Self {
        let (parent, child) = UnixStream::pair().unwrap();
        parent.set_nonblocking(true).unwrap();
        child.set_nonblocking(true).unwrap();
        let mut command = app_command(Path::new("/opt/skvoz-native/bin/skvoz-network-runtime"));
        command
            .arg("--config")
            .arg(profile)
            .args(["--control-fd", "3"])
            .stdin(Stdio::null());
        inherit_control(&mut command, child.as_fd(), 3).unwrap();
        if let Some(fd) = helper {
            command.args(["--helper-fd", "4"]);
            inherit_control(&mut command, fd, 4).unwrap();
        }
        let process = Process(command.spawn().unwrap());
        drop(command);
        drop(child);
        let mut runtime = Self {
            child: process,
            control: Channel {
                socket: IncrementalUnix::from_owned_fd(parent.into()).unwrap(),
                id: 0,
                events: VecDeque::new(),
            },
        };
        runtime
            .control
            .call("HELLO", json!({"api":1,"network":6}), None);
        let until = Instant::now() + Duration::from_secs(20);
        while runtime.control.call("STATUS", json!({}), None)["lifecycle"] != "ready" {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(20));
        }
        platform::capabilities(&format!("/proc/{}/status", runtime.child.0.id()), 0);
        let status = fs::read_to_string(format!("/proc/{}/status", runtime.child.0.id())).unwrap();
        assert!(
            status
                .lines()
                .any(|line| line == "Uid:\t10001\t10001\t10001\t10001")
        );
        runtime
    }
    fn shutdown(&mut self) {
        self.control.call("PREPARE_SHUTDOWN", json!({}), None);
        self.child.exited(true);
    }
}
fn server_namespace(egress: &str) {
    let gateway = format!("{V}1");
    command(
        "/usr/bin/timeout",
        &[
            "--kill-after=1",
            "2",
            "/usr/sbin/ip",
            "-6",
            "route",
            "replace",
            "default",
            "via",
            &gateway,
            "dev",
            egress,
        ],
    );
    let dump = |args: &[&str]| -> Value {
        let mut bounded = vec!["--kill-after=1", "2", "/usr/sbin/ip"];
        bounded.extend_from_slice(args);
        serde_json::from_str(&command("/usr/bin/timeout", &bounded)).unwrap()
    };
    let addresses = dump(&["-d", "-j", "address", "show"]);
    let routes4 = dump(&["-4", "-j", "route", "show", "table", "all"]);
    let routes6 = dump(&["-6", "-j", "route", "show", "table", "all"]);
    let limits: [(&str, u64); 8] = [
        ("/proc/sys/net/ipv4/ip_forward", 1),
        ("/proc/sys/net/ipv4/ipfrag_high_thresh", 4194304),
        ("/proc/sys/net/ipv4/ipfrag_time", 15),
        ("/proc/sys/net/ipv6/conf/all/forwarding", 1),
        ("/proc/sys/net/ipv6/ip6frag_high_thresh", 4194304),
        ("/proc/sys/net/ipv6/ip6frag_time", 15),
        (
            "/proc/sys/net/netfilter/nf_conntrack_frag6_high_thresh",
            4194304,
        ),
        ("/proc/sys/net/netfilter/nf_conntrack_frag6_timeout", 15),
    ];
    let mut sysctl = serde_json::Map::new();
    for (path, _) in limits {
        let value = match fs::read_to_string(path) {
            Ok(value) => json!(value.trim()),
            Err(error)
                if error.kind() == io::ErrorKind::NotFound && path.contains("/netfilter/") =>
            {
                Value::Null
            }
            Err(error) => panic!("server namespace setting {path}: {error}"),
        };
        sysctl.insert(path.into(), value);
    }
    // Namespace metadata only: no runtime profile, credentials or TLS material.
    let snapshot = json!({"egress":egress,"gateway":gateway,"addresses":addresses,
        "routes4":routes4,"routes6":routes6,"sysctl":sysctl});
    println!("Server namespace preflight {snapshot}");
    let link = snapshot["addresses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|link| link["ifname"] == egress)
        .unwrap();
    assert!(
        link["flags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|flag| flag == "UP")
    );
    assert!(link["addr_info"].as_array().unwrap().iter().any(|address| {
        address["local"] == format!("{V}20")
            && !address["tentative"].as_bool().unwrap_or(false)
            && !address["dadfailed"].as_bool().unwrap_or(false)
    }));
    assert!(snapshot["routes6"].as_array().unwrap().iter().any(|route| {
        route["dst"] == "default"
            && route["dev"] == egress
            && route["gateway"] == gateway
            && route["type"].as_str().is_none_or(|kind| kind == "unicast")
    }));
    for (path, max) in limits {
        if let Some(value) = snapshot["sysctl"][path].as_str() {
            assert!(
                (1..=max).contains(&value.parse::<u64>().unwrap()),
                "server namespace setting {path}"
            );
        }
    }
}
fn server(mut master: Value) {
    command("/usr/sbin/ip", &["link", "set", "lo", "up"]);
    master["runtime"]["server"]["ipv4"]["interface"] = json!(device(&format!("{E}20")));
    master["runtime"]["server"]["ipv6"]["interface"] =
        master["runtime"]["server"]["ipv4"]["interface"].clone();
    server_namespace(
        master["runtime"]["server"]["ipv6"]["interface"]
            .as_str()
            .unwrap(),
    );
    let profile = profile(&master);
    let config = Path::new(ROOT).join("helper.json");
    private(
        &config,
        &serde_json::to_vec(&json!({
            "v":1,"role":"server","state_dir":"/var/lib/skvoz-network",
            "policy":{"network":master["runtime"]["network"],"server":master["runtime"]["server"]}
        }))
        .unwrap(),
    );
    fs::create_dir_all("/var/lib/skvoz-network").unwrap();
    fs::set_permissions("/var/lib/skvoz-network", fs::Permissions::from_mode(0o700)).unwrap();
    let (left, right) = UnixStream::pair().unwrap();
    left.set_nonblocking(true).unwrap();
    right.set_nonblocking(true).unwrap();
    let mut command = Command::new("/usr/bin/setpriv");
    command
        .args([
            "--no-new-privs",
            "--bounding-set=-all,+net_admin",
            "--inh-caps=-all",
            "--ambient-caps=-all",
            "/opt/skvoz-native/bin/skvoz-network-helper",
            "--config",
        ])
        .arg(config)
        .args(["--control-fd", "3"]);
    inherit_control(&mut command, right.as_fd(), 3).unwrap();
    let mut helper = Process(command.spawn().unwrap());
    drop(command);
    drop(right);
    let mut runtime = Runtime::start(&profile, Some(left.as_fd()));
    drop(left);
    platform::capabilities(&format!("/proc/{}/status", helper.0.id()), 1 << 12);
    let socket = Path::new(ROOT).join("command.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(Path::new(ROOT).join("ready"), b"ready").unwrap();
    let until = Instant::now() + Duration::from_secs(210);
    loop {
        assert!(Instant::now() < until, "server fixture shutdown deadline");
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(true).unwrap();
                let mut control =
                    skvoz_network_native::FramedUnix::from_owned_fd(stream.into()).unwrap();
                let request = control
                    .receive_frame_until(Instant::now() + Duration::from_secs(5))
                    .unwrap();
                assert_eq!(request.body, b"shutdown");
                runtime.shutdown();
                helper.exited(true);
                platform::restored();
                control.send_frame(b"clean", None).unwrap();
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(e) => panic!("server control: {e}"),
        }
    }
}
fn client(master: Value) {
    let _services = platform::services();
    command("/usr/sbin/ip", &["link", "set", "lo", "up"]);
    route("-4", "198.19.0.0/24", &format!("{U}20"));
    route("-6", "2001:db8:900::/64", "2001:db8:800::20");
    let interface = device(&format!("{U}10"));
    traffic::physical(&interface, true);
    let profile = profile(&master);
    // Fixture services and their UID/capability setup are supplied by bootstrap.
    assert!(Path::new("/run/dbus/system_bus_socket").exists());
    let socket = Path::new("/run/skvoz-network-helper/10001/control.sock");
    fs::create_dir_all(socket.parent().unwrap()).unwrap();
    fs::set_permissions(socket.parent().unwrap(), fs::Permissions::from_mode(0o711)).unwrap();
    fs::set_permissions(
        socket.parent().unwrap().parent().unwrap(),
        fs::Permissions::from_mode(0o711),
    )
    .unwrap();
    let state = Path::new("/var/lib/skvoz-network-helper/10001");
    fs::create_dir_all(state).unwrap();
    fs::set_permissions(state, fs::Permissions::from_mode(0o700)).unwrap();
    let helper_config = Path::new(ROOT).join("helper.json");
    private(
        &helper_config,
        &serde_json::to_vec(&json!({"v":1,"role":"client","state_dir":state,"policy":null}))
            .unwrap(),
    );
    let mut helper = platform::helper(&helper_config, socket);
    let mut owner = platform::owner(socket);
    assert_eq!(owner.call("RECOVER", json!({})).0["state"], "idle");
    platform::capabilities(&format!("/proc/{}/status", helper.0.id()), 1 << 12);
    let mut runtime = Runtime::start(&profile, None);
    let handle = runtime.control.call(
        "START_IP",
        json!({"family_policy":"require_all","families":[4,6],"max_mtu":1280,"channels":1}),
        None,
    )["handle"]
        .clone();
    let config = runtime.control.event("CONFIGURED", &handle)["config"].clone();
    assert_eq!(config["families"], json!([4, 6]));
    assert_eq!(config["mtu"], 1280);
    let (prepared,tun)=owner.call("PREPARE_CLIENT",json!({"handle":handle,"config":config,"transport_endpoints":[{"ip":format!("{U}2"),"port":4222}]}));
    runtime.control.call(
        "ATTACH_IP",
        json!({"handle":handle,"interface":prepared["interface"],"mtu":prepared["mtu"]}),
        Some(tun.unwrap()),
    );
    owner.call("ACTIVATE_CLIENT", json!({"handle":handle}));
    runtime
        .control
        .call("LOCAL_READY", json!({"handle":handle}), None);
    runtime.control.event("ACTIVE", &handle);
    traffic::physical(&interface, false);
    traffic::broker_exception(&interface);
    let grant6 = config["source_grants"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .find(|s| s.contains(':'))
        .unwrap()
        .split('/')
        .next()
        .unwrap();
    traffic::through_tunnel(grant6);
    // Detect actual runtime death before performing the host's ABORT/owner EOF.
    runtime.child.0.kill().unwrap();
    runtime.child.exited(false);
    traffic::physical(&interface, false);
    traffic::broker_exception(&interface);
    assert!(command("/usr/sbin/nft", &["list", "tables"]).contains("skvoz_network"));
    owner.call("ABORT_CLIENT", json!({"handle":handle}));
    owner.close();
    helper.exited(true);
    let mut helper = platform::helper(&helper_config, socket);
    let mut owner = platform::owner(socket);
    let recovered = owner.call("RECOVER", json!({})).0;
    assert_eq!(recovered["state"], "guarded");
    assert_eq!(recovered["handle"], handle);
    traffic::physical(&interface, false);
    traffic::broker_exception(&interface);
    owner.call(
        "RESTORE_CLIENT",
        json!({"handle":handle,"reason":"user_stop"}),
    );
    owner.close();
    helper.exited(true);
    platform::restored();
    traffic::physical(&interface, true);
    println!("Frozen runtime/helper/Core NAT44/IPv6/DNS/guard/runtime-crash/recovery passed");
}
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if platform::child_mode() {
        return;
    }
    if args.len() == 2 && args[0] == "--profile" {
        platform::app_identity();
        let mut input = skvoz_network_native::FramedUnix::from_owned_fd(
            skvoz_network_native::adopt_inherited(3).unwrap(),
        )
        .unwrap();
        let bytes = input
            .receive_frame_until(Instant::now() + Duration::from_secs(5))
            .unwrap()
            .body;
        let master = parse_strict_json(&bytes).unwrap();
        let root = PathBuf::from(&args[1]);
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        private(
            &root.join("runtime.json"),
            &serde_json::to_vec(&master["runtime"]).unwrap(),
        );
        private(
            &root.join("ca.pem"),
            master["ca_pem"].as_str().unwrap().as_bytes(),
        );
        return;
    }
    if args.len() == 1 && args[0] == "--shutdown" {
        let stream = UnixStream::connect(Path::new(ROOT).join("command.sock")).unwrap();
        stream.set_nonblocking(true).unwrap();
        let mut channel = skvoz_network_native::FramedUnix::from_owned_fd(stream.into()).unwrap();
        channel.send_frame(b"shutdown", None).unwrap();
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            match channel.receive_frame() {
                Ok(frame) => {
                    assert_eq!(frame.body, b"clean");
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < until);
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("server shutdown reply: {e}"),
            }
        }
        return;
    }
    if args.len() != 1 {
        panic!("explicit fixture actor required");
    }
    let role = args[0].to_str().unwrap();
    if !matches!(
        role,
        "prepare" | "endpoint" | "server" | "client" | "kernel"
    ) {
        panic!("explicit fixture actor required");
    }
    assert_eq!(std::env::var("SKVOZ_KERNEL_FIXTURE").as_deref(), Ok("1"));
    envelope(role);
    if role == "kernel" {
        let egress = device(&format!("{E}20"));
        server_namespace(&egress);
        // The client helper preserves its broker route on the physical interface.
        command(
            "/usr/sbin/ip",
            &["address", "add", "198.18.0.10/24", "dev", &egress],
        );
        for (binary, args) in [
            (
                "linux_tun",
                vec!["--ignored", "--test-threads=1", "--nocapture"],
            ),
            (
                "kernel_lifecycle",
                vec!["--ignored", "--test-threads=1", "--nocapture"],
            ),
            ("kernel_client", vec![]),
        ] {
            assert!(
                Command::new(format!("/fixture/bin/{binary}"))
                    .args(args)
                    .status()
                    .unwrap()
                    .success(),
                "kernel fixture failed: {binary}"
            );
        }
        return;
    }
    if role == "prepare" {
        prepare::inputs(Path::new("/outputs"));
        return;
    }
    fs::create_dir_all(ROOT).unwrap();
    fs::set_permissions(ROOT, fs::Permissions::from_mode(0o755)).unwrap();
    let master =
        parse_strict_json(&fs::read(format!("/fixture/input/{role}/master.json")).unwrap())
            .unwrap();
    match role {
        "server" => server(master),
        "client" => client(master),
        "endpoint" => traffic::endpoint(master),
        _ => unreachable!(),
    }
}
