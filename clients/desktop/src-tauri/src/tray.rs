//! System-tray icon and menu. Icon swaps on every status transition.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::state::{AgentStatus, ConnectionStatus};

/// Dynamic menu items the rest of the app updates when state
/// transitions. Stored in Tauri's `manage` so [`refresh_labels`] can
/// pick them up without re-building the entire tray.
pub struct DynamicMenuItems {
    pub toggle_agent: MenuItem<Wry>,
    pub toggle_inbound: MenuItem<Wry>,
}

/// Tray icon variants. The PNG files live in `src-tauri/icons/`.
#[derive(Debug, Clone, Copy)]
pub enum TrayVariant {
    Idle,
    Working,
    Attention,
    Disconnected,
    Error,
}

impl TrayVariant {
    pub fn filename(self) -> &'static str {
        match self {
            TrayVariant::Idle => "icons/tray-idle.png",
            TrayVariant::Working => "icons/tray-working.png",
            TrayVariant::Attention => "icons/tray-attention.png",
            TrayVariant::Disconnected => "icons/tray-disconnected.png",
            TrayVariant::Error => "icons/tray-error.png",
        }
    }
}

/// Map combined status to a single tray icon. Order of priority:
/// disconnected > attention > error > working > idle.
pub fn variant_for(conn: &ConnectionStatus, agent: &AgentStatus) -> TrayVariant {
    match conn {
        ConnectionStatus::Reconnecting | ConnectionStatus::Paused => TrayVariant::Disconnected,
        ConnectionStatus::Connected => match agent {
            AgentStatus::WaitingForApproval { .. } | AgentStatus::WaitingForAnswer { .. } => {
                TrayVariant::Attention
            }
            AgentStatus::Error { .. } => TrayVariant::Error,
            AgentStatus::Running { .. } => TrayVariant::Working,
            AgentStatus::Stopped => TrayVariant::Idle,
        },
    }
}

/// Build the initial tray (idle variant) and wire menu / left-click
/// behaviour. Returns the tray handle so callers can swap the icon
/// later via [`set_variant`]. The dynamic menu items are stashed in
/// `app.manage(DynamicMenuItems)` for later updates by
/// [`refresh_labels`].
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let (menu, dynamic) = build_menu(app)?;
    app.manage(dynamic);
    let _tray = TrayIconBuilder::with_id("main")
        .tooltip("Vocal Cord")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Update the dynamic menu items' labels + enabled state to reflect
/// the current connection + agent status. No-op if the tray hasn't
/// been built yet.
pub fn refresh_labels(app: &AppHandle, conn: &ConnectionStatus, agent: &AgentStatus) {
    let Some(items) = app.try_state::<DynamicMenuItems>() else {
        return;
    };
    let (agent_label, agent_enabled) = match agent {
        AgentStatus::Stopped => ("Stop agent (idle)", false),
        AgentStatus::Running { .. } => ("Stop agent", true),
        AgentStatus::WaitingForApproval { .. } | AgentStatus::WaitingForAnswer { .. } => {
            ("Stop agent", true)
        }
        AgentStatus::Error { .. } => ("Stop agent (idle)", false),
    };
    if let Err(e) = items.toggle_agent.set_text(agent_label) {
        tracing::warn!(error = %e, "tray set_text toggle-agent");
    }
    if let Err(e) = items.toggle_agent.set_enabled(agent_enabled) {
        tracing::warn!(error = %e, "tray set_enabled toggle-agent");
    }
    let inbound_label = match conn {
        ConnectionStatus::Paused => "Resume inbound",
        ConnectionStatus::Connected | ConnectionStatus::Reconnecting => "Pause inbound",
    };
    if let Err(e) = items.toggle_inbound.set_text(inbound_label) {
        tracing::warn!(error = %e, "tray set_text toggle-inbound");
    }
}

/// Switch the running tray icon to the variant matching current status.
pub fn set_variant(app: &AppHandle, variant: TrayVariant) {
    let Some(tray) = app.tray_by_id("main") else {
        tracing::warn!("tray id 'main' not found; cannot set variant");
        return;
    };
    if let Err(e) = tray.set_tooltip(Some(format!("Vocal Cord — {variant:?}"))) {
        tracing::warn!(error = %e, "tray set_tooltip");
    }
    // Icon swap. The actual PNG bytes live under
    // `src-tauri/icons/`; we use `tauri::image::Image::from_path` to
    // load them at runtime so the tray reflects the current state
    // without re-embedding all variants in the binary.
    let path = match app.path().resource_dir() {
        Ok(p) => p.join(variant.filename()),
        Err(_) => std::path::PathBuf::from(variant.filename()),
    };
    match tauri::image::Image::from_path(&path) {
        Ok(img) => {
            if let Err(e) = tray.set_icon(Some(img)) {
                tracing::warn!(error = %e, "tray set_icon");
            }
        }
        Err(e) => tracing::warn!(error = %e, path = %path.display(), "load tray icon"),
    }
}

fn build_menu(app: &AppHandle) -> tauri::Result<(Menu<Wry>, DynamicMenuItems)> {
    let show = MenuItem::with_id(app, "show", "Show Vocal Cord", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let toggle_agent = MenuItem::with_id(
        app,
        "toggle-agent",
        "Stop agent (idle)",
        false,
        None::<&str>,
    )?;
    let toggle_inbound = MenuItem::with_id(
        app,
        "toggle-inbound",
        "Pause inbound",
        true,
        None::<&str>,
    )?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &settings,
            &sep1,
            &toggle_agent,
            &toggle_inbound,
            &sep2,
            &quit,
        ],
    )?;
    let dynamic = DynamicMenuItems {
        toggle_agent,
        toggle_inbound,
    };
    Ok((menu, dynamic))
}

fn handle_menu_event(app: &AppHandle, ev: tauri::menu::MenuEvent) {
    match ev.id.as_ref() {
        "show" => show_main_window(app),
        "settings" => {
            show_main_window(app);
            let _ = app.emit("nav", "settings");
        }
        "toggle-agent" => {
            let _ = app.emit("tray-action", "toggle-agent");
        }
        "toggle-inbound" => {
            let _ = app.emit("tray-action", "toggle-inbound");
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}
