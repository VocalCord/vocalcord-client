//! napi-rs binding exposing `push-core` to the OpenClaw plugin.
//!
//! Three things cross the FFI boundary:
//!
//! 1. `RateLimitGate` — a thin wrapper around `push_core::Gate`. The
//!    plugin's `before_tool_call` / `after_tool_call` hooks call into
//!    this from JavaScript to honor 429 + Retry-After.
//! 2. `Receiver` — an axum HTTP listener that verifies vocalcord's
//!    HMAC signatures, handles the verification handshake, dedupes,
//!    and forwards real messages directly to the local OpenClaw
//!    Gateway via `POST /hooks/wake`. We deliberately keep the
//!    forwarding inside Rust (not a JS callback) so the same code
//!    paths drive the upcoming Tauri desktop app.
//! 3. `wake()` — bare `POST /hooks/wake` helper, used by the
//!    plugin's WebSocket-fallback bridge when the webhook path
//!    can't be set up (e.g. unreachable host, free-tier account).

use napi::bindgen_prelude::*;
use napi_derive::napi;
use push_core::receiver::Forwarder as PcForwarder;
use push_core::{HooksClient, InboundMessage, Receiver as PcReceiver, ReceiverConfig};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// JS-facing wrapper around `push_core::Gate`.
#[napi(js_name = "RateLimitGate")]
pub struct RateLimitGate {
    inner: push_core::Gate,
}

#[napi]
impl RateLimitGate {
    #[napi(constructor)]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            inner: push_core::Gate::new(),
        }
    }

    #[napi]
    pub fn is_blocked(&self) -> bool {
        self.inner.is_blocked()
    }

    #[napi]
    pub fn record_429(&self, retry_after_seconds: u32) {
        self.inner
            .record_429(Duration::from_secs(retry_after_seconds as u64));
    }

    /// Remaining ms until the gate opens; null if not blocked.
    #[napi]
    pub fn remaining_ms(&self) -> Option<u32> {
        self.inner
            .remaining()
            .map(|d| d.as_millis().min(u32::MAX as u128) as u32)
    }

    #[napi]
    pub fn clear(&self) {
        self.inner.clear();
    }
}

#[napi(object)]
pub struct ReceiverOptions {
    /// Bind address, e.g. `"127.0.0.1:18790"`. Use `0.0.0.0:<port>`
    /// to accept connections on all interfaces.
    pub bind_addr: String,
    /// HTTP path the listener serves (e.g. `"/vocalcord/inbound"`).
    pub path: String,
    /// Webhook secret returned by `POST /v1/webhooks/configure`.
    /// Sent as base64url-no-pad by the server. We accept that, plain
    /// base64, or an arbitrary UTF-8 string fallback.
    pub secret: String,
    /// OpenClaw Gateway base URL (e.g.
    /// `"http://127.0.0.1:18789/hooks"` if `hooks.path = "/hooks"`).
    /// Whatever value goes in here, the receiver will append
    /// `/hooks/wake`. Match what is documented in
    /// <https://docs.openclaw.ai/automation/webhook>.
    pub gateway_base_url: String,
    /// Bearer token configured for `hooks.token` on the gateway.
    pub hook_token: String,
}

struct ForwardToGateway {
    client: HooksClient,
}

impl PcForwarder for ForwardToGateway {
    fn forward(
        &self,
        msg: InboundMessage,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), String>> + Send + '_>> {
        Box::pin(async move {
            let from_label = msg.from_display.as_deref().unwrap_or(&msg.from);
            let body_preview = truncate(&msg.body, 280);
            let text = if let Some(subj) = msg.subject.as_deref() {
                format!("New {} from {}: {} — {}", msg.channel, from_label, subj, body_preview)
            } else {
                format!("New {} from {}: {}", msg.channel, from_label, body_preview)
            };
            self.client.wake(&text).await.map_err(|e| e.to_string())
        })
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

fn decode_secret(s: &str) -> Vec<u8> {
    use base64::Engine;
    if let Ok(v) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s) {
        return v;
    }
    if let Ok(v) = base64::engine::general_purpose::STANDARD.decode(s) {
        return v;
    }
    s.as_bytes().to_vec()
}

/// Live receiver handle. Drop or call `stop()` to shut it down.
#[napi]
pub struct Receiver {
    inner: Option<PcReceiver>,
    /// Local socket address the listener is bound to (informational
    /// — useful for logging when port 0 was passed in).
    pub local_addr: String,
}

#[napi]
impl Receiver {
    /// Bind the listener and return a handle.
    #[napi(factory)]
    pub async fn start(opts: ReceiverOptions) -> Result<Receiver> {
        let bind: std::net::SocketAddr = opts
            .bind_addr
            .parse()
            .map_err(|e: std::net::AddrParseError| Error::from_reason(format!("bad bind addr: {e}")))?;
        let cfg = ReceiverConfig {
            bind_addr: bind,
            path: opts.path,
            secret: decode_secret(&opts.secret),
        };
        let client = HooksClient::new(opts.gateway_base_url, opts.hook_token);
        let fwd: Arc<dyn PcForwarder> = Arc::new(ForwardToGateway { client });
        let rx = PcReceiver::start(cfg, fwd)
            .await
            .map_err(|e| Error::from_reason(e.to_string()))?;
        let local_addr = rx.local_addr.to_string();
        Ok(Receiver {
            inner: Some(rx),
            local_addr,
        })
    }

    #[napi]
    pub fn stop(&mut self) {
        if let Some(r) = self.inner.take() {
            r.stop();
        }
    }
}

/// Bare `POST <gateway_base_url>/hooks/wake` for the WS fallback.
#[napi]
pub async fn wake(gateway_base_url: String, hook_token: String, text: String) -> Result<()> {
    let client = HooksClient::new(gateway_base_url, hook_token);
    client
        .wake(&text)
        .await
        .map_err(|e| Error::from_reason(e.to_string()))
}
