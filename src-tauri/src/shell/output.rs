//! Delivers raw terminal output to the frontend and keeps a bounded backlog,
//! so a reloaded webview can repaint the session instead of showing a blank
//! terminal.

use std::collections::VecDeque;
use std::sync::Mutex;

use tauri::ipc::{Channel, Response};

const BACKLOG_LIMIT: usize = 256 * 1024;

#[derive(Default)]
pub struct OutputSink {
    inner: Mutex<SinkState>,
}

#[derive(Default)]
struct SinkState {
    channel: Option<Channel<Response>>,
    backlog: VecDeque<u8>,
    /// Only this session's output is shown. A replaced session can keep
    /// printing until its pseudo-console closes (e.g. `ping -t`), and its
    /// leftover bytes, including mode switches, must not reach the new one.
    session_id: u64,
}

impl OutputSink {
    /// Routes output to `channel`, first replaying the backlog into it.
    pub fn attach(&self, channel: Channel<Response>) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !state.backlog.is_empty() {
            let backlog: Vec<u8> = state.backlog.iter().copied().collect();
            if let Err(e) = channel.send(Response::new(backlog)) {
                log::warn!("Could not replay terminal backlog to the frontend: {e}");
            }
        }
        state.channel = Some(channel);
    }

    /// Switches to a new session and forgets the previous one's output.
    pub fn start_session(&self, session_id: u64) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.session_id = session_id;
        state.backlog.clear();
    }

    pub fn push(&self, session_id: u64, bytes: &[u8]) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if session_id != state.session_id {
            return;
        }
        state.backlog.extend(bytes);
        let excess = state.backlog.len().saturating_sub(BACKLOG_LIMIT);
        state.backlog.drain(..excess);
        if let Some(channel) = &state.channel {
            if let Err(e) = channel.send(Response::new(bytes.to_vec())) {
                // The webview went away (reload or close); output keeps
                // accumulating in the backlog until it re-attaches.
                log::debug!("Dropping terminal output channel: {e}");
                state.channel = None;
            }
        }
    }
}
