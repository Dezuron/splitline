use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const APP_DIR_NAME: &str = "zapret-ui";
pub const CONFIG_FILE: &str = "config.json";
pub const RUNTIME_DIR_NAME: &str = "runtime";

/// Data folder override for the screenshot profile (`scripts/shot.ps1`).
/// Release builds never set it: the dev instance points at its own copy of the
/// config while the portable app keeps running next to it.
pub const APP_DIR_ENV: &str = "SPLITLINE_APP_DIR";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GameFilter {
    Disabled,
    All,
    Tcp,
    Udp,
}

impl GameFilter {
    pub fn tcp_value(&self) -> &'static str {
        match self {
            GameFilter::Disabled => "12",
            GameFilter::All | GameFilter::Tcp => "1024-65535",
            GameFilter::Udp => "12",
        }
    }

    pub fn udp_value(&self) -> &'static str {
        match self {
            GameFilter::Disabled => "12",
            GameFilter::All | GameFilter::Udp => "1024-65535",
            GameFilter::Tcp => "12",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            GameFilter::Disabled => "disabled",
            GameFilter::All => "all",
            GameFilter::Tcp => "tcp",
            GameFilter::Udp => "udp",
        }
    }
}

/// Settings of the bundled Telegram MTProto-over-WebSocket proxy
/// (headless build of Flowseal/tg-ws-proxy).
///
/// Only `host`, `port` and `secret` are edited from the UI; every other field
/// mirrors an upstream CLI flag and is meant to be tuned by hand in
/// `config.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TgwsConfig {
    /// Start the proxy together with the application.
    /// Kept for config compatibility; the session snapshot is what restores it.
    pub autostart: bool,
    /// The proxy was running when the app last changed it.
    pub session_running: bool,
    pub host: String,
    /// `0` is never valid: the app validates the range before saving.
    pub port: u16,
    /// 32 hex characters (MTProto secret). Generated on first use.
    pub secret: String,
    /// Telegram datacenter redirects, e.g. `2:149.154.167.220`.
    pub dc_ip: Vec<String>,
    pub buf_kb: u32,
    /// Ready WebSocket connections per DC. `0` disables the direct DC route.
    pub pool_size: u32,
    pub cfproxy: bool,
    pub h2: bool,
    pub cfproxy_user_domain: String,
    pub fake_tls_domain: String,
    pub verbose: bool,
    /// Upstream `--no-secure`: Cloudflare connections without TLS. The Rust
    /// engine has no exact counterpart; see `build_rs_args`.
    #[serde(default)]
    pub no_secure: bool,
    /// Upstream `--force-test-dc`: use the Telegram test datacenters.
    #[serde(default)]
    pub force_test_dc: bool,
    /// Upstream `--proxy-protocol`: expect the PROXY protocol header from the
    /// client (for a proxy in front of the proxy).
    #[serde(default)]
    pub proxy_protocol: bool,
    /// Version tag of the installed headless binary (`1.11.2-1`), empty when
    /// the binary was placed manually.
    pub installed_version: String,
    /// Release tag the user dismissed with "Позже".
    pub dismissed_update_tag: String,
    /// One-time migration flag: the secret/port may be inherited from a
    /// standalone TgWsProxy install, so the Telegram entry keeps working.
    pub secret_migrated: bool,
    /// The `tg://` link was opened automatically once after the first start;
    /// prevents nagging the user on every launch.
    pub link_opened: bool,
    /// Proxy engine: `python` (the proven headless Flowseal build) or `rs`
    /// (`valnesfjord/tg-ws-proxy-rs`). Kept as a string so a config written by
    /// a newer version still loads here; unknown values mean the default.
    pub engine: String,
    /// Cloudflare proxy domains, tried in the order given (`--cf-domain`).
    pub cf_domains: Vec<String>,
    /// Cloudflare Worker domain for the TCP-tunnel fallback
    /// (`--cf-worker-domain`). Kept for configs written by 2.2.0: the list
    /// below is the source of truth, this value is migrated into it.
    pub cf_worker_domain: String,
    /// Cloudflare Worker domains in priority order: the old engine takes
    /// `--cfproxy-worker-domain`, the Rust one `--cf-worker-domain`.
    /// Empty means "not configured".
    #[serde(default)]
    pub cf_worker_domains: Vec<String>,
    /// Rotate across `cf_domains` instead of always starting from the first
    /// one (`--cf-balance`). Has no effect with a single domain.
    pub cf_balance: bool,
    /// Version of the installed Rust engine binary, empty when it was placed
    /// by hand. Only used to remember what sits on disk.
    pub rs_version: String,
}

impl Default for TgwsConfig {
    fn default() -> Self {
        Self {
            autostart: true,
            session_running: false,
            host: "127.0.0.1".into(),
            // 1443 is the tg-ws-proxy convention; keep ours out of its way.
            port: 1555,
            secret: String::new(),
            dc_ip: vec!["2:149.154.167.220".into(), "4:149.154.167.220".into()],
            buf_kb: 256,
            pool_size: 4,
            cfproxy: true,
            h2: true,
            cfproxy_user_domain: String::new(),
            fake_tls_domain: String::new(),
            verbose: false,
            no_secure: false,
            force_test_dc: false,
            proxy_protocol: false,
            installed_version: String::new(),
            dismissed_update_tag: String::new(),
            secret_migrated: false,
            link_opened: false,
            // The new engine is opt-in for 2.2.0 (R1): the old one stays the
            // default until the new one proves itself.
            engine: "python".into(),
            cf_domains: Vec::new(),
            cf_worker_domain: String::new(),
            cf_worker_domains: Vec::new(),
            cf_balance: false,
            rs_version: String::new(),
        }
    }
}

/// VPN subscription settings: a URL that returns ready-made Xray configs.
///
/// The provider binds the subscription to a device id (Windows `MachineGuid`),
/// so the URL and the chosen profile are all that has to be remembered.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct XrayConfig {
    /// Subscription endpoint; empty means "not configured".
    pub subscription_url: String,
    /// Index of the profile inside the subscription.
    pub selected: usize,
    /// Local SOCKS5 port Telegram is pointed at.
    pub socks_port: u16,
    /// Local HTTP port for browsers.
    pub http_port: u16,
    /// Start the tunnel together with the application.
    /// Kept for config compatibility; the session snapshot is what restores it.
    pub autostart: bool,
    /// The tunnel was running when the app last changed it.
    pub session_running: bool,
    /// Domains that must go through the tunnel. Everything else stays direct:
    /// the list is a whitelist, not a blacklist.
    pub proxy_sites: Vec<String>,
    /// Point the Windows system proxy at the local HTTP inbound, so browsers
    /// follow the very same rules.
    pub system_proxy: bool,
    pub installed_version: String,
    /// Unix time of the last successful domain-list update (`geosite.dat`).
    pub bases_updated_at: u64,
    /// Switch to the fastest live location when the current one stops working.
    #[serde(default)]
    pub auto_switch: bool,
    /// Last `subscription-userinfo` values, kept across restarts.
    #[serde(default)]
    pub subscription_upload: u64,
    #[serde(default)]
    pub subscription_download: u64,
    #[serde(default)]
    pub subscription_total: u64,
    #[serde(default)]
    pub subscription_expire: u64,
    /// Provider's account page (`profile-web-page-url`), empty when absent.
    #[serde(default)]
    pub subscription_page: String,
}

impl Default for XrayConfig {
    fn default() -> Self {
        Self {
            subscription_url: String::new(),
            selected: 0,
            // Deliberately away from the 10808/10809 a stock client uses, so
            // both can run at once without fighting over the port.
            socks_port: 17808,
            http_port: 17809,
            autostart: false,
            session_running: false,
            proxy_sites: Vec::new(),
            system_proxy: false,
            installed_version: String::new(),
            bases_updated_at: 0,
            auto_switch: false,
            subscription_upload: 0,
            subscription_download: 0,
            subscription_total: 0,
            subscription_expire: 0,
            subscription_page: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub last_strategy: String,
    pub autostart: bool,
    /// The bypass was running when the app last changed it.
    pub session_zapret: bool,
    /// False until the session flags were seeded from the old autostart settings.
    pub session_initialized: bool,
    pub check_updates: bool,
    pub flashscore_bypass: bool,
    pub game_filter: GameFilter,
    pub runtime_version: String,
    /// How many mutants `run_search` generates.
    pub bench_budget: usize,
    /// Verify a freshly applied strategy after a minute and roll back on failure.
    pub apply_auto_rollback: bool,
    /// Apply the best working strategy automatically after a run.
    pub bench_auto_apply: bool,
    /// Check for application updates on startup.
    pub check_app_updates: bool,
    /// Custom folder for the zapret runtime. Empty means `%LOCALAPPDATA%\zapret-ui\runtime`.
    pub runtime_path: String,
    pub last_run_version: String,
    /// UI theme: `satellite` or `carmine`.
    pub theme: String,
    pub tgws: TgwsConfig,
    pub xray: XrayConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            last_strategy: String::new(),
            autostart: true,
            session_zapret: false,
            session_initialized: false,
            check_updates: true,
            flashscore_bypass: true,
            game_filter: GameFilter::Disabled,
            runtime_version: String::new(),
            bench_budget: 40,
            apply_auto_rollback: true,
            bench_auto_apply: false,
            check_app_updates: true,
            runtime_path: String::new(),
            last_run_version: String::new(),
            theme: "satellite".into(),
            tgws: TgwsConfig::default(),
            xray: XrayConfig::default(),
        }
    }
}

pub fn app_dir() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    // The override exists for `scripts/shot.ps1` only. Release builds keep the
    // documented `%LOCALAPPDATA%\zapret-ui` folder no matter the environment.
    #[cfg(debug_assertions)]
    let override_dir = std::env::var_os(APP_DIR_ENV).map(PathBuf::from);
    #[cfg(not(debug_assertions))]
    let override_dir = None;
    resolve_app_dir(base, override_dir)
}

/// `base/zapret-ui` by default; an explicitly set folder wins as-is.
fn resolve_app_dir(base: PathBuf, override_dir: Option<PathBuf>) -> PathBuf {
    match override_dir {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => base.join(APP_DIR_NAME),
    }
}

pub fn config_path() -> PathBuf {
    app_dir().join(CONFIG_FILE)
}

pub fn runtime_dir() -> PathBuf {
    resolve_runtime_dir(&app_dir(), &load_config().runtime_path)
}

/// Folder that holds the headless Telegram proxy binary, its log and its
/// update cache. Deliberately *outside* `runtime/`: updating the zapret bundle
/// replaces that whole folder and would wipe the proxy.
pub fn tgws_dir() -> PathBuf {
    app_dir().join("tgws")
}

pub fn tgws_binary_path() -> PathBuf {
    tgws_dir().join("tgws-cli.exe")
}

/// Binary of the Rust engine. It sits next to the old one so a switch never
/// depends on a download and going back is always one click away (R11).
pub fn tgws_rs_binary_path() -> PathBuf {
    tgws_dir().join("tgws-rs.exe")
}

pub fn tgws_log_path() -> PathBuf {
    tgws_dir().join("tgws.log")
}

/// Resolves the configured runtime folder. An empty value keeps the default
/// under `%LOCALAPPDATA%\zapret-ui\runtime`.
pub fn resolve_runtime_dir(base: &std::path::Path, configured: &str) -> PathBuf {
    let configured = configured.trim();
    if configured.is_empty() {
        base.join(RUNTIME_DIR_NAME)
    } else {
        PathBuf::from(configured)
    }
}

/// Creates the folder and validates that it is writable. A non-empty folder is
/// accepted only when it already contains a zapret runtime, so the app never
/// mixes its files into an unrelated directory.
pub fn prepare_runtime_dir(path: &std::path::Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("Не удалось создать папку: {e}"))?;

    let probe = path.join(".zapret-ui-write-test");
    fs::write(&probe, b"").map_err(|e| format!("Папка недоступна для записи: {e}"))?;
    let _ = fs::remove_file(&probe);

    let has_runtime = path.join("bin").join("winws.exe").exists();
    if !has_runtime {
        let non_empty = fs::read_dir(path)
            .map_err(|e| e.to_string())?
            .flatten()
            .next()
            .is_some();
        if non_empty {
            return Err(
                "Папка не пуста и не содержит runtime zapret. Выберите пустую папку.".into(),
            );
        }
    }
    Ok(())
}

/// Parses a config written by any build. Used both for `config.json` on disk and
/// for an imported archive, so an import cannot bring in a theme the UI would not
/// understand.
pub(crate) fn parse_config(text: &str) -> Option<AppConfig> {
    // A UTF-8 BOM is valid for many editors but serde_json rejects it. Without
    // stripping it, a hand-edited config silently resets every setting.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut cfg = serde_json::from_str::<AppConfig>(text).ok()?;
    // Themes trimmed to satellite/carmine: every old dark variant maps to
    // satellite, the paper themes to carmine.
    cfg.theme = match cfg.theme.to_ascii_lowercase().as_str() {
        "carmine" => "carmine".into(),
        "light" | "stencil" => "carmine".into(),
        _ => "satellite".into(),
    };
    migrate_tgws_domain_lists(&mut cfg.tgws);
    Some(cfg)
}

/// Carries the single-domain settings of 2.2.0 into the lists (R2).
///
/// The old fields stay in the file, so a build that does not know the lists
/// still reads a working domain; but from now on the list is the source of
/// truth and the legacy value is only a fallback for an empty list. Both
/// directions of the migration run here, once per load, so a hand-edited
/// `config.json` behaves like one written by the UI.
fn migrate_tgws_domain_lists(tgws: &mut TgwsConfig) {
    if domain_list_is_empty(&tgws.cf_worker_domains) {
        let legacy = tgws.cf_worker_domain.trim();
        if !legacy.is_empty() {
            tgws.cf_worker_domains = vec![legacy.to_ascii_lowercase()];
        }
    }
    if domain_list_is_empty(&tgws.cf_domains) {
        let legacy = tgws.cfproxy_user_domain.trim();
        if !legacy.is_empty() {
            tgws.cf_domains = vec![legacy.to_ascii_lowercase()];
        }
    }
}

/// A list of blank strings counts as empty: that is what a cleared UI form
/// saves, and it must not block the migration of the legacy value.
fn domain_list_is_empty(values: &[String]) -> bool {
    values.iter().all(|value| value.trim().is_empty())
}

/// Both remaining themes are dark, so the native title bar stays dark too.
pub fn theme_is_dark(_theme: &str) -> bool {
    true
}

pub fn load_config() -> AppConfig {
    match fs::read_to_string(config_path()) {
        Ok(text) => parse_config(&text).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    }
}

pub fn save_config(cfg: &AppConfig) -> Result<(), String> {
    save_config_at(&app_dir(), cfg)
}

/// Same write-then-rename contract, but into an explicit folder. The import
/// writes the merged config there, and tests point it at a temporary directory.
pub fn save_config_at(dir: &std::path::Path, cfg: &AppConfig) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    let path = dir.join(CONFIG_FILE);
    // Write-then-rename: a crash or a concurrent read can never observe a
    // half-written config (which used to silently reset every setting).
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("zapret-ui-config-{name}-{unique}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn legacy_light_theme_becomes_carmine() {
        let cfg = AppConfig {
            theme: "light".into(),
            ..AppConfig::default()
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        let parsed = parse_config(&json).expect("parse");
        assert_eq!(parsed.theme, "carmine");
        assert!(theme_is_dark("carmine"));
        assert!(theme_is_dark("satellite"));
    }

    #[test]
    fn legacy_dark_and_gray_map_to_the_new_themes() {
        for old in ["dark", "gray", "orbital", "orbit"] {
            let cfg = AppConfig {
                theme: old.into(),
                ..AppConfig::default()
            };
            let json = serde_json::to_string(&cfg).expect("serialize");
            assert_eq!(parse_config(&json).expect("parse").theme, "satellite");
        }
        for old in ["stencil", "light"] {
            let cfg = AppConfig {
                theme: old.into(),
                ..AppConfig::default()
            };
            let json = serde_json::to_string(&cfg).expect("serialize");
            assert_eq!(parse_config(&json).expect("parse").theme, "carmine");
        }
    }

    #[test]
    fn screenshot_profile_can_point_at_its_own_app_dir() {
        let base = PathBuf::from(r"C:\base");
        assert_eq!(resolve_app_dir(base.clone(), None), base.join(APP_DIR_NAME));
        assert_eq!(
            resolve_app_dir(base.clone(), Some(PathBuf::new())),
            base.join(APP_DIR_NAME)
        );
        assert_eq!(
            resolve_app_dir(base, Some(PathBuf::from(r"C:\shot"))),
            PathBuf::from(r"C:\shot")
        );
    }

    #[test]
    fn default_runtime_dir_is_under_app_data() {
        let base = PathBuf::from(r"C:\base");
        assert_eq!(resolve_runtime_dir(&base, ""), base.join(RUNTIME_DIR_NAME));
        assert_eq!(
            resolve_runtime_dir(&base, "  "),
            base.join(RUNTIME_DIR_NAME)
        );
    }

    #[test]
    fn custom_runtime_dir_is_used_as_is() {
        assert_eq!(
            resolve_runtime_dir(&PathBuf::from(r"C:\base"), r"D:\zapret"),
            PathBuf::from(r"D:\zapret")
        );
    }

    #[test]
    fn prepare_accepts_an_empty_folder() {
        let dir = temp_dir("empty");
        assert!(prepare_runtime_dir(&dir).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prepare_accepts_a_folder_with_a_runtime() {
        let dir = temp_dir("runtime");
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::write(dir.join("bin").join("winws.exe"), b"exe").unwrap();
        assert!(prepare_runtime_dir(&dir).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prepare_rejects_an_unrelated_non_empty_folder() {
        let dir = temp_dir("foreign");
        fs::write(dir.join("notes.txt"), b"not a runtime").unwrap();
        assert!(prepare_runtime_dir(&dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_parser_accepts_a_utf8_bom() {
        let config = parse_config("\u{feff}{\"bench_budget\":37}").unwrap();
        assert_eq!(config.bench_budget, 37);
        assert!(config.check_app_updates);
    }

    #[test]
    fn clean_install_has_no_whats_new_marker() {
        assert!(AppConfig::default().last_run_version.is_empty());
    }

    #[test]
    fn config_without_engine_fields_migrates_to_python() {
        let mut value = serde_json::to_value(AppConfig::default()).expect("json");
        // A config written before 2.2.0 has none of the new keys; loading it
        // must not fail and must pick the old engine.
        let tgws = value["tgws"].as_object_mut().expect("tgws object");
        for key in [
            "engine",
            "cf_domains",
            "cf_worker_domain",
            "cf_balance",
            "rs_version",
        ] {
            tgws.remove(key);
        }
        let parsed = parse_config(&value.to_string()).expect("parse");
        assert_eq!(parsed.tgws.engine, "python");
        assert!(parsed.tgws.cf_domains.is_empty());
        assert_eq!(parsed.tgws.cf_worker_domain, "");
        assert!(!parsed.tgws.cf_balance);
        // Fields that already existed keep their values.
        assert_eq!(parsed.tgws.port, 1555);
        assert_eq!(parsed.tgws.pool_size, 4);
    }

    #[test]
    fn config_without_proxy_flags_loads_with_defaults() {
        // R1: a 2.2.0 config has none of the new keys; loading it must neither
        // fail nor turn any switch on by itself.
        let mut value = serde_json::to_value(AppConfig::default()).expect("json");
        let tgws = value["tgws"].as_object_mut().expect("tgws object");
        for key in [
            "cf_worker_domains",
            "no_secure",
            "force_test_dc",
            "proxy_protocol",
        ] {
            tgws.remove(key);
        }
        let parsed = parse_config(&value.to_string()).expect("parse");
        assert!(parsed.tgws.cf_worker_domains.is_empty());
        assert!(!parsed.tgws.no_secure);
        assert!(!parsed.tgws.force_test_dc);
        assert!(!parsed.tgws.proxy_protocol);
    }

    #[test]
    fn the_legacy_worker_domain_migrates_into_the_list() {
        let mut value = serde_json::to_value(AppConfig::default()).expect("json");
        let tgws = value["tgws"].as_object_mut().expect("tgws object");
        tgws.insert("cf_worker_domain".into(), "Worker.Example.com".into());
        tgws.remove("cf_worker_domains");
        let parsed = parse_config(&value.to_string()).expect("parse");
        assert_eq!(parsed.tgws.cf_worker_domains, vec!["worker.example.com"]);
        // The legacy field itself is left alone for a downgrade.
        assert_eq!(parsed.tgws.cf_worker_domain, "Worker.Example.com");
    }

    #[test]
    fn the_legacy_single_domain_migrates_into_the_list() {
        let mut value = serde_json::to_value(AppConfig::default()).expect("json");
        let tgws = value["tgws"].as_object_mut().expect("tgws object");
        tgws.insert("cfproxy_user_domain".into(), " example.com ".into());
        tgws.remove("cf_domains");
        let parsed = parse_config(&value.to_string()).expect("parse");
        assert_eq!(parsed.tgws.cf_domains, vec!["example.com"]);
        assert_eq!(parsed.tgws.cfproxy_user_domain, " example.com ");

        // An existing list wins: the migration never overwrites a choice the
        // user already made in the new field.
        let mut both = serde_json::to_value(AppConfig::default()).expect("json");
        let tgws = both["tgws"].as_object_mut().expect("tgws object");
        tgws.insert("cfproxy_user_domain".into(), "legacy.example.com".into());
        tgws.insert("cf_domains".into(), serde_json::json!(["list.example.com"]));
        let parsed = parse_config(&both.to_string()).expect("parse");
        assert_eq!(parsed.tgws.cf_domains, vec!["list.example.com"]);
    }
}
