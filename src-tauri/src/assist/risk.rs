//! Static risk assessment for commands the app proposes (search results,
//! error fixes, AI output) before the user runs them. It is deliberately
//! conservative: a false warning costs a second look, a missed one can cost
//! data.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Safe,
    Caution,
    Danger,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskReport {
    pub level: RiskLevel,
    /// Plain-language reasons, most severe first. Empty when no risky pattern
    /// was found (which is not a guarantee of safety).
    pub reasons: Vec<String>,
}

impl RiskReport {
    /// Raises the level to at least `floor`, e.g. a catalog entry's own rating.
    pub fn at_least(mut self, floor: RiskLevel) -> Self {
        self.level = self.level.max(floor);
        self
    }
}

struct Rule {
    level: RiskLevel,
    pattern: Regex,
    reason: &'static str,
}

/// Where a command can start: the beginning, a statement or pipeline
/// separator, an opening bracket, an assignment (`$null = rm x`), the opening
/// quote of an inner command (`cmd /c "rd ..."`, `powershell -c '...'`), or
/// after `cmd /c`.
const COMMAND_START: &str = r#"(?:^|[;|&({"'=]|/c)\s*"#;

/// Matches `names` where a command starts.
fn command(names: &str) -> String {
    format!(r#"{COMMAND_START}(?:{names})(?:\.exe)?(?:\s|$|;|\||'|")"#)
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    use RiskLevel::{Caution, Danger};
    let rules: Vec<(RiskLevel, String, &'static str)> = vec![
        (Danger, command("remove-item|rm|del|erase|rd|rmdir|ri"),
            "Deletes files or folders permanently. They don't go to the Recycle Bin."),
        (Danger, r"\b(?:format-volume|clear-disk|initialize-disk|diskpart)\b".into(),
            "Erases or repartitions a disk."),
        (Danger, format!(r"{COMMAND_START}format(?:\.com)?\s+[a-z]:"), "Formats a drive, erasing everything on it."),
        (Danger, format!(r"\b(?:stop-computer|restart-computer)\b|{}", command("shutdown")),
            "Shuts down or restarts the computer. Unsaved work in other apps is lost."),
        (Danger, r"\b(?:invoke-webrequest|iwr|invoke-restmethod|irm|curl|wget)\b[^;]*\|\s*(?:invoke-expression|iex)\b".into(),
            "Downloads code from the internet and runs it immediately, without letting you read it first."),
        (Danger, r"\b(?:invoke-expression|iex)\b".into(), "Runs text as PowerShell code, which can do anything."),
        (Danger, r"\bset-executionpolicy\b.*\b(?:unrestricted|bypass)\b".into(),
            "Turns off PowerShell's script safety checks."),
        (Caution, r"\bset-executionpolicy\b".into(), "Changes which scripts PowerShell is allowed to run."),
        (Danger, r"\breg(?:\.exe)?\s+(?:delete|add|import)\b".into(), "Changes the Windows registry."),
        (Danger, r"\b(?:remove-itemproperty|set-itemproperty|new-itemproperty|remove-item)\b[^|;]*\b(?:hklm|hkcu|registry::)".into(),
            "Changes the Windows registry."),
        (Danger, r"\bgit\s+(?:reset\s+[^;|]*--hard|clean\s+-[a-z]*f|checkout\s+--\s|restore\s)".into(),
            "Discards Git changes. Uncommitted work can't be recovered."),
        (Danger, r"\bgit\s+push\b[^;|]*(?:--force|\s-f\b)".into(), "Overwrites history on the remote Git server."),
        (Danger, r"\b(?:bcdedit|takeown)\b|\bcipher(?:\.exe)?\s+/w".into(),
            "Changes boot settings, file ownership or wipes free space."),
        (Danger, r"\bicacls\b[^;|]*\s/(?:grant|deny|reset|remove|setowner)".into(), "Changes who can access files."),
        (Danger, r"\b(?:new-localuser|remove-localuser|add-localgroupmember|remove-localgroupmember)\b".into(),
            "Changes user accounts or their permissions."),
        (Danger, r"\bnet(?:\.exe)?\s+(?:user|localgroup)\b[^;|]*\s/(?:add|delete)".into(), "Changes user accounts or their permissions."),
        (Danger, r"\bset-netfirewallprofile\b[^;|]*-enabled\s+\$?false|\bset-mppreference\b[^;|]*-disable|\bnetsh\s+advfirewall\s+set\b[^;|]*\boff\b".into(),
            "Turns off Windows security protection."),
        (Danger, r"\bdocker\s+(?:system\s+prune|volume\s+(?:rm|prune)|image\s+prune|rmi)\b".into(), "Deletes Docker images or data."),
        (Danger, r"\brobocopy\b[^;|]*\s/(?:mir|purge)\b".into(), "Deletes files in the destination that aren't in the source."),
        (Danger, r"\b(?:remove-service|sc(?:\.exe)?\s+delete)\b".into(), "Removes a Windows service."),
        (Caution, command("stop-process|kill|spps|taskkill"), "Stops running programs. Unsaved work in them is lost."),
        (Caution, r"\b(?:stop-service|restart-service|sc(?:\.exe)?\s+stop)\b".into(), "Stops or restarts a Windows service."),
        (Caution, command("move-item|mv|move|mi|rename-item|ren|rni"), "Moves or renames files."),
        (Caution, r"\b(?:copy-item|cp|copy|cpi|xcopy)\b[^;|]*\s-force\b".into(), "May overwrite existing files."),
        (Caution, r"\b(?:set-content|out-file|clear-content|clc)\b".into(), "Replaces a file's contents."),
        (Caution, r"(?:^|[^>0-9&*])>(?:[^>&]|$)".into(), "Redirects output into a file, replacing what's in it."),
        (Caution, r"\b(?:npm|pnpm|yarn)\s+(?:install|i|add|uninstall|remove|rm|update|upgrade)\b|\bpip3?\s+(?:install|uninstall)\b|\b(?:winget|choco|scoop)\s+(?:install|uninstall|upgrade|remove)\b|\binstall-(?:module|package)\b|\bcargo\s+install\b".into(),
            "Installs or removes software."),
        (Caution, r"\bstart-process\b[^;|]*-verb\s+runas\b".into(), "Asks for administrator rights."),
        (Caution, r"\[environment\]::setenvironmentvariable\b|\bsetx\b".into(), "Permanently changes an environment variable."),
        (Caution, r"\bgit\s+(?:push|rebase|merge|stash\s+(?:drop|clear)|commit\s+[^;|]*--amend)\b".into(),
            "Changes your Git history or publishes it."),
        (Caution, r"\bdocker\s+(?:stop|kill|rm)\b".into(), "Stops or removes Docker containers."),
    ];
    rules
        .into_iter()
        .map(|(level, pattern, reason)| Rule {
            level,
            pattern: Regex::new(&pattern).unwrap_or_else(|e| panic!("invalid risk pattern {pattern:?}: {e}")),
            reason,
        })
        .collect()
});

/// `<placeholder>` slots in command templates; their angle brackets must not
/// read as redirection.
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[a-z0-9-]+>").expect("valid regex"));

pub fn assess(command: &str) -> RiskReport {
    let text = PLACEHOLDER.replace_all(&command.to_lowercase(), "value").into_owned();
    let mut matched: Vec<&Rule> = RULES.iter().filter(|rule| rule.pattern.is_match(&text)).collect();
    matched.sort_by(|a, b| b.level.cmp(&a.level));
    let mut reasons: Vec<String> = Vec::new();
    for rule in &matched {
        if !reasons.iter().any(|r| r == rule.reason) {
            reasons.push(rule.reason.to_string());
        }
    }
    RiskReport { level: matched.first().map_or(RiskLevel::Safe, |rule| rule.level), reasons }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(command: &str) -> RiskLevel {
        assess(command).level
    }

    #[test]
    fn read_only_commands_are_safe() {
        for command in [
            "Get-ChildItem -Force",
            "git status",
            "Get-Process | Sort-Object CPU -Descending | Select-Object -First 10",
            "Test-NetConnection example.com -Port 443",
            "Get-ChildItem -Filter *.rm",
            "dir 2>$null",
            "npm run build 2>&1",
            "if ($a -gt 5) { 'big' }",
        ] {
            assert_eq!(level(command), RiskLevel::Safe, "{command}");
        }
    }

    #[test]
    fn deletion_is_dangerous_in_any_spelling() {
        for command in ["Remove-Item -Recurse -Force .\\build", "rm -r node_modules", "del *.log", "cmd /c rd /s /q dist", "Get-ChildItem *.tmp | Remove-Item"] {
            assert_eq!(level(command), RiskLevel::Danger, "{command}");
        }
    }

    #[test]
    fn download_and_execute_is_dangerous() {
        let report = assess("iwr https://example.com/install.ps1 | iex");
        assert_eq!(report.level, RiskLevel::Danger);
        assert!(report.reasons[0].contains("internet"));
    }

    #[test]
    fn execution_policy_severity_depends_on_target() {
        assert_eq!(level("Set-ExecutionPolicy -Scope CurrentUser RemoteSigned"), RiskLevel::Caution);
        assert_eq!(level("Set-ExecutionPolicy Bypass"), RiskLevel::Danger);
    }

    #[test]
    fn git_history_rewrites_are_flagged() {
        assert_eq!(level("git reset --hard HEAD~1"), RiskLevel::Danger);
        assert_eq!(level("git push -f origin main"), RiskLevel::Danger);
        assert_eq!(level("git push origin main"), RiskLevel::Caution);
        assert_eq!(level("git log --oneline"), RiskLevel::Safe);
    }

    #[test]
    fn process_and_install_commands_need_caution() {
        assert_eq!(level("Stop-Process -Name notepad"), RiskLevel::Caution);
        assert_eq!(level("npm install express"), RiskLevel::Caution);
        assert_eq!(level("winget install --id Git.Git -e"), RiskLevel::Caution);
        assert_eq!(level("Get-Process > procs.txt"), RiskLevel::Caution);
        assert_eq!(level("Get-Process >> procs.txt"), RiskLevel::Safe);
    }

    #[test]
    fn placeholders_are_not_redirection() {
        assert_eq!(level("tar.exe -tf <archive>"), RiskLevel::Safe);
        assert_eq!(level("Remove-Item <path>"), RiskLevel::Danger);
    }

    #[test]
    fn deletes_inside_inner_commands_and_assignments_are_dangerous() {
        for command in [r#"cmd /c "rd /s /q dist""#, r#"powershell -c "Remove-Item -Recurse build""#, "$null = Remove-Item x", "pwsh -c 'rm -r out'"] {
            assert_eq!(level(command), RiskLevel::Danger, "{command}");
        }
    }

    #[test]
    fn floor_raises_but_never_lowers() {
        assert_eq!(assess("git status").at_least(RiskLevel::Caution).level, RiskLevel::Caution);
        assert_eq!(assess("rm x").at_least(RiskLevel::Safe).level, RiskLevel::Danger);
    }
}
