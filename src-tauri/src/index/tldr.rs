//! Command examples from tldr-pages (https://tldr.sh, CC BY 4.0), bundled as
//! `resources/tldr/pages.md` by `scripts/fetch-assets.ps1`.
//!
//! Pages are short "how do I…" examples for thousands of tools. They are
//! converted into the same `<placeholder>` templates as the curated catalog,
//! so values the user fills in are quoted the same safe way.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::{Captures, Regex};

use super::catalog::Param;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Platform-independent pages, which describe the Unix flavour of a tool.
    Common,
    Windows,
}

#[derive(Debug, Clone)]
pub struct TldrExample {
    pub title: String,
    /// Command template with `<placeholder>` slots.
    pub command: String,
    pub params: Vec<Param>,
}

#[derive(Debug, Clone)]
pub struct TldrPage {
    /// As written in the page title, e.g. "git commit" or "Get-ChildItem".
    pub name: String,
    pub platform: Platform,
    pub description: String,
    /// The package the command belongs to, from a "Part of `git-extras`" note.
    pub part_of: Option<String>,
    pub examples: Vec<TldrExample>,
}

impl TldrPage {
    /// The program or cmdlet the page is about: the first word of its title.
    pub fn program(&self) -> String {
        self.name.split_whitespace().next().unwrap_or("").to_lowercase()
    }

    /// For a subcommand that comes from an add-on named after its program
    /// (e.g. `git magic` from git-extras), the executable the add-on installs
    /// for it (`git-magic`). Such pages only apply when the add-on is there.
    pub fn add_on_executable(&self) -> Option<String> {
        let (program, subcommand) = self.name.split_once(' ')?;
        let package = self.part_of.as_deref()?.to_lowercase();
        let program = program.to_lowercase();
        if !package.starts_with(&format!("{program}-")) {
            return None;
        }
        let subcommand = subcommand.split_whitespace().next()?.to_lowercase();
        Some(format!("{program}-{subcommand}"))
    }
}

const PAGE_MARKER: &str = "%%% ";

/// Parses the bundle: pages separated by `%%% <platform>/<file>.md` lines.
pub fn parse_bundle(text: &str) -> Vec<TldrPage> {
    let mut pages = Vec::new();
    for chunk in text.split(PAGE_MARKER).filter(|c| !c.trim().is_empty()) {
        let (header, markdown) = chunk.split_once('\n').unwrap_or((chunk, ""));
        let platform = match header.split('/').next() {
            Some("common") => Platform::Common,
            Some("windows") => Platform::Windows,
            _ => continue,
        };
        if let Some(page) = parse_page(platform, markdown) {
            pages.push(page);
        }
    }
    pages
}

fn parse_page(platform: Platform, markdown: &str) -> Option<TldrPage> {
    let mut name = None;
    let mut description = Vec::new();
    let mut part_of = None;
    let mut examples = Vec::new();
    let mut pending_title: Option<String> = None;
    for line in markdown.lines().map(str::trim) {
        if let Some(title) = line.strip_prefix("# ") {
            name = Some(title.trim().to_string());
        } else if let Some(text) = line.strip_prefix("> ") {
            if let Some(package) = text.strip_prefix("Part of ") {
                part_of = Some(package.trim().trim_end_matches('.').trim_matches('`').to_string());
            }
            if !text.starts_with("More information:") && !text.starts_with("See also:") {
                description.push(text.trim().to_string());
            }
        } else if let Some(title) = line.strip_prefix("- ") {
            pending_title = Some(title.trim().trim_end_matches(':').to_string());
        } else if line.len() > 2
            && line.starts_with('`')
            && line.ends_with('`')
            && let Some(title) = pending_title.take()
            // Examples that can't be filled in safely are left out.
            && let Some((command, params)) = convert_command(&line[1..line.len() - 1])
        {
            examples.push(TldrExample { title: clean_markup(&title), command, params });
        }
    }
    Some(TldrPage { name: name?, platform, description: clean_markup(&description.join(" ")), part_of, examples })
}

/// Removes markdown backticks and tldr's `[c]reate`-style mnemonic brackets.
fn clean_markup(text: &str) -> String {
    static MNEMONIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([A-Za-z0-9])\]").expect("valid regex"));
    MNEMONIC.replace_all(&text.replace('`', ""), "$1").into_owned()
}

/// A placeholder name the template filler accepts: lower-case letters,
/// digits and hyphens.
fn slug(text: &str) -> String {
    let mut slug = String::new();
    for c in text.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(32).collect();
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() { "value".to_string() } else { slug }
}

/// Converts tldr placeholder syntax into a `<placeholder>` template:
/// - `{{[-m|--message]}}` (one option, short or long) becomes the long form,
///   which is easier to read;
/// - `"{{message}}"` loses its quotes, because VCE quotes values itself;
/// - any other `{{text}}` becomes `<text-as-slug>`, described by `text`;
/// - tldr's escaped braces `\{\{ \}\}` become literal `{{ }}`.
///
/// Returns `None` for an example that can't be filled in safely (see
/// `is_safe_to_fill`).
pub fn convert_command(raw: &str) -> Option<(String, Vec<Param>)> {
    static OPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{\[([^\]]*)\]\}\}").expect("valid regex"));
    static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""(\{\{[^}]*\}\})"|'(\{\{[^}]*\}\})'"#).expect("valid regex"));
    static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{(.*?)\}\}").expect("valid regex"));

    let with_options = OPTION.replace_all(raw, |caps: &Captures| caps[1].split('|').next_back().unwrap_or("").trim().to_string());
    let unquoted = QUOTED.replace_all(&with_options, |caps: &Captures| {
        caps.get(1).or_else(|| caps.get(2)).map_or(String::new(), |m| m.as_str().to_string())
    });
    if !is_safe_to_fill(&unquoted) {
        return None;
    }
    let mut params: Vec<Param> = Vec::new();
    let mut seen = HashSet::new();
    let command = PLACEHOLDER
        .replace_all(&unquoted, |caps: &Captures| {
            let text = caps[1].trim();
            let name = slug(text);
            if seen.insert(name.clone()) {
                params.push(Param { name: name.clone(), description: text.to_string() });
            }
            format!("<{name}>")
        })
        .replace(r"\{\{", "{{")
        .replace(r"\}\}", "}}");
    Some((command, params))
}

/// Whether every `{{placeholder}}` stands outside quotes and nothing outside
/// quotes is a `<` redirection. A filled-in value is quoted on its own, so a
/// placeholder inside a longer quoted string (`'--timeout={{seconds}}'`)
/// would end those quotes early and run the rest of the value as code; and
/// PowerShell rejects `<`, which it reserves for future use.
fn is_safe_to_fill(command: &str) -> bool {
    let mut quote = None;
    let mut rest = command;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with("{{") {
            if quote.is_some() {
                return false;
            }
            // Skip the placeholder: its text describes the value, it isn't syntax.
            let Some(end) = rest.find("}}") else {
                return false;
            };
            rest = &rest[end + 2..];
            continue;
        }
        match (quote, c) {
            (None, '\'' | '"') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '<') => return false,
            _ => {}
        }
        rest = &rest[c.len_utf8()..];
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLE: &str = "%%% common/git-commit.md\n# git commit\n\n> Commit files to the repository.\n> More information: <https://git-scm.com/docs/git-commit>.\n\n- Commit staged files with the specified message:\n\n`git commit {{[-m|--message]}} \"{{message}}\"`\n\n- [a]mend the last commit:\n\n`git commit --amend`\n%%% windows/dir.md\n# dir\n\n> List directory contents.\n\n- Show hidden files:\n\n`dir /a`\n%%% linux/apt.md\n# apt\n\n> Package manager.\n";

    #[test]
    fn parses_pages_and_skips_other_platforms() {
        let pages = parse_bundle(BUNDLE);
        assert_eq!(pages.len(), 2);
        let git = &pages[0];
        assert_eq!(git.name, "git commit");
        assert_eq!(git.program(), "git");
        assert_eq!(git.platform, Platform::Common);
        assert_eq!(git.description, "Commit files to the repository.");
        assert_eq!(git.examples[0].command, "git commit --message <message>");
        assert_eq!(git.examples[1].title, "amend the last commit");
        assert_eq!(pages[1].platform, Platform::Windows);
    }

    #[test]
    fn add_on_subcommands_name_their_own_executable() {
        let page =
            |name: &str, note: &str| parse_bundle(&format!("%%% common/x.md\n# {name}\n\n> Does something.\n> {note}\n")).pop().unwrap();
        assert_eq!(page("git magic", "Part of `git-extras`.").add_on_executable().as_deref(), Some("git-magic"));
        // The Azure CLI's own subcommands come with `az` itself.
        assert_eq!(page("az advisor", "Part of `azure-cli` (also known as `az`).").add_on_executable(), None);
        assert_eq!(page("git commit", "More information: <https://git-scm.com>.").add_on_executable(), None);
    }

    #[test]
    fn converts_placeholders_into_safe_templates() {
        let (command, params) = convert_command(r"robocopy {{path\to\source}} {{path\to\destination}} {{*.jpg}} /E").unwrap();
        assert_eq!(command, "robocopy <path-to-source> <path-to-destination> <jpg> /E");
        assert_eq!(params[0].description, r"path\to\source");
        let (command, params) = convert_command("tar czf {{path/to/target.tar.gz}} {{path/to/target.tar.gz}}").unwrap();
        assert_eq!(command, "tar czf <path-to-target-tar-gz> <path-to-target-tar-gz>");
        assert_eq!(params.len(), 1, "a repeated placeholder is one value");
        let (command, _) = convert_command("driverquery /fo {{table|list|csv}}").unwrap();
        assert_eq!(command, "driverquery /fo <table-list-csv>");
    }

    #[test]
    fn refuses_examples_that_cannot_be_filled_safely() {
        // A value like `1; calc` would close the outer quotes and run calc.
        assert!(convert_command("git config credential.helper 'cache --timeout={{time_in_seconds}}'").is_none());
        assert!(convert_command(r#"Resolve-Path -Path "\\{{hostname}}\{{path\to\file}}""#).is_none());
        // PowerShell reserves `<`, so input redirection can't run.
        assert!(convert_command("git < {{path/to/file_list}} check-ignore --stdin").is_none());
        // Quotes wrapping just the placeholder, and `<` inside quotes, are fine.
        assert_eq!(convert_command(r#"git commit -m "{{message}}""#).unwrap().0, "git commit -m <message>");
        assert_eq!(convert_command("Select-String '<title>' {{path}}").unwrap().0, "Select-String '<title>' <path>");
    }

    #[test]
    fn escaped_braces_become_literal() {
        let (command, _) = convert_command(r"docker inspect --format '\{\{.Name\}\}' {{container}}").unwrap();
        assert_eq!(command, "docker inspect --format '{{.Name}}' <container>");
    }

    #[test]
    fn placeholder_names_suit_the_template_filler() {
        let pattern = regex::Regex::new(r"^[a-z0-9-]+$").unwrap();
        for text in ["!!!", "path/to/file1 path/to/file2 ...", "Very Long Placeholder Name That Goes On And On", "key=value"] {
            assert!(pattern.is_match(&slug(text)), "{text:?} -> {:?}", slug(text));
        }
    }
}
