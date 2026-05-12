//! Runtime loop: consume the merged inbound stream, run it through
//! `approval_router::decide()`, dispatch each `Action` to the agent
//! runner, and emit Tauri events so the UI + tray reflect every
//! transition.

use std::sync::Arc;

use parking_lot::RwLock;
use push_core::receiver::InboundMessage;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

use crate::agent_runner::AgentRunner;
use crate::approval_router::{decide, Action};
use crate::inbound::InboundEvent;
use crate::outbound;
use crate::settings::Settings;
use crate::state::{AgentStatus, ConnectionStatus, InboxItem, SharedState};
use crate::tray::{self, variant_for};

/// Kick off the runtime — spawns a background task that drives the
/// loop until the channel closes (i.e. settings change / app exit).
pub fn spawn(
    app: AppHandle,
    mut rx: mpsc::Receiver<InboundEvent>,
    state: SharedState,
    settings: Arc<RwLock<Settings>>,
    agent: Arc<AgentRunner>,
) {
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                InboundEvent::Message(m) => {
                    handle_inbound(&app, m, &state, &settings, &agent).await;
                }
                InboundEvent::Connected => {
                    transition_connection(&app, &state, ConnectionStatus::Connected);
                }
                InboundEvent::Disconnected(reason) => {
                    {
                        let mut g = state.write();
                        g.log(format!("ws disconnected: {reason}"));
                    }
                    transition_connection(&app, &state, ConnectionStatus::Reconnecting);
                }
            }
        }
        tracing::info!("inbound stream closed; runtime exiting");
    });
}

fn transition_connection(app: &AppHandle, state: &SharedState, next: ConnectionStatus) {
    {
        let mut g = state.write();
        g.connection = next.clone();
        g.log(format!("connection → {:?}", next));
    }
    let (conn, agent) = {
        let g = state.read();
        (g.connection.clone(), g.agent.clone())
    };
    tray::set_variant(app, variant_for(&conn, &agent));
    emit_snapshot(app, state);
}

async fn handle_inbound(
    app: &AppHandle,
    msg: InboundMessage,
    state: &SharedState,
    settings: &Arc<RwLock<Settings>>,
    agent: &Arc<AgentRunner>,
) {
    // 1. Record in inbox + emit snapshot.
    {
        let mut g = state.write();
        g.push_inbox(InboxItem {
            id: msg.id.clone(),
            channel: msg.channel.clone(),
            from: msg.from.clone(),
            from_display: msg.from_display.clone(),
            preview: preview_of(&msg.body),
            ts: msg.ts.clone(),
        });
        g.log(format!(
            "inbound {} from {}",
            msg.channel,
            msg.from_display.as_deref().unwrap_or(&msg.from)
        ));
    }
    emit_snapshot(app, state);

    // 2. Decide.
    let agent_status_now = state.read().agent.clone();
    let action = decide(&agent_status_now, &msg.body);
    {
        let mut g = state.write();
        g.log(format!("decision: {action:?}"));
    }

    // 3. Dispatch.
    match action {
        Action::StartSession => {
            let prompt = msg.body.clone();
            transition_agent(app, state, AgentStatus::Running { session_id: String::new() });
            match agent.start_for(&msg.id, &prompt).await {
                Ok(sid) => {
                    transition_agent(
                        app,
                        state,
                        AgentStatus::Running { session_id: sid },
                    );
                }
                Err(e) => {
                    transition_agent(app, state, AgentStatus::Error { message: e.clone() });
                    auto_reply_error(state, settings, &msg.id, &e).await;
                }
            }
        }
        Action::SayInterrupt => {
            if let Err(e) = agent.say_to_current(&msg.body, true).await {
                state.write().log(format!("say failed: {e}"));
            }
        }
        Action::ResumeSession => {
            if let Err(e) = agent.say_to_current(&msg.body, false).await {
                state.write().log(format!("resume failed: {e}"));
            }
        }
        Action::ApproveTool { approval_id } => {
            agent.approve(&approval_id, true).await;
            transition_agent(app, state, AgentStatus::Running { session_id: String::new() });
        }
        Action::DenyTool { approval_id } => {
            agent.approve(&approval_id, false).await;
            transition_agent(app, state, AgentStatus::Running { session_id: String::new() });
        }
        Action::DenyToolThenSay { approval_id } => {
            agent.approve(&approval_id, false).await;
            if let Err(e) = agent.say_to_current(&msg.body, true).await {
                state.write().log(format!("follow-up say failed: {e}"));
            }
            transition_agent(app, state, AgentStatus::Running { session_id: String::new() });
        }
        Action::AnswerQuestion { approval_id } => {
            agent.answer(&approval_id, msg.body.clone()).await;
            transition_agent(app, state, AgentStatus::Running { session_id: String::new() });
        }
    }
}

/// Update agent status, refresh tray, emit snapshot.
fn transition_agent(app: &AppHandle, state: &SharedState, next: AgentStatus) {
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
    emit_snapshot(app, state);
}

fn describe(s: &AgentStatus) -> String {
    match s {
        AgentStatus::Stopped => "Idle".into(),
        AgentStatus::Running { session_id } => {
            if session_id.is_empty() {
                "Running".into()
            } else {
                format!("Running (session {})", &session_id[..session_id.len().min(8)])
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

fn preview_of(body: &str) -> String {
    let first = body.lines().next().unwrap_or("").trim();
    if first.chars().count() <= 96 {
        first.to_owned()
    } else {
        let mut s: String = first.chars().take(95).collect();
        s.push('…');
        s
    }
}

fn emit_snapshot(app: &AppHandle, state: &SharedState) {
    let snap = state.read().snapshot();
    let _ = app.emit("snapshot", snap);
}

async fn auto_reply_error(
    state: &SharedState,
    settings: &Arc<RwLock<Settings>>,
    inbound_id: &str,
    err: &str,
) {
    let s = settings.read().clone();
    if !s.is_configured() {
        return;
    }
    let body = format!("Agent run failed: {err}");
    if let Err(e) = outbound::send_reply(&s, inbound_id, body).await {
        state.write().log(format!("error-reply failed: {e}"));
    }
}
