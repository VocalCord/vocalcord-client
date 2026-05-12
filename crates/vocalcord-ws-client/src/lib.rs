//! Rust port of `@vocalcord/sdk`'s WebSocket subscriber.
//!
//! Connects to `wss://api.vocalcord.io/` with the bearer token in
//! the `Sec-WebSocket-Protocol` header (not the URL — query strings
//! get logged by API GW, CloudFront, and the browser; the subprotocol
//! doesn't). Yields decoded [`Frame`]s on an mpsc channel, owns its
//! own exponential-backoff reconnect loop (1s · 2^n capped at 30s),
//! and sends a heartbeat ping every 4 minutes.
//!
//! The crate is transport-only — no app-layer logic. The desktop app
//! glues this together with [`push_core::Dedup`] and the agent
//! runner.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use push_core::receiver::InboundMessage;
use serde::Deserialize;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::time::{sleep, Instant};
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest,
    handshake::client::generate_key,
    http::header::{HeaderValue, SEC_WEBSOCKET_PROTOCOL},
    protocol::Message,
};
use url::Url;

/// Reconnect backoff cap (`@vocalcord/sdk`'s value).
pub const RECONNECT_CAP: Duration = Duration::from_secs(30);
/// Base backoff: 1 second, doubled per attempt.
pub const RECONNECT_BASE: Duration = Duration::from_secs(1);
/// Heartbeat interval (matches the TS SDK).
pub const PING_INTERVAL: Duration = Duration::from_secs(4 * 60);

#[derive(Clone, Debug)]
pub struct WsOptions {
    /// Vocal Cord account UUID (acts as bearer token).
    pub api_key: String,
    /// WebSocket base, e.g. `wss://api.vocalcord.io/`. The path is
    /// always `/`.
    pub ws_base: String,
    /// Optional informational client id reported in the connect URL.
    pub client_id: Option<String>,
}

/// Decoded vocalcord WS frame.
#[derive(Debug, Clone)]
pub enum Frame {
    Welcome,
    Message(InboundMessage),
    Pong,
    Closing,
    /// Server-side error frame; parameter is the prose payload.
    ServerError(String),
    /// Unrecognised frame — kept rather than dropped so callers can
    /// log it without us having to rev the crate on every server-side
    /// addition.
    Unknown(serde_json::Value),
}

#[derive(Debug, Error)]
pub enum WsError {
    #[error("bad ws base URL: {0}")]
    BadUrl(String),
    #[error("connect failed: {0}")]
    Connect(String),
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum WireFrame {
    #[serde(rename = "welcome")]
    Welcome,
    #[serde(rename = "message")]
    Message(InboundMessage),
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "closing")]
    Closing,
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        message: String,
    },
}

/// Compute the next backoff delay for a given retry attempt
/// (`attempt >= 0`). Capped at [`RECONNECT_CAP`]. Pure function
/// for testability.
pub fn backoff_for(attempt: u32) -> Duration {
    let factor = 1u64
        .checked_shl(attempt)
        .unwrap_or(u64::MAX);
    let delay = RECONNECT_BASE.saturating_mul(factor as u32);
    delay.min(RECONNECT_CAP)
}

/// Live WS subscriber handle. The receiver yields [`Frame`]s; the
/// client task auto-reconnects in the background until [`stop`]
/// is called or the handle is dropped.
pub struct WsClient {
    rx: mpsc::Receiver<Frame>,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl WsClient {
    /// Spawn the reconnecting loop and return a handle.
    pub fn spawn(opts: WsOptions) -> Result<Self, WsError> {
        // Validate the base URL up front so the caller fails fast.
        let _ = Url::parse(&opts.ws_base).map_err(|e| WsError::BadUrl(e.to_string()))?;
        let (tx, rx) = mpsc::channel(64);
        let (sh_tx, sh_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(reconnect_loop(opts, tx, sh_rx));
        Ok(Self {
            rx,
            shutdown: sh_tx,
        })
    }

    /// Take the next frame; `None` when the connection task has
    /// shut down.
    pub async fn recv(&mut self) -> Option<Frame> {
        self.rx.recv().await
    }

    /// Signal shutdown. The background task drops the socket and
    /// exits; subsequent `recv()` calls return `None`.
    pub fn stop(&self) {
        let _ = self.shutdown.send(true);
    }
}

impl Drop for WsClient {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

async fn reconnect_loop(
    opts: WsOptions,
    tx: mpsc::Sender<Frame>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut attempt: u32 = 0;
    loop {
        if *shutdown.borrow() {
            return;
        }
        match one_connect(&opts, &tx, &mut shutdown).await {
            Ok(()) => {
                // Clean close. Reset backoff and reconnect.
                attempt = 0;
            }
            Err(e) => {
                tracing::warn!(error = %e, attempt, "ws connect failed");
            }
        }
        if *shutdown.borrow() {
            return;
        }
        let delay = backoff_for(attempt);
        attempt = attempt.saturating_add(1);
        tokio::select! {
            _ = sleep(delay) => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() { return; }
            }
        }
    }
}

async fn one_connect(
    opts: &WsOptions,
    tx: &mpsc::Sender<Frame>,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(), WsError> {
    let mut url = Url::parse(&opts.ws_base).map_err(|e| WsError::BadUrl(e.to_string()))?;
    if let Some(cid) = &opts.client_id {
        url.query_pairs_mut().append_pair("client", cid);
    }
    let host_header = url
        .host_str()
        .ok_or_else(|| WsError::BadUrl("missing host".into()))?
        .to_string();
    // Build a tungstenite request manually so we can set the
    // `Sec-WebSocket-Protocol` header. The TS SDK passes the bearer
    // as the second subprotocol value (first is the literal
    // `"vocalcord"`).
    let mut req = url
        .as_str()
        .into_client_request()
        .map_err(|e| WsError::BadUrl(e.to_string()))?;
    let protocol_val = format!("vocalcord, {}", opts.api_key);
    let headers = req.headers_mut();
    headers.insert(
        SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_str(&protocol_val).map_err(|e| WsError::BadUrl(e.to_string()))?,
    );
    headers.insert("host", HeaderValue::from_str(&host_header).map_err(|e| WsError::BadUrl(e.to_string()))?);
    headers.insert("sec-websocket-key", HeaderValue::from_str(&generate_key()).expect("key"));
    headers.insert("sec-websocket-version", HeaderValue::from_static("13"));
    headers.insert("upgrade", HeaderValue::from_static("websocket"));
    headers.insert("connection", HeaderValue::from_static("Upgrade"));

    let (ws_stream, _) = tokio_tungstenite::connect_async(req)
        .await
        .map_err(|e| WsError::Connect(e.to_string()))?;

    let (mut sink, mut stream) = ws_stream.split();
    let mut next_ping = Instant::now() + PING_INTERVAL;

    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    let _ = sink.close().await;
                    return Ok(());
                }
            }
            _ = tokio::time::sleep_until(next_ping) => {
                if sink.send(Message::Text(r#"{"type":"ping"}"#.to_string())).await.is_err() {
                    return Ok(());
                }
                next_ping = Instant::now() + PING_INTERVAL;
            }
            msg = stream.next() => {
                let Some(msg) = msg else { return Ok(()); };
                let msg = match msg {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!(error = %e, "ws read error");
                        return Ok(());
                    }
                };
                match msg {
                    Message::Text(t) => {
                        let frame = parse_frame(&t);
                        if tx.send(frame).await.is_err() {
                            return Ok(());
                        }
                    }
                    Message::Binary(_) => {
                        // Vocal Cord doesn't use binary frames today.
                        // Ignore rather than tear down.
                    }
                    Message::Ping(payload) => {
                        if sink.send(Message::Pong(payload)).await.is_err() {
                            return Ok(());
                        }
                    }
                    Message::Pong(_) => {}
                    Message::Close(_) => return Ok(()),
                    Message::Frame(_) => {}
                }
            }
        }
    }
}

fn parse_frame(text: &str) -> Frame {
    match serde_json::from_str::<WireFrame>(text) {
        Ok(WireFrame::Welcome) => Frame::Welcome,
        Ok(WireFrame::Message(m)) => Frame::Message(m),
        Ok(WireFrame::Pong) => Frame::Pong,
        Ok(WireFrame::Closing) => Frame::Closing,
        Ok(WireFrame::Error { message }) => Frame::ServerError(message),
        Err(_) => {
            let raw: serde_json::Value =
                serde_json::from_str(text).unwrap_or(serde_json::Value::String(text.to_owned()));
            Frame::Unknown(raw)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_schedule() {
        assert_eq!(backoff_for(0), Duration::from_secs(1));
        assert_eq!(backoff_for(1), Duration::from_secs(2));
        assert_eq!(backoff_for(2), Duration::from_secs(4));
        assert_eq!(backoff_for(3), Duration::from_secs(8));
        assert_eq!(backoff_for(4), Duration::from_secs(16));
        // Cap kicks in at 30s.
        assert_eq!(backoff_for(5), Duration::from_secs(30));
        assert_eq!(backoff_for(6), Duration::from_secs(30));
        assert_eq!(backoff_for(20), Duration::from_secs(30));
        // Doesn't panic on huge attempt counts.
        assert_eq!(backoff_for(u32::MAX), Duration::from_secs(30));
    }

    #[test]
    fn parses_welcome() {
        let frame = parse_frame(r#"{"type":"welcome"}"#);
        assert!(matches!(frame, Frame::Welcome));
    }

    #[test]
    fn parses_message() {
        let json = r#"{
            "type":"message",
            "id":"m1",
            "thread_id":"t1",
            "channel":"telegram",
            "from":"+1",
            "body":"hi",
            "ts":"2026-05-11T00:00:00Z"
        }"#;
        match parse_frame(json) {
            Frame::Message(m) => {
                assert_eq!(m.id, "m1");
                assert_eq!(m.channel, "telegram");
                assert_eq!(m.body, "hi");
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn parses_pong_and_closing() {
        assert!(matches!(parse_frame(r#"{"type":"pong"}"#), Frame::Pong));
        assert!(matches!(parse_frame(r#"{"type":"closing"}"#), Frame::Closing));
    }

    #[test]
    fn parses_server_error() {
        let f = parse_frame(r#"{"type":"error","message":"rate limited"}"#);
        match f {
            Frame::ServerError(m) => assert_eq!(m, "rate limited"),
            other => panic!("expected ServerError, got {other:?}"),
        }
    }

    #[test]
    fn unknown_frame_preserved() {
        let f = parse_frame(r#"{"type":"future_thing","x":42}"#);
        match f {
            Frame::Unknown(v) => {
                assert_eq!(v["x"], 42);
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }
}
