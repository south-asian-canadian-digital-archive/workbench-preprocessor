//! Runs a bundle's steps through Islandora Workbench, in order: every step's
//! `--check` first (Workbench's own validation against the live site), then the
//! real runs. A failed check stops everything before anything is written.

use super::bundle::Manifest;
use super::config::Config;
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// `uv run --project <workbench> python workbench --config <cfg>`, from the
/// workbench folder, streaming its output. Workbench exits 0 even when a node
/// or media fails, so its `ERROR` lines are counted and returned too.
fn workbench(cfg: &Config, config: &Path) -> Result<(bool, Vec<String>)> {
    use std::io::{BufRead, BufReader};
    let root = cfg.workbench_root();
    if !root.join("workbench").is_file() {
        bail!("no Islandora Workbench at {} (expected the `workbench` script there)", root.display());
    }
    let mut child = Command::new("uv")
        .arg("run")
        .arg("--project")
        .arg(&root)
        .arg("python")
        .arg("workbench")
        .arg("--config")
        .arg(config)
        .current_dir(&root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("couldn't start `uv` - is it installed? (run `organise doctor`)")?;
    let mut errors = Vec::new();
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            println!("{line}");
            let t = line.trim_start().trim_start_matches("- ");
            if t.starts_with("ERROR") || t.starts_with("Error:") || t.contains("Traceback") {
                errors.push(line);
            }
        }
    }
    let status = child.wait()?;
    Ok((status.success(), errors))
}

/// Workbench `--check` exits 0 even when it lists problems, so its output is
/// what tells us. Capture and look for its error markers.
fn workbench_check(cfg: &Config, config: &Path) -> Result<(bool, String)> {
    let root = cfg.workbench_root();
    let out = Command::new("uv")
        .arg("run")
        .arg("--project")
        .arg(&root)
        .arg("python")
        .arg("workbench")
        .arg("--config")
        .arg(config)
        .arg("--check")
        .current_dir(&root)
        .output()
        .context("couldn't start `uv` - is it installed? (run `organise doctor`)")?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let failed = !out.status.success()
        || text.lines().any(|l| {
            let l = l.trim_start();
            l.starts_with("Error:") || l.starts_with("ERROR") || l.contains("Traceback")
        });
    Ok((!failed, text))
}

pub fn run_bundle(cfg: &Config, dir: &Path, check_only: bool) -> Result<()> {
    let m = Manifest::read(dir)?;
    println!("Bundle for {} ({}): {} component(s), {} item(s), {} new term(s)", m.site, m.host, m.components, m.items, m.new_terms);

    println!("\n== Workbench --check ==");
    // Terms checks can only pass once the terms exist, and component/item
    // checks look terms up - so check terms jobs alone first, then run them,
    // then check the rest.
    let (terms, rest): (Vec<_>, Vec<_>) = m.steps.iter().partition(|s| s.name.starts_with("terms-"));
    for step in &terms {
        check_step(cfg, dir, &step.config, &step.name)?;
    }
    if check_only && !terms.is_empty() {
        println!("(skipping the ingest checks: they need the new terms, which a check-only run doesn't create)");
        return Ok(());
    }
    if !check_only {
        for step in &terms {
            println!("\n== {} ==", step.name);
            let (ok, errors) = workbench(cfg, &dir.join(&step.config))?;
            if !ok || !errors.is_empty() {
                bail!("{} failed ({} error line(s)) - see the log under g/logs/organise/. Nothing after it ran.", step.name, errors.len());
            }
        }
    }
    for step in &rest {
        check_step(cfg, dir, &step.config, &step.name)?;
    }
    if check_only {
        println!("\nAll checks passed. Run without --check-only to ingest.");
        return Ok(());
    }
    let mut problems: Vec<String> = Vec::new();
    for step in &rest {
        println!(
            "\n== {}{} ==",
            step.name,
            if step.covers.is_empty() { String::new() } else { format!(" (+ {})", step.covers.join(", ")) }
        );
        let (ok, errors) = workbench(cfg, &dir.join(&step.config))?;
        if !ok {
            bail!("{} failed - see the log under g/logs/organise/. Rollback files are in g/rollback/.", step.name);
        }
        problems.extend(errors.into_iter().map(|e| format!("{}: {e}", step.name)));
    }
    if !problems.is_empty() {
        println!("\nIngest finished WITH {} ERROR(S) - those nodes/media need fixing (log: g/logs/organise/):", problems.len());
        for p in &problems {
            println!("  {p}");
        }
        bail!("{} Workbench error(s) during the ingest", problems.len());
    }
    println!("\nIngest finished. Derivatives (thumbnails, OCR) keep generating on the site for a while.");
    Ok(())
}

fn check_step(cfg: &Config, dir: &Path, config: &str, name: &str) -> Result<()> {
    print!("  {name}: ");
    let (ok, text) = workbench_check(cfg, &dir.join(config))?;
    if ok {
        println!("ok");
        Ok(())
    } else {
        println!("FAILED\n{text}");
        bail!("Workbench's check failed for {name}; nothing was ingested")
    }
}
