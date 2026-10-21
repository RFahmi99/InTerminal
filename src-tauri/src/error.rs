//! The error type returned by every Tauri command. The `code` lets the
//! frontend react programmatically; `message` is written for the end user and
//! is announced to screen readers as-is.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    ShellNotRunning,
    ShellBusy,
    LineDirty,
    InvalidInput,
    NotFound,
    Io,
    IndexUnavailable,
    AiUnavailable,
    AiFailed,
    Cancelled,
    UpdateFailed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    /// Wraps an I/O failure, naming the operation and the path it was applied to.
    pub fn io(action: &str, path: &std::path::Path, err: &std::io::Error) -> Self {
        let code = if err.kind() == std::io::ErrorKind::NotFound { ErrorCode::NotFound } else { ErrorCode::Io };
        Self::new(code, format!("Could not {action} \"{}\": {err}", path.display()))
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

pub type CmdResult<T> = Result<T, AppError>;
