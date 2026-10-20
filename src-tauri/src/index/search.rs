//! Natural-language command search over the curated catalog, PowerShell
//! cmdlets and programs on PATH, limited to what is installed.
//!
//! Ranking blends semantic similarity (static embeddings) with keyword
//! overlap, so both "unzip" and "decompress an archive" find
//! `Expand-Archive`. Without the embedding model it falls back to keywords.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use regex::Regex;
use serde::Serialize;

use super::catalog::{Catalog, CatalogEntry, Param};
use super::embedding::{CompactVector, StaticEmbedder};
use super::fuzzy;
use super::inventory::{Inventory, PsCommandKind};
use super::tldr::{Platform, TldrPage};
use crate::assist::risk::{self, RiskLevel, RiskReport};

const SEMANTIC_WEIGHT: f32 = 0.6;
const LEXICAL_WEIGHT: f32 = 0.4;
const MIN_SCORE: f32 = 0.32;
/// How much the description vector counts relative to title and keywords,
/// which are written to match how people search.
const DESCRIPTION_WEIGHT: f32 = 0.9;
/// Maximum score deducted for a long command (reached at 100 characters).
const COMPLEXITY_WEIGHT: f32 = 0.04;

const STOPWORDS: &[&str] = &[
    "a", "an", "the", "to", "of", "for", "in", "on", "my", "all", "with", "and", "or", "from", "into", "how", "do", "i", "me", "what",
    "is", "are", "this", "that", "it", "by", "at", "be", "can", "which", "using", "use", "please", "want", "need", "some", "its", "your",
    "you", "there", "via",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HitKind {
    /// A curated catalog entry with a description and parameter help.
    Recipe,
    /// A PowerShell cmdlet or function not described in the catalog.
    Cmdlet,
    /// A program found on PATH.
    Program,
    /// A worked example from tldr-pages for an installed tool.
    Example,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub id: String,
    pub kind: HitKind,
    pub title: String,
    pub command: String,
    pub description: String,
    pub params: Vec<Param>,
    pub example: Option<String>,
    pub category: Option<String>,
    /// For cmdlets: the parameters of the default parameter set.
    pub usage: Option<String>,
    pub risk: RiskReport,
    pub score: f32,
}

/// Descriptions and examples beyond the curated catalog.
#[derive(Clone, Default)]
pub struct Knowledge {
    /// Lower-case cmdlet name to the synopsis from its installed help file.
    pub synopses: Arc<HashMap<String, String>>,
    pub tldr: Arc<Vec<TldrPage>>,
}

enum DocSource {
    Catalog(usize),
    /// Installing a program from the catalog's tool list with winget.
    Tool(usize),
    Cmdlet(String),
    Program {
        name: String,
        path: PathBuf,
    },
    /// Example `example` of tldr page `page`, written as it must be typed here.
    Tldr {
        page: usize,
        example: usize,
        command: String,
    },
}

/// How a tldr page's commands have to be typed in PowerShell.
#[derive(Debug, PartialEq)]
enum Invocation {
    AsIs,
    /// A PowerShell alias or function shadows the program (e.g. `curl` is an
    /// alias for Invoke-WebRequest in Windows PowerShell), so the command
    /// must name the executable file, e.g. `curl.exe`.
    Executable(String),
}

/// Unix tools Microsoft ships in System32 that behave like the originals, so
/// tldr's platform-independent pages describe them correctly. Other System32
/// programs (ping, sort, find…) share a name with a Unix tool but not its options.
const CROSS_PLATFORM_SYSTEM_TOOLS: &[&str] = &["curl", "tar", "ssh", "scp", "sftp", "ssh-keygen", "ssh-add", "ssh-agent", "ssh-keyscan"];

/// Syntax that only works in Unix shells, or needs administrator tools that
/// don't exist on Windows.
/// (`<` redirection is refused earlier, when the page is parsed.)
const UNIX_ONLY_SYNTAX: &[&str] = &["sudo ", "&&", "||", "$(", "/dev/", "~/"];

/// Search terms interned to ids, so a document stores four bytes per term
/// rather than its own copy of each string.
#[derive(Default)]
struct TermTable {
    ids: HashMap<Box<str>, u32>,
}

impl TermTable {
    /// Sorted, de-duplicated ids for `terms`, adding any that are new.
    fn intern(&mut self, terms: Vec<String>) -> Box<[u32]> {
        let mut ids: Vec<u32> = terms
            .into_iter()
            .map(|term| {
                let next = self.ids.len() as u32;
                *self.ids.entry(term.into_boxed_str()).or_insert(next)
            })
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids.into_boxed_slice()
    }

    fn len(&self) -> usize {
        self.ids.len()
    }

    fn id(&self, term: &str) -> Option<u32> {
        self.ids.get(term).copied()
    }

    /// Sorted ids of the terms that start with `prefix`.
    fn ids_with_prefix(&self, prefix: &str) -> Vec<u32> {
        let mut ids: Vec<u32> = self.ids.iter().filter(|(term, _)| term.starts_with(prefix)).map(|(_, &id)| id).collect();
        ids.sort_unstable();
        ids
    }
}

/// A query term with what scoring needs, looked up once per search.
struct QueryTerm {
    id: Option<u32>,
    /// Ids of the indexed terms it is a prefix of (for terms of three or
    /// more characters), so "proc" partly matches "process".
    prefixed: Vec<u32>,
    /// Inverse document frequency.
    weight: f32,
}

struct Doc {
    source: DocSource,
    /// (weight, normalised vector) pairs; the best match counts.
    vectors: Vec<(f32, CompactVector)>,
    /// Sorted term ids of the stemmed title, keywords and command name.
    primary_terms: Box<[u32]>,
    /// Sorted term ids of the stemmed description and full command.
    secondary_terms: Box<[u32]>,
    /// Lower-case command names this document is about.
    names: Vec<String>,
    /// Source prior: curated entries outrank bare names.
    prior: f32,
    /// Small deduction for long commands, so among near-equal matches a
    /// beginner sees the simplest command first.
    complexity: f32,
}

pub struct CommandIndex {
    catalog: Catalog,
    inventory: Inventory,
    embedder: Option<Arc<StaticEmbedder>>,
    knowledge: Knowledge,
    /// Program or cmdlet name to the tldr page describing it (Windows pages
    /// preferred), for descriptions of bare cmdlet and program results.
    tldr_by_program: HashMap<String, usize>,
    docs: Vec<Doc>,
    terms: TermTable,
    /// Number of documents containing each term, by term id, so common words
    /// like "file" count for less than distinctive ones like "unzip".
    doc_freq: Vec<u32>,
    /// Names people commonly mean, used to break ties in typo suggestions.
    popular: HashSet<String>,
}

static VERB_NOUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z]+-[A-Za-z0-9]+$").expect("valid regex"));

/// Light English stemming so "files"/"file" and "processes"/"process" meet.
fn stem(word: &str) -> String {
    let w = word.to_lowercase();
    let n = w.len();
    if n > 4 && w.ends_with("ies") {
        return format!("{}y", &w[..n - 3]);
    }
    if n > 4 && ["sses", "xes", "zes", "ches", "shes"].iter().any(|s| w.ends_with(s)) {
        return w[..n - 2].to_string();
    }
    if n > 5 && w.ends_with("ing") {
        return w[..n - 3].to_string();
    }
    if n > 4 && w.ends_with("ed") {
        return w[..n - 2].to_string();
    }
    if n > 3 && w.ends_with('s') && !w.ends_with("ss") {
        return w[..n - 1].to_string();
    }
    w
}

/// Splits text into stemmed search terms, dropping stopwords. Hyphenated
/// command names contribute both the whole name and its parts.
fn terms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '.')) {
        let raw = raw.trim_matches(|c| c == '-' || c == '.');
        if raw.is_empty() {
            continue;
        }
        let lower = raw.to_lowercase();
        if STOPWORDS.contains(&lower.as_str()) {
            continue;
        }
        if lower.contains('-') {
            out.push(lower.clone());
            out.extend(lower.split('-').filter(|p| !p.is_empty()).map(stem));
        } else {
            out.push(stem(&lower));
        }
    }
    out
}

/// "Get-NetTCPConnection" -> "get net tcp connection".
fn split_identifier(name: &str) -> String {
    let mut words = Vec::new();
    for part in name.split(['-', '_']) {
        let chars: Vec<char> = part.chars().collect();
        let mut current = String::new();
        for (i, &c) in chars.iter().enumerate() {
            let boundary =
                i > 0 && c.is_uppercase() && (chars[i - 1].is_lowercase() || chars.get(i + 1).is_some_and(|next| next.is_lowercase()));
            if boundary && !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            current.push(c);
        }
        if !current.is_empty() {
            words.push(current);
        }
    }
    words.join(" ").to_lowercase()
}

fn first_word(command: &str) -> String {
    command.split_whitespace().next().unwrap_or("").to_lowercase()
}

fn is_windows_system_file(path: &std::path::Path) -> bool {
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()).to_lowercase();
    let path = path.to_string_lossy().to_lowercase();
    path.starts_with(&format!(r"{system_root}\system32\")) || path.starts_with(&format!(r"{system_root}\syswow64\"))
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

impl CommandIndex {
    #[cfg(test)]
    pub fn build(catalog: Catalog, inventory: Inventory, embedder: Option<Arc<StaticEmbedder>>) -> Self {
        Self::build_with(catalog, inventory, embedder, Knowledge::default())
    }

    pub fn build_with(catalog: Catalog, inventory: Inventory, embedder: Option<Arc<StaticEmbedder>>, knowledge: Knowledge) -> Self {
        let mut index = Self {
            catalog,
            inventory,
            embedder,
            knowledge,
            tldr_by_program: HashMap::new(),
            docs: Vec::new(),
            terms: TermTable::default(),
            doc_freq: Vec::new(),
            popular: HashSet::new(),
        };
        index.tldr_by_program = index.describing_pages();
        index.popular = index
            .catalog
            .entries
            .iter()
            .flat_map(|e| e.requires.iter().map(|r| r.to_lowercase()))
            .chain(index.catalog.tools.iter().map(|t| t.name.to_lowercase()))
            .collect();
        let mut terms = TermTable::default();
        index.docs = index.build_docs(&mut terms);
        index.doc_freq = vec![0; terms.len()];
        for doc in &index.docs {
            let secondary_only = doc.secondary_terms.iter().filter(|id| doc.primary_terms.binary_search(id).is_err());
            for &id in doc.primary_terms.iter().chain(secondary_only) {
                index.doc_freq[id as usize] += 1;
            }
        }
        index.terms = terms;
        index
    }

    fn query_term(&self, term: &str) -> QueryTerm {
        let id = self.terms.id(term);
        let df = id.map_or(0, |id| self.doc_freq[id as usize]) as f32;
        QueryTerm {
            id,
            prefixed: if term.len() >= 3 { self.terms.ids_with_prefix(term) } else { Vec::new() },
            weight: (1.0 + self.docs.len() as f32 / (1.0 + df)).ln(),
        }
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    pub fn has_semantic(&self) -> bool {
        self.embedder.is_some()
    }

    pub fn document_count(&self) -> usize {
        self.docs.len()
    }

    /// How many cmdlets have a description from an installed help file.
    pub fn help_description_count(&self) -> usize {
        self.knowledge.synopses.len()
    }

    /// How many tldr-pages examples apply on this machine.
    pub fn example_count(&self) -> usize {
        self.docs.iter().filter(|d| matches!(d.source, DocSource::Tldr { .. })).count()
    }

    /// Whether everything `entry` needs is installed. Before the PowerShell
    /// command list is known, cmdlet names (which contain a hyphen) are
    /// assumed present rather than hiding most of the catalog.
    fn is_available(&self, entry: &CatalogEntry) -> bool {
        let cmdlets_unknown = self.inventory.ps_commands.is_empty();
        entry.requires.iter().all(|name| self.inventory.has(name) || (cmdlets_unknown && name.contains('-')))
    }

    /// For each program or cmdlet, the tldr page that applies here, preferring
    /// Windows pages over platform-independent ones.
    fn describing_pages(&self) -> HashMap<String, usize> {
        let mut pages = HashMap::new();
        for (i, page) in self.knowledge.tldr.iter().enumerate() {
            // Subcommand pages ("git commit") don't describe the program as a whole.
            if page.name.contains(' ') || self.tldr_invocation(page).is_none() {
                continue;
            }
            let program = page.program();
            let replace = pages.get(&program).is_none_or(|&j: &usize| self.knowledge.tldr[j].platform != Platform::Windows);
            if replace {
                pages.insert(program, i);
            }
        }
        pages
    }

    /// Whether a tldr page applies on this machine, and how its commands must
    /// be typed. Windows pages cover cmdlets and Windows programs; the
    /// platform-independent pages describe Unix tools, so they only apply to
    /// programs installed on PATH that aren't Windows' own namesakes. Pages
    /// for cmd.exe built-ins (`dir /a`) never apply: PowerShell's aliases of
    /// the same name take different options.
    fn tldr_invocation(&self, page: &TldrPage) -> Option<Invocation> {
        let program = page.program();
        if page.add_on_executable().is_some_and(|add_on| !self.inventory.has(&add_on)) {
            return None;
        }
        let ps_command = self.inventory.ps_commands.get(&program);
        if page.platform == Platform::Windows && ps_command.is_some_and(|c| c.kind != PsCommandKind::Alias) {
            return Some(Invocation::AsIs);
        }
        let executable = self.inventory.executables.get(&program)?;
        if page.platform == Platform::Common
            && is_windows_system_file(executable)
            && !CROSS_PLATFORM_SYSTEM_TOOLS.contains(&program.as_str())
        {
            return None;
        }
        // Until PowerShell's command list is known, any program might be
        // shadowed by an alias, and naming the file works either way.
        if ps_command.is_some() || self.inventory.ps_commands.is_empty() {
            Some(Invocation::Executable(executable.file_name()?.to_string_lossy().into_owned()))
        } else {
            Some(Invocation::AsIs)
        }
    }

    /// Whether `name`, piped into by an example from a `platform` page, runs
    /// what the page means. Platform-independent pages pipe into Unix tools,
    /// so an alias (`sort` is Sort-Object) or Windows' namesake program
    /// (sort.exe) would get options it doesn't understand.
    fn runs_as_written(&self, name: &str, platform: Platform) -> bool {
        if let Some(stem) = name.strip_suffix(".exe") {
            return self.inventory.executables.contains_key(stem);
        }
        if let Some(command) = self.inventory.ps_commands.get(name) {
            return command.kind != PsCommandKind::Alias || platform == Platform::Windows;
        }
        self.inventory.executables.get(name).is_some_and(|path| {
            platform == Platform::Windows || !is_windows_system_file(path) || CROSS_PLATFORM_SYSTEM_TOOLS.contains(&name)
        })
    }

    /// The example as it must be typed here, or `None` if it can't work in
    /// PowerShell on this machine.
    fn adapt_example(&self, program: &str, command: &str, invocation: &Invocation, platform: Platform) -> Option<String> {
        if UNIX_ONLY_SYNTAX.iter().any(|unix| command.contains(unix)) {
            return None;
        }
        for stage in command.split('|').skip(1) {
            let word = first_word(stage);
            if word.is_empty() || !self.runs_as_written(&word, platform) {
                return None;
            }
        }
        let (first, rest) = command.split_once(' ').unwrap_or((command, ""));
        if !first.eq_ignore_ascii_case(program) {
            return None;
        }
        Some(match invocation {
            Invocation::AsIs => command.to_string(),
            Invocation::Executable(file) if rest.is_empty() => file.clone(),
            Invocation::Executable(file) => format!("{file} {rest}"),
        })
    }

    fn tldr_description(&self, name: &str) -> Option<&str> {
        self.tldr_by_program.get(name).map(|&i| self.knowledge.tldr[i].description.as_str()).filter(|d| !d.is_empty())
    }

    /// The description shown for a bare cmdlet: its help synopsis, else the
    /// tldr page's description.
    fn cmdlet_description(&self, lower_name: &str) -> Option<String> {
        self.knowledge.synopses.get(lower_name).cloned().or_else(|| self.tldr_description(lower_name).map(str::to_string))
    }

    fn embed_all(&self, texts: &[(f32, String)]) -> Vec<(f32, CompactVector)> {
        let Some(embedder) = &self.embedder else {
            return Vec::new();
        };
        texts.iter().filter_map(|(weight, text)| embedder.embed(text).map(|v| (*weight, CompactVector::new(&v)))).collect()
    }

    fn build_docs(&self, table: &mut TermTable) -> Vec<Doc> {
        let mut docs = Vec::new();
        let mut covered_names = HashSet::new();
        for (i, entry) in self.catalog.entries.iter().enumerate() {
            if !self.is_available(entry) {
                continue;
            }
            let mut texts = vec![(1.0, entry.title.clone()), (DESCRIPTION_WEIGHT, entry.description.clone())];
            texts.extend(entry.keywords.iter().map(|k| (1.0, k.clone())));
            let names: Vec<String> = entry.requires.iter().map(|r| r.to_lowercase()).chain([first_word(&entry.command)]).collect();
            covered_names.extend(names.iter().cloned());
            docs.push(Doc {
                source: DocSource::Catalog(i),
                vectors: self.embed_all(&texts),
                primary_terms: table.intern(terms(&format!("{} {} {}", entry.title, entry.keywords.join(" "), names.join(" ")))),
                secondary_terms: table.intern(terms(&format!("{} {}", entry.description, entry.command))),
                names,
                prior: 1.0,
                complexity: COMPLEXITY_WEIGHT * (entry.command.len() as f32 / 100.0).min(1.0),
            });
        }

        let mut seen_examples = HashSet::new();
        for (page_index, page) in self.knowledge.tldr.iter().enumerate() {
            let Some(invocation) = self.tldr_invocation(page) else {
                continue;
            };
            let program = page.program();
            // A Windows page for the same command replaces the generic one.
            if page.platform == Platform::Common
                && self.tldr_by_program.get(&program).is_some_and(|&i| self.knowledge.tldr[i].platform == Platform::Windows)
            {
                continue;
            }
            for (example_index, example) in page.examples.iter().enumerate() {
                let Some(command) = self.adapt_example(&program, &example.command, &invocation, page.platform) else {
                    continue;
                };
                if !seen_examples.insert(command.to_lowercase()) {
                    continue;
                }
                docs.push(Doc {
                    source: DocSource::Tldr { page: page_index, example: example_index, command: command.clone() },
                    vectors: self.embed_all(&[(1.0, example.title.clone())]),
                    primary_terms: table.intern(terms(&format!("{} {}", example.title, page.name))),
                    secondary_terms: table.intern(terms(&format!("{} {command}", page.description))),
                    names: vec![program.clone(), page.name.to_lowercase()],
                    // Curated recipes explain more and come first on a tie.
                    prior: 0.9,
                    complexity: COMPLEXITY_WEIGHT * (command.len() as f32 / 100.0).min(1.0),
                });
            }
        }

        if self.inventory.has("winget") {
            for (i, tool) in self.catalog.tools.iter().enumerate() {
                let name = tool.name.to_lowercase();
                let texts: Vec<(f32, String)> = [format!("install {name}"), format!("download {name}"), format!("get {name}")]
                    .into_iter()
                    .map(|t| (1.0, t))
                    .chain([(DESCRIPTION_WEIGHT, format!("install {}", tool.description))])
                    .collect();
                docs.push(Doc {
                    source: DocSource::Tool(i),
                    vectors: self.embed_all(&texts),
                    primary_terms: table.intern(terms(&format!("install setup download {name}"))),
                    secondary_terms: table.intern(terms(&tool.description)),
                    names: vec![format!("install {name}")],
                    // Below the tool's own commands when it is already installed.
                    prior: if self.inventory.has(&name) { 0.6 } else { 0.95 },
                    complexity: 0.0,
                });
            }
        }

        let mut cmdlets: Vec<&str> = self
            .inventory
            .ps_commands
            .values()
            .filter(|c| c.kind != PsCommandKind::Alias && VERB_NOUN.is_match(&c.name))
            .map(|c| c.name.as_str())
            .collect();
        cmdlets.sort_unstable();
        for name in cmdlets {
            let lower = name.to_lowercase();
            if covered_names.contains(&lower) {
                continue;
            }
            let words = split_identifier(name);
            let description = self.cmdlet_description(&lower);
            let usage = self.inventory.ps_commands.get(&lower).and_then(|c| c.usage.clone()).unwrap_or_default();
            let mut texts = vec![(1.0, words.clone())];
            texts.extend(description.iter().map(|d| (DESCRIPTION_WEIGHT, d.clone())));
            docs.push(Doc {
                source: DocSource::Cmdlet(name.to_string()),
                vectors: self.embed_all(&texts),
                primary_terms: table.intern(terms(&format!("{lower} {words}"))),
                secondary_terms: table.intern(terms(&format!("{} {usage}", description.unwrap_or_default()))),
                names: vec![lower],
                prior: 0.8,
                complexity: 0.0,
            });
        }

        let mut programs: Vec<(&String, &PathBuf)> = self.inventory.executables.iter().collect();
        programs.sort_unstable();
        for (name, path) in programs {
            if covered_names.contains(name) {
                continue;
            }
            // A bare executable name says too little for semantic matching;
            // a tldr description, where there is one, says enough.
            let description = self.tldr_description(name).map(str::to_string);
            docs.push(Doc {
                source: DocSource::Program { name: name.clone(), path: path.clone() },
                vectors: description.as_ref().map_or_else(Vec::new, |d| self.embed_all(&[(DESCRIPTION_WEIGHT, d.clone())])),
                primary_terms: table.intern(vec![name.clone()]),
                secondary_terms: table.intern(terms(description.as_deref().unwrap_or_default())),
                names: vec![name.clone()],
                prior: 0.7,
                complexity: 0.0,
            });
        }
        docs
    }

    /// IDF-weighted share of the query terms found in `doc`, in 0..=1.
    fn lexical_score(query_terms: &[QueryTerm], doc: &Doc) -> f32 {
        let total_weight: f32 = query_terms.iter().map(|t| t.weight).sum();
        if total_weight == 0.0 {
            return 0.0;
        }
        let matched: f32 = query_terms
            .iter()
            .map(|term| {
                let in_terms = |ids: &[u32]| term.id.is_some_and(|id| ids.binary_search(&id).is_ok());
                let strength = if in_terms(&doc.primary_terms) {
                    1.0
                } else if in_terms(&doc.secondary_terms) || doc.primary_terms.iter().any(|id| term.prefixed.binary_search(id).is_ok()) {
                    0.5
                } else {
                    0.0
                };
                strength * term.weight
            })
            .sum();
        matched / total_weight
    }

    fn name_bonus(query: &str, doc: &Doc) -> f32 {
        let first = query.split_whitespace().next().unwrap_or("");
        if doc.names.iter().any(|n| n == query) {
            0.5
        } else if query.len() >= 2 && !query.contains(' ') && doc.names.iter().any(|n| n.starts_with(query)) {
            0.25
        } else if doc.names.iter().any(|n| n == first) {
            0.15
        } else {
            0.0
        }
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let query = query.trim();
        if query.is_empty() || limit == 0 {
            return Vec::new();
        }
        let lower = query.to_lowercase();
        let mut words = terms(query);
        if words.is_empty() {
            // The query was all stopwords ("how to"); match on the words anyway.
            words = lower.split_whitespace().map(stem).collect();
        }
        let query_terms: Vec<QueryTerm> = words.iter().map(|t| self.query_term(t)).collect();
        let query_vector = self.embedder.as_ref().and_then(|e| e.embed(query));

        let mut scored: Vec<(f32, &Doc)> = self
            .docs
            .iter()
            .map(|doc| {
                let lexical = Self::lexical_score(&query_terms, doc);
                let relevance = match &query_vector {
                    Some(q) => {
                        let semantic = doc.vectors.iter().map(|(w, v)| w * v.cosine(q)).fold(0.0, f32::max);
                        SEMANTIC_WEIGHT * semantic + LEXICAL_WEIGHT * lexical
                    }
                    None => lexical,
                };
                (doc.prior * relevance + Self::name_bonus(&lower, doc) - doc.complexity, doc)
            })
            .filter(|(score, _)| *score >= MIN_SCORE)
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));

        let mut seen_commands = HashSet::new();
        scored
            .into_iter()
            .map(|(score, doc)| self.to_hit(doc, score))
            .filter(|hit| seen_commands.insert(hit.command.to_lowercase()))
            .take(limit)
            .collect()
    }

    /// `(title, command)` for the curated recipes and tldr examples most
    /// relevant to `query`, used to ground the AI command generator in
    /// commands known to work on this machine.
    pub fn relevant_recipes(&self, query: &str, limit: usize) -> Vec<(String, String)> {
        self.search(query, limit * 3)
            .into_iter()
            .filter(|hit| matches!(hit.kind, HitKind::Recipe | HitKind::Example) && !hit.id.starts_with("tool:"))
            .map(|hit| (hit.title, hit.command))
            .take(limit)
            .collect()
    }

    /// Installed command names closest to a mistyped `name`. The nearest match
    /// is always offered; further ones only if they are commonly used commands,
    /// so "gti" suggests git without also suggesting the obscure alias gci.
    pub fn did_you_mean(&self, name: &str, limit: usize) -> Vec<String> {
        fuzzy::closest(name, self.inventory.names(), &self.popular, limit)
            .into_iter()
            .enumerate()
            .filter(|(i, candidate)| *i == 0 || self.popular.contains(&candidate.to_lowercase()))
            .map(|(_, candidate)| candidate)
            .collect()
    }

    fn to_hit(&self, doc: &Doc, score: f32) -> SearchHit {
        match &doc.source {
            DocSource::Catalog(i) => {
                let entry = &self.catalog.entries[*i];
                SearchHit {
                    id: entry.id.clone(),
                    kind: HitKind::Recipe,
                    title: entry.title.clone(),
                    command: entry.command.clone(),
                    description: entry.description.clone(),
                    params: entry.params.clone(),
                    example: entry.example.clone(),
                    category: entry.category.clone(),
                    usage: None,
                    risk: risk::assess(&entry.command).at_least(entry.risk),
                    score,
                }
            }
            DocSource::Tldr { page, example, command } => {
                let page = &self.knowledge.tldr[*page];
                let example = &page.examples[*example];
                SearchHit {
                    id: format!("tldr:{}:{}", page.name, example.title),
                    kind: HitKind::Example,
                    title: capitalize(&example.title),
                    command: command.clone(),
                    description: page.description.clone(),
                    params: example.params.clone(),
                    example: None,
                    category: None,
                    usage: None,
                    risk: risk::assess(command),
                    score,
                }
            }
            DocSource::Tool(i) => {
                let tool = &self.catalog.tools[*i];
                let command = format!("winget install --id {} -e", tool.winget);
                SearchHit {
                    id: format!("tool:{}", tool.name),
                    kind: HitKind::Recipe,
                    title: format!("Install {}", tool.name),
                    description: format!(
                        "Installs {} ({}) with winget, the Windows package manager. Restart the terminal afterwards so the new command is found.",
                        tool.name, tool.description
                    ),
                    risk: risk::assess(&command),
                    command,
                    params: Vec::new(),
                    example: None,
                    category: Some("packages".into()),
                    usage: None,
                    score,
                }
            }
            DocSource::Cmdlet(name) => {
                let lower = name.to_lowercase();
                let description = self
                    .cmdlet_description(&lower)
                    .unwrap_or_else(|| format!("PowerShell command. Run Get-Help {name} -Examples to see how to use it."));
                SearchHit {
                    id: format!("cmdlet:{name}"),
                    kind: HitKind::Cmdlet,
                    title: name.clone(),
                    command: name.clone(),
                    description,
                    params: Vec::new(),
                    example: None,
                    category: None,
                    usage: self.inventory.ps_commands.get(&lower).and_then(|c| c.usage.clone()),
                    risk: risk::assess(name),
                    score,
                }
            }
            DocSource::Program { name, path } => SearchHit {
                id: format!("program:{name}"),
                kind: HitKind::Program,
                title: name.clone(),
                command: name.clone(),
                description: match self.tldr_description(name) {
                    Some(description) => format!("{description} Installed at {}.", path.display()),
                    None => format!("Program installed at {}. Most programs list their options with {name} --help.", path.display()),
                },
                params: Vec::new(),
                example: None,
                category: None,
                usage: None,
                risk: RiskReport { level: RiskLevel::Safe, reasons: Vec::new() },
                score,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::index::inventory::PsCommand;

    fn entry(id: &str, title: &str, command: &str, keywords: &[&str], requires: &[&str]) -> CatalogEntry {
        CatalogEntry {
            id: id.into(),
            title: title.into(),
            command: command.into(),
            description: format!("{title}."),
            keywords: keywords.iter().map(|k| k.to_string()).collect(),
            params: Vec::new(),
            example: None,
            requires: requires.iter().map(|r| r.to_string()).collect(),
            category: None,
            risk: RiskLevel::Safe,
        }
    }

    fn small_index(embedder: Option<Arc<StaticEmbedder>>) -> CommandIndex {
        let catalog = Catalog {
            version: 1,
            entries: vec![
                entry(
                    "expand-archive",
                    "Extract files from a .zip archive",
                    "Expand-Archive -Path <archive>",
                    &["unzip", "extract", "decompress"],
                    &["Expand-Archive"],
                ),
                entry("git-status", "Show changed files in a Git repo", "git status", &["git changes", "modified files"], &["git"]),
                entry("docker-ps", "List running containers", "docker ps", &["containers"], &["docker"]),
            ],
            tools: Vec::new(),
        };
        let inventory = Inventory::new(
            HashMap::from([
                ("git".to_string(), PathBuf::from(r"C:\Git\git.exe")),
                ("ffmpeg".to_string(), PathBuf::from(r"C:\bin\ffmpeg.exe")),
            ]),
            vec![
                PsCommand { name: "Expand-Archive".into(), kind: PsCommandKind::Function, definition: None, usage: None },
                PsCommand { name: "Get-NetTCPConnection".into(), kind: PsCommandKind::Cmdlet, definition: None, usage: None },
            ],
        );
        CommandIndex::build(catalog, inventory, embedder)
    }

    #[test]
    fn stems_and_splits_terms() {
        assert_eq!(stem("files"), "file");
        assert_eq!(stem("processes"), "process");
        assert_eq!(stem("directories"), "directory");
        assert_eq!(terms("How to unzip the files"), vec!["unzip", "file"]);
        assert_eq!(split_identifier("Get-NetTCPConnection"), "get net tcp connection");
    }

    #[test]
    fn keyword_search_finds_synonyms_without_embeddings() {
        let index = small_index(None);
        let hits = index.search("unzip a file", 5);
        assert_eq!(hits[0].id, "expand-archive");
    }

    #[test]
    fn entries_needing_missing_tools_are_hidden() {
        let index = small_index(None);
        assert!(index.search("list running containers", 5).iter().all(|h| h.id != "docker-ps"));
    }

    #[test]
    fn exact_program_name_ranks_the_program() {
        let index = small_index(None);
        let hits = index.search("ffmpeg", 5);
        assert_eq!(hits[0].kind, HitKind::Program);
        assert!(hits[0].description.contains("ffmpeg.exe"));
    }

    #[test]
    fn catalog_covers_its_own_command_names() {
        let index = small_index(None);
        let hits = index.search("git", 5);
        assert_eq!(hits[0].id, "git-status");
        assert!(hits.iter().all(|h| h.id != "program:git"));
    }

    #[test]
    fn cmdlet_names_are_searchable_by_their_words() {
        let index = small_index(None);
        let hits = index.search("tcp connection", 5);
        assert_eq!(hits[0].id, "cmdlet:Get-NetTCPConnection");
    }

    #[test]
    fn did_you_mean_drops_obscure_runners_up() {
        let catalog = Catalog {
            version: 1,
            entries: vec![entry("git-status", "Show changes", "git status", &["changes"], &["git"])],
            tools: Vec::new(),
        };
        let inventory = Inventory::new(
            HashMap::from([("git".to_string(), PathBuf::from(r"C:\git.exe"))]),
            vec![PsCommand { name: "gci".into(), kind: PsCommandKind::Alias, definition: Some("Get-ChildItem".into()), usage: None }],
        );
        let index = CommandIndex::build(catalog, inventory, None);
        assert_eq!(index.did_you_mean("gti", 3), vec!["git"]);
    }

    #[test]
    fn did_you_mean_uses_installed_names() {
        let index = small_index(None);
        assert_eq!(index.did_you_mean("gti", 3).first().map(String::as_str), Some("git"));
    }

    fn system32(file: &str) -> PathBuf {
        PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join("System32").join(file)
    }

    const PAGES: &str = "%%% common/curl.md\n# curl\n\n> Transfer data from or to a server.\n\n- Download a file:\n\n`curl {{[-O|--remote-name]}} {{https://example.com/file.zip}}`\n\n- Only print the status:\n\n`curl {{url}} | grep HTTP`\n%%% common/ping.md\n# ping\n\n> Send ICMP ECHO_REQUEST packets.\n\n- Ping a host only a number of times:\n\n`ping -c {{count}} {{host}}`\n%%% windows/dir.md\n# dir\n\n> List directory contents.\n\n- Show hidden files:\n\n`dir /a`\n%%% windows/get-childitem.md\n# Get-ChildItem\n\n> List items in a directory.\n\n- List only directories:\n\n`Get-ChildItem -Directory`\n%%% common/git-magic.md\n# git magic\n\n> Commit changes with a generated message.\n> Part of `git-extras`.\n\n- Commit with a generated message:\n\n`git magic`\n%%% common/git-commit.md\n# git commit\n\n> Commit files to the repository.\n\n- Commit staged files with a message:\n\n`git commit {{[-m|--message]}} \"{{message}}\"`\n\n- Commit and push:\n\n`git commit && git push`\n";

    fn knowledge_index() -> CommandIndex {
        let inventory = Inventory::new(
            HashMap::from([
                ("curl".to_string(), system32("curl.exe")),
                ("ping".to_string(), system32("PING.EXE")),
                ("git".to_string(), PathBuf::from(r"C:\Program Files\Git\cmd\git.exe")),
            ]),
            vec![
                PsCommand { name: "curl".into(), kind: PsCommandKind::Alias, definition: Some("Invoke-WebRequest".into()), usage: None },
                PsCommand { name: "dir".into(), kind: PsCommandKind::Alias, definition: Some("Get-ChildItem".into()), usage: None },
                PsCommand {
                    name: "Get-ChildItem".into(),
                    kind: PsCommandKind::Cmdlet,
                    definition: None,
                    usage: Some("Get-ChildItem [[-Path] <string[]>] [-Recurse]".into()),
                },
                PsCommand { name: "Expand-Archive".into(), kind: PsCommandKind::Function, definition: None, usage: None },
            ],
        );
        let knowledge = Knowledge {
            synopses: Arc::new(HashMap::from([(
                "expand-archive".to_string(),
                "Extracts files from a specified archive (zipped) file.".to_string(),
            )])),
            tldr: Arc::new(super::super::tldr::parse_bundle(PAGES)),
        };
        CommandIndex::build_with(Catalog::default(), inventory, None, knowledge)
    }

    fn example_commands(index: &CommandIndex) -> Vec<String> {
        index
            .docs
            .iter()
            .filter_map(|d| match &d.source {
                DocSource::Tldr { command, .. } => Some(command.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn tldr_examples_apply_only_where_they_work_in_powershell() {
        let index = knowledge_index();
        let commands = example_commands(&index);
        // The curl alias would run Invoke-WebRequest, so name the executable.
        assert!(commands.contains(&"curl.exe --remote-name <https-example-com-file-zip>".to_string()), "{commands:?}");
        assert!(commands.contains(&"Get-ChildItem -Directory".to_string()));
        assert!(commands.contains(&"git commit --message <message>".to_string()));
        // Windows' ping takes different options than the Unix page describes.
        assert!(commands.iter().all(|c| !c.starts_with("ping")));
        // `dir /a` is cmd.exe syntax; in PowerShell dir is Get-ChildItem.
        assert!(commands.iter().all(|c| !c.starts_with("dir")));
        // git-extras isn't installed, so its `git magic` doesn't exist.
        assert!(commands.iter().all(|c| c != "git magic"));
        // Piping into a tool that isn't installed, and Unix-only `&&`.
        assert!(commands.iter().all(|c| !c.contains("grep") && !c.contains("&&")));
    }

    #[test]
    fn tldr_examples_are_searchable_with_their_parameters() {
        let index = knowledge_index();
        let hits = index.search("download a file", 5);
        let hit = hits.iter().find(|h| h.kind == HitKind::Example).expect("an example hit");
        assert_eq!(hit.title, "Download a file");
        assert_eq!(hit.params[0].description, "https://example.com/file.zip");
    }

    #[test]
    fn cmdlets_show_help_synopsis_and_usage() {
        let index = knowledge_index();
        let hit = index.search("expand-archive", 3).into_iter().find(|h| h.kind == HitKind::Cmdlet).unwrap();
        assert_eq!(hit.description, "Extracts files from a specified archive (zipped) file.");
        let hit = index.search("get-childitem", 5).into_iter().find(|h| h.kind == HitKind::Cmdlet).unwrap();
        assert_eq!(hit.usage.as_deref(), Some("Get-ChildItem [[-Path] <string[]>] [-Recurse]"));
        // No help file, so the tldr page describes it.
        assert_eq!(hit.description, "List items in a directory.");
    }

    #[test]
    fn programs_are_described_by_their_tldr_page() {
        let index = knowledge_index();
        let hit = index.search("git", 5).into_iter().find(|h| h.kind == HitKind::Program);
        // "git commit" is a subcommand page, so it doesn't describe git itself.
        assert!(hit.is_some_and(|h| h.description.starts_with("Program installed at")));
        let curl = index.search("curl", 5).into_iter().find(|h| h.kind == HitKind::Program).unwrap();
        assert!(curl.description.starts_with("Transfer data from or to a server."));
    }

    fn index_with_pages(executables: &[(&str, PathBuf)], ps_commands: Vec<PsCommand>, pages: &str) -> CommandIndex {
        let executables = executables.iter().map(|(name, path)| (name.to_string(), path.clone())).collect();
        let knowledge = Knowledge { synopses: Arc::default(), tldr: Arc::new(super::super::tldr::parse_bundle(pages)) };
        CommandIndex::build_with(Catalog::default(), Inventory::new(executables, ps_commands), None, knowledge)
    }

    fn alias(name: &str, definition: &str) -> PsCommand {
        PsCommand { name: name.into(), kind: PsCommandKind::Alias, definition: Some(definition.into()), usage: None }
    }

    #[test]
    fn piped_commands_must_run_as_the_page_means() {
        let pages = "%%% common/docker.md\n# docker\n\n> Manage containers.\n\n- Sort images by size:\n\n`docker images | sort -k 2`\n\n- Show a container's state:\n\n`docker inspect {{container}} | jq .State`\n%%% windows/sc.md\n# sc\n\n> Manage services.\n\n- Show a service's status:\n\n`sc query {{service_name}}`\n\n- List the commands:\n\n`sc`\n";
        let index = index_with_pages(
            &[
                ("docker", PathBuf::from(r"C:\Program Files\Docker\docker.exe")),
                ("jq", PathBuf::from(r"C:\tools\jq.exe")),
                ("sort", system32("sort.exe")),
                ("sc", system32("sc.exe")),
            ],
            vec![alias("sort", "Sort-Object"), alias("sc", "Set-Content")],
            pages,
        );
        let commands = example_commands(&index);
        // `sort` would be Sort-Object, which has no -k option.
        assert!(commands.iter().all(|c| !c.contains("sort")), "{commands:?}");
        assert!(commands.contains(&"docker inspect <container> | jq .State".to_string()), "{commands:?}");
        // In PowerShell, sc is Set-Content; the Windows page means sc.exe.
        assert!(commands.contains(&"sc.exe query <service-name>".to_string()), "{commands:?}");
        assert!(commands.contains(&"sc.exe".to_string()), "{commands:?}");
    }

    #[test]
    fn programs_are_named_by_file_until_the_command_list_is_known() {
        let index = index_with_pages(
            &[("curl", system32("curl.exe")), ("git", PathBuf::from(r"C:\Program Files\Git\cmd\git.exe"))],
            Vec::new(),
            PAGES,
        );
        let commands = example_commands(&index);
        // An alias may yet turn out to shadow curl, so don't type bare `curl`.
        assert!(commands.contains(&"curl.exe --remote-name <https-example-com-file-zip>".to_string()), "{commands:?}");
        assert!(commands.contains(&"git.exe commit --message <message>".to_string()), "{commands:?}");
    }

    /// The real bundled tldr pages parse and produce examples for programs
    /// every Windows 10/11 machine has; skipped when assets aren't fetched.
    #[test]
    fn real_tldr_bundle_yields_examples_for_builtin_tools() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources").join("tldr").join("pages.md");
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!("skipping: run scripts/fetch-assets.ps1 first");
            return;
        };
        let pages = super::super::tldr::parse_bundle(&text);
        assert!(pages.len() > 4000, "only {} pages parsed", pages.len());
        let inventory = Inventory::new(
            HashMap::from([
                ("robocopy".to_string(), system32("Robocopy.exe")),
                ("curl".to_string(), system32("curl.exe")),
                ("tar".to_string(), system32("tar.exe")),
            ]),
            vec![PsCommand { name: "curl".into(), kind: PsCommandKind::Alias, definition: Some("Invoke-WebRequest".into()), usage: None }],
        );
        let knowledge = Knowledge { tldr: Arc::new(pages), ..Knowledge::default() };
        let index = CommandIndex::build_with(Catalog::default(), inventory, None, knowledge);
        let commands = example_commands(&index);
        assert!(commands.iter().any(|c| c.starts_with("robocopy ")), "{commands:?}");
        assert!(commands.iter().any(|c| c.starts_with("curl.exe ")));
        assert!(commands.iter().any(|c| c.starts_with("tar ")));
        assert!(commands.iter().all(|c| !c.contains("{{")), "unconverted placeholder left");
    }

    /// End-to-end ranking with the real bundled catalog and model; skipped
    /// when the assets haven't been fetched.
    #[test]
    fn real_catalog_and_model_rank_expected_entries_first() {
        let resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources");
        let (Ok(catalog), Ok(embedder)) =
            (Catalog::load(&resources.join("catalog.json")), StaticEmbedder::load(&resources.join("embedding")))
        else {
            eprintln!("skipping: run scripts/fetch-assets.ps1 first");
            return;
        };
        // Pretend everything the catalog needs is installed: hyphenated names
        // as cmdlets, the rest as programs outside System32.
        let (cmdlets, programs): (Vec<&String>, Vec<&String>) =
            catalog.entries.iter().flat_map(|e| e.requires.iter()).partition(|name| name.contains('-'));
        let needed: Vec<PsCommand> = cmdlets
            .into_iter()
            .map(|name| PsCommand { name: name.clone(), kind: PsCommandKind::Cmdlet, definition: None, usage: None })
            .collect();
        let executables: HashMap<String, PathBuf> =
            programs.into_iter().map(|name| (name.to_lowercase(), PathBuf::from(format!(r"C:\Tools\{name}.exe")))).collect();
        // With the tldr examples in the mix, curated recipes must still surface.
        let tldr = std::fs::read_to_string(resources.join("tldr").join("pages.md"))
            .map(|text| super::super::tldr::parse_bundle(&text))
            .unwrap_or_default();
        let knowledge = Knowledge { tldr: Arc::new(tldr), ..Knowledge::default() };
        let index = CommandIndex::build_with(catalog, Inventory::new(executables, needed), Some(Arc::new(embedder)), knowledge);
        let cases = [
            ("extract zip file", "Expand-Archive -Path <archive> -DestinationPath <folder>"),
            ("kill whatever is using port 3000", "Get-NetTCPConnection"),
            ("search for text inside files", "Select-String"),
            ("how much free space is on my drives", "Get-PSDrive"),
            ("make a new folder", "New-Item"),
            ("show my ip address", "ipconfig"),
            ("undo my last git commit", "git reset"),
            ("create a python virtual environment", "venv"),
            ("install node", "winget install --id OpenJS.NodeJS"),
            ("delete a folder", "Remove-Item"),
            ("compress folder to zip", "Compress-Archive"),
            ("count lines in a file", "Measure-Object"),
            ("create a new branch", "git switch -c"),
            ("hash of a file", "Get-FileHash"),
            ("who am i", "whoami"),
        ];
        let mut failures = Vec::new();
        for (query, expected) in cases {
            let hits = index.search(query, 3);
            if !hits.iter().any(|h| h.command.to_lowercase().contains(&expected.to_lowercase())) {
                failures.push(format!("{query:?} -> {:?}", hits.iter().map(|h| &h.command).collect::<Vec<_>>()));
            }
        }
        assert!(failures.is_empty(), "expected commands missing from the top 3:\n{}", failures.join("\n"));
    }
}
