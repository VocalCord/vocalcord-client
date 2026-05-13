//! Vocal Cord desktop — Tauri 2 entry.
//!
//! Architecture (see `plan/.../vocalcord-desktop.md`):
//!
//! - `inbound` merges WS + (optional) webhook into one dedup'd stream.
//! - `approval_router` decides — pure function — what to do with each
//!   inbound body based on current agent state.
//! - `agent_runner` owns the `CodingAgentManager` façade.
//! - `outbound` sends the agent's reply via the MCP `SendMessage`.
//! - `tray` shows the current state in the menubar.
//!
//! The wiring below kicks off the inbound + agent loop on
//! `gateway_start`-equivalent (i.e. once Tauri's `RunEvent::Ready`
//! fires) and tears it down on exit.

pub mod agent_runner;
pub mod approval_router;
pub mod commands;
pub mod inbound;
pub mod outbound;
pub mod runtime;
pub mod settings;
pub mod state;
pub mod supervisor;
pub mod tray;

use std::sync::Arc;

use parking_lot::RwLock;
use tauri::{Listener, Manager};

use crate::agent_runner::{AgentRunner, LrManagerHandle, ManagerHandle};
use crate::settings::Settings;
use crate::state::AppState;
use crate::supervisor::Supervisor;

/// Tauri app entrypoint. Called from `main.rs` for binary builds and
/// from mobile / test harnesses for non-binary builds.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tauri=warn,hyper=warn".into()),
        )
        .init();

    let shared = AppState::new_shared();
    // Settings get reloaded from disk inside setup() once the store
    // plugin is initialised; default value here is just the
    // placeholder during plugin bring-up.
    let settings = Arc::new(RwLock::new(Settings::default()));

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(shared.clone())
        .manage(settings.clone())
        .setup({
            let shared = shared.clone();
            let settings = settings.clone();
            move |app| {
                let handle = app.handle().clone();
                tray::build(&handle)?;
                tray::set_variant(&handle, tray::TrayVariant::Disconnected);
                {
                    let g = shared.read();
                    tray::refresh_labels(&handle, &g.connection, &g.agent);
                }

                // Reload persisted settings now that the store
                // plugin is up.
                {
                    let loaded = settings::load(&handle);
                    *settings.write() = loaded;
                }

                // The window is `visible: false` in tauri.conf.json;
                // we only show it on tray click. This keeps the dock
                // icon out of the way on macOS once the user closes
                // the window (it minimises to tray rather than
                // quitting).

                // Construct the agent runner here — `LrManagerHandle::build`
                // needs the AppHandle so its PopupTrigger can emit
                // Tauri events. AgentRunner and the status watcher
                // share one SessionLink Arc so auto-reply on Done
                // knows which inbound to thread its SendMessage off.
                let s = settings.read().clone();
                let link = Arc::new(parking_lot::Mutex::new(
                    agent_runner::SessionLink::default(),
                ));
                let manager: Arc<dyn ManagerHandle> = LrManagerHandle::build(
                    handle.clone(),
                    shared.clone(),
                    &s,
                    settings.clone(),
                    link.clone(),
                );
                let _ = s; // settings already snapshotted into manager.build above
                let agent = Arc::new(AgentRunner::new_with_link(
                    settings.clone(),
                    manager,
                    link,
                ));
                handle.manage::<Arc<AgentRunner>>(agent.clone());

                // The supervisor owns the spawn lifecycle of inbound +
                // runtime so the tray actions and settings-changed
                // listener can pause / resume / reload cleanly.
                let supervisor = Supervisor::new(
                    handle.clone(),
                    shared.clone(),
                    settings.clone(),
                    agent.clone(),
                );
                handle.manage::<Arc<Supervisor>>(supervisor.clone());

                // Boot inbound + runtime now (no-op if api_key empty).
                {
                    let sup = supervisor.clone();
                    tauri::async_runtime::spawn(async move {
                        sup.start().await;
                    });
                }

                // Tray menu emits `tray-action` with "toggle-agent" or
                // "toggle-inbound". The runtime listens for these and
                // routes through the supervisor / agent runner.
                {
                    let sup = supervisor.clone();
                    let agent = agent.clone();
                    handle.listen("tray-action", move |ev| {
                        let payload = ev.payload();
                        let action = payload.trim_matches('"').to_string();
                        let sup = sup.clone();
                        let agent = agent.clone();
                        tauri::async_runtime::spawn(async move {
                            match action.as_str() {
                                "toggle-inbound" => {
                                    if sup.is_running().await {
                                        sup.pause().await;
                                    } else {
                                        sup.start().await;
                                    }
                                }
                                "toggle-agent" => {
                                    if let Err(e) = agent.stop_current().await {
                                        tracing::warn!(error = %e, "stop_current failed");
                                    }
                                }
                                _ => {}
                            }
                        });
                    });
                }

                // Settings page save → soft restart of inbound.
                // Agent-type / working-directory changes still need
                // an app restart (the manager is constructed once).
                {
                    let sup = supervisor.clone();
                    handle.listen("settings-changed", move |_ev| {
                        let sup = sup.clone();
                        tauri::async_runtime::spawn(async move {
                            sup.reload().await;
                        });
                    });
                }
                Ok(())
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::get_settings,
            commands::update_settings,
            commands::toggle_inbound,
            commands::toggle_agent,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            // Hide window instead of quitting when the user clicks
            // the red close button — the app stays alive in the tray.
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                api.prevent_exit();
            }
        });
}
