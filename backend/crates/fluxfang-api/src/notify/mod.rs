//! Notification dispatcher (Task 8.2).
//!
//! When an alert fires (Task 5.3's ingest evaluation) or a user clicks
//! "Send test" on an alert method (Task 6.6), the caller builds a
//! [`NotificationPayload`] and hands it to [`dispatch`] along with the
//! configured [`AlertMethod`] and the process's decryption key (Task 8.1's
//! `FLUXFANG_SECRET_KEY`, already parsed via
//! `fluxfang_core::secrets::key_from_base64`).
//!
//! `dispatch` never panics: decrypt failures, config-parse failures, and
//! send failures (bad creds, unreachable host, non-2xx webhook response,
//! ...) all collapse to `DeliveryStatus::Failed(reason)`. Callers persist
//! the result via `NotificationRepo::insert` (`delivery_status` — see
//! [`DeliveryStatus::as_db_str`] for the exact string) and, for `in_app`,
//! are responsible for the WS broadcast themselves; this module only
//! acknowledges in-app delivery, it doesn't touch the database or a
//! websocket.
//!
//! ## Channel implementations
//!
//! - [`email`]: SMTP via `lettre`, generic over `lettre::AsyncTransport` so
//!   the real `AsyncSmtpTransport` and tests' `AsyncStubTransport` share the
//!   exact same send path (see that module's `send_via`).
//! - [`webhook`]: JSON POST via `reqwest`, optionally HMAC-SHA256-signed
//!   (`X-FluxFang-Signature: sha256=<hex>` over the raw JSON body).
//!
//! `in_app` has no channel-specific code: `config_encrypted` is `{}` and
//! there is nothing to send, so `dispatch` handles it inline.

pub mod email;
pub mod webhook;

use serde::{Deserialize, Serialize};

use fluxfang_core::secrets::decrypt;
use fluxfang_db::models::AlertMethod;

/// The alert content to deliver — identical regardless of channel; each
/// channel implementation decides how to render `title`/`body` (email
/// subject/body, webhook JSON field, ...). `context` carries whatever
/// structured detail the caller wants available to e.g. webhook consumers
/// (entity/zone ids, rule id, ...) beyond the human-readable strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationPayload {
    pub title: String,
    pub body: String,
    pub context: serde_json::Value,
}

/// Outcome of a single delivery attempt.
///
/// Serde-tagged as `{"status": "delivered"}` / `{"status": "failed",
/// "reason": "..."}` for use in API responses (e.g. a synchronous "Send
/// test" result). For persistence in `notification.delivery_status`, use
/// [`DeliveryStatus::as_db_str`] instead — that column is `CHECK`-
/// constrained to `'pending' | 'sent' | 'failed'` and has no room for the
/// failure reason string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "reason", rename_all = "snake_case")]
pub enum DeliveryStatus {
    Delivered,
    Failed(String),
}

impl DeliveryStatus {
    /// The value to store in `notification.delivery_status`. Collapses any
    /// `Failed` reason (the DB column has no separate place to put it —
    /// callers that want the reason preserved should log it or fold it into
    /// `notification.payload` themselves).
    pub fn as_db_str(&self) -> &'static str {
        match self {
            DeliveryStatus::Delivered => "sent",
            DeliveryStatus::Failed(_) => "failed",
        }
    }
}

/// Decrypt `method.config_encrypted` under `key` and parse it as `T`.
///
/// Shared by every channel that has secret config (email, webhook — not
/// in_app, which has none). Never panics: a missing `config_encrypted`,
/// AES-GCM authentication failure (wrong key or tampered ciphertext), or a
/// JSON shape mismatch all become a descriptive `Err`, meant to be folded
/// straight into `DeliveryStatus::Failed`.
pub(crate) fn decrypt_config<T: serde::de::DeserializeOwned>(
    method: &AlertMethod,
    key: &[u8; 32],
) -> Result<T, String> {
    let ciphertext = method
        .config_encrypted
        .as_ref()
        .ok_or_else(|| "alert method has no config_encrypted set".to_string())?;
    let plaintext = decrypt(key, ciphertext).map_err(|e| e.to_string())?;
    serde_json::from_slice(&plaintext).map_err(|e| format!("failed to parse config: {e}"))
}

/// Dispatch `payload` via `method`'s configured channel.
///
/// `method.type_` selects the channel (`"email"` | `"webhook"` |
/// `"in_app"`); any other value is a `Failed` (never a panic) — this can
/// happen if the DB's `type` check constraint is ever loosened without this
/// dispatcher being updated to match.
pub async fn dispatch(
    method: &AlertMethod,
    key: &[u8; 32],
    payload: &NotificationPayload,
) -> DeliveryStatus {
    match method.type_.as_str() {
        "email" => email::dispatch(method, key, payload).await,
        "webhook" => webhook::dispatch(method, key, payload).await,
        "in_app" => DeliveryStatus::Delivered,
        other => DeliveryStatus::Failed(format!("unknown alert method type: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use fluxfang_core::secrets::encrypt;
    use uuid::Uuid;

    fn test_key() -> [u8; 32] {
        [0x11u8; 32]
    }

    fn method(type_: &str, config_encrypted: Option<Vec<u8>>) -> AlertMethod {
        AlertMethod {
            id: Uuid::new_v4(),
            created_at: Utc::now(),
            name: "test method".into(),
            type_: type_.into(),
            enabled: true,
            config: serde_json::json!({}),
            config_encrypted,
        }
    }

    fn payload() -> NotificationPayload {
        NotificationPayload {
            title: "Entity Bob's Phone entered zone Work".into(),
            body: "Bob's Phone entered Work at 2026-07-05T12:00:00Z".into(),
            context: serde_json::json!({"entity": "Bob's Phone", "zone": "Work"}),
        }
    }

    #[tokio::test]
    async fn in_app_dispatch_is_always_delivered() {
        let m = method("in_app", None);
        let status = dispatch(&m, &test_key(), &payload()).await;
        assert_eq!(status, DeliveryStatus::Delivered);
    }

    #[tokio::test]
    async fn unknown_method_type_fails_without_panicking() {
        let m = method("carrier_pigeon", None);
        let status = dispatch(&m, &test_key(), &payload()).await;
        assert!(matches!(status, DeliveryStatus::Failed(_)));
    }

    #[tokio::test]
    async fn missing_config_encrypted_fails_without_panicking() {
        let m = method("email", None);
        let status = dispatch(&m, &test_key(), &payload()).await;
        assert!(matches!(status, DeliveryStatus::Failed(_)));
    }

    #[tokio::test]
    async fn config_encrypted_under_wrong_key_fails_without_panicking() {
        let wrong_key = [0x99u8; 32];
        let ciphertext = encrypt(&wrong_key, br#"{"url":"http://example.invalid"}"#);
        let m = method("webhook", Some(ciphertext));
        let status = dispatch(&m, &test_key(), &payload()).await;
        assert!(matches!(status, DeliveryStatus::Failed(_)));
    }

    #[test]
    fn delivery_status_db_str_matches_check_constraint() {
        assert_eq!(DeliveryStatus::Delivered.as_db_str(), "sent");
        assert_eq!(
            DeliveryStatus::Failed("boom".into()).as_db_str(),
            "failed"
        );
    }

    #[test]
    fn delivery_status_serializes_tagged() {
        let delivered = serde_json::to_value(DeliveryStatus::Delivered).unwrap();
        assert_eq!(delivered, serde_json::json!({"status": "delivered"}));

        let failed = serde_json::to_value(DeliveryStatus::Failed("boom".into())).unwrap();
        assert_eq!(
            failed,
            serde_json::json!({"status": "failed", "reason": "boom"})
        );
    }
}
