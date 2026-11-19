use crate::registry;
use crate::sys;
use std::process::Stdio;
use windows_sys::Win32::System::Registry::HKEY_CURRENT_USER;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "zapret-ui";
const OLD_TASK_NAME: &str = "zapret-ui";

/// Remove the old scheduled-task autostart mechanism (migration to the Run key).
fn remove_old_scheduled_task() {
    let _ = sys::command("schtasks.exe")
        .args(["/Delete", "/TN", OLD_TASK_NAME, "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// The Run-key value data: quoted exe path + the autostart flag.
fn run_value(exe: &str) -> String {
    format!("\"{exe}\" --autostart")
}

/// Register autostart via the HKCU Run key, so the app shows up in the
/// Task Manager "Startup apps" list (same mechanism as other portable apps).
///
/// The registry is written directly instead of spawning `reg.exe`: a child
/// process started during logon or logoff fails with 0xc0000142.
pub fn install(exe: &str) -> Result<(), String> {
    remove_old_scheduled_task();

    registry::write_string(HKEY_CURRENT_USER, RUN_KEY, VALUE_NAME, &run_value(exe))
        .map_err(|e| format!("Run key: {e}"))?;

    if is_installed() {
        Ok(())
    } else {
        Err("Не удалось включить автозапуск (проверка не прошла).".into())
    }
}

pub fn remove() -> Result<(), String> {
    remove_old_scheduled_task();
    registry::delete_value(HKEY_CURRENT_USER, RUN_KEY, VALUE_NAME);
    if is_installed() {
        Err("Не удалось отключить автозапуск: запись Run осталась.".into())
    } else {
        Ok(())
    }
}

pub fn is_installed() -> bool {
    registered_exe().is_some()
}

/// The executable path currently registered in the Run key, if any.
pub fn registered_exe() -> Option<String> {
    let data = registry::read_string(HKEY_CURRENT_USER, RUN_KEY, VALUE_NAME)?;
    parse_run_value(&data)
}

/// Extract the exe path from a Run value such as `"C:\Apps\app.exe" --autostart`.
fn parse_run_value(data: &str) -> Option<String> {
    let data = data.trim();
    if data.is_empty() {
        return None;
    }
    if let Some(stripped) = data.strip_prefix('"') {
        return stripped.split('"').next().map(|s| s.to_string());
    }
    data.split_whitespace().next().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::{parse_run_value, run_value};

    #[test]
    fn run_value_quotes_exe_and_flag() {
        let exe = r"C:\Program Files\zapret-ui\zapret-ui.exe";
        assert_eq!(run_value(exe), format!("\"{exe}\" --autostart"));
    }

    #[test]
    fn parse_run_value_extracts_path() {
        let sample = r#""C:\Users\Test\IdeaProjects\zapret-ui\zapret-ui.exe" --autostart"#;
        assert_eq!(
            parse_run_value(sample).as_deref(),
            Some(r"C:\Users\Test\IdeaProjects\zapret-ui\zapret-ui.exe")
        );
    }

    #[test]
    fn parse_run_value_handles_an_unquoted_path() {
        assert_eq!(
            parse_run_value(r"C:\Apps\zapret-ui.exe --autostart").as_deref(),
            Some(r"C:\Apps\zapret-ui.exe")
        );
    }

    #[test]
    fn parse_run_value_none_when_empty() {
        assert_eq!(parse_run_value("   "), None);
    }
}
