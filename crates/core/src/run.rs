//! Runs a bundle's steps through Islandora Workbench. New-term jobs are checked
//! and run first, because the component and item checks look those terms up;
//! then every other step is checked, then run. A failed check stops what comes
//! after it, but terms created before it stay on the site (their rollback
//! files are in g/rollback/).

use super::bundle::{Manifest, Step};
use super::config::Config;
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// What a run reports as it goes, so a screen can show progress. Plain
/// `run_bundle` just prints the lines.
pub enum Event<'a> {
    /// A Workbench run starts: what it is, which step of how many, and how
    /// many records it should get through (0 = not known, e.g. a check).
    Stage { label: &'a str, index: usize, of: usize, total: usize, checking: bool },
    /// A line of output (organise's or Workbench's).
    Line(&'a str),
    /// Workbench finished one record: created, skipped as already there, or failed.
    Record,
    /// A run's file checksums were saved: how many matched Drupal's, how many didn't.
    Checksums { matched: usize, mismatched: usize, algorithm: &'a str, file: &'a Path },
}

/// Lines Workbench prints once per record.
fn is_record(line: &str) -> bool {
    let t = line.trim_start().trim_start_matches(['+', '-', ' ']);
    let t = t.strip_prefix("Error: ").unwrap_or(t);
    (t.starts_with("Node for ") && (t.contains(" created at ") || t.contains(" not created")))
        || (t.starts_with("Term \"") && (t.ends_with(" created.") || t.contains(" not created") || t.contains(" already exists")))
        || t.contains("skipping it.")
}

/// Rows a step's CSVs hold (its own, plus those of the tasks it covers).
fn rows_in(dir: &Path, step: &Step) -> usize {
    let csv_of = |config: &str| -> Option<String> {
        let text = std::fs::read_to_string(dir.join(config)).ok()?;
        text.lines().find_map(|l| l.strip_prefix("input_csv:")).map(|v| v.trim().trim_matches('"').to_string())
    };
    let count = |csv: String| -> usize {
        csv::Reader::from_path(dir.join(csv)).map(|mut r| r.records().count()).unwrap_or(0)
    };
    let mut n = csv_of(&step.config).map(count).unwrap_or(0);
    for c in &step.covers {
        n += csv_of(&format!("{c}.config.yml")).map(count).unwrap_or(0);
    }
    n
}

/// `uv run --project <workbench> python workbench --config <cfg> [--check]`,
/// from the workbench folder, handing each line of its output (stdout and
/// stderr, in the order they land) to `line`. Returns whether it exited 0.
/// If the job this thread works for is cancelled (the ingest screen's stop),
/// Workbench is killed and this returns an error.
fn spawn_workbench(cfg: &Config, config: &Path, check: bool, line: &mut dyn FnMut(String)) -> Result<bool> {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc::RecvTimeoutError;
    let root = cfg.workbench_root();
    if !root.join("workbench").is_file() {
        bail!("no Islandora Workbench at {} (expected the `workbench` script there)", root.display());
    }
    let mut cmd = Command::new("uv");
    cmd.arg("run").arg("--project").arg(&root).arg("python").arg("workbench").arg("--config").arg(config);
    if check {
        cmd.arg("--check");
    }
    let mut child = cmd
        .current_dir(&root)
        // Piped, Python holds output back in big blocks: no live progress.
        .env("PYTHONUNBUFFERED", "1")
        // Piped on Windows, Python writes the ANSI code page; ask for UTF-8.
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        // Nothing to type into: a Workbench prompt gets end-of-input and says
        // so, instead of waiting unseen under the ingest screen.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // Piped too: Python's tracebacks and warnings would otherwise print
        // over the screen and be missing from the error count.
        .stderr(Stdio::piped())
        .spawn()
        .context("couldn't start `uv` - is it installed? (run `organise doctor`)")?;
    // Both pipes are read to the end on their own threads, whatever the
    // bytes are: one left unread fills up, Workbench blocks writing to it, and
    // `wait` below never returns.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let pump = |pipe: Box<dyn std::io::Read + Send>, tx: std::sync::mpsc::Sender<String>| {
        std::thread::spawn(move || {
            let mut reader = BufReader::new(pipe);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
                let line = String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string();
                if tx.send(line).is_err() {
                    break;
                }
            }
        })
    };
    let mut pumps = Vec::new();
    if let Some(out) = child.stdout.take() {
        pumps.push(pump(Box::new(out), tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        pumps.push(pump(Box::new(err), tx.clone()));
    }
    drop(tx);
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(l) => line(l),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if crate::progress::cancelled_here() {
            let _ = child.kill();
            let _ = child.wait();
            bail!("stopped - Workbench was stopped partway through; what it had already created stays on the site (rollback files are in g/rollback/)");
        }
    }
    for p in pumps {
        let _ = p.join();
    }
    Ok(child.wait()?.success())
}

fn is_error_line(line: &str) -> bool {
    let t = line.trim_start().trim_start_matches("- ");
    t.starts_with("ERROR") || t.starts_with("Error:") || t.contains("Traceback")
}

/// A Workbench run, streamed. Workbench exits 0 even when a node or media
/// fails, so its `ERROR` lines are counted and returned too.
fn workbench(cfg: &Config, config: &Path, ev: &mut dyn FnMut(Event)) -> Result<(bool, Vec<String>)> {
    let mut errors = Vec::new();
    let ok = spawn_workbench(cfg, config, false, &mut |line| {
        ev(Event::Line(&line));
        if is_record(&line) {
            ev(Event::Record);
        }
        if is_error_line(&line) {
            errors.push(line);
        }
    })?;
    Ok((ok, errors))
}

/// Workbench `--check` exits 0 even when it lists problems, so its output is
/// what tells us. Captured, and only shown if it failed.
fn workbench_check(cfg: &Config, config: &Path) -> Result<(bool, String)> {
    let mut text = String::new();
    let mut failed = false;
    let ok = spawn_workbench(cfg, config, true, &mut |line| {
        let l = line.trim_start();
        failed |= l.starts_with("Error:") || l.starts_with("ERROR") || l.contains("Traceback");
        text.push_str(&line);
        text.push('\n');
    })?;
    Ok((ok && !failed, text))
}

pub fn run_bundle(cfg: &Config, dir: &Path, check_only: bool) -> Result<()> {
    run_bundle_with(cfg, dir, check_only, &mut |e| {
        if let Event::Line(l) = e {
            println!("{l}");
        }
    })
}

pub fn run_bundle_with(cfg: &Config, dir: &Path, check_only: bool, ev: &mut dyn FnMut(Event)) -> Result<()> {
    let m = Manifest::read(dir)?;
    let say = |ev: &mut dyn FnMut(Event), s: String| ev(Event::Line(&s));
    say(ev, format!("Bundle for {} ({}): {} component(s), {} item(s), {} new term(s)", m.site, m.host, m.components, m.items, m.new_terms));

    // Every Workbench run, in order, for the "step n of N" count.
    let (terms, rest): (Vec<_>, Vec<_>) = m.steps.iter().partition(|s| s.name.starts_with("terms-"));
    let runs = if check_only { terms.len() + rest.len() } else { 2 * (terms.len() + rest.len()) };
    let mut n = 0;
    let check = |ev: &mut dyn FnMut(Event), step: &Step, n: &mut usize| -> Result<()> {
        *n += 1;
        let label = format!("check {}", step.name);
        ev(Event::Stage { label: &label, index: *n, of: runs, total: 0, checking: true });
        check_step(cfg, dir, &step.config, &step.name, ev)
    };

    say(ev, "\n== Workbench --check ==".into());
    // Terms checks can only pass once the terms exist, and component/item
    // checks look terms up - so check terms jobs alone first, then run them,
    // then check the rest.
    for step in &terms {
        check(ev, step, &mut n)?;
    }
    if check_only && !terms.is_empty() {
        say(ev, "(skipping the ingest checks: they need the new terms, which a check-only run doesn't create)".into());
        return Ok(());
    }
    let run = |ev: &mut dyn FnMut(Event), step: &Step, n: &mut usize| -> Result<(bool, Vec<String>)> {
        *n += 1;
        let label = if step.covers.is_empty() { step.name.clone() } else { format!("{} (+ {})", step.name, step.covers.join(", ")) };
        ev(Event::Line(&format!("\n== {label} ==")));
        ev(Event::Stage { label: &label, index: *n, of: runs, total: rows_in(dir, step), checking: false });
        // Workbench logs each upload's checksums (main task and secondary tasks
        // alike, each to its own log); keep this run's.
        let configs: Vec<_> = std::iter::once(step.config.clone())
            .chain(step.covers.iter().map(|c| format!("{c}.config.yml")))
            .map(|c| dir.join(c))
            .collect();
        let marks = crate::fixity::mark(&configs);
        let (ok, mut errors) = workbench(cfg, &dir.join(&step.config), ev)?;
        let sums = crate::fixity::collect(dir, &marks);
        if !sums.is_empty() {
            let bundle = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "bundle".into());
            let out = cfg.g_root.join("logs/organise").join(format!("{bundle}-checksums.csv"));
            let bad: Vec<_> = sums.iter().filter(|c| !c.matched).collect();
            for c in &bad {
                let m = format!(
                    "ERROR: checksum mismatch for {} ({}): local {} {}, Drupal {} - the upload may be damaged",
                    c.file, c.identifier, c.algorithm, c.local, c.drupal
                );
                ev(Event::Line(&m));
                errors.push(m);
            }
            match crate::fixity::save(&out, &sums) {
                Ok(()) => {
                    ev(Event::Line(&format!(
                        "Checksums: {} file(s) checked ({}), {} matched Drupal's, {} didn't - saved to {}",
                        sums.len(),
                        sums[0].algorithm,
                        sums.len() - bad.len(),
                        bad.len(),
                        out.display()
                    )));
                    ev(Event::Checksums { matched: sums.len() - bad.len(), mismatched: bad.len(), algorithm: &sums[0].algorithm, file: &out });
                }
                Err(e) => ev(Event::Line(&format!("ERROR: couldn't save the checksums to {}: {e:#}", out.display()))),
            }
        }
        Ok((ok, errors))
    };
    if !check_only {
        for step in &terms {
            let (ok, errors) = run(ev, step, &mut n)?;
            if !ok || !errors.is_empty() {
                bail!("{} failed ({} error line(s)) - see the log under g/logs/organise/. Nothing after it ran.", step.name, errors.len());
            }
        }
    }
    for step in &rest {
        let checked = check(ev, step, &mut n);
        if !check_only && !terms.is_empty() {
            checked.map_err(|e| {
                anyhow::anyhow!("{e:#} - but the new terms were already created on the site (rollback files for them are in g/rollback/)")
            })?;
        } else {
            checked?;
        }
    }
    if check_only {
        say(ev, "\nAll checks passed. Run without --check-only to ingest.".into());
        return Ok(());
    }
    let mut problems: Vec<String> = Vec::new();
    for step in &rest {
        let (ok, errors) = run(ev, step, &mut n)?;
        if !ok {
            bail!("{} failed - see the log under g/logs/organise/. Rollback files are in g/rollback/.", step.name);
        }
        problems.extend(errors.into_iter().map(|e| format!("{}: {e}", step.name)));
    }
    if !problems.is_empty() {
        say(ev, format!("\nIngest finished WITH {} ERROR(S) - those nodes/media need fixing (log: g/logs/organise/):", problems.len()));
        for p in &problems {
            say(ev, format!("  {p}"));
        }
        bail!("{} Workbench error(s) during the ingest", problems.len());
    }
    say(ev, "\nIngest finished. Derivatives (thumbnails, OCR) keep generating on the site for a while.".into());
    Ok(())
}

fn check_step(cfg: &Config, dir: &Path, config: &str, name: &str, ev: &mut dyn FnMut(Event)) -> Result<()> {
    let (ok, text) = workbench_check(cfg, &dir.join(config))?;
    if ok {
        ev(Event::Line(&format!("  {name}: ok")));
        Ok(())
    } else {
        ev(Event::Line(&format!("  {name}: FAILED")));
        for l in text.lines() {
            ev(Event::Line(l));
        }
        bail!("Workbench's check failed for {name}; nothing from it or after it was ingested")
    }
}
