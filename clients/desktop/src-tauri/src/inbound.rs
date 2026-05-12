//! Inbound message stream — merges the WS subscriber and the (optional)
//! webhook receiver into a single deduped channel.

use std::sync::Arc;

use push_core::receiver::{Forwarder as PcForwarder, InboundMessage};
use push_core::{Dedup, Receiver as PcReceiver, ReceiverConfig};
use tokio::sync::mpsc;
use vocalcord_ws_client::{Frame, WsClient, WsOptions};

use crate::settings::Settings;

/// Events the runtime loop consumes. Message goes through the
/// approval router; connection events update tray + UI state but
/// don't trigger agent work.
#[derive(Debug, Clone)]
pub enum InboundEvent {
    Message(InboundMessage),
    /// WS handshake completed (Welcome frame received).
    Connected,
    /// Server-initiated close, transport error, or stream end.
    /// The WS client auto-reconnects internally.
    Disconnected(String),
}

/// Spawn the WS subscriber and (if `publicUrl` is set) the local
/// webhook receiver, merging both into the returned mpsc receiver.
///
/// Returns the receiver plus a list of shutdown handles the caller
/// invokes on settings-change or app exit.
pub async fn start(settings: &Settings) -> (mpsc::Receiver<InboundEvent>, Vec<Shutdown>) {
    let dedup = Dedup::default();
    let (out_tx, out_rx) = mpsc::channel::<InboundEvent>(64);
    let mut shutdowns: Vec<Shutdown> = Vec::new();

    // WS subscriber — always running.
    let ws_opts = WsOptions {
        api_key: settings.api_key.clone(),
        ws_base: settings.ws_base.clone(),
        client_id: Some("vocalcord-desktop".into()),
    };
    match WsClient::spawn(ws_opts) {
        Ok(mut ws) => {
            let tx = out_tx.clone();
            let dedup_for_ws = dedup.clone();
            let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = &mut cancel_rx => { ws.stop(); break; }
                        frame = ws.recv() => {
                            match frame {
                                Some(Frame::Message(m)) => {
                                    if dedup_for_ws.check_and_record(&m.id) { continue; }
                                    if tx.send(InboundEvent::Message(m)).await.is_err() { break; }
                                }
                                Some(Frame::Welcome) => {
                                    let _ = tx.send(InboundEvent::Connected).await;
                                }
                                Some(Frame::Closing) => {
                                    let _ = tx.send(InboundEvent::Disconnected("server closing".into())).await;
                                }
                                Some(Frame::ServerError(e)) => {
                                    tracing::warn!(error = %e, "ws server error");
                                    let _ = tx.send(InboundEvent::Disconnected(e)).await;
                                }
                                Some(_) => { /* pong / unknown */ }
                                None => {
                                    let _ = tx.send(InboundEvent::Disconnected("stream ended".into())).await;
                                    break;
                                }
                            }
                        }
                    }
                }
            });
            shutdowns.push(Shutdown::Oneshot(cancel_tx));
        }
        Err(e) => tracing::warn!(error = %e, "ws subscriber failed to start"),
    }

    // Webhook receiver — only if publicUrl set + paid tier. We don't
    // know the tier client-side; the server will 402 on
    // /v1/webhooks/configure. The receiver is bound regardless so
    // verification can use it; if configure fails, we shut it down.
    if settings.public_url.is_some() {
        let dedup_for_hook = dedup.clone();
        let tx = out_tx.clone();
        let forwarder: Arc<dyn PcForwarder> = Arc::new(MpscForwarder {
            tx,
            dedup: dedup_for_hook,
        });
        let secret = generate_secret();
        let cfg = ReceiverConfig {
            bind_addr: format!("0.0.0.0:{}", settings.webhook_port)
                .parse()
                .expect("addr"),
            path: settings.webhook_path.clone(),
            secret: secret.as_bytes().to_vec(),
        };
        match PcReceiver::start(cfg, forwarder).await {
            Ok(rx) => {
                if let Err(e) = configure_webhook(settings, &secret).await {
                    tracing::warn!(error = ?e, "webhook configure failed; tearing down listener");
                    rx.stop();
                } else {
                    shutdowns.push(Shutdown::Receiver(rx));
                }
            }
            Err(e) => tracing::warn!(error = ?e, "webhook receiver bind failed"),
        }
    }

    (out_rx, shutdowns)
}

/// Best-effort: ask vocalcord to (re-)register the webhook URL using
/// our client-provided secret. The server's verification handshake
/// runs synchronously; on failure we get a 4xx/409 back.
async fn configure_webhook(s: &Settings, secret: &str) -> Result<(), String> {
    let api_base = s.api_base.trim_end_matches('/');
    let body = serde_json::json!({
        "url": format!("{}{}", s.public_url.as_deref().unwrap_or(""), s.webhook_path),
        "port": s.webhook_port,
        "path": s.webhook_path,
        "secret": secret,
    });
    let res = reqwest::Client::new()
        .post(format!("{api_base}/v1/webhooks/configure"))
        .bearer_auth(&s.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("configure status {}", res.status()));
    }
    Ok(())
}

fn generate_secret() -> String {
    use base64::Engine;
    let mut bytes = [0u8; 32];
    // OsRng-equivalent: ring would be lighter, but std rand isn't
    // available on stable. Use getrandom via uuid's RNG.
    let id1 = uuid::Uuid::new_v4();
    let id2 = uuid::Uuid::new_v4();
    bytes[..16].copy_from_slice(id1.as_bytes());
    bytes[16..].copy_from_slice(id2.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

struct MpscForwarder {
    tx: mpsc::Sender<InboundEvent>,
    dedup: Dedup,
}

impl PcForwarder for MpscForwarder {
    fn forward(
        &self,
        msg: InboundMessage,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<(), String>> + Send + '_>,
    > {
        Box::pin(async move {
            if self.dedup.check_and_record(&msg.id) {
                return Ok(());
            }
            self.tx
                .send(InboundEvent::Message(msg))
                .await
                .map_err(|_| "downstream receiver closed".to_string())
        })
    }
}

/// Owns one of the inbound subsystems. Calling `shutdown()` (or
/// dropping the [`Vec<Shutdown>`]) tears it down.
pub enum Shutdown {
    Oneshot(tokio::sync::oneshot::Sender<()>),
    Receiver(PcReceiver),
}

impl Shutdown {
    pub fn shutdown(self) {
        match self {
            Shutdown::Oneshot(tx) => {
                let _ = tx.send(());
            }
            Shutdown::Receiver(r) => r.stop(),
        }
    }
}
