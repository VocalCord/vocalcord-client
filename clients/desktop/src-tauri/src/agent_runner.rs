//! Coding-agent runner — owns a single `CodingAgentManager` and
//! exposes a high-level handle the rest of the app uses.
//!
//! The integration with `lr-coding-agents` is deliberately thin: we
//! keep app-specific state (current session id, last inbound id for
//! reply threading) here, and let the manager handle process I/O,
//! resume logic, and approval bookkeeping.
//!
//! **Status:** scaffolded; the `wire_approval_service` call below
//! takes a placeholder PopupTrigger that just logs. Wiring it to
//! actual Tauri events lives in `lib.rs::run()` where the AppHandle
//! is available.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

/// Tracks the inbound message ID that initiated the current session,
/// so replies thread back correctly.
#[derive(Default, Debug)]
pub struct SessionLink {
    pub current_session_id: Option<String>,
    pub originating_inbound_id: Option<String>,
}

/// Façade over `lr_coding_agents::CodingAgentManager` plus our
/// session-link bookkeeping.
pub struct AgentRunner {
    link: Arc<Mutex<SessionLink>>,
    /// Working directory the agent runs in (from settings).
    pub working_directory: PathBuf,
    /// Wrapper around `Arc<lr_coding_agents::manager::CodingAgentManager>`.
    /// Held as a generic Send + Sync handle so we can compile against
    /// future API tweaks without changing the surface here.
    pub manager: Arc<dyn ManagerHandle>,
}

/// Object-safe wrapper trait around the bits of
/// `lr_coding_agents::CodingAgentManager` we actually use. Keeping
/// this small lets us swap in a stub for tests without bringing the
/// real lr-coding-agents into every build.
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

/// Concrete `ManagerHandle` implementation backed by
/// `lr_coding_agents::CodingAgentManager`. Placeholder shape — the
/// constructor is wired up in `lib.rs::run()` once the AppHandle
/// (and therefore the Tauri event emitter for approvals) is
/// available. See plan section "Agent runner" for the contract.
pub struct LrManagerHandle {
    // Held as an `Arc<dyn ...>` to keep this file independent of the
    // lr-coding-agents-specific generic surface.
}

impl LrManagerHandle {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait::async_trait]
impl ManagerHandle for LrManagerHandle {
    async fn start_session(
        &self,
        _prompt: &str,
        _working_directory: Option<PathBuf>,
    ) -> Result<String, String> {
        Err("lr-coding-agents integration not yet wired".into())
    }
    async fn say(
        &self,
        _session_id: &str,
        _message: &str,
        _interrupt: bool,
    ) -> Result<(), String> {
        Err("lr-coding-agents integration not yet wired".into())
    }
    async fn resolve_tool_approval(&self, _approval_id: &str, _approved: bool) {}
    async fn resolve_question(&self, _approval_id: &str, _answer: String) {}
}
