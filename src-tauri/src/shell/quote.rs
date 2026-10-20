//! Building PowerShell command text safely from untrusted values (file names,
//! user-typed parameters, model output).

use std::path::Path;

use base64::Engine as _;

/// Encodes a script for `powershell.exe -EncodedCommand` (base64 of UTF-16LE),
/// which sidesteps Windows command-line quoting rules entirely.
pub fn encoded_command(script: &str) -> String {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(utf16)
}

/// Characters PowerShell treats as a single quote inside a single-quoted string.
/// Besides `'` this includes the typographic quotes, which PowerShell also accepts
/// as string delimiters, so each must be doubled to stay literal.
const SINGLE_QUOTES: [char; 5] = ['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'];

/// Wraps `value` in a PowerShell single-quoted string, in which nothing is
/// expanded (`$`, backtick and `(` are all literal).
pub fn single_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for ch in value.chars() {
        if SINGLE_QUOTES.contains(&ch) {
            quoted.push(ch);
        }
        quoted.push(ch);
    }
    quoted.push('\'');
    quoted
}

/// Returns `value` unquoted when PowerShell would read it back as the same
/// single bare-word argument, otherwise single-quotes it. Keeps generated
/// commands readable (`cd src`) without risking injection.
pub fn argument(value: &str) -> String {
    let bare_ok = !value.is_empty()
        && !value.starts_with('-')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '\\' | ':' | '/'));
    if bare_ok { value.to_string() } else { single_quote(value) }
}

/// Text the app types into the terminal goes through ConPTY as keystrokes, so
/// a control character (Backspace via DEL, Esc, Ctrl+key, C1 codes) would act
/// as a key press and could undo the quoting around a value. Returns the first
/// such character.
pub fn first_control_char(text: &str) -> Option<char> {
    text.chars().find(|c| c.is_control())
}

fn has_wildcard(value: &str) -> bool {
    value.contains(['[', ']', '*', '?', '`'])
}

fn same_path(a: &Path, b: &Path) -> bool {
    let normalize = |p: &Path| p.to_string_lossy().trim_end_matches('\\').to_lowercase();
    normalize(a) == normalize(b)
}

/// Builds the `cd` command typed into the shell when the user navigates from
/// the file tree. Prefers the short relative form a person would type, and
/// switches to `-LiteralPath` when the name contains wildcard characters that
/// `cd`'s default `-Path` parameter would otherwise expand.
pub fn change_dir_command(target: &Path, cwd: Option<&Path>) -> String {
    if let Some(cwd) = cwd {
        if cwd.parent().is_some_and(|parent| same_path(parent, target)) {
            return "cd ..".to_string();
        }
        if let (Some(parent), Some(name)) = (target.parent(), target.file_name())
            && same_path(parent, cwd)
        {
            let name = name.to_string_lossy();
            return if has_wildcard(&name) {
                format!("cd -LiteralPath {}", single_quote(&name))
            } else {
                format!("cd {}", argument(&name))
            };
        }
    }
    let full = target.to_string_lossy();
    if has_wildcard(&full) {
        format!("cd -LiteralPath {}", single_quote(&full))
    } else {
        format!("cd {}", argument(&full))
    }
}

/// Collapses a multi-line command into one line so it can be typed at the
/// prompt: a raw newline would submit each line separately. Lines that end in
/// a continuation token are joined with a space, other lines with `; `.
pub fn to_single_line(command: &str) -> String {
    // A lone `\r` is a line break too: typed into the terminal it is Enter.
    let lines: Vec<&str> = command
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let mut joined = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            let previous = lines[i - 1];
            let continues = previous.ends_with(['|', '{', '(', ',', '`', '='])
                || line.starts_with(['|', '}', ')']);
            if continues {
                joined.push(' ');
            } else {
                joined.push_str("; ");
            }
        }
        joined.push_str(line.strip_suffix('`').map(str::trim_end).unwrap_or(line));
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn encoded_command_is_utf16le_base64() {
        // "dir" in UTF-16LE is 64 00 69 00 72 00.
        assert_eq!(encoded_command("dir"), "ZABpAHIA");
    }

    #[test]
    fn single_quote_doubles_every_quote_variant() {
        assert_eq!(single_quote("it's"), "'it''s'");
        assert_eq!(single_quote("a\u{2019}b"), "'a\u{2019}\u{2019}b'");
        assert_eq!(single_quote("$(rm -r C:\\)"), "'$(rm -r C:\\)'");
    }

    #[test]
    fn argument_leaves_simple_values_bare() {
        assert_eq!(argument("src"), "src");
        assert_eq!(argument(".\\photos.zip"), ".\\photos.zip");
        assert_eq!(argument("3000"), "3000");
    }

    #[test]
    fn argument_quotes_spaces_operators_and_leading_dash() {
        assert_eq!(argument("My Documents"), "'My Documents'");
        assert_eq!(argument("a;b"), "'a;b'");
        assert_eq!(argument("-rf"), "'-rf'");
        assert_eq!(argument(""), "''");
    }

    #[test]
    fn cd_into_child_is_relative() {
        let cwd = PathBuf::from(r"C:\Users\me");
        assert_eq!(change_dir_command(&cwd.join("src"), Some(&cwd)), "cd src");
        assert_eq!(change_dir_command(&cwd.join("My Docs"), Some(&cwd)), "cd 'My Docs'");
    }

    #[test]
    fn cd_to_parent_uses_dot_dot_case_insensitively() {
        let cwd = PathBuf::from(r"C:\Users\me");
        assert_eq!(change_dir_command(&PathBuf::from(r"c:\users"), Some(&cwd)), "cd ..");
    }

    #[test]
    fn cd_with_wildcard_characters_uses_literal_path() {
        let cwd = PathBuf::from(r"C:\data");
        assert_eq!(
            change_dir_command(&cwd.join("[draft]"), Some(&cwd)),
            "cd -LiteralPath '[draft]'"
        );
    }

    #[test]
    fn cd_elsewhere_uses_full_quoted_path() {
        let cwd = PathBuf::from(r"C:\data");
        assert_eq!(
            change_dir_command(&PathBuf::from(r"D:\Program Files\x"), Some(&cwd)),
            r"cd 'D:\Program Files\x'"
        );
        assert_eq!(change_dir_command(&PathBuf::from(r"D:\x"), None), r"cd D:\x");
    }

    #[test]
    fn single_line_joins_statements_and_continuations() {
        assert_eq!(to_single_line("cd src\nnpm install"), "cd src; npm install");
        assert_eq!(
            to_single_line("Get-ChildItem |\n  Sort-Object Length"),
            "Get-ChildItem | Sort-Object Length"
        );
        assert_eq!(
            to_single_line("Get-Process | ForEach-Object {\n $_.Name\n}"),
            "Get-Process | ForEach-Object { $_.Name }"
        );
        assert_eq!(to_single_line("Get-Item x `\n  -Force"), "Get-Item x -Force");
    }

    #[test]
    fn lone_carriage_return_is_a_line_break() {
        assert_eq!(to_single_line("echo a\recho b"), "echo a; echo b");
    }

    #[test]
    fn control_characters_are_detected() {
        assert_eq!(first_control_char("cd 'ok'"), None);
        assert_eq!(first_control_char("cd 'a\u{7f}\u{7f}'"), Some('\u{7f}'));
        assert_eq!(first_control_char("x\u{1b}[A"), Some('\u{1b}'));
        assert_eq!(first_control_char("x\u{9b}"), Some('\u{9b}'));
    }
}
