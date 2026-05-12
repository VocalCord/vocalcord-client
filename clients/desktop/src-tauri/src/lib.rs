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

pub mod approval_router;
pub mod commands;
pub mod inbound;
pub mod outbound;
pub mod agent_runner;
pub mod settings;
pub mod state;
pub mod tray;

use std::sync::Arc;

use parking_lot::RwLock;

use crate::agent_runner::{AgentRunner, LrManagerHandle};
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
    let agent_manager = Arc::new(LrManagerHandle::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(shared.clone())
        .manage(settings.clone())
        .manage::<Arc<AgentRunner>>(Arc::new(AgentRunner::new(
            settings.read().working_directory.clone(),
            agent_manager.clone(),
        )))
        .setup(move |app| {
            tray::build(&app.handle())?;
            tray::set_variant(&app.handle(), tray::TrayVariant::Disconnected);
            // The window is `visible: false` in tauri.conf.json; we
            // only show it on tray click. This keeps the dock icon
            // out of the way on macOS once the user closes the
            // window (it minimises to tray rather than quitting).
            Ok(())
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
