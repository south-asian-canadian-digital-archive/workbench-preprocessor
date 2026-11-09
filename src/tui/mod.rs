//! The interactive screen: pick a sheet and a site, read the precheck, accept
//! or skip fixes, then write the bundle (and optionally ingest it).
//!
//! Everything shown here comes from the same `Session::settle` / `bundle::write`
//! the plain mode uses - the TUI only decides which suggested fixes are on.

mod view;

use crate::sacda::bundle;
use crate::sacda::config::Config;
use crate::sacda::precheck::{Fix, Options, Report, Session};
use crate::sacda::source::{self, Table};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::Stdout;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub struct Start {
    pub cfg: Config,
    pub input: Option<String>,
    pub site: Option<String>,
    pub sheet: Option<String>,
    pub access_dir: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub collection_title: Option<String>,
    pub accept_suggestions: bool,
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

pub enum Input {
    None,
    /// Typing a Google Sheets link on the pick screen.
    Url(String),
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
}

pub enum Screen {
    Pick,
    Review(Box<Review>),
    Written { dir: PathBuf, summary: Vec<String> },
}

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
    start: Start,
    pending_input: Option<String>,
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
            screen: Screen::Pick,
            input: Input::None,
            status: String::new(),
            session: None,
            confirmed_site: None,
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

    /// Load a sheet and run the first check (blocking; the caller has drawn a
    /// "checking..." status first).
    fn open(&mut self, input: String) -> Result<()> {
        let table = source::load(&input, self.start.sheet.as_deref(), &self.cfg.sacda.aliases)?;
        let mut session = Session::new(
            self.cfg.clone(),
            &input,
            Options {
                site: Some(self.site().to_string()),
                access_dir: self.start.access_dir.clone(),
                collection_title: self.start.collection_title.clone(),
                offline: false,
            },
        )?;
        let accepted = if self.start.accept_suggestions {
            let (_, first, _) = session.settle(&table, &[], &[]);
            first.findings.iter().filter(|f| !f.auto).filter_map(|f| f.fix.clone()).collect()
        } else {
            Vec::new()
        };
        let (fixed, report, applied) = session.settle(&table, &accepted, &[]);
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
        };
        rebuild_entries(&mut review);
        self.session = Some(session);
        self.screen = Screen::Review(Box::new(review));
        Ok(())
    }

    fn resettle(&mut self) {
        let (Some(session), Screen::Review(r)) = (self.session.as_mut(), &mut self.screen) else { return };
        let (fixed, report, applied) = session.settle(&r.original, &r.accepted, &r.rejected);
        r.fixed = fixed;
        r.report = report;
        r.applied = applied;
        rebuild_entries(r);
        r.sel = r.sel.min(r.entries.len().saturating_sub(1));
    }

    fn write(&mut self) -> Result<PathBuf> {
        let Screen::Review(r) = &self.screen else { anyhow::bail!("nothing to write") };
        let dir = self
            .start
            .out
            .clone()
            .unwrap_or_else(|| bundle::default_dir(&r.input_name, &r.report, self.site()));
        let w = bundle::write(&self.cfg, self.site(), &r.fixed, &r.report, &r.applied, &dir)?;
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

pub fn rebuild_entries(r: &mut Review) {
    r.entries = (0..r.report.findings.len()).map(Entry::Finding).collect();
    r.entries.extend(r.applied.iter().cloned().map(Entry::Applied));
}

type Term = Terminal<CrosstermBackend<Stdout>>;

fn setup() -> Result<Term> {
    enable_raw_mode()?;
    let mut out = std::io::stdout();
    execute!(out, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

fn teardown(t: &mut Term) {
    let _ = disable_raw_mode();
    let _ = execute!(t.backend_mut(), LeaveAlternateScreen);
    let _ = t.show_cursor();
}

pub fn run(start: Start) -> Result<Option<Outcome>> {
    let mut app = App::new(start);
    let mut term = setup()?;
    // Restore the terminal even if something below panics.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
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
            busy(app, term, &format!("Checking {input} against {} ...", app.site()))?;
            if let Err(e) = app.open(input) {
                app.status = format!("{e:#}");
            } else {
                app.status.clear();
            }
        }
    }

    loop {
        term.draw(|f| view::draw(f, app))?;
        let Event::Key(key) = event::read()? else { continue };
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

enum Flow {
    Continue,
    Quit,
    Ingest(PathBuf),
}

fn handle(app: &mut App, term: &mut Term, key: KeyEvent) -> Result<Flow> {
    // Text input takes every key until Enter/Esc.
    match &mut app.input {
        Input::None => {}
        Input::Url(buf) | Input::Confirm(buf) | Input::Title { buf, .. } => {
            match key.code {
                KeyCode::Esc => {
                    app.input = Input::None;
                    app.pending_input = None;
                }
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) => buf.push(c),
                KeyCode::Enter => {
                    let input = std::mem::replace(&mut app.input, Input::None);
                    match input {
                        Input::Url(url) => {
                            let url = url.trim().to_string();
                            if !url.is_empty() {
                                if app.needs_confirm() {
                                    app.pending_input = Some(url);
                                    app.input = Input::Confirm(String::new());
                                } else {
                                    open(app, term, url)?;
                                }
                            }
                        }
                        Input::Confirm(typed) => {
                            if typed.trim() == app.site() {
                                app.confirmed_site = Some(app.site().to_string());
                                if let Some(p) = app.pending_input.take() {
                                    open(app, term, p)?;
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
                                }
                                busy(app, term, "Re-checking ...")?;
                                app.resettle();
                                app.status.clear();
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

    match &mut app.screen {
        Screen::Pick => match key.code {
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
            KeyCode::Enter => {
                if let Some(p) = app.files.get(app.pick_sel) {
                    let p = p.to_string_lossy().to_string();
                    if app.needs_confirm() {
                        app.pending_input = Some(p);
                        app.input = Input::Confirm(String::new());
                    } else {
                        open(app, term, p)?;
                    }
                }
            }
            _ => {}
        },
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
    Resettle,
    Reload,
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
            r.scroll = 0;
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
            if r.tab == Tab::Findings {
                r.sel = r.sel.saturating_sub(1)
            } else {
                r.scroll = r.scroll.saturating_sub(1)
            }
            Action::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if r.tab == Tab::Findings {
                r.sel = (r.sel + 1).min(r.entries.len().saturating_sub(1))
            } else {
                r.scroll = r.scroll.saturating_add(1)
            }
            Action::None
        }
        KeyCode::PageDown => {
            r.sel = (r.sel + 10).min(r.entries.len().saturating_sub(1));
            Action::None
        }
        KeyCode::PageUp => {
            r.sel = r.sel.saturating_sub(10);
            Action::None
        }
        KeyCode::Char(' ') if r.tab == Tab::Findings => {
            if toggle(r) {
                Action::Resettle
            } else {
                Action::None
            }
        }
        KeyCode::Char('a') if r.tab == Tab::Findings => {
            let fixes: Vec<Fix> = r
                .report
                .findings
                .iter()
                .filter(|f| !f.auto)
                .filter_map(|f| f.fix.clone())
                .filter(|f| !r.accepted.contains(f))
                .collect();
            if fixes.is_empty() {
                Action::None
            } else {
                r.accepted.extend(fixes);
                Action::Resettle
            }
        }
        KeyCode::Char('t') => Action::Title(
            r.report
                .tree
                .roots()
                .iter()
                .find(|n| n.title.is_empty() && !r.report.attach.contains_key(&n.id))
                .map(|n| n.id.clone()),
        ),
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
            app.screen = Screen::Pick;
            app.session = None;
            app.status.clear();
        }
        Action::Resettle => {
            busy(app, term, "Re-checking ...")?;
            app.resettle();
            app.status.clear();
        }
        Action::Title(Some(id)) => app.input = Input::Title { id, buf: String::new() },
        Action::Title(None) => app.status = "Every collection already has a title".into(),
        Action::Reload => {
            // Re-read the sheet and drop cached site lookups - e.g. after
            // editing the sheet or ingesting the thesaurus terms.
            busy(app, term, "Re-reading the sheet and the site ...")?;
            let Screen::Review(r) = &mut app.screen else { return Ok(Flow::Continue) };
            let input = r.input_name.clone();
            let table = source::load(&input, app.start.sheet.as_deref(), &app.cfg.sacda.aliases);
            let session = Session::new(
                app.cfg.clone(),
                &input,
                Options {
                    site: Some(app.sites[app.site_idx].clone()),
                    access_dir: app.start.access_dir.clone(),
                    collection_title: app.start.collection_title.clone(),
                    offline: false,
                },
            );
            match (table, session) {
                (Ok(t), Ok(sess)) => {
                    r.original = t;
                    app.session = Some(sess);
                    app.resettle();
                    app.status.clear();
                }
                (Err(e), _) | (_, Err(e)) => app.status = format!("{e:#}"),
            }
        }
        Action::Write { ingest } => {
            let blocked = matches!(&app.screen, Screen::Review(r) if r.report.blocked());
            if blocked {
                app.status = "Fix the errors first - nothing is written while there are errors".into();
                return Ok(Flow::Continue);
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

fn open(app: &mut App, term: &mut Term, input: String) -> Result<()> {
    busy(app, term, &format!("Checking {input} against {} ...", app.site()))?;
    match app.open(input) {
        Ok(()) => app.status.clear(),
        Err(e) => app.status = format!("{e:#}"),
    }
    Ok(())
}

/// Space on the selected entry: accept/un-accept a suggestion, or turn an
/// applied fix off/on. Returns whether anything changed.
fn toggle(r: &mut Review) -> bool {
    let Some(entry) = r.entries.get(r.sel).cloned() else { return false };
    match entry {
        Entry::Finding(i) => {
            let Some(f) = r.report.findings.get(i) else { return false };
            let Some(fix) = f.fix.clone() else { return false };
            if let Some(pos) = r.rejected.iter().position(|x| *x == fix) {
                r.rejected.remove(pos);
            } else if let Some(pos) = r.accepted.iter().position(|x| *x == fix) {
                r.accepted.remove(pos);
            } else {
                r.accepted.push(fix);
            }
            true
        }
        Entry::Applied(fix) => {
            if let Some(pos) = r.accepted.iter().position(|x| *x == fix) {
                r.accepted.remove(pos);
            } else {
                r.rejected.push(fix);
            }
            true
        }
    }
}
