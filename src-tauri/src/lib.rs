mod autostart;
mod bench;
mod commands;
mod config;
mod engine;
mod error;
mod generator;
mod job;
mod probe;
mod runtime;
mod strategy;
mod sys;
mod tgws;
mod updater;
mod whats_new;

#[cfg(test)]
mod bench_live_tests;
#[cfg(test)]
mod strategy_tests;

use bench::BenchSummary;
use config::AppConfig;
pub use error::{AppError, AppResult};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tauri::Manager;
use windows_sys::Win32::System::Registry::HKEY;

/// Locks a mutex even if another thread panicked while holding it. A poisoned
/// config mutex must not turn every later command into a panic: the value is
/// still readable/writable, and the app should keep running.
pub fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn lock_config(state: &AppState) -> std::sync::MutexGuard<'_, AppConfig> {
    lock_unpoisoned(&state.config)
}

pub struct AppState {
    pub config: Mutex<AppConfig>,
    /// True while a benchmark or search is running; guards concurrent runs.
    pub bench_running: Arc<AtomicBool>,
    /// True while the runtime is being updated.
    pub update_running: Arc<AtomicBool>,
    /// Set by `cancel_bench`, checked between candidates.
    pub bench_cancel: Arc<AtomicBool>,
    /// Last benchmark of this session (persisted to bench.json as well).
    pub last_bench: Mutex<Option<BenchSummary>>,
    /// Supervisor for the bundled Telegram MTProto proxy.
    pub tgws: crate::tgws::Tgws,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            config: Mutex::new(config::load_config()),
            bench_running: Arc::new(AtomicBool::new(false)),
            update_running: Arc::new(AtomicBool::new(false)),
            bench_cancel: Arc::new(AtomicBool::new(false)),
            last_bench: Mutex::new(None),
            tgws: crate::tgws::Tgws::new(),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
fn is_admin() -> bool {
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

/// Relaunch elevated (UAC) when the app is started normally but lacks admin rights.
///
/// Uses `ShellExecuteExW` with the "runas" verb directly instead of a PowerShell
/// subprocess: no console window flashes and the elevation behaves exactly like
/// "Run as administrator", which works regardless of the UAC prompt policy.
fn ensure_elevated() {
    #[cfg(not(debug_assertions))]
    {
        use windows_sys::Win32::UI::Shell::{
            ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
        };

        if is_admin() {
            return;
        }

        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let verb: Vec<u16> = "runas\0".encode_utf16().collect();
        let file: Vec<u16> = exe
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let params: Vec<u16> = std::env::args()
            .skip(1)
            .collect::<Vec<_>>()
            .join(" ")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = if params.is_empty() {
            std::ptr::null()
        } else {
            params.as_ptr()
        };
        info.nShow = 1; // SW_SHOWNORMAL

        let ok = unsafe { ShellExecuteExW(&mut info) };
        if ok == 0 {
            let err = std::io::Error::last_os_error();
            error_box(&format!(
                "Не удалось запустить Splitline с правами администратора.\n\nОшибка: {err}\n\nЗапустите приложение через «Запуск от имени администратора»."
            ));
        }
        std::process::exit(0);
    }
}

/// Ask DWM for a dark or light native title bar. The non-client caption is
/// drawn by Windows; the app theme only changes the webview unless this
/// attribute is kept in sync.
pub fn apply_titlebar_theme(window: &tauri::WebviewWindow, dark: bool) {
    use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};

    let _ = window.set_theme(Some(if dark {
        tauri::Theme::Dark
    } else {
        tauri::Theme::Light
    }));

    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let enabled: i32 = if dark { 1 } else { 0 };
    let caption: u32 = if dark { 0x000d0909 } else { 0x00f4f0ee };
    let caption_text: u32 = if dark { 0x00f6f3f2 } else { 0x00221917 };
    // Caption colours (attributes 35/36) exist only on Windows 11. On Windows 10
    // DwmSetWindowAttribute rejects them with E_INVALIDARG.
    let win11 = os_build_number().is_some_and(|build| build >= WIN11_BUILD);
    unsafe {
        DwmSetWindowAttribute(
            hwnd.0,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &enabled as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
        if win11 {
            // Paint the native caption explicitly, otherwise a system dark theme
            // can keep a dark title bar over a light app.
            DwmSetWindowAttribute(
                hwnd.0,
                35_u32,
                &caption as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<u32>() as u32,
            );
            DwmSetWindowAttribute(
                hwnd.0,
                36_u32,
                &caption_text as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<u32>() as u32,
            );
        }
    }
}

/// Reads `--name value` from the process arguments.
fn arg_value(name: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == name {
            return args.next();
        }
    }
    None
}

/// Waits for the previous app process to exit. The updater starts the new exe
/// before the old one terminates; the single-instance plugin must not run until
/// the old instance is gone.
fn wait_for_process_exit(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};

    const SYNCHRONIZE: u32 = 0x0010_0000;
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return;
    }
    unsafe {
        WaitForSingleObject(handle, 20_000);
        CloseHandle(handle);
    }
}

/// Shows a blocking error dialog. Runs before the Tauri runtime exists, so the
/// text must not rely on the webview.
fn error_box(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let message = wide(message);
    let title = wide("Splitline");
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// NUL-terminated UTF-16 copy of `text`, as the Win32 `*W` APIs expect.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Reads a `REG_DWORD` value. `None` when the key or the value is missing.
fn read_reg_dword(root: HKEY, subkey: &str, name: &str) -> Option<u32> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, KEY_READ, REG_DWORD,
    };

    let subkey = wide(subkey);
    let name = wide(name);
    let mut key: HKEY = std::ptr::null_mut();
    if unsafe { RegOpenKeyExW(root, subkey.as_ptr(), 0, KEY_READ, &mut key) } != ERROR_SUCCESS {
        return None;
    }

    let mut data: u32 = 0;
    let mut kind = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            (&mut data as *mut u32).cast::<u8>(),
            &mut size,
        )
    };
    unsafe { RegCloseKey(key) };
    if status != ERROR_SUCCESS || kind != REG_DWORD || size != 4 {
        return None;
    }
    Some(data)
}

/// Reads a non-empty `REG_SZ` value. `None` when the key or the value is missing.
fn read_reg_string(root: HKEY, subkey: &str, name: &str) -> Option<String> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, KEY_READ, REG_SZ,
    };

    let subkey = wide(subkey);
    let name = wide(name);
    let mut key: HKEY = std::ptr::null_mut();
    if unsafe { RegOpenKeyExW(root, subkey.as_ptr(), 0, KEY_READ, &mut key) } != ERROR_SUCCESS {
        return None;
    }

    let mut buffer = [0u16; 128];
    let mut kind = 0u32;
    let mut size = (buffer.len() * std::mem::size_of::<u16>()) as u32;
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            buffer.as_mut_ptr().cast::<u8>(),
            &mut size,
        )
    };
    unsafe { RegCloseKey(key) };
    if status != ERROR_SUCCESS || kind != REG_SZ {
        return None;
    }

    let len = (size as usize / std::mem::size_of::<u16>()).min(buffer.len());
    let text = String::from_utf16_lossy(&buffer[..len]);
    let text = text.trim_end_matches('\0').trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Windows build number from ntdll: `GetVersionExW` lies without the right
/// manifest compatibility section.
fn os_build_number() -> Option<u32> {
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(version_info: *mut OSVERSIONINFOW) -> i32;
    }

    let mut info: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
    info.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
    let status = unsafe { RtlGetVersion(&mut info) };
    if status != 0 {
        return None;
    }
    Some(info.dwBuildNumber)
}

/// Lowest Windows 10 release the app supports: 22H2 with the cumulative update
/// that raised the build revision to 19045.6456.
const WIN10_MIN_BUILD: u32 = 19045;
const WIN10_MIN_UBR: u32 = 6456;
/// Windows 11 keeps its original threshold.
const WIN11_BUILD: u32 = 22000;

/// Windows 11 (22000+) stays supported, Windows 10 is supported from 22H2
/// build 19045.6456 onward. Anything above that build passes too.
fn is_os_supported(build: u32, ubr: u32) -> bool {
    build >= WIN11_BUILD
        || build > WIN10_MIN_BUILD
        || (build == WIN10_MIN_BUILD && ubr >= WIN10_MIN_UBR)
}

#[cfg(test)]
mod os_tests {
    use super::{is_os_supported, os_build_number, read_reg_string};

    #[test]
    fn windows_10_22h2_requires_the_minimum_update_revision() {
        assert!(is_os_supported(19045, 6456));
        assert!(is_os_supported(19045, 7000));
        assert!(!is_os_supported(19045, 6455));
        assert!(!is_os_supported(19045, 5965));
    }

    #[test]
    fn windows_11_is_supported_regardless_of_revision() {
        assert!(is_os_supported(22000, 0));
        assert!(is_os_supported(26100, 1));
    }

    #[test]
    fn older_windows_versions_are_rejected() {
        assert!(!is_os_supported(19044, 6456));
        assert!(!is_os_supported(17763, 9999));
        assert!(!is_os_supported(9600, 0));
    }

    #[test]
    fn build_number_comes_from_ntdll() {
        let build = os_build_number().expect("RtlGetVersion must succeed");
        assert!(build >= 10240, "unexpected build number {build}");
    }

    #[test]
    fn registry_probe_reads_real_values() {
        use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;

        let product = read_reg_string(
            HKEY_LOCAL_MACHINE,
            "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
            "ProductName",
        )
        .expect("ProductName must be readable");
        assert!(product.starts_with("Windows"), "unexpected {product}");
    }
}

/// Blocks startup on Windows versions the app does not support: everything
/// below Windows 10 22H2 build 19045.6456.
fn ensure_supported_os() {
    use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;

    // An unreadable build number is not a reason to refuse to start.
    let Some(build) = os_build_number() else {
        return;
    };
    // `RtlGetVersion` reports no revision; the update build revision (the
    // `6456` in `19045.6456`) lives in the registry.
    let ubr = read_reg_dword(
        HKEY_LOCAL_MACHINE,
        "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
        "UBR",
    )
    .unwrap_or(0);
    if is_os_supported(build, ubr) {
        return;
    }

    error_box(&format!(
        "Splitline поддерживается на Windows 10 22H2 (сборка 19045.6456 и новее) и Windows 11.\n\n\
         Текущая сборка: {build}.{ubr}.\nОбновите Windows."
    ));
    std::process::exit(1);
}

/// Evergreen WebView2 Runtime client id, shared by the machine-wide
/// (`WOW6432Node`) and the per-user installations.
const WEBVIEW2_CLIENT_ID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

/// Windows 11 ships the WebView2 Runtime; on Windows 10 it comes with Microsoft
/// Edge and may be missing on stripped-down systems. Without it Tauri cannot
/// create a window, so the portable build would exit silently.
fn ensure_webview2() {
    if webview2_installed() {
        return;
    }

    error_box(
        "Не найден WebView2 Runtime, без него Splitline не может открыть окно.\n\n\
         Установите WebView2 Runtime от Microsoft и запустите Splitline снова:\n\
         https://go.microsoft.com/fwlink/p/?LinkId=2124703",
    );
    std::process::exit(1);
}

/// Looks for the Evergreen WebView2 Runtime registration: machine-wide (both
/// registry views) or per-user. The `pv` value holds the installed version.
fn webview2_installed() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    let keys = [
        (
            HKEY_LOCAL_MACHINE,
            format!("SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{WEBVIEW2_CLIENT_ID}"),
        ),
        (
            HKEY_LOCAL_MACHINE,
            format!("SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{WEBVIEW2_CLIENT_ID}"),
        ),
        (
            HKEY_CURRENT_USER,
            format!("SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{WEBVIEW2_CLIENT_ID}"),
        ),
    ];
    keys.iter()
        .any(|(root, subkey)| read_reg_string(*root, subkey, "pv").is_some())
}

/// Keeps the window reachable: if its rectangle does not intersect any monitor
/// (changed monitor layout, display-mode switch, stale position), center it on
/// the primary monitor.
fn ensure_on_screen(window: &tauri::WebviewWindow) {
    let Ok(position) = window.outer_position() else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };
    let Ok(monitors) = window.available_monitors() else {
        return;
    };

    let visible = monitors.iter().any(|monitor| {
        let origin = monitor.position();
        let extent = monitor.size();
        position.x < origin.x + extent.width as i32
            && position.x + size.width as i32 > origin.x
            && position.y < origin.y + extent.height as i32
            && position.y + size.height as i32 > origin.y
    });
    if visible {
        return;
    }

    if let Ok(Some(monitor)) = window.primary_monitor() {
        let origin = monitor.position();
        let extent = monitor.size();
        let x = origin.x + ((extent.width as i32 - size.width as i32) / 2).max(0);
        let y = origin.y + ((extent.height as i32 - size.height as i32) / 2).max(0);
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

/// Shows the main window and makes sure it is not parked outside every monitor.
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        ensure_on_screen(&window);
        let _ = window.show();
        apply_titlebar_theme(&window, config::theme_is_dark(&config::load_config().theme));
        let _ = window.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The updater launches the new exe with --wait-for-pid before this process
    // exits, so the single-instance plugin is not blocked by the old instance.
    if let Some(pid) = arg_value("--wait-for-pid").and_then(|value| value.parse().ok()) {
        wait_for_process_exit(pid);
    }
    updater::cleanup_backup();
    ensure_supported_os();
    ensure_webview2();
    ensure_elevated();
    let is_autostart = std::env::args().any(|a| a == "--autostart");

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::set_strategy,
            commands::start,
            commands::stop,
            commands::get_config,
            commands::get_whats_new,
            commands::dismiss_whats_new,
            commands::set_game_filter,
            commands::set_autostart,
            commands::set_check_updates,
            commands::set_check_app_updates,
            commands::set_theme,
            commands::set_runtime_path,
            commands::set_flashscore_bypass,
            commands::set_bench_auto_apply,
            commands::check_update,
            commands::check_app_update,
            commands::apply_update,
            commands::apply_app_update,
            commands::open_external,
            commands::open_runtime_folder,
            commands::run_bench,
            commands::run_search,
            commands::cancel_bench,
            commands::quick_check,
            commands::apply_candidate,
            commands::delete_generated,
            commands::delete_all_generated,
            commands::get_last_bench,
            commands::get_tgws_status,
            commands::start_tgws,
            commands::stop_tgws,
            commands::restart_tgws,
            commands::get_tgws_link,
            commands::regenerate_tgws_secret,
            commands::get_tgws_logs,
            commands::check_tgws_update,
            commands::install_tgws_update,
            commands::dismiss_tgws_update,
            commands::set_tgws_autostart,
            commands::set_tgws_host,
            commands::set_tgws_port,
            commands::set_tgws_secret,
            commands::use_tgws_fallback_port,
            commands::open_tgws_log,
            commands::open_telegram_link,
            commands::copy_to_clipboard,
        ])
        .setup(move |app| {
            // Tray
            let show_i =
                tauri::menu::MenuItem::with_id(app, "show", "Показать", true, None::<&str>)?;
            let start_i =
                tauri::menu::MenuItem::with_id(app, "start", "Запустить", true, None::<&str>)?;
            let stop_i =
                tauri::menu::MenuItem::with_id(app, "stop", "Остановить", true, None::<&str>)?;
            let quit_i = tauri::menu::MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&show_i, &start_i, &stop_i, &quit_i])?;

            let _tray = tauri::tray::TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Splitline")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        show_main_window(app);
                    }
                    "start" => {
                        let state = app.state::<AppState>();
                        let _ = crate::commands::start(state);
                    }
                    "stop" => {
                        let state = app.state::<AppState>();
                        let _ = crate::commands::stop(state);
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        show_main_window(app);
                    }
                })
                .build(app)?;

            // Hide window on close (minimize to tray) instead of exiting
            if let Some(w) = app.get_webview_window("main") {
                let dark = config::theme_is_dark(&config::load_config().theme);
                apply_titlebar_theme(&w, dark);
                let window = w.clone();
                w.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                });
            }

            // If the previous session died in the middle of a strategy check, the
            // bypass service is stopped; restore it before doing anything else.
            commands::recover_pending(&app.state::<AppState>());

            // Telegram proxy: start it in the background when the user asked for
            // it and the binary is already downloaded. A missing binary is not
            // an error — the Telegram page offers to download it on demand.
            {
                let enabled = {
                    let state = app.state::<AppState>();
                    let cfg = lock_config(&state);
                    cfg.tgws.autostart
                };
                if enabled {
                    let handle = app.handle().clone();
                    std::thread::spawn(move || {
                        // A fresh install has no binary yet: fetch it first, then
                        // start. Progress and failures are published as
                        // `tgws-progress` / `tgws-notice`, the rest via status.
                        if crate::tgws::ensure_installed(&handle).is_ok() {
                            let still_enabled = {
                                let state = handle.state::<AppState>();
                                let cfg = lock_config(&state);
                                cfg.tgws.autostart
                            };
                            if still_enabled {
                                let _ = crate::tgws::start(&handle);
                            }
                        }
                    });
                }
            }

            // On autostart, start hidden
            if is_autostart {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
                // Start last strategy silently (if the service isn't already running).
                let state = app.state::<AppState>();
                let cfg = lock_config(&state);
                let last = cfg.last_strategy.clone();
                let autostart_enabled = cfg.autostart;
                drop(cfg);
                if autostart_enabled && !last.is_empty() && !crate::engine::Engine::is_running() {
                    let _ = crate::commands::start(state);
                }
            } else {
                // Normal launch: show the window on a visible monitor.
                show_main_window(app.handle());
            }

            // Self-heal the autostart Run key: if the user enabled autostart but the
            // value is missing or points to a different exe path, recreate it.
            {
                let cfg = app.state::<AppState>();
                let autostart_enabled = lock_config(&cfg).autostart;
                if autostart_enabled {
                    if let Ok(exe) = std::env::current_exe() {
                        let exe_str = exe.to_string_lossy().to_string();
                        let task_points_elsewhere = crate::autostart::registered_exe()
                            .map(|p| !p.eq_ignore_ascii_case(&exe_str))
                            .unwrap_or(true);
                        if !crate::autostart::is_installed() || task_points_elsewhere {
                            let _ = crate::autostart::install(&exe_str);
                        }
                    }
                }
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // The Telegram proxy is a child process: never leave it running
            // after the window is gone, otherwise port 1443 stays occupied.
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                // Telegram proxy: a child process, stopped through the job object.
                crate::tgws::stop_on_exit(app_handle);
                // DPI bypass: a Windows service, stopped through the SCM (never
                // by spawning `sc.exe`, which fails during session shutdown).
                crate::engine::Engine::stop_service();
            }
        });
}

#[cfg(test)]
mod lock_tests {
    use super::lock_unpoisoned;
    use std::sync::{Arc, Mutex};

    #[test]
    fn recovers_a_poisoned_mutex() {
        let mutex = Arc::new(Mutex::new(42));
        let worker = mutex.clone();
        let _ = std::thread::spawn(move || {
            let _guard = worker.lock().unwrap();
            panic!("poison the lock");
        })
        .join();
        assert_eq!(*lock_unpoisoned(&mutex), 42);
    }
}
