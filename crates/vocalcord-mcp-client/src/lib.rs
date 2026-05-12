//! HTTP client for the Vocal Cord MCP server (`POST /mcp`).
//!
//! Wraps the JSON-RPC 2.0 envelope, exposes typed helpers for the
//! three published tools (`SendMessage`, `GetMessages`, `GetMessage`),
//! and surfaces server-side 429 responses as a typed error carrying
//! the `Retry-After` value so callers can short-circuit further
//! requests within the window.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

/// Errors raised by the MCP client.
#[derive(Debug, Error)]
pub enum McpError {
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("rate limited; retry after {retry_after:?}")]
    RateLimited { retry_after: Duration },
    #[error("server error {code}: {message}")]
    Server {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    #[error("bad response: {0}")]
    BadResponse(String),
}

/// Tool response — mirrors the TS SDK shape.
#[derive(Debug, Clone)]
pub struct ToolResponse {
    /// The prose text the LLM would see (concatenated from
    /// `result.content[].text`).
    pub text: String,
    /// Optional structured payload.
    pub structured: Option<Value>,
}

/// `SendMessage` arguments. All fields are optional except `body`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SendMessageArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<String>,
}

/// `GetMessages` arguments.
#[derive(Debug, Clone, Default, Serialize)]
pub struct GetMessagesArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unread_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Clone)]
pub struct McpClient {
    api_base: String,
    api_key: String,
    http: reqwest::Client,
    next_id: std::sync::Arc<AtomicI64>,
}

impl McpClient {
    /// `api_base` is the bare HTTPS root (e.g. `https://api.vocalcord.io`);
    /// the client appends `/mcp` itself.
    pub fn new(api_base: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            api_base: api_base.into(),
            api_key: api_key.into(),
            http: reqwest::Client::new(),
            next_id: std::sync::Arc::new(AtomicI64::new(1)),
        }
    }

    /// Override the underlying HTTP client (useful for setting custom
    /// timeouts or pooling settings shared with other crates).
    pub fn with_client(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    /// Generic `tools/call`. Prefer the typed wrappers below.
    pub async fn call_tool(
        &self,
        name: &str,
        args: &Value,
    ) -> Result<ToolResponse, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let url = format!("{}/mcp", self.api_base.trim_end_matches('/'));
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": args },
        });
        let res = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&payload)
            .send()
            .await?;

        let status = res.status();
        if status.as_u16() == 429 {
            let secs = res
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(60);
            return Err(McpError::RateLimited {
                retry_after: Duration::from_secs(secs),
            });
        }

        let body: Value = res
            .json()
            .await
            .map_err(|e| McpError::BadResponse(e.to_string()))?;
        parse_response(body)
    }

    pub async fn send_message(&self, args: SendMessageArgs) -> Result<ToolResponse, McpError> {
        let v = serde_json::to_value(args).map_err(|e| McpError::BadResponse(e.to_string()))?;
        self.call_tool("SendMessage", &v).await
    }

    pub async fn get_messages(&self, args: GetMessagesArgs) -> Result<ToolResponse, McpError> {
        let v = serde_json::to_value(args).map_err(|e| McpError::BadResponse(e.to_string()))?;
        self.call_tool("GetMessages", &v).await
    }

    pub async fn get_message(&self, id: &str) -> Result<ToolResponse, McpError> {
        self.call_tool("GetMessage", &json!({ "id": id })).await
    }
}

#[derive(Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(default)]
    data: Option<Value>,
}

#[derive(Deserialize)]
struct JsonRpcResult {
    #[serde(default)]
    content: Vec<ContentPart>,
    #[serde(default, rename = "structuredContent")]
    structured_content: Option<Value>,
}

#[derive(Deserialize)]
struct ContentPart {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

fn parse_response(body: Value) -> Result<ToolResponse, McpError> {
    if let Some(err_val) = body.get("error") {
        let err: JsonRpcError = serde_json::from_value(err_val.clone())
            .map_err(|e| McpError::BadResponse(e.to_string()))?;
        // The MCP server surfaces 429s as JSON-RPC errors with
        // `data.retryAfterSec`. Detect by presence (matches the
        // OpenClaw plugin's contract).
        if let Some(data) = &err.data {
            if let Some(ra) = data.get("retryAfterSec").and_then(Value::as_u64) {
                return Err(McpError::RateLimited {
                    retry_after: Duration::from_secs(ra),
                });
            }
        }
        return Err(McpError::Server {
            code: err.code,
            message: err.message,
            data: err.data,
        });
    }
    let Some(res_val) = body.get("result") else {
        return Err(McpError::BadResponse("missing result".into()));
    };
    let parsed: JsonRpcResult = serde_json::from_value(res_val.clone())
        .map_err(|e| McpError::BadResponse(e.to_string()))?;
    let text = parsed
        .content
        .iter()
        .filter(|c| c.kind == "text")
        .filter_map(|c| c.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(ToolResponse {
        text,
        structured: parsed.structured_content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ok_response() {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "content": [
                    { "type": "text", "text": "Message sent." }
                ],
                "structuredContent": { "id": "m1" }
            }
        });
        let r = parse_response(body).unwrap();
        assert_eq!(r.text, "Message sent.");
        assert!(r.structured.is_some());
    }

    #[test]
    fn parse_rate_limited_via_data() {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": -32600,
                "message": "rate_limit_exceeded: bucket=SendBurst",
                "data": { "retryAfterSec": 30, "bucket": "SendBurst" }
            }
        });
        match parse_response(body).unwrap_err() {
            McpError::RateLimited { retry_after } => {
                assert_eq!(retry_after, Duration::from_secs(30));
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn parse_generic_server_error() {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": -32600,
                "message": "channel kind blorp not registered"
            }
        });
        match parse_response(body).unwrap_err() {
            McpError::Server { code, message, .. } => {
                assert_eq!(code, -32600);
                assert!(message.contains("blorp"));
            }
            other => panic!("expected Server, got {other:?}"),
        }
    }

    #[test]
    fn send_message_args_serialization_drops_none() {
        let args = SendMessageArgs {
            body: "hi".into(),
            ..Default::default()
        };
        let v = serde_json::to_value(args).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.get("body").unwrap(), "hi");
        assert!(!obj.contains_key("channel"));
        assert!(!obj.contains_key("subject"));
    }
}
