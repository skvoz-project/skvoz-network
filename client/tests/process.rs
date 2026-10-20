//! Process-boundary checks use this Rust test executable as the pinned supplier.
use skvoz_network_client::{Credentials, Error, RuntimeClient};
use std::{
    collections::BTreeSet,
    future::Future,
    io::{Read, Write},
    pin::pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    thread,
    time::{Duration, Instant},
};
struct ThreadWake(thread::Thread);
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}
fn wait<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWake(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(result) = future.as_mut().poll(&mut context) {
            return result;
        }
        thread::park_timeout(Duration::from_millis(100));
    }
}
fn children() -> BTreeSet<u32> {
    std::fs::read_dir("/proc/self/task")
        .unwrap()
        .flat_map(|entry| {
            std::fs::read_to_string(entry.unwrap().path().join("children"))
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect::<Vec<_>>()
        })
        .collect()
}
fn main() {
    if std::env::args().nth(1).as_deref() == Some("--enroll-stdio") {
        return supplier();
    }
    let client = RuntimeClient::new(std::env::current_exe().unwrap()).unwrap();
    let mut credentials = Credentials {
        host: "localhost".into(),
        port: 4222,
        username: "success".into(),
        password: "test-password".into(),
        ca_file: String::new(),
        dial_ip: None,
    };
    let device = "0123456789abcdef0123456789abcdef";
    assert_eq!(
        wait(client.enroll(&credentials, device)).unwrap().peer_id,
        1
    );
    let previous = children();
    credentials.ca_file = "\"".repeat(4096);
    assert_eq!(
        wait(client.enroll(&credentials, device)).unwrap_err(),
        Error("enrollment_failed")
    );
    assert_eq!(
        children(),
        previous,
        "oversized encoding spawned a supplier"
    );
    credentials.ca_file.clear();

    for (user, error) in [
        ("duplicate", "enrollment_failed"),
        ("oversized", "enrollment_failed"),
        ("version", "version_mismatch"),
        ("auth", "authentication_failed"),
    ] {
        credentials.username = user.into();
        assert_eq!(
            wait(client.enroll(&credentials, device)).unwrap_err(),
            Error(error)
        );
    }
    credentials.username = "cancel".into();
    let previous = children();
    let mut operation = Box::pin(client.enroll(&credentials, device));
    let waker = Waker::from(Arc::new(ThreadWake(thread::current())));
    assert!(
        operation
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    let until = Instant::now() + Duration::from_secs(2);
    let spawned = loop {
        let fresh: Vec<_> = children().difference(&previous).copied().collect();
        if !fresh.is_empty() {
            break fresh;
        }
        assert!(Instant::now() < until, "supplier process did not start");
        thread::sleep(Duration::from_millis(5));
    };
    drop(operation);
    assert!(
        spawned
            .iter()
            .all(|pid| !std::path::Path::new(&format!("/proc/{pid}")).exists()),
        "cancelled supplier was not killed and reaped"
    );
    credentials.username = "success".into();
    assert!(
        wait(client.enroll(&credentials, device)).is_ok(),
        "admission permit leaked"
    );
    println!(
        "Process SDK checks passed: identity, strict/bounded output, typed errors, cancellation and reap"
    );
}
fn supplier() {
    let mut bytes = Vec::new();
    std::io::stdin().take(8193).read_to_end(&mut bytes).unwrap();
    let input: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(input["credentials"]["password"], "test-password");
    assert_eq!(std::env::args().count(), 2, "credentials escaped into argv");
    let body = match input["credentials"]["username"].as_str().unwrap() {
        "duplicate" => "{\"v\":1,\"v\":1,\"result\":null,\"error\":null}".into(),
        "oversized" => "x".repeat(2049),
        "cancel" => loop {
            thread::sleep(Duration::from_secs(1));
        },
        "auth" => "{\"v\":1,\"result\":null,\"error\":\"authentication_failed\"}".into(),
        user => serde_json::json!({"v":1,"result":{"v":4,"namespace":"fixture","peer_id":1,
            "network_runtime":{"version":if user=="version" {"0.6.0"} else {"0.7.0"},
                "api":1,"network":6,"core":"5.0.0"}},"error":null})
        .to_string(),
    };
    std::io::stdout().write_all(body.as_bytes()).unwrap();
}
