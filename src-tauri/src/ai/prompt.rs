//! Builds the grounded prompt for the command generator and turns the
//! model's reply into a reviewed, risk-assessed command.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::assist::errors::convert_chain_operators;
use crate::assist::risk::{self, RiskReport};
use crate::index::inventory::Inventory;
use crate::shell::quote::to_single_line;

/// Everything the model is told about the user's environment.
#[derive(Debug, Clone, Default)]
pub struct PromptContext {
    pub cwd: Option<String>,
    pub folder_entries: Vec<String>,
    pub os_version: Option<String>,
    pub ps_version: Option<String>,
    pub installed_tools: Vec<String>,
    /// (title, command template) pairs from the catalog relevant to the request.
    pub recipes: Vec<(String, String)>,
}

impl PromptContext {
    pub fn is_windows_powershell(&self) -> bool {
        self.ps_version.as_deref().is_none_or(|v| v.starts_with("5."))
    }
}

/// Programs worth telling the model about when installed; listing all of
/// System32 would only distract a small model.
pub const NOTABLE_TOOLS: &[&str] = &[
    "git",
    "node",
    "npm",
    "npx",
    "yarn",
    "pnpm",
    "python",
    "pip",
    "py",
    "uv",
    "docker",
    "winget",
    "choco",
    "scoop",
    "code",
    "cargo",
    "rustc",
    "go",
    "java",
    "javac",
    "mvn",
    "gradle",
    "dotnet",
    "gh",
    "az",
    "aws",
    "kubectl",
    "terraform",
    "ffmpeg",
    "7z",
    "tar",
    "curl",
    "ssh",
    "scp",
    "rg",
    "jq",
    "deno",
    "bun",
    "make",
    "cmake",
];

const EXAMPLES: &[(&str, &str, &str)] = &[
    (
        "list all files bigger than 100MB in this folder and subfolders",
        "Get-ChildItem -Recurse -File | Where-Object Length -gt 100MB | Sort-Object Length -Descending | Select-Object FullName, @{Name='SizeMB'; Expression={[math]::Round($_.Length / 1MB, 1)}}",
        "Searches this folder and its subfolders for files over 100 MB and lists them largest first, with their size in MB.",
    ),
    (
        "rename every .jpeg file here to .jpg",
        "Get-ChildItem -Filter *.jpeg | Rename-Item -NewName { $_.BaseName + '.jpg' }",
        "Finds each .jpeg file in this folder and renames it to end in .jpg instead.",
    ),
];

pub fn system_prompt(context: &PromptContext) -> String {
    let shell = if context.is_windows_powershell() {
        format!("Windows PowerShell {}", context.ps_version.as_deref().unwrap_or("5.1"))
    } else {
        format!("PowerShell {}", context.ps_version.as_deref().unwrap_or("7"))
    };
    let mut prompt = format!(
        "You turn requests into one {shell} command for a beginner on Windows.\n\
         Rules:\n\
         - Reply with JSON only: {{\"command\": \"...\", \"explanation\": \"...\"}}.\n\
         - The command must work in {shell} on Windows. Never use bash, Linux or macOS syntax (no sudo, grep, ls -la, rm -rf, export, cat <<EOF).\n\
         - Prefer built-in PowerShell cmdlets. Use other programs only if they are in the installed list below.\n\
         - Write a single line; separate steps with \";\".{}\n\
         - Use paths relative to the current folder when you can.\n\
         - Never delete, overwrite, or change system settings unless the user clearly asked for it.\n\
         - If the request is unclear, unsafe or impossible, set \"command\" to \"\" and say why in \"explanation\".\n\
         - \"explanation\" is one or two short sentences a beginner understands.\n",
        if context.is_windows_powershell() { " Do not use && or ||." } else { "" }
    );
    prompt.push_str("\nContext:\n");
    if let Some(os) = &context.os_version {
        prompt.push_str(&format!("- Operating system: {os}\n"));
    }
    if let Some(cwd) = &context.cwd {
        prompt.push_str(&format!("- Current folder: {cwd}\n"));
    }
    if !context.folder_entries.is_empty() {
        prompt.push_str(&format!("- Items in the current folder: {}\n", context.folder_entries.join(", ")));
    }
    if !context.installed_tools.is_empty() {
        prompt.push_str(&format!("- Installed programs: {}\n", context.installed_tools.join(", ")));
    }
    if !context.recipes.is_empty() {
        prompt.push_str("- Commands known to work here (fill in the <placeholders>):\n");
        for (title, command) in &context.recipes {
            prompt.push_str(&format!("  - {title}: {command}\n"));
        }
    }
    prompt
}

/// The chat messages sent to llama-server, including worked examples that
/// keep a small model on the expected format.
pub fn messages(context: &PromptContext, request: &str) -> serde_json::Value {
    let mut messages = vec![json!({"role": "system", "content": system_prompt(context)})];
    for (user, command, explanation) in EXAMPLES {
        messages.push(json!({"role": "user", "content": user}));
        messages.push(json!({"role": "assistant", "content": json!({"command": command, "explanation": explanation}).to_string()}));
    }
    messages.push(json!({"role": "user", "content": request}));
    serde_json::Value::Array(messages)
}

/// JSON schema the server uses to constrain decoding, so the reply always parses.
pub fn reply_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string"},
            "explanation": {"type": "string"}
        },
        "required": ["command", "explanation"]
    })
}

#[derive(Debug, Deserialize)]
struct ModelReply {
    command: String,
    explanation: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedCommand {
    /// Empty when the model declined the request.
    pub command: String,
    pub explanation: String,
    pub risk: RiskReport,
    /// Problems found while checking the command, e.g. unknown programs.
    pub warnings: Vec<String>,
}

static CODE_FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)```(?:\w+)?\s*\n(.*?)```").expect("valid regex"));
static JSON_OBJECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)\{.*\}").expect("valid regex"));

const KEYWORDS: &[&str] = &[
    "if", "elseif", "else", "foreach", "for", "while", "do", "until", "switch", "try", "catch", "finally", "function", "filter", "param",
    "return", "begin", "process", "end", "break", "continue", "throw", "exit", "trap", "in",
];

fn parse_reply(raw: &str) -> Option<ModelReply> {
    let raw = raw.trim();
    if let Ok(reply) = serde_json::from_str::<ModelReply>(raw) {
        return Some(reply);
    }
    if let Some(reply) = JSON_OBJECT.find(raw).and_then(|m| serde_json::from_str::<ModelReply>(m.as_str()).ok()) {
        return Some(reply);
    }
    // Some models ignore the format and answer with a code block.
    CODE_FENCE.captures(raw).map(|c| ModelReply { command: c[1].trim().to_string(), explanation: String::new() })
}

/// The command name a statement or pipeline segment starts with, if any.
fn command_word(segment: &str) -> Option<String> {
    let trimmed = segment.trim_start();
    // ". script.ps1" dot-sources a script; ".AddDays(" is a method call.
    let trimmed = match trimmed.strip_prefix('.') {
        Some(rest) if rest.starts_with(char::is_whitespace) => rest.trim_start(),
        Some(_) => return None,
        None => trimmed,
    };
    let trimmed = trimmed.strip_prefix('&').map_or(trimmed, str::trim_start);
    let word = trimmed.split_whitespace().next()?;
    let is_command = !word.starts_with(['$', '[', '@', '\'', '"', '-', '%', '?', '=', '+', '*', '/', '\\', ','])
        && !word.contains('=')
        && !word.chars().next().is_some_and(|c| c.is_ascii_digit())
        && !KEYWORDS.contains(&word.to_lowercase().as_str());
    is_command.then(|| word.to_string())
}

/// The names a command line invokes: the first word of every statement,
/// pipeline stage and script block, ignoring quoted text, method calls and
/// the keys inside `@{ ... }` hashtable literals.
fn command_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    // One entry per open brace: true for a hashtable literal, false for a script block.
    let mut braces: Vec<bool> = Vec::new();
    let mut previous: Option<char> = None;
    let mut flush = |current: &mut String, braces: &[bool]| {
        if !braces.contains(&true) {
            words.extend(command_word(current));
        }
        current.clear();
    };
    for ch in command.chars() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            current.push(ch);
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                current.push(ch);
            }
            '{' => {
                flush(&mut current, &braces);
                braces.push(previous == Some('@'));
            }
            '}' => {
                flush(&mut current, &braces);
                braces.pop();
            }
            '|' | ';' | '(' | ')' => flush(&mut current, &braces),
            _ => current.push(ch),
        }
        if !ch.is_whitespace() {
            previous = Some(ch);
        }
    }
    flush(&mut current, &braces);
    words
}

/// Validates and post-processes the model's raw reply.
pub fn finish(raw: &str, context: &PromptContext, inventory: Option<&Inventory>) -> Option<GeneratedCommand> {
    let reply = parse_reply(raw)?;
    let mut warnings = Vec::new();
    let mut command =
        to_single_line(reply.command.trim().trim_start_matches("PS> ").trim_start_matches("> ").trim_start_matches("$ ").trim());
    if context.is_windows_powershell() && command.contains("&&") {
        match convert_chain_operators(&command) {
            Some(converted) => {
                command = converted;
                warnings.push(
                    "Rewrote && (not supported by Windows PowerShell 5.1) so each step runs only if the previous one succeeded.".into(),
                );
            }
            None => warnings.push("This uses && or ||, which Windows PowerShell 5.1 doesn't support.".into()),
        }
    }
    // Before PowerShell's command list is known, real cmdlets would be flagged
    // as made up, so say the check couldn't run instead.
    match inventory.filter(|inventory| !inventory.ps_commands.is_empty()) {
        Some(inventory) => {
            for word in command_words(&command) {
                if word.contains(['\\', '/', ':']) || inventory.has(&word) {
                    continue;
                }
                warnings.push(format!("\"{word}\" isn't an installed program or known PowerShell command. The model may have made it up."));
            }
        }
        None => {
            warnings.push("VCE couldn't yet check that the commands used here exist on this computer, so review them carefully.".into())
        }
    }
    Some(GeneratedCommand { risk: risk::assess(&command), command, explanation: reply.explanation.trim().to_string(), warnings })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;
    use crate::index::inventory::{PsCommand, PsCommandKind};

    fn inventory() -> Inventory {
        let cmdlets = ["Get-ChildItem", "Where-Object", "Sort-Object", "Remove-Item", "Rename-Item", "ForEach-Object", "Get-Process"];
        Inventory::new(
            HashMap::from([("git".to_string(), PathBuf::from(r"C:\git.exe"))]),
            cmdlets.iter().map(|n| PsCommand { name: (*n).into(), kind: PsCommandKind::Cmdlet, definition: None, usage: None }).collect(),
        )
    }

    fn ps51() -> PromptContext {
        PromptContext { ps_version: Some("5.1.26100.1".into()), ..Default::default() }
    }

    #[test]
    fn parses_json_reply_and_assesses_risk() {
        let raw = r#"{"command": "Get-ChildItem *.log | Remove-Item", "explanation": "Deletes log files."}"#;
        let generated = finish(raw, &ps51(), Some(&inventory())).unwrap();
        assert_eq!(generated.command, "Get-ChildItem *.log | Remove-Item");
        assert_eq!(generated.risk.level, risk::RiskLevel::Danger);
        assert!(generated.warnings.is_empty());
    }

    #[test]
    fn extracts_json_from_chatty_reply_and_code_fences() {
        let chatty = "Sure! {\"command\": \"git status\", \"explanation\": \"Shows changes.\"} Hope that helps.";
        assert_eq!(finish(chatty, &ps51(), None).unwrap().command, "git status");
        let fenced = "```powershell\nGet-Process\n```";
        assert_eq!(finish(fenced, &ps51(), None).unwrap().command, "Get-Process");
        assert!(finish("I can't help with that", &ps51(), None).is_none());
    }

    #[test]
    fn flags_made_up_commands() {
        let raw = r#"{"command": "Get-ChildItem | Invoke-MagicCleanup", "explanation": "x"}"#;
        let generated = finish(raw, &ps51(), Some(&inventory())).unwrap();
        assert_eq!(generated.warnings.len(), 1);
        assert!(generated.warnings[0].contains("Invoke-MagicCleanup"));
    }

    #[test]
    fn ignores_variables_keywords_and_script_blocks_when_checking_names() {
        let words = command_words("Get-ChildItem | ForEach-Object { if ($_.Length -gt 1) { $_.Name } } ; 'a|b'");
        assert_eq!(words, vec!["Get-ChildItem", "ForEach-Object"]);
    }

    #[test]
    fn ignores_method_calls_and_hashtable_keys() {
        let command = r"Get-ChildItem -Recurse -File | Where-Object { $_.LastWriteTime -gt (Get-Date).AddDays(-7) } | Select-Object FullName, @{Name='SizeMB'; Expression={[math]::Round($_.Length / 1MB, 1)}}; . .\setup.ps1";
        assert_eq!(command_words(command), vec!["Get-ChildItem", "Where-Object", "Get-Date", "Select-Object", r".\setup.ps1"]);
    }

    #[test]
    fn rewrites_chain_operators_for_windows_powershell() {
        let raw = r#"{"command": "git add . && git commit", "explanation": "x"}"#;
        let generated = finish(raw, &ps51(), None).unwrap();
        assert_eq!(generated.command, "git add .; if ($?) { git commit }");
        assert!(generated.warnings[0].starts_with("Rewrote &&"));

        let quoted = r#"{"command": "git add . && git commit -m 'x'", "explanation": "x"}"#;
        let generated = finish(quoted, &ps51(), None).unwrap();
        assert_eq!(generated.command, "git add . && git commit -m 'x'");
        assert!(generated.warnings[0].contains("doesn't support"));
    }

    #[test]
    fn system_prompt_includes_grounding_context() {
        let context = PromptContext {
            cwd: Some(r"C:\proj".into()),
            folder_entries: vec!["src\\".into(), "package.json".into()],
            installed_tools: vec!["git".into(), "node".into()],
            recipes: vec![("Extract a .zip archive".into(), "Expand-Archive -Path <archive>".into())],
            ..ps51()
        };
        let prompt = system_prompt(&context);
        assert!(prompt.contains(r"Current folder: C:\proj"));
        assert!(prompt.contains("Installed programs: git, node"));
        assert!(prompt.contains("Expand-Archive -Path <archive>"));
        assert!(prompt.contains("Do not use && or ||"));
        assert_eq!(messages(&context, "hi").as_array().unwrap().len(), 2 + EXAMPLES.len() * 2);
    }
}
