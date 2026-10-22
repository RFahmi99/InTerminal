//! Command indexing and semantic search engine.
//!
//! Built on a background thread at startup so the terminal is usable
//! immediately. If any part fails (missing model, PowerShell query error),
//! search degrades rather than disappearing, and the terminal is unaffected.

pub mod catalog;
pub mod embedding;
pub mod fuzzy;
mod help;
pub mod inventory;
pub mod search;
pub mod tldr;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{AppError, CmdResult, ErrorCode};
use catalog::Catalog;
use embedding::StaticEmbedder;
use inventory::{Inventory, PsQuery};
use search::{CommandIndex, Knowledge, SearchHit};

pub const EVENT_STATUS: &str = "index:status";
/// Versioned so a cache written by an older InTerminal (a different shape) is ignored.
const PS_COMMANDS_CACHE: &str = "ps-commands-v2.json";
const PS_USAGE_CACHE: &str = "ps-usage-v1.json";
const LEGACY_COMMANDS_CACHE: &str = "ps-commands.json";
/// How long cached parameter usage is trusted before it is rebuilt.
const USAGE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const TLDR_UNAVAILABLE: &str = "The bundled command examples couldn't be loaded, so search shows fewer examples.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexState {
    Building,
    /// Searchable, possibly degraded; see `IndexStatus::messages`.
    Ready,
    /// The indexer itself couldn't run; `messages` says why.
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub state: IndexState,
    /// Explains degraded operation (e.g. keyword-only search) or failure.
    pub messages: Vec<String>,
    pub documents: usize,
    pub programs: usize,
    pub ps_commands: usize,
    /// Cmdlets with a description from installed help (see `Update-Help`).
    pub help_descriptions: usize,
    /// tldr-pages examples that apply on this machine.
    pub examples: usize,
    pub semantic: bool,
}

impl Default for IndexStatus {
    fn default() -> Self {
        Self {
            state: IndexState::Building,
            messages: Vec::new(),
            documents: 0,
            programs: 0,
            ps_commands: 0,
            help_descriptions: 0,
            examples: 0,
            semantic: false,
        }
    }
}

#[derive(Default)]
pub struct IndexManager {
    index: RwLock<Option<Arc<CommandIndex>>>,
    status: Mutex<IndexStatus>,
    /// Incremented by each build; a build that is no longer the latest stops
    /// publishing, so an older PATH scan can't replace a newer one.
    build_attempt: AtomicU64,
}

impl IndexManager {
    pub fn get(&self) -> Option<Arc<CommandIndex>> {
        self.index.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn is_latest(&self, attempt: u64) -> bool {
        self.build_attempt.load(Ordering::SeqCst) == attempt
    }

    fn publish(&self, app: &AppHandle, index: CommandIndex, messages: Vec<String>) {
        let status = IndexStatus {
            state: IndexState::Ready,
            messages,
            documents: index.document_count(),
            programs: index.inventory().executables.len(),
            ps_commands: index.inventory().ps_commands.len(),
            help_descriptions: index.help_description_count(),
            examples: index.example_count(),
            semantic: index.has_semantic(),
        };
        *self.index.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(index));
        self.set_status(app, status);
    }

    fn set_status(&self, app: &AppHandle, status: IndexStatus) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = status.clone();
        if let Err(e) = app.emit(EVENT_STATUS, status) {
            log::warn!("Could not emit {EVENT_STATUS}: {e}");
        }
    }

    fn status(&self) -> IndexStatus {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn resource(app: &AppHandle, relative: &str) -> Result<PathBuf, String> {
    app.path().resolve(relative, BaseDirectory::Resource).map_err(|e| format!("locating bundled {relative}: {e}"))
}

fn cache_path(app: &AppHandle, file: &str) -> Option<PathBuf> {
    match app.path().app_cache_dir() {
        Ok(dir) => Some(dir.join(file)),
        Err(e) => {
            log::warn!("No cache folder for {file}, so it is rebuilt every launch: {e}");
            None
        }
    }
}

fn read_cache<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!("Could not read cache {}: {e}", path.display());
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(value) => Some(value),
        Err(e) => {
            log::warn!("Ignoring unreadable cache {}: {e}", path.display());
            None
        }
    }
}

/// Writes to a temporary file first, so closing InTerminal mid-write can't leave a
/// truncated cache behind.
fn write_cache<T: Serialize>(path: &Path, value: &T) {
    let temporary = path.with_extension("tmp");
    let result = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| serde_json::to_vec(value).map_err(std::io::Error::other))
        .and_then(|json| std::fs::write(&temporary, json))
        .and_then(|()| std::fs::rename(&temporary, path));
    if let Err(e) = result {
        log::warn!("Could not write cache {}: {e}", path.display());
    }
}

/// Deletes the command cache older versions of InTerminal wrote in another format.
fn remove_legacy_cache(app: &AppHandle) {
    let Some(path) = cache_path(app, LEGACY_COMMANDS_CACHE) else {
        return;
    };
    match std::fs::remove_file(&path) {
        Ok(()) => log::info!("Removed outdated cache {}", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!("Could not remove outdated cache {}: {e}", path.display()),
    }
}

/// Whether the parameter usage cache is missing or old enough to rebuild.
fn usage_is_stale(path: Option<&Path>) -> bool {
    let age =
        path.and_then(|p| std::fs::metadata(p).ok()).and_then(|meta| meta.modified().ok()).and_then(|modified| modified.elapsed().ok());
    age.is_none_or(|age| age > USAGE_MAX_AGE)
}

/// Builds the index in passes, each published as soon as it's ready:
/// 1. PATH plus the cached PowerShell command list (instant);
/// 2. a fresh command list (a second or two);
/// 3. when `refresh_usage` is set or the cache is stale or unreadable, fresh
///    parameter usage, which needs every module loaded (about a minute).
///
/// Stops publishing once a newer build (`attempt`) has started.
fn build(app: &AppHandle, manager: &IndexManager, attempt: u64, refresh_usage: bool) {
    let mut messages = Vec::new();
    let catalog = match resource(app, "resources/catalog.json").and_then(|p| Catalog::load(&p)) {
        Ok(catalog) => catalog,
        Err(e) => {
            log::error!("Command catalog unavailable: {e}");
            messages.push("The command catalog couldn't be loaded, so search only knows command names.".to_string());
            Catalog::default()
        }
    };
    let embedder = match resource(app, "resources/embedding").and_then(|p| StaticEmbedder::load(&p)) {
        Ok(embedder) => Some(Arc::new(embedder)),
        Err(e) => {
            log::error!("Embedding model unavailable: {e}");
            messages.push("The search model couldn't be loaded, so search matches keywords only.".to_string());
            None
        }
    };
    let tldr = match resource(app, "resources/tldr/pages.md")
        .and_then(|p| std::fs::read_to_string(&p).map_err(|e| format!("reading {}: {e}", p.display())))
    {
        Ok(text) => {
            let pages = tldr::parse_bundle(&text);
            if pages.is_empty() {
                log::error!("tldr-pages examples unavailable: the bundle has no pages in the expected format");
                messages.push(TLDR_UNAVAILABLE.to_string());
            }
            pages
        }
        Err(e) => {
            log::error!("tldr-pages examples unavailable: {e}");
            messages.push(TLDR_UNAVAILABLE.to_string());
            Vec::new()
        }
    };
    let executables = inventory::scan_current_path();
    let commands_cache = cache_path(app, PS_COMMANDS_CACHE);
    let usage_cache = cache_path(app, PS_USAGE_CACHE);
    remove_legacy_cache(app);
    let cached_usages: Option<HashMap<String, String>> = usage_cache.as_deref().and_then(read_cache);
    // An unreadable cache is rebuilt now rather than when it would expire.
    let usage_outdated = refresh_usage || cached_usages.is_none() || usage_is_stale(usage_cache.as_deref());
    let mut usages = cached_usages.unwrap_or_default();
    let shell = crate::shell::find_shell();
    // Only pages for commands that exist here are kept in the published index
    // (a few hundred of ~5,000); the full parse is dropped when this build ends.
    let knowledge = |inventory: &Inventory, query: &PsQuery| {
        let pages = tldr.iter().filter(|page| inventory.has(&page.program())).cloned().collect();
        Knowledge { synopses: Arc::new(help::load_synopses(&query.help_roots)), tldr: Arc::new(pages) }
    };
    let publish = |query: &PsQuery, usages: &HashMap<String, String>, messages: &[String]| {
        let mut commands = query.commands.clone();
        inventory::apply_usage(&mut commands, usages);
        let inventory = Inventory::new(executables.clone(), commands);
        let knowledge = knowledge(&inventory, query);
        manager.publish(app, CommandIndex::build_with(catalog.clone(), inventory, embedder.clone(), knowledge), messages.to_vec());
    };
    // Shows a failure straight away; later publishes keep it, as it is in `messages`.
    let report = |messages: &[String]| {
        let mut status = manager.status();
        status.messages = messages.to_vec();
        manager.set_status(app, status);
    };
    let superseded = || {
        let superseded = !manager.is_latest(attempt);
        if superseded {
            log::info!("Index build {attempt} stopped: a newer one has started");
        }
        superseded
    };

    let cached: PsQuery = commands_cache.as_deref().and_then(read_cache).unwrap_or_default();
    if superseded() {
        return;
    }
    publish(&cached, &usages, &messages);

    let query = inventory::query_ps_commands(&shell);
    if superseded() {
        return;
    }
    let query = match query {
        Ok(query) => {
            if let Some(cache) = &commands_cache {
                write_cache(cache, &query);
            }
            log::info!("Indexed {} PowerShell commands and {} tldr pages", query.commands.len(), tldr.len());
            publish(&query, &usages, &messages);
            query
        }
        Err(e) => {
            log::error!("Listing PowerShell commands failed: {e}");
            messages.push("PowerShell's command list couldn't be read, so some cmdlets may be missing from search.".into());
            report(&messages);
            cached
        }
    };

    if usage_outdated {
        let fresh = inventory::query_ps_usage(&shell);
        if superseded() {
            return;
        }
        match fresh {
            Ok(fresh) => {
                log::info!("Read parameter usage for {} PowerShell commands", fresh.len());
                if let Some(cache) = &usage_cache {
                    write_cache(cache, &fresh);
                }
                usages = fresh;
                publish(&query, &usages, &messages);
            }
            Err(e) => {
                log::error!("Reading PowerShell parameter usage failed: {e}");
                messages.push("PowerShell commands' parameter lists couldn't be updated, so some may be missing or out of date.".into());
                report(&messages);
            }
        }
    }
}

fn mark_failed(app: &AppHandle, manager: &IndexManager, message: String) {
    log::error!("{message}");
    let mut status = manager.status();
    status.state = IndexState::Failed;
    status.messages.push(message);
    manager.set_status(app, status);
}

pub fn start_background_build(app: AppHandle, refresh_usage: bool) {
    let attempt = app.state::<IndexManager>().build_attempt.fetch_add(1, Ordering::SeqCst) + 1;
    let worker = app.clone();
    let spawned = std::thread::Builder::new().name("command-index".into()).spawn(move || {
        let manager = worker.state::<IndexManager>();
        // A panic here must not leave search stuck on "building" forever.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&worker, &manager, attempt, refresh_usage)));
        if let Err(panic) = outcome {
            let detail = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("unknown error");
            mark_failed(&worker, &manager, format!("Command search stopped while indexing ({detail}). The terminal still works."));
        }
    });
    if let Err(e) = spawned {
        let manager = app.state::<IndexManager>();
        mark_failed(&app, &manager, format!("Command search couldn't start ({e}). The terminal still works."));
    }
}

#[tauri::command]
pub fn index_status(manager: State<'_, IndexManager>) -> IndexStatus {
    manager.status()
}

#[tauri::command]
pub fn index_refresh(app: AppHandle, manager: State<'_, IndexManager>) {
    let mut status = manager.status();
    status.state = IndexState::Building;
    manager.set_status(&app, status);
    start_background_build(app, true);
}

#[tauri::command]
pub async fn search_commands(manager: State<'_, IndexManager>, query: String, limit: Option<usize>) -> CmdResult<Vec<SearchHit>> {
    let Some(index) = manager.get() else {
        let status = manager.status();
        let message = match status.state {
            IndexState::Failed => status.messages.last().cloned().unwrap_or_else(|| "Command search couldn't start.".into()),
            _ => "Command search is still getting ready. Try again in a moment.".into(),
        };
        return Err(AppError::new(ErrorCode::IndexUnavailable, message));
    };
    Ok(index.search(&query, limit.unwrap_or(8).min(25)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_cache_is_stale_when_missing_or_older_than_a_week() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.json");
        assert!(usage_is_stale(None));
        assert!(usage_is_stale(Some(&path)), "a missing file is stale");
        std::fs::write(&path, "{}").unwrap();
        assert!(!usage_is_stale(Some(&path)));
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(std::time::SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60)).unwrap();
        assert!(usage_is_stale(Some(&path)));
    }

    #[test]
    fn caches_round_trip_and_unreadable_ones_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("usage.json");
        let usages = HashMap::from([("new-volume".to_string(), "New-Volume -FriendlyName <string>".to_string())]);
        write_cache(&path, &usages);
        assert_eq!(read_cache::<HashMap<String, String>>(&path), Some(usages));
        assert!(!path.with_extension("tmp").exists(), "the temporary file is renamed into place");
        std::fs::write(&path, "{").unwrap();
        assert_eq!(read_cache::<HashMap<String, String>>(&path), None);
        assert_eq!(read_cache::<HashMap<String, String>>(&dir.path().join("missing.json")), None);
    }
}
