//! Tauri commands for the VPN page (Xray core behind a subscription).
//!
//! Network and process work is blocking, so every command that can touch the
//! disk or a process runs on the blocking pool.

use crate::xray::{self, XrayProfile, XrayStatus};
use crate::AppResult;

#[tauri::command]
pub fn get_xray_status(app: tauri::AppHandle) -> AppResult<XrayStatus> {
    Ok(xray::status(&app))
}

#[tauri::command]
pub async fn get_xray_profiles(
    app: tauri::AppHandle,
    with_ping: Option<bool>,
) -> AppResult<Vec<XrayProfile>> {
    Ok(tauri::async_runtime::spawn_blocking(move || {
        xray::profiles(&app, with_ping.unwrap_or(false))
    })
    .await
    .map_err(|e| e.to_string())??)
}

/// Re-downloads the subscription and returns the fresh list.
#[tauri::command]
pub async fn refresh_xray_subscription(
    app: tauri::AppHandle,
    with_ping: Option<bool>,
) -> AppResult<Vec<XrayProfile>> {
    Ok(tauri::async_runtime::spawn_blocking(move || {
        xray::fetch_subscription(&app)?;
        xray::profiles(&app, with_ping.unwrap_or(false))
    })
    .await
    .map_err(|e| e.to_string())??)
}

#[tauri::command]
pub async fn set_xray_subscription(app: tauri::AppHandle, url: String) -> AppResult<String> {
    let url = url.trim().to_string();
    if !url.is_empty() && !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("Ссылка подписки должна начинаться с https://".into());
    }
    let saved = url.clone();
    tauri::async_runtime::spawn_blocking(move || {
        xray::update_settings(&app, move |cfg| cfg.subscription_url = url)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(saved)
}

#[tauri::command]
pub async fn select_xray_profile(app: tauri::AppHandle, index: usize) -> AppResult<()> {
    tauri::async_runtime::spawn_blocking(move || xray::select_profile(&app, index))
        .await
        .map_err(|e| e.to_string())??;
    Ok(())
}

#[tauri::command]
pub async fn set_xray_ports(
    app: tauri::AppHandle,
    socks_port: u16,
    http_port: u16,
) -> AppResult<(u16, u16)> {
    let socks = xray::validate_port(socks_port)?;
    let http = xray::validate_port(http_port)?;
    if socks == http {
        return Err("Порты SOCKS и HTTP должны различаться.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        xray::update_settings(&app, move |cfg| {
            cfg.socks_port = socks;
            cfg.http_port = http;
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok((socks, http))
}

#[tauri::command]
pub async fn set_xray_autostart(app: tauri::AppHandle, enabled: bool) -> AppResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        xray::update_settings(&app, move |cfg| cfg.autostart = enabled)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(())
}

#[tauri::command]
pub async fn install_xray_core(app: tauri::AppHandle) -> AppResult<String> {
    Ok(
        tauri::async_runtime::spawn_blocking(move || xray::install_core(&app))
            .await
            .map_err(|e| e.to_string())??,
    )
}

/// Unpacks a core archive the user downloaded by hand.
#[tauri::command]
pub async fn install_xray_core_from_file(app: tauri::AppHandle, path: String) -> AppResult<String> {
    Ok(tauri::async_runtime::spawn_blocking(move || {
        xray::install_core_from_archive(&app, std::path::Path::new(&path))
    })
    .await
    .map_err(|e| e.to_string())??)
}

#[tauri::command]
pub async fn start_xray(app: tauri::AppHandle) -> AppResult<()> {
    tauri::async_runtime::spawn_blocking(move || xray::start(&app))
        .await
        .map_err(|e| e.to_string())??;
    Ok(())
}

#[tauri::command]
pub async fn stop_xray(app: tauri::AppHandle) -> AppResult<()> {
    tauri::async_runtime::spawn_blocking(move || xray::stop(&app))
        .await
        .map_err(|e| e.to_string())??;
    Ok(())
}

#[tauri::command]
pub async fn restart_xray(app: tauri::AppHandle) -> AppResult<()> {
    tauri::async_runtime::spawn_blocking(move || xray::restart(&app))
        .await
        .map_err(|e| e.to_string())??;
    Ok(())
}

#[tauri::command]
pub fn get_xray_logs(app: tauri::AppHandle) -> AppResult<Vec<String>> {
    Ok(xray::logs(&app))
}

/// Opens Telegram's own "add SOCKS5 proxy" dialog for the local port.
#[tauri::command]
pub fn open_xray_in_telegram(app: tauri::AppHandle) -> AppResult<()> {
    Ok(xray::open_in_telegram(&app)?)
}

/// The `tg://socks` link is shown in the UI so the user can copy it.
#[tauri::command]
pub fn get_xray_telegram_link(app: tauri::AppHandle) -> AppResult<String> {
    let status = xray::status(&app);
    Ok(xray::telegram_link(status.socks_port))
}

/// Replaces the whitelist of sites that must go through the tunnel.
#[tauri::command]
pub async fn set_xray_proxy_sites(
    app: tauri::AppHandle,
    sites: Vec<String>,
) -> AppResult<Vec<String>> {
    Ok(
        tauri::async_runtime::spawn_blocking(move || xray::set_proxy_sites(&app, sites))
            .await
            .map_err(|e| e.to_string())??,
    )
}

/// Ready-made service bundles: one `geosite:` category each.
#[tauri::command]
pub fn get_xray_site_bundles() -> AppResult<Vec<xray::SiteBundle>> {
    Ok(xray::site_bundles())
}

/// Downloads both community domain lists and merges them into one file.
#[tauri::command]
pub async fn update_xray_site_bases(app: tauri::AppHandle) -> AppResult<xray::BasesSummary> {
    Ok(
        tauri::async_runtime::spawn_blocking(move || xray::update_bases(&app))
            .await
            .map_err(|e| e.to_string())??,
    )
}

/// Suggests the extra domains a site needs (CDN, static hosts).
#[tauri::command]
pub async fn discover_xray_sites(
    app: tauri::AppHandle,
    site: String,
) -> AppResult<Vec<xray::SiteCandidate>> {
    Ok(
        tauri::async_runtime::spawn_blocking(move || xray::discover_sites(&app, &site))
            .await
            .map_err(|e| e.to_string())??,
    )
}

/// Merges a domain list from a text file into the whitelist.
#[tauri::command]
pub async fn import_xray_sites(app: tauri::AppHandle, path: String) -> AppResult<Vec<String>> {
    Ok(
        tauri::async_runtime::spawn_blocking(move || xray::import_sites(&app, &path))
            .await
            .map_err(|e| e.to_string())??,
    )
}

/// Writes the whitelist to a text file; returns how many domains were saved.
#[tauri::command]
pub async fn export_xray_sites(app: tauri::AppHandle, path: String) -> AppResult<usize> {
    Ok(
        tauri::async_runtime::spawn_blocking(move || xray::export_sites(&app, &path))
            .await
            .map_err(|e| e.to_string())??,
    )
}
/// Points the Windows system proxy at the local HTTP inbound (or restores it).
#[tauri::command]
pub async fn set_xray_system_proxy(app: tauri::AppHandle, enabled: bool) -> AppResult<()> {
    tauri::async_runtime::spawn_blocking(move || xray::set_system_proxy(&app, enabled))
        .await
        .map_err(|e| e.to_string())??;
    Ok(())
}
