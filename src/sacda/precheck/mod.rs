//! Prechecks: everything that can be known to fail before Workbench runs.
//!
//! `Session::check` takes a cleaned table and returns a `Report`: the tree,
//! every finding (error / warning / info) with the row and identifier it's
//! about, and an optional `Fix`. Safe fixes (EDTF rewrites, term
//! capitalisation, LCSH identifiers -> headings) are applied by default;
//! guesses (a column or term that looks misspelt) are offered but off until
//! someone accepts them. Applying fixes and re-checking is how the final
//! bundle is produced, so the bundle always matches a clean report.

mod checks;
mod vocab;

pub use vocab::{render as vocab_render, NewTerm, Val};

use super::authority::Cache;
use super::config::Config;
use super::site::{NodeRef, Site, Term};
use super::source::Table;
use super::tree::{FileIndex, Tree};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn label(&self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Fix {
    SetCell { row: usize, column: String, value: String },
    RenameColumn { from: String, to: String },
    /// Title for a container nobody described in the sheet.
    SetTitle { id: String, title: String },
}

impl Fix {
    /// `describe`, with the data row it touches (row index -> sheet row).
    pub fn describe_at(&self) -> String {
        match self {
            Fix::SetCell { row, .. } => format!("row {}: {}", row + 2, self.describe()),
            _ => self.describe(),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Fix::SetCell { column, value, .. } if value.is_empty() => format!("clear {column}"),
            Fix::SetCell { column, value, .. } => format!("set {column} to \"{value}\""),
            Fix::RenameColumn { from, to } => format!("rename column {from} -> {to}"),
            Fix::SetTitle { id, title } => format!("title {id} \"{title}\""),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub severity: Severity,
    /// Short check name, used to group findings: `site`, `columns`, `files`...
    pub check: &'static str,
    /// Spreadsheet row number (1-based, header = 1).
    pub line: Option<usize>,
    pub id: Option<String>,
    pub message: String,
    pub fix: Option<Fix>,
    /// Whether the fix is applied unless someone turns it off.
    pub auto: bool,
}

impl Finding {
    pub fn new(severity: Severity, check: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity,
            check,
            line: None,
            id: None,
            message: message.into(),
            fix: None,
            auto: false,
        }
    }
    pub fn at(mut self, line: usize, id: impl Into<String>) -> Self {
        self.line = Some(line);
        let id = id.into();
        if !id.is_empty() {
            self.id = Some(id);
        }
        self
    }
    pub fn with_fix(mut self, fix: Fix, auto: bool) -> Self {
        self.fix = Some(fix);
        self.auto = auto;
        self
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub tree: Tree,
    pub findings: Vec<Finding>,
    /// Resolved taxonomy cells: (row index, column) -> values.
    pub values: HashMap<(usize, String), Vec<Val>>,
    /// Terms the terms jobs must create, by vocabulary.
    pub new_terms: BTreeMap<String, Vec<NewTerm>>,
    /// Collections already on the site, by identifier.
    pub attach: HashMap<String, NodeRef>,
    /// Columns that go nowhere (unknown and not renamed).
    pub dropped_columns: Vec<String>,
    pub site_ok: bool,
}

impl Report {
    pub fn count(&self, s: Severity) -> usize {
        self.findings.iter().filter(|f| f.severity == s).count()
    }
    pub fn blocked(&self) -> bool {
        self.count(Severity::Error) > 0
    }
}

pub struct Options {
    pub site: Option<String>,
    pub access_dir: Option<PathBuf>,
    pub collection_title: Option<String>,
    /// Skip site lookups (unit tests, or a quick look at a sheet offline).
    pub offline: bool,
}

/// State that outlives one check: the site client and everything fetched
/// from it, so re-checking after a fix costs nothing.
pub struct Session {
    pub cfg: Config,
    pub site: Option<Site>,
    pub site_name: String,
    pub files: FileIndex,
    pub opts: Options,
    vocabs: HashMap<String, Result<Vec<Term>, String>>,
    lang: Option<Result<HashMap<String, String>, String>>,
    nodes: HashMap<String, Option<NodeRef>>,
    pub authorities: Cache,
    site_findings: Option<(Vec<Finding>, bool)>,
}

impl Session {
    pub fn new(cfg: Config, input: &str, opts: Options) -> anyhow::Result<Self> {
        let (site_name, _) = cfg.site(opts.site.as_deref())?;
        let site = if opts.offline {
            None
        } else {
            Some(Site::new(&cfg, Some(&site_name))?)
        };
        let access = opts.access_dir.clone().unwrap_or_else(|| {
            let base = if super::source::is_url(input) {
                std::env::current_dir().unwrap_or_default()
            } else {
                std::path::Path::new(input)
                    .parent()
                    .map(|p| if p.as_os_str().is_empty() { std::path::Path::new(".") } else { p })
                    .unwrap_or(std::path::Path::new("."))
                    .to_path_buf()
            };
            base.join(&cfg.sacda.ingest.access_dir)
        });
        let files = FileIndex::build(&access);
        Ok(Self {
            cfg,
            site,
            site_name,
            files,
            opts,
            vocabs: HashMap::new(),
            lang: None,
            nodes: HashMap::new(),
            authorities: if cfg!(test) { Cache::in_memory() } else { Cache::open() },
            site_findings: None,
        })
    }

    pub fn vocabulary(&mut self, vid: &str) -> Result<&[Term], String> {
        if !self.vocabs.contains_key(vid) {
            let got = match &self.site {
                Some(site) => site.vocabulary(vid).map_err(|e| e.to_string()),
                None => Err("offline".to_string()),
            };
            self.vocabs.insert(vid.to_string(), got);
        }
        match &self.vocabs[vid] {
            Ok(v) => Ok(v.as_slice()),
            Err(e) => Err(e.clone()),
        }
    }

    pub fn language_codes(&mut self) -> Result<&HashMap<String, String>, String> {
        if self.lang.is_none() {
            self.lang = Some(match &self.site {
                Some(site) => site.language_codes().map_err(|e| e.to_string()),
                None => Err("offline".to_string()),
            });
        }
        match self.lang.as_ref().unwrap() {
            Ok(m) => Ok(m),
            Err(e) => Err(e.clone()),
        }
    }

    pub fn find_node(&mut self, identifier: &str) -> Option<NodeRef> {
        if !self.nodes.contains_key(identifier) {
            let got = self.site.as_ref().and_then(|s| s.find_node(identifier).ok().flatten());
            self.nodes.insert(identifier.to_string(), got);
        }
        self.nodes[identifier].clone()
    }

    /// Run every check against `table`.
    pub fn check(&mut self, table: &Table, titles: &HashMap<String, String>) -> Report {
        let mut report = Report::default();
        checks::site(self, &mut report);
        checks::columns(self, table, &mut report);
        report.tree = super::tree::build(table, &self.cfg, &self.files);
        for (id, t) in titles {
            if let Some(n) = report.tree.nodes.get_mut(id) {
                n.title = t.clone();
            }
        }
        checks::structure(self, table, &mut report);
        checks::files(self, table, &mut report);
        checks::dates(self, table, &mut report);
        if report.site_ok {
            vocab::check(self, table, &mut report);
        }
        report.findings.sort_by(|a, b| {
            a.severity
                .cmp(&b.severity)
                .then(a.check.cmp(b.check))
                .then(a.line.cmp(&b.line))
        });
        self.authorities.save();
        report
    }

    /// Check, apply the fixes that are on (auto ones, plus any in `accepted`,
    /// minus any in `rejected`), and re-check until nothing changes. Returns the
    /// fixed table and its report.
    pub fn settle(
        &mut self,
        table: &Table,
        accepted: &[Fix],
        rejected: &[Fix],
    ) -> (Table, Report, Vec<Fix>) {
        let mut applied: Vec<Fix> = Vec::new();
        let mut current = table.clone();
        let mut titles = HashMap::new();
        for _ in 0..4 {
            let report = self.check(&current, &titles);
            let todo: Vec<Fix> = report
                .findings
                .iter()
                .filter_map(|f| {
                    let fix = f.fix.as_ref()?;
                    let on = (f.auto || accepted.contains(fix)) && !rejected.contains(fix);
                    (on && !applied.contains(fix)).then(|| fix.clone())
                })
                .collect();
            if todo.is_empty() {
                return (current, report, applied);
            }
            for fix in &todo {
                apply(&mut current, &mut titles, fix);
            }
            applied.extend(todo);
        }
        let report = self.check(&current, &titles);
        (current, report, applied)
    }
}

pub fn apply(table: &mut Table, titles: &mut HashMap<String, String>, fix: &Fix) {
    match fix {
        Fix::SetCell { row, column, value } => {
            let col = match table.col(column) {
                Some(c) => c,
                None => {
                    table.headers.push(column.clone());
                    for r in table.rows.iter_mut() {
                        r.cells.push(String::new());
                    }
                    table.headers.len() - 1
                }
            };
            if let Some(r) = table.rows.get_mut(*row) {
                if r.cells.len() <= col {
                    r.cells.resize(col + 1, String::new());
                }
                r.cells[col] = value.clone();
            }
        }
        Fix::RenameColumn { from, to } => {
            if table.col(to).is_none() {
                if let Some(c) = table.col(from) {
                    table.headers[c] = to.clone();
                }
            }
        }
        Fix::SetTitle { id, title } => {
            titles.insert(id.clone(), title.clone());
        }
    }
}
