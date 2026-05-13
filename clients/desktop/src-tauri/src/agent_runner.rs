//! Coding-agent runner — owns one `CodingAgentManager` and the
//! `AskPopupApprovalService` it issues per-session.
//!
//! The PopupTrigger we install emits Tauri events whenever the agent
//! pauses for tool approval or a clarifying question. The status
//! watcher we spawn from `start_status_watcher` subscribes to the
//! manager's broadcast channel and emits transition events for
//! Running/Done/Error.

use std::path::PathBuf;
use std::sync::Arc;

use executors::approvals::ExecutorApprovalService;
use lr_coding_agents::approval::{AskPopupApprovalService, PopupTrigger};
use lr_coding_agents::manager::CodingAgentManager;
use lr_coding_agents::types::SessionStatus;
use lr_config::{
    CodingAgentApprovalMode, CodingAgentType, CodingAgentsConfig, CodingPermissionMode,
};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter};
use workspace_utils::approvals::{ApprovalStatus, QuestionAnswer, QuestionStatus};

use crate::outbound;
use crate::settings::{AgentType, ApprovalMode, PermissionMode, Settings};
use crate::state::{AgentStatus, SharedState};
use crate::tray::{self, variant_for};

/// Stable client id we pass into every manager call. The manager
/// uses it to scope list/discovery queries; we only ever run one
/// session so the value is constant.
const CLIENT_ID: &str = "vocalcord-desktop";

/// Tracks the inbound message ID that initiated the current session,
/// so replies thread back correctly.
#[derive(Default, Debug)]
pub struct SessionLink {
    pub current_session_id: Option<String>,
    pub originating_inbound_id: Option<String>,
}

/// Object-safe wrapper trait around the bits of
/// `lr_coding_agents::CodingAgentManager` the runtime actually
/// touches. The blanket trait lets the unit tests in
/// `runtime_test.rs` (future) inject a stub without dragging the
/// whole lr-coding-agents transitive graph into the test binary.
#[async_trait::async_trait]
pub trait ManagerHandle: Send + Sync {
    async fn start_session(
        &self,
        prompt: &str,
        working_directory: Option<PathBuf>,
    ) -> Result<String, String>;
    async fn say(
        &self,
        session_id: &str,
        message: &str,
        interrupt: bool,
    ) -> Result<(), String>;
    async fn end_session(&self, session_id: &str) -> Result<(), String>;
    async fn resolve_tool_approval(&self, approval_id: &str, approved: bool);
    async fn resolve_question(&self, approval_id: &str, answer: String);
}

/// Façade over the real manager.
pub struct AgentRunner {
    link: Arc<Mutex<SessionLink>>,
    settings: Arc<parking_lot::RwLock<Settings>>,
    pub manager: Arc<dyn ManagerHandle>,
}

impl AgentRunner {
    pub fn new(
        settings: Arc<parking_lot::RwLock<Settings>>,
        manager: Arc<dyn ManagerHandle>,
    ) -> Self {
        Self::new_with_link(
            settings,
            manager,
            Arc::new(Mutex::new(SessionLink::default())),
        )
    }

    /// Same as [`Self::new`] but reuses a caller-provided
    /// `SessionLink`. Used in `lib.rs::setup` so the status watcher
    /// and the `AgentRunner` share the same link Arc.
    pub fn new_with_link(
        settings: Arc<parking_lot::RwLock<Settings>>,
        manager: Arc<dyn ManagerHandle>,
        link: Arc<Mutex<SessionLink>>,
    ) -> Self {
        Self {
            link,
            settings,
            manager,
        }
    }

    pub fn link(&self) -> Arc<Mutex<SessionLink>> {
        self.link.clone()
    }

    /// Resolves the current working directory from settings each
    /// call so changes via the Settings page take effect on the next
    /// inbound without an app restart.
    pub fn working_directory(&self) -> PathBuf {
        self.settings.read().working_directory.clone()
    }

    pub async fn start_for(&self, inbound_id: &str, prompt: &str) -> Result<String, String> {
        let session_id = self
            .manager
            .start_session(prompt, Some(self.working_directory()))
            .await?;
        let mut g = self.link.lock();
        g.current_session_id = Some(session_id.clone());
        g.originating_inbound_id = Some(inbound_id.to_string());
        Ok(session_id)
    }

    pub async fn say_to_current(&self, message: &str, interrupt: bool) -> Result<(), String> {
        let sid = self
            .link
            .lock()
            .current_session_id
            .clone()
            .ok_or_else(|| "no active session".to_string())?;
        self.manager.say(&sid, message, interrupt).await
    }

    /// Stop the current session, if any. Hard-kills the agent
    /// process via the manager's `end_session`. Idempotent — returns
    /// Ok(()) if there is no active session.
    pub async fn stop_current(&self) -> Result<(), String> {
        let sid = self.link.lock().current_session_id.clone();
        let Some(sid) = sid else {
            return Ok(());
        };
        let res = self.manager.end_session(&sid).await;
        // Clear the link regardless — even on error there is no
        // recovery from the desktop's side; the manager either
        // killed the process or it's already gone.
        {
            let mut g = self.link.lock();
            g.current_session_id = None;
            g.originating_inbound_id = None;
        }
        res
    }

    pub async fn approve(&self, approval_id: &str, approved: bool) {
        self.manager.resolve_tool_approval(approval_id, approved).await;
    }

    pub async fn answer(&self, approval_id: &str, answer: String) {
        self.manager.resolve_question(approval_id, answer).await;
    }

    pub fn originating_inbound_id(&self) -> Option<String> {
        self.link.lock().originating_inbound_id.clone()
    }
}

/// Real `ManagerHandle` implementation backed by
/// `lr_coding_agents::CodingAgentManager`.
///
/// `agent_type` and `permission_mode` are resolved from
/// `settings_arc` on every call so the Settings page can swap them
/// without rebuilding the manager. `approval_mode` is baked into the
/// manager's `CodingAgentsConfig` at construction — changing that
/// still requires an app restart (a follow-up could swap to
/// `update_config` but the manager only exposes that with `&mut
/// self`, which doesn't compose with `Arc`).
pub struct LrManagerHandle {
    manager: Arc<CodingAgentManager>,
    approvals: Arc<AskPopupApprovalService>,
    settings_arc: Arc<parking_lot::RwLock<Settings>>,
}

impl LrManagerHandle {
    fn agent_type(&self) -> CodingAgentType {
        map_agent_type(self.settings_arc.read().agent_type)
    }
    fn permission_mode(&self) -> CodingPermissionMode {
        map_permission_mode(self.settings_arc.read().permission_mode)
    }
}

impl LrManagerHandle {
    /// Build a runner wired to emit Tauri `approval-pending` and
    /// `agent-status` events. Constructed from `lib.rs::setup()`
    /// where the `AppHandle` (and therefore the emitter) is
    /// available.
    ///
    /// `settings_arc` is the shared `Arc<RwLock<Settings>>` so the
    /// status watcher can read up-to-date apiBase / apiKey when it
    /// sends the auto-reply on session Done. `link` lets the watcher
    /// look up which inbound message originated the session so the
    /// reply threads correctly.
    pub fn build(
        app: AppHandle,
        state: SharedState,
        settings: &Settings,
        settings_arc: Arc<parking_lot::RwLock<Settings>>,
        link: Arc<Mutex<SessionLink>>,
    ) -> Arc<Self> {
        let trigger: Arc<dyn PopupTrigger> = Arc::new(TauriPopupTrigger {
            app: app.clone(),
            state: state.clone(),
        });
        let approvals = Arc::new(AskPopupApprovalService::new().with_popup_trigger(trigger));

        let cfg = CodingAgentsConfig {
            // We only ever run one session at a time, but keep
            // the manager's cap generous so it doesn't reject
            // mid-resume traffic during a hand-off window.
            max_concurrent_sessions: 4,
            approval_mode: map_approval_mode(settings.approval_mode),
            ..CodingAgentsConfig::default()
        };

        let factory_svc = approvals.clone();
        let factory: lr_coding_agents::manager::ApprovalServiceFactory =
            Arc::new(move |_sid| {
                factory_svc.clone() as Arc<dyn ExecutorApprovalService>
            });

        let manager = Arc::new(
            CodingAgentManager::new(cfg).with_approval_service_factory(factory),
        );

        let handle = Arc::new(Self {
            manager: manager.clone(),
            approvals,
            settings_arc: settings_arc.clone(),
        });

        // Spawn status watcher so the UI + tray reflect Running →
        // Done/Error transitions the manager publishes. The watcher
        // also drives the auto-reply on Done.
        spawn_status_watcher(manager, app, state, settings_arc, link);

        handle
    }
}

#[async_trait::async_trait]
impl ManagerHandle for LrManagerHandle {
    async fn start_session(
        &self,
        prompt: &str,
        working_directory: Option<PathBuf>,
    ) -> Result<String, String> {
        let resp = self
            .manager
            .start_session(
                self.agent_type(),
                CLIENT_ID,
                prompt,
                working_directory,
                None, // model override — let lr-coding-agents default
                Some(self.permission_mode()),
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(resp.session_id)
    }

    async fn say(
        &self,
        session_id: &str,
        message: &str,
        interrupt: bool,
    ) -> Result<(), String> {
        self.manager
            .say(
                session_id,
                CLIENT_ID,
                Some(message),
                interrupt,
                Some(self.permission_mode()),
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn end_session(&self, session_id: &str) -> Result<(), String> {
        self.manager
            .end_session(session_id)
            .await
            .map_err(|e| e.to_string())
    }

    async fn resolve_tool_approval(&self, approval_id: &str, approved: bool) {
        let status = if approved {
            ApprovalStatus::Approved
        } else {
            ApprovalStatus::Denied { reason: None }
        };
        self.approvals.resolve_tool_approval(approval_id, status);
    }

    async fn resolve_question(&self, approval_id: &str, answer: String) {
        // We don't know which specific question text the executor
        // asked at this layer (the PopupTrigger carries only the
        // approval_id + tool_name + question_count). We pass the
        // body verbatim under an empty `question` so the executor's
        // resolver sees a single answer per outstanding question
        // slot. If the executor surfaces multi-question flows in the
        // future, expand this to split on newlines.
        self.approvals.resolve_question(
            approval_id,
            QuestionStatus::Answered {
                answers: vec![QuestionAnswer {
                    question: String::new(),
                    answer: vec![answer],
                }],
            },
        );
    }
}

/// Fires on every approval-related callback from the executor.
/// Updates app state + emits Tauri events so the UI and tray
/// reflect "Waiting on you" in real time.
struct TauriPopupTrigger {
    app: AppHandle,
    state: SharedState,
}

impl PopupTrigger for TauriPopupTrigger {
    fn trigger_tool_approval(&self, approval_id: &str, tool_name: &str) {
        let next = AgentStatus::WaitingForApproval {
            approval_id: approval_id.to_string(),
            tool_name: tool_name.to_string(),
        };
        apply_agent_transition(&self.app, &self.state, next);
        let _ = self.app.emit(
            "approval-pending",
            serde_json::json!({
                "kind": "tool",
                "approval_id": approval_id,
                "tool_name": tool_name,
            }),
        );
    }

    fn trigger_question_approval(
        &self,
        approval_id: &str,
        tool_name: &str,
        question_count: usize,
    ) {
        let next = AgentStatus::WaitingForAnswer {
            approval_id: approval_id.to_string(),
            tool_name: tool_name.to_string(),
        };
        apply_agent_transition(&self.app, &self.state, next);
        let _ = self.app.emit(
            "approval-pending",
            serde_json::json!({
                "kind": "question",
                "approval_id": approval_id,
                "tool_name": tool_name,
                "question_count": question_count,
            }),
        );
    }
}

fn apply_agent_transition(app: &AppHandle, state: &SharedState, next: AgentStatus) {
    {
        let mut g = state.write();
        g.agent = next.clone();
        g.now_doing = describe(&next);
        g.log(format!("agent → {next:?}"));
    }
    let (conn, agent) = {
        let g = state.read();
        (g.connection.clone(), g.agent.clone())
    };
    tray::set_variant(app, variant_for(&conn, &agent));
    tray::refresh_labels(app, &conn, &agent);
    let snap = state.read().snapshot();
    let _ = app.emit("snapshot", snap);
}

fn describe(s: &AgentStatus) -> String {
    match s {
        AgentStatus::Stopped => "Idle".into(),
        AgentStatus::Running { session_id } => {
            if session_id.is_empty() {
                "Running".into()
            } else {
                let cut = session_id.len().min(8);
                format!("Running (session {})", &session_id[..cut])
            }
        }
        AgentStatus::WaitingForApproval { tool_name, .. } => {
            format!("Waiting for approval: {tool_name}")
        }
        AgentStatus::WaitingForAnswer { tool_name, .. } => {
            format!("Waiting for answer: {tool_name}")
        }
        AgentStatus::Error { message } => format!("Error: {message}"),
    }
}

/// Watch the manager's broadcast channel; on every change tick,
/// query the current session (if we have one) and emit a transition
/// for the UI + tray. When a session transitions to Done/Error,
/// auto-reply the trailing output to the originating channel via
/// `outbound::send_reply`.
fn spawn_status_watcher(
    manager: Arc<CodingAgentManager>,
    app: AppHandle,
    state: SharedState,
    settings_arc: Arc<parking_lot::RwLock<Settings>>,
    link: Arc<Mutex<SessionLink>>,
) {
    tokio::spawn(async move {
        let mut rx = manager.subscribe_changes();
        loop {
            if rx.recv().await.is_err() {
                break;
            }
            // Determine the session id we're tracking — current
            // session in state if Running/Waiting, else whatever the
            // link recorded (so we still catch the terminal tick
            // that fires *after* state has already been transitioned
            // by an earlier event).
            let cur = state.read().agent.clone();
            let sid_opt = match &cur {
                AgentStatus::Running { session_id } if !session_id.is_empty() => {
                    Some(session_id.clone())
                }
                AgentStatus::WaitingForApproval { .. }
                | AgentStatus::WaitingForAnswer { .. } => link.lock().current_session_id.clone(),
                _ => None,
            };
            let Some(sid) = sid_opt else { continue };
            let Ok(resp) = manager.status(&sid, CLIENT_ID, Some(120)).await else {
                continue;
            };
            let new = match resp.status {
                SessionStatus::Active => match cur {
                    AgentStatus::WaitingForApproval { .. }
                    | AgentStatus::WaitingForAnswer { .. } => cur,
                    _ => AgentStatus::Running { session_id: sid.clone() },
                },
                SessionStatus::Done => AgentStatus::Stopped,
                SessionStatus::Error => AgentStatus::Error {
                    message: resp
                        .result
                        .clone()
                        .unwrap_or_else(|| "session error".into()),
                },
                SessionStatus::Interrupted => AgentStatus::Stopped,
            };
            apply_agent_transition(&app, &state, new.clone());

            // Auto-reply on Done / Error.
            if matches!(resp.status, SessionStatus::Done | SessionStatus::Error) {
                let body = build_reply_body(&resp);
                let settings_now = settings_arc.read().clone();
                let inbound_id = link.lock().originating_inbound_id.clone();
                if let (Some(inbound_id), true) =
                    (inbound_id, settings_now.is_configured() && !body.is_empty())
                {
                    let trimmed = outbound::truncate_for_reply(
                        &body,
                        settings_now.reply_truncate_chars,
                    );
                    if let Err(e) =
                        outbound::send_reply(&settings_now, &inbound_id, trimmed).await
                    {
                        tracing::warn!(error = ?e, "auto-reply send_message failed");
                        state
                            .write()
                            .log(format!("auto-reply failed: {e}"));
                    } else {
                        state.write().log("auto-reply sent");
                    }
                }
                // Clear the session link so a follow-up message
                // starts fresh rather than trying to .say() on a
                // dead session id.
                {
                    let mut g = link.lock();
                    g.current_session_id = None;
                    g.originating_inbound_id = None;
                }
            }
        }
    });
}

/// Build the reply body from a StatusResponse. Prefers the
/// agent's explicit `result` (final assistant text), falls back to
/// the last few output lines so the user always gets *something*
/// useful even for agents that don't emit a structured result.
fn build_reply_body(resp: &lr_coding_agents::types::StatusResponse) -> String {
    if let Some(r) = &resp.result {
        if !r.trim().is_empty() {
            return r.clone();
        }
    }
    // Take the trailing chunk of recent_output, dropping empties
    // and anything that looks like an executor log line.
    let lines: Vec<&str> = resp
        .recent_output
        .iter()
        .map(|s| s.as_str())
        .rev()
        .take_while(|s| !s.is_empty())
        .collect();
    let mut owned: Vec<String> = lines.into_iter().rev().map(String::from).collect();
    if owned.is_empty() {
        owned = resp.recent_output.iter().cloned().collect();
    }
    owned.join("\n")
}

// ---------- settings → lr_config mappings ----------

fn map_agent_type(t: AgentType) -> CodingAgentType {
    match t {
        AgentType::ClaudeCode => CodingAgentType::ClaudeCode,
        AgentType::GeminiCli => CodingAgentType::GeminiCli,
        AgentType::Codex => CodingAgentType::Codex,
        AgentType::Amp => CodingAgentType::Amp,
        AgentType::Aider => CodingAgentType::Aider,
        AgentType::Cursor => CodingAgentType::Cursor,
        AgentType::Opencode => CodingAgentType::Opencode,
        AgentType::QwenCode => CodingAgentType::QwenCode,
        AgentType::Copilot => CodingAgentType::Copilot,
        AgentType::Droid => CodingAgentType::Droid,
    }
}

fn map_permission_mode(p: PermissionMode) -> CodingPermissionMode {
    match p {
        PermissionMode::Auto => CodingPermissionMode::Auto,
        PermissionMode::Supervised => CodingPermissionMode::Supervised,
        PermissionMode::Plan => CodingPermissionMode::Plan,
    }
}

fn map_approval_mode(a: ApprovalMode) -> CodingAgentApprovalMode {
    match a {
        ApprovalMode::Allow => CodingAgentApprovalMode::Allow,
        ApprovalMode::Ask => CodingAgentApprovalMode::Ask,
        ApprovalMode::Elicitation => CodingAgentApprovalMode::Elicitation,
    }
}
