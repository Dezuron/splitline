use super::*;
use crate::AppResult;

#[tauri::command]
pub fn set_game_filter(state: State<'_, AppState>, mode: String) -> AppResult<()> {
    let gf = match mode.as_str() {
        "all" => GameFilter::All,
        "tcp" => GameFilter::Tcp,
        "udp" => GameFilter::Udp,
        _ => GameFilter::Disabled,
    };
    {
        let mut cfg = lock_config(&state);
        cfg.game_filter = gf;
        config::save_config(&cfg).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn set_autostart(
    state: State<'_, AppState>,
    enabled: bool,
) -> AppResult<autostart::AutostartStatus> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;

    // Task Scheduler work spawns `schtasks.exe`, so it runs on the blocking
    // pool instead of the command thread.
    let status = tauri::async_runtime::spawn_blocking(move || {
        if enabled {
            autostart::install(&exe)
        } else {
            autostart::remove().map(|()| autostart::AutostartStatus::disabled())
        }
    })
    .await
    .map_err(|e| e.to_string())??;

    // Persist the config only after the system change succeeded, otherwise the
    // UI would show autostart enabled while nothing is registered.
    let mut cfg = lock_config(&state);
    cfg.autostart = enabled;
    config::save_config(&cfg)?;
    Ok(status)
}

#[tauri::command]
pub fn set_check_updates(state: State<'_, AppState>, enabled: bool) -> AppResult<()> {
    let mut cfg = lock_config(&state);
    cfg.check_updates = enabled;
    Ok(config::save_config(&cfg).map_err(|e| e.to_string())?)
}

#[tauri::command]
pub fn set_runtime_path(state: State<'_, AppState>, path: String) -> AppResult<String> {
    ensure_idle(&state)?;
    if current_running(&state) {
        return Err("Сначала остановите обход, затем измените папку runtime.".into());
    }

    let trimmed = path.trim();
    let resolved = if trimmed.is_empty() {
        config::app_dir().join(config::RUNTIME_DIR_NAME)
    } else {
        std::path::PathBuf::from(trimmed)
    };
    config::prepare_runtime_dir(&resolved)?;

    {
        let mut cfg = lock_config(&state);
        cfg.runtime_path = if trimmed.is_empty() {
            String::new()
        } else {
            resolved.to_string_lossy().to_string()
        };
        config::save_config(&cfg).map_err(|e| e.to_string())?;
    }
    Ok(resolved.to_string_lossy().to_string())
}

#[tauri::command]
pub fn set_theme(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    theme: String,
) -> AppResult<()> {
    let theme = match theme.as_str() {
        "carmine" | "light" | "stencil" => "carmine",
        _ => "satellite",
    };
    {
        let mut cfg = lock_config(&state);
        cfg.theme = theme.to_string();
        config::save_config(&cfg)?;
    }
    if let Some(window) = tauri::Manager::get_webview_window(&app, "main") {
        crate::apply_titlebar_theme(&window, config::theme_is_dark(theme));
    }
    Ok(())
}

#[tauri::command]
pub fn set_flashscore_bypass(state: State<'_, AppState>, enabled: bool) -> AppResult<()> {
    ensure_idle(&state)?;
    {
        let mut cfg = lock_config(&state);
        cfg.flashscore_bypass = enabled;
        config::save_config(&cfg).map_err(|e| e.to_string())?;
    }
    // Apply immediately so the toggle takes effect without a manual restart.
    if current_running(&state) {
        start_inner(&state, None)?;
    }
    Ok(())
}

// ---------- backup: export and import of settings ----------

/// The save dialog returns whatever the user typed; the archive is always a zip.
fn archive_destination(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Не выбран файл для сохранения.".into());
    }
    let mut dest = PathBuf::from(trimmed);
    let already_zip = dest
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("zip"))
        .unwrap_or(false);
    if !already_zip {
        dest.set_extension("zip");
    }
    Ok(dest)
}

/// Writes one zip with the user's settings and returns its final path, so the UI
/// can show where the file landed (the dialog may have added `.zip`).
#[tauri::command]
pub async fn export_settings(
    path: String,
    include_secrets: bool,
) -> AppResult<crate::backup::ExportReport> {
    let dest = archive_destination(&path)?;
    let paths = crate::backup::BackupPaths::current();
    Ok(tauri::async_runtime::spawn_blocking(move || {
        crate::backup::write_backup(&paths, &dest, include_secrets)
    })
    .await
    .map_err(|e| e.to_string())??)
}

/// Reads and verifies an archive without writing anything: the confirmation
/// dialog shows what the user is about to import.
#[tauri::command]
pub async fn inspect_backup(path: String) -> AppResult<crate::backup::BackupPreview> {
    let archive = PathBuf::from(path);
    Ok(
        tauri::async_runtime::spawn_blocking(move || crate::backup::inspect(&archive))
            .await
            .map_err(|e| e.to_string())??,
    )
}

/// Replaces the current settings with a verified archive.
///
/// Order matters: the archive is verified first, then the state is copied to
/// `backups/`, only then are the services stopped and the files written. Without
/// that order a bad archive could leave the machine without a working bypass and
/// without a copy of what used to work.
#[tauri::command]
pub async fn import_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> AppResult<crate::backup::ImportReport> {
    ensure_idle(&state)?;

    let archive = PathBuf::from(path);
    let verified = tauri::async_runtime::spawn_blocking(move || crate::backup::verify(&archive))
        .await
        .map_err(|e| e.to_string())??;

    let merged = {
        let current = lock_config(&state);
        verified
            .config()
            .cloned()
            .map(|imported| crate::backup::merge_imported(&current, imported))
    };

    // Lists belong to the runtime the imported settings point at, which may be a
    // custom folder; the safety copy still describes the current state.
    let paths = crate::backup::BackupPaths::current();
    let safety_paths = paths.clone();
    let safety = tauri::async_runtime::spawn_blocking(move || {
        crate::backup::create_safety_backup(&safety_paths)
    })
    .await
    .map_err(|e| e.to_string())??;
    let mut apply_paths = paths;
    if let Some(imported) = &merged {
        apply_paths.runtime_dir =
            config::resolve_runtime_dir(&apply_paths.app_dir, &imported.runtime_path);
    }

    let zapret_was_running = current_running(&state);
    let tgws_was_running = matches!(
        crate::tgws::status(&app).state,
        crate::tgws::TgwsState::Running | crate::tgws::TgwsState::Starting
    );
    let xray_was_running = matches!(
        crate::xray::status(&app).state,
        crate::xray::XrayState::Running | crate::xray::XrayState::Starting
    );

    // The service and the proxy may hold the files open, and the Windows proxy
    // points at the old ports: stop what is running before the first write. The
    // flags say what has to come back if a later step fails.
    let mut restart_zapret = false;
    let mut restart_tgws = false;
    let mut restart_xray = false;
    if zapret_was_running {
        engine::Engine::remove()?;
        restart_zapret = true;
    }
    if tgws_was_running {
        if let Err(error) = stop_tgws(app.clone()).await {
            if restart_zapret {
                let _ = start_inner(&state, None);
            }
            return Err(error);
        }
        restart_tgws = true;
    }
    if xray_was_running {
        if let Err(error) = stop_xray(app.clone()).await {
            if restart_zapret {
                let _ = start_inner(&state, None);
            }
            if restart_tgws {
                let _ = start_tgws(app.clone()).await;
            }
            return Err(error);
        }
        restart_xray = true;
    }

    let apply_config = merged.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::backup::apply_verified(&apply_paths, &verified, apply_config.as_ref(), &safety)
    })
    .await
    .map_err(|e| e.to_string())?;

    let mut report = match result {
        Ok(report) => report,
        Err(error) => {
            // The previous settings are still on disk untouched (the config is
            // written last), but the services must come back regardless.
            restart_services(&app, &state, restart_zapret, restart_tgws, restart_xray).await;
            return Err(error.into());
        }
    };

    // Apply the imported settings without an app restart: the in-memory config is
    // what every command reads, and the title bar follows the imported theme.
    if let Some(imported) = merged {
        *lock_config(&state) = imported.clone();
        if let Some(window) = tauri::Manager::get_webview_window(&app, "main") {
            crate::apply_titlebar_theme(&window, config::theme_is_dark(&imported.theme));
        }
    }

    report
        .warnings
        .extend(restart_services(&app, &state, restart_zapret, restart_tgws, restart_xray).await);
    Ok(report)
}

/// Starts again what was running before the import and reports what did not come
/// back. A restart failure must not fail the import itself: the files are
/// already in place and the user can start the service by hand.
async fn restart_services(
    app: &tauri::AppHandle,
    state: &AppState,
    zapret: bool,
    tgws: bool,
    xray: bool,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if zapret {
        if let Err(error) = start_inner(state, None) {
            warnings.push(format!("Обход не запустился: {error}"));
        }
    }
    if tgws {
        if let Err(error) = start_tgws(app.clone()).await {
            warnings.push(format!("Telegram-прокси не запустился: {error}"));
        }
    }
    if xray {
        if let Err(error) = start_xray(app.clone()).await {
            warnings.push(format!("VPN не запустился: {error}"));
        }
    }
    warnings
}

/// Settings, autostart and the settings backup.
pub fn settings_commands() -> CommandSet {
    command_set![
        set_game_filter,
        set_autostart,
        set_check_updates,
        set_runtime_path,
        set_theme,
        set_flashscore_bypass,
        export_settings,
        inspect_backup,
        import_settings,
    ]
}
