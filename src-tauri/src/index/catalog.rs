//! The curated command catalog bundled as `resources/catalog.json`: plain
//! English descriptions, search keywords and parameter help for common tasks.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::assist::risk::RiskLevel;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Param {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub id: String,
    pub title: String,
    /// Command template; user-supplied values appear as `<name>` placeholders.
    pub command: String,
    pub description: String,
    pub keywords: Vec<String>,
    #[serde(default)]
    pub params: Vec<Param>,
    #[serde(default)]
    pub example: Option<String>,
    /// Commands that must be installed for this entry to apply.
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub category: Option<String>,
    pub risk: RiskLevel,
}

/// A program people commonly try to run, with how to install it.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub name: String,
    pub winget: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Catalog {
    pub version: u32,
    pub entries: Vec<CatalogEntry>,
    #[serde(default)]
    pub tools: Vec<ToolInfo>,
}

impl Catalog {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))
    }

    pub fn tool(&self, name: &str) -> Option<&ToolInfo> {
        self.tools.iter().find(|tool| tool.name.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_is_valid() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources").join("catalog.json");
        let catalog = Catalog::load(&path).expect("bundled catalog parses");
        assert!(catalog.entries.len() >= 100, "catalog has only {} entries", catalog.entries.len());
        let placeholder = regex::Regex::new(r"<([a-z0-9-]+)>").unwrap();
        for entry in &catalog.entries {
            let in_command: Vec<&str> = placeholder.captures_iter(&entry.command).map(|c| c.get(1).unwrap().as_str()).collect();
            for name in &in_command {
                assert!(entry.params.iter().any(|p| p.name == *name), "{}: <{name}> has no param", entry.id);
            }
            for param in &entry.params {
                assert!(in_command.contains(&param.name.as_str()), "{}: param {} unused", entry.id, param.name);
            }
        }
    }
}
