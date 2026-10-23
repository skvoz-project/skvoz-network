//! Supplier-side enrollment4 over the canonical verified NATS transport.
use futures_util::StreamExt;
pub use skvoz_network_contracts::enrollment::{
    Credentials, Enrollment, Error, RuntimeVersion, login, valid_token,
};
use std::{path::PathBuf, time::Duration};
pub async fn enroll(credentials: &Credentials, device: &str) -> Result<Enrollment, Error> {
    if !login(&credentials.username) {
        return Err(Error("invalid_login"));
    }
    if !(12..=72).contains(&credentials.password.len()) {
        return Err(Error("invalid_password"));
    }
    if !valid_token(device) {
        return Err(Error("enrollment_failed"));
    }
    let operation = async {
        let mut options = async_nats::ConnectOptions::with_user_and_password(
            credentials.username.clone(),
            credentials.password.clone(),
        )
        .require_tls(true)
        .connection_timeout(Duration::from_secs(5))
        .max_reconnects(0)
        .ignore_discovered_servers()
        .client_capacity(8)
        .subscription_capacity(8)
        .raw_message_limit(512)
        .custom_inbox_prefix(format!("skvoz.enroll.reply.{}", credentials.username));
        if !credentials.ca_file.is_empty() {
            options = options.add_root_certificates(PathBuf::from(&credentials.ca_file));
        }
        if credentials.dial_ip.is_some() {
            let trust = if credentials.ca_file.is_empty() {
                skvoz_core_tls::Trust::System
            } else {
                skvoz_core_tls::Trust::ManagedCa(PathBuf::from(&credentials.ca_file))
            };
            options = options.tls_client_config(
                skvoz_core_tls::verified_tls_config(&trust, &credentials.host)
                    .map_err(|_| Error("certificate_failed"))?,
            );
        }
        let dial_host = credentials
            .dial_ip
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| credentials.host.clone());
        let host = if dial_host.contains(':') {
            format!("[{}]", dial_host)
        } else {
            dial_host
        };
        let address = format!("tls://{host}:{}", credentials.port);
        let client = connect_with_auth_retry(options, &address)
            .await
            .map_err(|error| {
                let mut text = error.to_string().to_ascii_lowercase();
                use std::error::Error as _;
                let mut source = error.source();
                while let Some(error) = source {
                    text.push_str(&error.to_string().to_ascii_lowercase());
                    source = error.source();
                }
                #[cfg(feature = "enrollment-diagnostics")]
                {
                    let details = text
                        .replace(&credentials.password.to_ascii_lowercase(), "[redacted]")
                        .replace(&credentials.username.to_ascii_lowercase(), "[redacted]");
                    eprintln!(
                    "Enrollment connection failed: address={host}:{}, kind={:?}, details={details}",
                    credentials.port,
                    error.kind()
                );
                }
                if text.contains("authorization") || text.contains("authentication") {
                    Error("authentication_failed")
                } else if text.contains("certificate")
                    || text.contains("unknownissuer")
                    || text.contains("invalid peer")
                {
                    Error("certificate_failed")
                } else {
                    Error("server_unavailable")
                }
            })?;
        #[cfg(feature = "enrollment-diagnostics")]
        eprintln!(
            "Enrollment transport connected: address={host}:{}",
            credentials.port
        );
        let reply = format!(
            "skvoz.enroll.reply.{}.{}",
            credentials.username,
            token().map_err(|_| Error("random_failed"))?
        );
        let mut subscription = client
            .subscribe(reply.clone())
            .await
            .map_err(|_| Error("enrollment_failed"))?;
        client
            .flush()
            .await
            .map_err(|_| Error("enrollment_failed"))?;
        let body = serde_json::to_vec(&serde_json::json!({"v":4,"device":device}))
            .map_err(|_| Error("enrollment_failed"))?;
        client
            .publish_with_reply(
                format!("skvoz.enroll.v4.{}", credentials.username),
                reply,
                body.into(),
            )
            .await
            .map_err(|_| Error("enrollment_failed"))?;
        let message = subscription
            .next()
            .await
            .ok_or(Error("enrollment_failed"))?;
        #[cfg(feature = "enrollment-diagnostics")]
        eprintln!("Enrollment reply received: bytes={}", message.payload.len());
        if message.payload.len() > 512 {
            return Err(Error("enrollment_failed"));
        }
        let value = crate::local_api::parse_strict_json(&message.payload)
            .map_err(|_| Error("enrollment_failed"))?;
        if let Some(error) = value.get("error") {
            return Err(Error(if error == "device_limit" {
                "device_limit"
            } else {
                "enrollment_failed"
            }));
        }
        let result: Enrollment =
            serde_json::from_value(value).map_err(|_| Error("version_mismatch"))?;
        result.validate()?;
        let _ = client.drain().await;
        Ok(result)
    };
    tokio::time::timeout(Duration::from_secs(20), operation)
        .await
        .map_err(|_| {
            #[cfg(feature = "enrollment-diagnostics")]
            eprintln!("Enrollment deadline exceeded");
            Error("server_unavailable")
        })?
}

async fn connect_with_auth_retry(
    options: async_nats::ConnectOptions,
    address: &str,
) -> Result<async_nats::Client, async_nats::ConnectError> {
    // NATS also reports authorization violation before its callout is subscribed.
    // Keep retries finite, inside the enclosing enrollment deadline.
    for delay in [1, 2] {
        match options.clone().connect(address).await {
            Ok(client) => return Ok(client),
            Err(error) if error.kind() != async_nats::ConnectErrorKind::AuthorizationViolation => {
                return Err(error);
            }
            Err(_) => tokio::time::sleep(Duration::from_secs(delay)).await,
        }
    }
    options.connect(address).await
}

fn token() -> Result<String, getrandom::Error> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
