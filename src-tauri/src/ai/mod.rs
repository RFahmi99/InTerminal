//! Lazy-loaded local AI command generation. Nothing AI-related is in memory
//! until the user asks for a command; the engine is started for that one
//! request and stopped as soon as it answers, fails or is cancelled.

mod model;
mod prompt;
mod server;

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{AppError, CmdResult, ErrorCode};
use crate::index::IndexManager;
use crate::settings::SettingsStore;
use crate::shell::ShellManager;
use model::{DEFAULT_MODEL, DownloadProgress};
use prompt::{GeneratedCommand, NOTABLE_TOOLS, PromptContext};
use server::LlamaServer;

pub const EVENT_PROGRESS: &str = "ai:progress";
pub const EVENT_DOWNLOAD: &str = "ai:download";
const GENERATION_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_REQUEST_CHARS: usize = 1000;
const MAX_FOLDER_ENTRIES: usize = 40;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Loading,
    Generating,
    Unloaded,
}

#[derive(Default)]
struct Generation {
    active: bool,
    server_pid: Option<u32>,
}

#[derive(Default)]
pub struct AiManager {
    generation: Mutex<Generation>,
    cancel_generation: AtomicBool,
    downloading: AtomicBool,
    /// Identifies the current download; cancelling moves it on, which
    /// abandons the old attempt even if it is stuck in a network read.
    download_attempt: AtomicU64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultModelInfo {
    pub name: &'static str,
    pub bytes: u64,
    pub license: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStatus {
    /// Whether the bundled llama.cpp engine is present.
    pub engine_available: bool,
    pub model_path: Option<String>,
    pub model_ready: bool,
    pub using_default_model: bool,
    pub default_model: DefaultModelInfo,
    pub generating: bool,
    pub downloading: bool,
}

fn engine_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().resolve("resources/llama/llama-server.exe", BaseDirectory::Resource).ok().filter(|p| p.is_file())
}

fn models_dir(app: &AppHandle) -> CmdResult<PathBuf> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("models"))
        .map_err(|e| AppError::new(ErrorCode::Io, format!("Could not locate the app data folder: {e}")))
}

/// The model to load: the user's choice, else the default download.
fn model_path(app: &AppHandle, settings: &SettingsStore) -> CmdResult<(PathBuf, bool)> {
    match settings.get().model_path {
        Some(custom) => Ok((PathBuf::from(custom), false)),
        None => Ok((model::default_model_path(&models_dir(app)?), true)),
    }
}

fn emit<T: Serialize + Clone>(app: &AppHandle, event: &str, payload: T) {
    if let Err(e) = app.emit(event, payload) {
        log::warn!("Could not emit {event}: {e}");
    }
}

/// Clears the generation slot however `ai_generate` exits.
struct GenerationGuard<'a>(&'a AiManager);

impl Drop for GenerationGuard<'_> {
    fn drop(&mut self) {
        *self.0.generation.lock().unwrap_or_else(|e| e.into_inner()) = Generation::default();
    }
}

fn gather_context(app: &AppHandle, request: &str) -> (PromptContext, Option<std::sync::Arc<crate::index::search::CommandIndex>>) {
    let shell = app.state::<ShellManager>();
    let index = app.state::<IndexManager>().get();
    let cwd = shell.cwd_or_home(app);
    let hello = shell.hello();
    let folder_entries = cwd
        .as_deref()
        .and_then(|dir| crate::fs_ops::list_dir(dir, false).ok())
        .map(|listing| {
            listing
                .entries
                .iter()
                .take(MAX_FOLDER_ENTRIES)
                .map(|e| if e.is_dir { format!("{}\\", e.name) } else { e.name.clone() })
                .collect()
        })
        .unwrap_or_default();
    let (installed_tools, recipes) = match &index {
        Some(index) => (
            NOTABLE_TOOLS.iter().filter(|t| index.inventory().has(t)).map(|t| (*t).to_string()).collect(),
            index.relevant_recipes(request, 4),
        ),
        None => (Vec::new(), Vec::new()),
    };
    let context = PromptContext {
        cwd: cwd.map(|p| p.display().to_string()),
        folder_entries,
        os_version: hello.as_ref().map(|h| h.os_version.clone()).filter(|v| !v.is_empty()),
        ps_version: hello.map(|h| h.ps_version),
        installed_tools,
        recipes,
    };
    (context, index)
}

fn complete(server: &LlamaServer, context: &PromptContext, request: &str) -> CmdResult<String> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder().timeout_global(Some(GENERATION_TIMEOUT)).http_status_as_error(false).build(),
    );
    let body = json!({
        "messages": prompt::messages(context, request),
        "temperature": 0.2,
        "top_p": 0.9,
        "max_tokens": 400,
        "response_format": {"type": "json_schema", "json_schema": {"name": "command", "schema": prompt::reply_schema()}},
    });
    let failed = |what: String| AppError::new(ErrorCode::AiFailed, format!("The AI engine failed to answer: {what}"));
    let mut response = agent
        .post(format!("{}/v1/chat/completions", server.base_url))
        .header("Authorization", format!("Bearer {}", server.api_key))
        .send_json(body)
        .map_err(|e| failed(e.to_string()))?;
    let status = response.status();
    let text = response.body_mut().read_to_string().map_err(|e| failed(e.to_string()))?;
    if status != 200 {
        return Err(failed(format!("HTTP {status}: {}", text.chars().take(300).collect::<String>())));
    }
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| failed(format!("unreadable response ({e})")))?;
    value["choices"][0]["message"]["content"].as_str().map(str::to_string).ok_or_else(|| failed("the response had no message".into()))
}

#[tauri::command]
pub fn ai_status(app: AppHandle, ai: State<'_, AiManager>, settings: State<'_, SettingsStore>) -> CmdResult<AiStatus> {
    let (path, using_default) = model_path(&app, &settings)?;
    Ok(AiStatus {
        engine_available: engine_path(&app).is_some(),
        model_ready: path.is_file(),
        model_path: Some(path.display().to_string()),
        using_default_model: using_default,
        default_model: DefaultModelInfo { name: DEFAULT_MODEL.name, bytes: DEFAULT_MODEL.bytes, license: DEFAULT_MODEL.license },
        generating: ai.generation.lock().unwrap_or_else(|e| e.into_inner()).active,
        downloading: ai.downloading.load(Ordering::Relaxed),
    })
}

/// Downloads the default model. Only ever called after the user agreed to
/// the download in the UI.
#[tauri::command]
pub async fn ai_download_model(app: AppHandle) -> CmdResult<String> {
    let ai = app.state::<AiManager>();
    if ai.downloading.swap(true, Ordering::SeqCst) {
        return Err(AppError::new(ErrorCode::InvalidInput, "The model is already downloading."));
    }
    let attempt = ai.download_attempt.fetch_add(1, Ordering::SeqCst) + 1;
    let worker = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let ai = worker.state::<AiManager>();
        let dir = models_dir(&worker)?;
        let cancelled = || ai.download_attempt.load(Ordering::SeqCst) != attempt;
        model::download_default(&dir, attempt, cancelled, |progress: DownloadProgress| emit(&worker, EVENT_DOWNLOAD, progress))
    })
    .await
    .map_err(|e| AppError::new(ErrorCode::AiFailed, format!("The download task failed: {e}")));
    // A cancelled attempt has already handed the flag back.
    if ai.download_attempt.load(Ordering::SeqCst) == attempt {
        ai.downloading.store(false, Ordering::SeqCst);
    }
    let path = result??;
    // Downloading the default means the user wants it, even if a custom model
    // (perhaps since deleted) was selected before.
    app.state::<SettingsStore>().update(|s| s.model_path = None)?;
    Ok(path.display().to_string())
}

/// Abandons the current download straight away. Its worker stops at its next
/// read and deletes its partial file.
#[tauri::command]
pub fn ai_cancel_download(ai: State<'_, AiManager>) {
    ai.download_attempt.fetch_add(1, Ordering::SeqCst);
    ai.downloading.store(false, Ordering::SeqCst);
}

/// Uses a GGUF model the user picked instead of the default.
#[tauri::command]
pub fn ai_choose_model(settings: State<'_, SettingsStore>, path: String) -> CmdResult<()> {
    model::validate_gguf(&PathBuf::from(&path))?;
    settings.update(|s| s.model_path = Some(path)).map(|_| ())
}

#[tauri::command]
pub fn ai_use_default_model(settings: State<'_, SettingsStore>) -> CmdResult<()> {
    settings.update(|s| s.model_path = None).map(|_| ())
}

/// Generates a command for `request`. The result is only ever shown for
/// review; running it is a separate, explicit user action.
#[tauri::command]
pub async fn ai_generate(app: AppHandle, request: String) -> CmdResult<GeneratedCommand> {
    let request = request.trim().to_string();
    if request.is_empty() {
        return Err(AppError::new(ErrorCode::InvalidInput, "Describe what you want the command to do."));
    }
    if request.chars().count() > MAX_REQUEST_CHARS {
        return Err(AppError::new(ErrorCode::InvalidInput, format!("Please keep the request under {MAX_REQUEST_CHARS} characters.")));
    }
    let worker = app.clone();
    tauri::async_runtime::spawn_blocking(move || generate_blocking(&worker, &request))
        .await
        .map_err(|e| AppError::new(ErrorCode::AiFailed, format!("The generation task failed: {e}")))?
}

fn generate_blocking(app: &AppHandle, request: &str) -> CmdResult<GeneratedCommand> {
    let ai = app.state::<AiManager>();
    {
        let mut generation = ai.generation.lock().unwrap_or_else(|e| e.into_inner());
        if generation.active {
            return Err(AppError::new(ErrorCode::InvalidInput, "A command is already being generated."));
        }
        generation.active = true;
    }
    let _guard = GenerationGuard(&ai);
    ai.cancel_generation.store(false, Ordering::SeqCst);

    let engine = engine_path(app).ok_or_else(|| {
        AppError::new(
            ErrorCode::AiUnavailable,
            "The AI engine isn't installed with this copy of InTerminal, so command generation is unavailable.",
        )
    })?;
    let (model, using_default) = model_path(app, &app.state::<SettingsStore>())?;
    if !model.is_file() {
        let message = if using_default {
            "The AI model hasn't been downloaded yet.".to_string()
        } else {
            format!("The chosen model file \"{}\" no longer exists.", model.display())
        };
        return Err(AppError::new(ErrorCode::AiUnavailable, message));
    }
    let (context, index) = gather_context(app, request);

    emit(app, EVENT_PROGRESS, Stage::Loading);
    let should_stop = || ai.cancel_generation.load(Ordering::SeqCst);
    let result = LlamaServer::start(&engine, &model, should_stop).and_then(|server| {
        ai.generation.lock().unwrap_or_else(|e| e.into_inner()).server_pid = Some(server.process_id());
        emit(app, EVENT_PROGRESS, Stage::Generating);
        let raw = complete(&server, &context, request);
        let engine_note = server.stderr_summary();
        // Unregister before the process is reaped, so a late cancel can never
        // target a PID Windows has already handed to another process.
        ai.generation.lock().unwrap_or_else(|e| e.into_inner()).server_pid = None;
        drop(server); // Unload the model now, before any post-processing.
        raw.map_err(|e| {
            if should_stop() {
                AppError::new(ErrorCode::Cancelled, "Command generation was cancelled.")
            } else {
                AppError::new(e.code, format!("{} {engine_note}", e.message))
            }
        })
    });
    emit(app, EVENT_PROGRESS, Stage::Unloaded);

    let raw = result?;
    prompt::finish(&raw, &context, index.as_deref().map(|i| i.inventory())).ok_or_else(|| {
        log::warn!("Unparseable model reply: {}", raw.chars().take(500).collect::<String>());
        AppError::new(ErrorCode::AiFailed, "The model's answer couldn't be understood. Try rephrasing the request.")
    })
}

#[tauri::command]
pub fn ai_cancel(ai: State<'_, AiManager>) {
    ai.cancel_generation.store(true, Ordering::SeqCst);
    // Kill while holding the lock: the generator clears the PID under the same
    // lock before reaping the process, so the PID is still ours here.
    let generation = ai.generation.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(pid) = generation.server_pid {
        server::kill_process(pid);
    }
}
