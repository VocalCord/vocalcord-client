//! Shared runtime state for the desktop app.
//!
//! The app keeps one big `AppState` behind an `Arc<RwLock>`. The Tauri
//! window invokes `#[tauri::command]` handlers that read/write it, and
//! the inbound + agent watchers push transitions out as Tauri events
//! so the UI doesn't poll.

use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// Top-level app status snapshot — emitted on every transition for
/// the Status page to render directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSnapshot {
    pub connection: ConnectionStatus,
    pub agent: AgentStatus,
    pub now_doing: String,
    pub inbox_preview: Vec<InboxItem>,
    pub event_log: Vec<EventLogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum ConnectionStatus {
    /// Both WS and webhook (if configured) connected.
    Connected,
    /// WS reconnecting, webhook down or absent.
    Reconnecting,
    /// User toggled inbound off via the tray.
    Paused,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum AgentStatus {
    Stopped,
    Running { session_id: String },
    WaitingForApproval { approval_id: String, tool_name: String },
    WaitingForAnswer { approval_id: String, tool_name: String },
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxItem {
    pub id: String,
    pub channel: String,
    pub from: String,
    pub from_display: Option<String>,
    pub preview: String,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLogEntry {
    pub ts: String,
    pub message: String,
}

/// Maximum entries kept in the rolling inbox preview / event ticker.
pub const PREVIEW_CAP: usize = 10;
pub const EVENT_LOG_CAP: usize = 50;

pub type SharedState = Arc<RwLock<AppState>>;

#[derive(Debug)]
pub struct AppState {
    pub connection: ConnectionStatus,
    pub agent: AgentStatus,
    pub now_doing: String,
    pub inbox: VecDeque<InboxItem>,
    pub event_log: VecDeque<EventLogEntry>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            connection: ConnectionStatus::Paused,
            agent: AgentStatus::Stopped,
            now_doing: "Idle".to_string(),
            inbox: VecDeque::with_capacity(PREVIEW_CAP),
            event_log: VecDeque::with_capacity(EVENT_LOG_CAP),
        }
    }
}

impl AppState {
    pub fn new_shared() -> SharedState {
        Arc::new(RwLock::new(Self::default()))
    }

    pub fn snapshot(&self) -> AppSnapshot {
        AppSnapshot {
            connection: self.connection.clone(),
            agent: self.agent.clone(),
            now_doing: self.now_doing.clone(),
            inbox_preview: self.inbox.iter().cloned().collect(),
            event_log: self.event_log.iter().cloned().collect(),
        }
    }

    pub fn push_inbox(&mut self, item: InboxItem) {
        if self.inbox.len() == PREVIEW_CAP {
            self.inbox.pop_front();
        }
        self.inbox.push_back(item);
    }

    pub fn log(&mut self, message: impl Into<String>) {
        if self.event_log.len() == EVENT_LOG_CAP {
            self.event_log.pop_front();
        }
        self.event_log.push_back(EventLogEntry {
            ts: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
            message: message.into(),
        });
    }
}
