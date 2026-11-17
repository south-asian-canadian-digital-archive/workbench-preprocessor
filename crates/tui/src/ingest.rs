//! Running a bundle through Workbench in the terminal, with a progress bar
//! that stays at the bottom while Workbench's output scrolls above it.

use indicatif::{ProgressBar, ProgressStyle};
use sacda::config::Config;
use sacda::run::{self, Event};
use std::io::IsTerminal;
use std::path::Path;
use std::time::Duration;

/// `organise ingest`, the screen's `i`, and `--ingest`: a bar when there's a
/// terminal to draw it in, plain lines otherwise (logs, pipes).
pub fn ingest(cfg: &Config, dir: &Path, check_only: bool) -> anyhow::Result<()> {
    if !std::io::stdout().is_terminal() {
        return run::run_bundle(cfg, dir, check_only);
    }
    let bar = ProgressBar::new(0);
    bar.enable_steady_tick(Duration::from_millis(120));
    let spinner = ProgressStyle::with_template("{spinner:.cyan} {prefix:.bold} {msg}  {elapsed:.dim}")
        .unwrap()
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ");
    let counting = ProgressStyle::with_template(
        "{spinner:.cyan} {prefix:.bold} {msg}  [{bar:30.cyan/blue}] {pos}/{len}  {elapsed:.dim} (about {eta} left)",
    )
    .unwrap()
    .progress_chars("█▉▊▋▌▍▎▏ ")
    .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ");
    bar.set_style(spinner.clone());
    bar.set_prefix("Starting");

    let result = run::run_bundle_with(cfg, dir, check_only, &mut |e| match e {
        Event::Stage { label, index, of, total, checking } => {
            bar.set_prefix(format!("Step {index}/{of}"));
            if checking || total == 0 {
                bar.set_style(spinner.clone());
                bar.set_message(if checking { format!("Workbench is checking {}", label.trim_start_matches("check ")) } else { label.to_string() });
            } else {
                bar.set_style(counting.clone());
                bar.set_length(total as u64);
                bar.set_position(0);
                bar.reset_eta();
                bar.set_message(label.to_string());
            }
        }
        Event::Line(l) => bar.println(l),
        Event::Record => bar.inc(1),
    });
    bar.finish_and_clear();
    result
}
