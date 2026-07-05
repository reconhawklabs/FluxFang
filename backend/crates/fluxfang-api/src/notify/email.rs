//! Email delivery channel (Task 8.2): SMTP via `lettre`.
//!
//! Sending is factored through [`lettre::AsyncTransport`] via `send_via`
//! rather than being hardcoded to `AsyncSmtpTransport`, specifically so
//! tests can substitute `lettre::transport::stub::AsyncStubTransport`
//! (records messages in memory, no network) for the real SMTP connection.
//! `dispatch` (the only entry point used in production) is the one place
//! that builds the real transport; everything else in this module —
//! config parsing, message building, and the actual send — is exercised
//! directly by tests against the stub.

use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::Deserialize;

use fluxfang_db::models::AlertMethod;

use super::{decrypt_config, DeliveryStatus, NotificationPayload};

/// Decrypted `config_encrypted` shape for `alert_method.type = 'email'`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct EmailConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from: String,
    pub to: String,
    pub tls: bool,
}

/// Decrypt `method`'s config, build the message, and send it over a real
/// SMTP transport built from that config. Never panics — every failure
/// mode (bad key, bad config JSON, unparsable address, unreachable/
/// unauthenticated SMTP server) becomes `DeliveryStatus::Failed`.
pub(crate) async fn dispatch(
    method: &AlertMethod,
    key: &[u8; 32],
    payload: &NotificationPayload,
) -> DeliveryStatus {
    let config: EmailConfig = match decrypt_config(method, key) {
        Ok(c) => c,
        Err(reason) => return DeliveryStatus::Failed(reason),
    };

    let transport = match build_transport(&config) {
        Ok(t) => t,
        Err(reason) => return DeliveryStatus::Failed(reason),
    };

    dispatch_with_transport(&config, payload, &transport).await
}

/// The decrypt-independent half of [`dispatch`]: build the message from an
/// already-decrypted config and send it over an arbitrary
/// [`AsyncTransport`]. Kept separate (and `pub(crate)`) so tests can drive
/// the exact same config-parse -> message-build -> send pipeline
/// `dispatch` uses, but with a `lettre::transport::stub::AsyncStubTransport`
/// standing in for the network.
pub(crate) async fn dispatch_with_transport<T>(
    config: &EmailConfig,
    payload: &NotificationPayload,
    transport: &T,
) -> DeliveryStatus
where
    T: AsyncTransport + Sync,
    T::Error: std::fmt::Display,
{
    let message = match build_message(config, payload) {
        Ok(m) => m,
        Err(reason) => return DeliveryStatus::Failed(reason),
    };
    send_via(transport, message).await
}

/// Build the outgoing message: `from`/`to` from config, subject/body from
/// the payload.
fn build_message(config: &EmailConfig, payload: &NotificationPayload) -> Result<Message, String> {
    let from: Mailbox = config
        .from
        .parse()
        .map_err(|e| format!("invalid 'from' address: {e}"))?;
    let to: Mailbox = config
        .to
        .parse()
        .map_err(|e| format!("invalid 'to' address: {e}"))?;

    Message::builder()
        .from(from)
        .to(to)
        .subject(&payload.title)
        .body(payload.body.clone())
        .map_err(|e| format!("failed to build email message: {e}"))
}

/// Build the production SMTP transport from config. `tls: true` uses
/// `relay` (implicit TLS/STARTTLS negotiated by `lettre`); `tls: false`
/// uses `builder_dangerous` (plaintext) — acceptable here since it's
/// explicitly opted into per-method, e.g. for a local mail relay on the
/// same host.
fn build_transport(config: &EmailConfig) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
    let creds = Credentials::new(config.username.clone(), config.password.clone());

    let builder = if config.tls {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host)
            .map_err(|e| format!("failed to configure SMTP relay: {e}"))?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.host)
    };

    Ok(builder.port(config.port).credentials(creds).build())
}

/// Send `message` over any `AsyncTransport`. The only place that touches
/// the network (or, in tests, the stub) in this module.
async fn send_via<T>(transport: &T, message: Message) -> DeliveryStatus
where
    T: AsyncTransport + Sync,
    T::Error: std::fmt::Display,
{
    match transport.send(message).await {
        Ok(_) => DeliveryStatus::Delivered,
        Err(e) => DeliveryStatus::Failed(format!("SMTP send failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lettre::transport::stub::AsyncStubTransport;

    fn config() -> EmailConfig {
        EmailConfig {
            host: "smtp.example.com".into(),
            port: 587,
            username: "user".into(),
            password: "pass".into(),
            from: "alerts@fluxfang.local".into(),
            to: "ops@fluxfang.local".into(),
            tls: true,
        }
    }

    fn payload() -> NotificationPayload {
        NotificationPayload {
            title: "Entity Bob's Phone entered zone Work".into(),
            body: "Bob's Phone entered Work at 2026-07-05T12:00:00Z".into(),
            context: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn dispatch_with_stub_transport_sends_exactly_one_well_formed_message() {
        let cfg = config();
        let transport = AsyncStubTransport::new_ok();

        let status = dispatch_with_transport(&cfg, &payload(), &transport).await;
        assert_eq!(status, DeliveryStatus::Delivered);

        let sent = transport.messages().await;
        assert_eq!(sent.len(), 1, "exactly one message should have been sent");
        let (envelope, raw) = &sent[0];

        assert_eq!(envelope.from().unwrap().to_string(), cfg.from);
        assert_eq!(envelope.to().len(), 1);
        assert_eq!(envelope.to()[0].to_string(), cfg.to);
        assert!(raw.contains("Subject: Entity Bob's Phone entered zone Work"));
        assert!(raw.contains("Bob's Phone entered Work at 2026-07-05T12:00:00Z"));
    }

    #[tokio::test]
    async fn stub_transport_error_becomes_failed_status_not_panic() {
        let cfg = config();
        let transport = AsyncStubTransport::new_error();

        let status = dispatch_with_transport(&cfg, &payload(), &transport).await;
        assert!(matches!(status, DeliveryStatus::Failed(_)));
    }

    #[tokio::test]
    async fn invalid_from_address_fails_without_panicking() {
        let mut cfg = config();
        cfg.from = "not-an-email".into();
        let transport = AsyncStubTransport::new_ok();

        let status = dispatch_with_transport(&cfg, &payload(), &transport).await;
        assert!(matches!(status, DeliveryStatus::Failed(_)));
        assert_eq!(transport.messages().await.len(), 0);
    }
}
