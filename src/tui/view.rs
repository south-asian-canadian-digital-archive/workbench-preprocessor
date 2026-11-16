//! Drawing. Pure functions of `App` - no state changes here.

use super::{App, Entry, Input, Review, Screen, Tab};
use crate::sacda::precheck::Severity;
use crate::sacda::tree::Kind;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};
use ratatui::Frame;

const ACCENT: Color = Color::Cyan;

fn sev_style(s: Severity) -> Style {
    match s {
        Severity::Error => Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        Severity::Warning => Style::new().fg(Color::Yellow),
        Severity::Info => Style::new().fg(Color::DarkGray),
    }
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
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5), Constraint::Length(1), Constraint::Length(1)])
        .split(area);

    header(f, app, rows[0]);
    match &app.screen {
        Screen::Pick => pick(f, app, rows[1]),
        Screen::Review(r) => review(f, r, rows[1]),
        Screen::Written { summary, dir } => written(f, summary, dir, rows[1]),
    }
    let status = if app.status.is_empty() {
        Line::from("")
    } else {
        Line::from(Span::styled(app.status.clone(), Style::new().fg(Color::Yellow)))
    };
    f.render_widget(Paragraph::new(status), rows[2]);
    f.render_widget(Paragraph::new(footer(app)), rows[3]);
    input_popup(f, app, area);
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
        if e == 0 {
            spans.push(Span::styled("   ready to write", Style::new().fg(Color::Green).bold()));
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::BOTTOM)),
        area,
    );
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
    match &app.screen {
        Screen::Pick => keys(&[("↑↓", "sheet"), ("←→", "site"), ("Enter", "check"), ("u", "Google Sheets link"), ("q", "quit")]),
        Screen::Review(r) => match r.tab {
            Tab::Findings => keys(&[
                ("Space", "toggle fix"),
                ("a", "accept all"),
                ("t", "title"),
                ("r", "re-read"),
                ("w", "write"),
                ("i", "write+ingest"),
                ("Tab", "view"),
                ("Esc", "back"),
            ]),
            _ => keys(&[("↑↓", "scroll"), ("Tab", "view"), ("w", "write"), ("i", "write+ingest"), ("Esc", "back"), ("q", "quit")]),
        },
        Screen::Written { .. } => keys(&[("i", "ingest now"), ("Enter", "done")]),
    }
}

fn pick(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
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
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Files are looked up in the `access` folder next to the sheet.",
        Style::new().fg(Color::DarkGray),
    )));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(Block::bordered().title(" Where ")), cols[1]);
}

fn review(f: &mut Frame, r: &Review, area: Rect) {
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
    f.render_widget(
        Tabs::new(titles).select(sel).highlight_style(Style::new().fg(ACCENT).bold().underlined()),
        rows[0],
    );
    match r.tab {
        Tab::Findings => findings(f, r, rows[1]),
        Tab::Tree => tree(f, r, rows[1]),
        Tab::Terms => terms(f, r, rows[1]),
    }
}

fn findings(f: &mut Frame, r: &Review, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);
    let items: Vec<ListItem> = r
        .entries
        .iter()
        .map(|e| match e {
            Entry::Finding(i) => {
                let fd = &r.report.findings[*i];
                let mark = match &fd.fix {
                    Some(fix) if r.accepted.contains(fix) => "[x] ",
                    Some(fix) if r.rejected.contains(fix) => "[ ] ",
                    Some(_) if fd.auto => "[x] ",
                    Some(_) => "[ ] ",
                    None => "    ",
                };
                let loc = fd.line.map(|l| format!("r{l} ")).unwrap_or_default();
                ListItem::new(Line::from(vec![
                    Span::raw(mark),
                    Span::styled(format!("{:<8}", fd.severity.label()), sev_style(fd.severity)),
                    Span::styled(format!("{:<10}", fd.check), Style::new().fg(Color::DarkGray)),
                    Span::raw(loc),
                    Span::raw(fd.message.clone()),
                ]))
            }
            Entry::Applied(fix) => ListItem::new(Line::from(vec![
                Span::raw("[x] "),
                Span::styled(format!("{:<8}", "fixed"), Style::new().fg(Color::Green)),
                Span::styled(format!("{:<10}", ""), Style::new()),
                Span::raw(fix.describe_at(&r.original)),
            ])),
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

    // Detail of the selected entry.
    let mut text = Text::default();
    match r.entries.get(r.sel) {
        Some(Entry::Finding(i)) => {
            let fd = &r.report.findings[*i];
            text.push_line(Line::from(Span::styled(fd.severity.label().to_uppercase(), sev_style(fd.severity))));
            if let Some(l) = fd.line {
                text.push_line(format!("Sheet row {l}{}", fd.id.as_ref().map(|i| format!(" - {i}")).unwrap_or_default()));
            }
            text.push_line("");
            text.push_line(fd.message.clone());
            if let Some(fix) = &fd.fix {
                text.push_line("");
                let on = (fd.auto || r.accepted.contains(fix)) && !r.rejected.contains(fix);
                text.push_line(Line::from(vec![
                    Span::styled(if on { "Fix (on): " } else { "Suggested fix (off): " }, Style::new().fg(ACCENT).bold()),
                    Span::raw(fix.describe()),
                ]));
                text.push_line(Span::styled("Space turns it on/off. Fixes change the bundle, never your sheet.", Style::new().fg(Color::DarkGray)));
            }
        }
        Some(Entry::Applied(fix)) => {
            text.push_line(Line::from(Span::styled("FIXED", Style::new().fg(Color::Green).bold())));
            text.push_line("");
            text.push_line(fix.describe_at(&r.original));
            text.push_line("");
            text.push_line(Span::styled("Space turns this fix off.", Style::new().fg(Color::DarkGray)));
        }
        None => text.push_line("Nothing to report - the sheet is clean."),
    }
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }).block(Block::bordered().title(" Detail ")), cols[1]);
}

fn tree(f: &mut Frame, r: &Review, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    fn walk(r: &Review, id: &str, depth: usize, lines: &mut Vec<Line<'static>>) {
        let n = &r.report.tree.nodes[id];
        let indent = "  ".repeat(depth);
        match n.kind {
            Kind::Item => {
                let files = if n.files.len() > 1 { format!(" ({} files)", n.files.len()) } else { String::new() };
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

fn terms(f: &mut Frame, r: &Review, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    if r.report.new_terms.is_empty() {
        lines.push(Line::from("No new terms - every value is already on the site."));
    }
    for (vocab, terms) in &r.report.new_terms {
        lines.push(Line::from(Span::styled(format!("{vocab} ({} new)", terms.len()), Style::new().fg(ACCENT).bold())));
        for t in terms {
            let link = match &t.link {
                Some(l) => Span::styled(format!("  {} {}", l.source, l.uri), Style::new().fg(Color::Green)),
                None => Span::styled("  no authority link - name only", Style::new().fg(Color::Yellow)),
            };
            lines.push(Line::from(vec![Span::raw(format!("  {}", t.name)), link]));
        }
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        "These are created by a terms job before the ingest. SACDA Thesaurus terms are never created here.",
        Style::new().fg(Color::DarkGray),
    )));
    f.render_widget(
        Paragraph::new(lines).scroll((r.scroll, 0)).wrap(Wrap { trim: false }).block(Block::bordered().title(" New terms ")),
        area,
    );
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
        Input::Url(b) => (" Google Sheets link ", "Paste the sheet's link (it must be shared as 'anyone with the link'):".to_string(), b),
        Input::Confirm(b) => (
            " Confirm ",
            format!("This writes to {}. Type '{}' to continue:", app.site(), app.site()),
            b,
        ),
        Input::Title { id, buf } => (" Collection title ", format!("{id} isn't in the sheet or on the site. Title for the new collection:"), buf),
    };
    let w = area.width.saturating_sub(10).min(90);
    let rect = Rect::new(area.x + (area.width - w) / 2, area.y + area.height / 3, w, 5);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(vec![Line::from(prompt), Line::from(""), Line::from(Span::styled(format!("> {buf}_"), Style::new().fg(ACCENT)))])
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(title).border_style(Style::new().fg(ACCENT))),
        rect,
    );
}
