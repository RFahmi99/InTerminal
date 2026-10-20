//! One running PowerShell process attached to a Windows pseudo-console (ConPTY),
//! plus the named pipe its integration script reports prompt state over.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};

use super::input_state::{Activity, InputTracker};
use super::messages::{HelloInfo, PromptInfo, ShellMessage};
use super::output::OutputSink;
use super::quote::{encoded_command, first_control_char};
use crate::env_path;
use crate::error::{AppError, CmdResult, ErrorCode};

const INTEGRATION_SCRIPT: &str = include_str!("integration.ps1");
/// How long to wait for the integration script to connect. Generous because it
/// only runs after the user's profile, and some profiles (conda, oh-my-posh)
/// take several seconds.
const INTEGRATION_CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// Typed before GUI-initiated input on an empty line. Early in a session,
/// conhost/PSReadLine occasionally swallow the first key of injected input
/// (a stray escape from terminal reports combines with it into an Alt chord);
/// a leading space absorbs that loss and PowerShell otherwise ignores it.
const GUARD: &str = " ";

/// How long after the integration pipe closes to wait for the process exit.
const EXIT_GRACE: Duration = Duration::from_secs(2);

pub const EVENT_HELLO: &str = "shell:hello";
pub const EVENT_PROMPT: &str = "shell:prompt";
pub const EVENT_ACTIVITY: &str = "shell:activity";
pub const EVENT_INTEGRATION: &str = "shell:integration";
pub const EVENT_EXIT: &str = "shell:exit";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IntegrationStatus {
    Pending,
    Connected,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptEvent {
    pub session_id: u64,
    #[serde(flatten)]
    pub prompt: PromptInfo,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloEvent {
    pub session_id: u64,
    #[serde(flatten)]
    pub hello: HelloInfo,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    pub session_id: u64,
    pub busy: bool,
    pub line_dirty: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationEvent {
    pub session_id: u64,
    pub status: IntegrationStatus,
    /// Something the user should be told, e.g. why the tree stopped following.
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitEvent {
    pub session_id: u64,
    pub exit_code: Option<u32>,
    /// Set when VCE stopped the shell because it could no longer talk to it.
    pub reason: Option<String>,
}

/// Snapshot of a session, returned when the frontend attaches.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: u64,
    pub shell_path: String,
    pub cwd: Option<String>,
    pub hello: Option<HelloInfo>,
    pub integration: IntegrationStatus,
    pub integration_message: Option<String>,
    pub busy: bool,
    pub line_dirty: bool,
}

struct SessionState {
    input: InputTracker,
    /// Last filesystem location; the shell may currently be elsewhere.
    cwd: Option<PathBuf>,
    /// Whether the shell's current location is a filesystem folder (not e.g. `HKCU:\`).
    in_filesystem: bool,
    hello: Option<HelloInfo>,
    integration: IntegrationStatus,
    integration_message: Option<String>,
    warned_malformed: bool,
    exited: bool,
}

/// Lets the I/O threads end a session they can no longer serve: the child is
/// killed so the normal exit path runs and the user is offered a restart.
#[derive(Clone)]
struct FailureSignal {
    killer: Arc<Mutex<Box<dyn ChildKiller + Send + Sync>>>,
    reason: Arc<Mutex<Option<String>>>,
}

impl FailureSignal {
    fn fail(&self, reason: String) {
        log::warn!("{reason}");
        self.reason.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(reason);
        if let Err(e) = self.killer.lock().unwrap_or_else(|e| e.into_inner()).kill() {
            log::debug!("Stopping the shell after an I/O failure failed (it may have exited already): {e}");
        }
    }
}

pub struct ShellSession {
    pub id: u64,
    shell_path: PathBuf,
    app: AppHandle,
    /// Input bytes, written to the pseudo-console by a dedicated thread so a
    /// large paste never blocks the caller and keystrokes stay in order.
    input: mpsc::Sender<Vec<u8>>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    failure: FailureSignal,
    state: Mutex<SessionState>,
}

/// Prefers PowerShell 7 when it is on PATH, else the Windows PowerShell 5.1
/// that ships with every Windows 10/11 install.
pub fn find_shell() -> PathBuf {
    for dir in std::env::split_paths(&env_path::current()) {
        let candidate = dir.join("pwsh.exe");
        if candidate.is_file() {
            return candidate;
        }
    }
    let system_root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    system_root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
}

fn create_pipe(name: &str) -> std::io::Result<NamedPipeServer> {
    ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .max_instances(1)
        .access_outbound(false)
        .pipe_mode(PipeMode::Byte)
        .create(name)
}

fn control_char_error(what: &str, c: char) -> AppError {
    AppError::new(
        ErrorCode::InvalidInput,
        format!(
            "{what} contains a control character (U+{:04X}), which would act as a key press in the terminal, so VCE didn't type it.",
            c as u32
        ),
    )
}

impl ShellSession {
    /// Starts PowerShell in a new pseudo-console. Must be called from within
    /// the Tauri async runtime, because the named pipe registers with it.
    pub fn spawn(id: u64, app: AppHandle, output: Arc<OutputSink>, size: PtySize, start_dir: &Path) -> CmdResult<Arc<Self>> {
        let shell_path = find_shell();
        let shell_name = shell_path.display().to_string();
        let spawn_error =
            |what: &str, e: &dyn std::fmt::Display| AppError::new(ErrorCode::Io, format!("Could not start {shell_name}: {what}: {e}"));

        // Without the side channel the terminal still works; only the file
        // tree sync and error help are lost, so that's not a reason to fail.
        let pipe_name = format!(r"\\.\pipe\vce-{}-{}", std::process::id(), uuid::Uuid::new_v4().simple());
        let (pipe, pipe_error) = match create_pipe(&pipe_name) {
            Ok(pipe) => (Some(pipe), None),
            Err(e) => {
                log::warn!("Could not create the shell integration pipe {pipe_name}: {e}");
                (
                    None,
                    Some(format!("VCE couldn't connect to PowerShell ({e}), so the file tree and error help won't follow this session.")),
                )
            }
        };

        let pair = native_pty_system().openpty(size).map_err(|e| spawn_error("opening a pseudo-console failed", &e))?;
        let mut cmd = CommandBuilder::new(&shell_path);
        // Passing the script inline (rather than as a .ps1 file) means it runs
        // regardless of the user's execution policy, without weakening it.
        cmd.args(["-NoLogo", "-NoExit", "-EncodedCommand", &encoded_command(INTEGRATION_SCRIPT)]);
        cmd.cwd(start_dir);
        // Read PATH afresh so programs installed since VCE started are found.
        cmd.env("PATH", env_path::current());
        cmd.env("TERM_PROGRAM", "VCE");
        if pipe.is_some() {
            cmd.env("VCE_PIPE", &pipe_name);
        }
        let mut child = pair.slave.spawn_command(cmd).map_err(|e| spawn_error("launching the process failed", &e))?;
        drop(pair.slave);
        log::info!("Shell session {id} started {shell_name} (pid {:?})", child.process_id());

        let failure = FailureSignal { killer: Arc::new(Mutex::new(child.clone_killer())), reason: Arc::new(Mutex::new(None)) };
        let reader = pair.master.try_clone_reader().map_err(|e| spawn_error("reading its output failed", &e))?;
        let writer = pair.master.take_writer().map_err(|e| spawn_error("writing its input failed", &e))?;
        let (input, input_rx) = mpsc::channel();
        let input_failure = failure.clone();
        std::thread::Builder::new()
            .name(format!("pty-writer-{id}"))
            .spawn(move || pump_input(writer, &input_rx, &input_failure))
            .map_err(|e| spawn_error("starting the input writer failed", &e))?;

        let session = Arc::new(Self {
            id,
            shell_path,
            app,
            input,
            master: Mutex::new(Some(pair.master)),
            failure: failure.clone(),
            state: Mutex::new(SessionState {
                input: InputTracker::default(),
                cwd: Some(start_dir.to_path_buf()),
                in_filesystem: true,
                hello: None,
                integration: IntegrationStatus::Pending,
                integration_message: None,
                warned_malformed: false,
                exited: false,
            }),
        });

        std::thread::Builder::new()
            .name(format!("pty-reader-{id}"))
            .spawn(move || pump_output(id, reader, &output, &failure))
            .map_err(|e| spawn_error("starting the output reader failed", &e))?;

        let waiter = Arc::clone(&session);
        std::thread::Builder::new()
            .name(format!("pty-waiter-{id}"))
            .spawn(move || {
                let exit_code = match child.wait() {
                    Ok(status) => Some(status.exit_code()),
                    Err(e) => {
                        log::warn!("Could not read the exit code of shell session {id}: {e}");
                        None
                    }
                };
                waiter.on_exit(exit_code);
            })
            .map_err(|e| spawn_error("starting the exit watcher failed", &e))?;

        match pipe {
            Some(pipe) => {
                let listener = Arc::clone(&session);
                tauri::async_runtime::spawn(async move { listener.listen(pipe).await });
            }
            None => session.set_integration(IntegrationStatus::Unavailable, pipe_error),
        }
        Ok(session)
    }

    pub fn info(&self) -> SessionInfo {
        let state = self.lock_state();
        let activity = state.input.activity();
        SessionInfo {
            session_id: self.id,
            shell_path: self.shell_path.display().to_string(),
            cwd: state.cwd.as_ref().map(|p| p.display().to_string()),
            hello: state.hello.clone(),
            integration: state.integration,
            integration_message: state.integration_message.clone(),
            busy: activity.busy,
            line_dirty: activity.line_dirty,
        }
    }

    /// The last known filesystem folder, for display and context.
    pub fn cwd(&self) -> Option<PathBuf> {
        self.lock_state().cwd.clone()
    }

    /// The folder relative paths resolve against right now, or `None` when the
    /// shell is in a non-filesystem location such as the registry.
    pub fn filesystem_cwd(&self) -> Option<PathBuf> {
        let state = self.lock_state();
        state.in_filesystem.then(|| state.cwd.clone()).flatten()
    }

    pub fn is_exited(&self) -> bool {
        self.lock_state().exited
    }

    pub fn hello(&self) -> Option<HelloInfo> {
        self.lock_state().hello.clone()
    }

    /// Forwards keystrokes typed in the terminal.
    pub fn write_user_input(&self, data: &str) -> CmdResult<()> {
        self.ensure_running()?;
        self.track_input(data);
        self.write_raw(data)
    }

    /// Types `command` at the prompt and presses Enter. Refuses while a command
    /// is running or the user has unsent text on the line, so GUI actions never
    /// corrupt what the user is doing.
    pub fn run_command(&self, command: &str) -> CmdResult<()> {
        if let Some(c) = first_control_char(command) {
            return Err(control_char_error("This command", c));
        }
        self.ensure_idle(true)?;
        let line = format!("{GUARD}{command}\r");
        self.track_input(&line);
        self.write_raw(&line)
    }

    /// Types `text` at the cursor without submitting it, for the user to review.
    pub fn insert_text(&self, text: &str) -> CmdResult<()> {
        if let Some(c) = first_control_char(text) {
            return Err(control_char_error("This text", c));
        }
        self.ensure_idle(false)?;
        let line_dirty = self.lock_state().input.activity().line_dirty;
        let text = if line_dirty { text.to_string() } else { format!("{GUARD}{text}") };
        self.track_input(&text);
        self.write_raw(&text)
    }

    pub fn resize(&self, size: PtySize) -> CmdResult<()> {
        let master = self.master.lock().unwrap_or_else(|e| e.into_inner());
        let Some(master) = master.as_ref() else {
            return Err(AppError::new(ErrorCode::ShellNotRunning, "The shell has exited."));
        };
        master
            .resize(size)
            .map_err(|e| AppError::new(ErrorCode::Io, format!("Could not resize the terminal to {}x{}: {e}", size.cols, size.rows)))
    }

    pub fn kill(&self) {
        if let Err(e) = self.failure.killer.lock().unwrap_or_else(|e| e.into_inner()).kill() {
            // Already exited is the common case here and needs no action.
            log::debug!("Killing shell session {} failed: {e}", self.id);
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, SessionState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn ensure_running(&self) -> CmdResult<()> {
        if self.lock_state().exited {
            return Err(AppError::new(ErrorCode::ShellNotRunning, "The shell has exited. Restart it to continue."));
        }
        Ok(())
    }

    fn ensure_idle(&self, require_clean_line: bool) -> CmdResult<()> {
        let state = self.lock_state();
        if state.exited {
            return Err(AppError::new(ErrorCode::ShellNotRunning, "The shell has exited. Restart it to continue."));
        }
        let activity = state.input.activity();
        if activity.busy {
            let message = if state.hello.is_none() && state.integration == IntegrationStatus::Pending {
                "The shell is still starting. Try again in a moment."
            } else {
                "A command is still running. Wait for it to finish, or press Ctrl+C in the terminal to stop it."
            };
            return Err(AppError::new(ErrorCode::ShellBusy, message));
        }
        if require_clean_line && activity.line_dirty {
            return Err(AppError::new(
                ErrorCode::LineDirty,
                "The command line has text you haven't run yet. Press Enter to run it, or Esc to clear it, then try again.",
            ));
        }
        Ok(())
    }

    fn track_input(&self, data: &str) {
        let (before, after) = {
            let mut state = self.lock_state();
            let before = state.input.activity();
            state.input.on_input(data);
            (before, state.input.activity())
        };
        if before != after {
            self.emit_activity(after);
        }
    }

    fn write_raw(&self, data: &str) -> CmdResult<()> {
        self.input
            .send(data.as_bytes().to_vec())
            .map_err(|_| AppError::new(ErrorCode::ShellNotRunning, "The shell is no longer accepting input. Restart it to continue."))
    }

    fn emit_activity(&self, activity: Activity) {
        self.emit(EVENT_ACTIVITY, ActivityEvent { session_id: self.id, busy: activity.busy, line_dirty: activity.line_dirty });
    }

    fn emit<T: Serialize + Clone>(&self, event: &str, payload: T) {
        if let Err(e) = self.app.emit(event, payload) {
            log::warn!("Could not emit {event} for shell session {}: {e}", self.id);
        }
    }

    fn set_integration(&self, status: IntegrationStatus, message: Option<String>) {
        let activity = {
            let mut state = self.lock_state();
            state.integration = status;
            state.integration_message.clone_from(&message);
            if status == IntegrationStatus::Unavailable {
                state.input.assume_idle();
            }
            state.input.activity()
        };
        self.emit(EVENT_INTEGRATION, IntegrationEvent { session_id: self.id, status, message });
        self.emit_activity(activity);
    }

    async fn listen(self: Arc<Self>, pipe: NamedPipeServer) {
        match tokio::time::timeout(INTEGRATION_CONNECT_TIMEOUT, pipe.connect()).await {
            Ok(Ok(())) => {
                log::info!("Shell session {} integration connected", self.id);
                self.set_integration(IntegrationStatus::Connected, None);
            }
            Ok(Err(e)) => {
                return self.set_integration(IntegrationStatus::Unavailable, Some(format!("The shell integration pipe failed: {e}")));
            }
            Err(_) => {
                let message = format!(
                    "PowerShell didn't load VCE's integration within {} seconds, so the file tree won't follow this session.",
                    INTEGRATION_CONNECT_TIMEOUT.as_secs()
                );
                return self.set_integration(IntegrationStatus::Unavailable, Some(message));
            }
        }

        let mut lines = BufReader::new(pipe).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => match serde_json::from_str::<ShellMessage>(&line) {
                    Ok(message) => self.handle_message(message),
                    Err(e) => self.on_malformed_message(&line, &e),
                },
                Ok(None) => break,
                Err(e) => {
                    log::warn!("Shell integration pipe for session {} failed: {e}", self.id);
                    break;
                }
            }
        }
        // When PowerShell exits, its end of the pipe closes slightly before the
        // exit watcher records the exit; give it a moment so a normal `exit`
        // isn't reported as a lost connection.
        tokio::time::sleep(EXIT_GRACE).await;
        if !self.lock_state().exited {
            self.set_integration(
                IntegrationStatus::Unavailable,
                Some("The shell stopped reporting its state, so the file tree won't follow it any more.".into()),
            );
        }
    }

    /// The message was most likely a prompt report, so the shell is probably
    /// idle; don't leave GUI actions blocked on a "running" state that will
    /// never end. Warn the user once per session.
    fn on_malformed_message(&self, line: &str, error: &serde_json::Error) {
        log::warn!("Malformed shell integration message ({error}): {}", truncate(line, 300));
        let (activity, first) = {
            let mut state = self.lock_state();
            state.input.assume_idle();
            let first = !std::mem::replace(&mut state.warned_malformed, true);
            (state.input.activity(), first)
        };
        self.emit_activity(activity);
        if first {
            let message =
                "VCE received a status update from PowerShell it couldn't read; the file tree may be out of date until the next command.";
            self.emit(
                EVENT_INTEGRATION,
                IntegrationEvent { session_id: self.id, status: IntegrationStatus::Connected, message: Some(message.into()) },
            );
        }
    }

    fn handle_message(&self, message: ShellMessage) {
        match message {
            ShellMessage::Hello(hello) => {
                log::info!("Shell session {} is PowerShell {} ({})", self.id, hello.ps_version, hello.edition);
                self.lock_state().hello = Some(hello.clone());
                self.emit(EVENT_HELLO, HelloEvent { session_id: self.id, hello });
            }
            ShellMessage::Prompt(prompt) => {
                let activity = {
                    let mut state = self.lock_state();
                    state.in_filesystem = prompt.is_filesystem();
                    if state.in_filesystem {
                        state.cwd = prompt.cwd.as_ref().map(PathBuf::from);
                    }
                    state.input.on_prompt(prompt.edit_mode.as_deref());
                    state.input.activity()
                };
                self.emit(EVENT_PROMPT, PromptEvent { session_id: self.id, prompt });
                self.emit_activity(activity);
            }
        }
    }

    fn on_exit(&self, exit_code: Option<u32>) {
        let reason = self.failure.reason.lock().unwrap_or_else(|e| e.into_inner()).clone();
        log::info!("Shell session {} exited with {exit_code:?} ({})", self.id, reason.as_deref().unwrap_or("normal exit"));
        self.lock_state().exited = true;
        self.emit(EVENT_EXIT, ExitEvent { session_id: self.id, exit_code, reason });
        // ConPTY keeps the output pipe open after the child exits; closing the
        // pseudo-console lets the reader thread drain the last output and stop.
        // Closing can block for a while on some Windows builds, which is why
        // the exit is reported first.
        std::thread::sleep(Duration::from_millis(150));
        self.master.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

fn pump_input(mut writer: Box<dyn Write + Send>, input: &mpsc::Receiver<Vec<u8>>, failure: &FailureSignal) {
    // Ends when the session (the only sender) is dropped.
    while let Ok(bytes) = input.recv() {
        if let Err(e) = writer.write_all(&bytes).and_then(|()| writer.flush()) {
            failure.fail(format!("VCE lost the connection for sending keys to PowerShell ({e}), so it stopped the shell."));
            break;
        }
    }
}

fn pump_output(session_id: u64, mut reader: Box<dyn Read + Send>, output: &OutputSink, failure: &FailureSignal) {
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut buffer) {
            // A closed pseudo-console can surface as a broken pipe rather than EOF.
            Ok(0) => break,
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => break,
            Ok(n) => output.push(session_id, &buffer[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                failure.fail(format!("VCE lost PowerShell's output ({e}), so it stopped the shell."));
                break;
            }
        }
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integration_script_fits_on_a_command_line() {
        // CreateProcess caps the whole command line at 32,767 UTF-16 units.
        assert!(encoded_command(INTEGRATION_SCRIPT).len() < 30_000);
    }
}
