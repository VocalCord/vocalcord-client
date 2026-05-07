//! Minimal HTTP client for OpenClaw Gateway's `POST /hooks/wake`.
//!
//! Used by the webhook receiver and the WS-fallback bridge to
//! deliver inbound vocalcord messages into the local OpenClaw
//! agent. The gateway accepts a tiny payload — `{ text, mode }` —
//! so we keep this focused on that contract.

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum HooksError {
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("gateway returned status {status}: {body}")]
    Status { status: u16, body: String },
}

pub struct HooksClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

#[derive(Serialize)]
struct WakePayload<'a> {
    text: &'a str,
    mode: &'a str,
}

impl HooksClient {
    pub fn new(gateway_base_url: impl Into<String>, hook_token: impl Into<String>) -> Self {
        Self {
            base_url: gateway_base_url.into(),
            token: hook_token.into(),
            http: reqwest::Client::new(),
        }
    }

    /// POST `<base>/hooks/wake` with `{ text, mode: "now" }`.
    pub async fn wake(&self, text: &str) -> Result<(), HooksError> {
        let url = format!("{}/hooks/wake", self.base_url.trim_end_matches('/'));
        let res = self
            .http
            .post(&url)
            .bearer_auth(&self.token)
            .json(&WakePayload { text, mode: "now" })
            .send()
            .await?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(HooksError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(())
    }
}
