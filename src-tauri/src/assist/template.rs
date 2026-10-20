//! Fills `<placeholder>` slots in catalog command templates with values the
//! user typed, quoting each value so it is passed as one literal argument.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde::Serialize;

use super::risk::{self, RiskReport};
use crate::shell::quote::argument;

static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<([a-z0-9-]+)>").expect("valid regex"));

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilledCommand {
    pub command: String,
    /// Placeholders still without a value, in order of appearance.
    pub missing: Vec<String>,
    pub risk: RiskReport,
}

pub fn fill(template: &str, values: &HashMap<String, String>) -> FilledCommand {
    let mut missing = Vec::new();
    let command = PLACEHOLDER
        .replace_all(template, |caps: &Captures| {
            let name = &caps[1];
            match values.get(name).map(|v| v.trim()).filter(|v| !v.is_empty()) {
                Some(value) => argument(value),
                None => {
                    if !missing.iter().any(|m| m == name) {
                        missing.push(name.to_string());
                    }
                    caps[0].to_string()
                }
            }
        })
        .into_owned();
    let risk = risk::assess(&command);
    FilledCommand { command, missing, risk }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_and_quotes_values() {
        let values = HashMap::from([("archive".to_string(), "My Photos.zip".to_string()), ("folder".to_string(), "photos".to_string())]);
        let filled = fill("Expand-Archive -Path <archive> -DestinationPath <folder>", &values);
        assert_eq!(filled.command, "Expand-Archive -Path 'My Photos.zip' -DestinationPath photos");
        assert!(filled.missing.is_empty());
    }

    #[test]
    fn reports_missing_values_once_and_keeps_placeholders() {
        let filled = fill("Copy-Item <src> <dest>; Get-Item <src>", &HashMap::new());
        assert_eq!(filled.missing, vec!["src", "dest"]);
        assert!(filled.command.contains("<src>"));
    }

    #[test]
    fn injection_attempts_stay_literal() {
        let values = HashMap::from([("name".to_string(), "x; Remove-Item -Recurse C:\\".to_string())]);
        let filled = fill("Get-Process -Name <name>", &values);
        assert_eq!(filled.command, "Get-Process -Name 'x; Remove-Item -Recurse C:\\'");
    }
}
