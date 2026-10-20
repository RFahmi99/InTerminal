//! Tracks whether the shell is sitting at an empty prompt, so GUI actions that
//! type into the terminal never interleave with a running command or clobber a
//! half-typed command line.

use std::sync::LazyLock;

use regex::Regex;

/// Replies the terminal emulator sends on its own, not keystrokes: focus
/// in/out (ConPTY turns on focus reporting), cursor position reports and
/// device attribute responses. They never change the command line.
static TERMINAL_REPORTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\x1b\[(?:I|O|\d+;\d+R|[?>][\d;]*c|\d+(?:;\d+)*t))+$").expect("valid regex"));

/// What the user has done since the last prompt was drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activity {
    /// A command is executing (or PowerShell is reading continuation lines).
    pub busy: bool,
    /// The prompt is showing but the command line may contain unsent text.
    pub line_dirty: bool,
}

#[derive(Debug)]
pub struct InputTracker {
    at_prompt: bool,
    line_dirty: bool,
    escape_clears_line: bool,
    /// Keys typed while a command ran and not submitted to it with Enter.
    /// PSReadLine shows them on the next prompt, so that line starts dirty.
    typed_ahead: bool,
}

impl Default for InputTracker {
    fn default() -> Self {
        // The shell is still starting until its first prompt is reported.
        Self { at_prompt: false, line_dirty: false, escape_clears_line: true, typed_ahead: false }
    }
}

impl InputTracker {
    pub fn activity(&self) -> Activity {
        Activity { busy: !self.at_prompt, line_dirty: self.at_prompt && self.line_dirty }
    }

    /// A fresh, empty prompt was drawn. `edit_mode` is PSReadLine's edit mode,
    /// or `None` when PSReadLine isn't loaded.
    pub fn on_prompt(&mut self, edit_mode: Option<&str>) {
        self.at_prompt = true;
        self.line_dirty = std::mem::take(&mut self.typed_ahead);
        // In PSReadLine's default Windows mode Esc reverts the whole line; in
        // Emacs/Vi modes, and in the plain host line editor, it doesn't.
        self.escape_clears_line = edit_mode.is_some_and(|mode| mode.eq_ignore_ascii_case("windows"));
    }

    /// Bytes the user (or the app) sent to the shell.
    pub fn on_input(&mut self, data: &str) {
        if data.is_empty() || TERMINAL_REPORTS.is_match(data) {
            return;
        }
        if !self.at_prompt {
            // Enter or Ctrl+C hands the typed keys to the running program;
            // anything else is waiting for the next prompt.
            self.typed_ahead = !data.ends_with(['\r', '\x03']);
            return;
        }
        if data.contains(['\r', '\x03']) {
            // Enter submits the line and Ctrl+C cancels it; either way the next
            // prompt event marks the shell idle again.
            self.at_prompt = false;
            self.line_dirty = false;
        } else {
            let clears_line = data == "\x1b" && self.escape_clears_line;
            self.line_dirty = !clears_line;
        }
    }

    /// Without shell integration there are no prompt events, so the tracker
    /// can't know the shell state and stops blocking GUI actions.
    pub fn assume_idle(&mut self) {
        self.at_prompt = true;
        self.line_dirty = false;
        self.typed_ahead = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_busy_until_first_prompt() {
        let tracker = InputTracker::default();
        assert!(tracker.activity().busy);
    }

    #[test]
    fn typing_marks_line_dirty_and_enter_marks_busy() {
        let mut tracker = InputTracker::default();
        tracker.on_prompt(Some("Windows"));
        assert_eq!(tracker.activity(), Activity { busy: false, line_dirty: false });
        tracker.on_input("git st");
        assert_eq!(tracker.activity(), Activity { busy: false, line_dirty: true });
        tracker.on_input("\r");
        assert_eq!(tracker.activity(), Activity { busy: true, line_dirty: false });
        tracker.on_prompt(Some("Windows"));
        assert_eq!(tracker.activity(), Activity { busy: false, line_dirty: false });
    }

    #[test]
    fn escape_clears_only_in_windows_edit_mode() {
        let mut tracker = InputTracker::default();
        tracker.on_prompt(Some("Windows"));
        tracker.on_input("abc");
        tracker.on_input("\x1b");
        assert!(!tracker.activity().line_dirty);

        tracker.on_prompt(Some("Emacs"));
        tracker.on_input("abc");
        tracker.on_input("\x1b");
        assert!(tracker.activity().line_dirty);
    }

    #[test]
    fn terminal_reports_are_not_typing() {
        let mut tracker = InputTracker::default();
        tracker.on_prompt(Some("Windows"));
        for report in ["\x1b[O", "\x1b[I", "\x1b[12;40R", "\x1b[?1;2c", "\x1b[>0;10;1c", "\x1b[O\x1b[I"] {
            tracker.on_input(report);
            assert!(!tracker.activity().line_dirty, "{report:?} marked the line dirty");
        }
        // Arrow keys can recall history onto the line, so they still count.
        tracker.on_input("\x1b[A");
        assert!(tracker.activity().line_dirty);
    }

    #[test]
    fn ctrl_c_waits_for_the_next_prompt() {
        let mut tracker = InputTracker::default();
        tracker.on_prompt(None);
        tracker.on_input("abc");
        tracker.on_input("\x03");
        assert!(tracker.activity().busy);
    }

    #[test]
    fn keys_typed_during_a_command_make_the_next_line_dirty() {
        let mut tracker = InputTracker::default();
        tracker.on_prompt(Some("Windows"));
        tracker.on_input("npm install\r");
        tracker.on_input("git st");
        tracker.on_prompt(Some("Windows"));
        assert!(tracker.activity().line_dirty);
    }

    #[test]
    fn answers_submitted_to_a_running_program_are_not_type_ahead() {
        let mut tracker = InputTracker::default();
        tracker.on_prompt(Some("Windows"));
        tracker.on_input("Remove-Item x -Confirm\r");
        tracker.on_input("y");
        tracker.on_input("\r");
        tracker.on_prompt(Some("Windows"));
        assert!(!tracker.activity().line_dirty);
    }

    #[test]
    fn input_while_busy_is_ignored() {
        let mut tracker = InputTracker::default();
        tracker.on_input("y");
        assert!(tracker.activity().busy);
        assert!(!tracker.activity().line_dirty);
    }
}
