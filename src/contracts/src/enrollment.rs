//! Enrollment4 DTOs and current component identity validation.
use serde::Deserialize;
#[derive(serde::Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Enrollment {
    pub v: u8,
    pub namespace: String,
    pub peer_id: u64,
    pub network_runtime: RuntimeVersion,
}
#[derive(serde::Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct RuntimeVersion {
    pub version: String,
    pub api: u8,
    pub network: u8,
    pub core: String,
}
#[derive(Clone, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub ca_file: String,
    pub dial_ip: Option<std::net::IpAddr>,
}
impl Drop for Credentials {
    fn drop(&mut self) {
        self.password.clear();
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error(pub &'static str);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Error {}
pub fn login(input: &str) -> bool {
    !input.is_empty()
        && input.len() <= 64
        && input != "__skvoz_server"
        && input
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
pub fn valid_token(input: &str) -> bool {
    input.len() == 32
        && input
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Enrollment {
    pub fn validate(&self) -> Result<(), Error> {
        if self.v != 4
            || self.network_runtime.version != env!("CARGO_PKG_VERSION")
            || self.network_runtime.api != 1
            || self.network_runtime.network != crate::NETWORK_VERSION
            || self.network_runtime.core != "5.0.0"
        {
            return Err(Error("version_mismatch"));
        }
        if !crate::access::device_id(self.peer_id)
            || self.namespace.len() > 256
            || self.namespace.split('.').any(|part| {
                part.is_empty()
                    || part.len() > 64
                    || !part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
        {
            return Err(Error("enrollment_failed"));
        }
        Ok(())
    }
}

#[derive(serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessRequest {
    pub v: u8,
    pub credentials: Credentials,
    pub device: String,
}
#[derive(serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessReply {
    pub v: u8,
    #[serde(deserialize_with = "crate::config::required_option")]
    pub result: Option<Enrollment>,
    #[serde(deserialize_with = "crate::config::required_option")]
    pub error: Option<String>,
}
pub const PROCESS_INPUT_MAX: usize = 8192;
pub const PROCESS_OUTPUT_MAX: usize = 2048;

/// Borrow credentials while encoding so rejected input does not create secret clones.
pub fn process_request(credentials: &Credentials, device: &str) -> Result<Vec<u8>, Error> {
    #[derive(serde::Serialize)]
    struct Request<'a> {
        v: u8,
        credentials: &'a Credentials,
        device: &'a str,
    }
    if credentials.host.len() > 253 || credentials.ca_file.len() > 4096 {
        return Err(Error("enrollment_failed"));
    }
    crate::local_api::serialize_bounded(
        &Request {
            v: 1,
            credentials,
            device,
        },
        PROCESS_INPUT_MAX,
    )
    .map_err(|_| Error("enrollment_failed"))
}
