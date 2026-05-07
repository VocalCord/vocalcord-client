//! Shared client-side core for Vocal Cord integrations.
//!
//! Provides the pieces every Vocal Cord client (the OpenClaw plugin,
//! the upcoming Tauri desktop app, future browser/CLI/mobile
//! companions) needs to receive inbound messages and forward them
//! into a local agent / notification surface:
//!
//! - [`hmac_verify`] — parse and verify the `X-VocalCord-Signature`
//!   header that vocalcord stamps on every webhook delivery.
//! - [`receiver`] — an axum-based HTTP listener that verifies HMAC,
//!   handles the verification handshake, dedupes by message id, and
//!   forwards into a `Forwarder` callback.
//! - [`hooks_client`] — minimal HTTP client for OpenClaw Gateway's
//!   `POST /hooks/wake`.
//! - [`rate_limit`] — an in-memory gate honoring 429 + Retry-After
//!   so that callers can short-circuit further outbound calls within
//!   the indicated window.
//! - [`dedup`] — a small LRU of recent message ids so when both the
//!   webhook and the WS path deliver the same message, only the first
//!   one fires.

pub mod dedup;
pub mod hmac_verify;
pub mod hooks_client;
pub mod rate_limit;
pub mod receiver;

pub use dedup::Dedup;
pub use hmac_verify::{verify, HmacError};
pub use hooks_client::{HooksClient, HooksError};
pub use rate_limit::Gate;
pub use receiver::{InboundMessage, Receiver, ReceiverConfig, ReceiverError};
