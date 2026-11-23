//! The interactive screen: pick a sheet and a site, read the precheck, decide
//! on each suggested fix, re-check, then write the bundle (and optionally
//! ingest it).
//!
//! Everything shown here comes from sacda-core's `Session::settle` /
//! `bundle::write` - the same calls the plain mode makes. The screen only
//! records decisions. Checks run on a worker thread, so the screen stays
//! usable (and cancellable) while the site and id.loc.gov are being asked.

mod view;

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyCode,
    KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
pub mod ingest;
pub use ingest::ingest;

use sacda::bundle;
use sacda::config::Config;
use sacda::precheck::{Fix, Options, Report, Session, Severity};
use sacda::progress::Progress;
use sacda::sheets::{Pulled, Sheets};
use sacda::source::{self, Table};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io::Stdout;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

pub struct Start {
    pub cfg: Config,
    pub input: Option<String>,
    pub site: Option<String>,
    pub sheet: Option<String>,
    pub access_dir: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub collection_title: Option<String>,
    pub accept_suggestions: bool,
    pub files_by_identifier: bool,
    /// `--only`: the subset for the sheet given on the command line.
    pub only: Option<String>,
}

pub enum Outcome {
    Ingest { cfg: Config, dir: PathBuf },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Findings,
    Tree,
    Terms,
}

/// One row of the findings list: a current finding, or a fix already applied
/// (shown so it can be turned off).
#[derive(Clone)]
pub enum Entry {
    Finding(usize),
    Applied(Fix),
}

/// Where a suggested fix stands.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Applied (accepted, or a safe fix nobody turned off).
    On,
    /// Turned down: the sheet's value stays.
    Off,
    /// A guess nobody has answered yet - blocks writing.
    Undecided,
}

pub enum Input {
    None,
    /// Typing a Google Sheets link on the pick screen.
    Url(String),
    /// Typing the folder the access files are in.
    Folder(String),
    /// Typing which rows to use (page-style: 12-40, 57, 100-).
    Subset(String),
    /// Confirming the temp files clean-up (Enter = delete).
    Clean,
    /// Typing the site name to confirm a protected site.
    Confirm(String),
    /// Typing a title for a collection nobody described.
    Title { id: String, buf: String },
}

pub struct Review {
    pub input_name: String,
    /// The whole sheet as read, before any subset: a different subset is
    /// cut from this without reading or checking the site again.
    pub full: Table,
    /// The rows being checked (the whole sheet, or a subset of it).
    pub original: Table,
    pub fixed: Table,
    pub report: Report,
    pub applied: Vec<Fix>,
    pub accepted: Vec<Fix>,
    pub rejected: Vec<Fix>,
    pub entries: Vec<Entry>,
    pub sel: usize,
    pub tab: Tab,
    pub scroll: u16,
    /// Decisions made since the last check finished; writing waits for a re-check.
    pub changes: usize,
    /// New terms created without their authority link: (vocabulary, name).
    pub no_link: HashSet<(String, String)>,
    pub term_sel: usize,
    /// First visible row of each list, kept between draws so scrolling back up
    /// moves the selection, not the view.
    pub list_offset: Cell<usize>,
    pub term_offset: Cell<usize>,
}

impl Review {
    pub fn choice(&self, fix: &Fix, auto: bool) -> Choice {
        if self.rejected.contains(fix) {
            Choice::Off
        } else if auto || self.accepted.contains(fix) {
            Choice::On
        } else {
            Choice::Undecided
        }
    }

    /// Suggestions nobody has said yes or no to.
    pub fn undecided(&self) -> usize {
        self.report
            .findings
            .iter()
            .filter(|f| f.fix.as_ref().is_some_and(|x| self.choice(x, f.auto) == Choice::Undecided))
            .count()
    }

    /// Why this can't be written yet, or None when it can.
    pub fn not_ready(&self) -> Option<String> {
        let errors = self.report.count(Severity::Error);
        let undecided = self.undecided();
        if self.changes > 0 {
            Some(format!("{} change(s) not checked yet - press c to re-check", self.changes))
        } else if undecided > 0 {
            Some(format!("{undecided} suggestion(s) to decide (y use / n keep)"))
        } else if errors > 0 {
            Some(format!("{errors} error(s) to fix"))
        } else {
            None
        }
    }

    /// New terms, flattened in display order: (vocabulary, index in its list).
    pub fn term_rows(&self) -> Vec<(String, usize)> {
        self.report
            .new_terms
            .iter()
            .flat_map(|(v, ts)| (0..ts.len()).map(move |i| (v.clone(), i)))
            .collect()
    }

    fn key_of(&self, i: usize) -> Option<(String, Option<usize>)> {
        match self.entries.get(i)? {
            Entry::Finding(f) => self.report.findings.get(*f).map(|f| (f.message.clone(), f.line)),
            Entry::Applied(fix) => Some((fix.describe(), None)),
        }
    }
}

pub enum Screen {
    Pick,
    Review(Box<Review>),
    Written { dir: PathBuf, summary: Vec<String> },
}

/// A check running on the worker thread.
pub struct Job {
    pub label: String,
    pub progress: Arc<Progress>,
    pub started: Instant,
    rx: mpsc::Receiver<Result<Done, String>>,
    /// `Review::changes` when it started: those are the ones it covers.
    changes_at_start: usize,
    pub cancelling: bool,
    /// Open jobs are dropped on cancel; re-checks hand the session back first.
    detachable: bool,
}

enum Done {
    Pulled(Pulled),
    Opened {
        input: String,
        full: Table,
        table: Table,
        session: Box<Session>,
        settled: Settled,
        accepted: Vec<Fix>,
    },
    Rechecked {
        session: Box<Session>,
        settled: Settled,
        table: Option<Table>,
        /// The whole re-read sheet, before the subset.
        full: Option<Table>,
        /// Decisions moved onto the re-read sheet's rows.
        decisions: Option<(Vec<Fix>, Vec<Fix>)>,
        cancelled: bool,
    },
}

type Settled = (Table, Report, Vec<Fix>);

pub struct App {
    pub cfg: Config,
    pub sites: Vec<String>,
    pub site_idx: usize,
    pub files: Vec<PathBuf>,
    pub pick_sel: usize,
    pub screen: Screen,
    pub input: Input,
    pub status: String,
    pub session: Option<Session>,
    pub confirmed_site: Option<String>,
    pub job: Option<Job>,
    /// `--access-dir`: the files folder for a sheet that hasn't got its own.
    pub default_dir: Option<PathBuf>,
    /// `--files-by-identifier`, for sheets without their own setting.
    pub default_by_identifier: bool,
    /// Google Sheets history and each sheet's files folder / file-name mode.
    pub sheets: Sheets,
    /// The link pulled just now (highlighted in the list).
    pub just_pulled: Option<String>,
    /// Rows were changed (`s`) while a check ran: re-read once it's done.
    pub reread_after_job: bool,
    pub pick_offset: Cell<usize>,
    count_tx: mpsc::Sender<(PathBuf, Option<usize>)>,
    count_rx: mpsc::Receiver<(PathBuf, Option<usize>)>,
    start: Start,
    pending_input: Option<String>,
    /// Clickable areas from the last draw (the view fills it in).
    pub hits: RefCell<Vec<(Rect, Hit)>>,
    /// Links drawn on screen; `Hit::Link(i)` opens `links[i]`.
    pub links: RefCell<Vec<String>>,
    /// File counts per access folder, for the pick screen (None while counting -
    /// on a network share that takes a while, so it's never done on this thread).
    pub folder_counts: RefCell<HashMap<PathBuf, Option<Option<usize>>>>,
}

/// One line of the pick list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickRow {
    Header(&'static str),
    Sheet(String),
}

/// What a mouse click on a drawn area means.
#[derive(Debug, Clone, Copy)]
pub enum Hit {
    /// A key chip in the footer, a tab, an action button: behaves like pressing the key.
    Key(KeyCode),
    /// A site on the pick screen.
    Site(usize),
    /// The spreadsheet list: rows start at the rect's top, after `offset`.
    PickList { offset: usize },
    /// The findings list; the first `check_cols` columns are the [x] box.
    Findings { offset: usize, check_cols: u16 },
    /// The new-terms list; the first `check_cols` columns are the link box.
    Terms { offset: usize, check_cols: u16 },
    /// Open `App::links[i]` in the browser.
    Link(usize),
    /// A scrollable view (tree).
    Scroll,
}

const SHEET_EXTS: &[&str] = &["csv", "tsv", "xlsx", "xls", "xlsb", "ods"];

/// Spreadsheets under the current folder (3 levels), newest first.
fn find_sheets(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<(SystemTime, PathBuf)> = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.starts_with('~') {
                continue;
            }
            if p.is_dir() {
                let skip = ["target", "node_modules", "rollback", "logs", "__pycache__"].contains(&name.as_str())
                    || name.starts_with("ingest-");
                if depth < 3 && !skip {
                    stack.push((p, depth + 1));
                }
            } else if p
                .extension()
                .is_some_and(|x| SHEET_EXTS.contains(&x.to_string_lossy().to_lowercase().as_str()))
            {
                let m = e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                out.push((m, p));
            }
        }
    }
    out.sort_by_key(|x| std::cmp::Reverse(x.0));
    out.into_iter().map(|(_, p)| p).collect()
}

/// Files under `dir` (stops counting at 100k), or None if it isn't a folder.
pub fn count_files(dir: &Path) -> Option<usize> {
    if !dir.is_dir() {
        return None;
    }
    let mut n = 0;
    let mut stack = vec![dir.to_path_buf()];
    let mut seen = std::collections::HashSet::new();
    while let Some(d) = stack.pop() {
        // A link back up the tree would otherwise be walked forever.
        if !seen.insert(dunce::canonicalize(&d).unwrap_or(d.clone())) {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if !e.file_name().to_string_lossy().starts_with('.') {
                n += 1;
                if n >= 100_000 {
                    return Some(n);
                }
            }
        }
    }
    Some(n)
}

/// A Google Sheets link's document id, if it is one.
pub fn sheet_id(url: &str) -> Option<&str> {
    let rest = url.trim().split("docs.google.com/spreadsheets/d/").nth(1)?;
    let id = rest.split(['/', '?', '#']).next()?;
    (id.len() >= 20).then_some(id)
}

/// A typed or dropped folder path: quotes and shell escapes off, `~` expanded.
pub fn clean_path(raw: &str) -> PathBuf {
    let t = raw.trim().trim_matches(|c| c == '"' || c == '\'');
    let t = if cfg!(windows) { t.to_string() } else { t.replace("\\ ", " ") };
    match t.strip_prefix('~') {
        Some(rest) => std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|h| PathBuf::from(h).join(rest.trim_start_matches(['/', '\\'])))
            .unwrap_or_else(|| PathBuf::from(&t)),
        None => PathBuf::from(t),
    }
}

/// Open a link in the browser. Mouse capture stops the terminal's own
/// cmd/ctrl-click, so the screen does it.
pub fn open_url(url: &str) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else if cfg!(windows) {
        // `start` would split the URL on `&`.
        let mut c = Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|_| ())
}

impl App {
    fn new(start: Start) -> Self {
        let cfg = start.cfg.clone();
        // Sites that need confirming (prod) go last.
        let mut sites: Vec<String> = cfg.sacda.sites.keys().cloned().collect();
        sites.sort_by_key(|s| cfg.sacda.sites[s].confirm);
        let want = start.site.clone().unwrap_or_else(|| cfg.sacda.default_site.clone());
        let site_idx = sites.iter().position(|s| *s == want).unwrap_or(0);
        let cwd = std::env::current_dir().unwrap_or_default();
        let (count_tx, count_rx) = mpsc::channel();
        Self {
            cfg,
            sites,
            site_idx,
            files: find_sheets(&cwd),
            pick_sel: 0,
            hits: Default::default(),
            links: Default::default(),
            folder_counts: Default::default(),
            screen: Screen::Pick,
            input: Input::None,
            status: String::new(),
            session: None,
            confirmed_site: None,
            job: None,
            default_dir: start.access_dir.clone(),
            default_by_identifier: start.files_by_identifier,
            sheets: Sheets::load(),
            just_pulled: None,
            reread_after_job: false,
            pick_offset: Cell::new(0),
            count_tx,
            count_rx,
            pending_input: start.input.clone(),
            start,
        }
    }

    pub fn site(&self) -> &str {
        &self.sites[self.site_idx]
    }

    fn needs_confirm(&self) -> bool {
        self.cfg.sacda.sites.get(self.site()).is_some_and(|s| s.confirm)
            && self.confirmed_site.as_deref() != Some(self.site())
    }

    /// The pick list: Google Sheets links first, then sheets in this folder.
    pub fn pick_rows(&self) -> Vec<PickRow> {
        let mut rows = Vec::new();
        let links = self.sheets.links();
        if !links.is_empty() {
            rows.push(PickRow::Header("Google Sheets"));
            rows.extend(links.iter().map(|s| PickRow::Sheet(s.input.clone())));
        }
        if !self.files.is_empty() {
            rows.push(PickRow::Header("Spreadsheets in this folder"));
            rows.extend(self.files.iter().map(|p| PickRow::Sheet(p.to_string_lossy().to_string())));
        }
        rows
    }

    /// The sheet under the cursor on the pick screen.
    pub fn selected_input(&self) -> Option<String> {
        match self.pick_rows().get(self.pick_sel) {
            Some(PickRow::Sheet(s)) => Some(s.clone()),
            _ => None,
        }
    }

    /// Move the pick cursor by `d`, skipping section headers.
    fn pick_move(&mut self, d: isize) {
        let rows = self.pick_rows();
        let mut i = self.pick_sel as isize;
        loop {
            i += d.signum();
            if i < 0 || i as usize >= rows.len() {
                return;
            }
            if matches!(rows[i as usize], PickRow::Sheet(_)) {
                self.pick_sel = i as usize;
                if d.abs() <= 1 {
                    return;
                }
                return self.pick_move(d - d.signum());
            }
        }
    }

    fn select_input(&mut self, input: &str) {
        if let Some(i) = self.pick_rows().iter().position(|r| *r == PickRow::Sheet(input.to_string())) {
            self.pick_sel = i;
        }
    }

    /// First sheet row, so the cursor never starts on a header.
    fn pick_first(&mut self) {
        if let Some(i) = self.pick_rows().iter().position(|r| matches!(r, PickRow::Sheet(_))) {
            self.pick_sel = i;
        }
    }

    pub fn by_identifier(&self, input: Option<&str>) -> bool {
        input
            .and_then(|i| self.sheets.get(i))
            .map(|s| s.files_by_identifier)
            .unwrap_or(self.default_by_identifier)
    }

    /// The access folder a sheet would use, and whether it was chosen by hand.
    pub fn access_for(&self, input: Option<&str>) -> (PathBuf, bool) {
        if let Some(d) = input.and_then(|i| self.sheets.get(i)).and_then(|s| s.folder.clone()) {
            return (d, true);
        }
        if let Some(d) = &self.default_dir {
            return (d.clone(), true);
        }
        let base = match input {
            Some(i) if !source::is_url(i) => Path::new(i)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
            _ => std::env::current_dir().unwrap_or_default(),
        };
        (base.join(&self.cfg.sacda.ingest.access_dir), false)
    }

    /// Files in `dir`: Some(Some(n)), Some(None) if it isn't a folder, None
    /// while it's still being counted (on a worker).
    pub fn folder_count(&self, dir: &Path) -> Option<Option<usize>> {
        let mut c = self.folder_counts.borrow_mut();
        if let Some(v) = c.get(dir) {
            return *v;
        }
        c.insert(dir.to_path_buf(), None);
        let (tx, d) = (self.count_tx.clone(), dir.to_path_buf());
        std::thread::spawn(move || {
            let n = count_files(&d);
            let _ = tx.send((d, n));
        });
        None
    }

    fn poll_counts(&mut self) -> bool {
        let mut got = false;
        while let Ok((d, n)) = self.count_rx.try_recv() {
            self.folder_counts.borrow_mut().insert(d, Some(n));
            got = true;
        }
        got
    }

    fn counting(&self) -> bool {
        self.folder_counts.borrow().values().any(Option::is_none)
    }

    /// The rows chosen for a sheet (`s`), if not the whole sheet. A saved
    /// choice that doesn't parse is an error, never "the whole sheet".
    pub fn subset_for(&self, input: &str) -> Result<Option<sacda::subset::Subset>, String> {
        let raw = self.sheets.get(input).map(|s| s.subset.clone()).unwrap_or_default();
        sacda::subset::Subset::parse(&raw)
            .map_err(|e| format!("the rows chosen for this sheet (\"{raw}\") aren't valid: {e:#} - press s to change them"))
    }

    fn options(&self, input: &str) -> Options {
        Options {
            site: Some(self.site().to_string()),
            access_dir: Some(self.access_for(Some(input)).0),
            collection_title: self.start.collection_title.clone(),
            offline: false,
            files_by_identifier: self.by_identifier(Some(input)),
        }
    }

    /// Download a pasted link and add it to the list (no check yet - pick
    /// its files folder first, then Enter).
    fn start_pull(&mut self, url: String) {
        let cfg = self.cfg.clone();
        let progress = Arc::new(Progress::default());
        let p = progress.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            p.say("downloading from Google");
            let _ = tx.send(sacda::sheets::pull(&url, &cfg).map(Done::Pulled).map_err(|e| format!("{e:#}")));
        });
        self.job = Some(Job {
            label: "Pulling the Google Sheet".into(),
            progress,
            started: Instant::now(),
            rx,
            changes_at_start: 0,
            cancelling: false,
            detachable: true,
        });
        self.status.clear();
    }

    /// Load a sheet and run the first check on the worker.
    fn start_open(&mut self, input: String) {
        if source::is_url(&input) {
            self.sheets.entry(&input).used = sacda::sheets::now();
            self.sheets.save();
        }
        let cfg = self.cfg.clone();
        let sheet = self.start.sheet.clone();
        let opts = self.options(&input);
        let subset = match self.subset_for(&input) {
            Ok(s) => s,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let accept = self.start.accept_suggestions;
        let progress = Arc::new(Progress::default());
        let p = progress.clone();
        let label = if source::is_url(&input) { "Downloading and checking the sheet".into() } else { format!("Checking {}", short_name(&input)) };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let run = || -> Result<Done, String> {
                p.say(if source::is_url(&input) { "downloading the sheet from Google" } else { "reading the sheet" });
                let mut table = source::load_for(&input, sheet.as_deref(), &cfg).map_err(|e| format!("{e:#}"))?;
                let full = table.clone();
                if let Some(s) = &subset {
                    s.apply(&mut table).map_err(|e| format!("{e:#}"))?;
                }
                if p.cancelled() {
                    return Err("cancelled".into());
                }
                p.say("indexing the files folder");
                let mut session = Session::new(cfg, &input, opts).map_err(|e| format!("{e:#}"))?;
                session.job = p.clone();
                let accepted = if accept {
                    let (_, first, _) = session.settle(&table, &[], &[]);
                    first.findings.iter().filter(|f| !f.auto).filter_map(|f| f.fix.clone()).collect()
                } else {
                    Vec::new()
                };
                let settled = session.settle(&table, &accepted, &[]);
                if p.cancelled() {
                    return Err("cancelled".into());
                }
                Ok(Done::Opened { input, full, table, session: Box::new(session), settled, accepted })
            };
            let _ = tx.send(run());
        });
        self.job = Some(Job { label, progress, started: Instant::now(), rx, changes_at_start: 0, cancelling: false, detachable: true });
        self.status.clear();
    }

    /// Re-check the current decisions (or, with `reload`, re-read the sheet
    /// and start a fresh session) on the worker.
    fn start_recheck(&mut self, reload: bool) {
        if self.job.is_some() {
            self.status = "Still checking - wait for it, or Esc to cancel".into();
            return;
        }
        let Screen::Review(r) = &self.screen else { return };
        let input = r.input_name.clone();
        let original = r.original.clone();
        let accepted = r.accepted.clone();
        let rejected = r.rejected.clone();
        let changes_at_start = r.changes;
        let cfg = self.cfg.clone();
        let sheet = self.start.sheet.clone();
        let opts = self.options(&input);
        let subset = match self.subset_for(&input) {
            Ok(s) => s,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let old = self.session.take();
        let progress = Arc::new(Progress::default());
        let p = progress.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let run = || -> Result<Done, String> {
                let mut accepted = accepted;
                let mut rejected = rejected;
                let mut decisions = None;
                let mut full = None;
                let (mut session, table) = match (reload, old) {
                    (false, Some(s)) => (s, None),
                    _ => {
                        p.say(if source::is_url(&input) { "downloading the sheet again" } else { "re-reading the sheet" });
                        let mut t = source::load_for(&input, sheet.as_deref(), &cfg).map_err(|e| format!("{e:#}"))?;
                        full = Some(t.clone());
                        if let Some(s) = &subset {
                            s.apply(&mut t).map_err(|e| format!("{e:#}"))?;
                        }
                        // Row numbers in the decisions point into the old reading.
                        accepted = sacda::precheck::remap_rows(&accepted, &original, &t);
                        rejected = sacda::precheck::remap_rows(&rejected, &original, &t);
                        decisions = Some((accepted.clone(), rejected.clone()));
                        (Session::new(cfg, &input, opts).map_err(|e| format!("{e:#}"))?, Some(t))
                    }
                };
                session.job = p.clone();
                let base = table.as_ref().unwrap_or(&original);
                let settled = session.settle(base, &accepted, &rejected);
                let cancelled = p.cancelled();
                session.job = Default::default();
                Ok(Done::Rechecked { session: Box::new(session), settled, table, full, decisions, cancelled })
            };
            let _ = tx.send(run());
        });
        self.job = Some(Job {
            label: if reload { "Re-reading the sheet and the site".into() } else { "Re-checking".into() },
            progress,
            started: Instant::now(),
            rx,
            changes_at_start,
            cancelling: false,
            detachable: false,
        });
        self.status.clear();
    }

    /// The rows changed (`s`) on the review screen: cut them from the sheet
    /// already read and re-check with what's already fetched from the site -
    /// no download, no vocabulary fetch. Decisions follow their rows.
    fn use_subset(&mut self) {
        let subset = match &self.screen {
            Screen::Review(r) => match self.subset_for(&r.input_name) {
                Ok(s) => s,
                Err(e) => {
                    self.status = e;
                    return;
                }
            },
            _ => return,
        };
        let Screen::Review(r) = &mut self.screen else { return };
        let mut t = r.full.clone();
        if let Some(s) = &subset {
            if let Err(e) = s.apply(&mut t) {
                self.status = format!("{e:#}");
                return;
            }
        }
        r.accepted = sacda::precheck::remap_rows(&r.accepted, &r.original, &t);
        r.rejected = sacda::precheck::remap_rows(&r.rejected, &r.original, &t);
        r.original = t;
        // The last check was of other rows: what's shown is stale until this one's done.
        r.changes += 1;
        self.start_recheck(false);
    }

    fn cancel_job(&mut self) {
        let Some(job) = &mut self.job else { return };
        job.progress.cancel();
        if job.detachable {
            // An open has nothing worth waiting for: let the thread finish alone.
            self.job = None;
            self.status = "Cancelled".into();
        } else {
            job.cancelling = true;
        }
    }

    /// Pick up a finished job, if any.
    fn poll_job(&mut self) {
        let Some(job) = &self.job else { return };
        let got = match job.rx.try_recv() {
            Ok(g) => g,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("the check stopped unexpectedly".into()),
        };
        let job = self.job.take().unwrap();
        match got {
            Err(e) if e == "cancelled" => self.status = "Cancelled".into(),
            Err(e) => self.status = e,
            Ok(Done::Pulled(p)) => {
                self.sheets.remember(&p);
                self.sheets.save();
                self.just_pulled = Some(p.url.clone());
                self.select_input(&p.url);
                self.status = format!(
                    "Pulled \"{}\" ({} rows). Set its files folder with f (and m for file names), then Enter to check it.",
                    p.label, p.rows
                );
            }
            Ok(Done::Opened { input, full, table, session, settled, accepted }) => {
                let (fixed, report, applied) = settled;
                let mut review = Review {
                    input_name: input,
                    full,
                    original: table,
                    fixed,
                    report,
                    applied,
                    accepted,
                    rejected: Vec::new(),
                    entries: Vec::new(),
                    sel: 0,
                    tab: Tab::Findings,
                    scroll: 0,
                    changes: 0,
                    no_link: HashSet::new(),
                    term_sel: 0,
                    list_offset: Cell::new(0),
                    term_offset: Cell::new(0),
                };
                rebuild_entries(&mut review);
                self.session = Some(*session);
                self.screen = Screen::Review(Box::new(review));
                self.status.clear();
            }
            Ok(Done::Rechecked { session, settled, table, full, decisions, cancelled }) => {
                self.session = Some(*session);
                if cancelled {
                    self.status = "Re-check cancelled - showing the last full check".into();
                    return;
                }
                let Screen::Review(r) = &mut self.screen else { return };
                let keep = r.key_of(r.sel);
                let (fixed, report, applied) = settled;
                if let Some(t) = table {
                    r.original = t;
                }
                if let Some(t) = full {
                    r.full = t;
                }
                if let Some((a, rj)) = decisions {
                    r.accepted = a;
                    r.rejected = rj;
                }
                r.fixed = fixed;
                r.report = report;
                r.applied = applied;
                rebuild_entries(r);
                // Stay on the same finding if it's still there.
                r.sel = keep
                    .and_then(|k| (0..r.entries.len()).find(|&i| r.key_of(i).as_ref() == Some(&k)))
                    .unwrap_or_else(|| r.sel.min(r.entries.len().saturating_sub(1)));
                r.changes = r.changes.saturating_sub(job.changes_at_start);
                r.term_sel = r.term_sel.min(r.term_rows().len().saturating_sub(1));
                self.status = match r.not_ready() {
                    None => "Checked - ready to write (w) or ingest (i)".into(),
                    Some(why) => format!("Checked - {why}"),
                };
            }
        }
    }

    fn write(&mut self) -> Result<PathBuf> {
        let Screen::Review(r) = &mut self.screen else { anyhow::bail!("nothing to write") };
        // Authority links the user turned off.
        for (vocab, terms) in r.report.new_terms.iter_mut() {
            for t in terms.iter_mut() {
                if r.no_link.contains(&(vocab.clone(), t.name.clone())) {
                    t.link = None;
                }
            }
        }
        let site = self.sites[self.site_idx].clone();
        let dir = self.start.out.clone().unwrap_or_else(|| bundle::default_dir(&r.input_name, &r.report, &site));
        let w = bundle::write(&self.cfg, &site, &r.fixed, &r.report, &r.applied, &dir)?;
        let m = &w.manifest;
        let mut summary = vec![
            format!("Site: {} ({})", m.site, m.host),
            format!("Collections: {}", m.collections.join(", ")),
            format!("{} component(s), {} item(s), {} file(s), {} new term(s)", m.components, m.items, m.files, m.new_terms),
        ];
        if m.skipped_rows > 0 {
            summary.push(format!("Left out: {} row(s) - see {}", m.skipped_rows, w.dir.join(sacda::precheck::Gap::NoFile.csv()).display()));
        }
        if m.unmatched_files > 0 {
            summary.push(format!(
                "Left out: {} unused file(s) - see {}",
                m.unmatched_files,
                w.dir.join(sacda::precheck::Gap::Unmatched.csv()).display()
            ));
        }
        summary.extend([
            String::new(),
            "Steps, in order:".into(),
        ]);
        for s in &m.steps {
            summary.push(format!(
                "  {}{}",
                s.name,
                if s.covers.is_empty() { String::new() } else { format!(" (+ {})", s.covers.join(", ")) }
            ));
        }
        summary.push(String::new());
        summary.push(format!("Run later with:  organise ingest \"{}\"", w.dir.display()));
        self.screen = Screen::Written { dir: w.dir.clone(), summary };
        Ok(w.dir)
    }
}

fn short_name(input: &str) -> String {
    Path::new(input).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| input.to_string())
}

pub fn rebuild_entries(r: &mut Review) {
    r.entries = (0..r.report.findings.len()).map(Entry::Finding).collect();
    r.entries.extend(r.applied.iter().cloned().map(Entry::Applied));
}

type Term = Terminal<CrosstermBackend<Stdout>>;

fn setup() -> Result<Term> {
    enable_raw_mode()?;
    let mut out = std::io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

fn teardown(t: &mut Term) {
    let _ = disable_raw_mode();
    let _ = execute!(t.backend_mut(), DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen);
    let _ = t.show_cursor();
}

pub fn run(start: Start) -> Result<Option<Outcome>> {
    let only = start.only.clone();
    let first = start.input.clone();
    let mut app = App::new(start);
    if let (Some(only), Some(input)) = (only, first) {
        // Same check as plain mode: a typo stops here instead of meaning "all rows".
        sacda::subset::Subset::parse(&only).map_err(|e| anyhow::anyhow!("--only {only}: {e:#}"))?;
        app.sheets.entry(&input).subset = only;
        app.sheets.save();
    }
    let mut term = setup()?;
    // Restore the terminal even if something below panics.
    let hook = std::panic::take_hook();
    // Only for the screen's own thread: a panic on the check thread is caught
    // there (the screen says the check stopped) and the screen carries on.
    let ui = std::thread::current().id();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().id() == ui {
            let _ = disable_raw_mode();
            let _ = execute!(std::io::stdout(), DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen);
            hook(info);
        }
    }));
    let result = event_loop(&mut app, &mut term);
    teardown(&mut term);
    result
}

fn busy(app: &mut App, term: &mut Term, msg: &str) -> Result<()> {
    app.status = msg.to_string();
    term.draw(|f| view::draw(f, app))?;
    Ok(())
}

fn event_loop(app: &mut App, term: &mut Term) -> Result<Option<Outcome>> {
    app.pick_first();
    // A sheet given on the command line skips the pick screen.
    if let Some(input) = app.pending_input.take() {
        if app.needs_confirm() {
            app.input = Input::Confirm(String::new());
            app.pending_input = Some(input);
        } else {
            app.start_open(input);
        }
    }

    loop {
        app.poll_job();
        if app.reread_after_job && app.job.is_none() {
            app.reread_after_job = false;
            app.use_subset();
        }
        app.poll_counts();
        term.draw(|f| view::draw(f, app))?;
        // Tick while something runs so the spinner, progress and counts move.
        let ticking = app.job.is_some() || app.counting();
        if !event::poll(Duration::from_millis(if ticking { 120 } else { 1000 }))? {
            continue;
        }
        let key = match event::read()? {
            Event::Key(k) => k,
            Event::Mouse(m) => match mouse(app, m) {
                Some(k) => k,
                None => continue,
            },
            Event::Paste(text) => {
                paste(app, &text);
                continue;
            }
            _ => continue,
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(None);
        }
        match handle(app, term, key)? {
            Flow::Continue => {}
            Flow::Quit => return Ok(None),
            Flow::Ingest(dir) => return Ok(Some(Outcome::Ingest { cfg: app.cfg.clone(), dir })),
        }
    }
}

/// Pasted text goes into whatever box is open, as one line.
fn paste(app: &mut App, text: &str) {
    let one_line: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    match &mut app.input {
        Input::Url(b) | Input::Folder(b) | Input::Subset(b) | Input::Confirm(b) | Input::Title { buf: b, .. } => b.push_str(one_line.trim()),
        Input::Clean => {}
        Input::None => {
            // A link pasted straight onto the pick screen: open the box with it.
            if matches!(app.screen, Screen::Pick) && sheet_id(&one_line).is_some() {
                app.input = Input::Url(one_line.trim().to_string());
            }
        }
    }
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// A mouse event as the key press it stands for (the key handlers then do the
/// work), or `None` after handling it directly (selecting a row).
fn mouse(app: &mut App, m: MouseEvent) -> Option<KeyEvent> {
    match m.kind {
        MouseEventKind::ScrollUp => return Some(press(KeyCode::Up)),
        MouseEventKind::ScrollDown => return Some(press(KeyCode::Down)),
        MouseEventKind::Down(MouseButton::Left) => {}
        _ => return None,
    }
    let hit = app
        .hits
        .borrow()
        .iter()
        .rev()
        .find(|(r, _)| m.column >= r.x && m.column < r.x + r.width && m.row >= r.y && m.row < r.y + r.height)
        .copied();
    let (rect, hit) = hit?;
    // While a text box is open, only its own buttons (Enter/Esc) respond.
    if !matches!(app.input, Input::None) && !matches!(hit, Hit::Key(KeyCode::Enter | KeyCode::Esc)) {
        return None;
    }
    let row = (m.row - rect.y) as usize;
    match hit {
        Hit::Key(code) => Some(press(code)),
        Hit::Link(i) => {
            let url = app.links.borrow().get(i).cloned();
            if let Some(u) = url {
                app.status = match open_url(&u) {
                    Ok(()) => format!("Opened {u}"),
                    Err(e) => format!("Couldn't open {u}: {e}"),
                };
            }
            None
        }
        Hit::Site(i) => {
            if matches!(app.screen, Screen::Pick) && i < app.sites.len() && app.job.is_none() {
                app.site_idx = i;
            }
            None
        }
        Hit::PickList { offset } => {
            let i = offset + row;
            if app.job.is_some() || !matches!(app.pick_rows().get(i), Some(PickRow::Sheet(_))) {
                return None;
            }
            if app.pick_sel == i {
                // Clicking the selected sheet opens it, like Enter.
                return Some(press(KeyCode::Enter));
            }
            app.pick_sel = i;
            None
        }
        Hit::Findings { offset, check_cols } => {
            let Screen::Review(r) = &mut app.screen else { return None };
            let i = offset + row;
            if i >= r.entries.len() {
                return None;
            }
            r.sel = i;
            // The box toggles the fix, like Space.
            (m.column - rect.x < check_cols).then(|| press(KeyCode::Char(' ')))
        }
        Hit::Terms { offset, check_cols } => {
            let Screen::Review(r) = &mut app.screen else { return None };
            let i = offset + row;
            if i >= r.term_rows().len() {
                return None;
            }
            if m.column - rect.x < check_cols {
                r.term_sel = i;
                return Some(press(KeyCode::Char(' ')));
            }
            // Clicking the selected row opens its link, like o.
            if r.term_sel == i {
                return Some(press(KeyCode::Char('o')));
            }
            r.term_sel = i;
            None
        }
        Hit::Scroll => None,
    }
}

enum Flow {
    Continue,
    Quit,
    Ingest(PathBuf),
}

fn handle(app: &mut App, term: &mut Term, key: KeyEvent) -> Result<Flow> {
    // Text input takes every key until Enter/Esc.
    match &mut app.input {
        Input::None => {}
        Input::Clean => {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    app.status = match sacda::sheets::clear_temp() {
                        Ok(0) => "Nothing to clean".into(),
                        Ok(n) => format!("Deleted {n} temporary file(s)"),
                        Err(e) => format!("Clean-up stopped: {e:#}"),
                    };
                    app.input = Input::None;
                }
                KeyCode::Esc | KeyCode::Char('n') => app.input = Input::None,
                _ => {}
            }
            return Ok(Flow::Continue);
        }
        Input::Url(buf) | Input::Folder(buf) | Input::Subset(buf) | Input::Confirm(buf) | Input::Title { buf, .. } => {
            match key.code {
                KeyCode::Esc => {
                    app.input = Input::None;
                    app.pending_input = None;
                }
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => buf.clear(),
                KeyCode::Char(c) => buf.push(c),
                KeyCode::Enter => {
                    let input = std::mem::replace(&mut app.input, Input::None);
                    match input {
                        Input::Url(url) => {
                            let url = url.trim().to_string();
                            if url.is_empty() {
                            } else if sheet_id(&url).is_none() {
                                // Keep the box open with what they typed.
                                app.status = "That isn't a Google Sheets link (docs.google.com/spreadsheets/d/...)".into();
                                app.input = Input::Url(url);
                            } else {
                                // Pull it into the list first; checking is Enter on it.
                                app.start_pull(url);
                            }
                        }
                        Input::Folder(raw) => {
                            let Some(sel) = app.selected_input() else { return Ok(Flow::Continue) };
                            if raw.trim().is_empty() {
                                app.sheets.entry(&sel).folder = None;
                                app.sheets.save();
                                app.status = "Files folder: back to the default".into();
                            } else {
                                let p = clean_path(&raw);
                                if p.is_dir() {
                                    let p = dunce::canonicalize(&p).unwrap_or(p);
                                    app.status = format!("Files folder for this sheet: {}", p.display());
                                    app.sheets.entry(&sel).folder = Some(p);
                                    app.sheets.save();
                                } else {
                                    app.status = format!("{} isn't a folder", p.display());
                                    app.input = Input::Folder(raw);
                                }
                            }
                        }
                        Input::Clean => {}
                        Input::Subset(raw) => {
                            // On the review screen it's the open sheet, and it's re-read straight away.
                            let reviewing = match &app.screen {
                                Screen::Review(r) => Some(r.input_name.clone()),
                                _ => None,
                            };
                            let Some(sel) = reviewing.clone().or_else(|| app.selected_input()) else { return Ok(Flow::Continue) };
                            match sacda::subset::Subset::parse(&raw) {
                                Ok(s) => {
                                    app.status = match &s {
                                        Some(s) => format!("Only {} - Enter to check", s.describe()),
                                        None => "The whole sheet".into(),
                                    };
                                    let changed = app.sheets.get(&sel).map(|s| s.subset.as_str()) != Some(raw.trim());
                                    app.sheets.entry(&sel).subset = raw.trim().to_string();
                                    app.sheets.save();
                                    if reviewing.is_some() && changed {
                                        if app.job.is_some() {
                                            app.reread_after_job = true;
                                            app.status = "Rows saved - they're used when this check finishes".into();
                                        } else {
                                            app.use_subset();
                                        }
                                    }
                                }
                                Err(e) => {
                                    app.status = format!("{e:#}");
                                    app.input = Input::Subset(raw);
                                }
                            }
                        }
                        Input::Confirm(typed) => {
                            if typed.trim() == app.site() {
                                app.confirmed_site = Some(app.site().to_string());
                                if let Some(p) = app.pending_input.take() {
                                    app.start_open(p);
                                }
                            } else {
                                app.status = format!("Not confirmed - type '{}' exactly", app.site());
                                app.pending_input = None;
                            }
                        }
                        Input::Title { id, buf } => {
                            if !buf.trim().is_empty() {
                                if let Screen::Review(r) = &mut app.screen {
                                    r.accepted.push(Fix::SetTitle { id, title: buf.trim().to_string() });
                                    r.changes += 1;
                                    app.status = "Title set - press c to re-check".into();
                                }
                            }
                        }
                        Input::None => {}
                    }
                }
                _ => {}
            }
            return Ok(Flow::Continue);
        }
    }

    // Esc while checking cancels the check, wherever you are.
    if key.code == KeyCode::Esc && app.job.is_some() {
        app.cancel_job();
        return Ok(Flow::Continue);
    }

    match &mut app.screen {
        Screen::Pick => {
            if app.job.is_some() {
                // The sheet is being opened; only quitting or cancelling.
                if key.code == KeyCode::Char('q') {
                    return Ok(Flow::Quit);
                }
                return Ok(Flow::Continue);
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(Flow::Quit),
                KeyCode::Up | KeyCode::Char('k') => app.pick_move(-1),
                KeyCode::Down | KeyCode::Char('j') => app.pick_move(1),
                KeyCode::PageUp => app.pick_move(-10),
                KeyCode::PageDown => app.pick_move(10),
                KeyCode::Char('x') => {
                    if let Some(sel) = app.selected_input().filter(|s| source::is_url(s)) {
                        app.sheets.remove(&sel);
                        app.sheets.save();
                        app.status = "Removed the link from the list".into();
                        app.pick_sel = app.pick_sel.min(app.pick_rows().len().saturating_sub(1));
                        if app.selected_input().is_none() {
                            app.pick_move(-1);
                            if app.selected_input().is_none() {
                                app.pick_first();
                            }
                        }
                    }
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    app.site_idx = (app.site_idx + app.sites.len() - 1) % app.sites.len()
                }
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => app.site_idx = (app.site_idx + 1) % app.sites.len(),
                KeyCode::Char('u') => app.input = Input::Url(String::new()),
                KeyCode::Char('f') => match app.selected_input() {
                    Some(sel) => {
                        let now = app.sheets.get(&sel).and_then(|s| s.folder.as_ref()).map(|p| p.display().to_string());
                        app.input = Input::Folder(now.unwrap_or_default());
                    }
                    None => app.status = "Pick a sheet first, or paste a link with u".into(),
                },
                KeyCode::Char('c') => app.input = Input::Clean,
                KeyCode::Char('s') => match app.selected_input() {
                    Some(sel) => {
                        let now = app.sheets.get(&sel).map(|s| s.subset.clone()).unwrap_or_default();
                        app.input = Input::Subset(now);
                    }
                    None => app.status = "Pick a sheet first, or paste a link with u".into(),
                },
                KeyCode::Char('m') => match app.selected_input() {
                    Some(sel) => {
                        let on = !app.by_identifier(Some(&sel));
                        app.sheets.entry(&sel).files_by_identifier = on;
                        app.sheets.save();
                        app.status = if on {
                            "File names: <identifier>.<ext> for every item - the file column is ignored".into()
                        } else {
                            "File names: from the file column; rows without one use <identifier>.<ext>".into()
                        };
                    }
                    None => app.status = "Pick a sheet first, or paste a link with u".into(),
                },
                KeyCode::Enter => {
                    if let Some(p) = app.selected_input() {
                        if app.needs_confirm() {
                            app.pending_input = Some(p);
                            app.input = Input::Confirm(String::new());
                        } else {
                            app.start_open(p);
                        }
                    }
                }
                _ => {}
            }
        }
        Screen::Review(r) => {
            let action = review_key(r, key);
            return perform(app, term, action);
        }
        Screen::Written { dir, .. } => match key.code {
            KeyCode::Char('i') => return Ok(Flow::Ingest(dir.clone())),
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => return Ok(Flow::Quit),
            _ => {}
        },
    }
    Ok(Flow::Continue)
}

/// What a key on the review screen asks for, once the review itself has been updated.
enum Action {
    None,
    Quit,
    Back,
    Recheck,
    Reload,
    Open(Option<String>),
    Title(Option<String>),
    Write { ingest: bool },
    /// A message for the status line.
    Say(String),
    /// Change which rows are used, then re-read.
    Subset,
}

fn review_key(r: &mut Review, key: KeyEvent) -> Action {
    match key.code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Esc => Action::Back,
        KeyCode::Char('1') => {
            r.tab = Tab::Findings;
            Action::None
        }
        KeyCode::Char('2') => {
            r.tab = Tab::Tree;
            r.scroll = 0;
            Action::None
        }
        KeyCode::Char('3') => {
            r.tab = Tab::Terms;
            Action::None
        }
        KeyCode::Tab => {
            r.tab = match r.tab {
                Tab::Findings => Tab::Tree,
                Tab::Tree => Tab::Terms,
                Tab::Terms => Tab::Findings,
            };
            r.scroll = 0;
            Action::None
        }
        KeyCode::Up | KeyCode::Char('k') => {
            match r.tab {
                Tab::Findings => r.sel = r.sel.saturating_sub(1),
                Tab::Terms => r.term_sel = r.term_sel.saturating_sub(1),
                Tab::Tree => r.scroll = r.scroll.saturating_sub(1),
            }
            Action::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            match r.tab {
                Tab::Findings => r.sel = (r.sel + 1).min(r.entries.len().saturating_sub(1)),
                Tab::Terms => r.term_sel = (r.term_sel + 1).min(r.term_rows().len().saturating_sub(1)),
                Tab::Tree => r.scroll = r.scroll.saturating_add(1),
            }
            Action::None
        }
        KeyCode::PageDown => {
            match r.tab {
                Tab::Findings => r.sel = (r.sel + 10).min(r.entries.len().saturating_sub(1)),
                Tab::Terms => r.term_sel = (r.term_sel + 10).min(r.term_rows().len().saturating_sub(1)),
                Tab::Tree => r.scroll = r.scroll.saturating_add(10),
            }
            Action::None
        }
        KeyCode::PageUp => {
            match r.tab {
                Tab::Findings => r.sel = r.sel.saturating_sub(10),
                Tab::Terms => r.term_sel = r.term_sel.saturating_sub(10),
                Tab::Tree => r.scroll = r.scroll.saturating_sub(10),
            }
            Action::None
        }
        KeyCode::Char(' ') if r.tab == Tab::Terms => {
            toggle_link(r, None);
            Action::None
        }
        KeyCode::Char('L') if r.tab == Tab::Terms => {
            let rows = r.term_rows();
            let any_on = rows.iter().any(|(v, i)| {
                let t = &r.report.new_terms[v][*i];
                t.link.is_some() && !r.no_link.contains(&(v.clone(), t.name.clone()))
            });
            for (v, i) in rows {
                let t = &r.report.new_terms[&v][i];
                if t.link.is_some() {
                    toggle_link(r, Some((v.clone(), t.name.clone(), !any_on)));
                }
            }
            Action::None
        }
        KeyCode::Char(c @ (' ' | 'y' | 'n')) if r.tab == Tab::Findings => {
            if let Some(h) = locked_hint(r).filter(|_| c == 'n' || (c == ' ' && selected_on(r))) {
                return Action::Say(format!("\"{}\" can't be kept: {}", h.typed, sacda::precheck::locked_reason(&h.vocab)));
            }
            r.changes += decide(r, c);
            Action::None
        }
        KeyCode::Char('a') if r.tab == Tab::Findings => {
            let fixes: Vec<Fix> = r
                .report
                .findings
                .iter()
                .filter(|f| f.fix.as_ref().is_some_and(|x| r.choice(x, f.auto) == Choice::Undecided))
                .filter_map(|f| f.fix.clone())
                .collect();
            if !fixes.is_empty() {
                r.changes += fixes.len();
                r.accepted.extend(fixes);
            }
            Action::None
        }
        KeyCode::Char('o') => Action::Open(match r.tab {
            Tab::Findings => match r.entries.get(r.sel) {
                Some(Entry::Finding(i)) => r.report.findings.get(*i).and_then(|f| f.link.clone()),
                _ => None,
            },
            Tab::Terms => r
                .term_rows()
                .get(r.term_sel)
                .and_then(|(v, i)| r.report.new_terms[v][*i].link.as_ref().map(|l| l.uri.clone())),
            Tab::Tree => None,
        }),
        KeyCode::Char('t') => Action::Title(
            r.report
                .tree
                .roots()
                .iter()
                .find(|n| n.title.is_empty() && !r.report.attach.contains_key(&n.id))
                .map(|n| n.id.clone()),
        ),
        KeyCode::Char('c') => Action::Recheck,
        KeyCode::Char('r') => Action::Reload,
        KeyCode::Char('s') => Action::Subset,
        KeyCode::Char('w') => Action::Write { ingest: false },
        KeyCode::Char('i') => Action::Write { ingest: true },
        _ => Action::None,
    }
}

fn perform(app: &mut App, term: &mut Term, action: Action) -> Result<Flow> {
    match action {
        Action::None => {}
        Action::Say(m) => app.status = m,
        Action::Quit => return Ok(Flow::Quit),
        Action::Back => {
            if let Some(j) = &app.job {
                j.progress.cancel();
                app.job = None;
            }
            app.screen = Screen::Pick;
            app.session = None;
            app.status.clear();
        }
        Action::Recheck => app.start_recheck(false),
        Action::Subset => {
            if let Screen::Review(r) = &app.screen {
                let now = app.sheets.get(&r.input_name).map(|s| s.subset.clone()).unwrap_or_default();
                app.input = Input::Subset(now);
            }
        }
        Action::Reload => {
            // Re-read the sheet and drop cached site lookups - e.g. after
            // editing the sheet or ingesting the thesaurus terms.
            app.start_recheck(true)
        }
        Action::Open(Some(url)) => {
            app.status = match open_url(&url) {
                Ok(()) => format!("Opened {url}"),
                Err(e) => format!("Couldn't open {url}: {e}"),
            }
        }
        Action::Open(None) => app.status = "No link on this row".into(),
        Action::Title(Some(id)) => app.input = Input::Title { id, buf: String::new() },
        Action::Title(None) => app.status = "Every collection already has a title".into(),
        Action::Write { ingest } => {
            if app.job.is_some() {
                app.status = "Wait for the check to finish".into();
                return Ok(Flow::Continue);
            }
            if let Screen::Review(r) = &app.screen {
                if let Some(why) = r.not_ready() {
                    app.status = format!("Not yet: {why}");
                    return Ok(Flow::Continue);
                }
            }
            busy(app, term, "Writing the bundle ...")?;
            match app.write() {
                Ok(dir) => {
                    app.status.clear();
                    if ingest {
                        return Ok(Flow::Ingest(dir));
                    }
                }
                Err(e) => app.status = format!("{e:#}"),
            }
        }
    }
    Ok(Flow::Continue)
}

/// The selected finding's term hint, when its vocabulary takes no new terms.
fn locked_hint(r: &Review) -> Option<sacda::precheck::TermHint> {
    let Some(Entry::Finding(i)) = r.entries.get(r.sel) else { return None };
    r.report.findings.get(*i)?.hint.clone().filter(|h| !h.creatable)
}

fn selected_on(r: &Review) -> bool {
    let Some(Entry::Finding(i)) = r.entries.get(r.sel) else { return false };
    let f = &r.report.findings[*i];
    f.fix.as_ref().is_some_and(|x| r.choice(x, f.auto) == Choice::On)
}

/// The same suggestion on other rows: same value, same existing term.
pub fn same_suggestion(r: &Review, i: usize) -> Vec<usize> {
    let Some(h) = r.report.findings.get(i).and_then(|f| f.hint.as_ref()) else { return vec![i] };
    r.report
        .findings
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            f.fix.is_some()
                && f.hint.as_ref().is_some_and(|o| o.typed == h.typed && o.existing == h.existing && o.vocab == h.vocab)
        })
        .map(|(j, _)| j)
        .collect()
}

/// y / n / Space on the selected entry - and, for a term suggestion, on every
/// row with the same suggestion. Returns how many fixes changed.
fn decide(r: &mut Review, key: char) -> usize {
    let Some(entry) = r.entries.get(r.sel).cloned() else { return 0 };
    let targets = match entry {
        Entry::Finding(i) => same_suggestion(r, i),
        Entry::Applied(_) => vec![],
    };
    if targets.len() > 1 {
        // Decide them together from the selected one's state.
        let Entry::Finding(i) = entry else { return 0 };
        let f = &r.report.findings[i];
        let now = r.choice(f.fix.as_ref().unwrap(), f.auto);
        let want = match key {
            'y' => Choice::On,
            'n' => Choice::Off,
            _ if now == Choice::On => Choice::Off,
            _ => Choice::On,
        };
        let mut changed = 0;
        for j in targets {
            let f = &r.report.findings[j];
            let (fix, auto) = (f.fix.clone().unwrap(), f.auto);
            if set_choice(r, &fix, auto, want) {
                changed += 1;
            }
        }
        return changed;
    }
    decide_one(r, key) as usize
}

fn set_choice(r: &mut Review, fix: &Fix, auto: bool, want: Choice) -> bool {
    if r.choice(fix, auto) == want {
        return false;
    }
    r.accepted.retain(|x| x != fix);
    r.rejected.retain(|x| x != fix);
    match want {
        Choice::On if !auto => r.accepted.push(fix.clone()),
        Choice::Off => r.rejected.push(fix.clone()),
        _ => {}
    }
    true
}

fn decide_one(r: &mut Review, key: char) -> bool {
    let Some(entry) = r.entries.get(r.sel).cloned() else { return false };
    let (fix, auto, now) = match &entry {
        Entry::Finding(i) => {
            let Some(f) = r.report.findings.get(*i) else { return false };
            let Some(fix) = f.fix.clone() else { return false };
            let now = r.choice(&fix, f.auto);
            (fix, f.auto, now)
        }
        // An applied fix is on unless it's been turned off since.
        Entry::Applied(fix) => {
            let now = if r.rejected.contains(fix) { Choice::Off } else { Choice::On };
            let auto = !r.accepted.contains(fix);
            (fix.clone(), auto, now)
        }
    };
    let want = match key {
        'y' => Choice::On,
        'n' => Choice::Off,
        _ => match now {
            Choice::On => Choice::Off,
            _ => Choice::On,
        },
    };
    if want == now {
        return false;
    }
    r.accepted.retain(|x| *x != fix);
    r.rejected.retain(|x| *x != fix);
    match want {
        Choice::On if !auto => r.accepted.push(fix),
        Choice::Off => r.rejected.push(fix),
        _ => {}
    }
    true
}

/// Space on the terms list: leave the selected term's authority link out, or
/// put it back. `force` = (vocab, name, on) sets it directly.
fn toggle_link(r: &mut Review, force: Option<(String, String, bool)>) {
    let (key, on) = match force {
        Some((v, n, on)) => ((v, n), Some(on)),
        None => {
            let Some((v, i)) = r.term_rows().get(r.term_sel).cloned() else { return };
            let t = &r.report.new_terms[&v][i];
            if t.link.is_none() {
                return;
            }
            ((v, t.name.clone()), None)
        }
    };
    let currently_on = !r.no_link.contains(&key);
    match on.unwrap_or(!currently_on) {
        true => {
            r.no_link.remove(&key);
        }
        false => {
            r.no_link.insert(key);
        }
    }
}
