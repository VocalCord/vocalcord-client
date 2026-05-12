//! Inbound-runtime supervisor.
//!
//! Owns the spawn lifecycle of `inbound::start` + `runtime::spawn` so
//! the tray menu actions ("Pause inbound" / "Resume inbound") and the
//! `settings-changed` event can tear it down + re-spawn without
//! restarting the whole Tauri app.
//!
//! Single-task design: there's at most one inbound runtime alive at
//! any moment. All mutation goes through the async `Mutex` so we
//! serialise pause/resume/reload calls — racing them would otherwise
//! leak shutdowns.

use std::sync::Arc;

use parking_lot::RwLock;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::agent_runner::AgentRunner;
use crate::inbound::{self, Shutdown};
use crate::runtime;
use crate::settings::Settings;
use crate::state::{ConnectionStatus, SharedState};
use crate::tray::{self, variant_for};

pub struct Supervisor {
    app: AppHandle,
    shared: SharedState,
    settings: Arc<RwLock<Settings>>,
    agent: Arc<AgentRunner>,
    /// `None` when paused. `Some(...)` while inbound + runtime are
    /// active.
    state: Mutex<Option<Vec<Shutdown>>>,
}

impl Supervisor {
    pub fn new(
        app: AppHandle,
        shared: SharedState,
        settings: Arc<RwLock<Settings>>,
        agent: Arc<AgentRunner>,
    ) -> Arc<Self> {
        Arc::new(Self {
            app,
            shared,
            settings,
            agent,
            state: Mutex::new(None),
        })
    }

    /// Whether inbound is currently running.
    pub async fn is_running(&self) -> bool {
        self.state.lock().await.is_some()
    }

    /// Spin up inbound + runtime if not already running.
    pub async fn start(self: &Arc<Self>) {
        let mut g = self.state.lock().await;
        if g.is_some() {
            return;
        }
        let s = self.settings.read().clone();
        if !s.is_configured() {
            self.set_connection(ConnectionStatus::Paused);
            return;
        }
        let (rx, shutdowns) = inbound::start(&s).await;
        *g = Some(shutdowns);
        runtime::spawn(
            self.app.clone(),
            rx,
            self.shared.clone(),
            self.settings.clone(),
            self.agent.clone(),
        );
        self.shared.write().log("inbound runtime started");
    }

    /// Tear down inbound + runtime. Idempotent.
    pub async fn pause(self: &Arc<Self>) {
        let mut g = self.state.lock().await;
        if let Some(shutdowns) = g.take() {
            for s in shutdowns {
                s.shutdown();
            }
        }
        self.set_connection(ConnectionStatus::Paused);
        self.shared.write().log("inbound paused");
    }

    /// Pause + start (reads current settings on resume).
    pub async fn reload(self: &Arc<Self>) {
        self.pause().await;
        // Small grace so the previous runtime task observes its
        // channel closing and exits before we re-spawn.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        self.start().await;
    }

    fn set_connection(&self, next: ConnectionStatus) {
        let agent_now = {
            let mut g = self.shared.write();
            g.connection = next.clone();
            g.agent.clone()
        };
        tray::set_variant(&self.app, variant_for(&next, &agent_now));
        let snap = self.shared.read().snapshot();
        let _ = self.app.emit("snapshot", snap);
    }
}
