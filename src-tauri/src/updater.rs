//! Application self-update, modelled on Flowseal/tg-ws-proxy:
//! GitHub latest release -> SHA-256 check -> signature check -> replace the
//! running exe -> restart.
//!
//! Two independent checks guard the swap. The asset must carry a SHA-256
//! `digest` from the GitHub API (transport integrity), and the release must
//! publish `<asset>.sig` — a detached minisign signature made with the release
//! private key, verified against the public key compiled into the binary
//! (`signing.rs`). Without a signature the update is refused; see
//! `SECURITY.md` for what that protects against.

use crate::runtime;
use crate::signing;
use crate::sys;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const REPO: &str = "Dezuron/splitline";
const ASSET_NAME: &str = "Splitline.exe";
/// Name the app shipped under before the rename. Releases from 1.3.0 publish
/// both files so 1.2.x clients keep updating.
const LEGACY_ASSET_NAME: &str = "zapret-ui.exe";
const MAX_UPDATE_BYTES: u64 = 200 * 1024 * 1024;
/// A minisign `.sig` file is a few hundred bytes; anything larger is not one.
const MAX_SIGNATURE_BYTES: u64 = 8 * 1024;

/// Backup of the currently running exe, next to it: `Splitline_old.exe`.
fn backup_name(current: &Path) -> OsString {
    let stem = current
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "app".into());
    OsString::from(format!("{stem}_old.exe"))
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
    digest: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub available: bool,
    pub url: String,
    pub notes: String,
    pub size: u64,
    pub can_auto_update: bool,
    /// Why an automatic update is not possible, when it is not. The text is
    /// ready to be shown to the user as is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature_note: Option<String>,
    /// Key id (uppercase hex) of the public key this build checks signatures
    /// with. Shown next to the update channel so it can be compared with
    /// `SECURITY.md`; absent when the build has no key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_fingerprint: Option<String>,
}

fn client(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(format!("splitline/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())
}

fn fetch_latest() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let response = client(Duration::from_secs(30))?
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("Не удалось проверить обновления: {e}"))?;

    // A private repository or a repository without releases returns 404.
    // That is not an error for the user: there is simply nothing to install.
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!("GitHub ответил кодом {}", response.status()));
    }
    response
        .json::<Release>()
        .map(Some)
        .map_err(|e| format!("Некорректный ответ GitHub: {e}"))
}

pub fn normalize_version(tag: &str) -> String {
    tag.trim().trim_start_matches(['v', 'V']).to_string()
}

fn version_parts(version: &str) -> Vec<u64> {
    normalize_version(version)
        .split(['.', '-', '+'])
        .map(|part| {
            part.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect()
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    let latest = version_parts(latest);
    let current = version_parts(current);
    for index in 0..latest.len().max(current.len()) {
        let left = latest.get(index).copied().unwrap_or(0);
        let right = current.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    false
}

fn valid_digest(digest: Option<&str>) -> bool {
    digest
        .and_then(|value| value.strip_prefix("sha256:"))
        .map(|hex| hex.len() == 64 && hex.chars().all(|character| character.is_ascii_hexdigit()))
        .unwrap_or(false)
}

fn select_asset(release: &Release) -> Option<&Asset> {
    let usable = |name: &str| {
        release
            .assets
            .iter()
            .find(|asset| asset.name == name && valid_digest(asset.digest.as_deref()))
    };
    // Prefer the current name; fall back to the pre-rename asset so an
    // older release (or a partially staged one) can still be installed.
    usable(ASSET_NAME).or_else(|| usable(LEGACY_ASSET_NAME))
}

/// Detached signature published next to `asset` (`Splitline.exe.sig`).
fn signature_asset<'a>(release: &'a Release, asset: &Asset) -> Option<&'a Asset> {
    let wanted = signing::signature_file_name(&asset.name);
    release
        .assets
        .iter()
        .find(|candidate| candidate.name == wanted)
}

/// The asset that can actually be installed, or the reason it cannot.
///
/// This is the single place that decides whether the automatic update is
/// allowed, so `check` and `apply_latest` cannot disagree. An `Err` carries
/// text meant for the user.
fn auto_update_ready_for<'a>(
    release: &'a Release,
    public_key: Option<&str>,
) -> Result<&'a Asset, String> {
    let Some(asset) = select_asset(release) else {
        return Err("В релизе нет файла Splitline.exe с корректной SHA-256 суммой — обновитесь со страницы релиза.".into());
    };
    if public_key.is_none() {
        return Err(signing::MISSING_KEY_MESSAGE.into());
    }
    if signature_asset(release, asset).is_none() {
        return Err(signing::UNSIGNED_RELEASE_MESSAGE.into());
    }
    Ok(asset)
}

fn auto_update_ready(release: &Release) -> Result<&Asset, String> {
    auto_update_ready_for(release, signing::update_public_key())
}

pub fn check() -> Result<Option<UpdateInfo>, String> {
    let Some(release) = fetch_latest()? else {
        return Ok(None);
    };
    let current = env!("CARGO_PKG_VERSION").to_string();
    let latest = normalize_version(&release.tag_name);
    let size = select_asset(&release).map(|asset| asset.size).unwrap_or(0);
    let signature_note = auto_update_ready(&release).err();
    Ok(Some(UpdateInfo {
        available: is_newer(&latest, &current),
        latest,
        current,
        url: release.html_url,
        notes: release.body,
        size,
        can_auto_update: signature_note.is_none(),
        signature_note,
        key_fingerprint: signing::update_public_key().and_then(signing::fingerprint),
    }))
}

fn download(app: &tauri::AppHandle, asset: &Asset) -> Result<(PathBuf, u64), String> {
    let digest = asset
        .digest
        .as_deref()
        .filter(|digest| valid_digest(Some(digest)))
        .ok_or("У релиза нет корректной SHA-256 суммы.")?;

    let destination =
        std::env::temp_dir().join(format!("zapret-ui-update-{}.exe", std::process::id()));
    let _ = fs::remove_file(&destination);

    let mut response = client(Duration::from_secs(600))?
        .get(&asset.browser_download_url)
        .send()
        .map_err(|e| format!("Ошибка скачивания обновления: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Ошибка скачивания обновления: {}",
            response.status()
        ));
    }
    let total = response.content_length().unwrap_or(0);
    if total > MAX_UPDATE_BYTES {
        return Err("Файл обновления подозрительно большой.".into());
    }

    let mut file = fs::File::create(&destination).map_err(|e| e.to_string())?;
    let mut buffer = [0u8; 64 * 1024];
    let mut written = 0u64;
    let mut throttle = crate::progress::Throttle::new();
    crate::progress::update(app, "app-update", "Обновление приложения", 0, total);
    loop {
        let read = response.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        written += read as u64;
        if written > MAX_UPDATE_BYTES {
            let _ = fs::remove_file(&destination);
            return Err("Файл обновления подозрительно большой.".into());
        }
        file.write_all(&buffer[..read]).map_err(|e| e.to_string())?;
        if throttle.ready() {
            crate::progress::update(app, "app-update", "Обновление приложения", written, total);
        }
    }

    if let Err(error) = runtime::verify_digest(&destination, digest) {
        let _ = fs::remove_file(&destination);
        return Err(error);
    }
    Ok((destination, written))
}

/// Downloads the detached `.sig` that belongs to `asset`.
///
/// The signature is downloaded before the exe: it is a few hundred bytes, so a
/// release without a usable signature fails without downloading 40 MB first.
fn download_signature(release: &Release, asset: &Asset) -> Result<String, String> {
    let signature_asset = signature_asset(release, asset)
        .ok_or_else(|| signing::UNSIGNED_RELEASE_MESSAGE.to_string())?;

    let response = client(Duration::from_secs(60))?
        .get(&signature_asset.browser_download_url)
        .send()
        .map_err(|e| format!("Не удалось скачать подпись обновления: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Не удалось скачать подпись обновления: {}",
            response.status()
        ));
    }
    if response.content_length().unwrap_or(0) > MAX_SIGNATURE_BYTES {
        return Err(signing::CORRUPT_SIGNATURE_MESSAGE.into());
    }

    let mut text = String::new();
    response
        .take(MAX_SIGNATURE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|_| signing::CORRUPT_SIGNATURE_MESSAGE.to_string())?;
    if text.len() as u64 > MAX_SIGNATURE_BYTES || text.trim().is_empty() {
        return Err(signing::CORRUPT_SIGNATURE_MESSAGE.into());
    }
    Ok(text)
}

fn relaunch_args() -> Vec<OsString> {
    let mut args = Vec::new();
    let mut iterator = std::env::args_os().skip(1);
    while let Some(arg) = iterator.next() {
        if arg == "--wait-for-pid" {
            let _ = iterator.next();
            continue;
        }
        args.push(arg);
    }
    args
}

/// Replaces the running exe with the already verified bytes and starts the new
/// version.
///
/// The binary is written from memory on purpose: reading the downloaded file
/// again would open a window where a local process could swap it between the
/// signature check and the swap.
fn replace_and_restart(binary: &[u8]) -> Result<(), String> {
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = current
        .parent()
        .ok_or("Не удалось определить папку приложения.")?;
    let backup = parent.join(backup_name(&current));

    let _ = fs::remove_file(&backup);
    if backup.exists() {
        return Err("Не удалось удалить старую резервную копию приложения.".into());
    }

    fs::rename(&current, &backup).map_err(|e| format!("Не удалось подготовить замену exe: {e}"))?;

    if let Err(error) = fs::write(&current, binary) {
        let _ = fs::rename(&backup, &current);
        return Err(format!("Не удалось заменить exe: {error}"));
    }

    // A relaunch started while Windows is ending the session dies in the
    // loader with 0xc0000142 and shows a dialog; the new exe will start on the
    // next logon anyway.
    if sys::session_ending() {
        return Err(
            "Windows завершает сеанс — новая версия запустится при следующем входе.".into(),
        );
    }

    let mut command = Command::new(&current);
    command
        .args(relaunch_args())
        .arg("--wait-for-pid")
        .arg(std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    sys::hidden(&mut command);

    if let Err(error) = command.spawn() {
        let _ = fs::remove_file(&current);
        let _ = fs::rename(&backup, &current);
        return Err(format!("Не удалось перезапустить приложение: {error}"));
    }
    Ok(())
}

/// Downloads and installs the latest release. The caller must exit the current
/// process afterwards; the new exe waits for this PID and then starts normally.
pub fn apply_latest(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::Emitter;

    let Some(release) = fetch_latest()? else {
        return Err("Релизы приложения недоступны.".into());
    };
    let current = env!("CARGO_PKG_VERSION");
    let latest = normalize_version(&release.tag_name);
    if !is_newer(&latest, current) {
        return Err("Установлена последняя версия приложения.".into());
    }
    let asset = auto_update_ready(&release)?;
    let signature_text = download_signature(&release, asset)?;

    let _ = app.emit("app-update-progress", "Скачивание обновления…");
    let downloaded = match download(app, asset) {
        Ok((path, written)) => {
            crate::progress::finish(app, "app-update", "Обновление приложения", written);
            path
        }
        Err(error) => {
            crate::progress::fail(app, "app-update", "Обновление приложения", &error);
            return Err(error);
        }
    };

    // The signature is checked before the exe swap: a release whose signature
    // does not match the downloaded bytes must leave the installation intact.
    let _ = app.emit("app-update-progress", "Проверка подписи…");
    let verified = fs::read(&downloaded)
        .map_err(|e| format!("Не удалось прочитать файл обновления: {e}"))
        .and_then(|bytes| signing::verify_update(&bytes, &signature_text).map(|()| bytes));
    let binary = match verified {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = fs::remove_file(&downloaded);
            crate::progress::fail(app, "app-update", "Обновление приложения", &error);
            return Err(error);
        }
    };
    let _ = fs::remove_file(&downloaded);

    let _ = app.emit("app-update-progress", "Установка и перезапуск…");
    replace_and_restart(&binary)
}

/// Removes the backup left by the previous update after the old process exited.
pub fn cleanup_backup() {
    let Ok(current) = std::env::current_exe() else {
        return;
    };
    let Some(parent) = current.parent() else {
        return;
    };
    let _ = fs::remove_file(parent.join(backup_name(&current)));
    // Backups left by builds that shipped under the old name.
    let _ = fs::remove_file(parent.join("zapret-ui_old.exe"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("1.1.1", "1.1.0"));
        assert!(is_newer("v1.2.0", "1.1.9"));
        assert!(!is_newer("1.1.0", "1.1.0"));
        assert!(!is_newer("1.0.9", "1.1.0"));
        assert!(is_newer("2.0", "1.99.99"));
    }

    #[test]
    fn digest_requires_sha256_hex() {
        assert!(valid_digest(Some(
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        )));
        assert!(!valid_digest(Some("md5:deadbeef")));
        assert!(!valid_digest(Some("sha256:short")));
        assert!(!valid_digest(None));
    }

    #[test]
    fn asset_selection_requires_a_valid_digest() {
        let make = |digest: Option<&str>| Release {
            tag_name: "v1.1.1".into(),
            html_url: "https://example".into(),
            body: String::new(),
            assets: vec![Asset {
                name: ASSET_NAME.into(),
                browser_download_url: "https://example/app.exe".into(),
                size: 10,
                digest: digest.map(str::to_string),
            }],
        };
        assert!(select_asset(&make(Some(
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        )))
        .is_some());
        assert!(select_asset(&make(None)).is_none());
        assert!(select_asset(&make(Some("sha256:no"))).is_none());
    }

    const TEST_DIGEST: &str =
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn asset(name: &str, digest: bool) -> Asset {
        Asset {
            name: name.into(),
            browser_download_url: format!("https://example/{name}"),
            size: 10,
            digest: digest.then(|| TEST_DIGEST.to_string()),
        }
    }

    fn release(assets: Vec<Asset>) -> Release {
        Release {
            tag_name: "v2.2.0".into(),
            html_url: "https://example/release".into(),
            body: String::new(),
            assets,
        }
    }

    #[test]
    fn signature_is_looked_up_next_to_the_asset() {
        let signed = release(vec![
            asset(ASSET_NAME, true),
            asset("Splitline.exe.sig", false),
            asset("Splitline_2.2.0_x64_portable.zip", true),
        ]);
        let selected = select_asset(&signed).unwrap();
        assert_eq!(
            signature_asset(&signed, selected).map(|sig| sig.name.as_str()),
            Some("Splitline.exe.sig")
        );

        // The pre-rename asset carries its own signature file.
        let legacy = release(vec![
            asset(ASSET_NAME, false),
            asset(LEGACY_ASSET_NAME, true),
            asset("zapret-ui.exe.sig", false),
        ]);
        let selected = select_asset(&legacy).unwrap();
        assert_eq!(selected.name, LEGACY_ASSET_NAME);
        assert_eq!(
            signature_asset(&legacy, selected).map(|sig| sig.name.as_str()),
            Some("zapret-ui.exe.sig")
        );

        let unsigned = release(vec![asset(ASSET_NAME, true)]);
        let selected = select_asset(&unsigned).unwrap();
        assert!(signature_asset(&unsigned, selected).is_none());
    }

    #[test]
    fn auto_update_needs_digest_key_and_signature() {
        let signed = release(vec![
            asset(ASSET_NAME, true),
            asset("Splitline.exe.sig", false),
        ]);
        // Any non-empty key means "the build has one"; the key itself is not
        // parsed here (see signing.rs for the cryptographic checks).
        assert!(auto_update_ready_for(&signed, Some("RWQtest")).is_ok());

        // A build without a public key refuses to update at all.
        assert_eq!(
            auto_update_ready_for(&signed, None).unwrap_err(),
            signing::MISSING_KEY_MESSAGE
        );

        // A release without a signature is not installable either.
        let unsigned = release(vec![asset(ASSET_NAME, true)]);
        assert_eq!(
            auto_update_ready_for(&unsigned, Some("RWQtest")).unwrap_err(),
            signing::UNSIGNED_RELEASE_MESSAGE
        );

        // Nor is one without a usable asset (no digest).
        let no_digest = release(vec![
            asset(ASSET_NAME, false),
            asset("Splitline.exe.sig", false),
        ]);
        assert!(auto_update_ready_for(&no_digest, Some("RWQtest"))
            .unwrap_err()
            .contains("Splitline.exe"));
    }
}
