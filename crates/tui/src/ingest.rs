//! Running a bundle through Workbench in the terminal: a full screen with
//! Workbench's output in a pane that scrolls on its own (wheel, arrows,
//! PgUp/PgDn), and the progress bar pinned to the bottom row whatever is
//! being looked at. When it ends, a short summary is printed to the terminal
//! and the whole output is saved next to the step logs.

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event as CtEvent, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::{Frame, Terminal};
use sacda::config::Config;
use sacda::progress::Progress;
use sacda::run::{self, Event};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// What the worker running Workbench sends the screen.
enum Msg {
    Stage { label: String, index: usize, of: usize, total: usize, checking: bool },
    Line(String),
    Record,
    Paused(u64),
    Checksums { matched: usize, mismatched: usize, algorithm: String, file: PathBuf },
    Done(Result<(), String>),
}

/// `organise ingest`, the screen's `i`, and `--ingest`: the ingest screen
/// when there's a terminal to draw it in, plain lines otherwise (logs, pipes).
pub fn ingest(cfg: &Config, dir: &Path, check_only: bool) -> anyhow::Result<()> {
    if !std::io::stdout().is_terminal() {
        return run::run_bundle(cfg, dir, check_only);
    }
    let job = Arc::new(Progress::default());
    let (tx, rx) = mpsc::channel();
    {
        let (cfg, dir, job) = (cfg.clone(), dir.to_path_buf(), job.clone());
        std::thread::spawn(move || {
            let _job = sacda::progress::enter(job);
            let send = |m: Msg| {
                let _ = tx.send(m);
            };
            let result = run::run_bundle_with(&cfg, &dir, check_only, &mut |e| match e {
                Event::Stage { label, index, of, total, checking } => {
                    send(Msg::Stage { label: label.to_string(), index, of, total, checking })
                }
                Event::Line(l) => send(Msg::Line(l.to_string())),
                Event::Record => send(Msg::Record),
                Event::Paused { seconds } => send(Msg::Paused(seconds)),
                Event::Checksums { matched, mismatched, algorithm, file } => {
                    send(Msg::Checksums { matched, mismatched, algorithm: algorithm.to_string(), file: file.to_path_buf() })
                }
            });
            send(Msg::Done(result.map_err(|e| format!("{e:#}"))));
        });
    }

    let mut screen = Screen::new(dir, &cfg.g_root, &cfg.workbench_root());
    {
        let _guard = TermGuard::enter()?;
        let mut term = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
        screen.run(&mut term, &rx, &job)?;
    }
    // The terminal is back: leave a summary in its scrollback.
    let saved = screen.save();
    for l in screen.summary() {
        println!("{l}");
    }
    match &saved {
        Ok(p) => println!("Full output: {}", p.display()),
        Err(e) => println!("(couldn't save the full output: {e})"),
    }
    match screen.save_nodes() {
        Ok(Some(p)) => println!("New nodes (identifier, title, link): {}", p.display()),
        Ok(None) => {}
        Err(e) => println!("(couldn't save the list of new nodes: {e})"),
    }
    match screen.done.take() {
        Some(Ok(())) => Ok(()),
        Some(Err(e)) => Err(anyhow::anyhow!(e)),
        None => Err(anyhow::anyhow!("the ingest stopped without finishing")),
    }
}

/// Raw mode + the alternate screen while it lives, restored on drop - also
/// when something panics on the way out.
struct TermGuard;

impl TermGuard {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        execute!(std::io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        Ok(Self)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), DisableMouseCapture, LeaveAlternateScreen, crossterm::cursor::Show);
    }
}

/// One Workbench run's numbers, for the stats at the end.
struct StepStat {
    label: String,
    checking: bool,
    took: Option<Duration>,
    started: Instant,
    created: usize,
    skipped: usize,
    failed: usize,
    media_failed: usize,
}

/// A node Workbench created, or found already there.
struct NodeLink {
    id: String,
    title: String,
    url: String,
    new: bool,
}

/// What one line of Workbench output says about a record.
#[derive(Debug, PartialEq)]
enum Said {
    Created,
    Skipped,
    Failed,
    MediaFailed,
}

fn said(line: &str) -> Option<Said> {
    let t = line.trim_start().trim_start_matches(['+', '-', ' ']);
    let t = t.strip_prefix("Error: ").unwrap_or(t);
    if t.starts_with("Media for ") && t.contains(" not created") {
        Some(Said::MediaFailed)
    } else if (t.starts_with("Node for ") || t.starts_with("Term ")) && t.contains(" not created") {
        Some(Said::Failed)
    } else if (t.starts_with("Node for ") && t.contains(" created at ")) || (t.starts_with("Term \"") && t.ends_with(" created.")) {
        Some(Said::Created)
    } else if t.contains("skipping it.") || (t.starts_with("Term \"") && t.contains(" already exists")) {
        Some(Said::Skipped)
    } else {
        None
    }
}

/// The node in a "created at" or "already ingested at (url)" line.
fn node_link(line: &str) -> Option<NodeLink> {
    let t = line.trim_start().trim_start_matches(['+', '-', ' ']);
    let url_from = |s: &str| -> Option<String> {
        let start = s.find("http")?;
        let u: String = s[start..].chars().take_while(|c| !c.is_whitespace() && *c != ')').collect();
        Some(u.trim_end_matches(['.', ',']).to_string())
    };
    if let Some(rest) = t.strip_prefix("Node for \"") {
        let (title, rest) = rest.split_once("\" (record ")?;
        let (id, rest) = rest.split_once(')')?;
        let url = url_from(rest.split_once(" created at ")?.1)?;
        return Some(NodeLink { id: id.to_string(), title: title.to_string(), url, new: true });
    }
    for lead in ["Item in row ", "File "] {
        if let Some(rest) = t.strip_prefix(lead) {
            if t.contains("skipping it.") {
                let id = rest.split_whitespace().next()?.to_string();
                return Some(NodeLink { id, title: String::new(), url: url_from(rest)?, new: false });
            }
        }
    }
    None
}

struct Stage {
    label: String,
    index: usize,
    of: usize,
    total: usize,
    checking: bool,
    started: Instant,
}

struct Screen {
    bundle: String,
    lines: Vec<String>,
    /// Wrapped rows scrolled up from the bottom; 0 = following the output.
    back: usize,
    stage: Option<Stage>,
    done_records: usize,
    started: Instant,
    finished: Option<Duration>,
    done: Option<Result<(), String>>,
    /// Asked "stop the ingest?" and waiting for y/n.
    confirm_stop: bool,
    stopping: bool,
    /// Rows in the log pane at the last draw, for PgUp/PgDn.
    page: usize,
    /// The bundle, for the error report.
    dir: PathBuf,
    /// g/ (for logs/organise/) and the Workbench checkout (for the report).
    g_root: PathBuf,
    workbench_root: PathBuf,
    /// The error panel is open (it opens when Workbench fails; `v` hides it).
    show_error: bool,
    /// What happened to the last `c`.
    note: Option<String>,
    /// Where the last Stage started in `lines`, so a report has that step's output.
    stage_from: usize,
    stats: Vec<StepStat>,
    nodes: Vec<NodeLink>,
    /// The bundle's collections (their identifiers), for the links at the end.
    collections: Vec<String>,
    /// The stats panel is open (it opens when the ingest finishes).
    show_stats: bool,
    /// File checksums: (matched, mismatched, algorithm, where they're saved).
    checksums: Option<(usize, usize, String, PathBuf)>,
    /// When the step's last record came through (None: none yet this step).
    last_record: Option<Instant>,
    /// Workbench's adaptive pauses: how many, how long in all, and the last one.
    pauses: usize,
    paused_secs: u64,
    last_pause: Option<Instant>,
}

const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

impl Screen {
    fn new(dir: &Path, g_root: &Path, workbench_root: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            g_root: g_root.to_path_buf(),
            workbench_root: workbench_root.to_path_buf(),
            show_error: false,
            note: None,
            stage_from: 0,
            stats: Vec::new(),
            nodes: Vec::new(),
            collections: sacda::bundle::Manifest::read(dir).map(|m| m.collections).unwrap_or_default(),
            show_stats: false,
            checksums: None,
            last_record: None,
            pauses: 0,
            paused_secs: 0,
            last_pause: None,
            bundle: dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "bundle".into()),
            lines: Vec::new(),
            back: 0,
            stage: None,
            done_records: 0,
            started: Instant::now(),
            finished: None,
            done: None,
            confirm_stop: false,
            stopping: false,
            page: 10,
        }
    }

    fn run(&mut self, term: &mut Terminal<CrosstermBackend<std::io::Stdout>>, rx: &mpsc::Receiver<Msg>, job: &Progress) -> anyhow::Result<()> {
        loop {
            while let Ok(m) = rx.try_recv() {
                self.take(m);
            }
            term.draw(|f| self.draw(f))?;
            if !event::poll(Duration::from_millis(120))? {
                continue;
            }
            match event::read()? {
                CtEvent::Key(k) if k.kind != KeyEventKind::Release => {
                    let ctrl_c = k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL);
                    if self.done.is_some() {
                        let failed = matches!(self.done, Some(Err(_)));
                        match k.code {
                            KeyCode::Char('o') => {
                                self.note = Some(match self.main_link() {
                                    Some(u) => match crate::open_url(&u) {
                                        Ok(()) => format!("Opened {u}"),
                                        Err(e) => format!("Couldn't open {u}: {e}"),
                                    },
                                    None => "No collection link in Workbench's output".into(),
                                });
                                continue;
                            }
                            KeyCode::Char('v') | KeyCode::Tab if !failed => {
                                self.show_stats = !self.show_stats;
                                continue;
                            }
                            KeyCode::Esc if self.show_stats => {
                                self.show_stats = false;
                                continue;
                            }
                            KeyCode::Char('c') if failed && !ctrl_c => {
                                self.note = Some(self.copy_report());
                                continue;
                            }
                            KeyCode::Char('v') | KeyCode::Tab if failed => {
                                self.show_error = !self.show_error;
                                continue;
                            }
                            KeyCode::Esc if self.show_error => {
                                self.show_error = false;
                                continue;
                            }
                            _ => {}
                        }
                        if ctrl_c || matches!(k.code, KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter) {
                            return Ok(());
                        }
                    } else if self.confirm_stop {
                        match k.code {
                            KeyCode::Char('y') | KeyCode::Char('Y') => {
                                self.confirm_stop = false;
                                self.stopping = true;
                                job.cancel();
                            }
                            _ => self.confirm_stop = false,
                        }
                        continue;
                    } else if ctrl_c || matches!(k.code, KeyCode::Char('q') | KeyCode::Esc) {
                        if !self.stopping {
                            self.confirm_stop = true;
                        }
                        continue;
                    }
                    match k.code {
                        KeyCode::Up | KeyCode::Char('k') => self.scroll(1),
                        KeyCode::Down | KeyCode::Char('j') => self.scroll(-1),
                        KeyCode::PageUp => self.scroll(self.page.max(1) as isize),
                        KeyCode::PageDown | KeyCode::Char(' ') => self.scroll(-(self.page.max(1) as isize)),
                        KeyCode::Home | KeyCode::Char('g') => self.back = usize::MAX / 2,
                        KeyCode::End | KeyCode::Char('G') => self.back = 0,
                        _ => {}
                    }
                }
                CtEvent::Mouse(m) => match m.kind {
                    MouseEventKind::ScrollUp => self.scroll(3),
                    MouseEventKind::ScrollDown => self.scroll(-3),
                    _ => {}
                },
                _ => {}
            }
        }
    }

    fn take(&mut self, m: Msg) {
        match m {
            Msg::Stage { label, index, of, total, checking } => {
                self.last_record = None;
                self.done_records = 0;
                self.stage_from = self.lines.len();
                self.close_step();
                self.stats.push(StepStat {
                    label: label.trim_start_matches("check ").to_string(),
                    checking,
                    took: None,
                    started: Instant::now(),
                    created: 0,
                    skipped: 0,
                    failed: 0,
                    media_failed: 0,
                });
                self.stage = Some(Stage { label, index, of, total, checking, started: Instant::now() });
            }
            Msg::Line(l) => {
                // Scrolled up: keep the same text in view as lines arrive.
                if self.back > 0 {
                    self.back += 1;
                }
                if let Some(s) = self.stats.last_mut().filter(|s| !s.checking) {
                    match said(&l) {
                        Some(Said::Created) => s.created += 1,
                        Some(Said::Skipped) => s.skipped += 1,
                        Some(Said::Failed) => s.failed += 1,
                        Some(Said::MediaFailed) => s.media_failed += 1,
                        None => {}
                    }
                    if let Some(n) = node_link(&l) {
                        self.nodes.push(n);
                    }
                }
                self.lines.push(clean(&l));
            }
            Msg::Record => {
                self.done_records += 1;
                self.last_record = Some(Instant::now());
            }
            Msg::Paused(s) => {
                self.pauses += 1;
                self.paused_secs += s;
                self.last_pause = Some(Instant::now());
            }
            Msg::Checksums { matched, mismatched, algorithm, file } => {
                let (m, b) = self.checksums.as_ref().map(|c| (c.0, c.1)).unwrap_or((0, 0));
                self.checksums = Some((m + matched, b + mismatched, algorithm, file));
            }
            Msg::Done(r) => {
                self.finished = Some(self.started.elapsed());
                self.close_step();
                self.show_error = r.is_err();
                self.show_stats = r.is_ok() && self.stats.iter().any(|s| !s.checking);
                self.done = Some(r);
                self.confirm_stop = false;
            }
        }
    }

    fn scroll(&mut self, by: isize) {
        self.back = if by < 0 { self.back.saturating_sub(by.unsigned_abs()) } else { self.back.saturating_add(by as usize) };
    }

    fn draw(&mut self, f: &mut Frame) {
        let [log, bar, keys] = Layout::vertical([Constraint::Min(3), Constraint::Length(1), Constraint::Length(1)]).areas(f.area());
        let inner_w = log.width.saturating_sub(2).max(1) as usize;
        let inner_h = log.height.saturating_sub(2) as usize;
        self.page = inner_h.saturating_sub(1);

        // The rows on screen, walking back from the end: only what's visible
        // (plus what's scrolled past) is wrapped.
        let mut rows: Vec<(String, bool)> = Vec::new();
        let want = self.back.saturating_add(inner_h);
        for l in self.lines.iter().rev() {
            let err = is_error(l);
            let mut wrapped = wrap(l, inner_w);
            while let Some(r) = wrapped.pop() {
                rows.push((r, err));
            }
            if rows.len() >= want {
                break;
            }
        }
        // Can't scroll past the top.
        let max_back = rows.len().saturating_sub(inner_h);
        if self.back > max_back {
            self.back = max_back;
        }
        let shown: Vec<Line> = rows
            .iter()
            .skip(self.back)
            .take(inner_h)
            .rev()
            .map(|(r, err)| if *err { Line::from(Span::styled(r.clone(), Style::new().fg(Color::Red))) } else { Line::from(r.clone()) })
            .collect();
        let title = if self.back > 0 {
            format!(" {} - scrolled up, End to follow the output ", self.bundle)
        } else {
            format!(" {} - Workbench output ", self.bundle)
        };
        f.render_widget(Paragraph::new(shown).block(Block::bordered().title(title)), log);
        if self.show_error {
            if let Some(Err(e)) = &self.done {
                self.draw_error(f, log, e);
            }
        } else if self.show_stats {
            self.draw_stats(f, log);
        }

        // The bar.
        let tick = SPIN[(self.started.elapsed().as_millis() / 120) as usize % SPIN.len()];
        let total = fmt_dur(self.finished.unwrap_or_else(|| self.started.elapsed()));
        match (&self.done, &self.stage) {
            (Some(Ok(())), _) => f.render_widget(
                Paragraph::new(Line::from(vec![Span::styled("✓ Finished", Style::new().fg(Color::Green).bold()), Span::raw(format!("  in {total}"))])),
                bar,
            ),
            (Some(Err(e)), _) => f.render_widget(
                Paragraph::new(Line::from(vec![Span::styled("✗ Stopped: ", Style::new().fg(Color::Red).bold()), Span::raw(e.clone())])),
                bar,
            ),
            (None, Some(s)) if !s.checking && s.total > 0 => {
                let pos = self.done_records.min(s.total);
                let el = s.started.elapsed();
                let eta = eta_text(s.started, self.last_record, pos, s.total);
                let label = format!("{tick} Step {}/{}  {}  {pos}/{}  {}{eta}{}  (total {total})", s.index, s.of, s.label, s.total, fmt_dur(el), self.pause_note());
                f.render_widget(Paragraph::new(progress_line(&label, pos as f64 / s.total as f64, bar.width)), bar);
            }
            (None, Some(s)) => {
                let what = if s.checking { format!("Workbench is checking {}", s.label.trim_start_matches("check ")) } else { s.label.clone() };
                f.render_widget(
                    Paragraph::new(format!("{tick} Step {}/{}  {what}  {}  (total {total})", s.index, s.of, fmt_dur(s.started.elapsed()))).fg(Color::Cyan),
                    bar,
                );
            }
            (None, None) => f.render_widget(Paragraph::new(format!("{tick} Starting  {total}")).fg(Color::Cyan), bar),
        }

        // Keys.
        let hint = if self.confirm_stop {
            Line::from(Span::styled(
                "Stop the ingest? Workbench is stopped mid-step; what it already created stays on the site. y = stop, any other key = carry on",
                Style::new().fg(Color::Yellow).bold(),
            ))
        } else if self.stopping && self.done.is_none() {
            Line::from(Span::styled("Stopping Workbench ...", Style::new().fg(Color::Yellow)))
        } else if let Some(n) = &self.note {
            Line::from(Span::styled(n.clone(), Style::new().fg(Color::Green)))
        } else if matches!(self.done, Some(Err(_))) {
            Line::from(Span::styled(
                "c copy a debug report (error, traceback, step config) · v show/hide the error · ↑↓ wheel scroll · q close",
                Style::new().fg(Color::DarkGray),
            ))
        } else if self.done.is_some() {
            Line::from(Span::styled(
                "o open the collection · v show/hide the stats · ↑↓ wheel scroll · q/Enter close (a summary stays in the terminal)",
                Style::new().fg(Color::DarkGray),
            ))
        } else {
            Line::from(Span::styled("↑↓ PgUp/PgDn wheel scroll · End follow · q stop the ingest", Style::new().fg(Color::DarkGray)))
        };
        f.render_widget(Paragraph::new(hint), keys);
    }

    fn close_step(&mut self) {
        if let Some(s) = self.stats.last_mut().filter(|s| s.took.is_none()) {
            s.took = Some(s.started.elapsed());
        }
    }

    /// Each collection's page: created now, or already there.
    fn collection_links(&self) -> Vec<(String, String, bool)> {
        self.collections
            .iter()
            .filter_map(|c| self.nodes.iter().find(|n| &n.id == c).map(|n| (c.clone(), n.url.clone(), n.new)))
            .collect()
    }

    /// What `o` opens: the first collection, else the first new node.
    fn main_link(&self) -> Option<String> {
        self.collection_links().into_iter().next().map(|c| c.1).or_else(|| self.nodes.iter().find(|n| n.new).map(|n| n.url.clone()))
    }

    /// The numbers: per step, then totals and links.
    /// On the bar: a pause just now, or how many there were.
    fn pause_note(&self) -> String {
        match self.last_pause {
            Some(t) if t.elapsed() < Duration::from_secs(20) => {
                format!("  · site slow: Workbench paused ({} so far, {}s)", self.pauses, self.paused_secs)
            }
            _ if self.pauses > 0 => format!("  · {} pause(s)", self.pauses),
            _ => String::new(),
        }
    }

    fn stats_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let runs: Vec<_> = self.stats.iter().filter(|s| !s.checking).collect();
        let checks: Duration = self.stats.iter().filter(|s| s.checking).filter_map(|s| s.took).sum();
        let w = runs.iter().map(|s| s.label.chars().count()).max().unwrap_or(4).max(4);
        out.push(format!("{:<w$}  {:>8}  {:>7}  {:>7}  {:>6}", "Step", "Time", "Created", "Skipped", "Failed"));
        for s in &runs {
            let failed = if s.media_failed > 0 { format!("{} (+{} media)", s.failed, s.media_failed) } else { s.failed.to_string() };
            out.push(format!(
                "{:<w$}  {:>8}  {:>7}  {:>7}  {:>6}",
                s.label,
                fmt_dur(s.took.unwrap_or_default()),
                s.created,
                s.skipped,
                failed
            ));
        }
        let total = self.finished.unwrap_or_else(|| self.started.elapsed());
        let created: usize = runs.iter().map(|s| s.created).sum();
        let new_nodes = self.nodes.iter().filter(|n| n.new).count();
        out.push(String::new());
        out.push(format!("Total {} (checks {}); {created} created", fmt_dur(total), fmt_dur(checks)));
        if new_nodes > 0 {
            let ingest: Duration = runs.iter().filter(|s| !s.label.starts_with("terms-")).filter_map(|s| s.took).sum();
            out.push(format!(
                "{new_nodes} new node(s), {:.1}s each on average",
                ingest.as_secs_f64() / new_nodes as f64
            ));
        }
        if self.pauses > 0 {
            out.push(format!(
                "Workbench paused {} time(s), {}s in all: the site answered slowly (adaptive_pause)",
                self.pauses, self.paused_secs
            ));
        }
        if let Some((m, b, algo, file)) = &self.checksums {
            out.push(format!("Checksums ({algo}): {m} matched Drupal's{} - {}", if *b > 0 { format!(", {b} MISMATCHED") } else { String::new() }, file.display()));
        }
        for (id, url, new) in self.collection_links() {
            out.push(format!("Collection {id}{}: {url}", if new { " (new)" } else { "" }));
        }
        out
    }

    fn draw_stats(&self, f: &mut Frame, over: ratatui::layout::Rect) {
        use ratatui::widgets::Clear;
        let lines = self.stats_lines();
        let w = (lines.iter().map(|l| l.chars().count()).max().unwrap_or(20) as u16 + 4).min(over.width.saturating_sub(2)).max(30);
        let h = (lines.len() as u16 + 2).min(over.height.saturating_sub(2)).max(5);
        let area = ratatui::layout::Rect { x: over.x + (over.width.saturating_sub(w)) / 2, y: over.y + 1, width: w, height: h };
        let text: Vec<Line> = lines
            .into_iter()
            .enumerate()
            .map(|(i, l)| if i == 0 { Line::from(Span::styled(l, Style::new().bold())) } else if l.starts_with("Collection ") { Line::from(Span::styled(l, Style::new().fg(Color::Cyan))) } else { Line::from(l) })
            .collect();
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(text).block(Block::bordered().title(" Ingest finished - o open the collection · v hide · q close ").border_style(Style::new().fg(Color::Green))),
            area,
        );
    }

    /// The nodes Workbench made, as a CSV next to the logs.
    fn save_nodes(&self) -> std::io::Result<Option<PathBuf>> {
        if !self.nodes.iter().any(|n| n.new) {
            return Ok(None);
        }
        let p = self.g_root.join("logs/organise").join(format!("{}-new-nodes.csv", self.bundle));
        let mut w = csv::Writer::from_path(&p)?;
        w.write_record(["identifier", "title", "url"])?;
        for n in self.nodes.iter().filter(|n| n.new) {
            w.write_record([&n.id, &n.title, &n.url])?;
        }
        w.flush()?;
        Ok(Some(p))
    }

    /// The step that was running, from the bundle's steps.
    fn failed_step(&self) -> Option<sacda::bundle::Step> {
        let label = self.stage.as_ref()?.label.trim_start_matches("check ").to_string();
        let name = label.split(" (+").next().unwrap_or(&label).trim().to_string();
        sacda::bundle::Manifest::read(&self.dir).ok()?.steps.into_iter().find(|s| s.name == name)
    }

    /// The traceback (from the last "Traceback" line), else the end of the
    /// failed step's output.
    fn traceback(&self) -> Vec<String> {
        let step = &self.lines[self.stage_from.min(self.lines.len())..];
        let from = step.iter().rposition(|l| l.contains("Traceback")).unwrap_or(step.len().saturating_sub(40));
        step[from..].iter().rev().take(200).rev().cloned().collect()
    }

    fn draw_error(&self, f: &mut Frame, over: ratatui::layout::Rect, e: &str) {
        use ratatui::widgets::{Clear, Wrap};
        let w = over.width.saturating_sub(4).max(20);
        let h = over.height.saturating_sub(2).max(5);
        let area = ratatui::layout::Rect { x: over.x + (over.width - w) / 2, y: over.y + 1, width: w, height: h };
        let mut text = vec![Line::from(Span::styled(e.to_string(), Style::new().fg(Color::Red).bold())), Line::from("")];
        if let Some(s) = self.failed_step() {
            text.push(Line::from(vec![Span::styled("Step: ", Style::new().bold()), Span::raw(format!("{}  (config {})", s.name, self.dir.join(&s.config).display()))]));
            text.push(Line::from(""));
        }
        let tb = self.traceback();
        // The end of it is what says what went wrong; keep that in view.
        let room = (h as usize).saturating_sub(text.len() + 4);
        let skip = tb.len().saturating_sub(room);
        if skip > 0 {
            text.push(Line::from(Span::styled(format!("... {skip} line(s) above - c copies all of it, v shows the whole output"), Style::new().fg(Color::DarkGray))));
        }
        for l in &tb[skip..] {
            text.push(Line::from(l.clone()));
        }
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(" Workbench stopped - c copy a debug report · v hide · q close ").border_style(Style::new().fg(Color::Red))),
            area,
        );
    }

    /// Everything someone (or an LLM) needs to work out what went wrong.
    /// No credentials: the configs only name the credentials file, and any
    /// username/password line is blanked anyway.
    fn report(&self) -> String {
        let mut r = String::new();
        let err = match &self.done {
            Some(Err(e)) => e.clone(),
            _ => String::new(),
        };
        r.push_str("I'm ingesting into an Islandora (Drupal) site with Islandora Workbench, run by `organise` (a tool that\n");
        r.push_str("turns a spreadsheet into Workbench CSVs + configs). A Workbench step failed. Explain the cause and how to fix it.\n\n");
        r.push_str(&format!("organise {} on {} {}\n", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH));
        let root = self.workbench_root.clone();
        let rev = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["log", "-1", "--format=%h %cs"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".into());
        r.push_str(&format!("Workbench: {} (commit {rev})\n", root.display()));
        if let Ok(m) = sacda::bundle::Manifest::read(&self.dir) {
            r.push_str(&format!("Site: {} ({})\n", m.site, m.host));
            r.push_str(&format!(
                "Bundle: {} - {} component(s), {} item(s), {} new term(s); steps: {}\n",
                self.dir.display(),
                m.components,
                m.items,
                m.new_terms,
                m.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        if let Some(s) = &self.stage {
            r.push_str(&format!("Failed during: step {}/{} - {}\n", s.index, s.of, s.label));
        }
        r.push_str(&format!("\n## Error\n{err}\n\n## Workbench output for that step (traceback / last lines)\n"));
        for l in self.traceback() {
            r.push_str(&l);
            r.push('\n');
        }
        let errors: Vec<_> = self.lines.iter().filter(|l| is_error(l)).collect();
        if !errors.is_empty() {
            r.push_str("\n## Every error line in the run\n");
            for l in errors.iter().rev().take(50).rev() {
                r.push_str(l);
                r.push('\n');
            }
        }
        if let Some(step) = self.failed_step() {
            let cfg_path = self.dir.join(&step.config);
            if let Ok(yml) = std::fs::read_to_string(&cfg_path) {
                r.push_str(&format!("\n## Step config ({})\n", cfg_path.display()));
                for l in yml.lines() {
                    let k = l.trim_start().to_ascii_lowercase();
                    if k.starts_with("password") || k.starts_with("username") {
                        r.push_str(&format!("{}: <redacted>\n", l.split(':').next().unwrap_or("")));
                    } else {
                        r.push_str(l);
                        r.push('\n');
                    }
                }
                // Its CSV: the header and a few rows show which columns Workbench was given.
                let csv = yml
                    .lines()
                    .find_map(|l| l.trim().strip_prefix("input_csv:"))
                    .map(|v| v.trim().trim_matches(['"', '\'']).to_string());
                if let Some(csv) = csv {
                    let p = self.dir.join(&csv);
                    if let Ok(t) = std::fs::read_to_string(&p) {
                        let n = t.lines().count().saturating_sub(1);
                        r.push_str(&format!("\n## {csv} (header + first rows of {n})\n"));
                        for l in t.lines().take(4) {
                            r.push_str(l);
                            r.push('\n');
                        }
                    }
                }
            }
        }
        r.push_str(&format!("\nFull output: {}\n", self.g_root.join("logs/organise").join(format!("{}-output.log", self.bundle)).display()));
        r
    }

    /// Copy the report to the clipboard, and save it next to the logs either way.
    fn copy_report(&self) -> String {
        let text = self.report();
        let saved = (|| -> std::io::Result<PathBuf> {
            let dir = self.g_root.join("logs/organise");
            std::fs::create_dir_all(&dir)?;
            let p = dir.join(format!("{}-error-report.txt", self.bundle));
            std::fs::write(&p, &text)?;
            Ok(p)
        })();
        let how = crate::clipboard::copy(&text);
        let at = saved.map(|p| format!(" - also saved to {}", p.display())).unwrap_or_default();
        match how {
            Ok(how) => format!("Copied the debug report ({} lines, {how}){at}", text.lines().count()),
            Err(e) => format!("Couldn't copy ({e}){at}"),
        }
    }

    /// What's left in the terminal: each step's result, the error lines, and
    /// how it ended.
    fn summary(&self) -> Vec<String> {
        let mut out = Vec::new();
        for l in &self.lines {
            let t = l.trim_start();
            if l.starts_with("Bundle for ")
                || l.starts_with("== ")
                || (l.starts_with("  ") && (t.ends_with(": ok") || t.ends_with(": FAILED")))
                || is_error(l)
                || t.starts_with("Ingest finished")
                || t.starts_with("All checks passed")
            {
                out.push(l.clone());
            }
        }
        if let Some(Err(_)) = &self.done {
            // The lines just before it stopped usually say why (a traceback's last line).
            let tail: Vec<_> = self.lines.iter().rev().take(5).rev().cloned().collect();
            let tail: Vec<_> = tail.into_iter().filter(|l| !out.contains(l)).collect();
            if !tail.is_empty() {
                out.push("--- last lines ---".into());
                out.extend(tail);
            }
        }
        if self.stats.iter().any(|s| !s.checking) {
            out.push(String::new());
            out.extend(self.stats_lines());
        }
        // A failure is printed by organise itself on the way out.
        match &self.done {
            Some(Ok(())) => out.push(format!("Done in {}.", fmt_dur(self.finished.unwrap_or_default()))),
            Some(Err(_)) => {}
            None => out.push("Closed before it finished.".into()),
        }
        out
    }

    /// Everything the screen showed, in g/logs/organise/<bundle>-output.log.
    fn save(&self) -> std::io::Result<PathBuf> {
        let dir = self.g_root.join("logs/organise");
        std::fs::create_dir_all(&dir)?;
        let p = dir.join(format!("{}-output.log", self.bundle));
        let mut text = self.lines.join("\n");
        text.push('\n');
        std::fs::write(&p, text)?;
        Ok(p)
    }
}

fn is_error(l: &str) -> bool {
    let t = l.trim_start().trim_start_matches("- ");
    t.starts_with("ERROR") || t.starts_with("Error:") || t.starts_with("error:") || t.contains("Traceback") || t.contains("Error: ")
}

/// Tabs to spaces, and no escape codes or other control characters (they'd
/// move the cursor around the screen).
fn clean(l: &str) -> String {
    let mut out = String::with_capacity(l.len());
    let mut chars = l.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                // CSI: ESC [ ... final byte in @..~
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for d in chars.by_ref() {
                        if ('@'..='~').contains(&d) {
                            break;
                        }
                    }
                }
            }
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// `l` cut into rows of at most `w` characters (an empty line is one row).
fn wrap(l: &str, w: usize) -> Vec<String> {
    let chars: Vec<char> = l.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars.chunks(w.max(1)).map(|c| c.iter().collect()).collect()
}

fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// Time left in a step, from the pace of the records done so far up to the
/// last one. A stall (a slow site, Workbench pausing) doesn't stretch the
/// estimate: past twice the usual gap it says it's waiting instead.
fn eta_text(started: Instant, last: Option<Instant>, pos: usize, total: usize) -> String {
    let Some(last) = last.filter(|_| pos > 0) else { return String::new() };
    let per = last.saturating_duration_since(started) / pos as u32;
    let since = last.elapsed();
    if since > (per * 2).max(Duration::from_secs(15)) {
        return format!(", waiting on the site {}", fmt_dur(since));
    }
    let left = per.mul_f64(total.saturating_sub(pos) as f64).saturating_sub(since);
    if left.is_zero() {
        ", almost done".into()
    } else {
        format!(", about {} left", fmt_dur(left))
    }
}

/// The progress bar with its text on it: black on cyan where it's filled,
/// white on dark grey where it isn't, so every letter stays readable as the
/// bar passes under it (a Gauge gives the label one colour, which vanishes
/// into the bar).
fn progress_line(label: &str, ratio: f64, width: u16) -> Line<'static> {
    let width = width as usize;
    let filled = ((ratio.clamp(0.0, 1.0) * width as f64).round() as usize).min(width);
    let mut text: Vec<char> = format!(" {label}").chars().collect();
    text.resize(width.max(text.len()), ' ');
    text.truncate(width);
    let done = Style::new().fg(Color::Black).bg(Color::Cyan).bold();
    let todo = Style::new().fg(Color::White).bg(Color::Indexed(238));
    let (a, b) = text.split_at(filled);
    Line::from(vec![Span::styled(a.iter().collect::<String>(), done), Span::styled(b.iter().collect::<String>(), todo)])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A screen over a bundle that isn't there, with g/ in a temp folder: no
    /// config needed, so these run on CI without g/.
    fn screen() -> Screen {
        let g = std::env::temp_dir().join("organise-ingest-tests");
        Screen::new(Path::new("ingest-x-dev"), &g, &g)
    }

    #[test]
    fn report_has_the_traceback_and_no_passwords() {
        let mut s = screen();
        s.take(Msg::Line("before".into()));
        s.take(Msg::Stage { label: "terms-sacda_thesaurus".into(), index: 3, of: 6, total: 4, checking: false });
        for l in ["OK, connection verified.", "Traceback (most recent call last)", "  frame", "KeyError: 'field_authority_link'"] {
            s.take(Msg::Line(l.into()));
        }
        s.take(Msg::Done(Err("terms-sacda_thesaurus failed".into())));
        assert!(s.show_error);
        let r = s.report();
        assert!(r.contains("KeyError: 'field_authority_link'"));
        assert!(r.contains("Failed during: step 3/6"));
        assert!(!r.contains("before\n"));
        assert_eq!(s.traceback().first().map(String::as_str), Some("Traceback (most recent call last)"));
    }

    #[test]
    fn reads_workbench_record_lines() {
        let l = r#"Node for "Wedding photo" (record 2099_01_01_001) created at https://isle.dvnl.work/node/812."#;
        assert_eq!(said(l), Some(Said::Created));
        let n = node_link(l).unwrap();
        assert_eq!((n.id.as_str(), n.title.as_str(), n.url.as_str(), n.new), ("2099_01_01_001", "Wedding photo", "https://isle.dvnl.work/node/812", true));
        let skip = "Item in row 2099_01 has already been ingested at (https://isle.dvnl.work/node/700), skipping it.";
        assert_eq!(said(skip), Some(Said::Skipped));
        let n = node_link(skip).unwrap();
        assert_eq!((n.id.as_str(), n.url.as_str(), n.new), ("2099_01", "https://isle.dvnl.work/node/700", false));
        assert_eq!(said(r#"Term "Masjids" created."#), Some(Said::Created));
        assert_eq!(said("Media for /x/a.tif not created. See log for more information."), Some(Said::MediaFailed));
        assert_eq!(said("Node for CSV record 2099_01_02 not created"), Some(Said::Failed));
        assert_eq!(said("OK, connection to Drupal verified."), None);
    }

    #[test]
    fn stats_count_each_step_and_link_the_collection() {
        let mut s = screen();
        s.collections = vec!["2099_01".into()];
        s.take(Msg::Stage { label: "check components".into(), index: 1, of: 3, total: 0, checking: true });
        s.take(Msg::Line(r#"Node for "nope" (record 1) created at https://h/node/1."#.into()));
        s.take(Msg::Stage { label: "terms-subject".into(), index: 2, of: 3, total: 2, checking: false });
        s.take(Msg::Line(r#"Term "A" created."#.into()));
        s.take(Msg::Line(r#"Term "B" already exists in vocabulary."#.into()));
        s.take(Msg::Stage { label: "components (+ items)".into(), index: 3, of: 3, total: 2, checking: false });
        s.take(Msg::Line(r#"Node for "Coll" (record 2099_01) created at https://h/node/5."#.into()));
        s.take(Msg::Line("Node for CSV record 2099_01_01 not created".into()));
        s.take(Msg::Done(Ok(())));
        assert!(s.show_stats);
        let st: Vec<_> = s.stats.iter().filter(|x| !x.checking).map(|x| (x.created, x.skipped, x.failed)).collect();
        assert_eq!(st, vec![(1, 1, 0), (1, 0, 1)]);
        assert_eq!(s.main_link().as_deref(), Some("https://h/node/5"));
        assert!(s.stats_lines().iter().any(|l| l == "Collection 2099_01 (new): https://h/node/5"));
    }

    #[test]
    fn eta_counts_down_and_says_when_its_waiting() {
        let now = Instant::now();
        let start = now - Duration::from_secs(40);
        // 4 of 10 done in 40s, the last just now: about 60s left.
        let e = eta_text(start, Some(now), 4, 10);
        assert!(e == ", about 1m00s left" || e == ", about 59s left", "{e}");
        // The same, but nothing for 30s: waiting, not "2m15s left".
        let last = now - Duration::from_secs(30);
        let start = last - Duration::from_secs(40);
        assert!(eta_text(start, Some(last), 4, 10).starts_with(", waiting on the site"));
        assert_eq!(eta_text(start, None, 0, 10), "");
    }

    #[test]
    fn pauses_show_on_the_bar_and_in_the_stats() {
        let mut s = screen();
        s.take(Msg::Paused(3));
        s.take(Msg::Paused(3));
        assert!(s.pause_note().contains("2 so far, 6s"));
        assert!(s.stats_lines().iter().any(|l| l.contains("paused 2 time(s), 6s")));
    }

    #[test]
    fn bar_text_is_split_at_the_fill() {
        let l = progress_line("Step 1/2  3/10", 0.5, 20);
        let text: String = l.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(text, " Step 1/2  3/10     ");
        assert_eq!(l.spans[0].content.chars().count(), 10);
        assert_ne!(l.spans[0].style.bg, l.spans[1].style.bg);
        // Narrower than the text: cut, never wider than the row.
        let l = progress_line("a long label that does not fit", 1.0, 8);
        assert_eq!(l.spans.iter().map(|s| s.content.chars().count()).sum::<usize>(), 8);
    }

    #[test]
    fn wraps_and_cleans() {
        assert_eq!(wrap("abcdef", 4), vec!["abcd", "ef"]);
        assert_eq!(wrap("", 4), vec![""]);
        assert_eq!(clean("\x1b[31mred\x1b[0m\tx\r"), "red    x");
    }

    #[test]
    fn scrolled_up_view_stays_put() {
        let mut s = screen();
        s.take(Msg::Line("a".into()));
        s.back = 2;
        s.take(Msg::Line("b".into()));
        assert_eq!(s.back, 3);
        s.back = 0;
        s.take(Msg::Line("c".into()));
        assert_eq!(s.back, 0);
    }

    #[test]
    fn summary_keeps_steps_and_errors() {
        let mut s = screen();
        for l in ["Bundle for dev (h): 1 component(s)", "noise", "== components ==", "  terms-subject: ok", "ERROR: node not created", "more noise"] {
            s.take(Msg::Line(l.into()));
        }
        s.take(Msg::Done(Ok(())));
        let out = s.summary();
        assert!(out.iter().any(|l| l.starts_with("Bundle for")));
        assert!(out.iter().any(|l| l == "  terms-subject: ok"));
        assert!(out.iter().any(|l| l.starts_with("ERROR")));
        assert!(!out.iter().any(|l| l.contains("noise")));
    }
}
