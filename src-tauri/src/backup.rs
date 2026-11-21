//! Export and import of everything the user configured.
//!
//! One zip holds `config.json`, the `*-user.txt` lists, experimental strategies,
//! benchmark history and (optionally) the subscription snapshot. Binaries, logs,
//! the runtime itself and `tmp\` never go in: they are machine-specific and can
//! be downloaded again.
//!
//! `manifest.json` records the schema version, the application version, the
//! creation date and a SHA-256 for every file. Import verifies all of that before
//! the first byte is written, and every zip entry is resolved through
//! `enclosed_name`, so an archive cannot escape the data folder.
//!
//! Nothing here touches the network and nothing depends on the current user's
//! profile: every source and destination folder is an argument, which is what
//! makes the whole module testable in temporary directories.

use crate::config::{self, AppConfig};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

/// Archive format version. Bump only when the layout changes; an archive from a
/// newer version is refused instead of applied half-way.
pub const SCHEMA_VERSION: u32 = 1;
pub const MANIFEST_NAME: &str = "manifest.json";
/// Format of the name the save dialog suggests. The UI builds the same string in
/// `src/lib/backup.ts`; the test below keeps the two in sync.
#[cfg(test)]
pub const BACKUP_NAME_PREFIX: &str = "splitline-backup-";
const APP_NAME: &str = "Splitline";

/// A settings archive is small (lists and JSON). These limits stop a hand-made
/// zip bomb before it is read into memory.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 32 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 128 * 1024 * 1024;

const CONFIG_ENTRY: &str = "config.json";
const GENERATED_ENTRY: &str = "generated.json";
const BENCH_ENTRY: &str = "bench.json";
const SUBSCRIPTION_ENTRY: &str = "xray/subscription.json";
const PREVIOUS_SUBSCRIPTION_ENTRY: &str = "xray/subscription.prev.json";
/// User lists live in `<runtime>\lists`; inside the archive they are flattened
/// into `lists/`, so importing never depends on the old runtime location.
const LISTS_PREFIX: &str = "lists/";
const USER_LIST_SUFFIX: &str = "-user.txt";

/// Folders the export reads from and the import writes to. Passed explicitly so
/// tests never touch `%LOCALAPPDATA%`.
#[derive(Debug, Clone)]
pub struct BackupPaths {
    pub app_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub xray_dir: PathBuf,
}

impl BackupPaths {
    /// The folders of the running application.
    pub fn current() -> Self {
        Self {
            app_dir: config::app_dir(),
            runtime_dir: config::runtime_dir(),
            xray_dir: crate::xray::dir(),
        }
    }
}

/// One file recorded in `manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub app: String,
    pub app_version: String,
    /// Unix seconds; the human label keeps the local time of the machine that
    /// made the archive.
    pub created_at: u64,
    pub created_label: String,
    /// Whether the archive carries the proxy key and the subscription URL.
    pub secrets: bool,
    pub files: Vec<ManifestFile>,
}

/// A file as the confirmation dialog shows it.
#[derive(Debug, Clone, Serialize)]
pub struct PreviewFile {
    pub path: String,
    pub size: u64,
}

/// What `inspect_backup` returns before anything is written.
#[derive(Debug, Clone, Serialize)]
pub struct BackupPreview {
    pub schema: u32,
    pub app: String,
    pub app_version: String,
    pub created_at: u64,
    pub created_label: String,
    pub secrets: bool,
    pub files: Vec<PreviewFile>,
    pub has_config: bool,
    pub lists: usize,
    pub generated: usize,
    pub bench_runs: usize,
    pub has_subscription: bool,
    /// One line for the dialog: «Конфиг, 3 списка, 5 стратегий».
    pub summary: String,
    /// What the user must know before confirming (missing secrets, other version).
    pub warnings: Vec<String>,
}

/// Result of an export; the UI shows the final path.
#[derive(Debug, Clone, Serialize)]
pub struct ExportReport {
    pub path: String,
    pub files: Vec<String>,
    pub secrets: bool,
    pub size: u64,
}

/// Result of an import.
#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub applied: Vec<String>,
    pub lists: usize,
    /// Safety copy of the state that was on disk before the import.
    pub backup_path: String,
    pub warnings: Vec<String>,
}

/// A verified archive: every file already read and hash-checked.
#[derive(Debug)]
pub struct VerifiedBackup {
    preview: BackupPreview,
    files: BTreeMap<String, Vec<u8>>,
    config: Option<AppConfig>,
}

impl VerifiedBackup {
    pub fn config(&self) -> Option<&AppConfig> {
        self.config.as_ref()
    }
}

/// Local time of the machine: `YYYYMMDD-HHMM` for file names, `YYYY-MM-DD HH:MM`
/// for the dialog. Kept out of the serialized manifest as two strings so the UI
/// never has to format a raw timestamp.
#[derive(Debug, Clone)]
pub struct Stamp {
    pub epoch: u64,
    pub compact: String,
    pub human: String,
}

impl Stamp {
    pub fn now() -> Self {
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let (compact, human) = match local_parts() {
            Some((year, month, day, hour, minute)) => (
                format_compact(year, month, day, hour, minute),
                format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}"),
            ),
            // Without a local clock the epoch still gives a unique, sortable name.
            None => (epoch.to_string(), epoch.to_string()),
        };
        Self {
            epoch,
            compact,
            human,
        }
    }
}

fn format_compact(year: u16, month: u16, day: u16, hour: u16, minute: u16) -> String {
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}")
}

#[cfg(windows)]
fn local_parts() -> Option<(u16, u16, u16, u16, u16)> {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;

    // SAFETY: `GetLocalTime` only fills the provided SYSTEMTIME.
    let mut time = unsafe { std::mem::zeroed::<SYSTEMTIME>() };
    unsafe { GetLocalTime(&mut time) };
    Some((time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute))
}

#[cfg(not(windows))]
fn local_parts() -> Option<(u16, u16, u16, u16, u16)> {
    None
}

#[cfg(test)]
pub fn backup_file_name(stamp: &Stamp) -> String {
    format!("{BACKUP_NAME_PREFIX}{}.zip", stamp.compact)
}

fn sha256_hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

/// Reads the current state into memory. Redaction happens here, before anything
/// reaches the archive: with `include_secrets == false` the proxy key and the
/// subscription URL are blanked and the subscription snapshot is left out.
fn collect_files(
    paths: &BackupPaths,
    include_secrets: bool,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut files = BTreeMap::new();

    files.insert(
        CONFIG_ENTRY.to_string(),
        config_for_archive(paths, include_secrets)?,
    );

    let lists_dir = paths.runtime_dir.join("lists");
    match fs::read_dir(&lists_dir) {
        Ok(entries) => {
            let mut names: Vec<String> = entries
                .flatten()
                .filter(|entry| entry.path().is_file())
                .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                .filter(|name| is_user_list_name(name))
                .collect();
            names.sort();
            for name in names {
                let data = fs::read(lists_dir.join(&name))
                    .map_err(|e| format!("Не удалось прочитать список {name}: {e}"))?;
                files.insert(format!("{LISTS_PREFIX}{name}"), data);
            }
        }
        // No runtime yet: an archive with just the settings is still useful.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Не удалось прочитать списки: {error}")),
    }

    for (entry, path) in [
        (GENERATED_ENTRY, paths.app_dir.join("generated.json")),
        (BENCH_ENTRY, paths.app_dir.join("bench.json")),
    ] {
        match fs::read(&path) {
            Ok(data) => {
                files.insert(entry.to_string(), data);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Не удалось прочитать {entry}: {error}")),
        }
    }

    // The subscription snapshot contains server addresses and credentials: it is
    // a secret exactly like the subscription URL and only goes in on request.
    if include_secrets {
        for (entry, path) in [
            (SUBSCRIPTION_ENTRY, paths.xray_dir.join("subscription.json")),
            (
                PREVIOUS_SUBSCRIPTION_ENTRY,
                paths.xray_dir.join("subscription.prev.json"),
            ),
        ] {
            match fs::read(&path) {
                Ok(data) => {
                    files.insert(entry.to_string(), data);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("Не удалось прочитать {entry}: {error}")),
            }
        }
    }

    for (entry, data) in &files {
        if data.len() as u64 > MAX_ENTRY_BYTES {
            return Err(format!("Файл {entry} слишком большой для архива настроек."));
        }
    }
    let total: u64 = files.values().map(|data| data.len() as u64).sum();
    if total > MAX_TOTAL_BYTES {
        return Err("Настроек слишком много для одного архива.".into());
    }
    Ok(files)
}

/// `config.json` as it should be copied. A missing file means a fresh install:
/// the defaults are archived so an import always has a config to restore.
fn config_for_archive(paths: &BackupPaths, include_secrets: bool) -> Result<Vec<u8>, String> {
    let raw = match fs::read(paths.app_dir.join(CONFIG_ENTRY)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            serde_json::to_vec_pretty(&AppConfig::default()).map_err(|e| e.to_string())?
        }
        Err(error) => return Err(format!("Не удалось прочитать config.json: {error}")),
    };
    let text = String::from_utf8(raw)
        .map_err(|_| "config.json записан не в UTF-8 — экспорт отменён.".to_string())?;
    let mut value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("config.json повреждён, экспорт отменён: {e}"))?;
    if !value.is_object() {
        return Err("config.json повреждён, экспорт отменён.".into());
    }
    if !include_secrets {
        redact_secrets(&mut value);
    }
    serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())
}

/// Blanks the fields that must never leave the machine without an explicit
/// opt-in. Unknown neighbours are preserved: a config written by a newer build
/// keeps its fields and only the two known secrets are cleared.
fn redact_secrets(value: &mut Value) {
    if let Some(tgws) = value.get_mut("tgws").and_then(Value::as_object_mut) {
        tgws.insert("secret".into(), Value::String(String::new()));
    }
    if let Some(xray) = value.get_mut("xray").and_then(Value::as_object_mut) {
        xray.insert("subscription_url".into(), Value::String(String::new()));
    }
}

fn build_manifest(
    files: &BTreeMap<String, Vec<u8>>,
    include_secrets: bool,
    stamp: &Stamp,
) -> Manifest {
    Manifest {
        schema: SCHEMA_VERSION,
        app: APP_NAME.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        created_at: stamp.epoch,
        created_label: stamp.human.clone(),
        secrets: include_secrets,
        files: files
            .iter()
            .map(|(path, data)| ManifestFile {
                path: path.clone(),
                size: data.len() as u64,
                sha256: sha256_hex(data),
            })
            .collect(),
    }
}

/// Writes one zip with `manifest.json` and the collected files.
pub fn write_backup(
    paths: &BackupPaths,
    dest: &Path,
    include_secrets: bool,
) -> Result<ExportReport, String> {
    let files = collect_files(paths, include_secrets)?;
    let manifest = build_manifest(&files, include_secrets, &Stamp::now());

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Не удалось создать папку для архива: {e}"))?;
    }
    let result = write_zip(dest, &manifest, &files);
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result?;

    let size = fs::metadata(dest).map(|meta| meta.len()).unwrap_or(0);
    Ok(ExportReport {
        path: dest.to_string_lossy().to_string(),
        files: files.keys().cloned().collect(),
        secrets: include_secrets,
        size,
    })
}

fn write_zip(
    dest: &Path,
    manifest: &Manifest,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    let file = fs::File::create(dest).map_err(|e| format!("Не удалось создать архив: {e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let manifest_json = serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?;
    zip.start_file(MANIFEST_NAME, options)
        .map_err(|e| format!("Не удалось записать manifest.json: {e}"))?;
    zip.write_all(&manifest_json)
        .map_err(|e| format!("Не удалось записать manifest.json: {e}"))?;

    for (name, data) in files {
        zip.start_file(name.as_str(), options)
            .map_err(|e| format!("Не удалось записать {name}: {e}"))?;
        zip.write_all(data)
            .map_err(|e| format!("Не удалось записать {name}: {e}"))?;
    }
    zip.finish()
        .map_err(|e| format!("Не удалось завершить архив: {e}"))?;
    Ok(())
}

/// Reads and verifies an archive. Nothing is written: this is both the preview
/// used by the dialog and the gate every import must pass.
pub fn verify(archive: &Path) -> Result<VerifiedBackup, String> {
    let file = fs::File::open(archive).map_err(|e| format!("Не удалось открыть архив: {e}"))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| format!("Это не архив ZIP или он повреждён: {e}"))?;

    // Every entry must have a safe relative name, be unique and fit the limits.
    let mut sizes: BTreeMap<String, u64> = BTreeMap::new();
    let mut total = 0u64;
    for index in 0..zip.len() {
        let (raw_name, size, is_dir, enclosed) = {
            let entry = zip
                .by_index(index)
                .map_err(|e| format!("Архив повреждён: {e}"))?;
            let raw = String::from_utf8_lossy(entry.name_raw()).to_string();
            (raw, entry.size(), entry.is_dir(), entry.enclosed_name())
        };
        if is_dir {
            continue;
        }
        // `enclosed_name` rejects absolute paths and `..` components. Without it
        // a crafted entry name would decide where the import writes.
        let Some(relative) = enclosed else {
            return Err(format!("В архиве недопустимый путь: {raw_name}"));
        };
        let name = relative.to_string_lossy().replace('\\', "/");
        if sizes.insert(name.clone(), size).is_some() {
            return Err(format!("В архиве два файла с одним именем: {name}"));
        }
        total = total.saturating_add(size);
        if total > MAX_TOTAL_BYTES {
            return Err("Архив слишком большой для архива настроек.".into());
        }
    }

    let manifest_bytes = read_entry(&mut zip, MANIFEST_NAME, MAX_MANIFEST_BYTES)?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| format!("manifest.json повреждён: {e}"))?;

    if manifest.schema > SCHEMA_VERSION {
        return Err(format!(
            "Архив из более новой версии Splitline (схема {}). Обновите приложение и повторите импорт.",
            manifest.schema
        ));
    }
    if manifest.schema < 1 {
        return Err("Неизвестная версия схемы архива — импорт отменён.".into());
    }

    let mut files = BTreeMap::new();
    let mut listed = BTreeSet::new();
    let mut read_total = 0u64;
    for entry in &manifest.files {
        if !is_safe_entry_name(&entry.path) {
            return Err(format!("В manifest.json недопустимый путь: {}", entry.path));
        }
        if entry.path == MANIFEST_NAME {
            return Err("manifest.json не может ссылаться сам на себя.".into());
        }
        if !listed.insert(entry.path.clone()) {
            return Err(format!("В manifest.json повторяется файл: {}", entry.path));
        }
        let Some(&size) = sizes.get(&entry.path) else {
            return Err(format!(
                "В архиве нет файла {}, указанного в manifest.json.",
                entry.path
            ));
        };
        let data = read_entry(&mut zip, &entry.path, MAX_ENTRY_BYTES)?;
        if size != entry.size || data.len() as u64 != entry.size {
            return Err(format!(
                "Размер файла {} не совпал с manifest.json — архив повреждён.",
                entry.path
            ));
        }
        if !sha256_hex(&data).eq_ignore_ascii_case(&entry.sha256) {
            return Err(format!(
                "Контрольная сумма файла {} не совпала — архив повреждён или изменён.",
                entry.path
            ));
        }
        read_total = read_total.saturating_add(data.len() as u64);
        if read_total > MAX_TOTAL_BYTES {
            return Err("Архив слишком большой для архива настроек.".into());
        }
        files.insert(entry.path.clone(), data);
    }

    // An unlisted entry could smuggle data into the data folder, so the manifest
    // is the only source of truth: anything else means the archive was rebuilt.
    for name in sizes.keys() {
        if name == MANIFEST_NAME {
            continue;
        }
        if !listed.contains(name) {
            return Err(format!(
                "В архиве есть файл {name}, которого нет в manifest.json."
            ));
        }
    }

    let config =
        match files.get(CONFIG_ENTRY) {
            Some(data) => {
                let text = std::str::from_utf8(data)
                    .map_err(|_| "config.json в архиве записан не в UTF-8.".to_string())?;
                Some(config::parse_config(text).ok_or_else(|| {
                    "config.json в архиве не читается — импорт отменён.".to_string()
                })?)
            }
            None => None,
        };

    let generated = match files.get(GENERATED_ENTRY) {
        Some(data) => json_array_len(data, GENERATED_ENTRY)?,
        None => 0,
    };
    let bench_runs = match files.get(BENCH_ENTRY) {
        Some(data) => json_array_len(data, BENCH_ENTRY)?,
        None => 0,
    };
    let lists = files
        .keys()
        .filter_map(|name| name.strip_prefix(LISTS_PREFIX))
        .filter(|name| is_user_list_name(name))
        .count();
    let has_subscription = files.contains_key(SUBSCRIPTION_ENTRY);

    let mut warnings = Vec::new();
    if manifest.secrets {
        warnings.push("Архив содержит секреты: ключ прокси и ссылку подписки.".to_string());
    } else {
        warnings.push(
            "В архиве нет ключа прокси и ссылки подписки — при импорте останутся текущие."
                .to_string(),
        );
    }
    if config.is_none() {
        warnings.push("В архиве нет config.json — настройки не изменятся.".to_string());
    }
    if manifest.app_version != env!("CARGO_PKG_VERSION") {
        warnings.push(format!(
            "Архив создан версией {} — проверьте настройки после импорта.",
            manifest.app_version
        ));
    }

    let summary = preview_summary(
        config.is_some(),
        lists,
        generated,
        bench_runs,
        has_subscription,
    );
    let preview = BackupPreview {
        schema: manifest.schema,
        app: manifest.app.clone(),
        app_version: manifest.app_version.clone(),
        created_at: manifest.created_at,
        created_label: manifest.created_label.clone(),
        secrets: manifest.secrets,
        files: manifest
            .files
            .iter()
            .map(|entry| PreviewFile {
                path: entry.path.clone(),
                size: entry.size,
            })
            .collect(),
        has_config: config.is_some(),
        lists,
        generated,
        bench_runs,
        has_subscription,
        summary,
        warnings,
    };

    Ok(VerifiedBackup {
        preview,
        files,
        config,
    })
}

/// Preview only: the bytes are dropped at once.
pub fn inspect(archive: &Path) -> Result<BackupPreview, String> {
    verify(archive).map(|verified| verified.preview)
}

fn read_entry<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>, String> {
    let entry = zip.by_name(name).map_err(|error| match error {
        zip::result::ZipError::FileNotFound => format!("В архиве нет файла {name}."),
        other => format!("Не удалось прочитать {name}: {other}"),
    })?;
    if entry.size() > limit {
        return Err(format!("Файл {name} слишком большой для архива настроек."));
    }
    let mut data = Vec::new();
    let read = entry
        .take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|e| format!("Не удалось прочитать {name}: {e}"))?;
    if read as u64 > limit {
        return Err(format!("Файл {name} слишком большой для архива настроек."));
    }
    Ok(data)
}

/// A zip entry name that cannot leave the target folder: relative, no `..`, no
/// drive prefix, no NUL. The archive's own names are checked the same way through
/// `enclosed_name`; this guards the names that arrive in `manifest.json`.
fn is_safe_entry_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with('/') || name.contains('\\') || name.contains('\0') {
        return false;
    }
    let path = Path::new(name);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Only `<name>-user.txt` without a folder: a manifest cannot point the import at
/// an arbitrary file inside the runtime.
fn is_user_list_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < 128
        && name.ends_with(USER_LIST_SUFFIX)
        && !name.contains('/')
        && is_safe_entry_name(name)
}

fn json_array_len(data: &[u8], entry: &str) -> Result<usize, String> {
    let value: Value =
        serde_json::from_slice(data).map_err(|e| format!("{entry} в архиве повреждён: {e}"))?;
    value
        .as_array()
        .map(|items| items.len())
        .ok_or_else(|| format!("{entry} в архиве имеет неверный формат."))
}

fn preview_summary(
    has_config: bool,
    lists: usize,
    generated: usize,
    bench_runs: usize,
    has_subscription: bool,
) -> String {
    let mut parts = Vec::new();
    if has_config {
        parts.push("конфиг".to_string());
    }
    if lists > 0 {
        parts.push(format!(
            "{} {}",
            lists,
            plural(lists, "список", "списка", "списков")
        ));
    }
    if generated > 0 {
        parts.push(format!(
            "{} {}",
            generated,
            plural(generated, "стратегия", "стратегии", "стратегий")
        ));
    }
    if bench_runs > 0 {
        parts.push(format!(
            "{} {}",
            bench_runs,
            plural(bench_runs, "проверка", "проверки", "проверок")
        ));
    }
    if has_subscription {
        parts.push("подписка".to_string());
    }
    if parts.is_empty() {
        return "Архив пуст.".to_string();
    }
    let text = parts.join(", ");
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => "Архив пуст.".to_string(),
    }
}

fn plural(count: usize, one: &str, few: &str, many: &str) -> String {
    let tail = count % 100;
    if (11..=14).contains(&tail) {
        return many.to_string();
    }
    match count % 10 {
        1 => one.to_string(),
        2..=4 => few.to_string(),
        _ => many.to_string(),
    }
}

/// Imported settings are merged with what the machine knows about itself. A
/// backup moves *settings*, not the current session, the installed binaries or a
/// drive letter that may not exist on the new PC.
pub fn merge_imported(current: &AppConfig, mut imported: AppConfig) -> AppConfig {
    imported.session_zapret = current.session_zapret;
    imported.session_initialized = current.session_initialized;
    imported.runtime_version = current.runtime_version.clone();
    imported.last_run_version = env!("CARGO_PKG_VERSION").to_string();
    // Autostart is registered in Windows (a scheduled task), not stored in the
    // archive; taking the imported flag would make the toggle lie about it.
    imported.autostart = current.autostart;

    // A custom runtime folder from another PC usually does not exist here; in
    // that case the current one stays, otherwise the bypass would point nowhere.
    if !imported.runtime_path.trim().is_empty() && !Path::new(&imported.runtime_path).exists() {
        imported.runtime_path = current.runtime_path.clone();
    }

    imported.tgws.session_running = current.tgws.session_running;
    imported.tgws.installed_version = current.tgws.installed_version.clone();
    imported.tgws.rs_version = current.tgws.rs_version.clone();
    // An archive without secrets carries an empty key; wiping the working key of
    // the current machine would break Telegram for no reason.
    if imported.tgws.secret.is_empty() {
        imported.tgws.secret = current.tgws.secret.clone();
        imported.tgws.secret_migrated = current.tgws.secret_migrated;
    }

    imported.xray.session_running = current.xray.session_running;
    imported.xray.installed_version = current.xray.installed_version.clone();
    imported.xray.bases_updated_at = current.xray.bases_updated_at;
    if imported.xray.subscription_url.trim().is_empty() {
        imported.xray.subscription_url = current.xray.subscription_url.clone();
    }
    imported
}

/// Makes a copy of the state that is on disk right now and returns its path.
/// Called before the services are stopped, so the copy is the untouched state.
pub fn create_safety_backup(paths: &BackupPaths) -> Result<String, String> {
    let report = write_backup(paths, &safety_backup_path(&paths.app_dir), true)?;
    Ok(report.path)
}

/// Writes verified data over the current state. `safety_backup` is the copy made
/// by [`create_safety_backup`] just before the import and is only reported back
/// to the user. Nothing in `paths` comes from the archive; each entry is mapped
/// explicitly, which is the second half of the zip-slip defence.
pub fn apply_verified(
    paths: &BackupPaths,
    verified: &VerifiedBackup,
    config: Option<&AppConfig>,
    safety_backup: &str,
) -> Result<ImportReport, String> {
    let mut applied = Vec::new();
    let mut lists = 0;
    for (name, data) in &verified.files {
        if name == CONFIG_ENTRY {
            continue;
        }
        let Some(target) = target_path(paths, name) else {
            continue;
        };
        write_atomic(&target, data)?;
        if name.starts_with(LISTS_PREFIX) {
            lists += 1;
        }
        applied.push(name.clone());
    }
    // The config goes last: a failure while copying lists leaves the previous
    // settings in place, which keeps the app usable.
    if let Some(config) = config {
        config::save_config_at(&paths.app_dir, config)?;
        applied.push(CONFIG_ENTRY.to_string());
    }
    applied.sort();

    Ok(ImportReport {
        applied,
        lists,
        backup_path: safety_backup.to_string(),
        warnings: Vec::new(),
    })
}

/// Where an entry is allowed to land. Unknown entries are ignored on purpose: a
/// future archive with extra files imports what this version understands.
fn target_path(paths: &BackupPaths, entry: &str) -> Option<PathBuf> {
    match entry {
        GENERATED_ENTRY => Some(paths.app_dir.join("generated.json")),
        BENCH_ENTRY => Some(paths.app_dir.join("bench.json")),
        SUBSCRIPTION_ENTRY => Some(paths.xray_dir.join("subscription.json")),
        PREVIOUS_SUBSCRIPTION_ENTRY => Some(paths.xray_dir.join("subscription.prev.json")),
        _ => entry
            .strip_prefix(LISTS_PREFIX)
            .filter(|name| is_user_list_name(name))
            .map(|name| paths.runtime_dir.join("lists").join(name)),
    }
}

fn safety_backup_path(app_dir: &Path) -> PathBuf {
    let dir = app_dir.join("backups");
    let stamp = Stamp::now();
    let mut candidate = dir.join(format!("backup-{}.zip", stamp.compact));
    let mut counter = 2;
    while candidate.exists() {
        candidate = dir.join(format!("backup-{}-{counter}.zip", stamp.compact));
        counter += 1;
    }
    candidate
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Не удалось создать папку {}: {e}", parent.display()))?;
    }
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!("{file_name}.tmp"));
    fs::write(&tmp, data).map_err(|e| format!("Не удалось записать {file_name}: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("Не удалось записать {file_name}: {e}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{TgwsConfig, XrayConfig};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("splitline-backup-{name}-{unique}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture(base: &Path) -> BackupPaths {
        let app_dir = base.join("zapret-ui");
        let paths = BackupPaths {
            app_dir: app_dir.clone(),
            runtime_dir: app_dir.join("runtime"),
            xray_dir: app_dir.join("xray"),
        };
        fs::create_dir_all(paths.runtime_dir.join("lists")).unwrap();
        fs::create_dir_all(&paths.xray_dir).unwrap();
        paths
    }

    fn write_config(paths: &BackupPaths, config: &AppConfig) {
        fs::write(
            paths.app_dir.join(CONFIG_ENTRY),
            serde_json::to_vec_pretty(config).unwrap(),
        )
        .unwrap();
    }

    fn entry_names(archive: &Path) -> Vec<String> {
        let file = fs::File::open(archive).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        (0..zip.len())
            .map(|index| zip.by_index(index).unwrap().name().unwrap().to_string())
            .collect()
    }

    fn entry_data(archive: &Path, name: &str) -> Vec<u8> {
        let file = fs::File::open(archive).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let mut entry = zip.by_name(name).unwrap();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        data
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    /// Rebuilds an archive with one entry replaced, keeping everything else
    /// byte-for-byte — that is how a tampered archive looks in the wild.
    fn rewrite_entry(source: &Path, dest: &Path, name: &str, data: &[u8]) {
        let mut collected = Vec::new();
        {
            let file = fs::File::open(source).unwrap();
            let mut zip = zip::ZipArchive::new(file).unwrap();
            for index in 0..zip.len() {
                let mut entry = zip.by_index(index).unwrap();
                let entry_name = entry.name().unwrap().to_string();
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                collected.push((entry_name, bytes));
            }
        }
        let mut owned: Vec<(String, Vec<u8>)> = Vec::new();
        for (entry_name, bytes) in collected {
            let bytes = if entry_name == name {
                data.to_vec()
            } else {
                bytes
            };
            owned.push((entry_name, bytes));
        }
        let owned_refs: Vec<(&str, &[u8])> = owned
            .iter()
            .map(|(entry_name, bytes)| (entry_name.as_str(), bytes.as_slice()))
            .collect();
        write_zip(dest, &owned_refs);
    }

    fn manifest_json(schema: u32, files: Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema": schema,
            "app": APP_NAME,
            "app_version": env!("CARGO_PKG_VERSION"),
            "created_at": 0,
            "created_label": "2026-11-20 10:00",
            "secrets": false,
            "files": files,
        }))
        .unwrap()
    }

    #[test]
    fn stamp_names_the_archive_by_local_time() {
        assert_eq!(format_compact(2026, 11, 20, 18, 5), "20261120-1805");
        let stamp = Stamp {
            epoch: 0,
            compact: "20261120-1805".into(),
            human: String::new(),
        };
        assert_eq!(
            backup_file_name(&stamp),
            "splitline-backup-20261120-1805.zip"
        );
    }

    #[test]
    fn roundtrip_restores_user_data() {
        let base = temp_dir("roundtrip");
        let source = fixture(&base.join("source"));
        let config = AppConfig {
            theme: "carmine".into(),
            tgws: TgwsConfig {
                secret: "0123456789abcdef0123456789abcdef".into(),
                ..TgwsConfig::default()
            },
            xray: XrayConfig {
                subscription_url: "https://example.test/sub".into(),
                ..XrayConfig::default()
            },
            ..AppConfig::default()
        };
        write_config(&source, &config);
        fs::write(
            source
                .runtime_dir
                .join("lists")
                .join("list-general-user.txt"),
            "example.com\n",
        )
        .unwrap();
        fs::write(
            source
                .runtime_dir
                .join("lists")
                .join("ipset-exclude-user.txt"),
            "10.0.0.0/8\n",
        )
        .unwrap();
        fs::write(source.app_dir.join("generated.json"), "[]").unwrap();
        fs::write(source.app_dir.join("bench.json"), "[]").unwrap();
        fs::write(source.xray_dir.join("subscription.json"), "[]").unwrap();

        let archive = base.join("export").join(backup_file_name(&Stamp::now()));
        let report = write_backup(&source, &archive, true).unwrap();
        assert!(archive.exists());
        assert!(report.secrets);
        assert!(report.files.contains(&CONFIG_ENTRY.to_string()));

        let names = entry_names(&archive);
        assert!(names.contains(&MANIFEST_NAME.to_string()));
        assert!(names.contains(&"lists/list-general-user.txt".to_string()));
        assert!(names.contains(&SUBSCRIPTION_ENTRY.to_string()));
        assert!(
            !names.iter().any(|name| name.starts_with("runtime/")),
            "runtime не должен попадать в архив"
        );

        let preview = inspect(&archive).unwrap();
        assert_eq!(preview.schema, SCHEMA_VERSION);
        assert!(preview.has_config);
        assert_eq!(preview.lists, 2);
        assert!(preview.has_subscription);
        assert!(preview.secrets);
        assert!(preview.summary.starts_with("Конфиг"));

        let target = fixture(&base.join("target"));
        let verified = verify(&archive).unwrap();
        let merged = merge_imported(&AppConfig::default(), verified.config().unwrap().clone());
        let safety = create_safety_backup(&target).unwrap();
        let applied = apply_verified(&target, &verified, Some(&merged), &safety).unwrap();
        assert_eq!(applied.lists, 2);
        assert!(Path::new(&applied.backup_path).exists());
        assert!(applied.backup_path.contains("backups"));

        let restored: AppConfig =
            serde_json::from_str(&fs::read_to_string(target.app_dir.join(CONFIG_ENTRY)).unwrap())
                .unwrap();
        assert_eq!(restored.theme, "carmine");
        assert_eq!(restored.xray.subscription_url, "https://example.test/sub");
        assert_eq!(
            fs::read_to_string(
                target
                    .runtime_dir
                    .join("lists")
                    .join("list-general-user.txt")
            )
            .unwrap(),
            "example.com\n"
        );
        assert_eq!(
            fs::read_to_string(target.xray_dir.join("subscription.json")).unwrap(),
            "[]"
        );
    }

    #[test]
    fn export_without_secrets_redacts_and_skips_the_subscription() {
        let base = temp_dir("redaction");
        let source = fixture(&base);
        let config = AppConfig {
            tgws: TgwsConfig {
                secret: "0123456789abcdef0123456789abcdef".into(),
                ..TgwsConfig::default()
            },
            xray: XrayConfig {
                subscription_url: "https://example.test/sub".into(),
                ..XrayConfig::default()
            },
            ..AppConfig::default()
        };
        write_config(&source, &config);
        fs::write(source.xray_dir.join("subscription.json"), "[]").unwrap();

        let archive = base.join(backup_file_name(&Stamp::now()));
        write_backup(&source, &archive, false).unwrap();

        let value: Value = serde_json::from_slice(&entry_data(&archive, CONFIG_ENTRY)).unwrap();
        assert_eq!(value["tgws"]["secret"], "");
        assert_eq!(value["xray"]["subscription_url"], "");
        assert!(!entry_names(&archive).contains(&SUBSCRIPTION_ENTRY.to_string()));

        let preview = inspect(&archive).unwrap();
        assert!(!preview.secrets);
        assert!(preview.warnings.iter().any(|w| w.contains("ключа прокси")));
    }

    #[test]
    fn corrupt_archive_is_rejected() {
        let base = temp_dir("corrupt");
        let archive = base.join("broken.zip");
        fs::write(&archive, b"this is not a zip file").unwrap();
        assert!(inspect(&archive).is_err());
    }

    #[test]
    fn zip_slip_entry_is_rejected_and_nothing_is_written() {
        let base = temp_dir("slip");
        let archive = base.join("slip.zip");
        let manifest = manifest_json(SCHEMA_VERSION, serde_json::json!([]));
        write_zip(
            &archive,
            &[(MANIFEST_NAME, &manifest), ("../evil.txt", b"pwned")],
        );

        let error = verify(&archive).unwrap_err();
        assert!(error.contains("недопустимый путь"), "{error}");
        assert!(!base.join("evil.txt").exists());
        assert!(!base.parent().unwrap().join("evil.txt").exists());
    }

    #[test]
    fn manifest_path_outside_the_data_folder_is_rejected() {
        let base = temp_dir("slip-manifest");
        let archive = base.join("slip.zip");
        let manifest = manifest_json(
            SCHEMA_VERSION,
            serde_json::json!([{ "path": "../evil.txt", "size": 5, "sha256": sha256_hex(b"pwned") }]),
        );
        write_zip(&archive, &[(MANIFEST_NAME, &manifest)]);

        let error = verify(&archive).unwrap_err();
        assert!(error.contains("недопустимый путь"), "{error}");
    }

    #[test]
    fn hash_mismatch_is_rejected() {
        let base = temp_dir("hash");
        let source = fixture(&base.join("source"));
        write_config(&source, &AppConfig::default());
        let archive = base.join(backup_file_name(&Stamp::now()));
        write_backup(&source, &archive, false).unwrap();

        let tampered = base.join("tampered.zip");
        // Same length, different bytes: the size check passes and the hash is
        // what rejects the archive.
        let mut config_bytes = entry_data(&archive, CONFIG_ENTRY);
        config_bytes[0] ^= 0xff;
        rewrite_entry(&archive, &tampered, CONFIG_ENTRY, &config_bytes);
        let error = verify(&tampered).unwrap_err();
        assert!(error.contains("Контрольная сумма"), "{error}");
    }

    #[test]
    fn unlisted_entry_is_rejected() {
        let base = temp_dir("extra");
        let archive = base.join("extra.zip");
        let manifest = manifest_json(SCHEMA_VERSION, serde_json::json!([]));
        write_zip(
            &archive,
            &[(MANIFEST_NAME, &manifest), ("config.json", b"{}")],
        );
        let error = verify(&archive).unwrap_err();
        assert!(error.contains("которого нет в manifest.json"), "{error}");
    }

    #[test]
    fn newer_schema_is_refused_without_writing() {
        let base = temp_dir("schema");
        let archive = base.join("future.zip");
        let manifest = manifest_json(SCHEMA_VERSION + 1, serde_json::json!([]));
        write_zip(&archive, &[(MANIFEST_NAME, &manifest)]);
        let error = inspect(&archive).unwrap_err();
        assert!(error.contains("более новой версии"), "{error}");
        assert!(!base.join("zapret-ui").exists());
    }

    #[test]
    fn merge_keeps_secrets_and_session_of_the_machine() {
        let current = AppConfig {
            runtime_path: r"C:\current-runtime".into(),
            autostart: false,
            tgws: TgwsConfig {
                secret: "keep-me".into(),
                ..TgwsConfig::default()
            },
            xray: XrayConfig {
                subscription_url: "https://keep.test/sub".into(),
                ..XrayConfig::default()
            },
            ..AppConfig::default()
        };
        let imported = AppConfig {
            runtime_path: r"Z:\gone".into(),
            session_zapret: true,
            autostart: true,
            ..AppConfig::default()
        };
        assert!(imported.tgws.secret.is_empty());
        assert!(imported.xray.subscription_url.is_empty());

        let merged = merge_imported(&current, imported);
        assert_eq!(merged.tgws.secret, "keep-me");
        assert_eq!(merged.xray.subscription_url, "https://keep.test/sub");
        assert_eq!(merged.runtime_path, current.runtime_path);
        assert!(!merged.session_zapret);
        assert!(
            !merged.autostart,
            "автозапуск — состояние Windows, а не настройка архива"
        );
        assert_eq!(merged.last_run_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn imported_values_win_when_they_are_present() {
        let current = AppConfig::default();
        let imported = AppConfig {
            theme: "carmine".into(),
            tgws: TgwsConfig {
                secret: "from-archive".into(),
                ..TgwsConfig::default()
            },
            xray: XrayConfig {
                subscription_url: "https://archive.test/sub".into(),
                ..XrayConfig::default()
            },
            ..AppConfig::default()
        };

        let merged = merge_imported(&current, imported);
        assert_eq!(merged.tgws.secret, "from-archive");
        assert_eq!(merged.xray.subscription_url, "https://archive.test/sub");
        assert_eq!(merged.theme, "carmine");
    }

    #[test]
    fn list_names_are_limited_to_user_lists() {
        assert!(is_user_list_name("list-general-user.txt"));
        assert!(!is_user_list_name("service.bat"));
        assert!(!is_user_list_name("../list-user.txt"));
        assert!(!is_user_list_name("lists/list-user.txt"));
    }
}
