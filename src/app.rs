//! The default `organise` flow: load -> check -> review -> write bundle -> (ingest).

use anyhow::{bail, Context, Result};
use organise::sacda::bundle;
use organise::sacda::config::Config;
use organise::sacda::precheck::{Fix, Options, Session, Severity};
use organise::sacda::{doctor, run, source};
use organise::{tui, Cli, Command};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

pub fn main(cli: Cli) -> Result<()> {
    let g = cli.g.as_deref().map(Path::new);
    match cli.command {
        Some(Command::Doctor { site }) => {
            if !doctor::print(site.as_deref()) {
                std::process::exit(1);
            }
            return Ok(());
        }
        Some(Command::Ingest { bundle, check_only }) => {
            let cfg = Config::load(g)?;
            return run::run_bundle(&cfg, Path::new(&bundle), check_only);
        }
        _ => {}
    }

    let cfg = Config::load(g)?;
    let interactive = !cli.plain && std::io::stdout().is_terminal() && std::io::stdin().is_terminal();

    if interactive {
        let outcome = tui::run(tui::Start {
            cfg,
            input: cli.input.clone(),
            site: cli.site.clone(),
            sheet: cli.sheet.clone(),
            access_dir: cli.access_dir.clone().map(PathBuf::from),
            out: cli.out.clone().map(PathBuf::from),
            collection_title: cli.collection_title.clone(),
            accept_suggestions: cli.accept_suggestions,
        })?;
        if let Some(tui::Outcome::Ingest { cfg, dir }) = outcome {
            return run::run_bundle(&cfg, &dir, false);
        }
        return Ok(());
    }

    let Some(input) = cli.input.clone() else {
        bail!("give a spreadsheet: organise <sheet.xlsx | sheet.csv | Google Sheets link>");
    };
    let (site_name, site_cfg) = cfg.site(cli.site.as_deref())?;
    if site_cfg.confirm && !cli.offline && !cli.yes {
        confirm_typed(&site_name, &site_cfg.host)?;
    }

    let table = source::load(&input, cli.sheet.as_deref(), &cfg.sacda.aliases)
        .with_context(|| format!("reading {input}"))?;
    println!("Read {} row(s) from {input}", table.rows.len());
    let mut session = Session::new(
        cfg.clone(),
        &input,
        Options {
            site: Some(site_name.clone()),
            access_dir: cli.access_dir.clone().map(PathBuf::from),
            collection_title: cli.collection_title.clone(),
            offline: cli.offline,
        },
    )?;
    println!("Checking against {} ({}); files from {}", site_name, site_cfg.host, session.files.root.display());

    let (_, first, _) = session.settle(&table, &[], &[]);
    let accepted: Vec<Fix> = if cli.accept_suggestions {
        first.findings.iter().filter(|f| !f.auto).filter_map(|f| f.fix.clone()).collect()
    } else {
        Vec::new()
    };
    let (fixed, report, applied) = session.settle(&table, &accepted, &[]);

    println!("\n{}", bundle::report_text(&report, &applied, &table));
    print_tree(&report);

    if report.blocked() {
        bail!("{} error(s) above - fix them in the sheet (or accept suggestions) and run again", report.count(Severity::Error));
    }
    if cli.offline {
        println!("Offline check only - no bundle written.");
        return Ok(());
    }
    let dir = cli
        .out
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| bundle::default_dir(&input, &report, &site_name));
    let written = bundle::write(&cfg, &site_name, &fixed, &report, &applied, &dir)?;
    println!("\nBundle written to {}", written.dir.display());
    for s in &written.manifest.steps {
        println!("  step: {}{}", s.name, if s.covers.is_empty() { String::new() } else { format!(" (+ {})", s.covers.join(", ")) });
    }
    if cli.ingest {
        run::run_bundle(&cfg, &written.dir, false)?;
    } else {
        println!("\nTo ingest: organise ingest \"{}\"", written.dir.display());
    }
    Ok(())
}

fn confirm_typed(site: &str, host: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!("{site} ({host}) needs confirmation - pass --yes");
    }
    print!("This writes to {site} ({host}). Type '{site}' to continue: ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    if line.trim() != site {
        bail!("not confirmed");
    }
    Ok(())
}

fn print_tree(report: &organise::sacda::precheck::Report) {
    use organise::sacda::tree::Kind;
    fn walk(report: &organise::sacda::precheck::Report, id: &str, depth: usize) {
        let n = &report.tree.nodes[id];
        let kids = report.tree.children(id);
        let items = kids.iter().filter(|k| k.kind == Kind::Item).count();
        let tag = match n.kind {
            Kind::Item => return,
            _ if report.attach.contains_key(id) => format!("on site, node {}", report.attach[id].nid),
            _ => n.level.clone(),
        };
        println!(
            "{}{} {} [{}]{}",
            "  ".repeat(depth),
            n.id,
            if n.title.is_empty() { "(untitled)" } else { &n.title },
            tag,
            if items > 0 { format!(" - {items} item(s)") } else { String::new() }
        );
        for k in kids.iter().filter(|k| k.kind != Kind::Item) {
            walk(report, &k.id, depth + 1);
        }
    }
    println!("Tree:");
    for r in report.tree.roots() {
        walk(report, &r.id, 1);
    }
}
