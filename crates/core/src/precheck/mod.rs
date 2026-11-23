//! Prechecks: everything that can be known to fail before Workbench runs.
//!
//! `Session::check` takes a cleaned table and returns a `Report`: the tree,
//! every finding (error / warning / info) with the row and identifier it's
//! about, and an optional `Fix`. Safe fixes (EDTF rewrites, term
//! capitalisation, LCSH identifiers -> headings) are applied by default;
//! guesses (a column or term that looks misspelt) are offered but off until
//! someone accepts them. Applying fixes and re-checking is how the final
//! bundle is produced, so the bundle always matches a clean report.

pub(crate) mod checks;
mod vocab;
pub use vocab::locked_reason;

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
    /// Rows and files that don't pair up. On = leave them out of the ingest
    /// and list them in a CSV in the bundle; off = ingest them anyway.
    FileGap(Gap),
}

/// The ways rows and access files can fail to pair up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Gap {
    /// Item rows with no file at all.
    NoFile,
    /// Rows that name a file the folder doesn't have.
    MissingFile,
    /// Files in the folder that no row uses.
    Unmatched,
}

impl Gap {
    /// The CSV in the bundle that lists what was left out.
    pub fn csv(self) -> &'static str {
        match self {
            Gap::NoFile | Gap::MissingFile => "skipped-rows.csv",
            Gap::Unmatched => "unmatched-files.csv",
        }
    }
}

impl Fix {
    /// `describe`, with the sheet row it touches.
    pub fn describe_at(&self, table: &Table) -> String {
        match self {
            Fix::SetCell { row, .. } => match table.rows.get(*row) {
                Some(r) => format!("row {}: {}", r.line, self.describe()),
                None => self.describe(),
            },
            _ => self.describe(),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Fix::SetCell { column, value, .. } if value.is_empty() => format!("clear {column}"),
            Fix::SetCell { column, value, .. } => format!("set {column} to \"{value}\""),
            Fix::RenameColumn { from, to } => format!("rename column {from} -> {to}"),
            Fix::SetTitle { id, title } => format!("title {id} \"{title}\""),
            Fix::FileGap(g) => match g {
                Gap::NoFile => format!("leave out the rows with no file (listed in {})", g.csv()),
                Gap::MissingFile => format!("leave out the rows whose file is missing (listed in {})", g.csv()),
                Gap::Unmatched => format!("leave out the files no row uses (listed in {})", g.csv()),
            },
        }
    }
}

/// What a "did you mean" on a taxonomy value is about, so a screen can say it
/// plainly and offer the right choices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermHint {
    pub column: String,
    /// The value as the sheet has it.
    pub typed: String,
    /// The existing term it's probably meant to be.
    pub existing: String,
    pub vocab: String,
    pub tid: String,
    /// Why they look alike: "spelling - 1 letter different", "singular/plural"...
    pub why: String,
    /// Whether keeping the sheet's value creates a new term (false: it's an error).
    pub creatable: bool,
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
    /// A page that helps decide: the authority heading, the term on the site.
    pub link: Option<String>,
    pub hint: Option<TermHint>,
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
            link: None,
            hint: None,
        }
    }
    pub fn with_link(mut self, url: impl Into<String>) -> Self {
        self.link = Some(url.into());
        self
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
    /// Items already on the site (by identifier): left out of the bundle.
    pub on_site_items: std::collections::HashSet<String>,
    /// Files in the access folder that no row uses.
    pub unmatched_files: Vec<std::path::PathBuf>,
    /// Unmatched files that could become items of their own: (identifier
    /// from the file name, parent container). Empty unless all of them can.
    pub stub_items: Vec<(String, String)>,
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

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub site: Option<String>,
    pub access_dir: Option<PathBuf>,
    pub collection_title: Option<String>,
    /// Skip site lookups (unit tests, or a quick look at a sheet offline).
    pub offline: bool,
    /// Ignore the `file` column: every access file is `<identifier>.<ext>`.
    pub files_by_identifier: bool,
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
    prefetched: std::collections::HashSet<String>,
    /// Collections whose "what's already on the site" lookup failed in this
    /// check, with the error. Not cached: the next check asks again.
    node_errors: std::collections::BTreeMap<String, String>,
    /// Findings that must survive the settle passes even after a fix rewrites
    /// the cell that raised them (ID/heading mismatches).
    pub(crate) sticky: Vec<Finding>,
    /// Print what's being fetched to stderr (plain mode; never in the TUI).
    pub progress: bool,
    /// Progress + cancel for a check running on another thread (the TUI).
    pub job: std::sync::Arc<super::progress::Progress>,
    /// (vocabulary, value) -> (matching term, closest term): fuzzy matching a
    /// vocabulary is the slow part of a re-check, and the answers don't change.
    pub(crate) term_memo: HashMap<(String, String), (Option<String>, Option<String>)>,
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
            prefetched: Default::default(),
            node_errors: Default::default(),
            progress: false,
            job: Default::default(),
            term_memo: HashMap::new(),
            sticky: Vec::new(),
            authorities: if cfg!(test) { Cache::in_memory() } else { Cache::open() },
            site_findings: None,
        })
    }

    pub fn vocabulary(&mut self, vid: &str) -> Result<&[Term], String> {
        if !self.vocabs.contains_key(vid) {
            if self.progress && self.site.is_some() {
                eprint!("  fetching vocabulary {vid}... ");
            }
            self.job.say(format!("fetching the {vid} vocabulary from the site"));
            let t0 = std::time::Instant::now();
            let got = match &self.site {
                Some(site) => site.vocabulary(vid).map_err(|e| e.to_string()),
                None => Err("offline".to_string()),
            };
            if self.progress && self.site.is_some() {
                match &got {
                    Ok(v) => eprintln!("{} terms ({:.0}s)", v.len(), t0.elapsed().as_secs_f32()),
                    Err(e) => eprintln!("failed: {e}"),
                }
            }
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

    /// The node already on the site with this identifier. The first lookup under
    /// a collection fetches every node under that collection in one go; after
    /// that, anything not in the cache isn't on the site.
    ///
    /// A lookup that fails is never read as "not there" for good: it's recorded
    /// in `node_errors` (an error finding that blocks the ingest, so nothing is
    /// created twice) and asked again on the next check.
    pub fn find_node(&mut self, identifier: &str) -> Option<NodeRef> {
        if let Some(hit) = self.nodes.get(identifier) {
            return hit.clone();
        }
        let root = identifier
            .split('_')
            .take(super::tree::COLLECTION_SEGMENTS)
            .collect::<Vec<_>>()
            .join("_");
        if self.node_errors.contains_key(&root) {
            // Already failed in this check: don't sit through the retries again.
            return None;
        }
        if self.prefetched.insert(root.clone()) {
            if let Some(site) = &self.site {
                if self.progress {
                    eprint!("  looking up what's already on the site under {root}... ");
                }
                self.job.say(format!("looking up what's already on the site under {root}"));
                let got = site.nodes_under(&root);
                if self.progress {
                    match &got {
                        Ok(v) => eprintln!("{} node(s)", v.len()),
                        Err(e) => eprintln!("failed ({e})"),
                    }
                }
                match got {
                    Ok(found) => {
                        for (id, n) in found {
                            self.nodes.insert(id, Some(n));
                        }
                    }
                    Err(e) => {
                        self.prefetched.remove(&root);
                        self.node_errors.insert(root, format!("{e:#}"));
                        return None;
                    }
                }
            }
        }
        if self.prefetched.contains(&root) || self.site.is_none() {
            return self.nodes.entry(identifier.to_string()).or_insert(None).clone();
        }
        let got = match self.site.as_ref().map(|s| s.find_node(identifier)) {
            Some(Ok(got)) => got,
            Some(Err(e)) => {
                self.node_errors.insert(root, format!("{e:#}"));
                return None;
            }
            None => None,
        };
        self.nodes.insert(identifier.to_string(), got.clone());
        got
    }

    /// A term's page on the site.
    pub fn term_url(&self, tid: &str) -> Option<String> {
        self.site.as_ref().map(|s| format!("{}/taxonomy/term/{tid}", s.host()))
    }

    pub(crate) fn stick(&mut self, f: Finding) {
        if !self.sticky.iter().any(|x| x.message == f.message && x.line == f.line) {
            self.sticky.push(f);
        }
    }

    /// Run every check against `table`.
    pub fn check(&mut self, table: &Table, titles: &HashMap<String, String>) -> Report {
        let mut report = Report::default();
        checks::site(self, &mut report);
        checks::columns(self, table, &mut report);
        self.job.say("checking columns, identifiers and files");
        report.tree = super::tree::build_with(table, &self.cfg, &self.files, self.opts.files_by_identifier);
        for (id, t) in titles {
            if let Some(n) = report.tree.nodes.get_mut(id) {
                n.title = t.clone();
            }
        }
        self.node_errors.clear();
        checks::structure(self, table, &mut report);
        for (root, e) in &self.node_errors {
            report.findings.push(Finding::new(
                Severity::Error,
                "structure",
                format!(
                    "couldn't look up what's already on the site under {root} ({e}) - without it, collections and items \
                     already there could be created twice. Re-check (r) once the site answers"
                ),
            ));
        }
        checks::files(self, table, &mut report);
        checks::dates(self, table, &mut report);
        checks::lists(self, table, &mut report);
        if report.site_ok {
            vocab::check(self, table, &mut report);
        }
        for f in &self.sticky {
            if !report.findings.iter().any(|x| x.message == f.message && x.line == f.line) {
                report.findings.push(f.clone());
            }
        }
        report.findings.sort_by(|a, b| {
            a.severity
                .cmp(&b.severity)
                .then(a.check.cmp(b.check))
                .then(a.line.cmp(&b.line))
        });
        self.authorities.save();
        if self.progress && self.authorities.asked() > 0 {
            eprintln!();
        }
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
        // Accepted fixes apply even when no finding offers them (a title typed
        // in for a collection the sheet doesn't describe, say).
        for fix in accepted.iter().filter(|f| !rejected.contains(f)) {
            apply(&mut current, &mut titles, fix);
            applied.push(fix.clone());
        }
        for _ in 0..4 {
            let report = self.check(&current, &titles);
            if self.job.cancelled() {
                return (current, report, applied);
            }
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

/// Decisions made on one reading of a sheet, moved onto another (re-read,
/// or a different subset). A cell fix follows its row by identifier (and,
/// for an item's extra file rows, which of them it was), so rows inserted or
/// deleted in the sheet don't move a fix onto another item. A fix whose row
/// is gone is dropped.
pub fn remap_rows(fixes: &[Fix], from: &Table, to: &Table) -> Vec<Fix> {
    // (identifier, nth row with it) -> row index; by line when there's no identifier column.
    let keys = |t: &Table| -> Vec<(String, usize)> {
        let col = crate::tree::ID_COLUMNS.iter().find_map(|c| t.col(c));
        let mut seen: HashMap<String, usize> = HashMap::new();
        t.rows
            .iter()
            .map(|r| match col {
                Some(c) => {
                    let id = r.cells.get(c).map(|v| v.trim().to_string()).unwrap_or_default();
                    let n = seen.entry(id.clone()).or_insert(0);
                    *n += 1;
                    (id, *n)
                }
                None => (String::new(), r.line),
            })
            .collect()
    };
    let (from_keys, to_keys) = (keys(from), keys(to));
    fixes
        .iter()
        .filter_map(|f| match f {
            Fix::SetCell { row, column, value } => {
                let key = from_keys.get(*row)?;
                if key.0.is_empty() && from.col(crate::tree::ID_COLUMNS[0]).is_some() {
                    return None; // a row without an identifier can't be followed
                }
                let row = to_keys.iter().position(|k| k == key)?;
                Some(Fix::SetCell { row, column: column.clone(), value: value.clone() })
            }
            other => Some(other.clone()),
        })
        .collect()
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
        // Decided when the bundle is written.
        Fix::FileGap(_) => {}
    }
}

#[cfg(test)]
mod remap_tests {
    use super::*;
    use crate::source::Row;

    fn t(ids: &[&str]) -> Table {
        Table {
            source: "t".into(),
            headers: vec!["accessIdentifier".into(), "x".into()],
            rows: ids.iter().enumerate().map(|(i, id)| Row { line: i + 2, cells: vec![id.to_string(), String::new()] }).collect(),
            renamed: vec![],
            cells_repaired: 0,
            notes: vec![],
            headings: Default::default(),
            subset: None,
        }
    }

    #[test]
    fn fixes_follow_identifiers_not_row_numbers() {
        let before = t(&["a_1", "a_2", "a_3"]);
        // A row inserted above, and a_1 deleted.
        let after = t(&["a_0", "a_2", "a_3"]);
        let set = |row| Fix::SetCell { row, column: "x".into(), value: "v".into() };
        let moved = remap_rows(&[set(0), set(1), set(2)], &before, &after);
        assert_eq!(moved, vec![set(1), set(2)]); // a_1's fix dropped, a_2 and a_3 stay theirs
    }
}
