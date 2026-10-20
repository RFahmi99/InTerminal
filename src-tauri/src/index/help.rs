//! One-line descriptions for cmdlets, read straight from PowerShell's
//! installed help files (`*-help.xml`, MAML format). Calling `Get-Help`
//! instead takes ~0.2 s per command; reading the files takes milliseconds.
//!
//! Windows PowerShell ships with few help files until `Update-Help` runs,
//! so this often finds little until the user installs help.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// Module folders nest help as `<module>\<version>\<culture>\x-help.xml`.
const MAX_DEPTH: usize = 4;
const MAX_SYNOPSIS_CHARS: usize = 300;

fn find_help_files(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // Missing module roots are normal.
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < MAX_DEPTH {
                find_help_files(&path, depth + 1, found);
            }
        } else if path.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().ends_with("-help.xml")) {
            found.push(path);
        }
    }
}

fn decode_entities(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// `(command name, synopsis)` pairs from one MAML help file.
pub fn parse_maml(xml: &str) -> Vec<(String, String)> {
    static DETAILS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<command:details>(.*?)</command:details>").expect("valid regex"));
    static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<command:name>(.*?)</command:name>").expect("valid regex"));
    static DESCRIPTION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)<maml:description>(.*?)</maml:description>").expect("valid regex"));
    static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").expect("valid regex"));

    DETAILS
        .captures_iter(xml)
        .filter_map(|details| {
            let block = &details[1];
            let name = decode_entities(NAME.captures(block)?[1].trim());
            let text = TAG.replace_all(&DESCRIPTION.captures(block)?[1], " ").into_owned();
            let synopsis = decode_entities(&text.split_whitespace().collect::<Vec<_>>().join(" "));
            let synopsis: String = synopsis.chars().take(MAX_SYNOPSIS_CHARS).collect();
            (!name.is_empty() && !synopsis.is_empty()).then_some((name, synopsis))
        })
        .collect()
}

/// Lower-case command name to synopsis, from every help file under `roots`.
/// The first root that describes a command wins.
pub fn load_synopses(roots: &[PathBuf]) -> HashMap<String, String> {
    let mut files = Vec::new();
    for root in roots {
        find_help_files(root, 0, &mut files);
    }
    let mut synopses = HashMap::new();
    for file in files {
        match std::fs::read_to_string(&file) {
            Ok(xml) => {
                for (name, synopsis) in parse_maml(&xml) {
                    synopses.entry(name.to_lowercase()).or_insert(synopsis);
                }
            }
            Err(e) => log::debug!("Skipping unreadable help file {}: {e}", file.display()),
        }
    }
    synopses
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAML: &str = r#"<helpItems schema="maml">
<command:command><command:details><command:name>Expand-Archive</command:name>
<maml:description><maml:para>Extracts files from a specified archive (zipped) file.
</maml:para></maml:description></command:details>
<maml:description><maml:para>The long description that should be ignored.</maml:para></maml:description>
</command:command>
<command:command><command:details><command:name>Get-Thing</command:name>
<maml:description><maml:para>Gets &lt;things&gt; &amp; more.</maml:para></maml:description></command:details></command:command>
</helpItems>"#;

    #[test]
    fn reads_synopses_from_maml() {
        let parsed = parse_maml(MAML);
        assert_eq!(parsed[0], ("Expand-Archive".into(), "Extracts files from a specified archive (zipped) file.".into()));
        assert_eq!(parsed[1].1, "Gets <things> & more.");
    }

    #[test]
    fn loads_help_files_from_module_folders() {
        let dir = tempfile::tempdir().unwrap();
        let culture = dir.path().join("Microsoft.PowerShell.Archive").join("1.0.1.0").join("en-US");
        std::fs::create_dir_all(&culture).unwrap();
        std::fs::write(culture.join("Microsoft.PowerShell.Archive-help.xml"), MAML).unwrap();
        let synopses = load_synopses(&[dir.path().to_path_buf(), dir.path().join("missing")]);
        assert_eq!(synopses["expand-archive"], "Extracts files from a specified archive (zipped) file.");
    }
}
