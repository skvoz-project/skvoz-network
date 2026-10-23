//! Private, owned TLS/NATS/echo dependencies for actual Rust integration tests.
use serde_json::{Value, json};
use skvoz_network_contracts::{
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
pub struct TlsFixture {
    pub root: PathBuf,
    pub url: String,
    pub token: String,
    pub password: String,
    pub echo_port: u16,
    broker: Option<Child>,
    echo: Option<(Arc<AtomicBool>, thread::JoinHandle<()>)>,
}
impl Drop for TlsFixture {
    fn drop(&mut self) {
        if let Some(child) = self.broker.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some((stop, thread)) = self.echo.take() {
            stop.store(true, Ordering::Release);
            thread.join().expect("echo fixture thread failed");
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
impl TlsFixture {
    pub fn new() -> Self {
        let token = SessionId::random().unwrap().as_str().to_owned();
        let mut fixture = Self {
            root: std::env::temp_dir().join(format!("network-tls-{token}")),
            broker: None,
            echo: None,
            url: String::new(),
            token: token.clone(),
            password: String::new(),
            echo_port: 0,
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
        let version = Command::new(nats_binary())
            .arg("--version")
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&version.stdout).contains("v2.15.1"),
            "fixture NATS pin mismatch"
        );
        fixture.broker = Some(
            Command::new(nats_binary())
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
        fixture.echo_port = echo_port;
        fixture.echo = Some((stop, echo_thread));
        let mut client: Value = serde_json::from_slice(include_bytes!(
            "../../network/tests/fixtures/client-startup.json"
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
        server["network"]["limits"] =
            serde_json::to_value(Limits::canonical(Role::Server)).unwrap();
        server["access"] = json!({"revision":1,"devices":[1],"revoke":[]});
        server["server"] = json!({"ipv4":null,"ipv6":null,"dns_servers":[],"allow":[{"cidr":"127.0.0.1/32","protocols":[6],"ports":[echo_port]}],"deny":[],"service_prefixes":[],"lease_store":fixture.root.join("leases.json"),"server_addresses":[],"management_endpoints":[]});
        let server_path = fixture.root.join("server.json");
        let client_path = fixture.root.join("client.json");
        private(&server_path, &server);
        private(&client_path, &client);
        fixture.url = url;
        fixture.password = password;
        fixture
    }
}
impl Default for TlsFixture {
    fn default() -> Self {
        Self::new()
    }
}

/// One verified download in Cargo's ordinary target cache; every reuse hashes bytes.
pub fn nats_binary() -> PathBuf {
    static INSTALL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _lock = INSTALL.lock().unwrap();
    let cache = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../target"))
        .join("test-inputs/nats-2.15.1");
    fs::create_dir_all(&cache).unwrap();
    let binary = cache.join("nats-server");
    if !binary.exists() {
        let archive = cache.join(format!("server-{}.tar.gz", std::process::id()));
        checked({
            let mut c = Command::new("curl");
            c.args(["--fail", "--location", "--proto", "=https", "--proto-redir", "=https", "--connect-timeout", "10", "--max-time", "120", "https://github.com/nats-io/nats-server/releases/download/v2.15.1/nats-server-v2.15.1-linux-amd64.tar.gz", "--output"]).arg(&archive);
            c
        });
        assert_eq!(
            digest(&archive),
            "bf5edc7b1ce98e885c368cb43a886eb148dc5d0551d99f3cf6141fb5e1954c70"
        );
        checked({
            let mut c = Command::new("tar");
            c.arg("-xzf")
                .arg(&archive)
                .arg("-C")
                .arg(&cache)
                .arg("nats-server-v2.15.1-linux-amd64/nats-server");
            c
        });
        fs::rename(
            cache.join("nats-server-v2.15.1-linux-amd64/nats-server"),
            &binary,
        )
        .unwrap();
        fs::remove_dir(cache.join("nats-server-v2.15.1-linux-amd64")).unwrap();
        fs::remove_file(archive).unwrap();
    }
    assert_eq!(
        digest(&binary),
        "baab2eecb5a8ebdabf6edd3e22756f89805e3184468653ba67185adf5a3b57eb"
    );
    binary.canonicalize().unwrap()
}
fn digest(path: &Path) -> String {
    let out = Command::new("sha256sum").arg(path).output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .into()
}
