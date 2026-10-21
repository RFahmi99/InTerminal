//! What can actually run on this machine: executables on PATH plus the
//! cmdlets, functions and aliases PowerShell can load.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::shell::quote::encoded_command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
/// Listing commands takes a second or two; a cold start on a slow disk, longer.
const COMMANDS_TIMEOUT: Duration = Duration::from_secs(60);
/// Loading every module takes about a minute on a typical machine.
const USAGE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PsCommandKind {
    Cmdlet,
    Function,
    Alias,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PsCommand {
    pub name: String,
    pub kind: PsCommandKind,
    /// For aliases, the command the alias points to.
    pub definition: Option<String>,
    /// The default parameter set, e.g. `Expand-Archive [-Path] <string> [-Force]`.
    #[serde(default)]
    pub usage: Option<String>,
}

/// What the background PowerShell query reports.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PsQuery {
    pub commands: Vec<PsCommand>,
    /// Folders PowerShell keeps help files in (`$PSHOME`, module paths).
    pub help_roots: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct Inventory {
    /// Lower-case file stem (e.g. "git") to the first matching file on PATH.
    pub executables: HashMap<String, PathBuf>,
    /// Lower-case name to command.
    pub ps_commands: HashMap<String, PsCommand>,
}

impl Inventory {
    pub fn new(executables: HashMap<String, PathBuf>, ps_commands: Vec<PsCommand>) -> Self {
        let ps_commands = ps_commands.into_iter().map(|c| (c.name.to_lowercase(), c)).collect();
        Self { executables, ps_commands }
    }

    /// Whether `name` (with or without an extension like `.exe`) would run.
    pub fn has(&self, name: &str) -> bool {
        let lower = name.to_lowercase();
        if self.ps_commands.contains_key(&lower) || self.executables.contains_key(&lower) {
            return true;
        }
        Path::new(&lower).file_stem().and_then(OsStr::to_str).is_some_and(|stem| stem != lower && self.executables.contains_key(stem))
    }

    /// Every runnable name, in its display spelling, for "did you mean" matching.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.ps_commands.values().map(|c| c.name.as_str()).chain(self.executables.keys().map(String::as_str))
    }
}

/// Finds executables in every PATH directory whose extension is in PATHEXT.
/// Earlier directories win, matching how Windows resolves commands.
pub fn scan_path(path_var: &OsStr, pathext: &str) -> HashMap<String, PathBuf> {
    let extensions: Vec<String> =
        pathext.split(';').map(|e| e.trim().trim_start_matches('.').to_lowercase()).filter(|e| !e.is_empty()).collect();
    let mut found = HashMap::new();
    for dir in std::env::split_paths(path_var) {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            // Stale PATH entries are common and harmless.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                log::debug!("Skipping unreadable PATH folder {}: {e}", dir.display());
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(extension) = path.extension().and_then(OsStr::to_str).map(str::to_lowercase) else {
                continue;
            };
            if !extensions.contains(&extension) {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(OsStr::to_str) {
                found.entry(stem.to_lowercase()).or_insert(path);
            }
        }
    }
    found
}

/// Scans the PATH new programs would get (read fresh from the registry), so
/// tools installed since VCE started are found after a refresh.
pub fn scan_current_path() -> HashMap<String, PathBuf> {
    let path_var = crate::env_path::current();
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    scan_path(&path_var, &pathext)
}

// Lists commands without loading their modules, so it takes a second or two.
const LIST_COMMANDS_SCRIPT: &str = r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$ErrorActionPreference = 'SilentlyContinue'
$ProgressPreference = 'SilentlyContinue'
$commands = Get-Command -CommandType Alias, Cmdlet, Function | ForEach-Object {
    [pscustomobject]@{
        name = $_.Name
        kind = "$($_.CommandType)"
        definition = if ($_.CommandType -eq 'Alias') { $_.Definition } else { $null }
        usage = $null
    }
}
$helpRoots = @($PSHOME) + @($env:PSModulePath -split ';' | Where-Object { $_ }) +
    @(Join-Path ([Environment]::GetFolderPath('MyDocuments')) 'PowerShell\Help')
[pscustomobject]@{ commands = @($commands); helpRoots = @($helpRoots) } | ConvertTo-Json -Compress -Depth 4
"#;

// A command's parameter sets are only known once its module is loaded, so
// this loads every module first: ~50 s and several hundred MB in the hidden
// process, which is why its result is cached and refreshed rarely. A module
// that can't load simply leaves its commands without a usage line.
const LIST_USAGE_SCRIPT: &str = r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$ErrorActionPreference = 'SilentlyContinue'
$ProgressPreference = 'SilentlyContinue'
Get-Module -ListAvailable | Select-Object -ExpandProperty Name -Unique | ForEach-Object {
    # A module may print a banner while loading; it must not reach the JSON.
    Import-Module $_ -ErrorAction SilentlyContinue -WarningAction SilentlyContinue *> $null
}
$usages = Get-Command -CommandType Cmdlet, Function | Where-Object { $_.Name -match '^[A-Za-z]+-[A-Za-z0-9]+$' } | ForEach-Object {
    $set = $_.ParameterSets | Where-Object IsDefault | Select-Object -First 1
    if (-not $set) { $set = $_.ParameterSets | Select-Object -First 1 }
    $parameters = ("$set" -replace '\s*\[<CommonParameters>\]', '').Trim()
    if ($parameters) { [pscustomobject]@{ name = $_.Name; usage = "$($_.Name) $parameters" } }
}
ConvertTo-Json -InputObject @($usages) -Compress
"#;

/// Reads `pipe` to the end on its own thread, so a full pipe can't stall the
/// child while the caller waits for it to exit.
fn read_in_background(pipe: Option<impl Read + Send + 'static>, name: &'static str) -> JoinHandle<String> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe
            && let Err(e) = pipe.read_to_end(&mut bytes)
        {
            log::warn!("Reading PowerShell's {name} failed: {e}");
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

fn joined(reader: JoinHandle<String>) -> String {
    reader.join().unwrap_or_else(|_| {
        log::warn!("The thread reading PowerShell's output panicked");
        String::new()
    })
}

/// Runs `script` in a hidden PowerShell without the user's profile and
/// returns its output, killing it if it runs past `timeout` (a module that
/// hangs while loading would otherwise stall indexing for good).
fn run_hidden(shell: &Path, script: &str, priority: u32, timeout: Duration) -> Result<String, String> {
    let mut child = Command::new(shell)
        .args(["-NoProfile", "-NonInteractive", "-NoLogo", "-EncodedCommand", &encoded_command(script)])
        .creation_flags(CREATE_NO_WINDOW | priority)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("starting {}: {e}", shell.display()))?;
    let stdout = read_in_background(child.stdout.take(), "output");
    let stderr = read_in_background(child.stderr.take(), "error output");
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > timeout => {
                if let Err(e) = child.kill() {
                    log::warn!("Could not stop {} after it timed out: {e}", shell.display());
                }
                return Err(format!("{} didn't finish within {} s", shell.display(), timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("waiting for {}: {e}", shell.display())),
        }
    };
    let (stdout, stderr) = (joined(stdout), joined(stderr));
    let stderr_start: String = stderr.trim().chars().take(500).collect();
    if !status.success() {
        return Err(format!("{} exited with {status}: {stderr_start}", shell.display()));
    }
    // The scripts silence errors so one broken module can't fail the whole
    // query, which also means a successful run can still have hit problems.
    if !stderr_start.is_empty() {
        log::warn!("{} reported: {stderr_start}", shell.display());
    }
    Ok(stdout)
}

/// Asks a separate, hidden PowerShell (without the user's profile) for every
/// command its modules provide. Takes a second or two; callers run it in the
/// background and cache the result.
pub fn query_ps_commands(shell: &Path) -> Result<PsQuery, String> {
    let query = parse_ps_commands(&run_hidden(shell, LIST_COMMANDS_SCRIPT, 0, COMMANDS_TIMEOUT)?)?;
    // Errors are silenced in the script, so a broken PowerShell shows up as
    // an empty list rather than a failure; it must not replace a good cache.
    if query.commands.is_empty() {
        return Err("PowerShell listed no commands".to_string());
    }
    Ok(query)
}

/// Lower-case command name to its parameter usage. Slow and memory-hungry
/// (see `LIST_USAGE_SCRIPT`), so it runs at below-normal priority and its
/// result is cached.
pub fn query_ps_usage(shell: &Path) -> Result<HashMap<String, String>, String> {
    #[derive(Deserialize)]
    struct RawUsage {
        name: String,
        usage: String,
    }
    let json = run_hidden(shell, LIST_USAGE_SCRIPT, BELOW_NORMAL_PRIORITY_CLASS, USAGE_TIMEOUT)?;
    let raw: Vec<RawUsage> =
        serde_json::from_str(json.trim_start_matches('\u{feff}').trim()).map_err(|e| format!("parsing parameter usage output: {e}"))?;
    if raw.is_empty() {
        return Err("PowerShell reported no parameter usage".to_string());
    }
    Ok(raw.into_iter().map(|u| (u.name.to_lowercase(), u.usage)).collect())
}

/// Fills in each command's usage from `usages`, keyed by lower-case name.
pub fn apply_usage(commands: &mut [PsCommand], usages: &HashMap<String, String>) {
    for command in commands {
        command.usage = usages.get(&command.name.to_lowercase()).cloned();
    }
}

#[derive(Deserialize)]
struct RawPsCommand {
    name: String,
    kind: String,
    definition: Option<String>,
    usage: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPsQuery {
    commands: Vec<RawPsCommand>,
    #[serde(default)]
    help_roots: Vec<String>,
}

fn parse_ps_commands(json: &str) -> Result<PsQuery, String> {
    let json = json.trim_start_matches('\u{feff}').trim();
    let raw: RawPsQuery = serde_json::from_str(json).map_err(|e| format!("parsing Get-Command output: {e}"))?;
    let commands = raw
        .commands
        .into_iter()
        .filter_map(|c| {
            let kind = match c.kind.as_str() {
                "Cmdlet" => PsCommandKind::Cmdlet,
                "Function" => PsCommandKind::Function,
                "Alias" => PsCommandKind::Alias,
                _ => return None,
            };
            Some(PsCommand { name: c.name, kind, definition: c.definition, usage: c.usage })
        })
        .collect();
    Ok(PsQuery { commands, help_roots: raw.help_roots.into_iter().map(PathBuf::from).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_finds_pathext_files_and_first_dir_wins() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        std::fs::write(first.path().join("tool.EXE"), "").unwrap();
        std::fs::write(second.path().join("tool.cmd"), "").unwrap();
        std::fs::write(second.path().join("readme.txt"), "").unwrap();
        let path_var = std::env::join_paths([first.path(), Path::new(r"Z:\missing"), second.path()]).unwrap();
        let found = scan_path(&path_var, ".COM;.EXE;.BAT;.CMD");
        assert_eq!(found.len(), 1);
        assert_eq!(found["tool"], first.path().join("tool.EXE"));
    }

    #[test]
    fn hidden_powershell_returns_output_and_is_stopped_at_the_deadline() {
        let shell = crate::shell::find_shell();
        let output = run_hidden(&shell, "Write-Output 'ready'", 0, Duration::from_secs(60)).unwrap();
        assert_eq!(output.trim(), "ready");
        let started = Instant::now();
        let error = run_hidden(&shell, "Start-Sleep -Seconds 30", 0, Duration::from_secs(1)).unwrap_err();
        assert!(error.contains("didn't finish within 1 s"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn usage_is_applied_by_name_case_insensitively() {
        let mut commands = vec![
            PsCommand { name: "New-Volume".into(), kind: PsCommandKind::Function, definition: None, usage: None },
            PsCommand { name: "Clear-Host".into(), kind: PsCommandKind::Function, definition: None, usage: None },
        ];
        let usages = HashMap::from([("new-volume".to_string(), "New-Volume -FriendlyName <string>".to_string())]);
        apply_usage(&mut commands, &usages);
        assert_eq!(commands[0].usage.as_deref(), Some("New-Volume -FriendlyName <string>"));
        assert_eq!(commands[1].usage, None);
    }

    #[test]
    fn has_matches_names_case_insensitively_with_extensions() {
        let inventory = Inventory::new(
            HashMap::from([("git".to_string(), PathBuf::from(r"C:\git.exe"))]),
            vec![PsCommand { name: "Expand-Archive".into(), kind: PsCommandKind::Function, definition: None, usage: None }],
        );
        assert!(inventory.has("GIT"));
        assert!(inventory.has("git.exe"));
        assert!(inventory.has("expand-archive"));
        assert!(!inventory.has("docker"));
    }

    #[test]
    fn parses_get_command_json_and_skips_unknown_kinds() {
        let json = r#"{"commands":[{"name":"ls","kind":"Alias","definition":"Get-ChildItem","usage":null},{"name":"x","kind":"Filter","definition":null,"usage":null},{"name":"Expand-Archive","kind":"Function","definition":null,"usage":"Expand-Archive [-Path] <string>"}],"helpRoots":["C:\\Windows\\System32\\WindowsPowerShell\\v1.0"]}"#;
        let query = parse_ps_commands(&format!("\u{feff}{json}")).unwrap();
        assert_eq!(query.commands.len(), 2);
        assert_eq!(query.commands[0].definition.as_deref(), Some("Get-ChildItem"));
        assert_eq!(query.commands[1].usage.as_deref(), Some("Expand-Archive [-Path] <string>"));
        assert_eq!(query.help_roots, vec![PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0")]);
    }
}
