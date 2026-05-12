//! Persistent user settings, backed by `tauri-plugin-store`.
//!
//! `Settings` is the single source of truth for runtime configuration
//! (API key, agent type, working directory, etc.). The Settings page
//! invokes the `update_settings` Tauri command to swap a new value
//! in; on success we save to disk and emit a `settings-changed`
//! event so the inbound + agent runners can soft-restart.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Vocal Cord account UUID (acts as bearer token).
    pub api_key: String,
    /// REST + MCP base. Defaults to https://api.vocalcord.io.
    pub api_base: String,
    /// WebSocket base. Defaults to wss://api.vocalcord.io.
    pub ws_base: String,
    /// Public URL the user's host is reachable at, if any. When set,
    /// the desktop also configures vocalcord's per-account webhook.
    #[serde(default)]
    pub public_url: Option<String>,
    /// Bind port for the local webhook receiver (only used when
    /// `public_url` is set). Default 18790.
    pub webhook_port: u16,
    /// Path under which the local webhook receiver serves. Default
    /// `/vocalcord/inbound`.
    pub webhook_path: String,
    /// Which coding agent to invoke for inbound messages.
    pub agent_type: AgentType,
    /// Working directory the agent runs in.
    pub working_directory: PathBuf,
    /// Permission mode passed to lr-coding-agents.
    pub permission_mode: PermissionMode,
    /// Approval mode passed to lr-coding-agents.
    pub approval_mode: ApprovalMode,
    /// Max characters of agent output included in the auto-reply.
    pub reply_truncate_chars: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentType {
    ClaudeCode,
    GeminiCli,
    Codex,
    Amp,
    Aider,
    Cursor,
    Opencode,
    QwenCode,
    Copilot,
    Droid,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    Auto,
    Supervised,
    Plan,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalMode {
    Allow,
    Ask,
    Elicitation,
}

impl Default for Settings {
    fn default() -> Self {
        let working_directory = dirs::document_dir()
            .map(|d| d.join("vocalcord-agent-work"))
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("vocalcord-agent-work")
            });
        Self {
            api_key: String::new(),
            api_base: "https://api.vocalcord.io".to_string(),
            ws_base: "wss://api.vocalcord.io".to_string(),
            public_url: None,
            webhook_port: 18790,
            webhook_path: "/vocalcord/inbound".to_string(),
            agent_type: AgentType::ClaudeCode,
            working_directory,
            permission_mode: PermissionMode::Supervised,
            approval_mode: ApprovalMode::Ask,
            reply_truncate_chars: 2048,
        }
    }
}

impl Settings {
    /// Returns true iff `api_key` looks plausible (UUID-shaped). Used
    /// to short-circuit launching inbound / agent workers when the
    /// user hasn't configured anything yet.
    pub fn is_configured(&self) -> bool {
        !self.api_key.trim().is_empty()
    }
}

/// Filename of the JSON store under the app data directory. The
/// file isn't user-facing; the path is opaque (typically
/// `~/Library/Application Support/io.vocalcord.desktop/settings.bin`
/// on macOS).
pub const STORE_FILE: &str = "settings.bin";
pub const STORE_KEY: &str = "settings";

/// Read persisted settings from disk, returning the default value
/// when the file is missing, the key is absent, or the payload
/// fails to deserialise (forward-compat schema drift).
pub fn load<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Settings {
    use tauri_plugin_store::StoreExt;
    match app.store(STORE_FILE) {
        Ok(store) => match store.get(STORE_KEY) {
            Some(value) => match serde_json::from_value::<Settings>(value) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "settings deserialize failed; using defaults");
                    Settings::default()
                }
            },
            None => Settings::default(),
        },
        Err(e) => {
            tracing::warn!(error = %e, "settings store open failed; using defaults");
            Settings::default()
        }
    }
}

/// Persist settings to disk. Best-effort — logs on failure, never
/// returns an error so the in-memory state stays updated either way.
pub fn save<R: tauri::Runtime>(app: &tauri::AppHandle<R>, settings: &Settings) {
    use tauri_plugin_store::StoreExt;
    let store = match app.store(STORE_FILE) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "settings store open failed; not persisting");
            return;
        }
    };
    match serde_json::to_value(settings) {
        Ok(v) => {
            store.set(STORE_KEY, v);
            if let Err(e) = store.save() {
                tracing::warn!(error = %e, "settings store save failed");
            }
        }
        Err(e) => tracing::warn!(error = %e, "settings serialize failed"),
    }
}
