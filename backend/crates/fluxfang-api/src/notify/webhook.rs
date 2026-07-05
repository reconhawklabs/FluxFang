//! Webhook delivery channel (Task 8.2): JSON POST via `reqwest`, optionally
//! HMAC-SHA256-signed.
//!
//! Tested end-to-end (see `mod tests`) against a real HTTP server spun up
//! locally in-process with `axum`/`tokio` — `dispatch` is exercised exactly
//! as production calls it, no injected client, and the test asserts on
//! what the server actually received (method, headers, JSON body, and the
//! signature header when a secret is configured).

use std::collections::HashMap;

use hmac::{Hmac, Mac};
use reqwest::Method;
use serde::Deserialize;
use sha2::Sha256;

use fluxfang_db::models::AlertMethod;

use super::{decrypt_config, DeliveryStatus, NotificationPayload};

type HmacSha256 = Hmac<Sha256>;

/// Header carrying the HMAC-SHA256 signature of the raw JSON body, when
/// `config.secret` is set: `X-FluxFang-Signature: sha256=<hex>`.
const SIGNATURE_HEADER: &str = "X-FluxFang-Signature";

fn default_method() -> String {
    "POST".to_string()
}

/// Decrypted `config_encrypted` shape for `alert_method.type = 'webhook'`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WebhookConfig {
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub secret: Option<String>,
}

/// Decrypt `method`'s config and POST (or whatever `config.method` says)
/// `payload` as JSON to `config.url`. Never panics — bad key/config JSON,
/// an invalid HTTP method string, a network error, or a non-2xx response
/// all become `DeliveryStatus::Failed`.
pub(crate) async fn dispatch(
    method: &AlertMethod,
    key: &[u8; 32],
    payload: &NotificationPayload,
) -> DeliveryStatus {
    let config: WebhookConfig = match decrypt_config(method, key) {
        Ok(c) => c,
        Err(reason) => return DeliveryStatus::Failed(reason),
    };

    let body = match serde_json::to_vec(payload) {
        Ok(b) => b,
        Err(e) => return DeliveryStatus::Failed(format!("failed to serialize payload: {e}")),
    };

    let http_method = match Method::from_bytes(config.method.as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            return DeliveryStatus::Failed(format!("invalid HTTP method: {}", config.method))
        }
    };

    let client = reqwest::Client::new();
    let mut request = client
        .request(http_method, &config.url)
        .header("content-type", "application/json");

    for (name, value) in &config.headers {
        request = request.header(name.as_str(), value.as_str());
    }

    if let Some(secret) = &config.secret {
        let signature = match sign(secret, &body) {
            Ok(sig) => sig,
            Err(reason) => return DeliveryStatus::Failed(reason),
        };
        request = request.header(SIGNATURE_HEADER, format!("sha256={signature}"));
    }

    match request.body(body).send().await {
        Ok(resp) if resp.status().is_success() => DeliveryStatus::Delivered,
        Ok(resp) => DeliveryStatus::Failed(format!("webhook returned status {}", resp.status())),
        Err(e) => DeliveryStatus::Failed(format!("webhook request failed: {e}")),
    }
}

/// HMAC-SHA256 `body` under `secret`, hex-encoded.
fn sign(secret: &str, body: &[u8]) -> Result<String, String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| format!("failed to initialize HMAC: {e}"))?;
    mac.update(body);
    Ok(hex::encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::any;
    use axum::Router;
    use chrono::Utc;
    use fluxfang_core::secrets::encrypt;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex as AsyncMutex;
    use uuid::Uuid;

    type CapturedRequests = Arc<AsyncMutex<Vec<(HeaderMap, Bytes)>>>;

    #[derive(Clone)]
    struct StubState {
        requests: CapturedRequests,
        response_status: StatusCode,
    }

    async fn capture_handler(
        State(state): State<StubState>,
        headers: HeaderMap,
        body: Bytes,
    ) -> StatusCode {
        state.requests.lock().await.push((headers, body));
        state.response_status
    }

    /// Spin up a real local HTTP server on an OS-assigned port that
    /// records every request it receives at `/hook` and always answers
    /// with `response_status`. Returns the full URL to POST to and a
    /// handle to the captured requests.
    async fn spawn_stub_server(response_status: StatusCode) -> (String, CapturedRequests) {
        let requests: CapturedRequests = Arc::new(AsyncMutex::new(Vec::new()));
        let state = StubState {
            requests: requests.clone(),
            response_status,
        };
        let app = Router::new()
            .route("/hook", any(capture_handler))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stub server");
        let addr = listener.local_addr().expect("local_addr");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("stub server crashed");
        });

        (format!("http://{addr}/hook"), requests)
    }

    fn test_key() -> [u8; 32] {
        [0x22u8; 32]
    }

    fn method_with_config(config: serde_json::Value) -> AlertMethod {
        let ciphertext = encrypt(&test_key(), config.to_string().as_bytes());
        AlertMethod {
            id: Uuid::new_v4(),
            created_at: Utc::now(),
            name: "test webhook".into(),
            type_: "webhook".into(),
            enabled: true,
            config: serde_json::json!({}),
            config_encrypted: Some(ciphertext),
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
    async fn dispatch_posts_expected_json_and_signature() {
        let (url, requests) = spawn_stub_server(StatusCode::OK).await;
        let method = method_with_config(serde_json::json!({
            "url": url,
            "method": "POST",
            "headers": {"X-Custom": "abc"},
            "secret": "shh-its-a-secret",
        }));

        let status = dispatch(&method, &test_key(), &payload()).await;
        assert_eq!(status, DeliveryStatus::Delivered);

        let reqs = requests.lock().await;
        assert_eq!(reqs.len(), 1, "server should have received exactly one request");
        let (headers, body) = &reqs[0];

        let received: serde_json::Value = serde_json::from_slice(body).expect("valid JSON body");
        assert_eq!(received["title"], "Entity Bob's Phone entered zone Work");
        assert_eq!(
            received["body"],
            "Bob's Phone entered Work at 2026-07-05T12:00:00Z"
        );
        assert_eq!(received["context"]["zone"], "Work");

        assert_eq!(headers.get("x-custom").unwrap(), "abc");

        let sig_header = headers
            .get("x-fluxfang-signature")
            .expect("signature header present")
            .to_str()
            .unwrap();
        let expected_sig = sign("shh-its-a-secret", body).unwrap();
        assert_eq!(sig_header, format!("sha256={expected_sig}"));
    }

    #[tokio::test]
    async fn dispatch_without_secret_sends_no_signature_header() {
        let (url, requests) = spawn_stub_server(StatusCode::OK).await;
        let method = method_with_config(serde_json::json!({ "url": url }));

        let status = dispatch(&method, &test_key(), &payload()).await;
        assert_eq!(status, DeliveryStatus::Delivered);

        let reqs = requests.lock().await;
        let (headers, _) = &reqs[0];
        assert!(headers.get("x-fluxfang-signature").is_none());
    }

    #[tokio::test]
    async fn dispatch_fails_without_panicking_on_non_2xx_response() {
        let (url, _requests) = spawn_stub_server(StatusCode::INTERNAL_SERVER_ERROR).await;
        let method = method_with_config(serde_json::json!({ "url": url }));

        let status = dispatch(&method, &test_key(), &payload()).await;
        assert!(matches!(status, DeliveryStatus::Failed(_)));
    }

    #[tokio::test]
    async fn dispatch_fails_without_panicking_on_unreachable_url() {
        // Bind then immediately drop a listener to get a port nothing is
        // listening on, so the connection is refused quickly rather than
        // hanging or depending on external network state.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let method = method_with_config(serde_json::json!({
            "url": format!("http://{addr}/hook"),
        }));

        let status = dispatch(&method, &test_key(), &payload()).await;
        assert!(matches!(status, DeliveryStatus::Failed(_)));
    }

    #[tokio::test]
    async fn dispatch_fails_without_panicking_on_undecryptable_config() {
        let wrong_key = [0xAAu8; 32];
        let ciphertext = encrypt(&wrong_key, br#"{"url":"http://example.invalid"}"#);
        let method = AlertMethod {
            id: Uuid::new_v4(),
            created_at: Utc::now(),
            name: "test webhook".into(),
            type_: "webhook".into(),
            enabled: true,
            config: serde_json::json!({}),
            config_encrypted: Some(ciphertext),
        };

        let status = dispatch(&method, &test_key(), &payload()).await;
        assert!(matches!(status, DeliveryStatus::Failed(_)));
    }
}
