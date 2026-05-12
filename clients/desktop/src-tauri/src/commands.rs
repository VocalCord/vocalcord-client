//! `#[tauri::command]` surface — the IPC the front-end calls.

use tauri::{AppHandle, State};

use crate::settings::Settings;
use crate::state::{AppSnapshot, SharedState};

#[tauri::command]
pub fn get_snapshot(state: State<'_, SharedState>) -> AppSnapshot {
    state.read().snapshot()
}

#[tauri::command]
pub fn get_settings(settings: State<'_, parking_lot::RwLock<Settings>>) -> Settings {
    settings.read().clone()
}

#[tauri::command]
pub fn update_settings(
    app: AppHandle,
    settings: State<'_, parking_lot::RwLock<Settings>>,
    next: Settings,
) -> Result<(), String> {
    {
        let mut g = settings.write();
        *g = next;
    }
    // Front-end persists via tauri-plugin-store; emit so background
    // workers know to soft-restart with the new config.
    let _ = tauri::Emitter::emit(&app, "settings-changed", ());
    Ok(())
}

#[tauri::command]
pub fn toggle_inbound(app: AppHandle) {
    let _ = tauri::Emitter::emit(&app, "tray-action", "toggle-inbound");
}

#[tauri::command]
pub fn toggle_agent(app: AppHandle) {
    let _ = tauri::Emitter::emit(&app, "tray-action", "toggle-agent");
}
