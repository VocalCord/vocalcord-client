//! HMAC verification for vocalcord's `X-VocalCord-Signature` header.
//!
//! Wire format (matches the server-side signer in
//! `vocalcord/crates/vocalcord-webhook/src/lib.rs`):
//!
//! ```text
//! X-VocalCord-Signature: t=<unix_ts>,v1=<hex(HMAC-SHA256(secret, "<ts>." || body))>
//! ```
//!
//! Verification:
//! 1. Parse `t=` and `v1=` from the header.
//! 2. Reject if `|now - t| > REPLAY_WINDOW` (default 5 min).
//! 3. Compute `HMAC-SHA256(secret, "<ts>." || body)` over the raw
//!    request body bytes (NOT a re-serialized JSON).
//! 4. Constant-time compare against the parsed `v1` value.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Default replay window. The server's webhook delivery has an 8s
/// timeout so 5 minutes is generous; outside that window we reject.
pub const DEFAULT_REPLAY_WINDOW: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, thiserror::Error)]
pub enum HmacError {
    #[error("missing X-VocalCord-Signature header")]
    Missing,
    #[error("malformed X-VocalCord-Signature header")]
    Malformed,
    #[error("timestamp outside replay window")]
    StaleTimestamp,
    #[error("signature mismatch")]
    BadSignature,
    #[error("hmac key error: {0}")]
    KeyError(String),
}

/// Verify a signature header against the body bytes using the shared
/// secret.
///
/// `now` is parameterized so tests can pin time without touching the
/// system clock.
pub fn verify(
    header: &str,
    body: &[u8],
    secret: &[u8],
    replay_window: Duration,
    now: SystemTime,
) -> Result<(), HmacError> {
    let (ts_str, sig_hex) = parse_header(header)?;
    let ts: u64 = ts_str.parse().map_err(|_| HmacError::Malformed)?;

    let now_unix = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| HmacError::StaleTimestamp)?
        .as_secs();
    let drift = now_unix.abs_diff(ts);
    if drift > replay_window.as_secs() {
        return Err(HmacError::StaleTimestamp);
    }

    let mut mac = Hmac::<Sha256>::new_from_slice(secret).map_err(|e| HmacError::KeyError(e.to_string()))?;
    mac.update(format!("{ts}.").as_bytes());
    mac.update(body);
    let computed = mac.finalize().into_bytes();
    let computed_hex = hex::encode(computed);

    if !constant_time_eq(computed_hex.as_bytes(), sig_hex.as_bytes()) {
        return Err(HmacError::BadSignature);
    }
    Ok(())
}

/// Parse `t=<ts>,v1=<sig>` (order tolerant, whitespace tolerant).
fn parse_header(h: &str) -> Result<(&str, &str), HmacError> {
    let mut ts: Option<&str> = None;
    let mut sig: Option<&str> = None;
    for part in h.split(',') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("t=") {
            ts = Some(v);
        } else if let Some(v) = part.strip_prefix("v1=") {
            sig = Some(v);
        }
    }
    match (ts, sig) {
        (Some(t), Some(s)) if !t.is_empty() && !s.is_empty() => Ok((t, s)),
        _ => Err(HmacError::Malformed),
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    fn sign(secret: &[u8], ts: u64, body: &[u8]) -> String {
        let mut m = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        m.update(format!("{ts}.").as_bytes());
        m.update(body);
        format!("t={ts},v1={}", hex::encode(m.finalize().into_bytes()))
    }

    #[test]
    fn valid_signature() {
        let secret = b"super-secret";
        let body = b"{\"id\":\"x\"}";
        let ts = 1_000_000;
        let h = sign(secret, ts, body);
        let now = UNIX_EPOCH + Duration::from_secs(ts);
        assert!(verify(&h, body, secret, DEFAULT_REPLAY_WINDOW, now).is_ok());
    }

    #[test]
    fn tampered_body_fails() {
        let secret = b"super-secret";
        let ts = 1_000_000;
        let h = sign(secret, ts, b"original");
        let now = UNIX_EPOCH + Duration::from_secs(ts);
        let err = verify(&h, b"tampered", secret, DEFAULT_REPLAY_WINDOW, now).unwrap_err();
        assert!(matches!(err, HmacError::BadSignature));
    }

    #[test]
    fn replay_outside_window_fails() {
        let secret = b"super-secret";
        let body = b"hello";
        let ts = 1_000_000;
        let h = sign(secret, ts, body);
        let now = UNIX_EPOCH + Duration::from_secs(ts + 600); // +10min
        let err = verify(&h, body, secret, DEFAULT_REPLAY_WINDOW, now).unwrap_err();
        assert!(matches!(err, HmacError::StaleTimestamp));
    }

    #[test]
    fn malformed_header_fails() {
        let now = UNIX_EPOCH + Duration::from_secs(1);
        let err = verify("v1=abcd", b"", b"k", DEFAULT_REPLAY_WINDOW, now).unwrap_err();
        assert!(matches!(err, HmacError::Malformed));
    }

    #[test]
    fn order_tolerant() {
        let secret = b"k";
        let ts = 100;
        let body = b"x";
        let mut m = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        m.update(format!("{ts}.").as_bytes());
        m.update(body);
        let sig = hex::encode(m.finalize().into_bytes());
        let header = format!("v1={sig},t={ts}");
        let now = UNIX_EPOCH + Duration::from_secs(ts);
        assert!(verify(&header, body, secret, DEFAULT_REPLAY_WINDOW, now).is_ok());
    }
}
