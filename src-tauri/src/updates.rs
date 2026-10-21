//! In-app updates from the project's GitHub Releases. The updater checks
//! each download against the public key in `tauri.conf.json` before running
//! it, so only installers signed with the project's key are installed.
//! Nothing is downloaded or installed until the user asks.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

use crate::error::{AppError, CmdResult, ErrorCode};
use crate::shell::ShellManager;

pub const EVENT_PROGRESS: &str = "update:progress";

#[derive(Default)]
pub struct UpdateManager {
    /// The update found by the last check, installed by `update_install`.
    pending: Mutex<Option<Update>>,
    installing: AtomicBool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    /// Release notes from `latest.json` (the release tag's message).
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

fn update_error(what: &str, error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::UpdateFailed, format!("{what}: {error}"))
}

/// Whether enough has arrived since the last progress event to send another:
/// each whole percent, or each megabyte when the size isn't known.
fn worth_reporting(previous: u64, downloaded: u64, total: Option<u64>) -> bool {
    match total {
        Some(total) if total > 0 => previous * 100 / total != downloaded * 100 / total,
        _ => previous / 1_000_000 != downloaded / 1_000_000,
    }
}

#[tauri::command]
pub async fn update_check(app: AppHandle, manager: State<'_, UpdateManager>) -> CmdResult<Option<UpdateInfo>> {
    let update = updater(&app)?.check().await.map_err(|e| update_error("Couldn't check for updates", e))?;
    let info = update.as_ref().map(|u| UpdateInfo {
        version: u.version.clone(),
        current_version: u.current_version.clone(),
        notes: u.body.clone().filter(|notes| !notes.trim().is_empty()),
    });
    match &info {
        Some(info) => log::info!("Update available: {} (running {})", info.version, info.current_version),
        None => log::info!("No update available"),
    }
    *manager.pending.lock().unwrap_or_else(|e| e.into_inner()) = update;
    Ok(info)
}

/// Downloads the update found by the last check, verifies its signature and
/// runs the installer, which closes VCE and starts the new version.
#[tauri::command]
pub async fn update_install(app: AppHandle, manager: State<'_, UpdateManager>) -> CmdResult<()> {
    if manager.installing.swap(true, Ordering::SeqCst) {
        return Err(AppError::new(ErrorCode::InvalidInput, "The update is already being installed."));
    }
    let result = install(&app, &manager).await;
    manager.installing.store(false, Ordering::SeqCst);
    result
}

async fn install(app: &AppHandle, manager: &UpdateManager) -> CmdResult<()> {
    let Some(update) = manager.pending.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return Err(AppError::new(ErrorCode::InvalidInput, "Check for updates first, then install."));
    };
    log::info!("Installing update {}", update.version);
    let mut downloaded = 0u64;
    let on_chunk = |chunk: usize, total: Option<u64>| {
        let previous = downloaded;
        downloaded += chunk as u64;
        if worth_reporting(previous, downloaded, total)
            && let Err(e) = app.emit(EVENT_PROGRESS, UpdateProgress { downloaded, total })
        {
            log::warn!("Could not emit {EVENT_PROGRESS}: {e}");
        }
    };
    update.download_and_install(on_chunk, || log::info!("Update downloaded and verified")).await.map_err(|e| {
        log::error!("Installing update {} failed: {e}", update.version);
        update_error("The update couldn't be installed", e)
    })?;
    // Windows' installer exits VCE before this point; elsewhere, restart.
    app.restart();
}

/// An updater that stops PowerShell cleanly before the installer closes VCE:
/// on Windows it exits the process directly, skipping the usual exit event.
fn updater(app: &AppHandle) -> CmdResult<Updater> {
    let handle = app.clone();
    app.updater_builder()
        .on_before_exit(move || {
            handle.state::<ShellManager>().shutdown();
            handle.cleanup_before_exit();
        })
        .build()
        .map_err(|e| update_error("The updater couldn't start", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_reported_per_percent_or_megabyte() {
        assert!(!worth_reporting(0, 5, Some(1000)));
        assert!(worth_reporting(5, 10, Some(1000)));
        assert!(!worth_reporting(10_000, 20_000, None));
        assert!(worth_reporting(990_000, 1_010_000, None));
    }
}
