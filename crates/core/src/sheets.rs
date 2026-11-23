//! Sheets this machine has worked with: Google Sheets links (shown as
//! history) and the per-sheet settings - which folder holds its access files,
//! and whether file names come from the identifier. Kept next to organise's
//! settings file so a link pasted once is there next time.

use crate::config::{user_config_path, Config};
use crate::source;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Sheet {
    /// The Google Sheets link or the local path, as given.
    pub input: String,
    /// "2024_19 Chandra Bodalia fonds" - what to call it in a list.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub rows: usize,
    /// "2024_19_50 - 2024_19_118": first and last identifier, so batches of the
    /// same fonds can be told apart.
    #[serde(default)]
    pub span: String,
    /// Folder with the access files (None: `access` next to the sheet).
    #[serde(default)]
    pub folder: Option<PathBuf>,
    #[serde(default)]
    pub files_by_identifier: bool,
    /// Only these rows (page-style: "12-40, 57, 100-") or identifiers;
    /// empty = the whole sheet. See `subset::Subset`.
    #[serde(default)]
    pub subset: String,
    /// Last used, seconds since 1970.
    #[serde(default)]
    pub used: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Sheets {
    #[serde(default)]
    pub sheets: Vec<Sheet>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

/// How many links the history keeps.
const KEEP_LINKS: usize = 30;

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Sheets {
    pub fn load() -> Self {
        let path = user_config_path().map(|p| p.with_file_name("sheets.json"));
        let mut s: Sheets = path
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        s.path = path;
        s
    }

    /// Not saved anywhere (tests).
    pub fn in_memory() -> Self {
        Self::default()
    }

    pub fn save(&self) {
        let Some(p) = &self.path else { return };
        if let Some(d) = p.parent() {
            let _ = fs::create_dir_all(d);
        }
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = crate::write_atomic(p, t.as_bytes());
        }
    }

    pub fn get(&self, input: &str) -> Option<&Sheet> {
        self.sheets.iter().find(|s| s.input == input)
    }

    /// The entry for `input`, created if new.
    pub fn entry(&mut self, input: &str) -> &mut Sheet {
        if let Some(i) = self.sheets.iter().position(|s| s.input == input) {
            return &mut self.sheets[i];
        }
        self.sheets.push(Sheet { input: input.to_string(), ..Default::default() });
        self.sheets.last_mut().unwrap()
    }

    /// Google Sheets links, most recently used first.
    pub fn links(&self) -> Vec<&Sheet> {
        let mut v: Vec<&Sheet> = self.sheets.iter().filter(|s| source::is_url(&s.input)).collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.used));
        v
    }

    /// Forget a link (or a local sheet's settings).
    pub fn remove(&mut self, input: &str) {
        self.sheets.retain(|s| s.input != input);
    }

    /// Record a pulled link and keep the history to its size.
    pub fn remember(&mut self, pulled: &Pulled) {
        let e = self.entry(&pulled.url);
        e.label = pulled.label.clone();
        e.rows = pulled.rows;
        e.span = pulled.span.clone();
        e.used = now();
        let mut links: Vec<(u64, String)> =
            self.sheets.iter().filter(|s| source::is_url(&s.input)).map(|s| (s.used, s.input.clone())).collect();
        links.sort_by_key(|l| std::cmp::Reverse(l.0));
        for (_, old) in links.into_iter().skip(KEEP_LINKS) {
            self.remove(&old);
        }
    }
}

/// A Google Sheet downloaded to the cache folder.
#[derive(Debug, Clone)]
pub struct Pulled {
    pub url: String,
    pub path: PathBuf,
    pub label: String,
    pub rows: usize,
    pub span: String,
}

/// Download a Google Sheet to the cache folder and read enough of it to name
/// it: the collection identifier and title, and the row count.
pub fn pull(url: &str, cfg: &Config) -> Result<Pulled> {
    let link = crate::input::SheetLink::parse(url)?;
    let text = crate::input::fetch_sheet(url)?;
    let dir = crate::authority::cache_dir().unwrap_or_else(std::env::temp_dir).join("sheets");
    fs::create_dir_all(&dir).with_context(|| format!("can't create {}", dir.display()))?;
    // One file per tab, so two tabs of a sheet don't overwrite each other.
    let path = dir.join(match link.gid.as_deref() {
        Some(g) if g != "0" => format!("{}-{g}.csv", link.id),
        _ => format!("{}.csv", link.id),
    });
    fs::write(&path, &text).with_context(|| format!("can't write {}", path.display()))?;
    let table = source::load_for(&path.to_string_lossy(), None, cfg)?;
    Ok(Pulled { url: url.trim().to_string(), label: label_of(&table, cfg), rows: table.rows.len(), span: span_of(&table), path })
}

/// Temporary files organise keeps: downloaded sheets and the id.loc.gov
/// lookup cache. Never bundles, logs, rollback files or the id map - those
/// record what was ingested.
#[derive(Debug, Clone)]
pub struct TempFiles {
    pub label: &'static str,
    pub path: PathBuf,
    pub files: usize,
    pub bytes: u64,
}

pub fn temp_files() -> Vec<TempFiles> {
    let Some(dir) = crate::authority::cache_dir() else { return vec![] };
    let mut out = Vec::new();
    let sheets = dir.join("sheets");
    let (mut files, mut bytes) = (0, 0);
    for e in fs::read_dir(&sheets).into_iter().flatten().flatten() {
        if let Some(m) = e.metadata().ok().filter(|m| m.is_file()) {
            files += 1;
            bytes += m.len();
        }
    }
    out.push(TempFiles { label: "downloaded Google Sheets", path: sheets, files, bytes });
    let lc = dir.join("authority-cache-v2.json");
    let bytes = fs::metadata(&lc).map(|m| m.len()).unwrap_or(0);
    out.push(TempFiles { label: "id.loc.gov lookup cache", path: lc, files: usize::from(bytes > 0), bytes });
    out
}

/// Delete what `temp_files` lists. Returns how many files went.
pub fn clear_temp() -> Result<usize> {
    let mut n = 0;
    for t in temp_files() {
        if t.files == 0 {
            continue;
        }
        if t.path.is_dir() {
            for e in fs::read_dir(&t.path)?.flatten() {
                if e.path().is_file() {
                    fs::remove_file(e.path())?;
                    n += 1;
                }
            }
        } else if t.path.is_file() {
            fs::remove_file(&t.path)?;
            n += 1;
        }
    }
    Ok(n)
}

/// "2024_19 Chandra Bodalia fonds": the first identifier's collection part
/// and the sheet's most common collection title.
pub fn label_of(table: &source::Table, cfg: &Config) -> String {
    let id_col = crate::tree::ID_COLUMNS.iter().find(|c| table.col(c).is_some());
    let prefix = id_col
        .and_then(|c| table.rows.iter().map(|r| table.get(r, c)).find(|v| !v.is_empty()))
        .map(|id| id.split('_').take(crate::tree::COLLECTION_SEGMENTS).collect::<Vec<_>>().join("_"))
        .unwrap_or_default();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for r in &table.rows {
        let t = table.get(r, &cfg.sacda.sheet.collection_title_column);
        if !t.is_empty() {
            *counts.entry(t).or_default() += 1;
        }
    }
    let title = counts.into_iter().max_by_key(|(t, n)| (*n, std::cmp::Reverse(*t))).map(|(t, _)| t).unwrap_or("");
    match (prefix.is_empty(), title.is_empty()) {
        (true, true) => "Google Sheet".into(),
        (false, true) => prefix,
        (true, false) => title.to_string(),
        (false, false) => format!("{prefix} {title}"),
    }
}

/// First and last identifier in the sheet, to the component level
/// (`2024_19_50 - 2024_19_118`), ordered by their numbers.
pub fn span_of(table: &source::Table) -> String {
    let Some(col) = crate::tree::ID_COLUMNS.iter().find(|c| table.col(c).is_some()) else { return String::new() };
    let key = |id: &str| -> Vec<u64> { id.split('_').map(|p| p.parse().unwrap_or(u64::MAX)).collect() };
    let ids: Vec<String> = table
        .rows
        .iter()
        .map(|r| table.get(r, col))
        .filter(|v| !v.is_empty())
        .map(|id| id.split('_').take(crate::tree::COLLECTION_SEGMENTS + 1).collect::<Vec<_>>().join("_"))
        .collect();
    let (Some(lo), Some(hi)) = (ids.iter().min_by_key(|i| key(i)), ids.iter().max_by_key(|i| key(i))) else { return String::new() };
    if lo == hi { lo.clone() } else { format!("{lo} - {hi}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_orders_by_number() {
        let t = source::parse("t", b"accessIdentifier,title\n2024_19_9_001,a\n2024_19_118_002,b\n2024_19_50,c\n", &Default::default()).unwrap();
        assert_eq!(span_of(&t), "2024_19_9 - 2024_19_118");
    }
}
