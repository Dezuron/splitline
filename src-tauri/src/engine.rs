use crate::sys;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_MANAGER_CONNECT,
    SC_STATUS_PROCESS_INFO, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS_PROCESS,
};

const SERVICE_NAME: &str = "zapret";

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Current state of a service, read through the Service Control Manager.
/// `None` means the service does not exist (or cannot be queried).
///
/// The UI polls this every few seconds, so it must not spawn `sc.exe`: during
/// Windows shutdown a spawned console tool fails to start with `0xc0000142` and
/// Windows shows a "taskkill.exe/sc.exe — Ошибка приложения" dialog.
fn service_state(name: &str) -> Option<u32> {
    let name = wide(name);
    unsafe {
        let manager = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return None;
        }
        let service = OpenServiceW(manager, name.as_ptr(), SERVICE_QUERY_STATUS);
        CloseServiceHandle(manager);
        if service.is_null() {
            return None;
        }
        let mut status: SERVICE_STATUS_PROCESS = std::mem::zeroed();
        let mut needed = 0u32;
        let queried = QueryServiceStatusEx(
            service,
            SC_STATUS_PROCESS_INFO,
            &mut status as *mut _ as *mut u8,
            std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut needed,
        );
        CloseServiceHandle(service);
        (queried != 0).then_some(status.dwCurrentState)
    }
}

fn image_of(entry: &PROCESSENTRY32W) -> String {
    let name: Vec<u16> = entry
        .szExeFile
        .iter()
        .take_while(|unit| **unit != 0)
        .copied()
        .collect();
    String::from_utf16_lossy(&name)
}

/// Whether a process with this image name is running, read from a Toolhelp
/// snapshot instead of spawning `tasklist.exe` (same shutdown reasoning as
/// [`service_state`]).
fn image_running(image: &str) -> bool {
    unsafe {
        let snapshot: HANDLE = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut next = Process32FirstW(snapshot, &mut entry);
        while next != 0 {
            if image_of(&entry).eq_ignore_ascii_case(image) {
                found = true;
                break;
            }
            next = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        found
    }
}

/// Manages the DPI bypass as the Windows service `zapret` — the same principle as
/// service.bat "Install Service" / "Remove Services".
pub struct Engine;

impl Engine {
    fn quiet(mut cmd: Command) -> Command {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd
    }

    /// Runs a System32 tool quietly and reports whether it exited successfully.
    fn run(name: &str, args: &[&str]) -> bool {
        Self::quiet(sys::command(name))
            .args(args)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// Install winws as the `zapret` Windows service (start=auto) and start it.
    /// Mirrors service.bat: cleanup -> sc create -> sc start -> remember strategy.
    pub fn install(exe: &str, args: &[String], strategy: &str) -> Result<(), String> {
        // Clean any previous service/driver state first.
        let _ = Self::remove();

        // Enable TCP timestamps (same as service.bat :tcp_enable).
        let _ = Self::run(
            "netsh.exe",
            &["interface", "tcp", "set", "global", "timestamps=enabled"],
        );

        // binPath value must be a single sc argument: outer quotes wrapping the
        // inner-quoted exe path and the arguments.
        let bin_path = format!("\"\\\"{exe}\\\" {}\"", args.join(" "));

        // `sc delete` is asynchronous: the service is only "marked for deletion"
        // until its last handle closes, so `sc create` can briefly fail with 1072.
        let mut created = false;
        for attempt in 0..3 {
            let mut cmd = Self::quiet(sys::command("sc.exe"));
            cmd.arg("create")
                .arg(SERVICE_NAME)
                .arg("binPath=")
                .raw_arg(&bin_path)
                .arg("DisplayName=")
                .arg("zapret")
                .arg("start=")
                .arg("auto");
            if cmd.status().map(|s| s.success()).unwrap_or(false) {
                created = true;
                break;
            }
            if attempt < 2 {
                std::thread::sleep(Duration::from_millis(400));
            }
        }
        if !created {
            return Err("Не удалось создать службу zapret.".into());
        }

        let _ = Self::run(
            "sc.exe",
            &["description", SERVICE_NAME, "Zapret DPI bypass software"],
        );

        if !Self::run("sc.exe", &["start", SERVICE_NAME]) {
            return Err("Не удалось запустить службу zapret.".into());
        }

        // Remember the strategy in the service registry key (like the bat).
        let _ = Self::run(
            "reg.exe",
            &[
                "add",
                r"HKLM\System\CurrentControlSet\Services\zapret",
                "/v",
                "zapret-discord-youtube",
                "/t",
                "REG_SZ",
                "/d",
                strategy,
                "/f",
            ],
        );
        Ok(())
    }

    /// Stop and remove the service, kill orphaned winws, and remove the WinDivert
    /// driver service (mirrors service.bat "Remove Services").
    pub fn remove() -> Result<(), String> {
        let _ = Self::run("net.exe", &["stop", SERVICE_NAME]);
        let _ = Self::run("sc.exe", &["delete", SERVICE_NAME]);

        Self::kill_by_image();

        for svc in ["WinDivert", "WinDivert14"] {
            let _ = Self::run("net.exe", &["stop", svc]);
            let _ = Self::run("sc.exe", &["delete", svc]);
        }

        // `sc delete` is asynchronous: wait until the service is really gone
        // so Stop can report failure instead of claiming success.
        for _ in 0..12 {
            if !Self::is_installed() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        if Self::is_installed() {
            return Err("Не удалось удалить службу zapret (она ещё останавливается).".into());
        }
        Ok(())
    }

    /// Whether the `zapret` service exists (running or stopped).
    ///
    /// Answered through the Service Control Manager instead of spawning
    /// `sc.exe`: the UI polls this every few seconds, and during Windows
    /// shutdown a spawned tool fails with `0xc0000142`.
    pub fn is_installed() -> bool {
        service_state(SERVICE_NAME).is_some()
    }

    /// Whether the `zapret` service is installed and running.
    pub fn is_running() -> bool {
        service_state(SERVICE_NAME) == Some(SERVICE_RUNNING)
    }

    /// Kill any winws.exe process by image name (fallback for orphaned processes).
    /// Runs synchronously: the bench has to be sure the previous candidate is gone
    /// before the next one starts.
    pub fn kill_by_image() {
        let _ = Self::run("taskkill.exe", &["/IM", "winws.exe", "/F"]);
    }

    /// Start `winws.exe` as an invisible child process with the given arguments.
    /// Used by the bench, which runs candidates without touching the service:
    /// recreating the service (and the WinDivert driver) for every candidate
    /// would be slow and fragile.
    pub fn spawn_transient(exe: &str, args: &[String]) -> Result<(), String> {
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        sys::hidden(&mut cmd);
        if let Some(dir) = std::path::Path::new(exe).parent() {
            cmd.current_dir(dir);
        }
        let child = cmd
            .spawn()
            .map_err(|e| format!("Не удалось запустить winws.exe: {e}"))?;
        // Dies with the app even if it is terminated during shutdown.
        crate::job::adopt(&child);
        Ok(())
    }

    /// Wait until any winws.exe process appears, or the timeout elapses.
    pub fn wait_winws_ready(timeout: Duration) -> bool {
        let started = std::time::Instant::now();
        while started.elapsed() < timeout {
            if Self::is_any_winws_running() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    /// Whether any winws.exe process is running (snapshot fallback: the bench
    /// spawns transient processes that are not a service).
    pub fn is_any_winws_running() -> bool {
        image_running("winws.exe")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_state_comes_from_the_scm() {
        // `EventLog` ships with Windows and is always running.
        assert_eq!(service_state("EventLog"), Some(SERVICE_RUNNING));
        assert_eq!(service_state("zapret-ui-nonexistent-service"), None);
    }

    #[test]
    fn image_scan_finds_a_running_process() {
        let own = std::env::current_exe()
            .expect("current exe")
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .to_string();
        assert!(image_running(&own), "own image {own} must be visible");
        assert!(!image_running("zapret-ui-nonexistent-image.exe"));
    }
}
