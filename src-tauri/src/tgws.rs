//! Supervisor for the bundled Telegram MTProto-over-WebSocket proxy.
//!
//! Two engines speak the same protocol, so the page, the link format and the
//! supervisor are shared and only the command line differs:
//!
//! * `python` — [Dezuron/tgws-cli], a headless build (MIT) of
//!   [Flowseal/tg-ws-proxy], published from its own repository: this project
//!   only downloads the released binary and never builds it here. Proven,
//!   ~30–50 MB RSS.
//! * `rs` — [valnesfjord/tg-ws-proxy-rs] (MIT), a static Windows build with
//!   several Cloudflare domains, a Worker fallback, inbound FakeTLS and its own
//!   `--check-listener` self-check. ~3–5 MB RSS, young.
//!
//! The old engine stays the default and installable: switching is one setting,
//! the secret and therefore the `tg://` link survive it, and a failed start of
//! the new engine puts the old one back (R8, R9, R11).
//!
//! [Flowseal/tg-ws-proxy]: https://github.com/Flowseal/tg-ws-proxy
//! [Dezuron/tgws-cli]: https://github.com/Dezuron/tgws-cli
//! [valnesfjord/tg-ws-proxy-rs]: https://github.com/valnesfjord/tg-ws-proxy-rs

use crate::config;
use crate::lock_config;
use crate::supervisor::{RestartDecision, RestartPolicy, RESTART_DELAYS_SECS};
use crate::sys;
use crate::AppState;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

/// Repository that publishes the headless proxy binary. It lives outside this
/// project so the application's release list stays clean (and `/releases/latest`
/// keeps pointing at an app release, which the self-updater depends on).
const REPO: &str = "Dezuron/tgws-cli";
/// Project the proxy actually comes from. The user is offered a Flowseal
/// release, not our rebuild of it, so the UI shows this repository's versions
/// and release notes.
const UPSTREAM_REPO: &str = "Flowseal/tg-ws-proxy";
const ASSET_NAME: &str = "tgws-cli-windows-x64.exe";
/// Image name of our own binary; only this image is ever killed by name.
const BINARY_IMAGE: &str = "tgws-cli.exe";
/// Repository and assets of the Rust engine (R2). Releases carry one archive
/// per target; both Windows x64 archives hold a single `tg-ws-proxy.exe`.
const RS_REPO: &str = "valnesfjord/tg-ws-proxy-rs";
/// Accepted Windows x64 archives, most preferred first. The regular GNU build
/// needs Windows 10, which this application already requires; the Windows 7
/// MSVC build is only a fallback for releases that do not publish the regular
/// one (for example v2.5.8).
const RS_ASSETS: [&str; 2] = [
    "tg-ws-proxy-x86_64-pc-windows-gnu.zip",
    "tg-ws-proxy-x86_64-win7-windows-msvc.zip",
];
/// Name of the executable inside those archives.
const RS_ARCHIVE_EXE: &str = "tg-ws-proxy.exe";
/// File name the Rust engine gets on disk: next to the old binary, so both
/// engines can be installed at the same time.
const RS_IMAGE: &str = "tgws-rs.exe";
/// Smallest plausible executable; a shorter payload is an error page or a
/// truncated archive.
const MIN_EXE_BYTES: u64 = 256 * 1024;
/// Failed starts of the new engine after which the UI is told to go back.
/// The application never switches by itself (R8).
const RS_FAILED_STARTS: u32 = 3;
/// Budget of the on-demand `--check-listener` probe. It talks to Telegram, so
/// a blocked network must not leave the dialog spinning forever.
const LISTENER_CHECK_TIMEOUT: Duration = Duration::from_secs(45);
/// Largest Cloudflare domain list the UI may save. Ten is far beyond what the
/// proxy needs and keeps the argv bounded.
pub const MAX_CF_DOMAINS: usize = 10;
/// Largest `DC:IP` redirect list, for the same reason.
pub const MAX_DC_REDIRECTS: usize = 10;
/// Highest Telegram DC id, including the test datacenters.
pub const MAX_DC_ID: u16 = 203;
/// Socket buffer range accepted by both engines (`--buf-kb`). Four is the
/// upstream minimum the argv builder has always clamped to; the upper bound
/// keeps a typo from reserving a gigabyte per socket.
pub const MIN_BUF_KB: u32 = 4;
pub const MAX_BUF_KB: u32 = 4096;
/// Pre-warmed WebSocket connections per DC (`--pool-size`). Zero is a valid
/// "no pool" setting; 32 is far beyond any useful value.
pub const MAX_POOL_SIZE: u32 = 32;
/// Standalone Flowseal tray app that occupies the same default port.
const STANDALONE_IMAGE: &str = "TgWsProxy_windows.exe";

const LOG_LINES: usize = 300;
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
const MIN_DOWNLOAD_BYTES: u64 = 1024 * 1024;
const MAX_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024;
const CACHE_TTL_SECS: u64 = 12 * 60 * 60;
/// How long the child gets to fail its `bind()` before we call it healthy.
const START_GRACE: Duration = Duration::from_millis(700);
/// How often the supervisor polls the child for an exit. `Child` has no exit
/// event, so a light poll is the only portable option; one `try_wait` per
/// half-second costs nothing measurable.
const WATCH_INTERVAL: Duration = Duration::from_millis(500);
/// Step of the cancellable wait before a restart.
const RESTART_POLL: Duration = Duration::from_millis(250);
/// At most one "proxy restarted" toast per this window, so a flapping proxy
/// cannot flood the UI.
const RESTART_NOTICE_QUIET_SECS: u64 = 600;
/// One liveness probe per minute: a local TCP connect, no traffic at all.
const HEALTH_INTERVAL: Duration = Duration::from_secs(60);
/// Failed probes in a row that mean the listener is gone.
const HEALTH_FAILURES: u8 = 3;
/// Connect timeout of a liveness probe (same as the manual diagnostics check).
const HEALTH_TIMEOUT: Duration = Duration::from_millis(400);

/// Which proxy binary serves the user.
///
/// The value is persisted as a string (`tgws.engine`), so this type is the
/// only place that knows the vocabulary and the file names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyEngine {
    /// Headless Flowseal build, the default in 2.2.0.
    Python,
    /// `tg-ws-proxy-rs`; opt-in, offered with a way back.
    Rs,
}

impl ProxyEngine {
    /// Engine used when nothing is configured or the value is unknown: a
    /// hand-edited config must never leave the user without a proxy.
    pub const DEFAULT: ProxyEngine = ProxyEngine::Python;

    /// Reads the persisted value leniently. Anything unrecognised means the
    /// default engine.
    pub fn from_config(value: &str) -> Self {
        Self::parse(value).unwrap_or(Self::DEFAULT)
    }

    /// Reads the persisted value strictly, for user input. An empty string
    /// means the default engine.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "python" => Ok(Self::Python),
            "rs" | "rust" => Ok(Self::Rs),
            _ => Err("Неизвестный движок: допустимы python и rs.".into()),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Rs => "rs",
        }
    }

    /// Image name used by `tasklist` / `taskkill` and for killing leftovers.
    pub fn image(&self) -> &'static str {
        match self {
            Self::Python => BINARY_IMAGE,
            Self::Rs => RS_IMAGE,
        }
    }

    pub fn binary_path(&self) -> PathBuf {
        match self {
            Self::Python => config::tgws_binary_path(),
            Self::Rs => config::tgws_rs_binary_path(),
        }
    }

    /// Human-readable name for logs and notices.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Python => "Старый движок",
            Self::Rs => "Новый движок",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TgwsState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error,
    NotInstalled,
}

#[derive(Debug, Clone, Serialize)]
pub struct TgwsStatus {
    pub state: TgwsState,
    pub pid: Option<u32>,
    pub last_error: Option<String>,
    pub installed_version: String,
    pub binary_present: bool,
    /// True when the last start failed because the configured port is taken by
    /// a foreign process; the UI then offers a one-click fallback port.
    pub port_conflict: bool,
    /// Total connections accepted from a Telegram client since the proxy
    /// started (monotonic; parsed from proxy stats and connection events).
    pub client_connections_total: u64,
    /// Clients connected right now (`stats: … active=N`). `None` until the
    /// proxy prints a stats line that carries `active=`, and for builds that
    /// never print it; the UI then falls back to `last_client_at`.
    #[serde(default)]
    pub active_connections: Option<u32>,
    /// Unix time of the last proof that a Telegram client is on the proxy: a
    /// `] DC` / `WS pool hit` line or a stats line with `active > 0`.
    #[serde(default)]
    pub last_client_at: Option<u64>,
    /// Unix time of the last line that proves the proxy forwarded data (`] DC`
    /// / `WS pool hit`). The proxy logs no byte counter, so a client that is
    /// connected but idle keeps `last_client_at` fresh and leaves this one old.
    #[serde(default)]
    pub last_traffic_at: Option<u64>,
    /// 1-based number of the current automatic restart; `0` when the proxy
    /// started normally or was stopped by the user.
    #[serde(default)]
    pub restart_attempt: u32,
    /// Engine the config points at: `python` or `rs`.
    #[serde(default)]
    pub engine: String,
    /// Consecutive failed starts of the new engine. Reset by a successful
    /// start; the old engine ignores it.
    #[serde(default)]
    pub engine_start_failures: u32,
    /// The new engine failed [`RS_FAILED_STARTS`] times in a row: the UI should
    /// offer the old one. Nothing switches automatically (R8).
    #[serde(default)]
    pub fallback_recommended: bool,
    /// Whether the Rust engine is on disk, so `Дополнительно` can show the
    /// switch as "download on change" or "installed".
    #[serde(default)]
    pub rust_binary_present: bool,
}

/// Runtime state of the supervisor, stored in [`AppState`].
pub struct Tgws {
    child: Mutex<Option<Child>>,
    status: Mutex<TgwsStatus>,
    logs: Mutex<VecDeque<String>>,
    /// Serialises start/stop/restart so two clicks cannot race.
    op: Mutex<()>,
    cache: Mutex<Option<CachedRelease>>,
    cache_hydrated: Mutex<bool>,
    /// The user wants the proxy running. An unexpected exit is only restarted
    /// while this is set; a manual stop clears it.
    desired: AtomicBool,
    /// Restart budget shared by every watcher thread of the current episode.
    restarts: Mutex<RestartPolicy>,
    /// Bumped on every start and stop. A health-check timer from an older run
    /// sees a different value and exits, which is how it is cancelled.
    epoch: AtomicU64,
    /// One recovery at a time: the exit watcher and the health check can both
    /// observe the same broken process.
    recovering: AtomicBool,
    /// Last time the "proxy restarted" toast was shown.
    restart_notice_at: Mutex<Option<u64>>,
}

impl Default for Tgws {
    fn default() -> Self {
        Self::new()
    }
}

impl Tgws {
    pub fn new() -> Self {
        Self {
            child: Mutex::new(None),
            status: Mutex::new(TgwsStatus {
                state: TgwsState::Stopped,
                pid: None,
                last_error: None,
                installed_version: String::new(),
                binary_present: false,
                port_conflict: false,
                client_connections_total: 0,
                active_connections: None,
                last_client_at: None,
                last_traffic_at: None,
                restart_attempt: 0,
                engine: ProxyEngine::DEFAULT.as_str().into(),
                engine_start_failures: 0,
                fallback_recommended: false,
                rust_binary_present: false,
            }),
            logs: Mutex::new(VecDeque::new()),
            op: Mutex::new(()),
            cache: Mutex::new(None),
            cache_hydrated: Mutex::new(false),
            desired: AtomicBool::new(false),
            restarts: Mutex::new(RestartPolicy::new()),
            epoch: AtomicU64::new(0),
            recovering: AtomicBool::new(false),
            restart_notice_at: Mutex::new(None),
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

// ---------- user intent ----------

/// The proxy should be running. Automatic restarts follow this flag, so a
/// manual stop is enough to cancel a pending restart.
fn desired(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .tgws
        .desired
        .load(AtomicOrdering::SeqCst)
}

fn set_desired(app: &AppHandle, value: bool) {
    app.state::<AppState>()
        .tgws
        .desired
        .store(value, AtomicOrdering::SeqCst);
}

/// Starts a new supervision cycle. Timers from the previous proxy instance
/// compare their epoch with this value and stop.
fn bump_epoch(app: &AppHandle) -> u64 {
    app.state::<AppState>()
        .tgws
        .epoch
        .fetch_add(1, AtomicOrdering::SeqCst)
        + 1
}

// ---------- config access ----------

fn load_cfg(app: &AppHandle) -> config::TgwsConfig {
    let state = app.state::<AppState>();
    let cfg = lock_config(&state);
    cfg.tgws.clone()
}

fn save_cfg(app: &AppHandle, patch: impl FnOnce(&mut config::TgwsConfig)) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut cfg = lock_config(&state);
    patch(&mut cfg.tgws);
    config::save_config(&cfg)
}

// ---------- status ----------

/// Binary of the engine the config points at. Everything that decides between
/// `binary_present` and `not_installed` goes through this, so the UI reports
/// the engine it is actually about to run.
fn active_engine(app: &AppHandle) -> ProxyEngine {
    ProxyEngine::from_config(&load_cfg(app).engine)
}

fn active_binary_present(app: &AppHandle) -> bool {
    active_engine(app).binary_path().exists()
}

pub fn status(app: &AppHandle) -> TgwsStatus {
    let state = app.state::<AppState>();
    let mut snapshot = lock(&state.tgws.status).clone();
    let cfg = load_cfg(app);
    let engine = ProxyEngine::from_config(&cfg.engine);
    snapshot.engine = engine.as_str().into();
    snapshot.binary_present = engine.binary_path().exists();
    snapshot.rust_binary_present = config::tgws_rs_binary_path().exists();
    // The user sees the Flowseal version; the mirror's `-N` build revision is
    // an internal detail (see `upstream_version`). The Rust engine has no
    // mirror, so its tag is shown as is.
    snapshot.installed_version = match engine {
        ProxyEngine::Python => upstream_version(&cfg.installed_version),
        ProxyEngine::Rs => cfg
            .rs_version
            .trim()
            .trim_start_matches(['v', 'V'])
            .to_string(),
    };
    if !snapshot.binary_present
        && !matches!(
            snapshot.state,
            TgwsState::Running | TgwsState::Starting | TgwsState::Stopping
        )
    {
        snapshot.state = TgwsState::NotInstalled;
    }
    snapshot
}

fn set_status(app: &AppHandle, patch: impl FnOnce(&mut TgwsStatus)) -> TgwsStatus {
    let state = app.state::<AppState>();
    let snapshot = {
        let mut status = lock(&state.tgws.status);
        patch(&mut status);
        status.clone()
    };
    let _ = app.emit("tgws-status", &snapshot);
    snapshot
}

fn mark_failed(app: &AppHandle, message: impl Into<String>) -> String {
    let message = message.into();
    let installed = active_binary_present(app);
    set_status(app, |s| {
        s.state = if installed {
            TgwsState::Error
        } else {
            TgwsState::NotInstalled
        };
        s.pid = None;
        s.last_error = Some(message.clone());
        // Nothing is listening any more, so "clients right now" is unknown;
        // `last_client_at` stays as history.
        s.active_connections = None;
    });
    message
}

// ---------- log capture ----------

pub fn mask_secret(line: &str, secret: &str) -> String {
    if secret.is_empty() {
        line.to_string()
    } else {
        line.replace(secret, "***")
    }
}

fn push_log(app: &AppHandle, line: String) {
    apply_log_event(app, classify_line(&line));
    store_log(app, line);
}

/// Stores a line written by the supervisor itself. It is never classified: the
/// text may quote a proxy line (`stats:` or `] DC`), and a quoted line must not
/// be mistaken for fresh activity.
fn push_supervisor_log(app: &AppHandle, line: String) {
    store_log(app, line);
}

fn store_log(app: &AppHandle, line: String) {
    let state = app.state::<AppState>();
    {
        let mut logs = lock(&state.tgws.logs);
        if logs.len() >= LOG_LINES {
            logs.pop_front();
        }
        logs.push_back(line.clone());
    }
    append_log_file(&line);
    let _ = app.emit("tgws-log", &line);
}

/// Facts carried by one proxy log line. Pure: no clock, no application state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogEvent {
    /// `stats: total=N active=M`.
    Stats(StatsLine),
    /// A client stream reached a datacenter (`] DC`, `WS pool hit`).
    Traffic,
    Other,
}

/// One `stats:` line: the cumulative client counter and, when the build prints
/// it, the number of clients connected right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsLine {
    pub total: u64,
    pub active: Option<u32>,
}

fn classify_line(line: &str) -> LogEvent {
    match parse_stats(line) {
        Some(stats) => LogEvent::Stats(stats),
        None if is_client_connected_event(line) => LogEvent::Traffic,
        None => LogEvent::Other,
    }
}

/// Applies what a log line proves about the proxy: the cumulative client
/// counter, the live count and the activity timestamps.
///
/// Emits a status event at most once per second, so a busy proxy cannot flood
/// the UI while `last_client_at` still stays within a second of the truth.
fn apply_log_event(app: &AppHandle, event: LogEvent) {
    let now = now_secs();
    match event {
        LogEvent::Stats(stats) => {
            set_status(app, |s| {
                s.client_connections_total = s.client_connections_total.max(stats.total);
                // A stats line without `active=` means this build does not
                // report it: keep `None` rather than a count that never
                // changes, so the UI falls back to `last_client_at`.
                s.active_connections = stats.active;
                if stats.active.is_some_and(|active| active > 0) {
                    s.last_client_at = Some(now);
                }
            });
        }
        LogEvent::Traffic => {
            let fresh = {
                let state = app.state::<AppState>();
                let status = lock(&state.tgws.status);
                status.client_connections_total == 0 || status.last_traffic_at != Some(now)
            };
            if !fresh {
                return;
            }
            set_status(app, |s| {
                // A real connection must be visible immediately: the stats line
                // only arrives once a minute.
                s.client_connections_total = s.client_connections_total.max(1);
                s.last_client_at = Some(now);
                s.last_traffic_at = Some(now);
            });
        }
        LogEvent::Other => {}
    }
}

/// `[127.0.0.1:5013] DC2 -> WS pool hit` / `... DC203 -> trying CF proxy` is
/// logged as soon as the proxy accepts a client, unlike the 60-second stats.
fn is_client_connected_event(line: &str) -> bool {
    line.contains("] DC") || line.contains("WS pool hit")
}

/// `stats: total=3 active=1 ws=1 h2=0 bad=0`.
///
/// `total` is required (an unknown counter is worse than no counter); `active`
/// is optional because older builds do not print it and the field must never
/// make the whole line unparsable.
fn parse_stats(line: &str) -> Option<StatsLine> {
    let rest = line.split("stats:").nth(1)?;
    Some(StatsLine {
        total: stat_number(rest, "total")?,
        active: stat_number(rest, "active").and_then(|value| u32::try_from(value).ok()),
    })
}

/// Value of `key=N` among the space-separated `key=value` pairs of a stats line.
fn stat_number(text: &str, key: &str) -> Option<u64> {
    let value = text
        .split_whitespace()
        .find_map(|token| token.strip_prefix(key)?.strip_prefix('='))?;
    let digits: String = value.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn append_log_file(line: &str) {
    let path = config::tgws_log_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::metadata(&path)
        .map(|m| m.len() > LOG_MAX_BYTES)
        .unwrap_or(false)
    {
        let backup = path.with_extension("log.1");
        // Windows refuses to rename over an existing file, so drop the old
        // backup first; if the rename still fails (file locked), truncate the
        // log so it cannot grow without bound.
        let _ = fs::remove_file(&backup);
        if fs::rename(&path, &backup).is_err() {
            let _ = fs::write(&path, b"");
        }
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{line}");
    }
}

fn spawn_reader(app: AppHandle, stream: impl Read + Send + 'static, secret: String) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines().map_while(Result::ok) {
            push_log(&app, mask_secret(&line, &secret));
        }
    });
}

pub fn logs(app: &AppHandle) -> Vec<String> {
    let state = app.state::<AppState>();
    let logs = lock(&state.tgws.logs);
    logs.iter().cloned().collect()
}

/// Result of the on-demand Telegram proxy check.
#[derive(Debug, Clone, Serialize)]
pub struct TgwsDiagnosis {
    pub binary: bool,
    /// What the supervisor thread believes; may lag behind reality.
    pub supervisor_running: bool,
    /// Whether the active engine's executable is actually in the process table.
    pub process_running: bool,
    pub port_open: bool,
    pub port_conflict: bool,
    pub clients: u64,
    /// Matches in the recent log (Cloudflare/WebSocket/timeout errors).
    pub log_errors: usize,
    /// The last warning/error line, shown as-is under "technical details".
    pub last_error_line: Option<String>,
    pub last_error: Option<String>,
    /// One actionable sentence in plain language.
    pub hint: String,
    /// `ok` | `binary` | `port` | `stopped` | `not_connected` | `blocked`.
    /// The list is exhaustive: the page keys its buttons on it.
    pub kind: String,
    /// Engine the numbers above belong to: `python` or `rs`.
    #[serde(default)]
    pub engine: String,
    /// The Rust engine's own `--check-listener` probe; `None` for the old
    /// engine, which has no such self-check.
    #[serde(default)]
    pub listener: Option<TgwsListenerCheck>,
}

/// One run of the Rust engine's `--check-listener` self-check.
#[derive(Debug, Clone, Serialize)]
pub struct TgwsListenerCheck {
    /// Whether the probe actually ran. It binds the proxy port, so a running
    /// proxy means "not now" rather than a failed check.
    pub ran: bool,
    pub ok: bool,
    /// One sentence for the card.
    pub summary: String,
    /// The probe's own last line, with the secret masked.
    pub detail: Option<String>,
}

impl TgwsListenerCheck {
    fn skipped(summary: impl Into<String>) -> Self {
        Self {
            ran: false,
            ok: false,
            summary: summary.into(),
            detail: None,
        }
    }
}

/// Checks the proxy end to end when the user asks for it.
///
/// The supervisor flag is not trusted on its own: a process that lost its child
/// handle keeps running and serving Telegram, so reality is read from the
/// process table and the log.
pub fn diagnose(app: &AppHandle) -> TgwsDiagnosis {
    let snapshot = status(app);
    let engine = ProxyEngine::from_config(&snapshot.engine);
    let supervisor_running = snapshot.state == TgwsState::Running;
    let process_running = crate::engine::image_running(engine.image());

    let (host, port) = {
        let state = app.state::<AppState>();
        let cfg = lock_config(&state);
        (cfg.tgws.host.clone(), cfg.tgws.port)
    };
    let port_open = port_reachable(&host, port);

    let recent: Vec<String> = logs(app).into_iter().rev().take(120).collect();
    let needles = [
        "cf proxy failed",
        "ws pool unavailable",
        "bad handshake",
        "i/o timeout",
        "connection reset",
        "no such host",
        "connection refused",
        "timeout",
    ];
    let mut log_errors = 0;
    let mut last_error_line = None;
    for line in &recent {
        let lower = line.to_ascii_lowercase();
        if needles.iter().any(|needle| lower.contains(needle)) {
            log_errors += 1;
            if last_error_line.is_none() {
                last_error_line = Some(line.trim().to_string());
            }
        }
    }

    let alive = process_running || supervisor_running;
    let clients = snapshot.client_connections_total;
    let (kind, hint) = if !snapshot.binary_present {
        (
            "binary",
            "Прокси не установлен. Скачайте его кнопкой «Обновить прокси».",
        )
    } else if !alive && snapshot.fallback_recommended {
        (
            "stopped",
            "Новый движок не запускается. Вернитесь на старый движок в «Дополнительно».",
        )
    } else if !alive {
        (
            "stopped",
            "Прокси не запущен. Включите тумблер; если не стартует — обновите прокси.",
        )
    } else if !port_open {
        (
            "port",
            "Прокси запущен, но его порт не отвечает. Попробуйте занять свободный порт или перезапустить прокси.",
        )
    } else if log_errors > 0 {
        (
            "blocked",
            "Прокси включён и Telegram к нему подключается, но дальше соединения не проходят. Похоже, провайдер блокирует серверы, через которые работает прокси. Обновление прокси может найти другие адреса; если не поможет — включите VPN.",
        )
    } else if clients == 0 {
        (
            "not_connected",
            "Прокси работает, но Telegram к нему не подключён. Откройте «Подключить Telegram».",
        )
    } else {
        ("ok", "Прокси работает и Telegram подключён.")
    };

    // The new engine ships its own end-to-end probe (R6). It binds the port
    // and talks to Telegram, so it runs only here, on the user's click — never
    // from the minute-by-minute health check.
    let listener = (engine == ProxyEngine::Rs).then(|| listener_check(app, &snapshot));

    TgwsDiagnosis {
        binary: snapshot.binary_present,
        supervisor_running,
        process_running,
        port_open,
        port_conflict: snapshot.port_conflict,
        clients,
        log_errors,
        last_error_line,
        last_error: snapshot.last_error,
        hint: hint.into(),
        kind: kind.into(),
        engine: engine.as_str().into(),
        listener,
    }
}

/// Output of a child process that was given a deadline.
struct RunOutput {
    code: Option<i32>,
    lines: Vec<String>,
    timed_out: bool,
}

/// Runs `cmd` with a hard deadline and captures its output.
///
/// `Child` has no timeout, so a poll loop plus `kill` is the only portable
/// way: a blocked network must not leave the diagnostics dialog spinning.
/// Both pipes are drained on their own threads, so a chatty child cannot block
/// on a full buffer while we wait for it.
fn run_with_timeout(mut cmd: Command, limit: Duration) -> Result<RunOutput, String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    sys::hidden(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Не удалось запустить проверку: {e}"))?;

    let mut streams: Vec<Box<dyn Read + Send>> = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        streams.push(Box::new(stdout));
    }
    if let Some(stderr) = child.stderr.take() {
        streams.push(Box::new(stderr));
    }

    let (sender, receiver) = mpsc::channel::<Vec<String>>();
    let readers = streams.len();
    for stream in streams {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let lines = BufReader::new(stream)
                .lines()
                .map_while(Result::ok)
                .collect::<Vec<_>>();
            let _ = sender.send(lines);
        });
    }
    drop(sender);

    let deadline = Instant::now() + limit;
    let mut timed_out = false;
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => break None,
        }
    };

    // The readers finish as soon as the pipes close; the bound below only
    // covers a reader that never does (a handle another process inherited).
    let drain_deadline = Instant::now() + Duration::from_secs(2);
    let mut lines = Vec::new();
    let mut collected = 0;
    while collected < readers && Instant::now() < drain_deadline {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(mut chunk) => {
                collected += 1;
                lines.append(&mut chunk);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(RunOutput {
        code,
        lines,
        timed_out,
    })
}

/// Runs the Rust engine's own end-to-end probe (`--check-listener`).
///
/// The probe binds the configured port and performs a real MTProto handshake
/// with Telegram, so it is only reachable from the user-triggered diagnostics
/// and is skipped while the proxy holds the port itself (R6).
fn listener_check(app: &AppHandle, snapshot: &TgwsStatus) -> TgwsListenerCheck {
    if !snapshot.binary_present {
        return TgwsListenerCheck::skipped("Новый движок не установлен — проверять нечего.");
    }
    if matches!(
        snapshot.state,
        TgwsState::Running | TgwsState::Starting | TgwsState::Stopping
    ) {
        return TgwsListenerCheck::skipped(
            "Прокси запущен: встроенная проверка порта работает, когда прокси остановлен.",
        );
    }

    let secret = match ensure_secret(app) {
        Ok(secret) => secret,
        Err(error) => return TgwsListenerCheck::skipped(error),
    };
    let cfg = load_cfg(app);
    let mut args = build_rs_args(&cfg, &secret);
    args.push("--check-listener".into());

    let mut cmd = Command::new(ProxyEngine::Rs.binary_path());
    cmd.args(&args).current_dir(config::tgws_dir());
    let output = match run_with_timeout(cmd, LISTENER_CHECK_TIMEOUT) {
        Ok(output) => output,
        Err(error) => return TgwsListenerCheck::skipped(error),
    };

    let detail = output
        .lines
        .iter()
        .map(|line| mask_secret(line.trim(), &secret))
        .rfind(|line| !line.is_empty());
    let ok = !output.timed_out && output.code == Some(0);
    let summary = if ok {
        "Самопроверка пройдена: порт и маршрут до Telegram работают."
    } else if output.timed_out {
        "Самопроверка не уложилась в отведённое время — похоже, сеть блокирует соединения."
    } else if output.code.is_none() {
        "Самопроверка завершилась без кода возврата — смотрите лог."
    } else {
        "Самопроверка не прошла: прокси не смог достучаться до Telegram."
    };
    TgwsListenerCheck {
        ran: true,
        ok,
        summary: summary.into(),
        detail,
    }
}

fn log_tail(app: &AppHandle, count: usize) -> String {
    let state = app.state::<AppState>();
    let logs = lock(&state.tgws.logs);
    let start = logs.len().saturating_sub(count);
    logs.iter()
        .skip(start)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------- secret ----------

pub fn is_valid_secret(secret: &str) -> bool {
    secret.len() == 32 && secret.chars().all(|c| c.is_ascii_hexdigit())
}

fn generate_secret() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("Не удалось сгенерировать ключ: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Standalone TgWsProxy settings, used only for the one-time migration.
struct StandaloneConfig {
    host: Option<String>,
    port: Option<u16>,
    secret: String,
}

/// Reads `%APPDATA%\TgWsProxy\config.json` written by a standalone
/// [Flowseal/tg-ws-proxy] install. When such an install exists, its port and
/// secret are already configured in Telegram, so inheriting them makes the
/// switch to the built-in proxy seamless.
fn read_standalone_config() -> Option<StandaloneConfig> {
    let path = dirs::config_dir()?.join("TgWsProxy").join("config.json");
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let secret = value.get("secret")?.as_str()?.trim().to_string();
    if !is_valid_secret(&secret) {
        return None;
    }
    Some(StandaloneConfig {
        host: value
            .get("host")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        port: value
            .get("port")
            .and_then(|v| v.as_u64())
            .and_then(|p| u16::try_from(p).ok()),
        secret,
    })
}

/// Returns the persisted secret, generating and saving one when missing or
/// malformed (a hand-edited config must not brick the proxy).
///
/// On the first run after installing zapret-ui, a standalone TgWsProxy config
/// is inherited instead of generating a fresh secret: Telegram already holds
/// that secret for `127.0.0.1:1443`, so the user does not have to re-add the
/// proxy after migrating.
pub fn ensure_secret(app: &AppHandle) -> Result<String, String> {
    let cfg = load_cfg(app);
    if cfg.secret_migrated {
        if is_valid_secret(&cfg.secret) {
            return Ok(cfg.secret);
        }
        let fresh = generate_secret()?;
        save_cfg(app, |c| c.secret = fresh.clone())?;
        return Ok(fresh);
    }

    if let Some(standalone) = read_standalone_config() {
        let secret = standalone.secret.clone();
        let host = standalone
            .host
            .as_deref()
            .and_then(|h| validate_host(h).ok());
        let port = standalone.port.filter(|p| (1024..=65535).contains(p));
        save_cfg(app, |c| {
            c.secret = secret.clone();
            if let Some(host) = host.clone() {
                c.host = host;
            }
            if let Some(port) = port {
                c.port = port;
            }
            c.secret_migrated = true;
        })?;
        emit_notice(
            app,
            true,
            "Ключ и адрес взяты из установленного TgWsProxy — ссылка в Telegram остаётся рабочей.",
        );
        return Ok(secret);
    }

    // Nothing to inherit: mark the migration done and keep/generate a secret.
    save_cfg(app, |c| c.secret_migrated = true)?;
    let current = load_cfg(app).secret;
    if is_valid_secret(&current) {
        return Ok(current);
    }
    let fresh = generate_secret()?;
    save_cfg(app, |c| c.secret = fresh.clone())?;
    Ok(fresh)
}

// ---------- link ----------

/// `tg://` link for Telegram Desktop.
///
/// A plain MTProto secret is prefixed with `dd`; when fake TLS is enabled the
/// upstream entry point expects `ee<secret><hex(domain)>` instead, and a `dd`
/// link would not connect. This mirrors `proxy/tg_ws_proxy.py` upstream.
pub fn link_for(host: &str, port: u16, secret: &str, fake_tls_domain: &str) -> String {
    let host = match host.trim() {
        "" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1".to_string(),
        other => other.to_string(),
    };
    let host = if host.parse::<Ipv6Addr>().is_ok() {
        format!("[{host}]")
    } else {
        host
    };

    let fake_tls = fake_tls_domain.trim();
    let secret = if fake_tls.is_empty() {
        format!("dd{secret}")
    } else {
        let domain_hex: String = fake_tls.bytes().map(|b| format!("{b:02x}")).collect();
        format!("ee{secret}{domain_hex}")
    };

    format!("tg://proxy?server={host}&port={port}&secret={secret}")
}

pub fn link(app: &AppHandle) -> Result<String, String> {
    let secret = ensure_secret(app)?;
    let cfg = load_cfg(app);
    Ok(link_for(&cfg.host, cfg.port, &secret, &cfg.fake_tls_domain))
}

/// The user opened the connection link manually; do not auto-open it again.
pub fn mark_link_opened(app: &AppHandle) -> Result<(), String> {
    save_cfg(app, |cfg| cfg.link_opened = true)
}

/// Replaces the secret with a fresh one and restarts a running proxy so the
/// new `tg://` link becomes valid immediately.
pub fn regenerate_secret(app: &AppHandle) -> Result<String, String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.tgws.op);
    let fresh = generate_secret()?;
    save_cfg(app, |cfg| {
        cfg.secret = fresh.clone();
        cfg.link_opened = false;
    })?;
    if status(app).state == TgwsState::Running {
        stop_locked(app)?;
        std::thread::sleep(Duration::from_millis(300));
        start_locked(app)?;
    }
    Ok(fresh)
}

/// Applies a settings change and restarts the proxy when it is running, so the
/// UI never shows values that the live process does not use. Runs under the
/// operation lock: a concurrent start must not read a half-applied config.
pub fn update_settings(
    app: &AppHandle,
    patch: impl FnOnce(&mut config::TgwsConfig),
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.tgws.op);
    save_cfg(app, |cfg| {
        patch(cfg);
        // A changed endpoint/secret invalidates the Telegram entry; allow the
        // one-time "open link" hint to fire again.
        cfg.link_opened = false;
    })?;
    if status(app).state == TgwsState::Running {
        stop_locked(app)?;
        std::thread::sleep(Duration::from_millis(300));
        start_locked(app)?;
    }
    Ok(())
}

// ---------- engine ----------

/// Applies a proxy setting that is not part of the endpoint (host, port,
/// secret) and restarts the proxy when it is running, so the live process never
/// disagrees with the config.
///
/// These used to be Rust-engine-only switches. Since 2.3.0 both engines
/// understand them — the old one takes domains, Worker domains, `--no-secure`,
/// `--force-test-dc` and `--proxy-protocol` too — so the restart is no longer
/// conditional on the engine. A change made while the proxy is stopped is just
/// saved and reaches the argv on the next start.
fn update_engine_settings(
    app: &AppHandle,
    patch: impl FnOnce(&mut config::TgwsConfig),
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.tgws.op);
    save_cfg(app, patch)?;
    if status(app).state == TgwsState::Running {
        stop_locked(app)?;
        std::thread::sleep(Duration::from_millis(300));
        start_locked(app)?;
    }
    Ok(())
}

/// Switches the engine (R1, R8, R9, R11).
///
/// The target binary is installed *before* the running proxy is touched, so a
/// failed download changes nothing at all. The secret is a separate setting and
/// is never written here: that is what keeps the Telegram entry working across
/// the switch, because the `tg://` link is `secret + host + port` only.
pub fn set_engine(app: &AppHandle, requested: &str) -> Result<String, String> {
    let engine = ProxyEngine::parse(requested)?;

    // Installing runs under its own operation lock, so it happens before this
    // one is taken — and a failed download leaves everything untouched.
    if !engine.binary_path().exists() {
        let installed = match engine {
            ProxyEngine::Python => install_latest(app).map(|_| ()),
            ProxyEngine::Rs => install_rust_engine(app).map(|_| ()),
        };
        if let Err(error) = installed {
            let prefix = match engine {
                ProxyEngine::Python => "Не удалось скачать старый движок.",
                ProxyEngine::Rs => "Не удалось скачать новый движок. Остаёмся на старом.",
            };
            return Err(format!("{prefix} {error}"));
        }
    }

    let state = app.state::<AppState>();
    let _guard = lock(&state.tgws.op);

    let previous = active_engine(app);
    if engine == previous {
        return Ok(engine.as_str().to_string());
    }

    save_cfg(app, |cfg| cfg.engine = engine.as_str().to_string())?;
    reset_start_failures(app);

    if status(app).state == TgwsState::Running {
        stop_locked(app)?;
        std::thread::sleep(Duration::from_millis(300));
        if let Err(error) = start_locked(app) {
            // The new engine refuses to start: put the old one back rather
            // than leave the user without a proxy. Switching again stays the
            // user's decision, which is why nothing is retried silently (R8).
            save_cfg(app, |cfg| cfg.engine = previous.as_str().to_string())?;
            reset_start_failures(app);
            let _ = start_locked(app);
            return Err(format!(
                "{} не запускается. Вернулись на старый движок. {error}",
                engine.label()
            ));
        }
    }
    Ok(engine.as_str().to_string())
}

/// Buffers and pool size edited from the Telegram page.
///
/// Validated once on save instead of in every argv builder, so a hand-edited
/// config cannot smuggle a value the engines would reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TgwsLimits {
    pub buf_kb: u32,
    pub pool_size: u32,
}

/// On/off switches of the Telegram page. Every field maps to a CLI flag of at
/// least one engine; `build_rs_args` says which ones the Rust engine drops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TgwsFlags {
    pub cfproxy: bool,
    pub h2: bool,
    pub verbose: bool,
    pub no_secure: bool,
    pub force_test_dc: bool,
    pub proxy_protocol: bool,
}

/// Saves the Cloudflare domain list and returns it as stored (R5).
pub fn set_domains(app: &AppHandle, domains: &[String]) -> Result<Vec<String>, String> {
    let domains = validate_domains(domains)?;
    let saved = domains.clone();
    // The 2.2.0 field is kept equal to the first entry: an older build reads
    // only it, and it must not resurrect a domain the user removed here.
    let legacy = domains.first().cloned().unwrap_or_default();
    update_engine_settings(app, move |cfg| {
        cfg.cf_domains = domains;
        cfg.cfproxy_user_domain = legacy;
    })?;
    Ok(saved)
}

/// Saves the Cloudflare Worker domain; an empty string means "not configured".
pub fn set_worker_domain(app: &AppHandle, domain: &str) -> Result<String, String> {
    let domain = validate_worker_domain(domain)?;
    let saved = domain.clone();
    let list = if domain.is_empty() {
        Vec::new()
    } else {
        vec![domain.clone()]
    };
    update_engine_settings(app, move |cfg| {
        cfg.cf_worker_domain = domain;
        cfg.cf_worker_domains = list;
    })?;
    Ok(saved)
}

/// Saves the Worker domain list (R5). The legacy single field follows the
/// first entry for the same reason as in [`set_domains`].
pub fn set_worker_domains(app: &AppHandle, domains: &[String]) -> Result<Vec<String>, String> {
    let domains = validate_domains(domains)?;
    let saved = domains.clone();
    let legacy = domains.first().cloned().unwrap_or_default();
    update_engine_settings(app, move |cfg| {
        cfg.cf_worker_domains = domains;
        cfg.cf_worker_domain = legacy;
    })?;
    Ok(saved)
}

/// Saves the `DC:IP` redirects (R5). An empty list means the engine defaults.
pub fn set_dc_ips(app: &AppHandle, ips: &[String]) -> Result<Vec<String>, String> {
    let ips = validate_dc_ips(ips)?;
    let saved = ips.clone();
    update_engine_settings(app, move |cfg| cfg.dc_ip = ips)?;
    Ok(saved)
}

/// Saves the buffer and pool size (R5).
pub fn set_limits(app: &AppHandle, limits: TgwsLimits) -> Result<TgwsLimits, String> {
    let limits = TgwsLimits {
        buf_kb: validate_buf_kb(limits.buf_kb)?,
        pool_size: validate_pool_size(limits.pool_size)?,
    };
    update_engine_settings(app, move |cfg| {
        cfg.buf_kb = limits.buf_kb;
        cfg.pool_size = limits.pool_size;
    })?;
    Ok(limits)
}

/// Saves the on/off switches (R5).
pub fn set_flags(app: &AppHandle, flags: TgwsFlags) -> Result<TgwsFlags, String> {
    update_engine_settings(app, move |cfg| {
        cfg.cfproxy = flags.cfproxy;
        cfg.h2 = flags.h2;
        cfg.verbose = flags.verbose;
        cfg.no_secure = flags.no_secure;
        cfg.force_test_dc = flags.force_test_dc;
        cfg.proxy_protocol = flags.proxy_protocol;
    })?;
    Ok(flags)
}

/// Turns round-robin over the domain list on or off.
pub fn set_balance(app: &AppHandle, enabled: bool) -> Result<bool, String> {
    update_engine_settings(app, move |cfg| cfg.cf_balance = enabled)?;
    Ok(enabled)
}

/// Inbound FakeTLS domain (R5). An empty value turns the camouflage off.
///
/// Both engines understand it, so this goes through the general settings path:
/// the running proxy is restarted and the one-time "open link" hint is armed
/// again, because the link switches between `dd` and `ee` (R4).
pub fn set_fake_tls_domain(app: &AppHandle, domain: &str) -> Result<String, String> {
    let domain = validate_worker_domain(domain)?;
    let saved = domain.clone();
    update_settings(app, move |cfg| cfg.fake_tls_domain = domain)?;
    Ok(saved)
}

/// Clears the failed-start counter after a deliberate action (a manual start,
/// a switch or a successful run), so the way back is judged on fresh evidence.
fn reset_start_failures(app: &AppHandle) {
    set_status(app, |s| {
        s.engine_start_failures = 0;
        s.fallback_recommended = false;
    });
}

/// Counts consecutive failed starts of the new engine (R8).
///
/// Deliberately only records: the old engine is never restored without the
/// user asking for it, because a young engine may fail once and then work.
fn record_start_result(app: &AppHandle, engine: ProxyEngine, ok: bool) {
    if !ok && engine != ProxyEngine::Rs {
        return;
    }
    let failures = if ok {
        0
    } else {
        let state = app.state::<AppState>();
        let status = lock(&state.tgws.status);
        status.engine_start_failures.saturating_add(1)
    };
    let recommended = fallback_recommended(engine, failures);
    set_status(app, |s| {
        s.engine_start_failures = failures;
        s.fallback_recommended = recommended;
    });
}

/// Pure part of R8: after [`RS_FAILED_STARTS`] failures in a row the page
/// should offer the old engine.
pub fn fallback_recommended(engine: ProxyEngine, failures: u32) -> bool {
    engine == ProxyEngine::Rs && failures >= RS_FAILED_STARTS
}

// ---------- validation ----------

/// A Cloudflare domain from the UI: a bare hostname.
///
/// Full URLs, paths, ports and IP literals all look plausible in a text field
/// and all fail later inside the proxy, where the message is much harder to
/// act on — so they are refused here with a concrete reason.
pub fn validate_domain(value: &str) -> Result<String, String> {
    let domain = value.trim().trim_end_matches('.').to_ascii_lowercase();
    if domain.is_empty() {
        return Err("Укажите домен, например example.com.".into());
    }
    if domain.contains("://") {
        return Err("Домен указывается без схемы: example.com, а не https://example.com.".into());
    }
    if domain.contains('/') || domain.contains('?') || domain.contains('#') || domain.contains(':')
    {
        return Err("Домен указывается без пути и порта: example.com.".into());
    }
    if domain.len() > 253 {
        return Err("Домен длиннее 253 символов.".into());
    }

    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return Err("Домен должен содержать точку: example.com.".into());
    }
    for label in &labels {
        let valid = !label.is_empty()
            && label.len() <= 63
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-');
        if !valid {
            return Err(format!("Некорректный домен: {domain}."));
        }
    }

    // An IP literal is not a Cloudflare domain: the proxy would build a
    // `kws2.{ip}` host name that can never resolve.
    let tld = labels[labels.len() - 1];
    if tld.chars().all(|c| c.is_ascii_digit()) {
        return Err("Это IP-адрес. Укажите домен, например example.com.".into());
    }
    Ok(domain)
}

/// Validates and normalises the domain list: the user's order is kept, blank
/// fields are dropped and duplicates are collapsed (R5).
pub fn validate_domains(values: &[String]) -> Result<Vec<String>, String> {
    let mut domains: Vec<String> = Vec::new();
    for value in values {
        if value.trim().is_empty() {
            continue;
        }
        let domain = validate_domain(value)?;
        if !domains.contains(&domain) {
            domains.push(domain);
        }
    }
    if domains.len() > MAX_CF_DOMAINS {
        return Err(format!("Не больше {MAX_CF_DOMAINS} доменов."));
    }
    Ok(domains)
}

/// Worker domain: the same rules, but an empty value means "not configured".
pub fn validate_worker_domain(value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        return Ok(String::new());
    }
    validate_domain(value)
}

/// One `DC:IP` redirect, e.g. `2:149.154.167.220` (R6).
///
/// The split is at the first colon only: an IPv6 literal brings its own colons
/// (`2:2001:db8::1`), and splitting from the right would eat the DC number.
pub fn validate_dc_redirect(value: &str) -> Result<String, String> {
    let value = value.trim();
    let (dc, ip) = value
        .split_once(':')
        .ok_or("Перенаправление задаётся как 2:149.154.167.220.")?;

    let dc: u16 = dc
        .trim()
        .parse()
        .map_err(|_| format!("Номер дата-центра должен быть числом от 1 до {MAX_DC_ID}."))?;
    if !(1..=MAX_DC_ID).contains(&dc) {
        return Err(format!(
            "Номер дата-центра должен быть от 1 до {MAX_DC_ID}."
        ));
    }

    let ip_text = ip.trim();
    if ip_text.is_empty() {
        return Err("После двоеточия нужен IP-адрес дата-центра.".into());
    }
    let ip: IpAddr = ip_text
        .parse()
        .map_err(|_| format!("Не удалось разобрать IP-адрес: {ip_text}."))?;
    Ok(format!("{dc}:{ip}"))
}

/// Validates and normalises the redirect list: order is kept, blanks dropped,
/// duplicates collapsed (R6).
pub fn validate_dc_ips(values: &[String]) -> Result<Vec<String>, String> {
    let mut ips: Vec<String> = Vec::new();
    for value in values {
        if value.trim().is_empty() {
            continue;
        }
        let ip = validate_dc_redirect(value)?;
        if !ips.contains(&ip) {
            ips.push(ip);
        }
    }
    if ips.len() > MAX_DC_REDIRECTS {
        return Err(format!(
            "Не больше {MAX_DC_REDIRECTS} перенаправлений дата-центров."
        ));
    }
    Ok(ips)
}

pub fn validate_buf_kb(value: u32) -> Result<u32, String> {
    if (MIN_BUF_KB..=MAX_BUF_KB).contains(&value) {
        Ok(value)
    } else {
        Err(format!(
            "Буфер должен быть от {MIN_BUF_KB} до {MAX_BUF_KB} КБ."
        ))
    }
}

pub fn validate_pool_size(value: u32) -> Result<u32, String> {
    if value <= MAX_POOL_SIZE {
        Ok(value)
    } else {
        Err(format!(
            "Пул соединений должен быть от 0 до {MAX_POOL_SIZE}."
        ))
    }
}

pub fn validate_host(host: &str) -> Result<String, String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("Укажите хост.".into());
    }
    if host.eq_ignore_ascii_case("localhost") {
        return Ok("localhost".into());
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.to_string())
        .map_err(|_| "Хост должен быть IP-адресом или localhost.".to_string())
}

pub fn validate_port(port: u16) -> Result<u16, String> {
    if (1024..=65535).contains(&port) {
        Ok(port)
    } else {
        Err("Порт должен быть в диапазоне 1024–65535.".into())
    }
}

pub fn validate_secret(secret: &str) -> Result<String, String> {
    let secret = secret.trim();
    if is_valid_secret(secret) {
        Ok(secret.to_ascii_lowercase())
    } else {
        Err("Ключ должен состоять из 32 шестнадцатеричных символов.".into())
    }
}

// ---------- argv ----------

/// Builds the CLI arguments for the headless upstream entry point. Kept pure so
/// the mapping from config to argv is unit-testable without spawning anything.
pub fn build_args(cfg: &config::TgwsConfig, secret: &str) -> Vec<String> {
    let mut args = vec![
        "--host".to_string(),
        cfg.host.clone(),
        "--port".to_string(),
        cfg.port.to_string(),
        "--secret".to_string(),
        secret.to_string(),
    ];

    for dc in &cfg.dc_ip {
        let dc = dc.trim();
        if !dc.is_empty() {
            args.push("--dc-ip".into());
            args.push(dc.to_string());
        }
    }

    args.push("--buf-kb".into());
    args.push(cfg.buf_kb.clamp(MIN_BUF_KB, MAX_BUF_KB).to_string());
    args.push("--pool-size".into());
    args.push(cfg.pool_size.to_string());

    if cfg.verbose {
        args.push("-v".into());
    }
    if !cfg.cfproxy {
        args.push("--no-cfproxy".into());
    }
    if !cfg.h2 {
        args.push("--no-h2".into());
    }

    // Every domain of the list reaches the old engine too (R3): the repeatable
    // flag is what makes a migrated 2.2.0 config behave the same on both
    // engines instead of silently using the first entry.
    for domain in engine_domains(cfg) {
        args.push("--cfproxy-domain".into());
        args.push(domain);
    }
    for worker in engine_worker_domains(cfg) {
        args.push("--cfproxy-worker-domain".into());
        args.push(worker);
    }

    if cfg.no_secure {
        args.push("--no-secure".into());
    }
    if cfg.force_test_dc {
        args.push("--force-test-dc".into());
    }
    if cfg.proxy_protocol {
        args.push("--proxy-protocol".into());
    }

    let fake_tls = cfg.fake_tls_domain.trim();
    if !fake_tls.is_empty() {
        args.push("--fake-tls-domain".into());
        args.push(fake_tls.to_string());
    }

    args
}

/// Cloudflare domains for either engine: the edited list, or the single legacy
/// field so a config written for the old engine keeps its domain after the
/// switch. Order is the user's: it is the priority order of the engine.
fn engine_domains(cfg: &config::TgwsConfig) -> Vec<String> {
    let mut domains: Vec<String> = cfg
        .cf_domains
        .iter()
        .map(|domain| domain.trim().to_ascii_lowercase())
        .filter(|domain| !domain.is_empty())
        .collect();
    if domains.is_empty() {
        let legacy = cfg.cfproxy_user_domain.trim().to_ascii_lowercase();
        if !legacy.is_empty() {
            domains.push(legacy);
        }
    }
    domains
}

/// Worker domains for either engine, with the same legacy fallback and the
/// same "user order is priority order" contract as [`engine_domains`].
fn engine_worker_domains(cfg: &config::TgwsConfig) -> Vec<String> {
    let mut workers: Vec<String> = Vec::new();
    for worker in &cfg.cf_worker_domains {
        let worker = worker.trim().to_ascii_lowercase();
        if !worker.is_empty() && !workers.contains(&worker) {
            workers.push(worker);
        }
    }
    if workers.is_empty() {
        let legacy = cfg.cf_worker_domain.trim().to_ascii_lowercase();
        if !legacy.is_empty() {
            workers.push(legacy);
        }
    }
    workers
}

/// Builds the CLI arguments of `tg-ws-proxy-rs` (R3, R4).
///
/// Only flags that engine knows are mapped from the shared settings: `--h2`,
/// `--no-cfproxy`, `--force-test-dc` and `--proxy-protocol` have no
/// counterpart (the Rust port always speaks WebSocket and has no test-DC or
/// PROXY-protocol switch), so they are dropped instead of being passed as
/// unknown arguments that would fail the start. `--no-secure` maps to
/// `--cf-disable-tls` with the caveat documented at its push site.
pub fn build_rs_args(cfg: &config::TgwsConfig, secret: &str) -> Vec<String> {
    let mut args = vec![
        "--host".to_string(),
        cfg.host.clone(),
        "--port".to_string(),
        cfg.port.to_string(),
        "--secret".to_string(),
        secret.to_string(),
        "--pool-size".to_string(),
        cfg.pool_size.to_string(),
    ];

    for dc in &cfg.dc_ip {
        let dc = dc.trim();
        if !dc.is_empty() {
            args.push("--dc-ip".into());
            args.push(dc.to_string());
        }
    }

    let fake_tls = cfg.fake_tls_domain.trim();
    if !fake_tls.is_empty() {
        args.push("--listen-faketls-domain".into());
        args.push(fake_tls.to_string());
    }

    // `cfproxy == false` means "no Cloudflare routing at all": the Rust engine
    // has no single switch for that, so neither the domains nor the built-in
    // list are passed and it stays on direct WebSocket and TCP.
    if cfg.cfproxy {
        let domains = engine_domains(cfg);
        if !domains.is_empty() {
            args.push("--cf-domain".into());
            args.push(domains.join(","));
        }
        // Repeatable in the Rust engine as well, so the list needs no
        // separator and the user's order is preserved (R4).
        for worker in engine_worker_domains(cfg) {
            args.push("--cf-worker-domain".into());
            args.push(worker);
        }
        // `--no-secure` has no exact counterpart: `--cf-disable-tls` switches
        // the Cloudflare proxy and Worker hops to plaintext `ws://:80`, while
        // direct Telegram WebSocket stays on TLS. It only makes sense with
        // Cloudflare routing on, which is why it lives inside this branch.
        if cfg.no_secure {
            args.push("--cf-disable-tls".into());
        }
        // Only when the user has no domains of their own: this is the Rust
        // engine's counterpart of the old engine's built-in Cloudflare list.
        // It fetches the list from GitHub at startup, which is why it is not
        // passed on top of a configured list.
        if domains.is_empty() {
            args.push("--default-domains".into());
        }
    }

    if cfg.cf_balance {
        args.push("--cf-balance".into());
    }
    if cfg.verbose {
        args.push("-v".into());
    }
    // `force_test_dc` and `proxy_protocol` have no Rust-engine flag: dropping
    // them keeps the argv valid instead of failing the start on an unknown
    // argument. The settings are shared, so the old engine still gets them.

    args
}

/// Arguments for whichever engine the config selects.
pub fn build_engine_args(
    engine: ProxyEngine,
    cfg: &config::TgwsConfig,
    secret: &str,
) -> Vec<String> {
    match engine {
        ProxyEngine::Python => build_args(cfg, secret),
        ProxyEngine::Rs => build_rs_args(cfg, secret),
    }
}

// ---------- process ----------

fn kill_image(engine: ProxyEngine) {
    kill_named_image(engine.image());
}

fn kill_named_image(image: &str) {
    let _ = sys::command("taskkill.exe")
        .args(["/F", "/IM", image])
        .status();
}

/// Terminates a proxy image without spawning `taskkill`.
///
/// Only used on the exit path, where a new process cannot start any more: the
/// session is shutting down and Windows fails the spawn with `0xc0000142`.
fn kill_image_without_spawning(image: &str) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
            return;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let own = std::process::id();
        let mut next = Process32FirstW(snapshot, &mut entry);
        while next != 0 {
            let name: Vec<u16> = entry
                .szExeFile
                .iter()
                .take_while(|unit| **unit != 0)
                .copied()
                .collect();
            if entry.th32ProcessID != own
                && String::from_utf16_lossy(&name).eq_ignore_ascii_case(image)
            {
                let process = OpenProcess(PROCESS_TERMINATE, 0, entry.th32ProcessID);
                if !process.is_null() {
                    TerminateProcess(process, 1);
                    CloseHandle(process);
                }
            }
            next = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
    }
}

/// PID of the process listening on `port`, parsed from `netstat -ano`.
fn port_owner_pid(port: u16) -> Option<u32> {
    let output = sys::command("netstat.exe").args(["-ano"]).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let suffix = format!(":{port}");
    for line in text.lines() {
        let columns: Vec<&str> = line.split_whitespace().collect();
        if columns.len() < 5 || columns[0] != "TCP" {
            continue;
        }
        if !columns[1].ends_with(&suffix) || columns[3] != "LISTENING" {
            continue;
        }
        if let Ok(pid) = columns[4].parse::<u32>() {
            return Some(pid);
        }
    }
    None
}

/// Image name for a PID, parsed from `tasklist` CSV output.
fn process_image(pid: u32) -> Option<String> {
    let filter = format!("PID eq {pid}");
    let output = sys::command("tasklist.exe")
        .args(["/FI", &filter, "/FO", "CSV", "/NH"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.lines().next()?.trim();
    let name = first.split(',').next()?.trim().trim_matches('"');
    if name.is_empty() || name.eq_ignore_ascii_case("INFO:") {
        None
    } else {
        Some(name.to_string())
    }
}

fn port_available(host: &str, port: u16) -> bool {
    TcpListener::bind((host, port)).is_ok()
}

/// Address of the proxy's own listener.
///
/// A wildcard bind (`0.0.0.0`, `::`) is reached through the loopback address,
/// and `localhost` is mapped by hand: the health check must never resolve a
/// name, because that would be a network request outside the machine.
fn probe_address(host: &str, port: u16) -> Option<SocketAddr> {
    let host = match host.trim() {
        "" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
        other if other.eq_ignore_ascii_case("localhost") => "127.0.0.1",
        other => other,
    };
    host.parse::<IpAddr>()
        .ok()
        .map(|ip| SocketAddr::new(ip, port))
}

/// Local TCP handshake with the proxy. No HTTP, no payload, no external
/// traffic: one connect and an immediate close.
fn port_reachable(host: &str, port: u16) -> bool {
    probe_address(host, port)
        .map(|address| TcpStream::connect_timeout(&address, HEALTH_TIMEOUT).is_ok())
        .unwrap_or(false)
}

/// First free port after `current`, looking at most `attempts` candidates ahead.
fn next_free_port(host: &str, current: u16, attempts: u16) -> Option<u16> {
    for offset in 1..=attempts {
        let Some(candidate) = current.checked_add(offset) else {
            break;
        };
        if !(1024..=65535).contains(&candidate) {
            continue;
        }
        if port_available(host, candidate) {
            return Some(candidate);
        }
    }
    None
}

fn ensure_port_free(
    app: &AppHandle,
    cfg: &config::TgwsConfig,
    engine: ProxyEngine,
) -> Result<(), String> {
    if port_available(&cfg.host, cfg.port) {
        return Ok(());
    }
    // A leftover proxy from a crash or a manual launch is the usual cause; it
    // is our own image, so killing it by name is safe. Only the engine we are
    // about to start is killed, so switching never fights the other one.
    kill_image(engine);
    std::thread::sleep(Duration::from_millis(600));
    if port_available(&cfg.host, cfg.port) {
        return Ok(());
    }

    // The standalone Flowseal tray app uses the same default port. Slipgate
    // solves this by killing it and taking the port; we do the same, but only
    // for that exact image. The secret was inherited from its config on the
    // first run, so the Telegram entry keeps working.
    let owner = port_owner_pid(cfg.port).and_then(process_image);
    if owner
        .as_deref()
        .map(|name| name.eq_ignore_ascii_case(STANDALONE_IMAGE))
        .unwrap_or(false)
    {
        kill_named_image(STANDALONE_IMAGE);
        std::thread::sleep(Duration::from_millis(800));
        if port_available(&cfg.host, cfg.port) {
            emit_notice(
                app,
                true,
                format!(
                    "Остановлен внешний TgWsProxy: порт {} теперь у встроенного прокси.",
                    cfg.port
                ),
            );
            return Ok(());
        }
    }

    set_status(app, |s| s.port_conflict = true);
    let owner_hint = owner.map(|name| format!(" ({name})")).unwrap_or_default();
    Err(mark_failed(
        app,
        format!(
            "Порт {}{} занят другим приложением. Занять соседний свободный порт или укажите его в параметрах.",
            cfg.port, owner_hint
        ),
    ))
}

/// Moves the proxy to the first free port after the current one and starts it.
/// Used by the one-click action next to the "port is busy" error.
pub fn switch_to_free_port(app: &AppHandle) -> Result<u16, String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.tgws.op);

    let cfg = load_cfg(app);
    let previous_port = cfg.port;
    let port = next_free_port(&cfg.host, cfg.port, 20)
        .ok_or("Рядом нет свободного порта — укажите его в параметрах.")?;
    let was_running = status(app).state == TgwsState::Running;

    save_cfg(app, |c| {
        c.port = port;
        c.link_opened = false;
    })?;
    // A running proxy still holds the old port; restart it so the config, the
    // link and the process agree.
    stop_locked(app)?;
    set_status(app, |s| s.port_conflict = false);
    if let Err(error) = start_locked(app) {
        // Never leave the config pointing at a port where nothing listens.
        let _ = save_cfg(app, |c| c.port = previous_port);
        if was_running {
            let _ = start_locked(app);
        }
        return Err(error);
    }
    Ok(port)
}

pub fn start(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.tgws.op);
    // An explicit start by the user earns a fresh restart budget: after five
    // failed attempts the toggle must remain usable.
    lock(&state.tgws.restarts).reset();
    start_locked(app)
}

pub fn stop(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.tgws.op);
    stop_locked(app)
}

pub fn restart(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = lock(&state.tgws.op);
    lock(&state.tgws.restarts).reset();
    stop_locked(app)?;
    std::thread::sleep(Duration::from_millis(300));
    start_locked(app)
}

fn start_locked(app: &AppHandle) -> Result<(), String> {
    if sys::session_ending() {
        return Err("Windows завершает сеанс — прокси запустится при следующем входе.".into());
    }
    // The engine is read once: a settings change during the start must not
    // leave the failure counter of one engine attached to the other (R8).
    let engine = active_engine(app);
    let result = start_engine(app, engine);
    record_start_result(app, engine, result.is_ok());
    result
}

fn start_engine(app: &AppHandle, engine: ProxyEngine) -> Result<(), String> {
    set_desired(app, true);
    {
        let state = app.state::<AppState>();
        if lock(&state.tgws.child).is_some() {
            return Ok(());
        }
    }

    let dir = config::tgws_dir();
    recover_binary(&dir, engine.image());
    let binary = engine.binary_path();
    if !binary.exists() {
        let message = match engine {
            ProxyEngine::Python => "Прокси ещё не скачан. Включите его, чтобы загрузить бинарник.",
            ProxyEngine::Rs => "Новый движок ещё не скачан. Включите прокси, чтобы загрузить его.",
        };
        return Err(mark_failed(app, message));
    }

    // Ensure the secret first: the one-time migration may also change the
    // host/port inherited from a standalone TgWsProxy, so reload the config.
    let secret = ensure_secret(app)?;
    let mut cfg = load_cfg(app);
    cfg.secret = secret;

    set_status(app, |s| {
        s.state = TgwsState::Starting;
        s.pid = None;
        s.last_error = None;
    });
    // The two engines share one log file; this line says which one wrote what
    // follows, so a support log is not ambiguous.
    push_supervisor_log(
        app,
        format!(
            "supervisor: запуск движка {} ({}).",
            engine.as_str(),
            engine.image()
        ),
    );

    ensure_port_free(app, &cfg, engine)?;

    let args = build_engine_args(engine, &cfg, &cfg.secret);
    let mut cmd = Command::new(&binary);
    cmd.args(&args)
        .current_dir(config::tgws_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    sys::hidden(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return Err(mark_failed(
                app,
                format!("Не удалось запустить прокси: {e}"),
            ))
        }
    };
    let pid = child.id();
    // The proxy is a child of the app: it must not survive the app, even when
    // the app is terminated without cleanup (see the `job` module).
    crate::job::adopt(&child);

    if let Some(stdout) = child.stdout.take() {
        spawn_reader(app.clone(), stdout, cfg.secret.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_reader(app.clone(), stderr, cfg.secret.clone());
    }

    {
        let state = app.state::<AppState>();
        *lock(&state.tgws.child) = Some(child);
    }

    std::thread::sleep(START_GRACE);

    let exited = {
        let state = app.state::<AppState>();
        let mut guard = lock(&state.tgws.child);
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
        *lock(&state.tgws.child) = None;
        // Kill our own process tree only: a manually started instance on
        // another port must survive a failed start of ours.
        let _ = sys::command("taskkill.exe")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .status();
        let tail = log_tail(app, 3);
        let message = if tail.is_empty() {
            format!("Прокси завершился сразу после запуска ({reason}).")
        } else {
            format!("Прокси завершился сразу после запуска ({reason}). {tail}")
        };
        return Err(mark_failed(app, message));
    }

    set_status(app, |s| {
        s.state = TgwsState::Running;
        s.pid = Some(pid);
        s.last_error = None;
        s.port_conflict = false;
        s.client_connections_total = 0;
        s.active_connections = None;
        s.last_client_at = None;
        s.last_traffic_at = None;
        s.restart_attempt = 0;
    });
    // Remember the state so the next launch can restore it.
    let _ = save_cfg(app, |cfg| cfg.session_running = true);
    let epoch = bump_epoch(app);
    spawn_watcher(app.clone(), pid);
    spawn_health_check(app.clone(), pid, epoch);
    schedule_connect_hint(app.clone());
    Ok(())
}

/// Telegram has no API to configure an MTProto proxy, so the best we can do is
/// open the `tg://` link once when the proxy runs but no client has connected:
/// Telegram then shows its "add proxy" confirmation, which is a single click.
fn schedule_connect_hint(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(20));
        let snapshot = status(&app);
        if snapshot.state != TgwsState::Running || snapshot.client_connections_total > 0 {
            return;
        }
        if load_cfg(&app).link_opened {
            return;
        }
        if let Ok(url) = link(&app) {
            if sys::open_path(std::path::Path::new(&url)).is_ok() {
                let _ = save_cfg(&app, |c| c.link_opened = true);
                emit_notice(
                    &app,
                    true,
                    "Открыл Telegram: подтвердите добавление прокси одним нажатием.",
                );
            }
        }
    });
}

/// Detects a proxy that dies on its own and hands it to the supervisor.
///
/// `Child` has no exit event, so a light poll is the only portable option; the
/// cost is one `try_wait` per half-second.
fn spawn_watcher(app: AppHandle, pid: u32) {
    std::thread::spawn(move || loop {
        std::thread::sleep(WATCH_INTERVAL);

        let exited = {
            let state = app.state::<AppState>();
            let mut guard = lock(&state.tgws.child);
            match guard.as_mut() {
                Some(child) if child.id() == pid => match child.try_wait() {
                    Ok(Some(status)) => {
                        guard.take();
                        Some(status.to_string())
                    }
                    // Still alive.
                    _ => None,
                },
                // The user stopped or restarted the proxy.
                _ => return,
            }
        };

        let Some(code) = exited else {
            continue;
        };
        let tail = log_tail(&app, 3);
        let reason = if tail.is_empty() {
            format!("Прокси завершился неожиданно ({code}).")
        } else {
            format!("Прокси завершился неожиданно ({code}). {tail}")
        };
        push_supervisor_log(&app, format!("supervisor: {reason}"));
        recover(&app, &reason);
        return;
    });
}

/// Timer of the local liveness check: one TCP connect to our own listener per
/// minute, three failures in a row mean the proxy is alive but not serving.
///
/// Cancellation is the epoch captured at start: a stop, a restart or a new
/// proxy instance bumps it and the timer exits on its next look. The probe is a
/// connect + close on the loopback address — no HTTP, no DNS, no payload, so
/// the ten-minute budget is far below 0.1% CPU and zero network bytes leave
/// the machine.
fn spawn_health_check(app: AppHandle, pid: u32, epoch: u64) {
    std::thread::spawn(move || {
        let mut failures = 0u8;
        loop {
            std::thread::sleep(HEALTH_INTERVAL);
            if !timer_is_current(&app, pid, epoch) {
                return;
            }
            let (host, port) = {
                let cfg = load_cfg(&app);
                (cfg.host, cfg.port)
            };
            if port_reachable(&host, port) {
                failures = 0;
                continue;
            }
            failures += 1;
            push_supervisor_log(
                &app,
                format!(
                    "health-check: {host}:{port} не отвечает ({failures} из {HEALTH_FAILURES})."
                ),
            );
            if failures < HEALTH_FAILURES {
                continue;
            }
            let reason = format!(
                "Прокси не отвечает на {host}:{port}: {HEALTH_FAILURES} проверки подряд не прошли."
            );
            push_supervisor_log(&app, format!("supervisor: {reason}"));
            recover(&app, &reason);
            return;
        }
    });
}

/// A timer belongs to exactly one proxy process: a new epoch, a different pid
/// or a proxy the user stopped all end it.
fn timer_is_current(app: &AppHandle, pid: u32, epoch: u64) -> bool {
    let state = app.state::<AppState>();
    if state.tgws.epoch.load(AtomicOrdering::SeqCst) != epoch || !desired(app) {
        return false;
    }
    let mut guard = lock(&state.tgws.child);
    match guard.as_mut() {
        Some(child) if child.id() == pid => matches!(child.try_wait(), Ok(None)),
        _ => false,
    }
}

/// Brings a dead (or wedged) proxy back, following [`RestartPolicy`].
///
/// Called by the exit watcher and by the health check. Returns as soon as the
/// proxy runs again, the user stopped it, the budget is spent or the session is
/// shutting down — there is no unguarded loop here and no restart after
/// `session_ending` (R8).
fn recover(app: &AppHandle, reason: &str) {
    let state = app.state::<AppState>();
    // The watcher and the health check can notice the same failure; one
    // recovery is enough, and two would fight over the child handle.
    if state.tgws.recovering.swap(true, AtomicOrdering::SeqCst) {
        return;
    }

    let mut cause = reason.to_string();
    loop {
        if sys::session_ending() {
            mark_failed(
                app,
                "Windows завершает сеанс — прокси запустится при следующем входе.",
            );
            break;
        }
        // A manual stop during the backoff cancels the restart (R4).
        if !desired(app) {
            break;
        }

        // The guard is dropped before the decision is acted upon: `give_up`
        // and `start_locked` touch other locks.
        let decision = lock(&state.tgws.restarts).next_attempt(now_secs());
        let (attempt, delay) = match decision {
            RestartDecision::GiveUp { attempts } => {
                give_up(app, attempts, &cause);
                break;
            }
            RestartDecision::Retry { attempt, delay } => (attempt, delay),
        };

        mark_restarting(app, attempt, delay, &cause);
        if !sleep_unless_stopped(app, delay) {
            break;
        }

        let running = {
            let _guard = lock(&state.tgws.op);
            if !desired(app) {
                break;
            }
            // A settings change or the user already started it meanwhile.
            if child_running(app) {
                break;
            }
            kill_child(app);
            match start_locked(app) {
                Ok(()) => true,
                Err(error) => {
                    cause = error;
                    false
                }
            }
        };

        if running {
            set_status(app, |s| s.restart_attempt = attempt);
            push_supervisor_log(
                app,
                format!(
                    "supervisor: прокси снова работает (попытка {attempt} из {}).",
                    RESTART_DELAYS_SECS.len()
                ),
            );
            notify_restart(app, attempt);
            break;
        }
    }

    state.tgws.recovering.store(false, AtomicOrdering::SeqCst);
}

/// Status while the supervisor waits out the backoff: `starting`, never
/// `error`, so the user sees "перезапускается", not "не запущен". The attempt
/// number is exposed for the "Дополнительно" section (R10).
fn mark_restarting(app: &AppHandle, attempt: u32, delay: Duration, cause: &str) {
    let message = format!(
        "Прокси перезапускается (попытка {attempt} из {}), через {} с. {cause}",
        RESTART_DELAYS_SECS.len(),
        delay.as_secs()
    );
    set_status(app, |s| {
        s.state = TgwsState::Starting;
        s.pid = None;
        s.restart_attempt = attempt;
        s.last_error = Some(message.clone());
        s.active_connections = None;
    });
    push_supervisor_log(app, format!("supervisor: {message}"));
}

/// The budget is spent: say why in plain language and stop trying (R3).
fn give_up(app: &AppHandle, attempts: u32, cause: &str) {
    set_desired(app, false);
    let snapshot = status(app);
    let hint = if !snapshot.binary_present {
        "Файл прокси не найден — обновите прокси."
    } else if snapshot.port_conflict {
        "Порт занят другим приложением — займите свободный порт."
    } else if snapshot.fallback_recommended {
        // R8: the young engine is offered, never forced back automatically.
        "Новый движок не запускается — вернитесь на старый в «Дополнительно»."
    } else {
        "Откройте лог или проверьте порт."
    };
    mark_failed(
        app,
        format!("Прокси не запускается после {attempts} попыток. {hint} {cause}"),
    );
    set_status(app, |s| s.restart_attempt = attempts);
    emit_notice(app, false, format!("Прокси не запускается. {hint}"));
    push_supervisor_log(
        app,
        format!("supervisor: восстановление остановлено после {attempts} попыток: {cause}"),
    );
}

/// "Прокси перезапущен" — at most one toast per quiet window, so a proxy that
/// keeps dying cannot flood the UI (R9).
fn notify_restart(app: &AppHandle, attempt: u32) {
    let now = now_secs();
    {
        let state = app.state::<AppState>();
        let mut last = lock(&state.tgws.restart_notice_at);
        if (*last).is_some_and(|at| now.saturating_sub(at) < RESTART_NOTICE_QUIET_SECS) {
            return;
        }
        *last = Some(now);
    }
    let message = if attempt <= 1 {
        "Прокси перезапущен.".to_string()
    } else {
        format!("Прокси перезапущен с попытки {attempt}.")
    };
    emit_notice(app, true, message);
}

/// Waits `total`, giving up early when the user stops the proxy or Windows
/// starts logging the session off. Cheap: one atomic load per quarter second,
/// and only while a restart is pending.
fn sleep_unless_stopped(app: &AppHandle, total: Duration) -> bool {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if !desired(app) || sys::session_ending() {
            return false;
        }
        std::thread::sleep(RESTART_POLL);
    }
    desired(app)
}

fn take_child(app: &AppHandle) -> Option<Child> {
    let state = app.state::<AppState>();
    let mut guard = lock(&state.tgws.child);
    guard.take()
}

/// Whether the supervisor currently owns a live child.
fn child_running(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let mut guard = lock(&state.tgws.child);
    match guard.as_mut() {
        Some(child) => matches!(child.try_wait(), Ok(None)),
        None => false,
    }
}

/// Drops and kills the current child without touching the user's intent; used
/// by recovery when the process is wedged but still present in the table.
fn kill_child(app: &AppHandle) {
    if let Some(mut child) = take_child(app) {
        terminate_child(&mut child);
    }
}

/// `taskkill` on our own process tree, then a bounded wait for the handle.
fn terminate_child(child: &mut Child) {
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
}

fn stop_locked(app: &AppHandle) -> Result<(), String> {
    // A manual stop cancels the restart: the intent is gone, the budget is
    // reset and the health timer of this instance is invalidated (R4).
    set_desired(app, false);
    bump_epoch(app);
    {
        let state = app.state::<AppState>();
        lock(&state.tgws.restarts).reset();
    }
    let _ = save_cfg(app, |cfg| cfg.session_running = false);

    let taken = take_child(app);
    let Some(mut child) = taken else {
        let installed = active_binary_present(app);
        set_status(app, |s| {
            s.state = if installed {
                TgwsState::Stopped
            } else {
                TgwsState::NotInstalled
            };
            s.pid = None;
            s.active_connections = None;
            s.restart_attempt = 0;
        });
        return Ok(());
    };

    set_status(app, |s| s.state = TgwsState::Stopping);
    terminate_child(&mut child);

    set_status(app, |s| {
        s.state = TgwsState::Stopped;
        s.pid = None;
        s.port_conflict = false;
        s.active_connections = None;
        s.restart_attempt = 0;
    });
    Ok(())
}

/// Best-effort synchronous stop used from the application exit hook.
///
/// Nothing is spawned here: Windows cannot start a new process while the
/// session is shutting down, which is exactly how the "taskkill.exe — Ошибка
/// приложения (0xc0000142)" dialog appeared on logout. The proxy and the
/// process it extracts both live in the app's job object, so a kernel call is
/// enough to stop them.
pub fn stop_on_exit(app: &AppHandle) {
    // No restart on the way out: the watcher checks this flag and gives up.
    set_desired(app, false);
    bump_epoch(app);
    let taken = take_child(app);
    let Some(mut child) = taken else {
        return;
    };
    crate::job::terminate_all();

    // Give the kernel a moment to reap it; the process is about to exit anyway.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    // The job could not be created or did not take the child (for example when
    // the app itself runs inside a restrictive job): stop what we own directly.
    let _ = child.kill();
    let _ = child.try_wait();
    // The bootloader extracts a second executable; without the job it would
    // keep running and hold the port.
    kill_image_without_spawning(active_engine(app).image());
}

// ---------- updates ----------

#[derive(Debug, Clone, Serialize)]
pub struct TgwsUpdateInfo {
    /// Version shown to the user (Flowseal), e.g. `1.11.4`.
    pub installed: String,
    pub latest: Option<String>,
    /// Exact mirror tag of the offered build (`1.11.4-2`). Used for installs
    /// and dismissal so a rebuild of the same upstream version is still
    /// distinguishable from what is already on disk.
    pub latest_tag: Option<String>,
    pub has_update: bool,
    pub asset_name: Option<String>,
    pub asset_url: Option<String>,
    pub asset_size: u64,
    pub digest: Option<String>,
    pub release_url: Option<String>,
    pub published_at: Option<String>,
    pub dismissed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedRelease {
    /// Repository the entry was resolved from; a cache written before the
    /// binary moved to its own repository must not be reused.
    #[serde(default)]
    repo: String,
    at: u64,
    latest: String,
    asset_name: Option<String>,
    asset_url: Option<String>,
    asset_size: u64,
    digest: Option<String>,
    release_url: Option<String>,
    published_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

/// Version of the upstream project behind a mirror tag: `1.11.4-2` → `1.11.4`.
/// The mirror appends a revision for rebuilds of the same upstream release; it
/// is an implementation detail, so the UI and the update check only use the
/// Flowseal version.
pub fn upstream_version(tag: &str) -> String {
    tag.trim()
        .trim_start_matches(['v', 'V'])
        .split('-')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Release notes of the upstream project for a mirror tag. `None` when the tag
/// carries no version at all.
pub fn upstream_release_url(tag: &str) -> Option<String> {
    let version = upstream_version(tag);
    (!version.is_empty())
        .then(|| format!("https://github.com/{UPSTREAM_REPO}/releases/tag/v{version}"))
}

/// Compares `1.11.2-1` style versions. Numeric parts compare numerically, a
/// numeric part beats a textual one, and a missing part counts as `0`.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    fn parts(value: &str) -> Vec<String> {
        value
            .trim_start_matches(['v', 'V'])
            .split(['.', '-', '+'])
            .map(|part| part.to_string())
            .collect()
    }

    let left = parts(a);
    let right = parts(b);
    for index in 0..left.len().max(right.len()) {
        let lv = left.get(index).map(String::as_str).unwrap_or("0");
        let rv = right.get(index).map(String::as_str).unwrap_or("0");
        let ordering = match (lv.parse::<u64>(), rv.parse::<u64>()) {
            (Ok(l), Ok(r)) => l.cmp(&r),
            (Ok(_), Err(_)) => Ordering::Greater,
            (Err(_), Ok(_)) => Ordering::Less,
            (Err(_), Err(_)) => lv.cmp(rv),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

/// Turns the latest release of the proxy repository into a cache entry. Returns
/// `None` when the release does not carry our asset (a hand-made or draft
/// release), so the UI simply reports "no update".
fn release_from(release: GhRelease) -> Option<CachedRelease> {
    let version = release.tag_name.trim().trim_start_matches('v').to_string();
    if version.is_empty() {
        return None;
    }
    let asset = release.assets.iter().find(|a| a.name == ASSET_NAME)?;
    Some(CachedRelease {
        repo: REPO.to_string(),
        at: now_secs(),
        latest: version,
        asset_name: Some(asset.name.clone()),
        asset_url: Some(asset.browser_download_url.clone()),
        asset_size: asset.size,
        digest: asset.digest.clone(),
        release_url: if release.html_url.is_empty() {
            None
        } else {
            Some(release.html_url)
        },
        published_at: release.published_at,
    })
}

fn cache_path() -> PathBuf {
    config::app_dir().join("tgws_update_cache.json")
}

fn hydrate_cache(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut hydrated = lock(&state.tgws.cache_hydrated);
    if *hydrated {
        return;
    }
    *hydrated = true;
    drop(hydrated);

    let Ok(text) = fs::read_to_string(cache_path()) else {
        return;
    };
    if let Ok(cached) = serde_json::from_str::<CachedRelease>(&text) {
        // A cache written when the binary lived in another repository (or an
        // older format) must not be trusted.
        if cached.repo == REPO {
            *lock(&state.tgws.cache) = Some(cached);
        }
    }
}

fn store_cache(app: &AppHandle, cached: &CachedRelease) {
    let state = app.state::<AppState>();
    *lock(&state.tgws.cache) = Some(cached.clone());
    if let Ok(text) = serde_json::to_string(cached) {
        let path = cache_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, text);
    }
}

fn fetch_latest_release(repo: &str) -> Result<Option<GhRelease>, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("zapret-ui/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("Не удалось проверить обновления прокси: {e}"))?;

    // No releases at all (fresh repository) is not an error for the user.
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("GitHub ответил кодом {}", response.status()));
    }
    response
        .json::<GhRelease>()
        .map(Some)
        .map_err(|e| format!("Некорректный ответ GitHub: {e}"))
}

fn info_from(cached: &CachedRelease, installed: &str, dismissed_tag: &str) -> TgwsUpdateInfo {
    // The mirror tag is the identity (`1.11.4-2`), the upstream version is what
    // the user sees (`1.11.4`). Updates are decided on the exact tags, so a
    // rebuild of the same Flowseal version is still offered.
    let installed_raw = installed.trim();
    let latest_raw = cached.latest.trim();
    let installed_up = upstream_version(installed_raw);
    let latest_up = upstream_version(latest_raw);
    let same_upstream = !installed_up.is_empty() && installed_up == latest_up;
    let shown = |tag: &str, upstream: &str| -> String {
        if upstream.is_empty() {
            return String::new();
        }
        match (same_upstream, tag.split_once('-')) {
            (true, Some((_, revision))) => format!("{upstream} · {revision}"),
            _ => upstream.to_string(),
        }
    };

    // Without a downloadable asset there is nothing to offer, even when the
    // version is newer: such a release is incomplete.
    let has_update = cached.asset_url.is_some()
        && !latest_raw.is_empty()
        && (installed_raw.is_empty()
            || compare_versions(latest_raw, installed_raw) == Ordering::Greater);
    TgwsUpdateInfo {
        installed: shown(installed_raw, &installed_up),
        latest: (!latest_raw.is_empty()).then(|| shown(latest_raw, &latest_up)),
        latest_tag: (!latest_raw.is_empty()).then(|| latest_raw.to_string()),
        has_update,
        asset_name: cached.asset_name.clone(),
        asset_url: cached.asset_url.clone(),
        asset_size: cached.asset_size,
        digest: cached.digest.clone(),
        release_url: upstream_release_url(latest_raw),
        published_at: cached.published_at.clone(),
        dismissed: !dismissed_tag.trim().is_empty() && dismissed_tag.trim() == latest_raw,
    }
}

pub fn check_update(app: &AppHandle, force: bool) -> Result<TgwsUpdateInfo, String> {
    hydrate_cache(app);

    let cfg = load_cfg(app);
    let cached = {
        let state = app.state::<AppState>();
        let guard = lock(&state.tgws.cache);
        guard.clone()
    };

    if let Some(cached) = cached {
        let fresh = now_secs().saturating_sub(cached.at) < CACHE_TTL_SECS;
        if fresh && !force {
            return Ok(info_from(
                &cached,
                &cfg.installed_version,
                &cfg.dismissed_update_tag,
            ));
        }
    }

    let fetched = fetch_latest_release(REPO)?.and_then(release_from);
    let Some(fetched) = fetched else {
        return Ok(TgwsUpdateInfo {
            installed: upstream_version(&cfg.installed_version),
            latest: None,
            latest_tag: None,
            has_update: false,
            asset_name: None,
            asset_url: None,
            asset_size: 0,
            digest: None,
            release_url: None,
            published_at: None,
            dismissed: false,
        });
    };

    store_cache(app, &fetched);
    Ok(info_from(
        &fetched,
        &cfg.installed_version,
        &cfg.dismissed_update_tag,
    ))
}

pub fn dismiss_update(app: &AppHandle, tag: &str) -> Result<(), String> {
    let tag = tag.trim();
    if tag.is_empty() {
        return Ok(());
    }
    save_cfg(app, |cfg| cfg.dismissed_update_tag = tag.to_string())
}

fn download(app: &AppHandle, url: &str) -> Result<Vec<u8>, String> {
    let result = download_inner(app, url);
    if let Err(error) = &result {
        crate::progress::fail(app, "tgws", "Обновление Telegram-прокси", error);
    }
    result
}

fn download_inner(app: &AppHandle, url: &str) -> Result<Vec<u8>, String> {
    if !url.starts_with("https://") {
        return Err("Некорректная ссылка на бинарник.".into());
    }
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("zapret-ui/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;

    let mut response = client
        .get(url)
        .send()
        .map_err(|e| format!("Не удалось скачать прокси: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Не удалось скачать прокси: {}", response.status()));
    }
    let total = response.content_length().unwrap_or(0);
    if total > MAX_DOWNLOAD_BYTES {
        return Err("Файл прокси подозрительно большой — загрузка отменена.".into());
    }

    let mut bytes = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut throttle = crate::progress::Throttle::new();
    crate::progress::update(app, "tgws", "Обновление Telegram-прокси", 0, total);
    loop {
        let read = response
            .read(&mut buffer)
            .map_err(|e| format!("Не удалось скачать прокси: {e}"))?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() as u64 > MAX_DOWNLOAD_BYTES {
            return Err("Файл прокси подозрительно большой — загрузка отменена.".into());
        }
        if throttle.ready() {
            crate::progress::update(
                app,
                "tgws",
                "Обновление Telegram-прокси",
                bytes.len() as u64,
                total,
            );
        }
    }
    if (bytes.len() as u64) < MIN_DOWNLOAD_BYTES {
        return Err(format!(
            "Загруженный файл слишком маленький ({} байт) — вероятно, это страница ошибки.",
            bytes.len()
        ));
    }
    crate::progress::finish(
        app,
        "tgws",
        "Обновление Telegram-прокси",
        bytes.len() as u64,
    );
    Ok(bytes)
}

/// Writes the downloaded binary next to the current one and swaps it in.
///
/// `tgws-cli.exe.new` is written first and only renamed into place after the
/// bytes are fully on disk, so a crash can never leave a truncated executable.
/// The previous binary is kept as `.old` until the swap succeeds; a startup
/// recovery restores it if the process dies between the two renames.
fn write_binary(dir: &Path, image: &str, bytes: &[u8]) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Не удалось создать папку прокси: {e}"))?;

    let binary = dir.join(image);
    let staged = dir.join(format!("{image}.new"));
    let backup = dir.join(format!("{image}.old"));

    let _ = fs::remove_file(&staged);
    fs::write(&staged, bytes).map_err(|e| format!("Не удалось записать бинарник: {e}"))?;
    let written = fs::metadata(&staged).map(|m| m.len()).unwrap_or(0);
    if written != bytes.len() as u64 {
        let _ = fs::remove_file(&staged);
        return Err("Записанный бинарник повреждён — обновление отменено.".into());
    }

    let _ = fs::remove_file(&backup);
    if binary.exists() {
        fs::rename(&binary, &backup).map_err(|e| format!("Не удалось заменить бинарник: {e}"))?;
    }
    if let Err(e) = fs::rename(&staged, &binary) {
        let _ = fs::remove_file(&staged);
        if backup.exists() {
            let _ = fs::rename(&backup, &binary);
        }
        return Err(format!("Не удалось заменить бинарник: {e}"));
    }
    let _ = fs::remove_file(&backup);
    Ok(())
}

/// Restores the previous binary when an update was interrupted after the old
/// one had been moved aside. Called before every start.
fn recover_binary(dir: &Path, image: &str) {
    let binary = dir.join(image);
    let backup = dir.join(format!("{image}.old"));
    if !binary.exists() && backup.exists() {
        let _ = fs::rename(&backup, &binary);
    }
    let _ = fs::remove_file(dir.join(format!("{image}.new")));
}

/// Event payload for the background auto-install of the proxy.
#[derive(Debug, Clone, Serialize)]
pub struct TgwsNotice {
    pub ok: bool,
    pub message: String,
}

fn emit_progress(app: &AppHandle, message: &str) {
    let _ = app.emit("tgws-progress", message);
}

fn emit_notice(app: &AppHandle, ok: bool, message: impl Into<String>) {
    let _ = app.emit(
        "tgws-notice",
        TgwsNotice {
            ok,
            message: message.into(),
        },
    );
}

/// Downloads the proxy of the *active* engine when it is missing. Used by the
/// autostart path so a fresh install becomes usable without visiting the
/// Telegram page first, and so a switch survives a deleted folder.
pub fn ensure_installed(app: &AppHandle) -> Result<(), String> {
    let engine = active_engine(app);
    if engine.binary_path().exists() {
        return Ok(());
    }
    let installed = match engine {
        ProxyEngine::Python => install_latest(app).map(|_| ()),
        ProxyEngine::Rs => install_rust_engine(app).map(|_| ()),
    };
    match installed {
        Ok(()) => {
            let message = match engine {
                ProxyEngine::Python => "Telegram-прокси скачан и готов к работе.".to_string(),
                ProxyEngine::Rs => "Новый движок скачан и готов к работе.".to_string(),
            };
            emit_notice(app, true, message);
            Ok(())
        }
        Err(error) => {
            emit_notice(
                app,
                false,
                format!("Не удалось скачать Telegram-прокси: {error}"),
            );
            Err(error)
        }
    }
}

/// Everything needed to install a release, resolved on the backend so the
/// renderer cannot point the updater at an arbitrary executable.
struct TgwsAsset {
    url: String,
    version: String,
    digest: String,
}

fn resolve_asset(app: &AppHandle) -> Result<TgwsAsset, String> {
    let info = check_update(app, false)?;
    let url = info
        .asset_url
        .ok_or("Релиз прокси ещё не опубликован — скачивание недоступно.")?;
    let version = info
        .latest_tag
        .ok_or("В релизе прокси не указана версия — обновление отменено.")?;
    let digest = info
        .digest
        .ok_or("В релизе нет SHA-256 — обновление отменено.")?;
    Ok(TgwsAsset {
        url,
        version,
        digest,
    })
}

/// Downloads and installs the newest published proxy release. Returns its size.
/// Emits `tgws-progress` for the UI and always clears it afterwards.
pub fn install_latest(app: &AppHandle) -> Result<u64, String> {
    let result = install_latest_inner(app);
    emit_progress(app, "");
    result
}

fn install_latest_inner(app: &AppHandle) -> Result<u64, String> {
    let asset = resolve_asset(app)?;
    let expected = asset
        .digest
        .trim()
        .strip_prefix("sha256:")
        .ok_or("Релиз не содержит SHA-256 — обновление отменено.")?
        .to_string();

    let state = app.state::<AppState>();
    let _guard = lock(&state.tgws.op);

    // A concurrent auto-install may have completed while we waited for the lock.
    if config::tgws_binary_path().exists()
        && load_cfg(app).installed_version.trim() == asset.version.trim()
    {
        return Ok(0);
    }

    // Only a python run is bounced for a python update: with the new engine
    // selected the old binary is refreshed on disk without touching the live
    // process (R11).
    let was_running =
        active_engine(app) == ProxyEngine::Python && status(app).state == TgwsState::Running;

    if was_running {
        stop_locked(app)?;
    }
    // Windows keeps a write lock on the running image even after taskkill
    // returns; kill by name and give the OS a moment.
    kill_image(ProxyEngine::Python);
    std::thread::sleep(Duration::from_millis(300));

    recover_binary(&config::tgws_dir(), BINARY_IMAGE);
    emit_progress(app, "Скачивание Telegram-прокси…");
    let bytes = download(app, &asset.url)?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(expected.trim()) {
        return Err("Контрольная сумма бинарника не совпала — обновление отменено.".into());
    }

    emit_progress(app, "Установка…");
    write_binary(&config::tgws_dir(), BINARY_IMAGE, &bytes)?;

    save_cfg(app, |cfg| {
        cfg.installed_version = asset.version.clone();
        cfg.dismissed_update_tag = String::new();
    })?;

    *lock(&state.tgws.cache) = None;
    let _ = fs::remove_file(cache_path());

    // With the other engine live, its `running` state is the truth and must
    // not be overwritten by an old-engine file refresh.
    if status(app).state != TgwsState::Running {
        set_status(app, |s| {
            s.state = TgwsState::Stopped;
            s.pid = None;
            s.last_error = None;
            s.binary_present = true;
            s.port_conflict = false;
        });
    }

    if was_running {
        start_locked(app)?;
    }
    Ok(bytes.len() as u64)
}

// ---------- the Rust engine ----------

/// One downloadable build of the Rust engine, resolved from a release.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RsAsset {
    name: String,
    url: String,
    /// `sha256:<hex>` exactly as GitHub publishes it. Required (R2).
    digest: String,
    size: u64,
}

/// Picks the Windows x64 archive of a release (R2).
///
/// Pure, so the choice — and every reason to refuse — is testable without the
/// network. A release that carries none of [`RS_ASSETS`], or carries one
/// without a SHA-256 digest, is not installable: the user gets a sentence, not
/// an unverified executable.
fn pick_rs_asset(assets: &[GhAsset]) -> Result<RsAsset, String> {
    let Some(asset) = RS_ASSETS
        .iter()
        .find_map(|name| assets.iter().find(|asset| asset.name == *name))
    else {
        return Err(
            "В релизе нового движка нет сборки для Windows x64 — установка отменена.".into(),
        );
    };
    if !asset.browser_download_url.starts_with("https://") {
        return Err(
            "Ссылка на новый движок должна начинаться с https — установка отменена.".into(),
        );
    }
    let digest = asset
        .digest
        .as_deref()
        .map(str::trim)
        .filter(|digest| digest.starts_with("sha256:"))
        .ok_or("В релизе нового движка нет SHA-256 — установка отменена.")?;
    Ok(RsAsset {
        name: asset.name.clone(),
        url: asset.browser_download_url.clone(),
        digest: digest.to_string(),
        size: asset.size,
    })
}

/// Reads `tg-ws-proxy.exe` out of the release archive.
///
/// Only the executable is taken: the archive is never unpacked onto disk, so a
/// hostile entry cannot escape anywhere, and every other file is ignored.
fn extract_exe_from_zip(bytes: &[u8], exe_name: &str) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Архив нового движка повреждён: {e}"))?;

    let mut index = None;
    for position in 0..archive.len() {
        let entry = archive
            .by_index(position)
            .map_err(|e| format!("Архив нового движка повреждён: {e}"))?;
        let file_name = entry
            .enclosed_name()
            .and_then(|path| path.file_name().map(|name| name.to_owned()));
        if let Some(name) = file_name {
            if name.to_string_lossy().eq_ignore_ascii_case(exe_name) {
                index = Some(position);
                break;
            }
        }
    }

    let Some(index) = index else {
        return Err(format!(
            "В архиве нового движка нет {exe_name} — установка отменена."
        ));
    };
    let mut entry = archive
        .by_index(index)
        .map_err(|e| format!("Архив нового движка повреждён: {e}"))?;
    if entry.size() > MAX_DOWNLOAD_BYTES {
        return Err("Файл нового движка подозрительно большой — установка отменена.".into());
    }
    let mut exe = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut exe)
        .map_err(|e| format!("Не удалось прочитать архив нового движка: {e}"))?;
    if (exe.len() as u64) < MIN_EXE_BYTES {
        return Err("Файл нового движка слишком маленький — установка отменена.".into());
    }
    Ok(exe)
}

/// Downloads and installs the newest release of the Rust engine (R2).
///
/// The same rules as the old engine's updater: https only, a required SHA-256
/// published by GitHub, a size limit, and `.new` → rename so a crash cannot
/// leave a truncated executable in place.
pub fn install_rust_engine(app: &AppHandle) -> Result<u64, String> {
    let result = install_rust_engine_inner(app);
    emit_progress(app, "");
    result
}

fn install_rust_engine_inner(app: &AppHandle) -> Result<u64, String> {
    emit_progress(app, "Проверка версии нового движка…");
    let release = fetch_latest_release(RS_REPO)?
        .ok_or("Релиз нового движка ещё не опубликован — установка отменена.")?;
    let asset = pick_rs_asset(&release.assets)?;
    let version = release
        .tag_name
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_string();
    // The declared size is checked before a byte is transferred: the download
    // loop enforces its own limit, this one refuses obviously wrong assets.
    if asset.size > MAX_DOWNLOAD_BYTES {
        return Err("Файл нового движка подозрительно большой — установка отменена.".into());
    }

    let expected = asset
        .digest
        .strip_prefix("sha256:")
        .ok_or("Релиз нового движка не содержит SHA-256 — установка отменена.")?
        .to_string();

    let dir = config::tgws_dir();
    emit_progress(app, &format!("Скачивание нового движка ({})…", asset.name));
    let bytes = download(app, &asset.url)?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(expected.trim()) {
        return Err("Контрольная сумма нового движка не совпала — установка отменена.".into());
    }

    emit_progress(app, "Установка нового движка…");
    let exe = extract_exe_from_zip(&bytes, RS_ARCHIVE_EXE)?;

    let state = app.state::<AppState>();
    let _guard = lock(&state.tgws.op);
    // Only a *running* instance of this engine is stopped, and it is not
    // restarted here: the caller (a switch or a manual start) decides what runs
    // next, so installing never changes the user's engine behind their back.
    let was_running =
        active_engine(app) == ProxyEngine::Rs && status(app).state == TgwsState::Running;
    if was_running {
        stop_locked(app)?;
    }
    // Windows keeps a write lock on a running image even after taskkill, so a
    // leftover of this engine is stopped before the swap.
    kill_image(ProxyEngine::Rs);
    std::thread::sleep(Duration::from_millis(300));
    recover_binary(&dir, RS_IMAGE);
    write_binary(&dir, RS_IMAGE, &exe)?;

    save_cfg(app, |cfg| cfg.rs_version = version)?;
    let binary_present = active_engine(app).binary_path().exists();
    set_status(app, |s| {
        s.rust_binary_present = true;
        s.binary_present = binary_present;
        s.last_error = None;
    });

    let installed = exe.len() as u64;
    push_supervisor_log(
        app,
        format!(
            "supervisor: новый движок установлен ({}, {installed} байт).",
            asset.name
        ),
    );
    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> config::TgwsConfig {
        config::TgwsConfig::default()
    }

    #[test]
    fn args_contain_required_flags() {
        let secret = "00112233445566778899aabbccddeeff";
        let args = build_args(&cfg(), secret);
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--host" && w[1] == "127.0.0.1"));
        assert!(args.windows(2).any(|w| w[0] == "--port" && w[1] == "1555"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--secret" && w[1] == secret));
        assert_eq!(args.iter().filter(|a| *a == "--dc-ip").count(), 2);
        assert!(args.windows(2).any(|w| w[0] == "--buf-kb" && w[1] == "256"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--pool-size" && w[1] == "4"));
        assert!(!args.iter().any(|a| a == "--no-cfproxy"));
        assert!(!args.iter().any(|a| a == "-v"));
    }

    #[test]
    fn args_follow_advanced_config() {
        let mut cfg = cfg();
        cfg.cfproxy = false;
        cfg.h2 = false;
        cfg.verbose = true;
        cfg.buf_kb = 1; // clamped to the upstream minimum of 4
        cfg.pool_size = 0;
        cfg.dc_ip = vec![" ".into()];
        cfg.cfproxy_user_domain = " example.com ".into();
        cfg.fake_tls_domain = "tls.example.com".into();

        let args = build_args(&cfg, "aa");
        assert!(args.iter().any(|a| a == "--no-cfproxy"));
        assert!(args.iter().any(|a| a == "--no-h2"));
        assert!(args.iter().any(|a| a == "-v"));
        assert!(args.windows(2).any(|w| w[0] == "--buf-kb" && w[1] == "4"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--pool-size" && w[1] == "0"));
        assert!(!args.iter().any(|a| a == "--dc-ip"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--cfproxy-domain" && w[1] == "example.com"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--fake-tls-domain" && w[1] == "tls.example.com"));
    }

    #[test]
    fn link_uses_fake_tls_secret_and_loopback() {
        let link = link_for("127.0.0.1", 1443, "00112233445566778899aabbccddeeff", "");
        assert_eq!(
            link,
            "tg://proxy?server=127.0.0.1&port=1443&secret=dd00112233445566778899aabbccddeeff"
        );
        assert!(link_for("0.0.0.0", 1443, "aa", "").contains("server=127.0.0.1"));
        assert!(link_for("::", 1443, "aa", "").contains("server=127.0.0.1"));
    }

    #[test]
    fn link_switches_to_ee_with_fake_tls_domain() {
        let link = link_for(
            "127.0.0.1",
            1443,
            "00112233445566778899aabbccddeeff",
            "example.com",
        );
        assert_eq!(
            link,
            "tg://proxy?server=127.0.0.1&port=1443&secret=ee00112233445566778899aabbccddeeff6578616d706c652e636f6d"
        );
        assert!(!link.contains("secret=dd"));
    }

    #[test]
    fn link_brackets_ipv6_hosts() {
        let link = link_for("::1", 1443, "aa", "");
        assert!(
            link.starts_with("tg://proxy?server=[::1]&port=1443"),
            "{link}"
        );
    }

    #[test]
    fn secret_validation_rejects_bad_input() {
        assert!(is_valid_secret("00112233445566778899aabbccddeeff"));
        assert!(is_valid_secret("00112233445566778899AABBCCDDEEFF"));
        assert!(!is_valid_secret("00112233445566778899aabbccddeef"));
        assert!(!is_valid_secret("zz112233445566778899aabbccddeeff"));
        assert_eq!(
            validate_secret(" 00112233445566778899AABBCCDDEEFF ").unwrap(),
            "00112233445566778899aabbccddeeff"
        );
        assert!(validate_secret("nope").is_err());
    }

    #[test]
    fn stats_line_yields_client_count() {
        let line = "00:14:38  INFO   stats: total=7 active=1 ws=1 h2=0 bad=0";
        assert_eq!(
            parse_stats(line),
            Some(StatsLine {
                total: 7,
                active: Some(1)
            })
        );
        // Zero clients is a normal state, not a missing value.
        assert_eq!(
            parse_stats("stats: total=0 active=0 ws=0"),
            Some(StatsLine {
                total: 0,
                active: Some(0)
            })
        );
        // `active=` appeared later upstream: a line without it still counts.
        assert_eq!(
            parse_stats("stats: total=3 ws=1"),
            Some(StatsLine {
                total: 3,
                active: None
            })
        );
        assert_eq!(parse_stats("no stats here"), None);
        assert_eq!(parse_stats("stats: active=1"), None);
        assert_eq!(parse_stats("stats: total="), None);
    }

    #[test]
    fn connection_events_are_recognised() {
        assert!(is_client_connected_event(
            "01:17:07  INFO   [127.0.0.1:5013] DC2 -> WS pool hit"
        ));
        assert!(is_client_connected_event(
            "01:17:07  INFO   [127.0.0.1:5008] DC203 -> trying CF proxy"
        ));
        assert!(!is_client_connected_event(
            "01:17:07  WARNING  [127.0.0.1:5009] bad handshake (wrong secret or proto)"
        ));
        assert!(!is_client_connected_event("stats: total=0 active=0"));
    }

    #[test]
    fn log_lines_are_classified_by_what_they_prove() {
        assert_eq!(
            classify_line("stats: total=1 active=1 ws=1"),
            LogEvent::Stats(StatsLine {
                total: 1,
                active: Some(1)
            })
        );
        assert_eq!(
            classify_line("01:17:07  INFO   [127.0.0.1:5013] DC2 -> WS pool hit"),
            LogEvent::Traffic
        );
        assert_eq!(
            classify_line("01:17:07  WARNING  [127.0.0.1:5009] bad handshake"),
            LogEvent::Other
        );
        assert_eq!(
            classify_line("supervisor: прокси снова работает"),
            LogEvent::Other
        );
    }

    #[test]
    fn liveness_probe_only_touches_the_local_listener() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let port = listener.local_addr().expect("addr").port();
        assert!(port_reachable("127.0.0.1", port));
        // A wildcard listener is reached through the loopback address.
        assert!(port_reachable("0.0.0.0", port));
        assert!(port_reachable("localhost", port));
        drop(listener);
        assert!(!port_reachable("127.0.0.1", port));
    }

    #[test]
    fn probe_addresses_never_need_a_name_lookup() {
        assert_eq!(
            probe_address("localhost", 1555).map(|address| address.to_string()),
            Some("127.0.0.1:1555".to_string())
        );
        assert_eq!(
            probe_address("0.0.0.0", 1555).map(|address| address.to_string()),
            Some("127.0.0.1:1555".to_string())
        );
        // Anything that is not an IP would need DNS, so it is refused: the
        // health check must never send a packet outside the machine.
        assert_eq!(probe_address("example.com", 1555), None);
    }

    #[test]
    fn status_carries_the_activity_fields_the_ui_reads() {
        let status = TgwsStatus {
            state: TgwsState::Running,
            pid: Some(42),
            last_error: None,
            installed_version: "1.11.4".into(),
            binary_present: true,
            port_conflict: false,
            client_connections_total: 3,
            active_connections: Some(2),
            last_client_at: Some(1_700_000_000),
            last_traffic_at: Some(1_699_999_990),
            restart_attempt: 1,
            engine: "rs".into(),
            engine_start_failures: 2,
            fallback_recommended: false,
            rust_binary_present: true,
        };
        let json = serde_json::to_value(&status).expect("json");
        assert_eq!(json["active_connections"], 2);
        assert_eq!(json["last_client_at"], 1_700_000_000u64);
        assert_eq!(json["last_traffic_at"], 1_699_999_990u64);
        assert_eq!(json["restart_attempt"], 1);
        assert_eq!(json["engine"], "rs");
        assert_eq!(json["engine_start_failures"], 2);
        assert_eq!(json["rust_binary_present"], true);
        // The old fields must keep their names and types.
        assert_eq!(json["client_connections_total"], 3);
        assert_eq!(json["state"], "running");
    }

    #[test]
    fn host_and_port_validation() {
        assert_eq!(validate_host("127.0.0.1").unwrap(), "127.0.0.1");
        assert_eq!(validate_host("LocalHost").unwrap(), "localhost");
        assert!(validate_host("example.com").is_err());
        assert!(validate_host(" ").is_err());
        assert!(validate_port(1443).is_ok());
        assert!(validate_port(80).is_err());
        assert!(validate_port(0).is_err());
    }

    #[test]
    fn version_comparison() {
        use Ordering::*;
        assert_eq!(compare_versions("1.11.2", "1.11.2"), Equal);
        assert_eq!(compare_versions("1.11.2-1", "1.11.2"), Greater);
        assert_eq!(compare_versions("1.11.2", "1.11.10"), Less);
        assert_eq!(compare_versions("v1.11.2", "1.11.2"), Equal);
        assert_eq!(compare_versions("1.11.2-2", "1.11.2-10"), Less);
    }

    #[test]
    fn secret_is_masked_in_logs() {
        let secret = "00112233445566778899aabbccddeeff";
        let line = format!("config secret={secret} port=1443");
        let masked = mask_secret(&line, secret);
        assert!(!masked.contains(secret));
        assert!(masked.contains("***"));
        assert_eq!(mask_secret("nothing", ""), "nothing");
    }

    #[test]
    fn release_is_parsed_only_with_our_asset() {
        let release = |tag: &str, asset: &str, url: &str| GhRelease {
            tag_name: tag.into(),
            html_url: format!("https://github.com/x/y/releases/tag/{tag}"),
            published_at: None,
            assets: vec![GhAsset {
                name: asset.into(),
                browser_download_url: url.into(),
                size: 15_000_000,
                digest: Some("sha256:abc".into()),
            }],
        };

        let picked = release_from(release(
            "v1.11.2-1",
            ASSET_NAME,
            "https://github.com/tgws-cli.exe",
        ))
        .expect("release");
        assert_eq!(picked.latest, "1.11.2-1");
        assert_eq!(picked.repo, REPO);
        assert_eq!(
            picked.asset_url.as_deref(),
            Some("https://github.com/tgws-cli.exe")
        );
        assert_eq!(picked.digest.as_deref(), Some("sha256:abc"));

        // A release without our asset is not an update we can install.
        assert!(release_from(release("v1.11.2-1", "readme.txt", "https://x")).is_none());
        // An empty tag is never trusted.
        assert!(release_from(release("", ASSET_NAME, "https://x")).is_none());
    }

    #[test]
    fn update_is_offered_when_installed_version_is_unknown() {
        let cached = CachedRelease {
            repo: REPO.to_string(),
            at: now_secs(),
            latest: "1.11.4-1".into(),
            asset_name: Some(ASSET_NAME.into()),
            asset_url: Some("https://github.com/a.exe".into()),
            asset_size: 15_000_000,
            digest: Some("sha256:abc".into()),
            release_url: None,
            published_at: None,
        };
        assert!(info_from(&cached, "", "").has_update);
        assert!(!info_from(&cached, "1.11.4-1", "").has_update);
        assert!(info_from(&cached, "1.11.2-1", "").has_update);
        // A rebuild of the same upstream version is still a different binary.
        assert!(!info_from(&cached, "1.11.4-2", "").has_update);
        assert!(info_from(&cached, "1.11.4-1", "1.11.4-1").dismissed);
        assert!(!info_from(&cached, "1.11.4-1", "1.11.4").dismissed);
        assert!(!info_from(&cached, "1.11.4-1", "1.11.2-1").dismissed);

        // `1.11.4-2` offered over an installed `1.11.4-1` is a real update.
        let rebuilt = CachedRelease {
            latest: "1.11.4-2".into(),
            ..cached.clone()
        };
        assert!(info_from(&rebuilt, "1.11.4-1", "").has_update);
    }

    #[test]
    fn the_upstream_version_is_what_the_user_sees() {
        let cached = CachedRelease {
            repo: REPO.to_string(),
            at: now_secs(),
            latest: "1.11.4-2".into(),
            asset_name: Some(ASSET_NAME.into()),
            asset_url: Some("https://github.com/a.exe".into()),
            asset_size: 15_000_000,
            digest: None,
            release_url: Some("https://github.com/Dezuron/tgws-cli/releases/tag/v1.11.4-2".into()),
            published_at: None,
        };
        let info = info_from(&cached, "1.11.2-1", "");
        assert_eq!(info.installed, "1.11.2");
        assert_eq!(info.latest.as_deref(), Some("1.11.4"));
        // Never point the user at our mirror repository.
        assert_eq!(
            info.release_url.as_deref(),
            Some("https://github.com/Flowseal/tg-ws-proxy/releases/tag/v1.11.4")
        );

        assert_eq!(upstream_version("v1.11.4-1"), "1.11.4");
        assert_eq!(upstream_version("1.11.4"), "1.11.4");
        assert_eq!(upstream_version("  "), "");
        assert_eq!(upstream_release_url(""), None);

        // Two builds of the same Flowseal release are told apart by revision.
        let rebuilt = CachedRelease {
            latest: "1.11.4-2".into(),
            ..cached
        };
        let same = info_from(&rebuilt, "1.11.4-1", "");
        assert_eq!(same.installed, "1.11.4 · 1");
        assert_eq!(same.latest.as_deref(), Some("1.11.4 · 2"));
        assert_eq!(same.latest_tag.as_deref(), Some("1.11.4-2"));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("zapret-ui-tgws-{name}-{unique}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn next_free_port_skips_a_busy_candidate() {
        let busy = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let port = busy.local_addr().expect("addr").port();
        // Pretend the busy port is the current one, so the first candidate is
        // exactly that occupied port.
        let chosen = next_free_port("127.0.0.1", port.saturating_sub(1), 20).expect("free port");
        assert!(chosen > port, "chosen {chosen} must be after busy {port}");
        assert!(port_available("127.0.0.1", chosen));
    }

    #[test]
    fn binary_swap_replaces_the_file_without_leaving_a_backup() {
        let dir = temp_dir("swap");
        fs::write(dir.join(BINARY_IMAGE), b"old").unwrap();

        write_binary(&dir, BINARY_IMAGE, b"new-binary").unwrap();

        assert_eq!(fs::read(dir.join(BINARY_IMAGE)).unwrap(), b"new-binary");
        assert!(!dir.join(format!("{BINARY_IMAGE}.old")).exists());
        assert!(!dir.join(format!("{BINARY_IMAGE}.new")).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_rust_engine_is_swapped_in_next_to_the_old_one() {
        let dir = temp_dir("rs-swap");
        fs::write(dir.join(BINARY_IMAGE), b"python").unwrap();
        fs::write(dir.join(RS_IMAGE), b"old-rs").unwrap();

        write_binary(&dir, RS_IMAGE, b"new-rs").unwrap();

        assert_eq!(fs::read(dir.join(RS_IMAGE)).unwrap(), b"new-rs");
        // Installing one engine never touches the other: that is what makes
        // switching back a restart instead of a download (R11).
        assert_eq!(fs::read(dir.join(BINARY_IMAGE)).unwrap(), b"python");
        assert!(!dir.join(format!("{RS_IMAGE}.old")).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn interrupted_swap_is_recovered_on_start() {
        let dir = temp_dir("recover");
        fs::write(dir.join(BINARY_IMAGE), b"previous").unwrap();
        // Simulate a crash after "old -> .old" but before "new -> final".
        fs::rename(
            dir.join(BINARY_IMAGE),
            dir.join(format!("{BINARY_IMAGE}.old")),
        )
        .unwrap();
        fs::write(dir.join(format!("{BINARY_IMAGE}.new")), b"partial").unwrap();

        recover_binary(&dir, BINARY_IMAGE);

        assert_eq!(fs::read(dir.join(BINARY_IMAGE)).unwrap(), b"previous");
        assert!(!dir.join(format!("{BINARY_IMAGE}.new")).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    // ---------- the Rust engine ----------

    fn gh_asset(name: &str, digest: Option<&str>) -> GhAsset {
        GhAsset {
            name: name.into(),
            browser_download_url: format!(
                "https://github.com/{RS_REPO}/releases/download/v2.7.0/{name}"
            ),
            size: 2_300_000,
            digest: digest.map(str::to_string),
        }
    }

    #[test]
    fn the_link_does_not_depend_on_the_engine() {
        let secret = "00112233445566778899aabbccddeeff";
        // R9: the link is a function of secret, host and port only, so a
        // switch cannot invalidate the entry the user added in Telegram.
        assert_eq!(
            link_for("0.0.0.0", 1555, secret, ""),
            link_for("127.0.0.1", 1555, secret, "")
        );
        assert!(link_for("0.0.0.0", 1555, secret, "").contains(&format!("secret=dd{secret}")));
        assert!(
            !link_for("0.0.0.0", 1555, secret, "").contains(&format!("secret=ee{secret}")),
            "a plain secret must use dd"
        );
        // FakeTLS switches the format for both engines; the domain is hex.
        let fake = link_for("::", 1555, secret, "example.com");
        assert_eq!(
            fake,
            format!(
                "tg://proxy?server=127.0.0.1&port=1555&secret=ee{secret}6578616d706c652e636f6d"
            )
        );
    }

    #[test]
    fn engine_values_are_read_leniently_and_written_strictly() {
        assert_eq!(ProxyEngine::parse("rs").unwrap(), ProxyEngine::Rs);
        assert_eq!(ProxyEngine::parse(" Rust ").unwrap(), ProxyEngine::Rs);
        assert_eq!(ProxyEngine::parse("python").unwrap(), ProxyEngine::Python);
        // An empty value means "not chosen yet", which is the default engine.
        assert_eq!(ProxyEngine::parse("").unwrap(), ProxyEngine::Python);
        assert!(ProxyEngine::parse("node").is_err());

        // A hand-edited config must never leave the user without a proxy.
        assert_eq!(ProxyEngine::from_config("unknown"), ProxyEngine::Python);
        assert_eq!(ProxyEngine::from_config(""), ProxyEngine::Python);

        assert_eq!(ProxyEngine::Python.image(), BINARY_IMAGE);
        assert_eq!(ProxyEngine::Rs.image(), RS_IMAGE);
        assert!(ProxyEngine::Rs.binary_path().ends_with(RS_IMAGE));
        assert!(ProxyEngine::Python.binary_path().ends_with(BINARY_IMAGE));
        assert_eq!(ProxyEngine::Rs.as_str(), "rs");
    }

    #[test]
    fn the_new_engine_gets_its_own_flags() {
        let secret = "00112233445566778899aabbccddeeff";
        let default = cfg();
        let args = build_rs_args(&default, secret);
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--host" && w[1] == "127.0.0.1"));
        assert!(args.windows(2).any(|w| w[0] == "--port" && w[1] == "1555"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--secret" && w[1] == secret));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--pool-size" && w[1] == "4"));
        assert_eq!(args.iter().filter(|a| *a == "--dc-ip").count(), 2);
        // Nothing configured: the engine falls back to the upstream CF list.
        assert!(args.iter().any(|a| a == "--default-domains"));
        // Flags only the old engine knows must not leak into the new argv.
        for only_old in ["--buf-kb", "--cfproxy-domain", "--fake-tls-domain", "-v"] {
            assert!(
                !args.iter().any(|a| a == only_old),
                "{only_old} must not be passed to the new engine"
            );
        }
        assert!(!args.iter().any(|a| a == "--cf-balance"));
    }

    #[test]
    fn the_new_engine_gets_domains_worker_faketls_and_balance() {
        let mut full = cfg();
        full.cf_domains = vec!["one.example.com".into(), " two.example.com ".into()];
        full.cf_worker_domain = " Worker.Example.Workers.dev ".into();
        full.cf_balance = true;
        full.fake_tls_domain = "tls.example.com".into();
        full.verbose = true;

        let args = build_rs_args(&full, "aa");
        // One comma-separated value keeps the user's priority order.
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--cf-domain" && w[1] == "one.example.com,two.example.com"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--cf-worker-domain" && w[1] == "worker.example.workers.dev"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--listen-faketls-domain" && w[1] == "tls.example.com"));
        assert!(args.iter().any(|a| a == "--cf-balance"));
        assert!(args.iter().any(|a| a == "-v"));
        // A configured list wins; the engine does not need the fetched one.
        assert!(!args.iter().any(|a| a == "--default-domains"));

        // The old single-domain setting survives the switch.
        let mut legacy = cfg();
        legacy.cfproxy_user_domain = " legacy.example.com ".into();
        let args = build_rs_args(&legacy, "aa");
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--cf-domain" && w[1] == "legacy.example.com"));
        assert!(!args.iter().any(|a| a == "--default-domains"));
    }

    #[test]
    fn cloudflare_routing_stays_off_when_the_user_turned_it_off() {
        let mut off = cfg();
        off.cfproxy = false;
        off.cf_balance = true;
        off.cf_domains = vec!["one.example.com".into()];
        let args = build_rs_args(&off, "aa");
        assert!(!args.iter().any(|a| a == "--cf-domain"));
        assert!(!args.iter().any(|a| a == "--default-domains"));
    }

    #[test]
    fn the_engine_selects_its_own_command_line() {
        let secret = "00112233445566778899aabbccddeeff";
        let base = cfg();
        // The old engine's argv is exactly what it was before the switch: no
        // existing user is affected by this change (R11).
        assert_eq!(
            build_engine_args(ProxyEngine::Python, &base, secret),
            build_args(&base, secret)
        );
        assert_eq!(
            build_engine_args(ProxyEngine::Rs, &base, secret),
            build_rs_args(&base, secret)
        );
    }

    #[test]
    fn the_old_engine_takes_the_whole_domain_list() {
        let mut base = cfg();
        assert!(!build_args(&base, "aa")
            .iter()
            .any(|a| a == "--cfproxy-domain"));

        base.cf_domains = vec![" First.Example.com ".into(), "second.example.com".into()];
        let args = build_args(&base, "aa");
        // R3: the repeatable flag carries every domain in priority order.
        let domains: Vec<&str> = args
            .windows(2)
            .filter(|w| w[0] == "--cfproxy-domain")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(domains, vec!["first.example.com", "second.example.com"]);

        // A config written for the old engine keeps its single domain.
        let mut legacy = cfg();
        legacy.cfproxy_user_domain = "legacy.example.com".into();
        let args = build_args(&legacy, "aa");
        assert_eq!(args.iter().filter(|a| *a == "--cfproxy-domain").count(), 1);
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--cfproxy-domain" && w[1] == "legacy.example.com"));
    }

    #[test]
    fn python_argv_carries_every_domain_worker_and_flag() {
        let mut full = cfg();
        full.cf_domains = vec!["one.example.com".into(), "two.example.com".into()];
        full.cf_worker_domains = vec!["w1.workers.dev".into(), " w2.workers.dev ".into()];
        full.no_secure = true;
        full.force_test_dc = true;
        full.proxy_protocol = true;

        let args = build_args(&full, "aa");
        let domains: Vec<&str> = args
            .windows(2)
            .filter(|w| w[0] == "--cfproxy-domain")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(domains, vec!["one.example.com", "two.example.com"]);

        let workers: Vec<&str> = args
            .windows(2)
            .filter(|w| w[0] == "--cfproxy-worker-domain")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(workers, vec!["w1.workers.dev", "w2.workers.dev"]);

        for flag in ["--no-secure", "--force-test-dc", "--proxy-protocol"] {
            assert!(args.iter().any(|a| a == flag), "{flag} is missing");
        }
    }

    #[test]
    fn default_config_keeps_the_argv_minimal() {
        // R8: the defaults are the same as before, including the engine's own
        // datacenter map and Cloudflare list.
        let args = build_args(&cfg(), "aa");
        for absent in [
            "--no-secure",
            "--force-test-dc",
            "--proxy-protocol",
            "--cfproxy-domain",
            "--cfproxy-worker-domain",
        ] {
            assert!(
                !args.iter().any(|a| a == absent),
                "{absent} must not be passed by default"
            );
        }
        assert_eq!(args.iter().filter(|a| *a == "--dc-ip").count(), 2);
        assert!(args.windows(2).any(|w| w[0] == "--buf-kb" && w[1] == "256"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--pool-size" && w[1] == "4"));
    }

    #[test]
    fn the_new_engine_takes_every_worker_and_no_secure() {
        let mut full = cfg();
        full.cf_worker_domains = vec!["W1.workers.dev".into(), "w2.workers.dev".into()];
        full.no_secure = true;
        full.force_test_dc = true;
        full.proxy_protocol = true;

        let args = build_rs_args(&full, "aa");
        let workers: Vec<&str> = args
            .windows(2)
            .filter(|w| w[0] == "--cf-worker-domain")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(workers, vec!["w1.workers.dev", "w2.workers.dev"]);
        // R4: `--no-secure` maps to the Rust engine's Cloudflare-only switch.
        assert!(args.iter().any(|a| a == "--cf-disable-tls"));
        // No Rust equivalent: these must be dropped, never passed as unknown.
        for absent in ["--no-secure", "--force-test-dc", "--proxy-protocol"] {
            assert!(
                !args.iter().any(|a| a == absent),
                "{absent} has no Rust equivalent"
            );
        }

        // TLS stays on unless the user asked otherwise.
        assert!(!build_rs_args(&cfg(), "aa")
            .iter()
            .any(|a| a == "--cf-disable-tls"));
    }

    #[test]
    fn dc_redirects_are_dc_colon_ip() {
        let saved = validate_dc_ips(&[
            " 2:149.154.167.220 ".into(),
            // IPv6 brings its own colons: only the first one separates the DC.
            "203:2A00:1450:4001::1".into(),
            "2:149.154.167.220".into(),
            "".into(),
        ])
        .expect("ips");
        assert_eq!(saved, vec!["2:149.154.167.220", "203:2a00:1450:4001::1"]);
        assert!(validate_dc_ips(&[]).expect("empty list").is_empty());
    }

    #[test]
    fn dc_redirects_reject_bad_input() {
        for bad in [
            "",
            "149.154.167.220",
            "0:149.154.167.220",
            "204:149.154.167.220",
            "x:149.154.167.220",
            "2:nope",
            "2:",
        ] {
            assert!(validate_dc_redirect(bad).is_err(), "{bad} must be refused");
        }

        let count = MAX_DC_REDIRECTS + 1;
        let too_many: Vec<String> = (1..=count)
            .map(|dc| format!("{dc}:149.154.167.220"))
            .collect();
        let error = validate_dc_ips(&too_many).expect_err("must refuse");
        assert!(error.contains(&MAX_DC_REDIRECTS.to_string()), "{error}");
    }

    #[test]
    fn limits_stay_inside_the_engine_range() {
        assert_eq!(validate_buf_kb(MIN_BUF_KB).unwrap(), MIN_BUF_KB);
        assert_eq!(validate_buf_kb(MAX_BUF_KB).unwrap(), MAX_BUF_KB);
        assert!(validate_buf_kb(MIN_BUF_KB - 1).is_err());
        assert!(validate_buf_kb(MAX_BUF_KB + 1).is_err());

        assert_eq!(validate_pool_size(0).unwrap(), 0);
        assert_eq!(validate_pool_size(MAX_POOL_SIZE).unwrap(), MAX_POOL_SIZE);
        assert!(validate_pool_size(MAX_POOL_SIZE + 1).is_err());
    }

    #[test]
    fn the_windows_archive_is_preferred_over_the_win7_one() {
        // A release that publishes both: the regular Windows 10 build wins.
        let both = vec![
            gh_asset(RS_ASSETS[1], Some("sha256:win7")),
            gh_asset(
                "tg-ws-proxy-x86_64-unknown-linux-musl.tar.gz",
                Some("sha256:l"),
            ),
            gh_asset(RS_ASSETS[0], Some("sha256:regular")),
        ];
        let picked = pick_rs_asset(&both).expect("asset");
        assert_eq!(picked.name, RS_ASSETS[0]);
        assert_eq!(picked.digest, "sha256:regular");
        assert_eq!(picked.size, 2_300_000);

        // Older releases (v2.5.8 and earlier) only ship the Windows 7 build.
        let only_win7 = vec![gh_asset(RS_ASSETS[1], Some("sha256:win7"))];
        assert_eq!(pick_rs_asset(&only_win7).expect("asset").name, RS_ASSETS[1]);
    }

    #[test]
    fn a_release_without_a_windows_build_is_refused() {
        let assets = vec![
            gh_asset("luci-app-tg-ws-proxy-rs-2.7.0-r1.apk", Some("sha256:a")),
            gh_asset("tg-ws-proxy-i686-win7-windows-msvc.zip", Some("sha256:b")),
        ];
        let error = pick_rs_asset(&assets).expect_err("must refuse");
        assert!(error.contains("Windows x64"), "{error}");
    }

    #[test]
    fn a_release_without_a_sha256_digest_is_refused() {
        // R2: no digest, no install.
        let missing = vec![gh_asset(RS_ASSETS[0], None)];
        let error = pick_rs_asset(&missing).expect_err("must refuse");
        assert!(error.contains("SHA-256"), "{error}");

        // A digest of another algorithm is not a SHA-256 either.
        let foreign = vec![gh_asset(RS_ASSETS[0], Some("md5:abc"))];
        assert!(pick_rs_asset(&foreign).is_err());
        // An empty one is worse than none.
        let empty = vec![gh_asset(RS_ASSETS[0], Some(""))];
        assert!(pick_rs_asset(&empty).is_err());
    }

    #[test]
    fn an_insecure_download_url_is_refused() {
        let assets = vec![GhAsset {
            name: RS_ASSETS[0].into(),
            browser_download_url: format!("http://example.com/{}", RS_ASSETS[0]),
            size: 2_300_000,
            digest: Some("sha256:abc".into()),
        }];
        let error = pick_rs_asset(&assets).expect_err("must refuse");
        assert!(error.contains("https"), "{error}");
    }

    /// Builds a release archive in memory, so the unpacking test needs neither
    /// the network nor a file on disk.
    fn archive(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            for (name, body) in entries {
                writer.start_file(*name, options).expect("entry");
                writer.write_all(body).expect("write");
            }
            writer.finish().expect("finish");
        }
        buffer.into_inner()
    }

    #[test]
    fn the_executable_is_taken_out_of_the_release_archive() {
        let payload = vec![7u8; MIN_EXE_BYTES as usize];
        let bytes = archive(&[
            ("readme.txt", b"release notes".to_vec()),
            (RS_ARCHIVE_EXE, payload.clone()),
        ]);

        let exe = extract_exe_from_zip(&bytes, RS_ARCHIVE_EXE).expect("exe");
        assert_eq!(exe, payload);

        // A short "executable" is an error page or a truncated download.
        let tiny = archive(&[(RS_ARCHIVE_EXE, b"MZ".to_vec())]);
        assert!(extract_exe_from_zip(&tiny, RS_ARCHIVE_EXE).is_err());

        // No executable at all: refuse instead of writing something else.
        let wrong = archive(&[("readme.txt", b"notes".to_vec())]);
        assert!(extract_exe_from_zip(&wrong, RS_ARCHIVE_EXE).is_err());

        // Not a zip at all.
        assert!(extract_exe_from_zip(b"<html>error</html>", RS_ARCHIVE_EXE).is_err());
    }

    #[test]
    fn the_old_engine_is_offered_but_never_forced() {
        assert!(!fallback_recommended(ProxyEngine::Rs, 1));
        assert!(!fallback_recommended(ProxyEngine::Rs, RS_FAILED_STARTS - 1));
        assert!(fallback_recommended(ProxyEngine::Rs, RS_FAILED_STARTS));
        // The old engine has nothing to fall back to.
        assert!(!fallback_recommended(ProxyEngine::Python, 99));
    }

    #[test]
    fn domains_are_bare_hostnames() {
        assert_eq!(validate_domain(" Example.COM. ").unwrap(), "example.com");
        assert_eq!(
            validate_domain("worker.example.workers.dev").unwrap(),
            "worker.example.workers.dev"
        );
        // Schemes, paths, ports and queries all come from a copy-paste.
        for bad in [
            "https://example.com",
            "example.com/path",
            "example.com:8443",
            "example.com?x=1",
            "example.com#frag",
            "localhost",
            "1.2.3.4",
            "-bad.example.com",
            "bad-.example.com",
            "exa mple.com",
            "_dmarc.example.com",
            "",
            "   ",
        ] {
            assert!(validate_domain(bad).is_err(), "{bad} must be refused");
        }
        // An empty worker domain is "not configured", not an error.
        assert_eq!(validate_worker_domain(" ").unwrap(), "");
        assert_eq!(
            validate_worker_domain("Worker.Example.com").unwrap(),
            "worker.example.com"
        );
    }

    #[test]
    fn the_domain_list_keeps_order_drops_blanks_and_stops_at_ten() {
        let saved = validate_domains(&[
            " b.example.com ".into(),
            "".into(),
            "a.example.com".into(),
            "B.example.com".into(),
        ])
        .expect("domains");
        // Order is priority order, duplicates collapse, blanks vanish.
        assert_eq!(saved, vec!["b.example.com", "a.example.com"]);

        let too_many: Vec<String> = (0..=MAX_CF_DOMAINS)
            .map(|index| format!("d{index}.example.com"))
            .collect();
        assert_eq!(too_many.len(), MAX_CF_DOMAINS + 1);
        let error = validate_domains(&too_many).expect_err("must refuse");
        assert!(error.contains(&MAX_CF_DOMAINS.to_string()), "{error}");

        // Exactly the limit is fine.
        let limit: Vec<String> = too_many[..MAX_CF_DOMAINS].to_vec();
        assert_eq!(
            validate_domains(&limit).expect("domains").len(),
            MAX_CF_DOMAINS
        );

        assert!(validate_domains(&["not a domain".into()]).is_err());
        assert!(validate_domains(&[]).expect("empty list").is_empty());
    }
}
