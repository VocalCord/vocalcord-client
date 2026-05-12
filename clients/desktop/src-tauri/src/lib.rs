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
pub mod tray;

use std::sync::Arc;

use parking_lot::RwLock;
use tauri::Manager;

use crate::agent_runner::{AgentRunner, LrManagerHandle, ManagerHandle};
use crate::settings::Settings;
use crate::state::AppState;

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

                // The window is `visible: false` in tauri.conf.json;
                // we only show it on tray click. This keeps the dock
                // icon out of the way on macOS once the user closes
                // the window (it minimises to tray rather than
                // quitting).

                // Construct the agent runner here — `LrManagerHandle::build`
                // needs the AppHandle so its PopupTrigger can emit
                // Tauri events.
                let s = settings.read().clone();
                let manager: Arc<dyn ManagerHandle> =
                    LrManagerHandle::build(handle.clone(), shared.clone(), &s);
                let agent = Arc::new(AgentRunner::new(
                    s.working_directory.clone(),
                    manager,
                ));
                handle.manage::<Arc<AgentRunner>>(agent.clone());

                // Kick off the inbound runtime if the user has an
                // apiKey configured. Otherwise the Status page shows
                // "Paused" until they save Settings.
                if s.is_configured() {
                    tauri::async_runtime::spawn(async move {
                        let (rx, _shutdowns) = inbound::start(&s).await;
                        runtime::spawn(handle, rx, shared, settings, agent);
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
