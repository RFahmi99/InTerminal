//! Runs llama.cpp's `llama-server` as a short-lived child process: started
//! for one generation, then killed so all of the model's memory returns to
//! the OS. A Windows Job Object ensures the server dies with the app even if
//! the app crashes, so a 500 MB process is never left behind.

use std::collections::VecDeque;
use std::io::Read;
use std::net::TcpListener;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};

use crate::error::{AppError, CmdResult, ErrorCode};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(90);
const STDERR_TAIL_BYTES: usize = 4096;

/// A job object that kills every process assigned to it when the handle
/// closes, including when this process exits abnormally.
struct KillOnCloseJob(HANDLE);

// SAFETY: a job handle is a kernel object reference with no thread affinity.
unsafe impl Send for KillOnCloseJob {}
unsafe impl Sync for KillOnCloseJob {}

impl KillOnCloseJob {
    fn new() -> std::io::Result<Self> {
        // SAFETY: null security attributes and name are valid; the returned
        // handle is checked and owned by Self.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let job = Self(handle);
        // SAFETY: zeroed is a valid bit pattern for this plain C struct.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `info` is the struct this information class expects and
        // outlives the call.
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(job)
    }

    fn assign(&self, child: &Child) -> std::io::Result<()> {
        // SAFETY: both handles are valid for the duration of the call.
        let ok = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) };
        if ok == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
    }
}

impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        // SAFETY: the handle was created by CreateJobObjectW and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}

pub struct LlamaServer {
    child: Child,
    _job: KillOnCloseJob,
    pub base_url: String,
    pub api_key: String,
    stderr_tail: Arc<Mutex<VecDeque<u8>>>,
}

fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

fn generation_threads() -> usize {
    let logical = std::thread::available_parallelism().map_or(4, |n| n.get());
    (logical / 2).clamp(2, 8)
}

impl LlamaServer {
    /// Starts the server and waits until the model has finished loading.
    /// `should_stop` is polled so a cancel during loading takes effect quickly.
    pub fn start(server_exe: &Path, model: &Path, should_stop: impl Fn() -> bool) -> CmdResult<Self> {
        let unavailable = |what: String| AppError::new(ErrorCode::AiUnavailable, what);
        let port = free_port().map_err(|e| unavailable(format!("No free local port for the AI engine: {e}")))?;
        let api_key = uuid::Uuid::new_v4().simple().to_string();
        let job = KillOnCloseJob::new().map_err(|e| unavailable(format!("Could not create a job object for the AI engine: {e}")))?;

        let mut child = Command::new(server_exe)
            .args(["--model"])
            .arg(model)
            .args(["--host", "127.0.0.1", "--port", &port.to_string(), "--api-key", &api_key])
            .args(["--ctx-size", "4096", "--parallel", "1", "--threads", &generation_threads().to_string()])
            // No web UI, no network model fetching, no host-memory prompt cache.
            .args(["--no-webui", "--offline", "--cache-ram", "0", "--no-warmup"])
            .current_dir(server_exe.parent().unwrap_or(Path::new(".")))
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| unavailable(format!("Could not start the AI engine ({}): {e}", server_exe.display())))?;
        if let Err(e) = job.assign(&child) {
            if let Err(kill_error) = child.kill() {
                log::warn!("Could not stop llama-server after failing to attach it to a job object: {kill_error}");
            }
            return Err(unavailable(format!("Could not attach the AI engine to a job object: {e}")));
        }

        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_BYTES)));
        if let Some(mut stderr) = child.stderr.take() {
            let tail = Arc::clone(&stderr_tail);
            std::thread::spawn(move || {
                let mut buffer = [0u8; 4096];
                while let Ok(n @ 1..) = stderr.read(&mut buffer) {
                    let mut tail = tail.lock().unwrap_or_else(|e| e.into_inner());
                    tail.extend(&buffer[..n]);
                    let excess = tail.len().saturating_sub(STDERR_TAIL_BYTES);
                    tail.drain(..excess);
                }
            });
        }

        let mut server = Self { child, _job: job, base_url: format!("http://127.0.0.1:{port}"), api_key, stderr_tail };
        server.wait_until_ready(&should_stop)?;
        Ok(server)
    }

    fn wait_until_ready(&mut self, should_stop: &impl Fn() -> bool) -> CmdResult<()> {
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(2))).http_status_as_error(false).build(),
        );
        let started = Instant::now();
        loop {
            if should_stop() {
                return Err(AppError::new(ErrorCode::Cancelled, "Command generation was cancelled."));
            }
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    return Err(AppError::new(
                        ErrorCode::AiUnavailable,
                        format!("The AI engine stopped while loading the model ({status}). {}", self.stderr_summary()),
                    ));
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(AppError::new(ErrorCode::AiUnavailable, format!("Couldn't check on the AI engine while it loaded: {e}")));
                }
            }
            // 503 means "still loading"; connection errors mean "not listening yet".
            if let Ok(response) = agent.get(format!("{}/health", self.base_url)).call() {
                if response.status() == 200 {
                    return Ok(());
                }
            }
            if started.elapsed() > STARTUP_TIMEOUT {
                return Err(AppError::new(
                    ErrorCode::AiUnavailable,
                    format!("The AI engine didn't finish loading within {} seconds. {}", STARTUP_TIMEOUT.as_secs(), self.stderr_summary()),
                ));
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// The last line llama.cpp logged, which usually names the problem.
    pub fn stderr_summary(&self) -> String {
        let tail = self.stderr_tail.lock().unwrap_or_else(|e| e.into_inner());
        let text = String::from_utf8_lossy(&tail.iter().copied().collect::<Vec<u8>>()).into_owned();
        text.lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| format!("Last engine message: {}", l.trim()))
            .unwrap_or_default()
    }

    pub fn process_id(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for LlamaServer {
    fn drop(&mut self) {
        if let Err(e) = self.child.kill() {
            log::debug!("Stopping llama-server failed (it may have exited already): {e}");
        }
        if let Err(e) = self.child.wait() {
            log::warn!("Waiting for llama-server to exit failed: {e}");
        }
    }
}

/// Kills a running server by process ID from another thread (used to cancel
/// an in-flight generation). The owning `LlamaServer` still reaps it.
pub fn kill_process(pid: u32) {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    // SAFETY: OpenProcess returns null on failure, which is checked; the
    // handle is closed exactly once.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            log::debug!("Could not open llama-server process {pid} to cancel it: {}", std::io::Error::last_os_error());
            return;
        }
        if TerminateProcess(handle, 1) == 0 {
            log::warn!("Could not stop llama-server process {pid} to cancel it: {}", std::io::Error::last_os_error());
        }
        CloseHandle(handle);
    }
}
