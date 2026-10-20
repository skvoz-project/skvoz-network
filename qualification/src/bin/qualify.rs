//! Run already built owning test executables against one private verified TLS broker.
use serde_json::{Value, json};
use skvoz_network::{
    SessionId,
    config::{Limits, Role},
};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
struct Fixture {
    root: PathBuf,
    broker: Option<Child>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(child) = self.broker.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn checked(mut command: Command) {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "fixture command failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn private(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
fn openssl(root: &Path, args: &[&str]) {
    let mut cmd = Command::new("openssl");
    cmd.current_dir(root).args(args);
    checked(cmd);
}
fn main() {
    let tests: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    assert!(
        !tests.is_empty(),
        "expected built owning test executable paths"
    );
    let token = SessionId::random().unwrap().as_str().to_owned();
    let mut fixture = Fixture {
        root: std::env::temp_dir().join(format!("network-tls-{token}")),
        broker: None,
    };
    fs::create_dir(&fixture.root).unwrap();
    fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o700)).unwrap();
    openssl(
        &fixture.root,
        &[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "ca.key",
            "-out",
            "ca.pem",
            "-days",
            "1",
            "-subj",
            "/CN=Network fixture CA",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
        ],
    );
    openssl(
        &fixture.root,
        &[
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            "leaf.key",
            "-out",
            "leaf.csr",
            "-subj",
            "/CN=localhost",
        ],
    );
    fs::write(fixture.root.join("leaf.ext"),"basicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1,DNS:localhost\n").unwrap();
    openssl(
        &fixture.root,
        &[
            "x509",
            "-req",
            "-in",
            "leaf.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-CAcreateserial",
            "-out",
            "leaf.pem",
            "-days",
            "1",
            "-extfile",
            "leaf.ext",
        ],
    );
    for file in fs::read_dir(&fixture.root).unwrap() {
        fs::set_permissions(file.unwrap().path(), fs::Permissions::from_mode(0o600)).unwrap();
    }
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let password = SessionId::random().unwrap().as_str().to_owned();
    let users: Vec<_> = ["p0", "p1", "peer0", "peer1", "peer2", "ffi-p0", "ffi-p1"]
        .iter()
        .map(|user| json!({"user":user,"password":password}))
        .collect();
    private(
        &fixture.root.join("nats.json"),
        &json!({"host":"127.0.0.1","port":port,"max_payload":65588,"authorization":{"users":users},"tls":{"cert_file":fixture.root.join("leaf.pem"),"key_file":fixture.root.join("leaf.key"),"timeout":3}}),
    );
    let version = Command::new("nats-server")
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&version.stdout).contains("v2.15.1"),
        "fixture NATS pin mismatch"
    );
    fixture.broker = Some(
        Command::new("nats-server")
            .arg("--config")
            .arg(fixture.root.join("nats.json"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let until = Instant::now() + Duration::from_secs(5);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < until, "TLS broker startup deadline");
        thread::sleep(Duration::from_millis(10));
    }
    let url = format!("tls://127.0.0.1:{port}");
    let namespace = format!("skvoz.runtime.{token}");
    let runtime = fixture.root.join("runtime.json");
    private(
        &runtime,
        &json!({"url":url,"ca":fixture.root.join("ca.pem"),"namespace":namespace,"passwords":{"0":password,"1":password,"2":password}}),
    );
    let echo = TcpListener::bind("127.0.0.1:0").unwrap();
    let echo_port = echo.local_addr().unwrap().port();
    echo.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let done = stop.clone();
    let echo_thread = thread::spawn(move || {
        while !done.load(Ordering::Acquire) {
            match echo.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut bytes = [0; 4096];
                    while let Ok(n) = stream.read(&mut bytes) {
                        if n == 0 {
                            break;
                        }
                        stream.write_all(&bytes[..n]).unwrap();
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("echo accept: {e}"),
            }
        }
    });
    let mut client: Value = serde_json::from_slice(include_bytes!(
        "../../../tests/fixtures/client-startup.json"
    ))
    .unwrap();
    client["core"]["url"] = json!(url);
    client["core"]["ca_file"] = json!(fixture.root.join("ca.pem"));
    client["core"]["trust"] = json!("managed_ca");
    client["core"]["namespace"] = json!(format!("skvoz.ffi.{token}"));
    client["core"]["username"] = json!("ffi-p1");
    client["core"]["password"] = json!(password);
    client["core"]["peer_id"] = json!("1");
    let mut server = client.clone();
    server["role"] = json!("server");
    server["core"]["username"] = json!("ffi-p0");
    server["core"]["peer_id"] = json!("0");
    server["core"]["allowed_peers"] = json!(["1"]);
    server["core"]["initiate"] = json!([]);
    server["network"]["families"] = json!([]);
    server["network"]["limits"] = serde_json::to_value(Limits::canonical(Role::Server)).unwrap();
    server["access"] = json!({"revision":1,"devices":[1],"revoke":[]});
    server["server"] = json!({"ipv4":null,"ipv6":null,"dns_servers":[],"allow":[{"cidr":"127.0.0.1/32","protocols":[6],"ports":[echo_port]}],"deny":[],"service_prefixes":[],"lease_store":fixture.root.join("leases.json"),"server_addresses":[],"management_endpoints":[]});
    let server_path = fixture.root.join("server.json");
    let client_path = fixture.root.join("client.json");
    private(&server_path, &server);
    private(&client_path, &client);
    let mut success = true;
    for test in tests {
        let mut command = Command::new(&test);
        command.args(["--test-threads=1", "--nocapture"]);
        if test
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("runtime_access-")
            || test
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("real_nats-")
        {
            command.arg("--ignored");
        }
        command
            .env("SKVOZ_NATS_URL", &url)
            .env("SKVOZ_NATS_CA", fixture.root.join("ca.pem"))
            .env("SKVOZ_NATS_RUN_TOKEN", &token)
            .env("SKVOZ_NATS_P0_PASSWORD", &password)
            .env("SKVOZ_NATS_P1_PASSWORD", &password)
            .env("SKVOZ_RUNTIME_FIXTURE", &runtime)
            .env("SKVOZ_NETWORK_SERVER_CONFIG", &server_path)
            .env("SKVOZ_NETWORK_CLIENT_CONFIG", &client_path)
            .env("SKVOZ_NETWORK_ECHO_HOST", "127.0.0.1")
            .env("SKVOZ_NETWORK_ECHO_PORT", echo_port.to_string());
        let status = command.status().unwrap();
        if !status.success() {
            success = false;
            break;
        }
    }
    stop.store(true, Ordering::Release);
    echo_thread.join().unwrap();
    drop(fixture);
    assert!(success, "owning real-TLS qualification failed");
}
