//! Messages sent by `integration.ps1` over the named pipe, one JSON object per line.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ShellMessage {
    Hello(HelloInfo),
    Prompt(PromptInfo),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HelloInfo {
    pub ps_version: String,
    #[serde(default)]
    pub edition: String,
    #[serde(default)]
    pub os_version: String,
    #[serde(default)]
    pub ps_read_line: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PromptInfo {
    /// Provider-native path of the current location (a filesystem path when
    /// `provider` is "FileSystem").
    pub cwd: Option<String>,
    pub provider: Option<String>,
    /// Whether a command ran since the previous prompt (false for a bare Enter).
    pub ran: bool,
    pub command_line: Option<String>,
    pub success: bool,
    /// `$LASTEXITCODE`; only meaningful when the last command was a native program.
    pub exit_code: Option<i64>,
    pub error: Option<PsError>,
    pub edit_mode: Option<String>,
}

impl PromptInfo {
    pub fn is_filesystem(&self) -> bool {
        self.provider.as_deref().is_some_and(|p| p.eq_ignore_ascii_case("FileSystem"))
    }
}

/// The PowerShell `ErrorRecord` produced by the last failed command.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PsError {
    pub message: String,
    #[serde(default)]
    pub error_id: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub exception_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prompt_message_from_convert_to_json() {
        let line = r#"{"error":{"category":"ObjectNotFound","target":"gti","message":"The term 'gti' is not recognized","exceptionType":"System.Management.Automation.CommandNotFoundException","errorId":"CommandNotFoundException"},"exitCode":null,"success":false,"commandLine":"gti status","cwd":"C:\\Users\\me","editMode":"Windows","provider":"FileSystem","type":"prompt","ran":true}"#;
        let ShellMessage::Prompt(prompt) = serde_json::from_str(line).unwrap() else {
            panic!("expected a prompt message");
        };
        assert_eq!(prompt.cwd.as_deref(), Some(r"C:\Users\me"));
        assert!(prompt.is_filesystem());
        assert_eq!(prompt.error.unwrap().target, "gti");
        assert_eq!(prompt.exit_code, None);
    }

    #[test]
    fn parses_hello_message() {
        let line = r#"{"type":"hello","psVersion":"5.1.26100.1","edition":"Desktop","osVersion":"Microsoft Windows NT 10.0.26300.0","psReadLine":true}"#;
        let ShellMessage::Hello(hello) = serde_json::from_str(line).unwrap() else {
            panic!("expected a hello message");
        };
        assert!(hello.ps_read_line);
        assert_eq!(hello.edition, "Desktop");
    }
}
