//! Xray-core supervisor for a VPN subscription.
//!
//! The subscription endpoint returns ready-made Xray configs (one per location):
//! SOCKS/HTTP inbounds, a VLESS outbound over Reality, plus the routing rules
//! the provider wants (RU traffic direct, `geoip:telegram` through the tunnel).
//! This module owns the core: download, start/stop, port rewriting, logs, pings.
//!
//! The core is downloaded from the official `XTLS/Xray-core` releases and is
//! never taken from another vendor's installation.

use crate::config;
use crate::geo;
use crate::lock_config;
use crate::sys;
use crate::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

const CORE_REPO: &str = "XTLS/Xray-core";
const CORE_ASSET: &str = "Xray-windows-64.zip";
const BINARY_IMAGE: &str = "xray.exe";
/// Shown for a core unpacked from a user-picked zip: the release tag is unknown.
const MANUAL_CORE_VERSION: &str = "из файла";
/// Xray reads these next to the executable (`current_dir` is the xray folder).
const GEOSITE_FILE: &str = "geosite.dat";
const GEOSITE_INDEX: &str = "geosite-index.json";
/// Community domain lists, merged into one `geosite.dat` on update.
const BASE_SOURCES: &[(&str, &str)] = &[
    (
        "v2fly",
        "https://github.com/v2fly/domain-list-community/releases/latest/download/dlc.dat",
    ),
    (
        "loyalsoldier",
        "https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geosite.dat",
    ),
];
const LOG_LINES: usize = 300;
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
const START_GRACE: Duration = Duration::from_millis(900);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum XrayState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error,
    NotInstalled,
}

#[derive(Debug, Clone, Serialize)]
pub struct XrayStatus {
    pub state: XrayState,
    pub pid: Option<u32>,
    pub last_error: Option<String>,
    pub installed_version: String,
    pub binary_present: bool,
    pub socks_port: u16,
    pub http_port: u16,
    pub selected: usize,
    pub profile_name: String,
    /// Established sessions on the SOCKS port: Telegram has switched to it.
    pub client_connections: u64,
    /// The Windows system proxy currently points at our HTTP inbound.
    pub system_proxy: bool,
    /// When the subscription was last downloaded (unix seconds), 0 when never.
    pub fetched_at: u64,
}

/// One entry of the subscription, as shown in the UI.
#[derive(Debug, Clone, Serialize)]
pub struct XrayProfile {
    pub index: usize,
    pub name: String,
    pub host: String,
    pub port: u16,
    /// `VLESS / TCP / REALITY` style summary.
    pub scheme: String,
    /// TCP round-trip to the server, `None` when it does not answer.
    pub ping_ms: Option<u64>,
}

pub struct Xray {
    child: Mutex<Option<Child>>,
    status: Mutex<XrayStatus>,
    logs: Mutex<VecDeque<String>>,
    op: Mutex<()>,
    /// Last downloaded subscription; kept in memory and on disk so the UI works
    /// before (and without) a fresh fetch.
    profiles: Mutex<Vec<Value>>,
}

impl Default for Xray {
    fn default() -> Self {
        Self::new()
    }
}

impl Xray {
    pub fn new() -> Self {
        Self {
            child: Mutex::new(None),
            status: Mutex::new(XrayStatus {
                state: XrayState::Stopped,
                pid: None,
                last_error: None,
                installed_version: String::new(),
                binary_present: false,
                socks_port: config::XrayConfig::default().socks_port,
                http_port: config::XrayConfig::default().http_port,
                selected: 0,
                profile_name: String::new(),
                client_connections: 0,
                system_proxy: false,
                fetched_at: 0,
            }),
            logs: Mutex::new(VecDeque::new()),
            op: Mutex::new(()),
            profiles: Mutex::new(Vec::new()),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    crate::lock_unpoisoned(mutex)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------- paths ----------

pub fn dir() -> PathBuf {
    config::app_dir().join("xray")
}

pub fn binary_path() -> PathBuf {
    dir().join(BINARY_IMAGE)
}

pub fn geosite_path() -> PathBuf {
    dir().join(GEOSITE_FILE)
}

fn index_path() -> PathBuf {
    dir().join(GEOSITE_INDEX)
}

fn config_path() -> PathBuf {
    dir().join("config.json")
}

fn subscription_path() -> PathBuf {
    dir().join("subscription.json")
}

// ---------- config access ----------

fn load_cfg(app: &AppHandle) -> config::XrayConfig {
    let state = app.state::<AppState>();
    let cfg = lock_config(&state);
    cfg.xray.clone()
}

fn save_cfg(app: &AppHandle, patch: impl FnOnce(&mut config::XrayConfig)) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut cfg = lock_config(&state);
    patch(&mut cfg.xray);
    config::save_config(&cfg)
}

// ---------- status ----------

/// Established TCP sessions on the local SOCKS port.
///
/// Telegram keeps several connections open once it switches to the proxy, so a
/// non-zero count is the honest "it is actually in use" signal — unlike a
/// button having been pressed once.
pub fn client_connections(port: u16) -> u64 {
    let Ok(output) = sys::command("netstat.exe").args(["-ano"]).output() else {
        return 0;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let local = format!("127.0.0.1:{port}");
    text.lines()
        .filter(|line| line.contains("ESTABLISHED"))
        .filter(|line| {
            let columns: Vec<&str> = line.split_whitespace().collect();
            columns.len() >= 4 && columns[1] == local
        })
        .count() as u64
}

pub fn status(app: &AppHandle) -> XrayStatus {
    let cfg = load_cfg(app);
    let state = app.state::<AppState>();
    let mut snapshot = lock(&state.xray.status).clone();
    snapshot.binary_present = binary_path().exists();
    snapshot.socks_port = cfg.socks_port;
    snapshot.http_port = cfg.http_port;
    snapshot.selected = cfg.selected;
    snapshot.installed_version = cfg.installed_version.clone();
    snapshot.client_connections = if snapshot.state == XrayState::Running {
        client_connections(cfg.socks_port)
    } else {
        0
    };
    snapshot.system_proxy = system_proxy_enabled();
    let profiles = lock(&state.xray.profiles);
    snapshot.profile_name = profiles
        .get(cfg.selected)
        .and_then(profile_name)
        .unwrap_or_default();
    if !snapshot.binary_present
        && !matches!(
            snapshot.state,
            XrayState::Running | XrayState::Starting | XrayState::Stopping
        )
    {
        snapshot.state = XrayState::NotInstalled;
    }
    snapshot
}

fn set_status(app: &AppHandle, patch: impl FnOnce(&mut XrayStatus)) -> XrayStatus {
    let state = app.state::<AppState>();
    let snapshot = {
        let mut status = lock(&state.xray.status);
        patch(&mut status);
        status.clone()
    };
    let _ = app.emit("xray-status", &snapshot);
    snapshot
}

fn mark_failed(app: &AppHandle, message: impl Into<String>) -> String {
    let message = message.into();
    set_status(app, |s| {
        s.state = if binary_path().exists() {
            XrayState::Error
        } else {
            XrayState::NotInstalled
        };
        s.pid = None;
        s.last_error = Some(message.clone());
    });
    message
}

// ---------- machine id ----------

/// The identifier the subscription provider binds a device to.
///
/// Windows keeps it in `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid`, and
/// that is exactly what a stock client sends, so reusing it keeps this machine's
/// existing device slot instead of consuming a new one.
pub fn machine_hwid() -> Option<String> {
    let output = sys::command("reg.exe")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Cryptography",
            "/v",
            "MachineGuid",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .find(|part| part.len() == 36 && part.matches('-').count() == 4)
        .map(|part| part.to_string())
}

// ---------- subscription ----------

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent("Happ/2.9.1")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())
}

/// Downloads the subscription. The provider answers with a JSON array of full
/// Xray configs; a plain text answer means the endpoint rejected the device id,
/// and that message is shown to the user as is.
pub fn fetch_subscription(app: &AppHandle) -> Result<Vec<Value>, String> {
    let cfg = load_cfg(app);
    let url = cfg.subscription_url.trim();
    if url.is_empty() {
        return Err("Укажите ссылку на подписку.".into());
    }
    let hwid = machine_hwid().ok_or("Не удалось прочитать MachineGuid устройства.")?;

    let client = http_client()?;
    let response = client
        .get(url)
        .header("x-hwid", &hwid)
        .send()
        .map_err(|e| format!("Не удалось скачать подписку: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Подписка ответила кодом {}.", response.status()));
    }
    let text = response
        .text()
        .map_err(|e| format!("Не удалось прочитать подписку: {e}"))?;

    let value: Value = serde_json::from_str(text.trim()).map_err(|_| {
        // Not JSON: the provider answered with a human-readable stub such as
        // "device limit exceeded", which is more useful than a parse error.
        let hint = decode_text_stub(&text);
        format!("Подписка не отдала конфиги. Ответ сервера: {hint}")
    })?;

    let list = match value {
        Value::Array(items) => items,
        other => vec![other],
    };
    let usable: Vec<Value> = list
        .into_iter()
        .filter(|item| item.get("outbounds").is_some())
        .collect();
    if usable.is_empty() {
        return Err("В подписке нет ни одного конфига.".into());
    }

    {
        let state = app.state::<AppState>();
        *lock(&state.xray.profiles) = usable.clone();
        set_status(app, |s| s.fetched_at = now_secs());
    }
    persist_subscription(&usable);
    Ok(usable)
}

/// A text answer may be base64-wrapped; decode it before showing.
fn decode_text_stub(text: &str) -> String {
    let trimmed = text.trim();
    let printable = trimmed
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>();
    if printable.chars().count() > 400 {
        // Byte slicing can land inside a Cyrillic character and panic.
        let head: String = printable.chars().take(400).collect();
        return format!("{head}…");
    }
    printable
}

fn persist_subscription(profiles: &[Value]) {
    let path = subscription_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(profiles) {
        let _ = fs::write(path, text);
    }
}

fn hydrate_subscription(app: &AppHandle) {
    let state = app.state::<AppState>();
    if !lock(&state.xray.profiles).is_empty() {
        return;
    }
    let Ok(text) = fs::read_to_string(subscription_path()) else {
        return;
    };
    if let Ok(profiles) = serde_json::from_str::<Vec<Value>>(&text) {
        *lock(&state.xray.profiles) = profiles;
        if let Ok(meta) = fs::metadata(subscription_path()).and_then(|m| m.modified()) {
            let at = meta
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            set_status(app, |s| s.fetched_at = at);
        }
    }
}

pub fn ensure_profiles(app: &AppHandle) -> Result<Vec<Value>, String> {
    hydrate_subscription(app);
    let state = app.state::<AppState>();
    let cached = lock(&state.xray.profiles).clone();
    if !cached.is_empty() {
        return Ok(cached);
    }
    fetch_subscription(app)
}

fn profile_name(cfg: &Value) -> Option<String> {
    cfg.get("remarks")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Primary outbound of a config: the one tagged `proxy`, otherwise the first
/// `vless` one.
fn primary_outbound(cfg: &Value) -> Option<&Value> {
    let outbounds = cfg.get("outbounds")?.as_array()?;
    outbounds
        .iter()
        .find(|o| o.get("tag").and_then(Value::as_str) == Some("proxy"))
        .or_else(|| {
            outbounds
                .iter()
                .find(|o| o.get("protocol").and_then(Value::as_str) != Some("freedom"))
        })
}

fn outbound_endpoint(cfg: &Value) -> Option<(String, u16)> {
    let outbound = primary_outbound(cfg)?;
    let vnext = outbound
        .get("settings")
        .and_then(|s| s.get("vnext"))
        .and_then(Value::as_array)
        .and_then(|v| v.first());
    if let Some(v) = vnext {
        let host = v.get("address").and_then(Value::as_str)?.to_string();
        let port = v.get("port").and_then(Value::as_u64).unwrap_or(443) as u16;
        return Some((host, port));
    }
    let server = outbound
        .get("settings")
        .and_then(|s| s.get("servers"))
        .and_then(Value::as_array)
        .and_then(|v| v.first())?;
    let host = server.get("address").and_then(Value::as_str)?.to_string();
    let port = server.get("port").and_then(Value::as_u64).unwrap_or(443) as u16;
    Some((host, port))
}

fn outbound_scheme(cfg: &Value) -> String {
    let Some(outbound) = primary_outbound(cfg) else {
        return String::new();
    };
    let protocol = outbound
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_uppercase();
    let stream = outbound.get("streamSettings");
    let network = stream
        .and_then(|s| s.get("network"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_uppercase();
    let security = stream
        .and_then(|s| s.get("security"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_uppercase();
    [protocol, network, security]
        .iter()
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" / ")
}

fn tcp_ping(host: &str, port: u16, timeout: Duration) -> Option<u64> {
    use std::net::ToSocketAddrs;
    let address = format!("{host}:{port}");
    let socket_addr = address.to_socket_addrs().ok()?.next()?;
    let started = Instant::now();
    match std::net::TcpStream::connect_timeout(&socket_addr, timeout) {
        Ok(stream) => {
            let _ = stream.set_nodelay(true);
            Some(started.elapsed().as_millis() as u64)
        }
        Err(_) => None,
    }
}

pub fn profiles(app: &AppHandle, with_ping: bool) -> Result<Vec<XrayProfile>, String> {
    let list = ensure_profiles(app)?;
    let mut out = Vec::with_capacity(list.len());
    for (index, cfg) in list.iter().enumerate() {
        let (host, port) = outbound_endpoint(cfg).unwrap_or_default();
        let ping_ms = if with_ping && !host.is_empty() {
            tcp_ping(&host, port, Duration::from_millis(2500))
        } else {
            None
        };
        out.push(XrayProfile {
            index,
            name: profile_name(cfg).unwrap_or_else(|| format!("Профиль {}", index + 1)),
            host,
            port,
            scheme: outbound_scheme(cfg),
            ping_ms,
        });
    }
    Ok(out)
}

// ---------- core binary ----------

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    /// `sha256:<hex>` published by GitHub for every uploaded asset.
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

fn fetch_core_release() -> Result<GhRelease, String> {
    let client = http_client()?;
    let response = client
        .get(format!(
            "https://api.github.com/repos/{CORE_REPO}/releases/latest"
        ))
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("Не удалось проверить релиз Xray: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("GitHub ответил кодом {}.", response.status()));
    }
    response
        .json::<GhRelease>()
        .map_err(|e| format!("Некорректный ответ GitHub: {e}"))
}

/// The core archive is ~17 MB, so it gets its own client: the shared client caps
/// the whole request at 60 s, which cut slow downloads mid-body and surfaced as
/// the unhelpful "error decoding response body". Ten minutes covers a link that
/// trickles at 30 KB/s.
fn download_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent("Splitline")
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| describe_error(&e))
}

/// Walks the error chain: reqwest's own text is often only "error decoding
/// response body", while the reason (timeout, reset, TLS) sits one level down.
fn describe_error(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(" — ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

/// Streams the archive straight to disk, retrying: release downloads do break
/// on flaky links, and a second attempt usually lands on a better edge node.
fn download_file(url: &str, target: &Path) -> Result<u64, String> {
    let client = download_client()?;
    let mut last_error = String::from("неизвестная ошибка");
    for attempt in 1..=3 {
        let result = (|| -> Result<u64, String> {
            let mut response = client
                .get(url)
                .send()
                .and_then(reqwest::blocking::Response::error_for_status)
                .map_err(|e| describe_error(&e))?;
            let mut file = fs::File::create(target).map_err(|e| describe_error(&e))?;
            // `Response: Read` converts body failures into `io::Error`, and the
            // chain keeps the original reason.
            std::io::copy(&mut response, &mut file).map_err(|e| describe_error(&e))
        })();
        match result {
            Ok(size) => return Ok(size),
            Err(error) => {
                last_error = error;
                if attempt < 3 {
                    std::thread::sleep(Duration::from_secs(2 * attempt));
                }
            }
        }
    }
    Err(last_error)
}

/// Unpacks a core archive over the installed binary.
///
/// `remove_archive` is true for the downloaded copy (deleted once unpacked) and
/// false for a file the user picked, which must stay where it is.
fn unpack_core(archive: &Path, remove_archive: bool) -> Result<(), String> {
    let staging = dir().join("staging");
    let _ = fs::remove_dir_all(&staging);
    let extracted_ok = crate::runtime::extract_zip(archive, &staging);
    if remove_archive {
        let _ = fs::remove_file(archive);
    }
    extracted_ok?;

    let extracted = staging.join(BINARY_IMAGE);
    if !extracted.exists() {
        let _ = fs::remove_dir_all(&staging);
        return Err("В архиве не оказалось xray.exe.".into());
    }
    let target = binary_path();
    let _ = fs::remove_file(&target);
    fs::rename(&extracted, &target).map_err(|e| format!("Не удалось установить ядро: {e}"))?;
    let _ = fs::remove_dir_all(&staging);
    Ok(())
}

/// Marks the core as installed and keeps the reported version in one place.
fn remember_core_version(app: &AppHandle, version: &str) -> Result<(), String> {
    save_cfg(app, |cfg| cfg.installed_version = version.to_string())?;
    set_status(app, |s| {
        s.binary_present = true;
        s.installed_version = version.to_string();
        if s.state == XrayState::NotInstalled {
            s.state = XrayState::Stopped;
        }
    });
    Ok(())
}

pub fn install_core(app: &AppHandle) -> Result<String, String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.xray.op);
    install_core_locked(app)
}

/// Installs the core without taking the operation lock.
///
/// `start_locked` already holds it, and `std::sync::Mutex` is not reentrant:
/// locking twice on the first run (when the binary is missing) deadlocked the
/// command until the app was killed.
fn install_core_locked(app: &AppHandle) -> Result<String, String> {
    let release = fetch_core_release()?;
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == CORE_ASSET)
        .ok_or_else(|| format!("В релизе {} нет файла {CORE_ASSET}.", release.tag_name))?;

    fs::create_dir_all(dir()).map_err(|e| format!("Не удалось создать папку ядра: {e}"))?;
    let archive = dir().join("xray.zip");
    let size = download_file(&asset.browser_download_url, &archive)
        .map_err(|e| format!("Не удалось скачать Xray: {e}"))?;
    if size < 1024 * 1024 {
        return Err("Скачанный архив Xray подозрительно мал.".into());
    }
    // The core runs elevated, so a tampered archive must never be unpacked.
    if let Some(digest) = asset.digest.as_deref() {
        if let Err(error) = crate::runtime::verify_digest(&archive, digest) {
            let _ = fs::remove_file(&archive);
            return Err(format!("Проверка архива Xray не прошла: {error}"));
        }
    }
    unpack_core(&archive, true)?;

    let version = release.tag_name.trim_start_matches('v').to_string();
    remember_core_version(app, &version)?;
    Ok(version)
}

/// Escape hatch for networks where GitHub is unreachable: the user downloads
/// `Xray-windows-64.zip` by hand and points the app at it.
pub fn install_core_from_archive(app: &AppHandle, archive: &Path) -> Result<String, String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.xray.op);
    if !archive.is_file() {
        return Err("Файл не найден.".into());
    }
    fs::create_dir_all(dir()).map_err(|e| format!("Не удалось создать папку ядра: {e}"))?;
    unpack_core(archive, false)?;
    remember_core_version(app, MANUAL_CORE_VERSION)?;
    Ok(MANUAL_CORE_VERSION.into())
}

// ---------- domain lists (geosite) ----------

#[derive(Debug, Clone, Serialize)]
pub struct BasesSummary {
    pub categories: usize,
    pub domains: usize,
    pub updated_at: u64,
}

/// Downloads both community lists and merges them into one `geosite.dat`.
///
/// Running core keeps the file it loaded, so the change lands on the next start.
pub fn update_bases(app: &AppHandle) -> Result<BasesSummary, String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.xray.op);
    fs::create_dir_all(dir()).map_err(|e| format!("Не удалось создать папку ядра: {e}"))?;

    let mut sources = Vec::new();
    for (name, url) in BASE_SOURCES {
        let target = dir().join(format!("{name}.dat"));
        let size = download_file(url, &target)
            .map_err(|e| format!("Не удалось скачать список {name}: {e}"))?;
        if size < 100 * 1024 {
            return Err(format!("Список {name} скачался подозрительно малым."));
        }
        sources.push(geo::read(&target)?);
        let _ = fs::remove_file(&target);
    }

    let merged = geo::merge(sources);
    if merged.is_empty() {
        return Err("Списки доменов пусты.".into());
    }
    geo::write(&geosite_path(), &merged)?;
    let index = geo::index(&merged);
    fs::write(
        index_path(),
        serde_json::to_vec(&index).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("Не удалось записать индекс списков: {e}"))?;

    let updated_at = index.updated_at;
    save_cfg(app, move |cfg| cfg.bases_updated_at = updated_at)?;
    let domains: usize = index
        .categories
        .iter()
        .map(|category| category.domains)
        .sum();
    Ok(BasesSummary {
        categories: index.categories.len(),
        domains,
        updated_at,
    })
}

pub fn load_index() -> Option<geo::GeoIndex> {
    let text = fs::read(index_path()).ok()?;
    serde_json::from_slice(&text).ok()
}

/// Validates a `geosite:` category against the downloaded lists.
fn check_category(code: &str) -> Result<(), String> {
    let Some(index) = load_index() else {
        return Err("Базы доменов не загружены. Нажмите «Обновить базы».".into());
    };
    if index.get(code).is_none() {
        return Err(format!(
            "В базах нет категории «{code}». Обновите базы и попробуйте снова."
        ));
    }
    Ok(())
}

fn ensure_core_locked(app: &AppHandle) -> Result<(), String> {
    if binary_path().exists() {
        // A config written before the version was known (reinstall, config reset)
        // leaves `installed_version` empty and the UI says "не установлено"
        // while the core runs. Read it from the binary once.
        if load_cfg(app).installed_version.is_empty() {
            if let Some(version) = probe_core_version(&binary_path()) {
                remember_core_version(app, &version)?;
            }
        }
        return Ok(());
    }
    install_core_locked(app).map(|_| ())
}

/// `xray -version` prints `Xray 26.11.9 (Xray, Penetrates Everything.) ...`.
fn probe_core_version(binary: &Path) -> Option<String> {
    let output = Command::new(binary).arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?.trim();
    let mut parts = line.split_whitespace();
    match parts.next() {
        Some("Xray") => parts.next().map(str::to_string),
        _ => None,
    }
}

pub fn ensure_core(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.xray.op);
    ensure_core_locked(app)
}

// ---------- config generation ----------

/// Writes the selected profile with the ports the user configured.
///
/// Everything else (outbounds, routing rules, DNS) is kept exactly as the
/// provider shipped it: the routing rules are the "which sites go through the
/// tunnel" policy, and rewriting them would silently change behaviour.
/// Telegram ranges: the client talks to these addresses directly, so they stay
/// on the tunnel side even though the whitelist sends everything else direct.
const TELEGRAM_CIDRS: &[&str] = &[
    "91.105.192.0/23",
    "91.108.4.0/22",
    "91.108.8.0/22",
    "91.108.12.0/22",
    "91.108.16.0/22",
    "91.108.20.0/22",
    "91.108.56.0/22",
    "95.161.76.0/23",
    "149.154.160.0/20",
    "185.76.151.0/24",
];

/// Local networks and the router itself never belong on the tunnel.
const DIRECT_CIDRS: &[&str] = &[
    "0.0.0.0/8",
    "10.0.0.0/8",
    "100.64.0.0/10",
    "127.0.0.0/8",
    "169.254.0.0/16",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "224.0.0.0/4",
    "255.255.255.255/32",
];

/// Well-known sights that need more than one domain.
///
/// A page is rarely served from a single host: Instagram pulls CSS, JS and
/// images from `cdninstagram.com` and `fbcdn.net`, TikTok from its CDN hosts,
/// Discord from `discordapp.net`. Whitelisting only the "main" domain leaves the
/// page without its assets, and it looks like the site does not open at all.
pub const SITE_GROUPS: &[(&str, &[&str])] = &[
    (
        "Instagram",
        &["instagram.com", "cdninstagram.com", "fbcdn.net"],
    ),
    (
        "TikTok",
        &[
            "tiktok.com",
            "tiktokcdn.com",
            "tiktokv.com",
            "byteoversea.com",
            "ibytedtos.com",
        ],
    ),
    (
        "Discord",
        &[
            "discord.com",
            "discord.gg",
            "discordapp.com",
            "discordapp.net",
            "discord.media",
        ],
    ),
    ("Facebook", &["facebook.com", "fbcdn.net", "fb.com"]),
    ("X (Twitter)", &["x.com", "twitter.com", "twimg.com"]),
    (
        "YouTube",
        &[
            "youtube.com",
            "youtu.be",
            "ytimg.com",
            "googlevideo.com",
            "ggpht.com",
        ],
    ),
    (
        "Rutracker",
        &["rutracker.org", "rutracker.cc", "rutracker.net"],
    ),
    (
        "ChatGPT",
        &[
            "chatgpt.com",
            "openai.com",
            "oaistatic.com",
            "oaiusercontent.com",
        ],
    ),
    ("BBC", &["bbc.com", "bbc.co.uk", "bbci.co.uk"]),
];

/// Ready-made service bundles: one `geosite:` category each, so a service can be
/// added without listing every CDN domain it happens to use.
const SITE_BUNDLES: &[(&str, &str)] = &[
    ("Instagram", "meta"),
    ("Discord", "discord"),
    ("TikTok", "tiktok"),
    ("YouTube", "youtube"),
    ("Twitter / X", "twitter"),
    ("Facebook", "facebook"),
    ("ChatGPT", "openai"),
    ("Rutracker", "rutracker"),
    ("Telegram", "telegram"),
    ("WhatsApp", "whatsapp"),
    ("Reddit", "reddit"),
    ("Spotify", "spotify"),
    ("LinkedIn", "linkedin"),
];

#[derive(Debug, Clone, Serialize)]
pub struct SiteBundle {
    pub name: String,
    pub category: String,
    pub domains: usize,
    /// False until the domain lists are downloaded and contain the category.
    pub available: bool,
    /// What the proxy cannot carry for this service, if anything.
    pub note: Option<String>,
}

pub fn site_bundles() -> Vec<SiteBundle> {
    let index = load_index();
    SITE_BUNDLES
        .iter()
        .map(|(name, code)| {
            let info = index
                .as_ref()
                .and_then(|index| index.get(code))
                .map(|category| category.domains);
            SiteBundle {
                name: (*name).to_string(),
                category: (*code).to_string(),
                domains: info.unwrap_or(0),
                available: info.is_some(),
                note: bundle_note(code).map(str::to_string),
            }
        })
        .collect()
}

/// Some traffic of a service never reaches the proxy: Discord voice is raw UDP
/// to an address the client got from the API, and the Windows system proxy only
/// carries TCP. Zapret handles exactly that case, so the app points at it.
fn bundle_note(category: &str) -> Option<&'static str> {
    match category {
        "discord" => Some(
            "Голос Discord идёт по UDP мимо прокси: для звонков и голосовых каналов нужен \
             включённый zapret. Переписка при этом работает через VPN.",
        ),
        _ => None,
    }
}

/// Turns a hand-typed site into the form Xray expects.
///
/// Users paste `https://example.com/path`, `example.com:443`, `www.example.com`
/// or a bare domain; only the host survives. `www.` is dropped on purpose:
/// Xray's `domain:` matcher already covers the bare host *and* its subdomains,
/// while `domain:www.example.com` would cover only `www.` — and a browser that
/// opens `example.com` first would then leave the tunnel and get blocked.
/// An already prefixed value (`domain:`, `full:`, `regexp:`) is kept as is so
/// power users can use Xray's own matchers.
pub fn normalize_site(value: &str) -> Option<String> {
    let raw = value.trim();
    if raw.is_empty() {
        return None;
    }
    // Xray's own matchers: `full:`/`regexp:` are kept verbatim, `geosite:` is a
    // category from the downloaded domain lists.
    for prefix in ["full:", "regexp:", "geosite:"] {
        if let Some(rest) = raw.strip_prefix(prefix) {
            let rest = rest.trim();
            if rest.is_empty() {
                return None;
            }
            if prefix == "geosite:" {
                let code = rest.to_ascii_lowercase();
                let valid = code
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '!'));
                return valid.then(|| format!("geosite:{code}"));
            }
            return Some(format!("{prefix}{rest}"));
        }
    }
    // A stored `domain:www.example.com` (saved by an older build) must be
    // cleaned too, so the prefix is dropped and re-applied later.
    let raw = raw.strip_prefix("domain:").unwrap_or(raw);
    let without_scheme = raw
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .trim();
    let host = host.rsplit_once(':').map_or(host, |(left, right)| {
        if right.chars().all(|c| c.is_ascii_digit()) {
            left
        } else {
            host
        }
    });
    let host = host
        .to_ascii_lowercase()
        .trim_start_matches("*.")
        .trim_start_matches("www.")
        .trim_end_matches('.')
        .to_string();
    if host.is_empty() || !host.contains('.') || host.contains(' ') {
        return None;
    }
    Some(format!("domain:{host}"))
}

/// Where the whitelisted traffic has to go.
///
/// Most providers ship a balancer that picks a working outbound (`auto-sni` in
/// the subscription used here), but a plain config has none. Falling back to the
/// `proxy` outbound keeps a different provider from failing to start with
/// "unknown balancer".
fn proxy_target(cfg: &Value) -> (String, String) {
    if let Some(tag) = cfg
        .pointer("/routing/balancers/0/tag")
        .and_then(Value::as_str)
        .filter(|tag| !tag.is_empty())
    {
        return ("balancerTag".into(), tag.to_string());
    }
    let tagged = cfg
        .get("outbounds")
        .and_then(Value::as_array)
        .and_then(|outbounds| {
            outbounds
                .iter()
                .find(|o| o.get("tag").and_then(Value::as_str) == Some("proxy"))
                .or_else(|| {
                    outbounds
                        .iter()
                        .find(|o| o.get("protocol").and_then(Value::as_str) != Some("freedom"))
                })
        })
        .and_then(|o| o.get("tag"))
        .and_then(Value::as_str)
        .unwrap_or("proxy");
    ("outboundTag".into(), tagged.to_string())
}

/// Whitelist routing.
///
/// Replaces the provider's policy (which tunnels a long list of social networks
/// and keeps RU domains direct) with an explicit one: only `proxy_sites` and
/// Telegram leave through the tunnel, everything else is direct.
pub fn build_rules(proxy_sites: &[String], target: (&str, &str), direct: &str) -> Value {
    let list = |items: &[&str]| Value::Array(items.iter().map(|item| Value::from(*item)).collect());
    let with_route = |extra: Value| {
        let mut rule = extra;
        rule[target.0] = Value::from(target.1);
        rule
    };
    let mut rules = vec![
        json!({ "ip": list(DIRECT_CIDRS), "outboundTag": direct }),
        json!({ "ip": ["77.88.8.8"], "outboundTag": direct }),
        with_route(json!({ "ip": list(TELEGRAM_CIDRS) })),
    ];
    let domains: Vec<Value> = proxy_sites
        .iter()
        .filter_map(|site| normalize_site(site))
        .map(Value::from)
        .collect();
    if !domains.is_empty() {
        rules.push(with_route(json!({ "domain": domains })));
    }
    // Nothing else gets out through the tunnel.
    rules.push(json!({ "network": "tcp,udp", "outboundTag": direct }));
    Value::Array(rules)
}

/// Tag of the outbound that sends traffic straight out.
///
/// A hardcoded `direct` breaks on subscriptions that name it differently:
/// Xray only logs a warning and falls back to the first outbound, which is the
/// tunnel — the whitelist would silently turn into "everything through VPN".
fn direct_tag(value: &Value) -> Option<String> {
    let outbounds = value.get("outbounds").and_then(Value::as_array)?;
    if outbounds
        .iter()
        .any(|outbound| outbound.get("tag").and_then(Value::as_str) == Some("direct"))
    {
        return Some("direct".into());
    }
    outbounds
        .iter()
        .find(|outbound| {
            outbound.get("protocol").and_then(Value::as_str) == Some("freedom")
                && outbound.get("tag").and_then(Value::as_str).is_some()
        })
        .and_then(|outbound| outbound.get("tag").and_then(Value::as_str))
        .map(str::to_string)
}

fn write_config(app: &AppHandle, index: usize) -> Result<PathBuf, String> {
    let list = ensure_profiles(app)?;
    let cfg = list
        .get(index)
        .ok_or_else(|| format!("Профиль {index} не найден в подписке."))?;
    let ports = load_cfg(app);
    let mut value = cfg.clone();

    if let Some(inbounds) = value.get_mut("inbounds").and_then(Value::as_array_mut) {
        for inbound in inbounds.iter_mut() {
            let protocol = inbound
                .get("protocol")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let port = match protocol.as_str() {
                "socks" => ports.socks_port,
                "http" => ports.http_port,
                _ => continue,
            };
            inbound["port"] = Value::from(port);
            inbound["listen"] = Value::from("127.0.0.1");
        }
    }

    // A `geosite:` rule without the file stops the core from starting, so the
    // check happens here, where the user still gets a readable reason.
    let uses_geosite = ports
        .proxy_sites
        .iter()
        .filter_map(|site| normalize_site(site))
        .any(|site| site.starts_with("geosite:"));
    if uses_geosite && !geosite_path().exists() {
        return Err(
            "В списке есть наборы, но базы доменов не загружены. Нажмите «Обновить базы».".into(),
        );
    }

    // The routing rules are the "which sites go through the tunnel" policy, so
    // they are replaced by our whitelist instead of the provider's list.
    let (kind, tag) = proxy_target(&value);
    let Some(direct) = direct_tag(&value) else {
        return Err(
            "В подписке нет outbound'а для прямого трафика — белый список работать не будет."
                .into(),
        );
    };
    // A provider without routing (or with a routing script) would ignore our
    // rules entirely; both cases are normalised here.
    if !value.get("routing").map(Value::is_object).unwrap_or(false) {
        value["routing"] = json!({});
    }
    if let Some(routing) = value.get_mut("routing").and_then(Value::as_object_mut) {
        routing.remove("script");
        routing.insert(
            "rules".into(),
            build_rules(&ports.proxy_sites, (&kind, &tag), &direct),
        );
        // `AsIs`: the domain is already known from the HTTP/SOCKS request, and
        // `IPOnDemand` resolved it again for every new host before routing.
        routing.insert("domainStrategy".into(), Value::from("AsIs"));
    }

    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Xray rejects a UTF-8 BOM, so the file is written as plain bytes.
    let text = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("Не удалось записать конфиг ядра: {e}"))?;
    Ok(path)
}

// ---------- logs ----------

fn push_log(app: &AppHandle, line: String) {
    let state = app.state::<AppState>();
    {
        let mut logs = lock(&state.xray.logs);
        if logs.len() >= LOG_LINES {
            logs.pop_front();
        }
        logs.push_back(line.clone());
    }
    append_log_file(&line);
    let _ = app.emit("xray-log", &line);
}

fn append_log_file(line: &str) {
    let path = dir().join("xray.log");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::metadata(&path)
        .map(|m| m.len() > LOG_MAX_BYTES)
        .unwrap_or(false)
    {
        let _ = fs::remove_file(path.with_extension("log.1"));
        let _ = fs::rename(&path, path.with_extension("log.1"));
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{line}");
    }
}

fn spawn_reader(app: AppHandle, stream: impl Read + Send + 'static) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines().map_while(Result::ok) {
            push_log(&app, line);
        }
    });
}

pub fn logs(app: &AppHandle) -> Vec<String> {
    let state = app.state::<AppState>();
    let logs = lock(&state.xray.logs);
    logs.iter().cloned().collect()
}

fn log_tail(app: &AppHandle, count: usize) -> String {
    let state = app.state::<AppState>();
    let logs = lock(&state.xray.logs);
    let start = logs.len().saturating_sub(count);
    logs.iter()
        .skip(start)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------- process ----------

fn port_available(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

pub fn validate_port(port: u16) -> Result<u16, String> {
    if (1024..=65535).contains(&port) {
        Ok(port)
    } else {
        Err("Порт должен быть в диапазоне 1024–65535.".into())
    }
}

/// PID listening on a loopback port, when netstat can tell.
fn port_owner(port: u16) -> Option<u32> {
    let output = sys::command("netstat.exe").args(["-ano"]).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let local = format!("127.0.0.1:{port}");
    text.lines()
        .filter(|line| line.contains("LISTENING") && line.contains(&local))
        .filter_map(|line| line.split_whitespace().last())
        .find_map(|pid| pid.parse().ok())
}

pub fn start(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.xray.op);
    start_locked(app)
}

pub fn stop(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.xray.op);
    stop_locked(app)
}

pub fn restart(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.xray.op);
    stop_locked(app)?;
    std::thread::sleep(Duration::from_millis(300));
    start_locked(app)
}

fn start_locked(app: &AppHandle) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        if lock(&state.xray.child).is_some() {
            return Ok(());
        }
    }
    ensure_core_locked(app)?;

    let cfg = load_cfg(app);
    if !port_available(cfg.socks_port) || !port_available(cfg.http_port) {
        let busy = if !port_available(cfg.socks_port) {
            cfg.socks_port
        } else {
            cfg.http_port
        };
        // Never kill by image name: another client's xray.exe may be running.
        let owner = port_owner(busy)
            .map(|pid| format!(" (PID {pid})"))
            .unwrap_or_default();
        return Err(mark_failed(
            app,
            format!(
                "Порт {busy} занят{owner}. Закройте программу, которая его держит, или выберите другой порт."
            ),
        ));
    }

    set_status(app, |s| {
        s.state = XrayState::Starting;
        s.pid = None;
        s.last_error = None;
    });

    let config_file = write_config(app, cfg.selected)?;
    let binary = binary_path();
    let mut cmd = Command::new(&binary);
    cmd.arg("-c")
        .arg(&config_file)
        .current_dir(dir())
        // Xray looks for geosite.dat next to its executable; pin the resource
        // directory to ours so a moved binary still finds the lists.
        .env("XRAY_LOCATION_ASSET", dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    sys::hidden(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return Err(mark_failed(app, format!("Не удалось запустить Xray: {e}"))),
    };
    let pid = child.id();
    crate::job::adopt(&child);
    if let Some(stdout) = child.stdout.take() {
        spawn_reader(app.clone(), stdout);
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_reader(app.clone(), stderr);
    }
    {
        let state = app.state::<AppState>();
        *lock(&state.xray.child) = Some(child);
    }

    std::thread::sleep(START_GRACE);
    let exited = {
        let state = app.state::<AppState>();
        let mut guard = lock(&state.xray.child);
        match guard.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(Some(code)) => Some(format!("код {code}")),
                _ => None,
            },
            None => Some("процесс не запустился".to_string()),
        }
    };
    if let Some(reason) = exited {
        let state = app.state::<AppState>();
        *lock(&state.xray.child) = None;
        let tail = log_tail(app, 3);
        let message = if tail.is_empty() {
            format!("Xray завершился сразу после запуска ({reason}).")
        } else {
            format!("Xray завершился сразу после запуска ({reason}). {tail}")
        };
        return Err(mark_failed(app, message));
    }

    set_status(app, |s| {
        s.state = XrayState::Running;
        s.pid = Some(pid);
        s.last_error = None;
    });
    // Browsers follow the same whitelist through the HTTP inbound.
    if cfg.system_proxy {
        if let Err(error) = apply_system_proxy(cfg.http_port) {
            push_log(app, format!("Системный прокси не применён: {error}"));
        }
    }
    spawn_watcher(app.clone(), pid);
    Ok(())
}

fn spawn_watcher(app: AppHandle, pid: u32) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(2));
        let exited = {
            let state = app.state::<AppState>();
            let mut guard = lock(&state.xray.child);
            match guard.as_mut() {
                Some(child) if child.id() == pid => match child.try_wait() {
                    Ok(Some(status)) => {
                        guard.take();
                        Some(status.to_string())
                    }
                    _ => None,
                },
                _ => return,
            }
        };
        let Some(code) = exited else {
            continue;
        };
        let tail = log_tail(&app, 3);
        let message = if tail.is_empty() {
            format!("Xray завершился неожиданно ({code}).")
        } else {
            format!("Xray завершился неожиданно ({code}). {tail}")
        };
        // The inbound is gone: a leftover WinINET proxy would break every browser.
        restore_system_proxy();
        set_status(&app, |s| s.system_proxy = false);
        mark_failed(&app, message);
        return;
    });
}

fn stop_locked(app: &AppHandle) -> Result<(), String> {
    let taken = {
        let state = app.state::<AppState>();
        let mut guard = lock(&state.xray.child);
        guard.take()
    };
    let Some(mut child) = taken else {
        set_status(app, |s| {
            s.state = if binary_path().exists() {
                XrayState::Stopped
            } else {
                XrayState::NotInstalled
            };
            s.pid = None;
        });
        return Ok(());
    };
    set_status(app, |s| s.state = XrayState::Stopping);
    let pid = child.id();
    let _ = sys::command("taskkill.exe")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .status();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    let _ = child.kill();
    let _ = child.wait();
    // The HTTP inbound is gone, so a leftover system proxy would break browsers.
    restore_system_proxy();
    set_status(app, |s| {
        s.state = XrayState::Stopped;
        s.pid = None;
        s.system_proxy = false;
    });
    Ok(())
}

pub fn stop_on_exit(app: &AppHandle) {
    // Restore WinINET even when the core was already stopped: a crash during
    // shutdown must not leave the machine pointed at a dead local proxy.
    restore_system_proxy();
    let taken = {
        let state = app.state::<AppState>();
        let mut guard = lock(&state.xray.child);
        guard.take()
    };
    if taken.is_none() {
        return;
    }
    crate::job::terminate_all();
}

// ---------- Windows system proxy ----------

const WININET_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";

/// What WinINET had before we touched it, so the user's setup can be restored.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SystemProxyBackup {
    #[serde(default)]
    proxy_enable: Option<String>,
    #[serde(default)]
    proxy_server: Option<String>,
    #[serde(default)]
    auto_config: Option<String>,
}

fn backup_path() -> PathBuf {
    dir().join("system-proxy-backup.json")
}

fn wininet_read(name: &str) -> Option<String> {
    let output = sys::command("reg.exe")
        .args(["query", WININET_KEY, "/v", name])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    // `    ProxyEnable    REG_DWORD    0x1`
    let line = text.lines().find(|line| line.contains(name))?;
    line.split_whitespace()
        .last()
        .map(|value| value.to_string())
}

fn wininet_write(name: &str, kind: &str, value: &str) -> Result<(), String> {
    let status = sys::command("reg.exe")
        .args([
            "add",
            WININET_KEY,
            "/v",
            name,
            "/t",
            kind,
            "/d",
            value,
            "/f",
        ])
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Не удалось записать {name} в реестр."))
    }
}

fn wininet_delete(name: &str) {
    let _ = sys::command("reg.exe")
        .args(["delete", WININET_KEY, "/v", name, "/f"])
        .status();
}

fn system_proxy_active() -> bool {
    wininet_read("ProxyEnable")
        .map(|value| value != "0x0" && value != "0")
        .unwrap_or(false)
}

/// Points WinINET at the local HTTP inbound.
///
/// PAC is removed while we are in charge: when `AutoConfigURL` is set, Windows
/// ignores `ProxyServer`, and the user would silently keep the old rules.
pub fn apply_system_proxy(port: u16) -> Result<(), String> {
    if !backup_path().exists() {
        let backup = SystemProxyBackup {
            proxy_enable: wininet_read("ProxyEnable"),
            proxy_server: wininet_read("ProxyServer"),
            auto_config: wininet_read("AutoConfigURL"),
        };
        // Without a backup the original settings cannot be restored, so never
        // touch the registry until it is safely on disk.
        let text = serde_json::to_string(&backup).map_err(|e| e.to_string())?;
        fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
        fs::write(backup_path(), text)
            .map_err(|e| format!("Не удалось сохранить настройки прокси перед изменением: {e}"))?;
    }
    wininet_write("ProxyEnable", "REG_DWORD", "1")?;
    wininet_write("ProxyServer", "REG_SZ", &format!("127.0.0.1:{port}"))?;
    wininet_delete("AutoConfigURL");
    Ok(())
}

/// Puts the previous WinINET settings back. Safe to call when nothing was set.
pub fn restore_system_proxy() {
    let Ok(text) = fs::read_to_string(backup_path()) else {
        return;
    };
    let Ok(backup) = serde_json::from_str::<SystemProxyBackup>(&text) else {
        let _ = fs::remove_file(backup_path());
        return;
    };
    let mut errors: Vec<String> = Vec::new();
    let mut apply = |result: Result<(), String>| {
        if let Err(error) = result {
            errors.push(error);
        }
    };
    apply(match backup.proxy_enable.as_deref() {
        Some("0x1") | Some("1") => wininet_write("ProxyEnable", "REG_DWORD", "1"),
        _ => wininet_write("ProxyEnable", "REG_DWORD", "0"),
    });
    match backup.proxy_server.as_deref() {
        Some(server) if !server.is_empty() => {
            apply(wininet_write("ProxyServer", "REG_SZ", server));
        }
        _ => wininet_delete("ProxyServer"),
    }
    match backup.auto_config.as_deref() {
        Some(pac) if !pac.is_empty() => {
            apply(wininet_write("AutoConfigURL", "REG_SZ", pac));
        }
        _ => wininet_delete("AutoConfigURL"),
    }
    if errors.is_empty() {
        let _ = fs::remove_file(backup_path());
    } else {
        // Keep the backup: the next start or exit will retry with it.
        append_log_file(&format!(
            "Не удалось восстановить системный прокси: {}",
            errors.join("; ")
        ));
    }
}

pub fn system_proxy_enabled() -> bool {
    backup_path().exists() && system_proxy_active()
}

// ---------- settings ----------

pub fn update_settings(
    app: &AppHandle,
    patch: impl FnOnce(&mut config::XrayConfig),
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.xray.op);
    save_cfg(app, patch)?;
    if status(app).state == XrayState::Running {
        // `stop_locked` restores WinINET before the inbound disappears and
        // `start_locked` applies it again, so browsers never point at a dead
        // port while the core is restarting.
        stop_locked(app)?;
        std::thread::sleep(Duration::from_millis(300));
        start_locked(app)?;
    }
    Ok(())
}

/// Saves a setting that the running core does not read (autostart, UI flags).
///
/// Restarting the tunnel for these dropped every connection for nothing.
pub fn update_settings_hot(
    app: &AppHandle,
    patch: impl FnOnce(&mut config::XrayConfig),
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.xray.op);
    save_cfg(app, patch)
}

pub fn select_profile(app: &AppHandle, index: usize) -> Result<(), String> {
    let list = ensure_profiles(app)?;
    if index >= list.len() {
        return Err(format!("Профиль {index} не найден."));
    }
    update_settings(app, move |cfg| cfg.selected = index)
}

/// Replaces the whitelist of sites that go through the tunnel.
///
/// Values are normalised here (a pasted URL becomes a bare domain) so the same
/// list can be compared and rendered without surprises.
pub fn set_proxy_sites(app: &AppHandle, sites: Vec<String>) -> Result<Vec<String>, String> {
    let mut cleaned: Vec<String> = Vec::new();
    for raw in sites {
        if let Some(site) = normalize_site(&raw) {
            if let Some(code) = site.strip_prefix("geosite:") {
                check_category(code)?;
            }
            if !cleaned
                .iter()
                .any(|known| known.eq_ignore_ascii_case(&site))
            {
                cleaned.push(site);
            }
        }
    }
    let saved = cleaned.clone();
    update_settings(app, move |cfg| cfg.proxy_sites = cleaned)?;
    Ok(saved)
}

/// Hosts that never belong to the site itself: counters, analytics, fonts and
/// plain social links that a page happens to reference.
const NOISE_HOSTS: &[&str] = &[
    "yadro.ru",
    "liveinternet.ru",
    "google-analytics.com",
    "googletagmanager.com",
    "googlesyndication.com",
    "doubleclick.net",
    "adservice.google.com",
    "mc.yandex.ru",
    "yandex.ru",
    "vk.com",
    "ok.ru",
    "mail.ru",
    "t.me",
    "telegram.org",
    "fonts.googleapis.com",
    "fonts.gstatic.com",
    "ajax.googleapis.com",
    "cdn.jsdelivr.net",
    "unpkg.com",
    "jquery.com",
    "cloudflare.com",
];

/// Second-level label of a domain: `static.rutracker.cc` → `rutracker`.
fn site_token(domain: &str) -> String {
    let labels: Vec<&str> = domain.split('.').filter(|part| !part.is_empty()).collect();
    match labels.len() {
        0 => String::new(),
        1 => labels[0].to_string(),
        _ => labels[labels.len() - 2].to_string(),
    }
}

fn group_for(domain: &str) -> Option<&'static [&'static str]> {
    let token = site_token(domain);
    SITE_GROUPS
        .iter()
        .find(|(_, domains)| {
            domains
                .iter()
                .any(|candidate| *candidate == domain || site_token(candidate) == token)
        })
        .map(|(_, domains)| *domains)
}

fn is_noise(host: &str) -> bool {
    NOISE_HOSTS
        .iter()
        .any(|noise| host == *noise || host.ends_with(&format!(".{noise}")))
}

/// A host the page references, with how often it appears.
///
/// `hits` is the poor man's DevTools: resources are referenced dozens of times,
/// while a plain link appears once, so frequency separates assets from links.
#[derive(Debug, Clone, Serialize)]
pub struct SiteCandidate {
    pub host: String,
    pub hits: usize,
    /// Same service, a known bundle member, or referenced heavily.
    pub strong: bool,
}

/// Extracts every host the page references, keeping the ones that are not the
/// site itself and not obvious noise.
fn related_hosts(domain: &str, html: &str) -> Vec<SiteCandidate> {
    let token = site_token(domain);
    let group = group_for(domain);
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for marker in ["https://", "http://"] {
        let mut rest = html;
        while let Some(index) = rest.find(marker) {
            rest = &rest[index + marker.len()..];
            let host: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
                .collect();
            let host = host
                .trim_end_matches('.')
                .trim_start_matches("www.")
                .to_ascii_lowercase();
            if host.is_empty() || !host.contains('.') {
                continue;
            }
            if host == domain || host.ends_with(&format!(".{domain}")) || is_noise(&host) {
                continue;
            }
            *counts.entry(host).or_insert(0) += 1;
        }
    }

    let mut hosts: Vec<SiteCandidate> = counts
        .into_iter()
        .map(|(host, hits)| {
            let by_name = site_token(&host) == token;
            let by_bundle = group
                .map(|domains| {
                    domains.iter().any(|candidate| {
                        host == *candidate || host.ends_with(&format!(".{candidate}"))
                    })
                })
                .unwrap_or(false);
            SiteCandidate {
                strong: by_name || by_bundle || hits >= 20,
                host,
                hits,
            }
        })
        .collect();
    // Strong matches first, then the most referenced ones.
    hosts.sort_by(|a, b| {
        b.strong
            .cmp(&a.strong)
            .then_with(|| b.hits.cmp(&a.hits))
            .then_with(|| a.host.cmp(&b.host))
    });
    hosts.truncate(12);
    hosts
}

/// Loads the site through the tunnel, retrying while the core comes back up.
///
/// Adding a site restarts the tunnel, and the first request can hit the inbound
/// before it is listening. reqwest's own text for that is only "error sending
/// request for url (...)"; the chain is unwrapped so the real reason is shown.
fn fetch_page(client: &reqwest::blocking::Client, domain: &str) -> Result<String, String> {
    let mut last_error = String::from("неизвестная ошибка");
    for attempt in 1..=3 {
        let result = client
            .get(format!("https://{domain}/"))
            .send()
            .map_err(|e| describe_error(&e))
            .and_then(|response| {
                if response.status().is_success() {
                    response.text().map_err(|e| describe_error(&e))
                } else {
                    Err(format!("сервер ответил {}", response.status()))
                }
            });
        match result {
            Ok(html) => return Ok(html),
            Err(error) => {
                last_error = error;
                if attempt < 3 {
                    std::thread::sleep(Duration::from_millis(700));
                }
            }
        }
    }
    Err(format!("Не удалось загрузить {domain}: {last_error}"))
}

/// Suggests the extra domains a site needs, by loading its page through the
/// tunnel and looking at what hosts it references.
pub fn discover_sites(app: &AppHandle, site: &str) -> Result<Vec<SiteCandidate>, String> {
    let cfg = load_cfg(app);
    if status(app).state != XrayState::Running {
        return Err("Сначала включите VPN: страница загружается через туннель.".into());
    }
    let domain = normalize_site(site)
        .and_then(|normalized| normalized.strip_prefix("domain:").map(str::to_string))
        .ok_or("Некорректный домен.")?;

    let proxy = reqwest::Proxy::http(format!("http://127.0.0.1:{}", cfg.http_port))
        .map_err(|e| e.to_string())?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Splitline")
        .proxy(proxy)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|e| e.to_string())?;
    let html = fetch_page(&client, &domain)?;

    let known: Vec<String> = load_cfg(app)
        .proxy_sites
        .iter()
        .filter_map(|site| normalize_site(site))
        .collect();
    Ok(related_hosts(&domain, &html)
        .into_iter()
        .filter(|candidate| {
            !known
                .iter()
                .any(|site| *site == format!("domain:{}", candidate.host))
        })
        .collect())
}

/// Imports a domain list from a text file (one entry per line, `#` comments).
pub fn import_sites(app: &AppHandle, path: &str) -> Result<Vec<String>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("Не удалось прочитать файл: {e}"))?;
    let mut merged = load_cfg(app).proxy_sites;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        // Accept plain hostnames as well as full URLs and Xray prefixes.
        merged.push(line.to_string());
    }
    set_proxy_sites(app, merged)
}

/// Writes the whitelist as a plain domain list, one per line.
///
/// The file has exactly the shape [`import_sites`] reads back, so a list can be
/// moved between machines (comments and blank lines are ignored on import).
pub fn export_sites(app: &AppHandle, path: &str) -> Result<usize, String> {
    let mut domains: Vec<String> = Vec::new();
    for site in load_cfg(app).proxy_sites {
        let Some(normalized) = normalize_site(&site) else {
            continue;
        };
        let domain = normalized
            .strip_prefix("domain:")
            .unwrap_or(&normalized)
            .to_string();
        if !domains
            .iter()
            .any(|known| known.eq_ignore_ascii_case(&domain))
        {
            domains.push(domain);
        }
    }
    if domains.is_empty() {
        return Err("Список пуст — выгружать нечего.".into());
    }
    let mut text = String::from("# Splitline: домены через VPN, по одному в строке\n");
    for domain in &domains {
        text.push_str(domain);
        text.push('\n');
    }
    fs::write(path, text).map_err(|e| format!("Не удалось записать файл: {e}"))?;
    Ok(domains.len())
}

/// Turns the Windows system proxy on or off without recreating the core.
pub fn set_system_proxy(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.xray.op);
    save_cfg(app, move |cfg| cfg.system_proxy = enabled)?;
    let cfg = load_cfg(app);
    if enabled {
        if status(app).state == XrayState::Running {
            apply_system_proxy(cfg.http_port)?;
        }
    } else {
        restore_system_proxy();
    }
    set_status(app, |s| s.system_proxy = enabled && system_proxy_active());
    Ok(())
}

/// `tg://socks` link: Telegram Desktop understands it and opens its own
/// "add proxy" dialog, so the user only confirms one button.
pub fn telegram_link(socks_port: u16) -> String {
    format!("tg://socks?server=127.0.0.1&port={socks_port}")
}

pub fn open_in_telegram(app: &AppHandle) -> Result<(), String> {
    let cfg = load_cfg(app);
    let link = telegram_link(cfg.socks_port);
    let path = Path::new(&link);
    sys::open_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "remarks": "🇩🇪 Германия №2",
            "inbounds": [
                { "protocol": "socks", "listen": "127.0.0.1", "port": 10808 },
                { "protocol": "http", "listen": "127.0.0.1", "port": 10809 }
            ],
            "outbounds": [
                {
                    "tag": "proxy",
                    "protocol": "vless",
                    "settings": { "vnext": [{ "address": "176.100.36.187", "port": 443 }] },
                    "streamSettings": { "network": "tcp", "security": "reality" }
                },
                { "tag": "direct", "protocol": "freedom" }
            ]
        })
    }

    #[test]
    fn profile_name_comes_from_remarks() {
        assert_eq!(profile_name(&sample()).as_deref(), Some("🇩🇪 Германия №2"));
        assert_eq!(profile_name(&json!({})), None);
    }

    #[test]
    fn primary_outbound_is_the_proxy_one() {
        let (host, port) = outbound_endpoint(&sample()).expect("endpoint");
        assert_eq!(host, "176.100.36.187");
        assert_eq!(port, 443);
        assert_eq!(outbound_scheme(&sample()), "VLESS / TCP / REALITY");
    }

    #[test]
    fn direct_outbound_is_found_by_tag() {
        assert_eq!(direct_tag(&sample()).as_deref(), Some("direct"));
    }

    #[test]
    fn direct_outbound_falls_back_to_any_freedom() {
        let cfg = json!({
            "outbounds": [
                { "tag": "proxy", "protocol": "vless" },
                { "tag": "freedom-out", "protocol": "freedom" }
            ]
        });
        assert_eq!(direct_tag(&cfg).as_deref(), Some("freedom-out"));
    }

    #[test]
    fn direct_outbound_is_missing_without_freedom() {
        let cfg = json!({ "outbounds": [{ "tag": "proxy", "protocol": "vless" }] });
        assert_eq!(direct_tag(&cfg), None);
    }

    #[test]
    fn telegram_ranges_cover_media_dcs() {
        for cidr in ["91.108.4.0/22", "91.108.56.0/22", "95.161.76.0/23"] {
            assert!(TELEGRAM_CIDRS.contains(&cidr), "missing {cidr}");
        }
    }

    #[test]
    fn telegram_link_is_a_socks_link() {
        assert_eq!(
            telegram_link(17808),
            "tg://socks?server=127.0.0.1&port=17808"
        );
    }

    #[test]
    fn sites_are_normalised_to_domains() {
        assert_eq!(
            normalize_site("https://Example.com/path?x=1"),
            Some("domain:example.com".into())
        );
        assert_eq!(
            normalize_site("example.com:443"),
            Some("domain:example.com".into())
        );
        // `www.` is dropped: `domain:example.com` already covers the bare host
        // and every subdomain, while `domain:www.example.com` would not.
        assert_eq!(
            normalize_site("https://www.instagram.com/direct/"),
            Some("domain:instagram.com".into())
        );
        assert_eq!(
            normalize_site("WWW.Example.COM"),
            Some("domain:example.com".into())
        );
        assert_eq!(
            normalize_site("full:cdn.example.com"),
            Some("full:cdn.example.com".into())
        );
        // A bundle is a category from the downloaded lists, not a host.
        assert_eq!(
            normalize_site("geosite:Discord"),
            Some("geosite:discord".into())
        );
        assert_eq!(
            normalize_site("geosite:geolocation-!cn"),
            Some("geosite:geolocation-!cn".into())
        );
        assert_eq!(normalize_site("geosite:"), None);
        assert_eq!(normalize_site("geosite:has space"), None);
        assert_eq!(normalize_site("   "), None);
        assert_eq!(normalize_site("localhost"), None);
    }

    #[test]
    fn whitelist_routing_keeps_telegram_and_sends_the_rest_direct() {
        let rules = build_rules(
            &["example.com".into()],
            ("balancerTag", "auto-sni"),
            "direct",
        );
        let list = rules.as_array().expect("rules");
        // Last rule is the default: everything else stays outside the tunnel.
        let last = list.last().expect("default rule");
        assert_eq!(last["outboundTag"], "direct");
        // Telegram must stay on the tunnel side, otherwise the SOCKS proxy
        // would send Telegram into the blocked direct route.
        assert!(
            list.iter().any(|rule| rule["ip"]
                .as_array()
                .map(|ips| ips.iter().any(|ip| ip == "149.154.160.0/20"))
                .unwrap_or(false)
                && rule["balancerTag"] == "auto-sni"),
            "telegram ranges must be routed through the tunnel"
        );
        assert!(
            list.iter().any(|rule| rule["domain"]
                .as_array()
                .map(|domains| domains.iter().any(|d| d == "domain:example.com"))
                .unwrap_or(false)),
            "the whitelisted site must be routed through the tunnel"
        );
    }

    #[test]
    fn stored_domain_prefix_is_cleaned_too() {
        // Saved by a build that did not strip `www.` yet.
        assert_eq!(
            normalize_site("domain:www.tiktok.com"),
            Some("domain:tiktok.com".into())
        );
        assert_eq!(
            normalize_site("domain:www.instagram.com"),
            Some("domain:instagram.com".into())
        );
    }

    #[test]
    fn bundles_cover_the_services_the_user_picked() {
        let bundles = site_bundles();
        assert_eq!(bundles.len(), 13);
        let names: Vec<&str> = bundles.iter().map(|bundle| bundle.name.as_str()).collect();
        for expected in [
            "Instagram",
            "Discord",
            "TikTok",
            "YouTube",
            "Twitter / X",
            "Facebook",
            "ChatGPT",
            "Rutracker",
            "Telegram",
            "WhatsApp",
            "Reddit",
            "Spotify",
            "LinkedIn",
        ] {
            assert!(names.contains(&expected), "missing bundle {expected}");
        }
        // Instagram needs `meta`: the `instagram` category misses fbcdn.net.
        let instagram = bundles
            .iter()
            .find(|bundle| bundle.name == "Instagram")
            .expect("instagram bundle");
        assert_eq!(instagram.category, "meta");
    }

    #[test]
    fn related_hosts_keeps_the_same_service_only() {
        let html = r#"
            <link href="https://static.rutracker.cc/a.css">
            <script src="https://static.rutracker.cc/b.js"></script>
            <img src="https://rutracker.wiki/logo.png">
            <img src="https://counter.yadro.ru/hit">
            <a href="https://t.me/rutracker">tg</a>
            <img src="https://cdninstagram.com/x.png">
        "#;
        let candidates = related_hosts("rutracker.org", html);
        let hosts: Vec<String> = candidates.iter().map(|c| c.host.clone()).collect();
        // The asset host comes first and is pre-selected.
        assert_eq!(hosts[0], "static.rutracker.cc");
        assert!(candidates[0].strong);
        assert!(hosts.contains(&"rutracker.wiki".to_string()));
        // Unrelated hosts are offered too (weak), so nothing is silently missed.
        assert!(hosts.contains(&"cdninstagram.com".to_string()));
        // Counters and messenger links never make it into the list.
        assert!(!hosts
            .iter()
            .any(|host| host.contains("yadro") || host == "t.me"));
    }

    #[test]
    fn related_hosts_uses_the_bundle_for_cdn_domains() {
        // fbcdn.net does not share the "instagram" token, but it is part of the
        // Instagram bundle, so it must be suggested.
        let html = r#"
            <script src="https://static.cdninstagram.com/app.js"></script>
            <img src="https://scontent-hel3-1.cdninstagram.com/p.jpg">
            <img src="https://scontent.xx.fbcdn.net/p.jpg">
        "#;
        let hosts: Vec<String> = related_hosts("instagram.com", html)
            .into_iter()
            .map(|candidate| candidate.host)
            .collect();
        assert!(hosts.contains(&"static.cdninstagram.com".to_string()));
        assert!(hosts.contains(&"scontent.xx.fbcdn.net".to_string()));
    }

    #[test]
    fn site_token_is_the_second_level_label() {
        assert_eq!(site_token("rutracker.org"), "rutracker");
        assert_eq!(site_token("static.rutracker.cc"), "rutracker");
        assert_eq!(site_token("example.com"), "example");
    }

    #[test]
    fn empty_whitelist_has_no_domain_rule() {
        let rules = build_rules(&[], ("balancerTag", "auto-sni"), "direct");
        let list = rules.as_array().expect("rules");
        assert!(!list.iter().any(|rule| rule.get("domain").is_some()));
        assert_eq!(list.last().expect("default")["outboundTag"], "direct");
    }

    #[test]
    fn port_validation_matches_the_ui() {
        assert!(validate_port(17808).is_ok());
        assert!(validate_port(80).is_err());
    }
}
