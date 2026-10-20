//! Backend shell controller: owns the PowerShell session and brokers input,
//! output and prompt state between it and the frontend.

mod input_state;
mod messages;
mod output;
pub mod quote;
mod session;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::PtySize;
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, CmdResult, ErrorCode};
pub use messages::{HelloInfo, PsError};
use output::OutputSink;
pub use session::find_shell;
use session::{SessionInfo, ShellSession};

#[derive(Default)]
pub struct ShellManager {
    session: Mutex<Option<Arc<ShellSession>>>,
    output: Arc<OutputSink>,
    next_id: AtomicU64,
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize { cols: cols.clamp(10, 1000), rows: rows.clamp(2, 500), pixel_width: 0, pixel_height: 0 }
}

impl ShellManager {
    pub fn current(&self) -> CmdResult<Arc<ShellSession>> {
        self.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| AppError::new(ErrorCode::ShellNotRunning, "The shell hasn't started yet."))
    }

    /// The shell's working directory, or the user's home when unknown.
    pub fn cwd_or_home(&self, app: &AppHandle) -> Option<PathBuf> {
        self.current().ok().and_then(|s| s.cwd()).or_else(|| app.path().home_dir().ok())
    }

    pub fn hello(&self) -> Option<HelloInfo> {
        self.current().ok().and_then(|s| s.hello())
    }

    /// Starts a new shell, replacing any current one. With `reuse_running`,
    /// a running shell is kept instead; checking and starting under one lock
    /// means two attaches at once (e.g. a reload during start-up) can't both
    /// spawn a shell.
    fn start(&self, app: &AppHandle, size: PtySize, reuse_running: bool) -> CmdResult<Arc<ShellSession>> {
        let mut slot = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if reuse_running && let Some(session) = slot.as_ref().filter(|s| !s.is_exited()) {
            return Ok(Arc::clone(session));
        }
        if let Some(old) = slot.take() {
            old.kill();
        }
        let start_dir = app.path().home_dir().unwrap_or_else(|_| PathBuf::from(r"C:\"));
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.output.start_session(id);
        let session = ShellSession::spawn(id, app.clone(), Arc::clone(&self.output), size, &start_dir)?;
        *slot = Some(Arc::clone(&session));
        Ok(session)
    }

    /// Kills the shell; called when the app exits.
    pub fn shutdown(&self) {
        if let Some(session) = self.session.lock().unwrap_or_else(|e| e.into_inner()).take() {
            session.kill();
        }
    }
}

/// Connects the terminal view to the shell, starting one if needed. Calling it
/// again (e.g. after a webview reload) re-attaches and replays recent output.
#[tauri::command]
pub async fn shell_attach(
    app: AppHandle,
    manager: State<'_, ShellManager>,
    output: Channel<Response>,
    cols: u16,
    rows: u16,
) -> CmdResult<SessionInfo> {
    manager.output.attach(output);
    Ok(manager.start(&app, pty_size(cols, rows), true)?.info())
}

#[tauri::command]
pub async fn shell_restart(app: AppHandle, manager: State<'_, ShellManager>, cols: u16, rows: u16) -> CmdResult<SessionInfo> {
    Ok(manager.start(&app, pty_size(cols, rows), false)?.info())
}

/// Keystrokes from the terminal. Deliberately synchronous: sync commands run
/// in arrival order, which keeps typed characters in order.
#[tauri::command]
pub fn shell_write(manager: State<'_, ShellManager>, data: String) -> CmdResult<()> {
    manager.current()?.write_user_input(&data)
}

#[tauri::command]
pub fn shell_resize(manager: State<'_, ShellManager>, cols: u16, rows: u16) -> CmdResult<()> {
    manager.current()?.resize(pty_size(cols, rows))
}

/// Navigates the shell to `path` by typing a visible `cd` command, so the
/// terminal stays the single source of truth and the user sees the
/// equivalent command. Returns the command that was run.
#[tauri::command]
pub fn shell_change_dir(manager: State<'_, ShellManager>, path: String) -> CmdResult<String> {
    let target = Path::new(&path);
    if !target.is_dir() {
        return Err(AppError::new(ErrorCode::NotFound, format!("\"{path}\" is not a folder that exists.")));
    }
    if let Some(c) = quote::first_control_char(&path) {
        return Err(AppError::new(
            ErrorCode::InvalidInput,
            format!(
                "This folder's name contains a control character (U+{:04X}) that can't be typed into the terminal safely. Open it from the terminal instead.",
                c as u32
            ),
        ));
    }
    let session = manager.current()?;
    // Relative `cd` only when the shell is in a folder; from e.g. HKCU:\ a
    // relative path would resolve inside the registry.
    let command = quote::change_dir_command(target, session.filesystem_cwd().as_deref());
    session.run_command(&command)?;
    Ok(command)
}

/// Runs a command the user explicitly approved (a suggestion, search result
/// or reviewed AI command).
#[tauri::command]
pub fn shell_run(manager: State<'_, ShellManager>, command: String) -> CmdResult<()> {
    let command = quote::to_single_line(&command);
    if command.is_empty() {
        return Err(AppError::new(ErrorCode::InvalidInput, "There is no command to run."));
    }
    manager.current()?.run_command(&command)
}

/// Types text at the prompt without running it, for the user to edit first.
#[tauri::command]
pub fn shell_insert(manager: State<'_, ShellManager>, text: String) -> CmdResult<()> {
    let text = quote::to_single_line(&text);
    if text.is_empty() {
        return Err(AppError::new(ErrorCode::InvalidInput, "There is no text to insert."));
    }
    manager.current()?.insert_text(&text)
}
