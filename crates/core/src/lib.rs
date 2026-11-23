//! The SACDA ingest standard: one spreadsheet in, a checked Workbench bundle out.
//!
//! - `config`     g/config/sacda.toml + schema.toml
//! - `source`     load and clean the sheet
//! - `tree`       collection -> component -> item tree from identifiers
//! - `precheck`   every check, findings and fixes
//! - `authority`  LCSH / LC name lookups for new terms
//! - `suggest`    "did you mean" for columns and terms
//! - `edtf`       EDTF validation and normalisation
//! - `site`       read-only JSON:API client
//! - `bundle`     write CSVs + configs + manifest
//! - `run`        drive Workbench over a bundle
//! - `doctor`     machine + site health check
//! - `sheets`     Google Sheets history + per-sheet settings (files folder)
//! - `progress`   progress + cancel handle for a check running elsewhere

pub mod authority;
pub mod bundle;
pub mod config;
pub mod doctor;
pub mod edtf;
pub mod precheck;
pub mod progress;
pub mod run;
pub mod sheets;
pub mod site;
pub mod source;
pub mod subset;
pub mod suggest;
pub mod tree;

/// Write a file whole or not at all: to a temp file beside it, then renamed
/// over it. A crash, or two organise windows saving at once, can't leave half
/// a JSON file that the next start reads as empty.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map(|_| ()).map_err(|e| e.error)
}
