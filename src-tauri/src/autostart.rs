//! Autostart registration.
//!
//! Windows starts the app at logon through a Task Scheduler task with
//! `RunLevel=HighestAvailable`. Unlike the HKCU Run key, a high-runlevel task
//! starts the app already elevated, so a machine with UAC enabled does not show
//! a confirmation prompt at every logon.
//!
//! `schtasks.exe` is always started with separate array arguments from an XML
//! file: no `cmd /c`, no command line assembled by hand. The XML file lives in
//! the app's `tmp` folder only for the duration of the call.
//!
//! When the task cannot be created (an unelevated debug build, a policy, an
//! antivirus) the app falls back to the Run key and warns the user that Windows
//! will ask for confirmation at logon — autostart never breaks silently.

use crate::registry;
use crate::sys;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use windows_sys::Win32::System::Registry::HKEY_CURRENT_USER;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Historical value name: renaming it would orphan the setting of every user.
const VALUE_NAME: &str = "zapret-ui";
pub const TASK_NAME: &str = "Splitline";
/// Command-line flag that tells the app it was started at logon (hide the
/// window). Shared with the task XML so the two can never drift apart.
pub const AUTOSTART_FLAG: &str = "--autostart";
/// Autostart task of versions before 2.0; removed when it is still around.
const LEGACY_TASK_NAME: &str = "zapret-ui";
/// Logon delay: Explorer must be running before the app registers its tray
/// icon, otherwise the icon can stay missing for the whole session.
const LOGON_DELAY: &str = "PT20S";
const WARNING_FALLBACK: &str = "Автозапуск включён, но Windows запросит подтверждение при входе.";

/// How autostart is registered after a successful toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutostartMode {
    /// Task Scheduler task: starts elevated, no UAC prompt.
    Task,
    /// HKCU Run key: starts unelevated, so Windows asks for confirmation.
    RunKey,
    /// Autostart is off.
    Disabled,
}

/// Result of `set_autostart`, shown by the UI as an optional warning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutostartStatus {
    pub mode: AutostartMode,
    pub warning: Option<String>,
}

impl AutostartStatus {
    fn task() -> Self {
        Self {
            mode: AutostartMode::Task,
            warning: None,
        }
    }

    fn run_key() -> Self {
        Self {
            mode: AutostartMode::RunKey,
            warning: Some(WARNING_FALLBACK.to_string()),
        }
    }

    pub fn disabled() -> Self {
        Self {
            mode: AutostartMode::Disabled,
            warning: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Task XML
// ---------------------------------------------------------------------------

/// Escapes text for an XML text node or attribute value.
pub fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Reverses [`xml_escape`] for the entities Task Scheduler emits.
fn xml_unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        let Some(end) = after.find(';') else {
            out.push('&');
            rest = after;
            continue;
        };
        // Entity names are short; a longer run means a literal ampersand.
        if end > 8 {
            out.push('&');
            rest = after;
            continue;
        }
        let entity = &after[..end];
        match entity {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let code = entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| {
                        entity
                            .strip_prefix('#')
                            .and_then(|dec| dec.parse::<u32>().ok())
                    })
                    .and_then(char::from_u32);
                match code {
                    Some(ch) => out.push(ch),
                    None => {
                        out.push('&');
                        out.push_str(entity);
                        out.push(';');
                    }
                }
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Text of the first `<tag>…</tag>` element.
fn element_text<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)?;
    Some(xml[start..start + end].trim())
}

/// Executable path from a task XML produced by `schtasks /Query /XML`.
pub fn task_command(xml: &str) -> Option<String> {
    let path = xml_unescape(element_text(xml, "Command")?);
    let path = path.trim().trim_matches('"').trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

/// Whether the task already starts exactly this executable.
fn task_matches(xml: Option<&str>, exe: &str) -> bool {
    xml.and_then(task_command)
        .map(|registered| registered.eq_ignore_ascii_case(exe))
        .unwrap_or(false)
}

/// Task XML: logon trigger, highest available run level, one action.
///
/// `user` scopes both the trigger and the principal to the account that owns
/// the autostart; without it Task Scheduler falls back to the current user and
/// fires the trigger on every logon.
pub fn task_xml(exe: &str, workdir: &str, user: Option<&str>) -> String {
    let user = user.map(|value| format!("      <UserId>{}</UserId>\n", xml_escape(value)));
    let trigger_user = user.clone().unwrap_or_default();
    let principal_user = user.unwrap_or_default();

    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Splitline — запуск при входе в Windows</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
{trigger_user}      <Delay>{delay}</Delay>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
{principal_user}      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>false</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>{flag}</Arguments>
      <WorkingDirectory>{workdir}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#,
        trigger_user = trigger_user,
        principal_user = principal_user,
        delay = LOGON_DELAY,
        flag = AUTOSTART_FLAG,
        exe = xml_escape(exe),
        workdir = xml_escape(workdir),
    )
}

/// Task file body: UTF-16LE with a BOM, matching the declared encoding, so
/// Cyrillic paths and the Russian description survive `schtasks`.
fn encode_task_file(xml: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(xml.len() * 2 + 2);
    bytes.extend_from_slice(&[0xFF, 0xFE]);
    for unit in xml.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

/// `schtasks /Query /XML` writes UTF-16LE to stdout even when it is a pipe.
fn decode_console_output(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16le(rest);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if looks_like_utf16le(bytes) {
        return decode_utf16le(bytes);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes).to_string()
}

fn decode_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

fn looks_like_utf16le(bytes: &[u8]) -> bool {
    // ASCII XML in UTF-16LE has a zero high byte every other byte.
    let mut pairs = 0;
    let mut zero_high = 0;
    for pair in bytes.chunks_exact(2).take(64) {
        pairs += 1;
        if pair[1] == 0 {
            zero_high += 1;
        }
    }
    pairs > 4 && zero_high * 2 > pairs
}

// ---------------------------------------------------------------------------
// schtasks arguments
// ---------------------------------------------------------------------------

fn create_args(xml_path: &str) -> Vec<String> {
    vec![
        "/Create".into(),
        "/TN".into(),
        TASK_NAME.into(),
        "/XML".into(),
        xml_path.into(),
        "/F".into(),
    ]
}

fn query_args() -> Vec<String> {
    vec![
        "/Query".into(),
        "/TN".into(),
        TASK_NAME.into(),
        "/XML".into(),
    ]
}

fn delete_args() -> Vec<String> {
    vec![
        "/Delete".into(),
        "/TN".into(),
        TASK_NAME.into(),
        "/F".into(),
    ]
}

fn legacy_delete_args() -> Vec<String> {
    vec![
        "/Delete".into(),
        "/TN".into(),
        LEGACY_TASK_NAME.into(),
        "/F".into(),
    ]
}

/// Run key value data for the fallback: quoted exe path + the autostart flag.
fn run_value(exe: &str) -> String {
    format!("\"{exe}\" {AUTOSTART_FLAG}")
}

/// Account that owns the autostart, as `DOMAIN\User`.
///
/// The name is resolved by Task Scheduler when the task is created; without a
/// SID API call this is the least invasive way to scope the logon trigger.
fn current_user() -> Option<String> {
    let user = std::env::var("USERNAME").ok()?;
    if user.trim().is_empty() {
        return None;
    }
    match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.trim().is_empty() => Some(format!("{domain}\\{user}")),
        _ => Some(user),
    }
}

fn split_paths(exe: &Path) -> (String, String) {
    let exe = exe.to_string_lossy().to_string();
    let workdir = Path::new(&exe)
        .parent()
        .map(|parent| parent.to_string_lossy().to_string())
        .unwrap_or_else(|| exe.clone());
    (exe, workdir)
}

// ---------------------------------------------------------------------------
// System backend
// ---------------------------------------------------------------------------

/// Side effects, split out so the fallback and self-heal logic is testable
/// without touching the real Task Scheduler or the registry.
trait Backend {
    fn create_task(&self, xml: &str) -> Result<(), String>;
    fn delete_task(&self) -> Result<(), String>;
    fn task_xml(&self) -> Option<String>;
    /// Best effort: the legacy task is usually absent, so a non-zero exit is
    /// not an error worth reporting.
    fn delete_legacy_task(&self);
    fn read_run(&self) -> Option<String>;
    fn write_run(&self, value: &str) -> Result<(), String>;
    fn delete_run(&self) -> bool;
}

struct WindowsBackend;

impl Backend for WindowsBackend {
    fn create_task(&self, xml: &str) -> Result<(), String> {
        let path = write_task_file(xml)?;
        let result = run_schtasks(&create_args(&path.to_string_lossy()));
        // The XML only has to exist while schtasks reads it.
        let _ = fs::remove_file(&path);
        let output = result?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "Планировщик заданий не создал задачу «{TASK_NAME}» (код {}).",
                exit_code(&output)
            ))
        }
    }

    fn delete_task(&self) -> Result<(), String> {
        let output = run_schtasks(&delete_args())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "Планировщик заданий не удалил задачу «{TASK_NAME}» (код {}).",
                exit_code(&output)
            ))
        }
    }

    fn task_xml(&self) -> Option<String> {
        let output = run_schtasks(&query_args()).ok()?;
        if !output.status.success() {
            return None;
        }
        let xml = decode_console_output(&output.stdout);
        if xml.trim().is_empty() {
            None
        } else {
            Some(xml)
        }
    }

    fn delete_legacy_task(&self) {
        let _ = run_schtasks(&legacy_delete_args());
    }

    fn read_run(&self) -> Option<String> {
        registry::read_string(HKEY_CURRENT_USER, RUN_KEY, VALUE_NAME)
    }

    fn write_run(&self, value: &str) -> Result<(), String> {
        registry::write_string(HKEY_CURRENT_USER, RUN_KEY, VALUE_NAME, value)
            .map_err(|error| format!("Не удалось записать автозапуск в реестр: {error}"))
    }

    fn delete_run(&self) -> bool {
        registry::delete_value(HKEY_CURRENT_USER, RUN_KEY, VALUE_NAME)
    }
}

fn write_task_file(xml: &str) -> Result<PathBuf, String> {
    // The self-heal thread and a UI toggle can run at the same time; a unique
    // name keeps one call from deleting the XML another call is still reading.
    static SEQUENCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let dir = crate::config::app_dir().join("tmp");
    fs::create_dir_all(&dir)
        .map_err(|error| format!("Не удалось подготовить папку для задачи автозапуска: {error}"))?;
    let path = dir.join(format!("autostart-{}-{sequence}.xml", std::process::id()));
    fs::write(&path, encode_task_file(xml))
        .map_err(|error| format!("Не удалось записать файл задачи автозапуска: {error}"))?;
    Ok(path)
}

fn run_schtasks(args: &[String]) -> Result<Output, String> {
    if sys::session_ending() {
        return Err("Windows завершает работу, Планировщик заданий не изменён.".into());
    }
    sys::command("schtasks.exe")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("Не удалось запустить Планировщик заданий: {error}"))
}

fn exit_code(output: &Output) -> String {
    output
        .status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "неизвестен".to_string())
}

// ---------------------------------------------------------------------------
// Install / remove / self-heal
// ---------------------------------------------------------------------------

fn install_with(
    backend: &impl Backend,
    exe: &str,
    workdir: &str,
    user: Option<&str>,
) -> Result<AutostartStatus, String> {
    backend.delete_legacy_task();

    let xml = task_xml(exe, workdir, user);
    if backend.create_task(&xml).is_ok() {
        // R7: the Run key is no longer needed. Leaving it would start a second
        // copy of the app at logon.
        backend.delete_run();
        return Ok(AutostartStatus::task());
    }

    // R6: a task may be impossible to create — an unelevated debug build, a
    // restrictive policy, an antivirus. The Run key still works, only with a
    // UAC prompt at logon, so fall back instead of leaving autostart broken.
    backend.write_run(&run_value(exe)).map_err(|error| {
        format!(
            "Не удалось включить автозапуск: Windows не дал создать задачу Планировщика, \
             а запись в реестр не удалась. {error}"
        )
    })?;
    Ok(AutostartStatus::run_key())
}

fn remove_with(backend: &impl Backend) -> Result<(), String> {
    backend.delete_legacy_task();
    // A missing task makes `schtasks /Delete` fail; the query below decides
    // whether the removal actually worked, so that error is not reported here.
    let delete_error = backend.delete_task().err();
    backend.delete_run();

    if backend.task_xml().is_some() {
        return Err(match delete_error {
            Some(error) => format!("Не удалось удалить задачу автозапуска: {error}"),
            None => format!("Не удалось удалить задачу автозапуска «{TASK_NAME}»."),
        });
    }
    if backend.read_run().is_some() {
        return Err("Не удалось удалить запись автозапуска из реестра.".into());
    }
    Ok(())
}

fn repair_with(
    backend: &impl Backend,
    exe: &str,
    workdir: &str,
    user: Option<&str>,
) -> Result<AutostartStatus, String> {
    if task_matches(backend.task_xml().as_deref(), exe) {
        // R8: the task is already correct, do not recreate it. A stale Run
        // value (left from a fallback) would start a second copy — drop it.
        backend.delete_run();
        return Ok(AutostartStatus::task());
    }
    install_with(backend, exe, workdir, user)
}

/// Creates the autostart task for the current exe, or falls back to the Run key.
pub fn install(exe: &Path) -> Result<AutostartStatus, String> {
    let (exe, workdir) = split_paths(exe);
    install_with(&WindowsBackend, &exe, &workdir, current_user().as_deref())
}

/// Removes both mechanisms (task and Run key).
pub fn remove() -> Result<(), String> {
    remove_with(&WindowsBackend)
}

/// Startup self-heal: recreates the task when it is missing or starts another
/// exe (the app is portable and the folder may have moved).
pub fn repair_if_needed(exe: &Path) -> Result<AutostartStatus, String> {
    let (exe, workdir) = split_paths(exe);
    repair_with(&WindowsBackend, &exe, &workdir, current_user().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const EXE: &str = r"C:\Apps\Splitline\Splitline.exe";
    const WORKDIR: &str = r"C:\Apps\Splitline";

    /// In-memory backend: no real `schtasks` call and no registry write.
    #[derive(Default)]
    struct FakeBackend {
        task: RefCell<Option<String>>,
        run: RefCell<Option<String>>,
        fail_create: bool,
        fail_write: bool,
        fail_delete: bool,
        create_calls: RefCell<usize>,
    }

    impl FakeBackend {
        fn with_task(xml: &str) -> Self {
            let backend = Self::default();
            *backend.task.borrow_mut() = Some(xml.to_string());
            backend
        }

        fn create_calls(&self) -> usize {
            *self.create_calls.borrow()
        }

        fn task(&self) -> Option<String> {
            self.task.borrow().clone()
        }

        fn run(&self) -> Option<String> {
            self.run.borrow().clone()
        }
    }

    impl Backend for FakeBackend {
        fn create_task(&self, xml: &str) -> Result<(), String> {
            *self.create_calls.borrow_mut() += 1;
            if self.fail_create {
                return Err("доступ запрещён".into());
            }
            *self.task.borrow_mut() = Some(xml.to_string());
            Ok(())
        }

        fn delete_task(&self) -> Result<(), String> {
            if self.fail_delete {
                return Err("доступ запрещён".into());
            }
            *self.task.borrow_mut() = None;
            Ok(())
        }

        fn task_xml(&self) -> Option<String> {
            self.task.borrow().clone()
        }

        fn delete_legacy_task(&self) {}

        fn read_run(&self) -> Option<String> {
            self.run.borrow().clone()
        }

        fn write_run(&self, value: &str) -> Result<(), String> {
            if self.fail_write {
                return Err("доступ запрещён".into());
            }
            *self.run.borrow_mut() = Some(value.to_string());
            Ok(())
        }

        fn delete_run(&self) -> bool {
            let had_value = self.run.borrow().is_some();
            *self.run.borrow_mut() = None;
            had_value
        }
    }

    // -- task XML ----------------------------------------------------------

    #[test]
    fn task_xml_has_a_logon_trigger_and_the_highest_run_level() {
        let xml = task_xml(EXE, WORKDIR, Some(r"PC\user"));

        assert!(xml.starts_with(r#"<?xml version="1.0" encoding="UTF-16"?>"#));
        assert!(xml.contains("<LogonTrigger>"));
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(xml.contains("<UserId>PC\\user</UserId>"));
        assert!(xml.contains(r"<Command>C:\Apps\Splitline\Splitline.exe</Command>"));
        assert!(xml.contains("<Arguments>--autostart</Arguments>"));
        assert!(xml.contains(r"<WorkingDirectory>C:\Apps\Splitline</WorkingDirectory>"));
    }

    #[test]
    fn task_xml_omits_the_user_when_it_is_unknown() {
        let xml = task_xml(EXE, WORKDIR, None);
        assert!(!xml.contains("<UserId>"));
    }

    #[test]
    fn task_xml_escapes_quotes_ampersands_and_keeps_unicode() {
        let exe = "C:\\Программы\\Split & Co \"beta\" <x>\\Splitline.exe";
        let workdir = "C:\\Программы\\O'Brien";

        let xml = task_xml(exe, workdir, Some("ПК\\Пользователь"));

        assert!(xml.contains("&amp;"));
        assert!(xml.contains("&quot;"));
        assert!(xml.contains("&lt;"));
        assert!(xml.contains("&apos;"));
        assert!(xml.contains("Программы"));
        assert!(xml.contains("ПК\\Пользователь"));
        assert!(!xml.contains("Split & Co"));
        // The self-heal comparison parses the file back: escaping must round-trip.
        assert_eq!(task_command(&xml).as_deref(), Some(exe));
    }

    #[test]
    fn task_name_and_arguments_are_stable() {
        assert_eq!(TASK_NAME, "Splitline");
        assert_eq!(AUTOSTART_FLAG, "--autostart");
        assert_eq!(
            create_args(r"C:\Temp\autostart.xml"),
            vec![
                "/Create",
                "/TN",
                "Splitline",
                "/XML",
                r"C:\Temp\autostart.xml",
                "/F"
            ]
        );
        assert_eq!(query_args(), vec!["/Query", "/TN", "Splitline", "/XML"]);
        assert_eq!(delete_args(), vec!["/Delete", "/TN", "Splitline", "/F"]);
    }

    #[test]
    fn run_key_fallback_starts_the_exe_with_the_autostart_flag() {
        assert_eq!(run_value(EXE), format!("\"{EXE}\" --autostart"));
    }

    #[test]
    fn arguments_are_separate_elements_so_paths_keep_their_spaces() {
        let xml_path = r"C:\Users\Test User\AppData\Local\Temp\autostart-task.xml";
        let args = create_args(xml_path);

        // No shell: every argument is its own element, nothing is glued.
        assert!(args.iter().any(|arg| arg == xml_path));
        assert!(!args.iter().any(|arg| arg.contains("cmd.exe")));
        assert_eq!(args.iter().filter(|arg| arg.as_str() == "/TN").count(), 1);
    }

    #[test]
    fn task_command_reads_escaped_paths_from_the_query_output() {
        let xml = r#"<Task><Actions Context="Author"><Exec>
            <Command>C:\Tools &amp; Games\Splitline.exe</Command>
            <Arguments>--autostart</Arguments>
        </Exec></Actions></Task>"#;

        assert_eq!(
            task_command(xml).as_deref(),
            Some(r"C:\Tools & Games\Splitline.exe")
        );
        assert_eq!(task_command("<Task></Task>"), None);
    }

    #[test]
    fn task_matches_the_current_exe_case_insensitively() {
        let xml = task_xml(r"c:\apps\splitline.exe", WORKDIR, None);

        assert!(task_matches(Some(xml.as_str()), r"C:\Apps\Splitline.exe"));
        assert!(!task_matches(Some(xml.as_str()), r"C:\Other\Splitline.exe"));
        assert!(!task_matches(None, EXE));
    }

    // -- file encoding -----------------------------------------------------

    #[test]
    fn task_file_is_utf16_with_a_bom() {
        let xml = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><Task>Привет</Task>";
        let bytes = encode_task_file(xml);

        assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
        assert_eq!(decode_console_output(&bytes), xml);
    }

    #[test]
    fn console_output_decoding_handles_utf16_and_utf8() {
        let xml = "<Task>привет</Task>";
        let mut utf16 = Vec::new();
        for unit in xml.encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }

        assert_eq!(decode_console_output(&utf16), xml);
        assert_eq!(decode_console_output(xml.as_bytes()), xml);
    }

    // -- install / fallback ------------------------------------------------

    #[test]
    fn install_creates_the_task_and_drops_the_old_run_key() {
        let backend = FakeBackend::default();
        *backend.run.borrow_mut() = Some(run_value(EXE));

        let status = install_with(&backend, EXE, WORKDIR, None).expect("install");

        assert_eq!(status.mode, AutostartMode::Task);
        assert_eq!(status.warning, None);
        assert_eq!(
            task_command(&backend.task().expect("task created")).as_deref(),
            Some(EXE)
        );
        assert_eq!(backend.run(), None, "the old Run key must be migrated away");

        let installed = backend.task().unwrap();
        assert!(installed.contains("<Arguments>--autostart</Arguments>"));
    }

    #[test]
    fn install_falls_back_to_the_run_key_when_the_task_cannot_be_created() {
        let backend = FakeBackend {
            fail_create: true,
            ..Default::default()
        };

        let status = install_with(&backend, EXE, WORKDIR, None).expect("fallback");

        assert_eq!(status.mode, AutostartMode::RunKey);
        assert_eq!(status.warning.as_deref(), Some(WARNING_FALLBACK));
        assert_eq!(backend.run().as_deref(), Some(run_value(EXE).as_str()));
        assert_eq!(backend.task(), None);
    }

    #[test]
    fn install_reports_an_error_when_both_mechanisms_fail() {
        let backend = FakeBackend {
            fail_create: true,
            fail_write: true,
            ..Default::default()
        };

        let error = install_with(&backend, EXE, WORKDIR, None).expect_err("must fail");

        assert!(error.contains("Не удалось включить автозапуск"));
    }

    #[test]
    fn install_is_idempotent() {
        let backend = FakeBackend::default();

        install_with(&backend, EXE, WORKDIR, None).expect("first");
        let status = install_with(&backend, EXE, WORKDIR, None).expect("second");

        assert_eq!(status.mode, AutostartMode::Task);
        assert_eq!(
            backend.create_calls(),
            2,
            "/F overwrites, it does not duplicate"
        );
        assert!(backend.task().is_some());
        assert_eq!(backend.run(), None);
    }

    // -- remove ------------------------------------------------------------

    #[test]
    fn remove_deletes_the_task_and_the_run_key() {
        let backend = FakeBackend::with_task(&task_xml(EXE, WORKDIR, None));
        *backend.run.borrow_mut() = Some(run_value(EXE));

        remove_with(&backend).expect("remove");

        assert_eq!(backend.task(), None);
        assert_eq!(backend.run(), None);
    }

    #[test]
    fn remove_is_idempotent_when_nothing_is_registered() {
        let backend = FakeBackend::default();

        remove_with(&backend).expect("nothing to remove");
        remove_with(&backend).expect("second remove");
    }

    #[test]
    fn remove_reports_a_task_that_stayed_in_place() {
        let mut backend = FakeBackend::with_task(&task_xml(EXE, WORKDIR, None));
        backend.fail_delete = true;

        let error = remove_with(&backend).expect_err("must fail");

        assert!(error.contains("Не удалось удалить задачу автозапуска"));
        assert!(backend.task().is_some());
    }

    // -- self-heal ---------------------------------------------------------

    #[test]
    fn repair_recreates_a_missing_task() {
        let backend = FakeBackend::default();

        let status = repair_with(&backend, EXE, WORKDIR, None).expect("repair");

        assert_eq!(status.mode, AutostartMode::Task);
        assert_eq!(backend.create_calls(), 1);
    }

    #[test]
    fn repair_recreates_a_task_that_starts_another_exe() {
        let stale = task_xml(r"C:\Old\Splitline.exe", r"C:\Old", None);
        let backend = FakeBackend::with_task(&stale);

        let status = repair_with(&backend, EXE, WORKDIR, None).expect("repair");

        assert_eq!(status.mode, AutostartMode::Task);
        assert_eq!(backend.create_calls(), 1);
        assert_eq!(
            task_command(&backend.task().expect("task")).as_deref(),
            Some(EXE)
        );
    }

    #[test]
    fn repair_leaves_a_current_task_alone_and_drops_a_stale_run_key() {
        let backend = FakeBackend::with_task(&task_xml(EXE, WORKDIR, None));
        *backend.run.borrow_mut() = Some(run_value(EXE));

        let status = repair_with(&backend, EXE, WORKDIR, None).expect("repair");

        assert_eq!(status.mode, AutostartMode::Task);
        assert_eq!(
            backend.create_calls(),
            0,
            "no needless recreation on every start"
        );
        assert_eq!(
            backend.run(),
            None,
            "a stale Run key would start a second copy"
        );
    }

    #[test]
    fn repair_falls_back_when_recreation_fails() {
        let stale = task_xml(r"C:\Old\Splitline.exe", r"C:\Old", None);
        let mut backend = FakeBackend::with_task(&stale);
        backend.fail_create = true;

        let status = repair_with(&backend, EXE, WORKDIR, None).expect("fallback");

        assert_eq!(status.mode, AutostartMode::RunKey);
        assert_eq!(backend.run().as_deref(), Some(run_value(EXE).as_str()));
    }
}
