//! Drawing. Pure functions of `App` - no state changes here, apart from
//! recording where the clickable things ended up.

use super::{sheet_id, App, Choice, Entry, Hit, Input, Review, Screen, Tab};
use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};
use ratatui::Frame;
use sacda::precheck::Severity;
use sacda::tree::Kind;

const ACCENT: Color = Color::Cyan;
const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

fn sev_style(s: Severity) -> Style {
    match s {
        Severity::Error => Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        Severity::Warning => Style::new().fg(Color::Yellow),
        Severity::Info => Style::new().fg(Color::DarkGray),
    }
}

fn link_style() -> Style {
    Style::new().fg(Color::Blue).add_modifier(Modifier::UNDERLINED)
}

fn keys(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (k, what) in pairs {
        spans.push(Span::styled(format!(" {k} "), Style::new().fg(Color::Black).bg(ACCENT)));
        spans.push(Span::raw(format!(" {what}  ")));
    }
    Line::from(spans)
}

pub fn draw(f: &mut Frame, app: &App) {
    app.hits.borrow_mut().clear();
    app.links.borrow_mut().clear();
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5), Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    header(f, app, rows[0]);
    match &app.screen {
        Screen::Pick => pick(f, app, rows[1]),
        Screen::Review(r) => review(f, app, r, rows[1]),
        Screen::Written { summary, dir } => written(f, summary, dir, rows[1]),
    }
    f.render_widget(Paragraph::new(status_line(app)), rows[2]);
    let foot = footer(app);
    register_chips(app, &foot, rows[3]);
    f.render_widget(Paragraph::new(foot), rows[3]);
    input_popup(f, app, area);
}

fn status_line(app: &App) -> Line<'static> {
    if let Some(job) = &app.job {
        let secs = job.started.elapsed().as_secs();
        let spin = SPINNER[(job.started.elapsed().as_millis() / 100) as usize % SPINNER.len()];
        let what = job.progress.message();
        let tail = if job.cancelling { "  cancelling at the next row ...".to_string() } else { "  (Esc cancels)".to_string() };
        return Line::from(vec![
            Span::styled(format!("{spin} {}", job.label), Style::new().fg(ACCENT).bold()),
            Span::raw(if what.is_empty() { String::new() } else { format!(" - {what}") }),
            Span::styled(format!("  {secs}s"), Style::new().fg(Color::DarkGray)),
            Span::styled(tail, Style::new().fg(Color::DarkGray)),
        ]);
    }
    if app.status.is_empty() {
        Line::from("")
    } else {
        Line::from(Span::styled(app.status.clone(), Style::new().fg(Color::Yellow)))
    }
}

/// The key a footer chip stands for, so it can be clicked.
fn chip_key(label: &str) -> Option<KeyCode> {
    match label.trim() {
        "Space" => Some(KeyCode::Char(' ')),
        "Enter" => Some(KeyCode::Enter),
        "Esc" => Some(KeyCode::Esc),
        "Tab" => Some(KeyCode::Tab),
        k if k.chars().count() == 1 => k.chars().next().map(KeyCode::Char),
        _ => None, // arrows: use the wheel or click the rows instead
    }
}

fn register_chips(app: &App, line: &Line, area: Rect) {
    let mut x = area.x;
    let mut hits = app.hits.borrow_mut();
    for pair in line.spans.chunks(2) {
        let w: u16 = pair.iter().map(|s| s.content.chars().count() as u16).sum();
        if let Some(code) = chip_key(&pair[0].content) {
            hits.push((Rect::new(x, area.y, w.min(area.right().saturating_sub(x)), 1), Hit::Key(code)));
        }
        x = x.saturating_add(w);
    }
}

fn header(f: &mut Frame, app: &App, area: Rect) {
    let site = app.site();
    let host = app.cfg.sacda.sites.get(site).map(|s| s.host.clone()).unwrap_or_default();
    let protected = app.cfg.sacda.sites.get(site).is_some_and(|s| s.confirm);
    let mut spans = vec![
        Span::styled(" organise ", Style::new().fg(Color::Black).bg(ACCENT).bold()),
        Span::raw("  site "),
        Span::styled(
            site.to_string(),
            if protected { Style::new().fg(Color::Red).bold() } else { Style::new().fg(ACCENT).bold() },
        ),
        Span::styled(format!(" {host}"), Style::new().fg(Color::DarkGray)),
    ];
    if let Screen::Review(r) = &app.screen {
        let e = r.report.count(Severity::Error);
        let w = r.report.count(Severity::Warning);
        spans.push(Span::raw(format!("   {}", short(&r.input_name, 40))));
        spans.push(Span::raw("   "));
        spans.push(Span::styled(format!("{e} error(s)"), if e > 0 { sev_style(Severity::Error) } else { Style::new().fg(Color::Green) }));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(format!("{w} warning(s)"), sev_style(Severity::Warning)));
        spans.push(Span::raw("   "));
        match (app.job.is_some(), r.not_ready()) {
            (true, _) => spans.push(Span::styled("checking ...", Style::new().fg(ACCENT))),
            (false, None) => spans.push(Span::styled("ready to write / ingest", Style::new().fg(Color::Green).bold())),
            (false, Some(why)) => spans.push(Span::styled(why, Style::new().fg(Color::Yellow))),
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::BOTTOM)),
        area,
    );
}

/// The inside of a bordered block.
fn inner(r: Rect) -> Rect {
    Rect::new(r.x + 1, r.y + 1, r.width.saturating_sub(2), r.height.saturating_sub(2))
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("...{}", s.chars().rev().take(n - 3).collect::<String>().chars().rev().collect::<String>())
    }
}

fn footer(app: &App) -> Line<'static> {
    if !matches!(app.input, Input::None) {
        return keys(&[("Enter", "ok"), ("Esc", "cancel")]);
    }
    if app.job.is_some() && !matches!(app.screen, Screen::Review(_)) {
        return keys(&[("Esc", "cancel"), ("q", "quit")]);
    }
    match &app.screen {
        Screen::Pick => keys(&[
            ("↑↓", "sheet"),
            ("←→", "site"),
            ("Enter", "check"),
            ("u", "Google Sheets link"),
            ("f", "files folder"),
            ("m", "file names"),
            ("q", "quit"),
        ]),
        Screen::Review(r) => {
            let ready = app.job.is_none() && r.not_ready().is_none();
            let mut k: Vec<(&str, &str)> = match r.tab {
                Tab::Findings => vec![("y", "use fix"), ("n", "keep as is"), ("a", "use all"), ("o", "open link")],
                Tab::Terms => vec![("Space", "link on/off"), ("L", "all links"), ("o", "open link")],
                Tab::Tree => vec![("↑↓", "scroll")],
            };
            if app.job.is_some() {
                k.push(("Esc", "cancel check"));
            } else {
                let c_label = if r.changes > 0 { "re-check (needed)" } else { "re-check" };
                k.push(("c", c_label));
                k.push(("r", "re-read sheet"));
            }
            if ready {
                k.push(("w", "write"));
                k.push(("i", "write+ingest"));
            }
            k.push(("Tab", "view"));
            if app.job.is_none() {
                k.push(("Esc", "back"));
            }
            keys(&k)
        }
        Screen::Written { .. } => keys(&[("i", "ingest now"), ("Enter", "done")]),
    }
}

fn pick(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);
    let cwd = std::env::current_dir().unwrap_or_default();
    let items: Vec<ListItem> = if app.files.is_empty() {
        vec![ListItem::new("No spreadsheets here. Press u to paste a Google Sheets link, or run organise from the folder that has the sheet.")]
    } else {
        app.files
            .iter()
            .map(|p| ListItem::new(p.strip_prefix(&cwd).unwrap_or(p).display().to_string()))
            .collect()
    };
    let mut state = ListState::default().with_selected(Some(app.pick_sel));
    f.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(" Spreadsheet "))
            .highlight_style(Style::new().bg(Color::DarkGray).bold())
            .highlight_symbol("> "),
        cols[0],
        &mut state,
    );
    app.hits.borrow_mut().push((inner(cols[0]), Hit::PickList { offset: state.offset() }));

    // Site lines start two rows into the "Where" box.
    let wi = inner(cols[1]);
    for i in 0..app.sites.len() {
        let y = wi.y + 2 + i as u16;
        if y < wi.bottom() {
            app.hits.borrow_mut().push((Rect::new(wi.x, y, wi.width, 1), Hit::Site(i)));
        }
    }
    let mut lines = vec![Line::from("Site (←→ to change):"), Line::from("")];
    for (i, s) in app.sites.iter().enumerate() {
        let cfg = &app.cfg.sacda.sites[s];
        let mark = if i == app.site_idx { "> " } else { "  " };
        let style = if i == app.site_idx { Style::new().fg(ACCENT).bold() } else { Style::new() };
        lines.push(Line::from(vec![
            Span::styled(format!("{mark}{s}"), style),
            Span::styled(format!("  {}", cfg.host), Style::new().fg(Color::DarkGray)),
            Span::styled(if cfg.confirm { "  (asks to confirm)" } else { "" }, Style::new().fg(Color::Red)),
        ]));
    }

    // Files folder for the selected sheet (or a link).
    let selected = app.files.get(app.pick_sel).map(|p| p.to_string_lossy().to_string());
    let (dir, chosen) = app.access_for(selected.as_deref());
    let count = app.folder_count(&dir);
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::raw("Files folder "), Span::styled("(f to change):", Style::new().fg(Color::DarkGray))]));
    lines.push(Line::from(Span::styled(format!("  {}", dir.display()), Style::new().fg(ACCENT))));
    lines.push(match count {
        Some(n) => Line::from(Span::styled(
            format!("  {n} file(s){}", if chosen { "" } else { " - `access` next to the sheet" }),
            Style::new().fg(Color::Green),
        )),
        None => Line::from(Span::styled(
            "  not found - press f and type or drop the folder in",
            Style::new().fg(Color::Yellow),
        )),
    });
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::raw("File names "), Span::styled("(m to change):", Style::new().fg(Color::DarkGray))]));
    lines.push(Line::from(Span::styled(
        if app.files_by_identifier {
            "  <identifier>.<ext> for every item (file column ignored)"
        } else {
            "  the file column; rows without one use <identifier>.<ext>"
        },
        Style::new().fg(ACCENT),
    )));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(Block::bordered().title(" Where ")), cols[1]);
}

fn review(f: &mut Frame, app: &App, r: &Review, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3)])
        .split(area);
    let titles = vec![
        format!(" 1 Findings ({}) ", r.report.findings.len()),
        format!(" 2 Tree ({}) ", r.report.tree.nodes.len()),
        format!(" 3 New terms ({}) ", r.report.new_terms.values().map(Vec::len).sum::<usize>()),
    ];
    let sel = match r.tab {
        Tab::Findings => 0,
        Tab::Tree => 1,
        Tab::Terms => 2,
    };
    // Tabs draw as " title " separated by "│" - one cell of divider each.
    let mut x = rows[0].x;
    for (i, t) in titles.iter().enumerate() {
        let w = t.chars().count() as u16 + 2;
        let code = KeyCode::Char(char::from(b'1' + i as u8));
        app.hits.borrow_mut().push((Rect::new(x, rows[0].y, w.min(rows[0].right().saturating_sub(x)), 1), Hit::Key(code)));
        x = x.saturating_add(w + 1);
    }
    f.render_widget(
        Tabs::new(titles).select(sel).highlight_style(Style::new().fg(ACCENT).bold().underlined()),
        rows[0],
    );
    match r.tab {
        Tab::Findings => findings(f, app, r, rows[1]),
        Tab::Tree => {
            app.hits.borrow_mut().push((rows[1], Hit::Scroll));
            tree(f, r, rows[1])
        }
        Tab::Terms => terms(f, app, r, rows[1]),
    }
}

fn mark(c: Choice) -> (&'static str, Style) {
    match c {
        Choice::On => ("[✓] ", Style::new().fg(Color::Green)),
        Choice::Off => ("[–] ", Style::new().fg(Color::DarkGray)),
        Choice::Undecided => ("[?] ", Style::new().fg(Color::Yellow).bold()),
    }
}

fn findings(f: &mut Frame, app: &App, r: &Review, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);
    let items: Vec<ListItem> = r
        .entries
        .iter()
        .map(|e| match e {
            Entry::Finding(i) => {
                let fd = &r.report.findings[*i];
                let (m, ms) = match &fd.fix {
                    Some(fix) => mark(r.choice(fix, fd.auto)),
                    None => ("    ", Style::new()),
                };
                let loc = fd.line.map(|l| format!("r{l} ")).unwrap_or_default();
                ListItem::new(Line::from(vec![
                    Span::styled(m, ms),
                    Span::styled(format!("{:<8}", fd.severity.label()), sev_style(fd.severity)),
                    Span::styled(format!("{:<8}", fd.check), Style::new().fg(Color::DarkGray)),
                    Span::raw(loc),
                    Span::raw(fd.message.clone()),
                ]))
            }
            Entry::Applied(fix) => {
                let off = r.rejected.contains(fix);
                let (m, ms) = mark(if off { Choice::Off } else { Choice::On });
                ListItem::new(Line::from(vec![
                    Span::styled(m, ms),
                    Span::styled(format!("{:<8}", if off { "off" } else { "fixed" }), Style::new().fg(Color::Green)),
                    Span::raw(format!("{:<8}", "")),
                    Span::raw(fix.describe_at(&r.original)),
                ]))
            }
        })
        .collect();
    let mut state = ListState::default().with_selected(if r.entries.is_empty() { None } else { Some(r.sel) });
    f.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(" Precheck "))
            .highlight_style(Style::new().bg(Color::DarkGray))
            .highlight_symbol("> "),
        cols[0],
        &mut state,
    );
    // "> " highlight column + "[x] " box = the first 6 cells toggle.
    app.hits.borrow_mut().push((inner(cols[0]), Hit::Findings { offset: state.offset(), check_cols: 6 }));
    detail(f, app, r, cols[1]);
}

/// An action button: `[y] Use "Garba"`, clickable like the key.
fn button(key: char, label: String, on: bool) -> Line<'static> {
    let style = if on { Style::new().fg(Color::Black).bg(Color::Green).bold() } else { Style::new().fg(Color::Black).bg(ACCENT) };
    Line::from(vec![
        Span::styled(format!(" {key} "), style),
        Span::raw(format!(" {label}{}", if on { "   ← chosen" } else { "" })),
    ])
}

fn detail(f: &mut Frame, app: &App, r: &Review, area: Rect) {
    let mut text = Text::default();
    // (key, label, chosen) buttons, drawn under the text.
    let mut buttons: Vec<(char, String, bool)> = Vec::new();
    let mut link: Option<String> = None;
    match r.entries.get(r.sel) {
        Some(Entry::Finding(i)) => {
            let fd = &r.report.findings[*i];
            text.push_line(Line::from(Span::styled(fd.severity.label().to_uppercase(), sev_style(fd.severity))));
            if let Some(l) = fd.line {
                text.push_line(format!("Sheet row {l}{}", fd.id.as_ref().map(|i| format!(" - {i}")).unwrap_or_default()));
            }
            text.push_line("");
            link = fd.link.clone();
            match (&fd.hint, &fd.fix) {
                (Some(h), Some(fix)) => {
                    let c = r.choice(fix, fd.auto);
                    text.push_line(Line::from(vec![
                        Span::raw(format!("{}: the sheet has ", h.column)),
                        Span::styled(format!("\"{}\"", h.typed), Style::new().bold()),
                        Span::raw(", which isn't on the site."),
                    ]));
                    text.push_line("");
                    text.push_line(Line::from(vec![
                        Span::raw("Existing "),
                        Span::styled(h.vocab.clone(), Style::new().fg(ACCENT)),
                        Span::raw(" term: "),
                        Span::styled(format!("\"{}\"", h.existing), Style::new().fg(Color::Green).bold()),
                        Span::styled(if h.tid.is_empty() { String::new() } else { format!("  (term {})", h.tid) }, Style::new().fg(Color::DarkGray)),
                    ]));
                    text.push_line(Line::from(vec![Span::raw("Why it's suggested: "), Span::styled(h.why.clone(), Style::new().fg(Color::Yellow))]));
                    buttons.push(('y', format!("Use the existing term \"{}\"", h.existing), c == Choice::On));
                    buttons.push((
                        'n',
                        if h.creatable {
                            format!("Keep \"{}\" - add it as a new {} term", h.typed, h.vocab)
                        } else {
                            format!("Keep \"{}\" (stays an error - {} can't get new terms)", h.typed, h.vocab)
                        },
                        c == Choice::Off,
                    ));
                }
                (None, Some(fix)) => {
                    let c = r.choice(fix, fd.auto);
                    text.push_line(fd.message.clone());
                    text.push_line("");
                    text.push_line(Line::from(vec![Span::styled("Fix: ", Style::new().fg(ACCENT).bold()), Span::raw(fix.describe())]));
                    if fd.auto {
                        text.push_line(Span::styled("A safe fix - on unless you turn it off.", Style::new().fg(Color::DarkGray)));
                    }
                    buttons.push(('y', "Apply this fix".into(), c == Choice::On));
                    buttons.push(('n', "Keep the sheet's value".into(), c == Choice::Off));
                }
                (_, None) => {
                    text.push_line(fd.message.clone());
                    if fd.severity == Severity::Error {
                        text.push_line("");
                        text.push_line(Span::styled(
                            "Fix this in the sheet, then press r to re-read it.",
                            Style::new().fg(Color::DarkGray),
                        ));
                    }
                }
            }
        }
        Some(Entry::Applied(fix)) => {
            let off = r.rejected.contains(fix);
            text.push_line(Line::from(Span::styled(if off { "FIX TURNED OFF" } else { "FIXED" }, Style::new().fg(Color::Green).bold())));
            text.push_line("");
            text.push_line(fix.describe_at(&r.original));
            buttons.push(('y', "Keep this fix".into(), !off));
            buttons.push(('n', "Turn it off - keep the sheet's value".into(), off));
        }
        None => text.push_line("Nothing to report - the sheet is clean."),
    }
    if !buttons.is_empty() {
        text.push_line("");
        text.push_line(Span::styled(
            "Choices are applied when you re-check (c). Nothing changes your sheet.",
            Style::new().fg(Color::DarkGray),
        ));
    }

    let block = Block::bordered().title(" Detail ");
    let body = block.inner(area);
    f.render_widget(block, area);
    let extra = buttons.len() as u16 + if link.is_some() { 2 } else { 0 } + if buttons.is_empty() { 0 } else { 1 };
    // Buttons go right under the text (estimated after wrapping), not at the
    // bottom of a tall pane where they're easy to miss.
    let w = body.width.max(1) as usize;
    let text_h: usize = text.lines.iter().map(|l| l.width().max(1).div_ceil(w)).sum();
    let text_h = (text_h as u16).min(body.height.saturating_sub(extra));
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(text_h), Constraint::Length(extra), Constraint::Min(0)])
        .split(body);
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), parts[0]);

    let mut y = parts[1].y;
    if !buttons.is_empty() {
        y += 1;
    }
    for (k, label, on) in buttons {
        if y >= parts[1].bottom() {
            break;
        }
        let rect = Rect::new(parts[1].x, y, parts[1].width, 1);
        f.render_widget(Paragraph::new(button(k, label, on)), rect);
        app.hits.borrow_mut().push((rect, Hit::Key(KeyCode::Char(k))));
        y += 1;
    }
    if let Some(url) = link {
        y += 1;
        if y < parts[1].bottom() {
            let rect = Rect::new(parts[1].x, y, parts[1].width, 1);
            let idx = push_link(app, &url);
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" o ", Style::new().fg(Color::Black).bg(ACCENT)),
                    Span::raw(" "),
                    Span::styled(url, link_style()),
                ])),
                rect,
            );
            app.hits.borrow_mut().push((rect, Hit::Link(idx)));
        }
    }
}

fn push_link(app: &App, url: &str) -> usize {
    let mut l = app.links.borrow_mut();
    l.push(url.to_string());
    l.len() - 1
}

fn tree(f: &mut Frame, r: &Review, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    fn walk(r: &Review, id: &str, depth: usize, lines: &mut Vec<Line<'static>>) {
        let n = &r.report.tree.nodes[id];
        let indent = "  ".repeat(depth);
        match n.kind {
            Kind::Item => {
                let files = match n.files.len() {
                    0 => " (no file)".to_string(),
                    1 => String::new(),
                    k => format!(" ({k} files)"),
                };
                lines.push(Line::from(vec![
                    Span::raw(format!("{indent}· ")),
                    Span::styled(n.id.clone(), Style::new().fg(Color::DarkGray)),
                    Span::raw(format!(" {} ", n.title)),
                    Span::styled(format!("[{}]{files}", n.model), Style::new().fg(Color::Magenta)),
                ]));
            }
            _ => {
                let tag = match r.report.attach.get(id) {
                    Some(node) => format!("on site, node {}", node.nid),
                    None => n.level.clone(),
                };
                lines.push(Line::from(vec![
                    Span::raw(format!("{indent}▸ ")),
                    Span::styled(n.id.clone(), Style::new().fg(ACCENT).bold()),
                    Span::raw(format!(" {} ", if n.title.is_empty() { "(untitled - press t)" } else { &n.title })),
                    Span::styled(format!("[{tag}]"), Style::new().fg(Color::Green)),
                ]));
            }
        }
        let mut kids = r.report.tree.children(id);
        kids.sort_by_key(|k| (k.kind == Kind::Item, k.id.clone()));
        for k in kids {
            walk(r, &k.id, depth + 1, lines);
        }
    }
    for root in r.report.tree.roots() {
        walk(r, &root.id, 0, &mut lines);
    }
    if lines.is_empty() {
        lines.push(Line::from("No identifiers found - see Findings."));
    }
    f.render_widget(
        Paragraph::new(lines).scroll((r.scroll, 0)).block(Block::bordered().title(" What will be created ")),
        area,
    );
}

fn terms(f: &mut Frame, app: &App, r: &Review, area: Rect) {
    let rows = r.term_rows();
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);
    let vw = r.report.new_terms.keys().map(|v| v.chars().count()).max().unwrap_or(0) + 2;
    let items: Vec<ListItem> = if rows.is_empty() {
        vec![ListItem::new("No new terms - every value is already on the site.")]
    } else {
        rows.iter()
            .map(|(v, i)| {
                let t = &r.report.new_terms[v][*i];
                let uses = t.uses.len();
                let mut spans = Vec::new();
                match &t.link {
                    Some(_) if r.no_link.contains(&(v.clone(), t.name.clone())) => {
                        spans.push(Span::styled("[ ] ", Style::new().fg(Color::DarkGray)))
                    }
                    Some(_) => spans.push(Span::styled("[✓] ", Style::new().fg(Color::Green))),
                    None => spans.push(Span::raw("    ")),
                }
                spans.push(Span::styled(format!("{v:<vw$}"), Style::new().fg(Color::DarkGray)));
                spans.push(Span::styled(t.name.clone(), Style::new().bold()));
                spans.push(Span::styled(format!("  ({uses} row{})", if uses == 1 { "" } else { "s" }), Style::new().fg(Color::DarkGray)));
                match &t.link {
                    Some(l) if r.no_link.contains(&(v.clone(), t.name.clone())) => {
                        spans.push(Span::styled(format!("  {} link left out", l.source), Style::new().fg(Color::DarkGray)))
                    }
                    Some(l) => {
                        spans.push(Span::raw(format!("  {} ", l.source)));
                        spans.push(Span::styled(l.uri.clone(), link_style()));
                    }
                    None => spans.push(Span::styled("  no authority link - name only", Style::new().fg(Color::Yellow))),
                }
                ListItem::new(Line::from(spans))
            })
            .collect()
    };
    let mut state = ListState::default().with_selected(if rows.is_empty() { None } else { Some(r.term_sel) });
    f.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(" New terms - created by a terms job before the ingest "))
            .highlight_style(Style::new().bg(Color::DarkGray))
            .highlight_symbol("> "),
        parts[0],
        &mut state,
    );
    app.hits.borrow_mut().push((inner(parts[0]), Hit::Terms { offset: state.offset(), check_cols: 6 }));
    let help = vec![
        Line::from(Span::styled(
            "[✓] the term is created with its authority link; Space (or click the box) leaves it out. o, or clicking the selected row, opens the link.",
            Style::new().fg(Color::DarkGray),
        )),
        Line::from(Span::styled("SACDA Thesaurus terms are never created here.", Style::new().fg(Color::DarkGray))),
    ];
    f.render_widget(Paragraph::new(help).wrap(Wrap { trim: false }).block(Block::default().borders(Borders::TOP)), parts[1]);
}

fn written(f: &mut Frame, summary: &[String], dir: &std::path::Path, area: Rect) {
    let mut lines = vec![
        Line::from(Span::styled("Bundle written", Style::new().fg(Color::Green).bold())),
        Line::from(dir.display().to_string()),
        Line::from(""),
    ];
    lines.extend(summary.iter().map(|s| Line::from(s.clone())));
    lines.push(Line::from(""));
    lines.push(Line::from("Press i to ingest now (Workbench checks every step first), or Enter to finish."));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(Block::bordered().title(" Done ")), area);
}

fn input_popup(f: &mut Frame, app: &App, area: Rect) {
    let (title, prompt, buf) = match &app.input {
        Input::None => return,
        Input::Url(b) => (" Google Sheets link ", "Paste the sheet's link (shared as 'anyone with the link can view'):".to_string(), b),
        Input::Folder(b) => (
            " Files folder ",
            "Folder with the access files - type it, or drag the folder into this window. Empty = `access` next to the sheet:".to_string(),
            b,
        ),
        Input::Confirm(b) => (
            " Confirm ",
            format!("This writes to {}. Type '{}' to continue:", app.site(), app.site()),
            b,
        ),
        Input::Title { id, buf } => (" Collection title ", format!("{id} isn't in the sheet or on the site. Title for the new collection:"), buf),
    };
    let w = area.width.saturating_sub(6).min(100);
    let rect = Rect::new(area.x + (area.width - w) / 2, area.y + area.height / 3, w, 8);
    // The end of what's typed, so a long link is visibly there.
    let room = w.saturating_sub(6) as usize;
    let shown = if buf.chars().count() > room {
        format!("…{}", buf.chars().rev().take(room - 1).collect::<String>().chars().rev().collect::<String>())
    } else {
        buf.clone()
    };
    let check = match &app.input {
        Input::Url(b) if b.trim().is_empty() => Line::from(Span::styled("Ctrl+U clears", Style::new().fg(Color::DarkGray))),
        Input::Url(b) => match sheet_id(b) {
            Some(id) => Line::from(Span::styled(
                format!("✓ Google Sheets link ({} characters, sheet {}…) - Enter to check it", b.trim().len(), &id[..8.min(id.len())]),
                Style::new().fg(Color::Green),
            )),
            None => Line::from(Span::styled("✗ not a Google Sheets link yet (docs.google.com/spreadsheets/d/...)", Style::new().fg(Color::Yellow))),
        },
        Input::Folder(b) if b.trim().is_empty() => Line::from(Span::styled("Enter = use `access` next to the sheet", Style::new().fg(Color::DarkGray))),
        Input::Folder(b) => {
            // Only exists-or-not while typing: counting `/` would walk the disk.
            if super::clean_path(b).is_dir() {
                Line::from(Span::styled("✓ folder found - Enter to use it", Style::new().fg(Color::Green)))
            } else {
                Line::from(Span::styled("✗ no such folder", Style::new().fg(Color::Yellow)))
            }
        }
        _ => Line::from(""),
    };
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(prompt),
            Line::from(""),
            Line::from(Span::styled(format!("> {shown}▏"), Style::new().fg(ACCENT))),
            Line::from(""),
            check,
        ])
        .wrap(Wrap { trim: false })
        .block(Block::bordered().title(title).border_style(Style::new().fg(ACCENT))),
        rect,
    );
}
