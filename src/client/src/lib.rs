//! Host enrollment through an installed, artifact-verified Network executable.
#![cfg(target_os = "linux")]

pub use skvoz_network_contracts::enrollment::{
    Credentials, Enrollment, Error, RuntimeVersion, login, valid_token,
};
use skvoz_network_contracts::{enrollment::*, local_api::parse_strict_json_bounded};
use std::{
    future::Future,
    io::{self, Read, Write},
    os::{fd::AsFd, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
    pin::Pin,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The installer verifies the executable digest and its complete native closure.
/// Keep this path and its ancestors trusted for the entire operation.
#[derive(Clone)]
pub struct RuntimeClient(PathBuf);
impl RuntimeClient {
    pub fn new(executable: impl AsRef<Path>) -> Result<Self, Error> {
        let path = executable.as_ref();
        let metadata = path.metadata().map_err(|_| Error("server_unavailable"))?;
        if !path.is_absolute()
            || !metadata.is_file()
            || metadata.permissions().mode() & 0o022 != 0
            || metadata.permissions().mode() & 0o111 == 0
        {
            return Err(Error("server_unavailable"));
        }
        Ok(Self(path.to_path_buf()))
    }

    pub async fn enroll(
        &self,
        credentials: &Credentials,
        device: &str,
    ) -> Result<Enrollment, Error> {
        if !login(&credentials.username) {
            return Err(Error("invalid_login"));
        }
        if !(12..=72).contains(&credentials.password.len()) {
            return Err(Error("invalid_password"));
        }
        if !valid_token(device) {
            return Err(Error("enrollment_failed"));
        }
        let input = process_request(credentials, device)?;
        if ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Error("server_unavailable"));
        }
        let shared = Arc::new(Shared::default());
        let worker_state = shared.clone();
        let path = self.0.clone();
        let worker = match thread::Builder::new()
            .name("skvoz-enrollment".into())
            .spawn(move || {
                struct Permit;
                impl Drop for Permit {
                    fn drop(&mut self) {
                        ACTIVE.store(false, Ordering::Release);
                    }
                }
                let _permit = Permit;
                let result = run(&path, input, &worker_state.cancelled);
                let mut state = worker_state
                    .result
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                state.value = Some(result);
                let waker = state.waker.take();
                drop(state);
                if let Some(waker) = waker {
                    waker.wake();
                }
            }) {
            Ok(worker) => worker,
            Err(_) => {
                ACTIVE.store(false, Ordering::Release);
                return Err(Error("server_unavailable"));
            }
        };
        Operation {
            shared,
            worker: Some(worker),
        }
        .await
    }
}

#[derive(Default)]
struct ResultState {
    value: Option<Result<Enrollment, Error>>,
    waker: Option<Waker>,
}
#[derive(Default)]
struct Shared {
    result: Mutex<ResultState>,
    cancelled: AtomicBool,
}
struct Operation {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}
impl Future for Operation {
    type Output = Result<Enrollment, Error>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.shared.result.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(value) = state.value.take() {
            return Poll::Ready(value);
        }
        state.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        self.shared.cancelled.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            // The worker owns kill/wait and all pipes. Cancellation completes
            // cleanup before this operation releases its one-process admission.
            let _ = worker.join();
        }
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn run(path: &Path, mut input: Vec<u8>, cancelled: &AtomicBool) -> Result<Enrollment, Error> {
    let until = Instant::now() + Duration::from_secs(20);
    let mut process = Process(
        Command::new(path)
            .arg("--enroll-stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| Error("server_unavailable"))?,
    );
    let stdin = process.0.stdin.take().ok_or(Error("server_unavailable"))?;
    let mut stdout = process.0.stdout.take().ok_or(Error("server_unavailable"))?;
    skvoz_network_native::set_nonblocking(stdin.as_fd())
        .map_err(|_| Error("server_unavailable"))?;
    skvoz_network_native::set_nonblocking(stdout.as_fd())
        .map_err(|_| Error("server_unavailable"))?;
    let mut stdin = Some(stdin);
    let mut offset = 0;
    let mut output = Vec::with_capacity(PROCESS_OUTPUT_MAX);
    let mut eof = false;
    loop {
        if cancelled.load(Ordering::Acquire) || Instant::now() >= until {
            return Err(Error("server_unavailable"));
        }
        if let Some(pipe) = stdin.as_mut() {
            match pipe.write(&input[offset..]) {
                Ok(0) => return Err(Error("server_unavailable")),
                Ok(n) => offset += n,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return Err(Error("server_unavailable")),
            }
            if offset == input.len() {
                input.fill(0);
                stdin = None;
            }
        }
        let mut chunk = [0; 512];
        match stdout.read(&mut chunk) {
            Ok(0) => eof = true,
            Ok(n) if output.len() + n <= PROCESS_OUTPUT_MAX => {
                output.extend_from_slice(&chunk[..n])
            }
            Ok(_) => return Err(Error("enrollment_failed")),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err(Error("server_unavailable")),
        }
        if let Some(status) = process
            .0
            .try_wait()
            .map_err(|_| Error("server_unavailable"))?
        {
            if !status.success() {
                return Err(Error("server_unavailable"));
            }
            if eof {
                break;
            }
        }
        thread::park_timeout(Duration::from_millis(10));
    }
    let value = parse_strict_json_bounded(&output, PROCESS_OUTPUT_MAX)
        .map_err(|_| Error("enrollment_failed"))?;
    let reply: ProcessReply =
        serde_json::from_value(value).map_err(|_| Error("enrollment_failed"))?;
    match (reply.v, reply.result, reply.error.as_deref()) {
        (1, Some(result), None) => {
            result.validate()?;
            Ok(result)
        }
        (1, None, Some(error)) => Err(Error(match error {
            "invalid_login" => "invalid_login",
            "invalid_password" => "invalid_password",
            "authentication_failed" => "authentication_failed",
            "certificate_failed" => "certificate_failed",
            "server_unavailable" => "server_unavailable",
            "device_limit" => "device_limit",
            "version_mismatch" => "version_mismatch",
            "random_failed" => "random_failed",
            _ => "enrollment_failed",
        })),
        _ => Err(Error("enrollment_failed")),
    }
}
