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
    async fn resolve_tool_approval(&self, approval_id: &str, approved: bool);
    async fn resolve_question(&self, approval_id: &str, answer: String);
}

/// Façade over the real manager.
pub struct AgentRunner {
    link: Arc<Mutex<SessionLink>>,
    pub working_directory: PathBuf,
    pub manager: Arc<dyn ManagerHandle>,
}

impl AgentRunner {
    pub fn new(working_directory: PathBuf, manager: Arc<dyn ManagerHandle>) -> Self {
        Self {
            link: Arc::new(Mutex::new(SessionLink::default())),
            working_directory,
            manager,
        }
    }

    pub fn link(&self) -> Arc<Mutex<SessionLink>> {
        self.link.clone()
    }

    pub async fn start_for(&self, inbound_id: &str, prompt: &str) -> Result<String, String> {
        let session_id = self
            .manager
            .start_session(prompt, Some(self.working_directory.clone()))
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
pub struct LrManagerHandle {
    manager: Arc<CodingAgentManager>,
    approvals: Arc<AskPopupApprovalService>,
    agent_type: CodingAgentType,
    permission_mode: CodingPermissionMode,
}

impl LrManagerHandle {
    /// Build a runner wired to emit Tauri `approval-pending` and
    /// `agent-status` events. Constructed from `lib.rs::setup()`
    /// where the `AppHandle` (and therefore the emitter) is
    /// available.
    pub fn build(app: AppHandle, state: SharedState, settings: &Settings) -> Arc<Self> {
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
            agent_type: map_agent_type(settings.agent_type),
            permission_mode: map_permission_mode(settings.permission_mode),
        });

        // Spawn status watcher so the UI + tray reflect Running →
        // Done/Error transitions the manager publishes.
        spawn_status_watcher(manager, app, state);

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
                self.agent_type,
                CLIENT_ID,
                prompt,
                working_directory,
                None, // model override — let lr-coding-agents default
                Some(self.permission_mode),
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
                Some(self.permission_mode),
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
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
/// for the UI + tray.
fn spawn_status_watcher(
    manager: Arc<CodingAgentManager>,
    app: AppHandle,
    state: SharedState,
) {
    tokio::spawn(async move {
        let mut rx = manager.subscribe_changes();
        loop {
            if rx.recv().await.is_err() {
                break;
            }
            let sid_opt = match state.read().agent.clone() {
                AgentStatus::Running { session_id } if !session_id.is_empty() => {
                    Some(session_id)
                }
                _ => None,
            };
            let Some(sid) = sid_opt else { continue };
            let Ok(resp) = manager.status(&sid, CLIENT_ID, Some(20)).await else {
                continue;
            };
            // Don't override a pending approval/question state with
            // a stale "Active" tick — the PopupTrigger callback is
            // the source of truth for WaitingFor* states.
            let cur = state.read().agent.clone();
            let new = match resp.status {
                SessionStatus::Active => match cur {
                    AgentStatus::WaitingForApproval { .. }
                    | AgentStatus::WaitingForAnswer { .. } => cur,
                    _ => AgentStatus::Running { session_id: sid },
                },
                SessionStatus::Done => AgentStatus::Stopped,
                SessionStatus::Error => AgentStatus::Error {
                    message: "session error".into(),
                },
                SessionStatus::Interrupted => AgentStatus::Stopped,
            };
            apply_agent_transition(&app, &state, new);
        }
    });
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
