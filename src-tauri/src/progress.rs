//! Progress of long downloads, shared by every download path.
//!
//! One `download-progress` event feeds a single overlay in the UI: the Xray
//! core, the domain lists, the zapret runtime, the Telegram proxy and the app
//! update all report through it.

use serde::Serialize;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub id: String,
    pub label: String,
    pub received: u64,
    pub total: u64,
    pub done: bool,
    pub error: Option<String>,
}

fn emit(app: &AppHandle, progress: DownloadProgress) {
    let _ = app.emit("download-progress", progress);
}

pub fn update(app: &AppHandle, id: &str, label: &str, received: u64, total: u64) {
    emit(
        app,
        DownloadProgress {
            id: id.to_string(),
            label: label.to_string(),
            received,
            total,
            done: false,
            error: None,
        },
    );
}

pub fn finish(app: &AppHandle, id: &str, label: &str, total: u64) {
    emit(
        app,
        DownloadProgress {
            id: id.to_string(),
            label: label.to_string(),
            received: total,
            total,
            done: true,
            error: None,
        },
    );
}

pub fn fail(app: &AppHandle, id: &str, label: &str, error: &str) {
    emit(
        app,
        DownloadProgress {
            id: id.to_string(),
            label: label.to_string(),
            received: 0,
            total: 0,
            done: true,
            error: Some(error.to_string()),
        },
    );
}

/// Throttles events: a 17 MB archive at 10 MB/s would otherwise flood the IPC
/// with hundreds of messages per second and stutter the UI.
pub struct Throttle {
    last: Instant,
    interval: Duration,
}

impl Default for Throttle {
    fn default() -> Self {
        Self::new()
    }
}

impl Throttle {
    pub fn new() -> Self {
        Self {
            // Ready immediately, so the first chunk is reported.
            last: Instant::now() - Duration::from_secs(1),
            interval: Duration::from_millis(120),
        }
    }

    pub fn ready(&mut self) -> bool {
        if self.last.elapsed() >= self.interval {
            self.last = Instant::now();
            true
        } else {
            false
        }
    }
}
