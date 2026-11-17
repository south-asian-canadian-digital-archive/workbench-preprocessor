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
use sacda::bundle;
use sacda::config::Config;
use sacda::precheck::{Fix, Options, Report, Session, Severity};
use sacda::progress::Progress;
use sacda::source::{self, Table};
use std::cell::RefCell;
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
    /// Typing the site name to confirm a protected site.
    Confirm(String),
    /// Typing a title for a collection nobody described.
    Title { id: String, buf: String },
}

pub struct Review {
    pub input_name: String,
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
    Opened {
        input: String,
        table: Table,
        session: Box<Session>,
        settled: Settled,
        accepted: Vec<Fix>,
    },
    Rechecked {
        session: Box<Session>,
        settled: Settled,
        table: Option<Table>,
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
    /// Folder the access files are in (None: `access` next to the sheet).
    pub access_dir: Option<PathBuf>,
    /// Ignore the `file` column and name access files after their identifier.
    pub files_by_identifier: bool,
    start: Start,
    pending_input: Option<String>,
    /// Clickable areas from the last draw (the view fills it in).
    pub hits: RefCell<Vec<(Rect, Hit)>>,
    /// Links drawn on screen; `Hit::Link(i)` opens `links[i]`.
    pub links: RefCell<Vec<String>>,
    /// File counts per access folder, for the pick screen.
    pub folder_counts: RefCell<HashMap<PathBuf, Option<usize>>>,
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
    while let Some(d) = stack.pop() {
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
            access_dir: start.access_dir.clone(),
            files_by_identifier: start.files_by_identifier,
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

    /// The access folder a sheet would use, and whether it was chosen by hand.
    pub fn access_for(&self, input: Option<&str>) -> (PathBuf, bool) {
        if let Some(d) = &self.access_dir {
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

    pub fn folder_count(&self, dir: &Path) -> Option<usize> {
        *self.folder_counts.borrow_mut().entry(dir.to_path_buf()).or_insert_with(|| count_files(dir))
    }

    fn options(&self, input: &str) -> Options {
        Options {
            site: Some(self.site().to_string()),
            access_dir: Some(self.access_for(Some(input)).0),
            collection_title: self.start.collection_title.clone(),
            offline: false,
            files_by_identifier: self.files_by_identifier,
        }
    }

    /// Load a sheet and run the first check on the worker.
    fn start_open(&mut self, input: String) {
        let cfg = self.cfg.clone();
        let sheet = self.start.sheet.clone();
        let opts = self.options(&input);
        let accept = self.start.accept_suggestions;
        let progress = Arc::new(Progress::default());
        let p = progress.clone();
        let label = if source::is_url(&input) { "Downloading and checking the sheet".into() } else { format!("Checking {}", short_name(&input)) };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let run = || -> Result<Done, String> {
                p.say(if source::is_url(&input) { "downloading the sheet from Google" } else { "reading the sheet" });
                let table = source::load_for(&input, sheet.as_deref(), &cfg).map_err(|e| format!("{e:#}"))?;
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
                Ok(Done::Opened { input, table, session: Box::new(session), settled, accepted })
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
        let old = self.session.take();
        let progress = Arc::new(Progress::default());
        let p = progress.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let run = || -> Result<Done, String> {
                let (mut session, table) = match (reload, old) {
                    (false, Some(s)) => (s, None),
                    _ => {
                        p.say(if source::is_url(&input) { "downloading the sheet again" } else { "re-reading the sheet" });
                        let t = source::load_for(&input, sheet.as_deref(), &cfg).map_err(|e| format!("{e:#}"))?;
                        (Session::new(cfg, &input, opts).map_err(|e| format!("{e:#}"))?, Some(t))
                    }
                };
                session.job = p.clone();
                let base = table.as_ref().unwrap_or(&original);
                let settled = session.settle(base, &accepted, &rejected);
                let cancelled = p.cancelled();
                session.job = Default::default();
                Ok(Done::Rechecked { session: Box::new(session), settled, table, cancelled })
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
            Ok(Done::Opened { input, table, session, settled, accepted }) => {
                let (fixed, report, applied) = settled;
                let mut review = Review {
                    input_name: input,
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
                };
                rebuild_entries(&mut review);
                self.session = Some(*session);
                self.screen = Screen::Review(Box::new(review));
                self.status.clear();
            }
            Ok(Done::Rechecked { session, settled, table, cancelled }) => {
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
            String::new(),
            "Steps, in order:".into(),
        ];
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
    let mut app = App::new(start);
    let mut term = setup()?;
    // Restore the terminal even if something below panics.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen);
        hook(info);
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
        term.draw(|f| view::draw(f, app))?;
        // Tick while a job runs so the spinner and its progress move.
        if !event::poll(Duration::from_millis(if app.job.is_some() { 120 } else { 1000 }))? {
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
        Input::Url(b) | Input::Folder(b) | Input::Confirm(b) | Input::Title { buf: b, .. } => b.push_str(one_line.trim()),
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
            if i >= app.files.len() || app.job.is_some() {
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
        Input::Url(buf) | Input::Folder(buf) | Input::Confirm(buf) | Input::Title { buf, .. } => {
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
                            } else if app.needs_confirm() {
                                app.pending_input = Some(url);
                                app.input = Input::Confirm(String::new());
                            } else {
                                app.start_open(url);
                            }
                        }
                        Input::Folder(raw) => {
                            if raw.trim().is_empty() {
                                app.access_dir = None;
                                app.status = "Files folder: back to `access` next to the sheet".into();
                            } else {
                                let p = clean_path(&raw);
                                if p.is_dir() {
                                    let p = std::fs::canonicalize(&p).unwrap_or(p);
                                    app.status = format!("Files folder: {}", p.display());
                                    app.access_dir = Some(p);
                                } else {
                                    app.status = format!("{} isn't a folder", p.display());
                                    app.input = Input::Folder(raw);
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
                KeyCode::Up | KeyCode::Char('k') => app.pick_sel = app.pick_sel.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    app.pick_sel = (app.pick_sel + 1).min(app.files.len().saturating_sub(1))
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    app.site_idx = (app.site_idx + app.sites.len() - 1) % app.sites.len()
                }
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => app.site_idx = (app.site_idx + 1) % app.sites.len(),
                KeyCode::Char('u') => app.input = Input::Url(String::new()),
                KeyCode::Char('f') => {
                    let now = app.access_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                    app.input = Input::Folder(now);
                }
                KeyCode::Char('m') => {
                    app.files_by_identifier = !app.files_by_identifier;
                    app.status = if app.files_by_identifier {
                        "File names: <identifier>.<ext> for every item - the file column is ignored".into()
                    } else {
                        "File names: from the file column; rows without one use <identifier>.<ext>".into()
                    };
                }
                KeyCode::Enter => {
                    if let Some(p) = app.files.get(app.pick_sel) {
                        let p = p.to_string_lossy().to_string();
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
            if decide(r, c) {
                r.changes += 1;
            }
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
        KeyCode::Char('w') => Action::Write { ingest: false },
        KeyCode::Char('i') => Action::Write { ingest: true },
        _ => Action::None,
    }
}

fn perform(app: &mut App, term: &mut Term, action: Action) -> Result<Flow> {
    match action {
        Action::None => {}
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

/// y / n / Space on the selected entry. Returns whether anything changed.
fn decide(r: &mut Review, key: char) -> bool {
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
