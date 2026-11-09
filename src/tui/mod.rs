//! Interactive screen (ratatui). Filled in below.
use crate::sacda::config::Config;
use std::path::PathBuf;

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

pub fn run(_start: Start) -> anyhow::Result<Option<Outcome>> {
    anyhow::bail!("tui not built yet")
}
