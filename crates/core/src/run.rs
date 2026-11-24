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
    /// Workbench waited this many seconds because the site answered slowly
    /// (its `adaptive_pause`). It says so only in its log, read as it runs.
    Paused { seconds: u64 },
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
/// What a Workbench run gives back as it goes.
enum Out {
    Line(String),
    Paused(u64),
}

/// "HTTP requests paused for 3 seconds because ..." in a Workbench log.
fn pause_secs(line: &str) -> Option<u64> {
    const AT: &str = "HTTP requests paused for ";
    line[line.find(AT)? + AT.len()..].split_whitespace().next()?.parse().ok()
}

/// Follows Workbench logs from where they were when the run started, for
/// what it writes only there (its adaptive pauses).
struct LogTail {
    files: Vec<(std::path::PathBuf, u64)>,
    last: std::time::Instant,
}

impl LogTail {
    fn new(files: &[(std::path::PathBuf, u64)]) -> Self {
        Self { files: files.to_vec(), last: std::time::Instant::now() }
    }

    /// Pauses logged since the last look (at most every half second unless `now`).
    fn poll(&mut self, now: bool) -> Vec<u64> {
        use std::io::{Read, Seek, SeekFrom};
        if !now && self.last.elapsed() < std::time::Duration::from_millis(500) {
            return vec![];
        }
        self.last = std::time::Instant::now();
        let mut out = Vec::new();
        for (path, pos) in &mut self.files {
            let Ok(mut f) = std::fs::File::open(&*path) else { continue };
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if len < *pos {
                *pos = 0; // started again
            }
            if len == *pos || f.seek(SeekFrom::Start(*pos)).is_err() {
                continue;
            }
            let mut buf = Vec::new();
            if f.take(len - *pos).read_to_end(&mut buf).is_err() {
                continue;
            }
            // Whole lines only: the rest is read next time.
            let end = buf.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
            *pos += end as u64;
            out.extend(String::from_utf8_lossy(&buf[..end]).lines().filter_map(pause_secs));
        }
        out
    }
}

/// Run Workbench on `config`, passing on its output line by line and the
/// pauses in `watch` (its log files, from these offsets).
fn spawn_workbench(cfg: &Config, config: &Path, check: bool, watch: &[(std::path::PathBuf, u64)], out: &mut dyn FnMut(Out)) -> Result<bool> {
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
    // Under a screen that can stop it, Workbench gets a process group of its
    // own, so stopping reaches Python under `uv` too. Not in plain mode:
    // there Ctrl-C must still reach it with the terminal's own group.
    #[cfg(unix)]
    if crate::progress::has_job() {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
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
    let mut tail = LogTail::new(watch);
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(l) => out(Out::Line(l)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        for s in tail.poll(false) {
            out(Out::Paused(s));
        }
        if crate::progress::cancelled_here() {
            kill_tree(&mut child);
            bail!("Workbench was stopped partway through; what it had already created stays on the site (rollback files are in g/rollback/)");
        }
    }
    for p in pumps {
        let _ = p.join();
    }
    for s in tail.poll(true) {
        out(Out::Paused(s));
    }
    Ok(child.wait()?.success())
}

/// Stop `uv` and everything it started (Python running Workbench): killing
/// only `uv` would leave Workbench running on, still creating nodes.
fn kill_tree(child: &mut std::process::Child) {
    let pid = child.id().to_string();
    #[cfg(unix)]
    {
        // The group's id is uv's pid (process_group(0) above). A polite stop
        // first, so Python can finish writing its log line; then for good.
        let group = format!("-{pid}");
        let _ = Command::new("kill").args(["-TERM", "--", &group]).stderr(Stdio::null()).status();
        for _ in 0..30 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = Command::new("kill").args(["-KILL", "--", &group]).stderr(Stdio::null()).status();
    }
    #[cfg(windows)]
    {
        // /T: the whole tree under uv.
        let _ = Command::new("taskkill").args(["/PID", &pid, "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn is_error_line(line: &str) -> bool {
    let t = line.trim_start().trim_start_matches("- ");
    t.starts_with("ERROR") || t.starts_with("Error:") || t.contains("Traceback")
}

/// A Workbench run, streamed. Workbench exits 0 even when a node or media
/// fails, so its `ERROR` lines are counted and returned too.
fn workbench(cfg: &Config, config: &Path, watch: &[(std::path::PathBuf, u64)], ev: &mut dyn FnMut(Event)) -> Result<(bool, Vec<String>)> {
    let mut errors = Vec::new();
    let ok = spawn_workbench(cfg, config, false, watch, &mut |out| match out {
        Out::Line(line) => {
            ev(Event::Line(&line));
            if is_record(&line) {
                ev(Event::Record);
            }
            if is_error_line(&line) {
                errors.push(line);
            }
        }
        Out::Paused(seconds) => {
            ev(Event::Line(&format!("(Workbench paused {seconds}s: the site answered slowly - adaptive_pause)")));
            ev(Event::Paused { seconds });
        }
    })?;
    Ok((ok, errors))
}

/// Workbench `--check` exits 0 even when it lists problems, so its output is
/// what tells us. Captured, and only shown if it failed.
fn workbench_check(cfg: &Config, config: &Path) -> Result<(bool, String)> {
    let mut text = String::new();
    let mut failed = false;
    let ok = spawn_workbench(cfg, config, true, &[], &mut |out| {
        if let Out::Line(line) = out {
            let l = line.trim_start();
            failed |= l.starts_with("Error:") || l.starts_with("ERROR") || l.contains("Traceback");
            text.push_str(&line);
            text.push('\n');
        }
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
    // Workbench runs in its own folder, so a relative bundle path
    // ("organise ingest ingest-x-dev") would point it at the wrong place.
    let abs = dunce::canonicalize(dir).with_context(|| format!("no bundle folder at {}", dir.display()))?;
    let dir = abs.as_path();
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
        let watch: Vec<_> = marks.iter().map(|(_, log, len)| (log.clone(), *len)).collect();
        let (ok, mut errors) = workbench(cfg, &dir.join(&step.config), &watch, ev)?;
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

#[cfg(test)]
mod pause_tests {
    use super::*;

    #[test]
    fn reads_pauses_from_the_log_as_it_grows() {
        let t = tempfile::tempdir().unwrap();
        let log = t.path().join("wb.log");
        std::fs::write(&log, "24-Nov-26 10:50:24 - INFO - HTTP requests paused for 3 seconds because request in next log entry exceeded adaptive threshold of 2.\n").unwrap();
        // Only what's written after the run starts counts.
        let mut tail = LogTail::new(&[(log.clone(), std::fs::metadata(&log).unwrap().len())]);
        assert!(tail.poll(true).is_empty());
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        write!(f, "x - INFO - Node created\nx - INFO - HTTP requests paused for 5 seconds because ...\nx - INFO - HTTP requests paused for 3 sec").unwrap();
        assert_eq!(tail.poll(true), vec![5]);
        // The half-written line is read once it's whole.
        writeln!(f, "onds because ...").unwrap();
        assert_eq!(tail.poll(true), vec![3]);
        assert_eq!(pause_secs("nothing here"), None);
    }
}

#[cfg(all(test, unix))]
mod kill_tests {
    use super::*;

    /// A grandchild (like Python under uv) is stopped along with the child.
    #[test]
    fn stopping_reaches_the_grandchild() {
        use std::os::unix::process::CommandExt;
        let d = tempfile::tempdir().unwrap();
        let pidfile = d.path().join("pid");
        let script = format!("sleep 60 & echo $! > {}; wait", pidfile.display());
        let mut child = Command::new("sh").arg("-c").arg(script).process_group(0).spawn().unwrap();
        let mut grandchild = String::new();
        for _ in 0..50 {
            grandchild = std::fs::read_to_string(&pidfile).unwrap_or_default().trim().to_string();
            if !grandchild.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(!grandchild.is_empty());
        kill_tree(&mut child);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let alive = Command::new("kill").args(["-0", &grandchild]).stderr(Stdio::null()).status().unwrap().success();
        assert!(!alive, "the grandchild {grandchild} is still running");
    }
}
