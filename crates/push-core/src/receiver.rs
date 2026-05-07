//! HTTP receiver for vocalcord webhook deliveries.
//!
//! Binds an axum server on a caller-supplied address, verifies the
//! `X-VocalCord-Signature` HMAC on every POST, and either:
//!
//! 1. Echoes back the verification challenge — used during webhook
//!    setup, when vocalcord sends `{"type":"verification","challenge":"…"}`
//!    and expects `{"challenge":"<echo>"}` in reply (see the
//!    `verify_url` helper in vocalcord-webhook server-side).
//! 2. Forwards the inbound message into the supplied callback,
//!    after deduping by `id` so concurrent webhook + WS deliveries
//!    only fire once.
//!
//! The receiver only knows about HTTP-shaped delivery; routing the
//! decoded message into a specific OpenClaw `/hooks/wake` (or some
//! other surface) is the caller's job, supplied as a `Forwarder`.

use crate::dedup::Dedup;
use crate::hmac_verify::{verify, DEFAULT_REPLAY_WINDOW};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::oneshot;

/// Inbound message payload — mirrors vocalcord's `MessageEvent`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InboundMessage {
    pub id: String,
    pub thread_id: String,
    pub channel: String,
    pub from: String,
    #[serde(default)]
    pub from_display: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    pub body: String,
    pub ts: String,
    #[serde(default)]
    pub parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VerificationPing {
    challenge: String,
}

/// Pluggable forwarder. Implementations receive a deduped, HMAC-
/// verified message and decide what to do with it (POST to
/// `/hooks/wake`, emit a desktop notification, write to a queue,
/// etc.).
pub trait Forwarder: Send + Sync + 'static {
    fn forward(
        &self,
        msg: InboundMessage,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;
}

#[derive(Clone)]
pub struct ReceiverConfig {
    /// Address to bind. Use `127.0.0.1:<port>` for loopback or
    /// `0.0.0.0:<port>` to accept connections on all interfaces.
    pub bind_addr: SocketAddr,
    /// HTTP path the listener serves (e.g. `"/vocalcord/inbound"`).
    pub path: String,
    /// Shared HMAC secret returned by `POST /v1/webhooks/configure`.
    pub secret: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReceiverError {
    #[error("bind failed: {0}")]
    Bind(#[from] std::io::Error),
    #[error("server task ended unexpectedly")]
    ServerEnded,
}

/// Handle to a running receiver. Drop or `stop()` to shut down.
pub struct Receiver {
    shutdown: Option<oneshot::Sender<()>>,
    pub local_addr: SocketAddr,
    pub dedup: Dedup,
}

#[derive(Clone)]
struct AppState {
    secret: Arc<Vec<u8>>,
    dedup: Dedup,
    forwarder: Arc<dyn Forwarder>,
}

impl Receiver {
    /// Spawn the listener. Returns once it has bound the port; the
    /// task continues running in the background until `stop()` is
    /// called or the handle is dropped.
    pub async fn start(
        cfg: ReceiverConfig,
        forwarder: Arc<dyn Forwarder>,
    ) -> Result<Self, ReceiverError> {
        let dedup = Dedup::default();
        let state = AppState {
            secret: Arc::new(cfg.secret),
            dedup: dedup.clone(),
            forwarder,
        };

        let app = Router::new().route(&cfg.path, post(handle)).with_state(state);

        let listener = tokio::net::TcpListener::bind(cfg.bind_addr).await?;
        let local_addr = listener.local_addr()?;
        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move {
            let server = axum::serve(listener, app);
            tokio::select! {
                res = server => {
                    if let Err(e) = res {
                        tracing::error!(?e, "vocalcord receiver: server task errored");
                    }
                }
                _ = rx => {
                    tracing::info!("vocalcord receiver: shutting down on signal");
                }
            }
        });

        Ok(Self {
            shutdown: Some(tx),
            local_addr,
            dedup,
        })
    }

    pub fn stop(mut self) {
        if let Some(s) = self.shutdown.take() {
            let _ = s.send(());
        }
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        if let Some(s) = self.shutdown.take() {
            let _ = s.send(());
        }
    }
}

async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let sig_header = match headers
        .get("X-VocalCord-Signature")
        .and_then(|v| v.to_str().ok())
    {
        Some(h) => h,
        None => return (StatusCode::UNAUTHORIZED, "missing signature").into_response(),
    };

    if let Err(e) = verify(
        sig_header,
        body.as_ref(),
        &state.secret,
        DEFAULT_REPLAY_WINDOW,
        SystemTime::now(),
    ) {
        tracing::warn!(?e, "vocalcord receiver: signature verify failed");
        return (StatusCode::UNAUTHORIZED, e.to_string()).into_response();
    }

    // Try to parse as a verification handshake first. The handshake
    // body has a top-level `type: "verification"` and a `challenge`.
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) {
        if v.get("type").and_then(|t| t.as_str()) == Some("verification") {
            let ping: VerificationPing = match serde_json::from_value(v) {
                Ok(p) => p,
                Err(_) => return (StatusCode::BAD_REQUEST, "bad verification body").into_response(),
            };
            return Json(serde_json::json!({ "challenge": ping.challenge })).into_response();
        }
    }

    // Not a verification — must be an InboundMessage.
    let msg: InboundMessage = match serde_json::from_slice(&body) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(%e, "vocalcord receiver: malformed message body");
            return (StatusCode::BAD_REQUEST, "malformed body").into_response();
        }
    };

    if state.dedup.check_and_record(&msg.id) {
        // Already delivered via the other path — silently ack.
        return StatusCode::OK.into_response();
    }

    if let Err(e) = state.forwarder.forward(msg).await {
        tracing::error!(%e, "vocalcord receiver: forwarder failed");
        return (StatusCode::BAD_GATEWAY, e).into_response();
    }
    StatusCode::OK.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use std::sync::Mutex;

    struct Recorder {
        seen: Arc<Mutex<Vec<InboundMessage>>>,
    }

    impl Forwarder for Recorder {
        fn forward(
            &self,
            msg: InboundMessage,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            let seen = self.seen.clone();
            Box::pin(async move {
                seen.lock().unwrap().push(msg);
                Ok(())
            })
        }
    }

    fn sign(secret: &[u8], body: &[u8]) -> String {
        let ts = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut m = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        m.update(format!("{ts}.").as_bytes());
        m.update(body);
        format!("t={ts},v1={}", hex::encode(m.finalize().into_bytes()))
    }

    #[tokio::test]
    async fn full_round_trip_message_then_dedup_then_handshake() {
        let secret = b"sek";
        let seen: Arc<Mutex<Vec<InboundMessage>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::new(Recorder { seen: seen.clone() });

        let cfg = ReceiverConfig {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            path: "/inbound".to_string(),
            secret: secret.to_vec(),
        };
        let rx = Receiver::start(cfg, recorder).await.unwrap();
        let url = format!("http://{}/inbound", rx.local_addr);
        let client = reqwest::Client::new();

        // 1. real message
        let body = serde_json::to_vec(&serde_json::json!({
            "id": "m1",
            "thread_id": "t1",
            "channel": "telegram",
            "from": "+1",
            "body": "hi",
            "ts": "2026-05-07T00:00:00Z"
        })).unwrap();
        let sig = sign(secret, &body);
        let res = client.post(&url).header("X-VocalCord-Signature", &sig).body(body.clone()).send().await.unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(seen.lock().unwrap().len(), 1);

        // 2. duplicate (same id) — should not forward again
        let res = client.post(&url).header("X-VocalCord-Signature", &sig).body(body).send().await.unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(seen.lock().unwrap().len(), 1);

        // 3. verification handshake echoes challenge
        let body = serde_json::to_vec(&serde_json::json!({
            "type": "verification",
            "challenge": "abc123"
        })).unwrap();
        let sig = sign(secret, &body);
        let res = client.post(&url).header("X-VocalCord-Signature", &sig).body(body).send().await.unwrap();
        assert_eq!(res.status(), 200);
        let echo: serde_json::Value = res.json().await.unwrap();
        assert_eq!(echo["challenge"], "abc123");
        assert_eq!(seen.lock().unwrap().len(), 1, "verification should not deliver to forwarder");

        rx.stop();
    }

    #[tokio::test]
    async fn bad_signature_rejected() {
        let secret = b"sek";
        let seen: Arc<Mutex<Vec<InboundMessage>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::new(Recorder { seen: seen.clone() });

        let cfg = ReceiverConfig {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            path: "/inbound".to_string(),
            secret: secret.to_vec(),
        };
        let rx = Receiver::start(cfg, recorder).await.unwrap();
        let url = format!("http://{}/inbound", rx.local_addr);

        let res = reqwest::Client::new()
            .post(&url)
            .header("X-VocalCord-Signature", "t=1,v1=deadbeef")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401);
        assert_eq!(seen.lock().unwrap().len(), 0);
        rx.stop();
    }
}
