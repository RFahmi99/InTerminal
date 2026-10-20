//! Actionable error interception: turns a failed command (PowerShell's error
//! record plus the text it printed) into concrete fixes the user can review
//! and run. Suggestions are never executed automatically.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::risk::{self, RiskReport};
use crate::index::fuzzy;
use crate::index::search::CommandIndex;
use crate::shell::PsError;
use crate::shell::quote::{argument, single_quote};

const MAX_SUGGESTIONS: usize = 3;

/// What the frontend observed about a command that failed.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureReport {
    pub command_line: String,
    pub cwd: Option<String>,
    pub error: Option<PsError>,
    /// The terminal text the command printed (ANSI codes already removed).
    #[serde(default)]
    pub output: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    /// Which rule produced this, for tests and diagnostics.
    pub rule: &'static str,
    pub title: String,
    pub explanation: String,
    /// The fix to run, if there is one; otherwise the suggestion is advice.
    pub command: Option<String>,
    pub risk: Option<RiskReport>,
}

impl Suggestion {
    fn fix(rule: &'static str, title: impl Into<String>, explanation: impl Into<String>, command: impl Into<String>) -> Self {
        let command = command.into();
        Self { rule, title: title.into(), explanation: explanation.into(), risk: Some(risk::assess(&command)), command: Some(command) }
    }

    fn advice(rule: &'static str, title: impl Into<String>, explanation: impl Into<String>) -> Self {
        Self { rule, title: title.into(), explanation: explanation.into(), command: None, risk: None }
    }
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|e| panic!("invalid error-rule pattern {pattern:?}: {e}"))
}

static TERM_NOT_RECOGNIZED: LazyLock<Regex> = LazyLock::new(|| re(r"The term '([^']+)' is not recognized"));
static CMD_NOT_RECOGNIZED: LazyLock<Regex> =
    LazyLock::new(|| re(r"'([^']+)' is not recognized as an internal or external command"));
static NODE_MISSING_MODULE: LazyLock<Regex> = LazyLock::new(|| re(r"Cannot find module '([^']+)'"));
static NPM_MISSING_SCRIPT: LazyLock<Regex> = LazyLock::new(|| re(r#"(?i)missing script:?\s*"?([^"\s]+)"?"#));
static PORT_IN_USE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)EADDRINUSE[^\n]*?:(\d{2,5})|address already in use[^\n]*?:(\d{2,5})|port (\d{2,5}) is already in use"));
static PY_MISSING_MODULE: LazyLock<Regex> = LazyLock::new(|| re(r"ModuleNotFoundError: No module named '([^']+)'"));
// Values lifted from command output are untrusted text that ends up in a
// command line, so captures only accept the characters remote, branch and
// subcommand names legitimately use.
static GIT_SET_UPSTREAM: LazyLock<Regex> =
    LazyLock::new(|| re(r"(git push --set-upstream [A-Za-z0-9_][A-Za-z0-9._/-]* [A-Za-z0-9_][A-Za-z0-9._/-]*)(?:\s|$)"));
static GIT_NOT_A_COMMAND: LazyLock<Regex> = LazyLock::new(|| re(r"git: '([^']+)' is not a git command"));
static GIT_SIMILAR: LazyLock<Regex> = LazyLock::new(|| re(r"most similar commands? (?:is|are)\s+([a-z][a-z-]*)\b"));
/// npm package names: optional `@scope/`, then URL-safe lower-case characters.
static NPM_PACKAGE_NAME: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:@[a-z0-9][a-z0-9._~-]*/)?[a-z0-9][a-z0-9._~-]*$"));
/// PyPI project names (PEP 508).
static PIP_PACKAGE_NAME: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z0-9][A-Za-z0-9._-]*$"));
static PATH_NOT_FOUND: LazyLock<Regex> = LazyLock::new(|| re(r"Cannot find path '([^']+)' because it does not exist"));
static BAD_PARAMETER: LazyLock<Regex> = LazyLock::new(|| re(r"A parameter cannot be found that matches parameter name '([^']+)'"));
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| re(r"<[a-z0-9-]+>"));

/// Splits a command line into its first word and the rest, ignoring a leading
/// call operator and quotes.
fn split_first(command_line: &str) -> (String, String) {
    let trimmed = command_line.trim().trim_start_matches('&').trim_start();
    let (first, rest) = if let Some(quoted) = trimmed.strip_prefix(['"', '\'']) {
        let quote = trimmed.chars().next().unwrap_or('"');
        match quoted.find(quote) {
            Some(end) => (&quoted[..end], &quoted[end + 1..]),
            None => (quoted, ""),
        }
    } else {
        match trimmed.find(char::is_whitespace) {
            Some(end) => (&trimmed[..end], &trimmed[end..]),
            None => (trimmed, ""),
        }
    };
    (first.to_string(), rest.trim().to_string())
}

/// The first word of `command_line` replaced by `replacement`.
fn with_first_word(replacement: &str, rest: &str) -> String {
    if rest.is_empty() { replacement.to_string() } else { format!("{replacement} {rest}") }
}

/// Windows equivalents for commands people bring from Linux and macOS.
fn linux_equivalent(name: &str, rest: &str) -> Option<(String, &'static str)> {
    let args: Vec<&str> = rest.split_whitespace().collect();
    let arg = |i: usize, default: &str| args.get(i).map_or_else(|| default.to_string(), |a| (*a).to_string());
    let suggestion = match name.to_lowercase().as_str() {
        "touch" => (format!("New-Item -ItemType File -Path {}", arg(0, "<file>")), "PowerShell creates empty files with New-Item."),
        "grep" => {
            let pattern = args.first().map_or("<text>".to_string(), |p| argument(p.trim_matches(['"', '\''])));
            let files = if args.len() > 1 { args[1..].join(" ") } else { "*".into() };
            (format!("Select-String -Pattern {pattern} -Path {files}"), "PowerShell searches text with Select-String.")
        }
        "which" => (format!("Get-Command {}", arg(0, "<name>")), "Get-Command shows where a command comes from."),
        "export" => {
            let assignment = arg(0, "NAME=value");
            let (key, value) = assignment.split_once('=').unwrap_or((assignment.as_str(), ""));
            (format!("$env:{key} = {}", single_quote(value.trim_matches(['"', '\'']))), "PowerShell sets environment variables through $env:.")
        }
        "sudo" => (
            "Start-Process powershell -Verb RunAs".into(),
            "Windows has no sudo by default. This opens a new PowerShell window as administrator, where you can run the command. (Windows 11 24H2 and later can turn on sudo under Settings > System > For developers.)",
        ),
        "ifconfig" => ("ipconfig /all".into(), "Windows shows network settings with ipconfig."),
        "head" => (format!("Get-Content {} -TotalCount 10", arg(0, "<file>")), "Get-Content -TotalCount shows the first lines of a file."),
        "tail" => (format!("Get-Content {} -Tail 10", arg(0, "<file>")), "Get-Content -Tail shows the last lines; add -Wait to follow the file."),
        "man" => (format!("Get-Help {} -Full", arg(0, "<command>")), "PowerShell's manual pages come from Get-Help."),
        "nano" | "vim" | "vi" => (format!("notepad {}", arg(0, "<file>")), "Notepad is always available for editing text files."),
        "open" | "xdg-open" => (format!("Invoke-Item {}", arg(0, ".")), "Invoke-Item opens a file or folder with its default app."),
        "unzip" => (format!("Expand-Archive -Path {}", arg(0, "<archive>")), "PowerShell extracts .zip files with Expand-Archive."),
        "df" => ("Get-PSDrive -PSProvider FileSystem".into(), "Get-PSDrive lists drives with their used and free space."),
        "du" => (
            "Get-ChildItem -Recurse -File | Measure-Object -Property Length -Sum".into(),
            "This adds up the size of every file under the current folder.",
        ),
        "top" | "htop" => (
            "Get-Process | Sort-Object CPU -Descending | Select-Object -First 15".into(),
            "This lists the processes using the most CPU time.",
        ),
        "pkill" | "killall" => (format!("Stop-Process -Name {}", arg(0, "<name>")), "Stop-Process stops programs by name."),
        "ll" => ("Get-ChildItem -Force".into(), "Get-ChildItem -Force lists everything, including hidden files."),
        "source" => (format!(". {}", arg(0, "<script>")), "PowerShell runs a script in the current session with a dot and a space."),
        "pbcopy" => ("Set-Clipboard".into(), "Pipe text into Set-Clipboard to copy it."),
        "pbpaste" => ("Get-Clipboard".into(), "Get-Clipboard prints what's on the clipboard."),
        "nproc" => ("$env:NUMBER_OF_PROCESSORS".into(), "Windows stores the processor count in an environment variable."),
        "uname" => ("[System.Environment]::OSVersion".into(), "This shows the Windows version."),
        _ => return None,
    };
    Some(suggestion)
}

/// winget package IDs for interpreters whose command name differs from the tool entry.
fn tool_alias(name: &str) -> &str {
    match name.to_lowercase().as_str() {
        "python3" | "py" | "pip" | "pip3" => "python",
        "npx" | "npm" => "node",
        _ => name,
    }
}

fn command_not_found(report: &FailureReport, index: Option<&CommandIndex>, text: &str) -> Vec<Suggestion> {
    let error = report.error.as_ref();
    let is_not_found = error.is_some_and(|e| e.error_id.contains("CommandNotFoundException") || e.exception_type.ends_with("CommandNotFoundException"))
        || CMD_NOT_RECOGNIZED.is_match(text)
        || TERM_NOT_RECOGNIZED.is_match(text);
    if !is_not_found {
        return Vec::new();
    }
    let (first, rest) = split_first(&report.command_line);
    let name = error
        .map(|e| e.target.clone())
        .filter(|t| !t.is_empty())
        .or_else(|| TERM_NOT_RECOGNIZED.captures(text).map(|c| c[1].to_string()))
        .or_else(|| CMD_NOT_RECOGNIZED.captures(text).map(|c| c[1].to_string()))
        .unwrap_or(first.clone());
    let mut suggestions = Vec::new();

    // PowerShell won't run programs from the current folder without .\
    // The name is checked before touching the disk: joined onto the folder, a
    // name like `\\host\share\x` would replace it and make Windows contact
    // that host.
    let plain_name = !name.is_empty() && !name.contains(['\\', '/', ':']) && name != "..";
    if let (true, Some(cwd)) = (plain_name, report.cwd.as_deref()) {
        let cwd = Path::new(cwd);
        let runnable = ["", ".exe", ".ps1", ".bat", ".cmd"].iter().any(|ext| cwd.join(format!("{name}{ext}")).is_file());
        if runnable {
            let local = format!(".\\{name}");
            let invocation = if argument(&local) == local { local } else { format!("& {}", single_quote(&local)) };
            suggestions.push(Suggestion::fix(
                "run-from-current-folder",
                format!("Run {name} from this folder"),
                "PowerShell only runs programs from the current folder when you put .\\ in front of the name. This protects you from running a file you didn't mean to.",
                with_first_word(&invocation, &rest),
            ));
        }
    }

    if let Some((command, why)) = linux_equivalent(&name, &rest) {
        suggestions.push(Suggestion::fix("linux-equivalent", format!("{name} doesn't exist on Windows"), why, command));
    }

    if let Some(index) = index {
        let catalog = index.catalog();
        if let Some(tool) = catalog.tool(tool_alias(&name)) {
            if name.eq_ignore_ascii_case("pip") || name.eq_ignore_ascii_case("pip3") {
                if index.inventory().has("python") {
                    suggestions.push(Suggestion::fix(
                        "pip-through-python",
                        "Run pip through Python",
                        "pip isn't on your PATH, but Python is. Running it as a Python module works the same way.",
                        with_first_word("python -m pip", &rest),
                    ));
                }
            } else if !index.inventory().has(&tool.name) {
                suggestions.push(Suggestion::fix(
                    "install-tool",
                    format!("Install {}", tool.name),
                    format!("{} ({}) isn't installed, or isn't on your PATH. Install it with winget, then restart the terminal so it can be found.", tool.name, tool.description),
                    format!("winget install --id {} -e", tool.winget),
                ));
            }
        }
        for candidate in index.did_you_mean(&name, 2) {
            suggestions.push(Suggestion::fix(
                "did-you-mean",
                format!("Did you mean {candidate}?"),
                format!("\"{name}\" isn't a command here, but \"{candidate}\" is installed."),
                with_first_word(&argument(&candidate), &rest),
            ));
        }
    }

    if suggestions.is_empty() {
        suggestions.push(Suggestion::advice(
            "not-found",
            format!("\"{name}\" isn't a known command"),
            "Check the spelling. If it's a program you installed recently, restart the terminal so it picks up the new PATH. You can also search for what you want to do with Ctrl+Shift+P.",
        ));
    }
    suggestions
}

fn python_store_stub(text: &str) -> Vec<Suggestion> {
    if !text.contains("Python was not found; run without arguments to install from the Microsoft Store") {
        return Vec::new();
    }
    vec![Suggestion::fix(
        "python-store-stub",
        "Install Python",
        "Windows includes a placeholder \"python\" that only opens the Microsoft Store. Installing Python with winget replaces it with the real thing.",
        "winget install --id Python.Python.3.12 -e",
    )]
}

fn scripts_disabled(report: &FailureReport, text: &str) -> Vec<Suggestion> {
    if !text.contains("running scripts is disabled on this system") {
        return Vec::new();
    }
    let mut suggestions = Vec::new();
    let (first, rest) = split_first(&report.command_line);
    if ["npm", "npx", "yarn", "pnpm"].contains(&first.to_lowercase().as_str()) {
        suggestions.push(Suggestion::fix(
            "use-cmd-shim",
            format!("Use {first}.cmd instead"),
            format!("{first} ships both a PowerShell script (blocked by your execution policy) and a .cmd version that isn't affected."),
            with_first_word(&format!("{first}.cmd"), &rest),
        ));
    }
    suggestions.push(Suggestion::fix(
        "execution-policy",
        "Allow local scripts for your account",
        "Windows blocks PowerShell scripts by default. RemoteSigned lets scripts you write or install run, while still blocking unsigned scripts downloaded from the internet. It only affects your user account.",
        "Set-ExecutionPolicy -Scope CurrentUser -ExecutionPolicy RemoteSigned",
    ));
    suggestions
}

/// npm package name for a module path: "lodash/fp" -> "lodash", "@scope/pkg/x" -> "@scope/pkg".
fn npm_package(module: &str) -> Option<String> {
    if module.starts_with(['.', '/']) || Path::new(module).is_absolute() {
        return None;
    }
    let mut parts = module.split('/');
    let first = parts.next()?;
    let package = if let Some(scoped) = first.strip_prefix('@') {
        format!("@{scoped}/{}", parts.next()?)
    } else {
        first.to_string()
    };
    // Anything that isn't a valid npm name (including `node:` built-ins) is
    // not something `npm install` can fix, and mustn't reach a command line.
    NPM_PACKAGE_NAME.is_match(&package).then_some(package)
}

fn node_errors(text: &str) -> Vec<Suggestion> {
    let mut suggestions = Vec::new();
    if let Some(module) = NODE_MISSING_MODULE.captures(text).map(|c| c[1].to_string()) {
        match npm_package(&module) {
            Some(package) => suggestions.push(Suggestion::fix(
                "npm-missing-module",
                format!("Install {package}"),
                format!("The code needs the package \"{package}\", which isn't installed in this project."),
                format!("npm install {package}"),
            )),
            None => suggestions.push(Suggestion::advice(
                "node-missing-file",
                format!("The file {module} wasn't found"),
                "Node couldn't find a file the code imports. Check the path, or that you're in the right folder.",
            )),
        }
    }
    if NPM_MISSING_SCRIPT.is_match(text) {
        suggestions.push(Suggestion::fix(
            "npm-list-scripts",
            "See which scripts this project has",
            "That script isn't defined in package.json. Running npm run on its own lists the scripts that are.",
            "npm run",
        ));
    }
    if text.contains("ENOENT") && text.contains("package.json") {
        suggestions.push(Suggestion::fix(
            "npm-no-package-json",
            "Create a package.json",
            "There's no package.json in this folder. If this should be a Node project, create one; otherwise cd into the project folder first.",
            "npm init -y",
        ));
    }
    if let Some(port) = PORT_IN_USE.captures(text).and_then(|c| c.iter().skip(1).flatten().next().map(|m| m.as_str().to_string())) {
        suggestions.push(Suggestion::fix(
            "port-in-use",
            format!("See what's using port {port}"),
            format!("Another program is already listening on port {port}. This shows which one, so you can close it or pick a different port."),
            format!("Get-NetTCPConnection -LocalPort {port} -State Listen | ForEach-Object {{ Get-Process -Id $_.OwningProcess }}"),
        ));
        suggestions.push(Suggestion::fix(
            "port-in-use-stop",
            format!("Stop the program using port {port}"),
            "Stops whatever is listening on the port. Unsaved work in that program is lost.",
            format!("Get-NetTCPConnection -LocalPort {port} -State Listen | ForEach-Object {{ Stop-Process -Id $_.OwningProcess }}"),
        ));
    }
    suggestions
}

/// pip package name for an import name where the two differ.
fn pip_package(module: &str) -> String {
    let top = module.split('.').next().unwrap_or(module);
    let package = match top {
        "cv2" => "opencv-python",
        "PIL" => "pillow",
        "sklearn" => "scikit-learn",
        "skimage" => "scikit-image",
        "yaml" => "pyyaml",
        "bs4" => "beautifulsoup4",
        "dotenv" => "python-dotenv",
        "Crypto" => "pycryptodome",
        "dateutil" => "python-dateutil",
        "jwt" => "PyJWT",
        "serial" => "pyserial",
        "docx" => "python-docx",
        "pptx" => "python-pptx",
        "fitz" => "PyMuPDF",
        "attr" => "attrs",
        "win32api" | "win32con" | "win32com" => "pywin32",
        other => other,
    };
    package.to_string()
}

fn python_errors(text: &str) -> Vec<Suggestion> {
    let Some(module) = PY_MISSING_MODULE.captures(text).map(|c| c[1].to_string()) else {
        return Vec::new();
    };
    let package = pip_package(&module);
    if !PIP_PACKAGE_NAME.is_match(&package) {
        return Vec::new();
    }
    vec![Suggestion::fix(
        "pip-missing-module",
        format!("Install {package}"),
        format!("The script imports \"{module}\", which isn't installed for this Python. If you use a virtual environment, activate it first."),
        format!("python -m pip install {package}"),
    )]
}

fn git_errors(report: &FailureReport, text: &str) -> Vec<Suggestion> {
    let mut suggestions = Vec::new();
    if text.contains("not a git repository") {
        suggestions.push(Suggestion::fix(
            "git-init",
            "Start a Git repository here",
            "This folder isn't tracked by Git. If it should be, initialise a repository; if you meant another project, cd into it first.",
            "git init",
        ));
    }
    if let Some(command) = GIT_SET_UPSTREAM.captures(text).map(|c| c[1].to_string()) {
        suggestions.push(Suggestion::fix(
            "git-set-upstream",
            "Push and link this branch to the remote",
            "This branch doesn't exist on the remote yet. This pushes it and remembers the link, so plain git push works next time.",
            command,
        ));
    }
    if text.contains("Please tell me who you are") {
        suggestions.push(Suggestion::fix(
            "git-identity",
            "Tell Git your name and email",
            "Git records an author on every commit. Replace the placeholders with your details, then commit again.",
            "git config --global user.name <your-name>; git config --global user.email <your-email>",
        ));
    }
    if let (Some(_), Some(similar)) = (GIT_NOT_A_COMMAND.captures(text), GIT_SIMILAR.captures(text).map(|c| c[1].to_string())) {
        let (_, rest) = split_first(&report.command_line);
        let (_, after_subcommand) = split_first(&rest);
        suggestions.push(Suggestion::fix(
            "git-typo",
            format!("Did you mean git {similar}?"),
            "Git didn't recognise that subcommand.",
            with_first_word(&format!("git {similar}"), &after_subcommand),
        ));
    }
    if text.contains("Updates were rejected") && (text.contains("fetch first") || text.contains("non-fast-forward") || text.contains("behind")) {
        suggestions.push(Suggestion::fix(
            "git-pull-rebase",
            "Get the remote changes first",
            "The remote has commits you don't have yet. This downloads them and replays your commits on top; then push again.",
            "git pull --rebase",
        ));
    }
    if text.contains("CONFLICT") && text.contains("Automatic merge failed") {
        suggestions.push(Suggestion::fix(
            "git-conflict",
            "See which files conflict",
            "Git couldn't combine some changes automatically. Edit the files it lists to resolve the conflicts, git add them, and commit.",
            "git status",
        ));
    }
    suggestions
}

/// Linux-style flags that PowerShell cmdlets don't accept.
fn bad_parameter(report: &FailureReport, text: &str) -> Vec<Suggestion> {
    let Some(parameter) = BAD_PARAMETER.captures(text).map(|c| c[1].to_lowercase()) else {
        return Vec::new();
    };
    let (first, rest) = split_first(&report.command_line);
    let lower = first.to_lowercase();
    let without_flag = |flag: &str| {
        rest.split_whitespace().filter(|w| !w.eq_ignore_ascii_case(flag)).collect::<Vec<_>>().join(" ")
    };
    let fixed = match (lower.as_str(), parameter.as_str()) {
        ("rm" | "remove-item" | "del" | "rmdir" | "rd", "rf" | "fr" | "r") => {
            Some(format!("{first} -Recurse -Force {}", without_flag(&format!("-{parameter}"))))
        }
        ("ls" | "dir" | "get-childitem" | "gci", "la" | "al" | "a" | "l" | "lah") => {
            Some(format!("{first} -Force {}", without_flag(&format!("-{parameter}"))))
        }
        ("cp" | "copy" | "copy-item", "r") => Some(format!("{first} -Recurse {}", without_flag("-r"))),
        ("kill" | "stop-process", "9") => Some(format!("Stop-Process -Force -Id {}", without_flag("-9"))),
        _ => None,
    };
    let mut suggestions = Vec::new();
    if let Some(command) = fixed {
        suggestions.push(Suggestion::fix(
            "linux-flags",
            "Use PowerShell's parameter names",
            format!("PowerShell parameters are words, not single letters: -{parameter} isn't valid for {first}."),
            command.trim_end().to_string(),
        ));
    }
    suggestions.push(Suggestion::fix(
        "show-parameters",
        format!("See the parameters {first} accepts"),
        format!("{first} doesn't have a -{parameter} parameter. Its help page lists the ones it does have, with examples."),
        format!("Get-Help {first} -Examples"),
    ));
    suggestions
}

/// Suggests the closest existing name when a path doesn't exist.
fn path_not_found(report: &FailureReport, text: &str) -> Vec<Suggestion> {
    let Some(missing) = PATH_NOT_FOUND.captures(text).map(|c| PathBuf::from(&c[1])) else {
        return Vec::new();
    };
    let (Some(parent), Some(name)) = (missing.parent(), missing.file_name().map(|n| n.to_string_lossy().into_owned())) else {
        return Vec::new();
    };
    // The path comes from command output, which anything can print. Only look
    // at local drives: merely listing `\\host\share` makes Windows send the
    // user's credentials to that host.
    if !is_local_drive_path(parent) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(parent) else {
        return vec![Suggestion::advice(
            "path-not-found",
            "That path doesn't exist",
            format!("The folder {} doesn't exist either. Check the path, or use the file tree to find what you're after.", parent.display()),
        )];
    };
    let names: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    let closest = fuzzy::closest(&name, names.iter().map(String::as_str), &HashSet::new(), 1).into_iter().next();
    match closest {
        Some(found) if report.command_line.contains(&name) => {
            let replacement = argument(&found);
            vec![Suggestion::fix(
                "path-typo",
                format!("Did you mean {found}?"),
                format!("There's no \"{name}\" here, but there is \"{found}\"."),
                report.command_line.replacen(&name, &replacement, 1),
            )]
        }
        _ => vec![Suggestion::fix(
            "path-not-found",
            "List what's in this folder",
            format!("There's nothing called \"{name}\" in {}. Listing the folder shows what is there.", parent.display()),
            "Get-ChildItem",
        )],
    }
}

/// `C:\x` or `C:/x`, as opposed to UNC (`\\host\share`) and device paths.
fn is_local_drive_path(path: &Path) -> bool {
    let bytes = path.as_os_str().as_encoded_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/')
}

/// `a && b` -> `a; if ($?) { b }` for Windows PowerShell 5.1. Gives up when
/// the line contains quotes, because `&&` inside a string (e.g. passed to
/// `cmd /c "a && b"`) is valid and must not be split.
pub fn convert_chain_operators(command_line: &str) -> Option<String> {
    if command_line.contains(['\'', '"']) {
        return None;
    }
    let parts: Vec<&str> = command_line.split("&&").map(str::trim).collect();
    if parts.len() < 2 || parts.iter().any(|p| p.is_empty() || p.contains("||")) {
        return None;
    }
    let mut converted = parts[0].to_string();
    for part in &parts[1..] {
        converted = format!("{converted}; if ($?) {{ {part} }}");
    }
    Some(converted)
}

fn powershell_syntax(report: &FailureReport, text: &str) -> Vec<Suggestion> {
    let mut suggestions = Vec::new();
    if text.contains("is not a valid statement separator")
        && report.command_line.contains("&&")
        && let Some(command) = convert_chain_operators(&report.command_line)
    {
        suggestions.push(Suggestion::fix(
            "chain-operator",
            "Run the commands one after another",
            "Windows PowerShell 5.1 doesn't understand && (PowerShell 7 does). This runs each command only if the previous one succeeded.",
            command,
        ));
    }
    if text.contains("operator is reserved for future use") && PLACEHOLDER.is_match(&report.command_line) {
        suggestions.push(Suggestion::advice(
            "unfilled-placeholder",
            "Fill in the placeholders",
            "The command still contains <placeholders>. Replace each one, including the angle brackets, with a real value.",
        ));
    }
    let denied = text.contains("Access to the path") && text.contains("is denied")
        || text.contains("requires elevation")
        || text.contains("Run as administrator")
        || report.error.as_ref().is_some_and(|e| e.exception_type.ends_with("UnauthorizedAccessException"));
    if denied {
        suggestions.push(Suggestion::fix(
            "needs-admin",
            "Open an administrator PowerShell",
            "Windows refused because this needs administrator rights, or the file is in use or read-only. If it needs admin, run it from an elevated window.",
            "Start-Process powershell -Verb RunAs",
        ));
    }
    suggestions
}

/// Runs every rule over a failed command and returns the most specific fixes.
pub fn analyze(report: &FailureReport, index: Option<&CommandIndex>) -> Vec<Suggestion> {
    let mut text = report.output.clone();
    if let Some(error) = &report.error {
        text.push('\n');
        text.push_str(&error.message);
    }
    let rules: [Vec<Suggestion>; 9] = [
        python_store_stub(&text),
        scripts_disabled(report, &text),
        powershell_syntax(report, &text),
        command_not_found(report, index, &text),
        node_errors(&text),
        python_errors(&text),
        git_errors(report, &text),
        bad_parameter(report, &text),
        path_not_found(report, &text),
    ];
    let mut seen = HashSet::new();
    rules
        .into_iter()
        .flatten()
        .filter(|s| seen.insert(s.command.clone().unwrap_or_else(|| s.title.clone())))
        .take(MAX_SUGGESTIONS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(command_line: &str, output: &str) -> FailureReport {
        FailureReport { command_line: command_line.into(), output: output.into(), ..Default::default() }
    }

    fn not_found(name: &str, command_line: &str) -> FailureReport {
        FailureReport {
            command_line: command_line.into(),
            error: Some(PsError {
                message: format!("The term '{name}' is not recognized as the name of a cmdlet, function, script file, or operable program."),
                error_id: "CommandNotFoundException".into(),
                category: "ObjectNotFound".into(),
                target: name.into(),
                exception_type: "System.Management.Automation.CommandNotFoundException".into(),
            }),
            ..Default::default()
        }
    }

    fn first_command(suggestions: &[Suggestion]) -> Option<&str> {
        suggestions.iter().find_map(|s| s.command.as_deref())
    }

    #[test]
    fn linux_commands_get_windows_equivalents() {
        let s = analyze(&not_found("touch", "touch notes.txt"), None);
        assert_eq!(first_command(&s), Some("New-Item -ItemType File -Path notes.txt"));
        let s = analyze(&not_found("export", "export API_URL=http://localhost"), None);
        assert_eq!(first_command(&s), Some("$env:API_URL = 'http://localhost'"));
        let s = analyze(&not_found("grep", "grep TODO *.ts"), None);
        assert_eq!(first_command(&s), Some("Select-String -Pattern TODO -Path *.ts"));
    }

    #[test]
    fn program_in_current_folder_needs_dot_slash() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("build.ps1"), "").unwrap();
        let mut failure = not_found("build.ps1", "build.ps1 -Release");
        failure.cwd = Some(dir.path().to_string_lossy().into_owned());
        let s = analyze(&failure, None);
        assert_eq!(s[0].rule, "run-from-current-folder");
        assert_eq!(s[0].command.as_deref(), Some(r".\build.ps1 -Release"));
    }

    #[test]
    fn unknown_command_without_index_gives_advice() {
        let s = analyze(&not_found("frobnicate", "frobnicate"), None);
        assert_eq!(s.len(), 1);
        assert!(s[0].command.is_none());
    }

    #[test]
    fn npm_missing_module_suggests_install_of_package_root() {
        let s = analyze(&report("node app.js", "Error: Cannot find module '@aws-sdk/client-s3/dist'\nRequire stack:"), None);
        assert_eq!(first_command(&s), Some("npm install @aws-sdk/client-s3"));
        let s = analyze(&report("node app.js", "Error: Cannot find module './config'"), None);
        assert!(s[0].command.is_none());
    }

    #[test]
    fn port_in_use_offers_inspect_then_stop() {
        let s = analyze(&report("npm run dev", "Error: listen EADDRINUSE: address already in use :::3000"), None);
        assert_eq!(s[0].rule, "port-in-use");
        assert!(s[0].command.as_deref().unwrap().contains("-LocalPort 3000"));
        assert_eq!(s[1].risk.as_ref().unwrap().level, risk::RiskLevel::Caution);
    }

    #[test]
    fn python_import_names_map_to_pip_packages() {
        let s = analyze(&report("python app.py", "ModuleNotFoundError: No module named 'cv2'"), None);
        assert_eq!(first_command(&s), Some("python -m pip install opencv-python"));
    }

    #[test]
    fn git_upstream_command_is_lifted_from_output() {
        let output = "fatal: The current branch feature has no upstream branch.\nTo push the current branch and set the remote as upstream, use\n\n    git push --set-upstream origin feature\n";
        let s = analyze(&report("git push", output), None);
        assert_eq!(first_command(&s), Some("git push --set-upstream origin feature"));
    }

    #[test]
    fn git_subcommand_typo_uses_gits_own_suggestion() {
        let output = "git: 'stauts' is not a git command. See 'git --help'.\n\nThe most similar command is\n\tstatus";
        let s = analyze(&report("git stauts -s", output), None);
        assert_eq!(first_command(&s), Some("git status -s"));
    }

    #[test]
    fn linux_flags_are_translated() {
        let failure = FailureReport {
            command_line: "rm -rf build".into(),
            error: Some(PsError {
                message: "A parameter cannot be found that matches parameter name 'rf'.".into(),
                error_id: "NamedParameterNotFound,Microsoft.PowerShell.Commands.RemoveItemCommand".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let s = analyze(&failure, None);
        assert_eq!(s[0].command.as_deref(), Some("rm -Recurse -Force build"));
        assert_eq!(s[0].risk.as_ref().unwrap().level, risk::RiskLevel::Danger);
    }

    #[test]
    fn chain_operator_is_rewritten_for_powershell_5() {
        let s = analyze(&report("npm install && npm test", "The token '&&' is not a valid statement separator in this version."), None);
        assert_eq!(first_command(&s), Some("npm install; if ($?) { npm test }"));
    }

    #[test]
    fn execution_policy_prefers_cmd_shim_for_npm() {
        let output = "npm : File C:\\Program Files\\nodejs\\npm.ps1 cannot be loaded because running scripts is disabled on this system.";
        let s = analyze(&report("npm install", output), None);
        assert_eq!(s[0].command.as_deref(), Some("npm.cmd install"));
        assert!(s[1].command.as_deref().unwrap().starts_with("Set-ExecutionPolicy -Scope CurrentUser"));
    }

    #[test]
    fn missing_path_suggests_closest_sibling() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Documents")).unwrap();
        let missing = dir.path().join("Documnets");
        let output = format!("Cannot find path '{}' because it does not exist.", missing.display());
        let s = analyze(&report("cd Documnets", &output), None);
        assert_eq!(first_command(&s), Some("cd Documents"));
    }

    #[test]
    fn unrelated_failures_produce_nothing() {
        assert!(analyze(&report("npm test", "1 test failed"), None).is_empty());
    }

    #[test]
    fn network_paths_from_output_are_never_touched() {
        let output = r"Cannot find path '\\attacker.example\share\x' because it does not exist.";
        assert!(analyze(&report("cd x", output), None).is_empty());
        let mut failure = not_found(r"\\attacker.example\share\tool", r"\\attacker.example\share\tool");
        failure.cwd = Some(r"C:\Users".into());
        assert!(analyze(&failure, None).iter().all(|s| s.rule != "run-from-current-folder"));
    }

    #[test]
    fn program_names_with_spaces_are_quoted_when_run_from_current_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("my tool.exe"), "").unwrap();
        let mut failure = not_found("my tool.exe", "'my tool.exe' --help");
        failure.cwd = Some(dir.path().to_string_lossy().into_owned());
        let s = analyze(&failure, None);
        assert_eq!(s[0].command.as_deref(), Some(r"& '.\my tool.exe' --help"));
    }

    #[test]
    fn values_lifted_from_output_cannot_inject_commands() {
        let upstream = "To push, use\n\n    git push --set-upstream origin x;calc.exe\n";
        assert!(analyze(&report("git push", upstream), None).iter().all(|s| s.rule != "git-set-upstream"));
        let npm = analyze(&report("node app.js", "Error: Cannot find module 'x;calc'"), None);
        assert!(npm[0].command.is_none());
        let pip = analyze(&report("python a.py", "ModuleNotFoundError: No module named 'x$(calc)'"), None);
        assert!(pip.is_empty());
        let git_typo = "git: 'stauts' is not a git command.\n\nThe most similar command is\n\tstatus;calc";
        assert_eq!(first_command(&analyze(&report("git stauts", git_typo), None)), Some("git status"));
    }

    #[test]
    fn chain_operators_inside_quotes_are_left_alone() {
        assert_eq!(convert_chain_operators(r#"cmd /c "npm i && npm test""#), None);
        assert_eq!(convert_chain_operators("npm i && npm test").as_deref(), Some("npm i; if ($?) { npm test }"));
    }
}
