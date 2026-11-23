//! Drawing. Pure functions of `App` - no state changes here, apart from
//! recording where the clickable things ended up.

use super::{same_suggestion, sheet_id, App, Choice, Entry, Hit, Input, PickRow, Review, Screen, Tab};
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
    job_dialog(f, app, area);
    input_popup(f, app, area);
}

/// Jobs that hold up everything else (pulling a sheet, the first check)
/// get a dialog in the middle; re-checks on the review screen stay in the
/// status line so the findings can still be browsed.
fn blocking(app: &App) -> bool {
    app.job.is_some() && !matches!(app.screen, Screen::Review(_))
}

/// "row 120 of 400" in a progress message, as a fraction.
fn row_fraction(msg: &str) -> Option<f64> {
    let rest = &msg[msg.find("row ")? + 4..];
    let (n, rest) = rest.split_once(" of ")?;
    let total: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let (n, total) = (n.trim().parse::<f64>().ok()?, total.parse::<f64>().ok()?);
    (total > 0.0).then(|| (n / total).clamp(0.0, 1.0))
}

fn job_dialog(f: &mut Frame, app: &App, area: Rect) {
    let Some(job) = app.job.as_ref().filter(|_| blocking(app)) else { return };
    let w = area.width.saturating_sub(6).min(76);
    let h = 11.min(area.height);
    let rect = Rect::new(area.x + (area.width - w) / 2, area.y + area.height.saturating_sub(h) / 2, w, h);
    let inner = w.saturating_sub(2) as usize;

    let elapsed = job.started.elapsed();
    let spin = SPINNER[(elapsed.as_millis() / 100) as usize % SPINNER.len()];
    let msg = job.progress.message();
    // Kept to one line so the button stays where its click target is.
    let mut step = if msg.is_empty() { "starting ...".to_string() } else { msg.clone() };
    if step.chars().count() > inner {
        step = step.chars().take(inner.saturating_sub(1)).collect::<String>() + "…";
    }

    // A real bar when the message says how far along it is, otherwise a
    // block sliding back and forth so it's visibly still working.
    let frac = row_fraction(&msg);
    let bar_w = if frac.is_some() { inner.saturating_sub(5) } else { inner }.max(10);
    let bar = match frac {
        Some(frac) => {
            let done = (frac * bar_w as f64).round() as usize;
            Line::from(vec![
                Span::styled("█".repeat(done), Style::new().fg(ACCENT)),
                Span::styled("░".repeat(bar_w - done), Style::new().fg(Color::DarkGray)),
                Span::raw(format!(" {:>3.0}%", frac * 100.0)),
            ])
        }
        None => {
            let block = 6.min(bar_w);
            let span = bar_w - block;
            let t = (elapsed.as_millis() / 60) as usize % (2 * span.max(1));
            let at = if t > span { 2 * span - t } else { t };
            Line::from(vec![
                Span::styled("░".repeat(at), Style::new().fg(Color::DarkGray)),
                Span::styled("█".repeat(block), Style::new().fg(ACCENT)),
                Span::styled("░".repeat(span - at), Style::new().fg(Color::DarkGray)),
            ])
        }
    };

    let secs = elapsed.as_secs();
    let took = if secs >= 60 { format!("{}m {:02}s", secs / 60, secs % 60) } else { format!("{secs}s") };
    let sheet = app.selected_input().map(|s| super::short_name(&s)).filter(|s| !job.label.contains(s.as_str())).unwrap_or_default();
    let cancel = if job.cancelling { " cancelling ... " } else { " Esc  cancel " };

    let lines = vec![
        Line::from(vec![
            Span::styled(format!("{spin} "), Style::new().fg(ACCENT).bold()),
            Span::styled(job.label.clone(), Style::new().bold()),
        ]),
        Line::from(Span::styled(sheet, Style::new().fg(Color::DarkGray))),
        Line::from(""),
        Line::from(step),
        Line::from(""),
        bar,
        Line::from(""),
        Line::from(vec![
            Span::styled(format!("{took} so far"), Style::new().fg(Color::DarkGray)),
            Span::raw("   "),
            Span::styled(cancel, Style::new().fg(Color::Black).bg(if job.cancelling { Color::DarkGray } else { Color::Yellow })),
        ]),
    ];
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(" Working ").border_style(Style::new().fg(ACCENT))),
        rect,
    );

    // Only the cancel button is clickable while this is up.
    let mut hits = app.hits.borrow_mut();
    hits.clear();
    let took_w = format!("{took} so far   ").chars().count() as u16;
    let btn_y = rect.y + 1 + 7;
    if btn_y < rect.bottom().saturating_sub(1) && !job.cancelling {
        hits.push((Rect::new(rect.x + 1 + took_w, btn_y, cancel.chars().count() as u16, 1), Hit::Key(KeyCode::Esc)));
    }
}

fn status_line(app: &App) -> Line<'static> {
    if let Some(job) = app.job.as_ref().filter(|_| !blocking(app)) {
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
        if let Ok(Some(sub)) = app.subset_for(&r.input_name) {
            spans.push(Span::styled(format!("  only {}", sub.describe()), Style::new().fg(Color::Yellow).bold()));
        }
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
        Screen::Pick => {
            let mut k = vec![("↑↓", "sheet"), ("←→", "site"), ("Enter", "check"), ("u", "add Google Sheets link"), ("f", "files folder"), ("m", "file names"), ("s", "rows"), ("c", "clean temp files")];
            if app.selected_input().is_some_and(|s| sacda::source::is_url(&s)) {
                k.push(("x", "remove link"));
            }
            k.push(("q", "quit"));
            keys(&k)
        }
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
                k.push(("s", "rows"));
                k.push(("f", "files folder"));
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
    let rows = app.pick_rows();
    let items: Vec<ListItem> = if rows.is_empty() {
        vec![ListItem::new("No spreadsheets yet. Press u to paste a Google Sheets link, or run organise from the folder that has the sheet.")]
    } else {
        rows.iter()
            .map(|r| match r {
                PickRow::Header(h) => ListItem::new(Line::from(Span::styled(format!("── {h} "), Style::new().fg(ACCENT).bold()))),
                PickRow::Sheet(input) if sacda::source::is_url(input) => {
                    let e = app.sheets.get(input);
                    let label = e.map(|e| e.label.clone()).filter(|l| !l.is_empty()).unwrap_or_else(|| "Google Sheet".into());
                    let rows = e.map(|e| e.rows).unwrap_or(0);
                    let fresh = app.just_pulled.as_deref() == Some(input.as_str());
                    let mut spans = vec![Span::styled(label, if fresh { Style::new().fg(Color::Green).bold() } else { Style::new().bold() })];
                    let span = e.map(|e| e.span.clone()).unwrap_or_default();
                    spans.push(Span::styled(
                        format!("  {rows} rows{}", if span.is_empty() { String::new() } else { format!(", {span}") }),
                        Style::new().fg(Color::DarkGray),
                    ));
                    if fresh {
                        spans.push(Span::styled("  ● just pulled", Style::new().fg(Color::Black).bg(Color::Green)));
                    }
                    let id = sheet_id(input).unwrap_or("");
                    spans.push(Span::styled(format!("  …{}", &id[id.len().saturating_sub(8)..]), Style::new().fg(Color::DarkGray)));
                    ListItem::new(Line::from(spans))
                }
                PickRow::Sheet(input) => {
                    let p = std::path::Path::new(input);
                    ListItem::new(p.strip_prefix(&cwd).unwrap_or(p).display().to_string())
                }
            })
            .collect()
    };
    let mut state = ListState::default()
        .with_offset(app.pick_offset.get())
        .with_selected((!rows.is_empty()).then_some(app.pick_sel));
    f.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(" Sheets "))
            .highlight_style(Style::new().bg(Color::DarkGray).bold())
            .highlight_symbol("> "),
        cols[0],
        &mut state,
    );
    app.pick_offset.set(state.offset());
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

    // Files folder for the selected sheet (or link).
    let selected = app.selected_input();
    lines.push(Line::from(""));
    match &selected {
        Some(sel) => {
            let name = if sacda::source::is_url(sel) {
                app.sheets.get(sel).map(|e| e.label.clone()).unwrap_or_else(|| "this Google Sheet".into())
            } else {
                std::path::Path::new(sel).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
            };
            lines.push(Line::from(vec![Span::raw("For "), Span::styled(name, Style::new().bold()), Span::raw(":")]));
        }
        None => lines.push(Line::from(Span::styled("Pick a sheet, or paste a link with u", Style::new().fg(Color::DarkGray)))),
    }
    let (dir, chosen) = app.access_for(selected.as_deref());
    let count = app.folder_count(&dir);
    lines.push(Line::from(vec![Span::raw("Files folder "), Span::styled("(f to change):", Style::new().fg(Color::DarkGray))]));
    lines.push(Line::from(Span::styled(format!("  {}", dir.display()), Style::new().fg(ACCENT))));
    let where_from = if chosen {
        ""
    } else if selected.as_deref().is_some_and(sacda::source::is_url) {
        " - `access` in the folder organise was started from"
    } else {
        " - `access` next to the sheet"
    };
    lines.push(match count {
        None => Line::from(Span::styled("  counting files ...", Style::new().fg(Color::DarkGray))),
        Some(Some(n)) => Line::from(Span::styled(format!("  {n} file(s){where_from}"), Style::new().fg(Color::Green))),
        Some(None) => Line::from(Span::styled(
            "  not found - press f and type or drop the folder in",
            Style::new().fg(Color::Yellow),
        )),
    });
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::raw("File names "), Span::styled("(m to change):", Style::new().fg(Color::DarkGray))]));
    lines.push(Line::from(Span::styled(
        if app.by_identifier(selected.as_deref()) {
            "  <identifier>.<ext> for every item (file column ignored)"
        } else {
            "  the file column; rows without one use <identifier>.<ext>"
        },
        Style::new().fg(ACCENT),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::raw("Rows "), Span::styled("(s to change):", Style::new().fg(Color::DarkGray))]));
    let subset = selected.as_deref().map(|s| app.subset_for(s)).unwrap_or(Ok(None));
    lines.push(Line::from(match &subset {
        Ok(Some(s)) => Span::styled(format!("  only {} (+ their collection/series rows)", s.describe()), Style::new().fg(Color::Yellow).bold()),
        Ok(None) => Span::styled("  the whole sheet", Style::new().fg(ACCENT)),
        Err(_) => Span::styled("  not valid - press s to change them", sev_style(Severity::Error)),
    }));
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
    let mut state = ListState::default()
        .with_offset(r.list_offset.get())
        .with_selected(if r.entries.is_empty() { None } else { Some(r.sel) });
    f.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(" Precheck "))
            .highlight_style(Style::new().bg(Color::DarkGray))
            .highlight_symbol("> "),
        cols[0],
        &mut state,
    );
    r.list_offset.set(state.offset());
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
                    let same = same_suggestion(r, *i).len();
                    if same > 1 {
                        text.push_line("");
                        text.push_line(Span::styled(
                            format!("Your choice applies to all {same} rows with \"{}\" -> \"{}\".", h.typed, h.existing),
                            Style::new().fg(ACCENT),
                        ));
                    }
                    buttons.push(('y', format!("Use the existing term \"{}\"", h.existing), c == Choice::On));
                    if h.creatable {
                        buttons.push(('n', format!("Keep \"{}\" - add it as a new {} term", h.typed, h.vocab), c == Choice::Off));
                    } else {
                        text.push_line("");
                        text.push_line(Line::from(vec![
                            Span::styled("Can't keep it: ", Style::new().fg(Color::Red).bold()),
                            Span::raw(sacda::precheck::locked_reason(&h.vocab)),
                        ]));
                        text.push_line(Span::styled(
                            "Use the existing term, or correct the sheet and re-read it (r).",
                            Style::new().fg(Color::DarkGray),
                        ));
                    }
                }
                (None, Some(fix @ sacda::precheck::Fix::FileGap(g))) => {
                    use sacda::precheck::Gap;
                    let c = r.choice(fix, fd.auto);
                    text.push_line(fd.message.clone());
                    let keep = match g {
                        Gap::NoFile => "Ingest anyway, no media".to_string(),
                        Gap::MissingFile => "Ingest anyway, without the file".to_string(),
                        Gap::Unmatched => "Add each as its own item".to_string(),
                    };
                    buttons.push(('y', format!("Leave out -> {}", g.csv()), c == Choice::On));
                    buttons.push(('n', keep, c == Choice::Off));
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
    let mut state = ListState::default()
        .with_offset(r.term_offset.get())
        .with_selected(if rows.is_empty() { None } else { Some(r.term_sel) });
    f.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(" New terms - created by a terms job before the ingest "))
            .highlight_style(Style::new().bg(Color::DarkGray))
            .highlight_symbol("> "),
        parts[0],
        &mut state,
    );
    r.term_offset.set(state.offset());
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

fn clean_popup(f: &mut Frame, area: Rect) {
    let items = sacda::sheets::temp_files();
    let size = |b: u64| if b >= 1_000_000 { format!("{:.1} MB", b as f64 / 1e6) } else { format!("{} KB", b.div_ceil(1000)) };
    let mut lines = vec![Line::from("Delete organise's temporary files?"), Line::from("")];
    for t in &items {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<26}", t.label), Style::new().bold()),
            Span::raw(format!("{} file(s), {}", t.files, size(t.bytes))),
        ]));
        lines.push(Line::from(Span::styled(format!("    {}", t.path.display()), Style::new().fg(Color::DarkGray))));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Sheets are downloaded again when opened; the lookup cache only makes checks faster.",
        Style::new().fg(Color::DarkGray),
    )));
    lines.push(Line::from(Span::styled(
        "Bundles, logs, rollback files and the id map are kept - they record what was ingested.",
        Style::new().fg(Color::DarkGray),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" Enter ", Style::new().fg(Color::Black).bg(Color::Yellow)),
        Span::raw(" delete   "),
        Span::styled(" Esc ", Style::new().fg(Color::Black).bg(ACCENT)),
        Span::raw(" keep them"),
    ]));
    let w = area.width.saturating_sub(6).min(100);
    let h = (lines.len() as u16 + 2).min(area.height);
    let rect = Rect::new(area.x + (area.width - w) / 2, area.y + area.height.saturating_sub(h) / 3, w, h);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Temporary files ").border_style(Style::new().fg(Color::Yellow))),
        rect,
    );
}

fn input_popup(f: &mut Frame, app: &App, area: Rect) {
    let (title, prompt, buf) = match &app.input {
        Input::None => return,
        Input::Clean => return clean_popup(f, area),
        Input::Url(b) => (" Google Sheets link ", "Paste the sheet's link (shared as 'anyone with the link can view'):".to_string(), b),
        Input::Folder(b) => (
            " Files folder ",
            "Folder with the access files - type it, or drag the folder into this window. Empty = `access` next to the sheet:".to_string(),
            b,
        ),
        Input::Subset(b) => (
            " Rows to use ",
            "Like PDF pages, by sheet row: 12  |  12,15  |  -40  |  100-  |  12-40. Or an identifier (2024_19_1545). Empty = the whole sheet:".to_string(),
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
        Input::Subset(b) if b.trim().is_empty() => Line::from(Span::styled("Enter = the whole sheet", Style::new().fg(Color::DarkGray))),
        Input::Subset(b) => match sacda::subset::Subset::parse(b) {
            Ok(Some(s)) => Line::from(Span::styled(format!("✓ {} - Enter to use it", s.describe()), Style::new().fg(Color::Green))),
            Ok(None) => Line::from(""),
            Err(e) => Line::from(Span::styled(format!("✗ {e:#}"), Style::new().fg(Color::Yellow))),
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
