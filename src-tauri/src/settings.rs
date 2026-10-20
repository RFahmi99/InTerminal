//! Persistent settings in the app's config folder. The `ui` section belongs
//! to the frontend and is stored as-is.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::error::{AppError, CmdResult, ErrorCode};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// A user-chosen GGUF model; `None` means the default download.
    #[serde(default)]
    pub model_path: Option<String>,
    #[serde(default)]
    pub ui: serde_json::Value,
}

pub struct SettingsStore {
    path: PathBuf,
    current: Mutex<Settings>,
    /// Why saved settings couldn't be used, shown to the user once.
    load_warning: Option<String>,
}

/// What `settings_get` returns: the settings plus any problem loading them.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedSettings {
    #[serde(flatten)]
    pub settings: Settings,
    pub load_warning: Option<String>,
}

impl SettingsStore {
    /// Loads settings, falling back to defaults when the file is missing or
    /// unreadable, so a bad file never stops the app starting. A corrupt file
    /// is moved aside rather than silently overwritten by the next save.
    pub fn load(path: PathBuf) -> Self {
        let (current, load_warning) = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(settings) => (settings, None),
                Err(e) => {
                    let backup = path.with_extension("json.corrupt");
                    let kept = match std::fs::rename(&path, &backup) {
                        Ok(()) => format!(" The old file was kept as {}.", backup.display()),
                        Err(rename_error) => {
                            log::warn!("Could not move corrupt settings aside to {}: {rename_error}", backup.display());
                            String::new()
                        }
                    };
                    log::warn!("Unreadable settings file {}: {e}", path.display());
                    (
                        Settings::default(),
                        Some(format!("Your saved settings couldn't be read ({e}), so VCE started with the defaults.{kept}")),
                    )
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
            Err(e) => {
                log::warn!("Could not read settings file {}: {e}", path.display());
                (Settings::default(), Some(format!("Your saved settings couldn't be opened ({e}), so VCE started with the defaults.")))
            }
        };
        Self { path, current: Mutex::new(current), load_warning }
    }

    pub fn get(&self) -> Settings {
        self.current.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn update(&self, change: impl FnOnce(&mut Settings)) -> CmdResult<Settings> {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = current.clone();
        change(&mut next);
        write_atomically(&self.path, &next)?;
        *current = next.clone();
        Ok(next)
    }
}

/// Writes via a temporary file and rename, so a crash mid-write can't leave
/// a truncated settings file behind.
fn write_atomically(path: &Path, settings: &Settings) -> CmdResult<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| AppError::io("create folder", dir, &e))?;
    }
    let json = serde_json::to_vec_pretty(settings).map_err(|e| AppError::new(ErrorCode::Io, format!("Could not encode settings: {e}")))?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, json).map_err(|e| AppError::io("write", &temporary, &e))?;
    std::fs::rename(&temporary, path).map_err(|e| AppError::io("save", path, &e))
}

#[tauri::command]
pub fn settings_get(store: State<'_, SettingsStore>) -> LoadedSettings {
    LoadedSettings { settings: store.get(), load_warning: store.load_warning.clone() }
}

#[tauri::command]
pub fn settings_set_ui(store: State<'_, SettingsStore>, ui: serde_json::Value) -> CmdResult<()> {
    store.update(|s| s.ui = ui).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_survives_corrupt_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let store = SettingsStore::load(path.clone());
        store.update(|s| s.model_path = Some("m.gguf".into())).unwrap();
        assert_eq!(SettingsStore::load(path.clone()).get().model_path.as_deref(), Some("m.gguf"));

        std::fs::write(&path, "{not json").unwrap();
        let store = SettingsStore::load(path.clone());
        assert!(store.get().model_path.is_none());
        assert!(store.load_warning.as_deref().unwrap().contains("couldn't be read"));
        assert!(path.with_extension("json.corrupt").is_file(), "corrupt file should be kept");
    }
}
