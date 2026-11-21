//! System tray: tooltip, menu and the three service toggles.
//!
//! The menu is built once; later updates only change texts and enabled flags,
//! so the user never sees it rebuilt under the cursor. The frontend sends the
//! running flags it already receives from `tgws-status` / `xray-status`; zapret
//! and "is the service installed" are read here, because both are facts about
//! this machine and the SCM query is cheap.

use crate::tgws::TgwsState;
use crate::xray::XrayState;
use crate::{commands, config, engine, lock_config, runtime, AppState};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

const TRAY_ID: &str = "main";

/// One menu row: the service is on, and the row may be clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Service {
    running: bool,
    available: bool,
}

/// What the tooltip and the three toggles should say right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct TrayState {
    zapret: Service,
    tgws: Service,
    xray: Service,
}

/// Running flags sent by the frontend for the two supervised services. Zapret
/// is read here from the service manager: it is cheap and the UI polls it less
/// often, so its value in the webview goes stale while the window is hidden.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct TrayRunning {
    pub tgws: bool,
    pub xray: bool,
}

/// Shown as a toast by the frontend; queued until the window is opened again
/// when the window is hidden.
#[derive(Debug, Clone, Serialize)]
struct TrayNotice {
    message: String,
    visible: bool,
}

struct TrayUi {
    tray: TrayIcon<tauri::Wry>,
    zapret: MenuItem<tauri::Wry>,
    tgws: MenuItem<tauri::Wry>,
    xray: MenuItem<tauri::Wry>,
    /// Last state written to the menu; an unchanged state writes nothing.
    applied: TrayState,
}

static TRAY: Mutex<Option<TrayUi>> = Mutex::new(None);

/// Serialises menu actions: only one service change runs at a time.
static ACTION: Mutex<()> = Mutex::new(());

impl TrayState {
    /// Availability is read from the disk, the supervised running flags come
    /// from the caller that already receives them.
    fn from_running(app: &AppHandle, running: TrayRunning) -> Self {
        let (zapret, tgws, xray) = ready(app);
        Self {
            zapret: Service {
                running: zapret_running(),
                available: zapret,
            },
            tgws: Service {
                running: running.tgws,
                available: tgws,
            },
            xray: Service {
                running: running.xray,
                available: xray,
            },
        }
    }

    /// Reads everything locally: used at startup and right after a menu click,
    /// when no frontend update is on its way.
    fn detect(app: &AppHandle) -> Self {
        Self::from_running(
            app,
            TrayRunning {
                tgws: crate::tgws::status(app).state == TgwsState::Running,
                xray: crate::xray::status(app).state == XrayState::Running,
            },
        )
    }
}

/// The same "is it on" the UI shows: the service, or a benchmark's temporary
/// `winws.exe`.
fn zapret_running() -> bool {
    engine::Engine::is_running() || engine::Engine::is_any_winws_running()
}

/// Whether each service can be started right now: the binary is in place and
/// there is something to start (a strategy, a subscription).
fn ready(app: &AppHandle) -> (bool, bool, bool) {
    let state = app.state::<AppState>();
    let cfg = lock_config(&state);
    let zapret =
        runtime::is_runtime_valid(&config::runtime_dir()) && !cfg.last_strategy.trim().is_empty();
    let tgws = config::tgws_binary_path().exists();
    let xray = crate::xray::binary_path().exists() && !cfg.xray.subscription_url.trim().is_empty();
    (zapret, tgws, xray)
}

/// «Splitline — …»: what is on right now, in words a user can read at a glance.
fn tooltip(state: &TrayState) -> String {
    let mut names: Vec<&str> = Vec::new();
    if state.zapret.running {
        names.push("Zapret");
    }
    if state.tgws.running {
        names.push("Telegram");
    }
    if state.xray.running {
        names.push("VPN");
    }
    let tail = match names.len() {
        0 => "всё выключено".to_string(),
        1 if state.xray.running => "VPN включён".to_string(),
        1 => format!("{} работает", names[0]),
        _ if state.xray.running => format!("{} включены", join_names(&names)),
        _ => format!("{} работают", join_names(&names)),
    };
    format!("Splitline — {tail}")
}

/// «Zapret и Telegram», «Zapret, Telegram и VPN».
fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => (*only).to_string(),
        [rest @ .., last] => format!("{} и {last}", rest.join(", ")),
    }
}

fn on_off(label: &str, running: bool) -> String {
    format!("{label}: {}", if running { "вкл" } else { "выкл" })
}

/// Builds the tray once. A missing default icon is not fatal: the menu and the
/// tooltip still work, and a panic here would take the whole app down.
pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let state = TrayState::detect(app);

    let show = MenuItem::with_id(app, "show", "Показать", true, None::<&str>)?;
    let zapret = MenuItem::with_id(
        app,
        "zapret",
        on_off("Обход", state.zapret.running),
        state.zapret.available,
        None::<&str>,
    )?;
    let tgws = MenuItem::with_id(
        app,
        "tgws",
        on_off("Telegram", state.tgws.running),
        state.tgws.available,
        None::<&str>,
    )?;
    let xray = MenuItem::with_id(
        app,
        "xray",
        on_off("VPN", state.xray.running),
        state.xray.available,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &zapret, &tgws, &xray, &quit])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(tooltip(&state))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| menu_clicked(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let tray = builder.build(app)?;

    *crate::lock_unpoisoned(&TRAY) = Some(TrayUi {
        tray,
        zapret,
        tgws,
        xray,
        applied: state,
    });
    Ok(())
}

/// Menu clicks arrive on the main thread; starting a service blocks for
/// seconds, so the work moves to its own thread instead of freezing the UI.
fn menu_clicked(app: &AppHandle, id: &str) {
    let app = app.clone();
    let id = id.to_string();
    std::thread::spawn(move || match id.as_str() {
        "show" => crate::show_main_window(&app),
        "quit" => app.exit(0),
        _ => {
            // One service operation at a time: a second click while the first
            // is still starting would otherwise race it.
            let Ok(_guard) = ACTION.try_lock() else {
                return;
            };
            match id.as_str() {
                "zapret" => toggle_zapret(&app),
                "tgws" => toggle_tgws(&app),
                "xray" => toggle_xray(&app),
                _ => {}
            }
        }
    });
}

fn toggle_zapret(app: &AppHandle) {
    let state = app.state::<AppState>();
    let result = if zapret_running() {
        commands::stop(state)
    } else {
        commands::start(state)
    };
    if let Err(error) = result {
        notice(app, "Обход", error.to_string());
    }
    refresh(app);
}

fn toggle_tgws(app: &AppHandle) {
    let running = matches!(
        crate::tgws::status(app).state,
        TgwsState::Running | TgwsState::Starting
    );
    let result = if running {
        crate::tgws::stop(app)
    } else {
        crate::tgws::start(app)
    };
    if let Err(error) = result {
        notice(app, "Telegram", error);
    }
    refresh(app);
}

fn toggle_xray(app: &AppHandle) {
    let running = matches!(
        crate::xray::status(app).state,
        XrayState::Running | XrayState::Starting
    );
    let result = if running {
        crate::xray::stop(app)
    } else {
        crate::xray::start(app)
    };
    if let Err(error) = result {
        notice(app, "VPN", error);
    }
    refresh(app);
}

/// A failure is never swallowed: the frontend shows it as a toast, immediately
/// when the window is open and on the next show otherwise.
fn notice(app: &AppHandle, service: &str, error: String) {
    let visible = app
        .get_webview_window("main")
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);
    let _ = app.emit(
        "tray-notice",
        TrayNotice {
            message: format!("{service}: {error}"),
            visible,
        },
    );
}

fn refresh(app: &AppHandle) {
    apply(app, TrayState::detect(app));
}

/// Writes a state into the existing menu. Safe to call from any thread: Tauri
/// marshals the setters to the main thread, and an unchanged state does nothing.
fn apply(app: &AppHandle, next: TrayState) {
    let _ = app.run_on_main_thread(move || {
        let mut guard = crate::lock_unpoisoned(&TRAY);
        let Some(ui) = guard.as_mut() else {
            return;
        };
        if ui.applied == next {
            return;
        }
        let _ = ui.zapret.set_text(on_off("Обход", next.zapret.running));
        let _ = ui.zapret.set_enabled(next.zapret.available);
        let _ = ui.tgws.set_text(on_off("Telegram", next.tgws.running));
        let _ = ui.tgws.set_enabled(next.tgws.available);
        let _ = ui.xray.set_text(on_off("VPN", next.xray.running));
        let _ = ui.xray.set_enabled(next.xray.available);
        let _ = ui.tray.set_tooltip(Some(tooltip(&next)));
        ui.applied = next;
    });
}

/// Called by the frontend whenever the running services it displays change.
#[tauri::command]
pub fn sync_tray(app: AppHandle, running: TrayRunning) {
    apply(&app, TrayState::from_running(&app, running));
}

/// The tray has exactly one command; it is listed here like any other module's.
pub fn tray_commands() -> commands::CommandSet {
    commands::command_set![sync_tray]
}

#[cfg(test)]
mod tests {
    use super::{join_names, on_off, tooltip, Service, TrayState};

    fn state(zapret: bool, tgws: bool, xray: bool) -> TrayState {
        TrayState {
            zapret: Service {
                running: zapret,
                available: true,
            },
            tgws: Service {
                running: tgws,
                available: true,
            },
            xray: Service {
                running: xray,
                available: true,
            },
        }
    }

    #[test]
    fn tooltip_names_what_is_running() {
        assert_eq!(
            tooltip(&state(false, false, false)),
            "Splitline — всё выключено"
        );
        assert_eq!(
            tooltip(&state(true, false, false)),
            "Splitline — Zapret работает"
        );
        assert_eq!(
            tooltip(&state(false, true, false)),
            "Splitline — Telegram работает"
        );
        assert_eq!(
            tooltip(&state(false, false, true)),
            "Splitline — VPN включён"
        );
    }

    #[test]
    fn tooltip_lists_every_combination() {
        assert_eq!(
            tooltip(&state(true, true, false)),
            "Splitline — Zapret и Telegram работают"
        );
        assert_eq!(
            tooltip(&state(true, false, true)),
            "Splitline — Zapret и VPN включены"
        );
        assert_eq!(
            tooltip(&state(false, true, true)),
            "Splitline — Telegram и VPN включены"
        );
        assert_eq!(
            tooltip(&state(true, true, true)),
            "Splitline — Zapret, Telegram и VPN включены"
        );
    }

    #[test]
    fn menu_labels_show_the_current_state() {
        assert_eq!(on_off("Обход", true), "Обход: вкл");
        assert_eq!(on_off("Обход", false), "Обход: выкл");
        assert_eq!(on_off("Telegram", true), "Telegram: вкл");
        assert_eq!(on_off("VPN", false), "VPN: выкл");
    }

    #[test]
    fn names_are_joined_with_russian_conjunction() {
        assert_eq!(join_names(&["Zapret"]), "Zapret");
        assert_eq!(join_names(&["Zapret", "VPN"]), "Zapret и VPN");
        assert_eq!(
            join_names(&["Zapret", "Telegram", "VPN"]),
            "Zapret, Telegram и VPN"
        );
    }
}
