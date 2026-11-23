//! The fixity checks Workbench makes on every uploaded file (`fixity_algorithm`
//! in the items config): after the upload it compares the local file's hash
//! with the one Drupal computed. It only writes that to its own log file, so
//! after each run the new part of the log is read and every file's checksum is
//! kept in `g/logs/organise/<bundle>-checksums.csv` - a record of what was
//! ingested, and proof each file arrived intact.

use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Checksum {
    /// Workbench's log timestamp.
    pub when: String,
    pub identifier: String,
    pub file: String,
    pub algorithm: String,
    pub local: String,
    /// Drupal's hash (the same as `local` when they match).
    pub drupal: String,
    pub matched: bool,
}

/// A config's value for `key` (organise writes these one per line).
fn yaml_value(yml: &str, key: &str) -> Option<String> {
    yml.lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
        .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
        .filter(|v| !v.is_empty())
}

/// Where each of these configs logs, and how long that log is now: only what
/// a run adds after this is read.
pub fn mark(configs: &[PathBuf]) -> Vec<(PathBuf, PathBuf, u64)> {
    configs
        .iter()
        .filter_map(|c| {
            let yml = std::fs::read_to_string(c).ok()?;
            let log = PathBuf::from(yaml_value(&yml, "log_file_path")?);
            let len = std::fs::metadata(&log).map(|m| m.len()).unwrap_or(0);
            Some((c.clone(), log, len))
        })
        .collect()
}

/// One Workbench log line about a file's checksums.
fn parse_line(line: &str) -> Option<Checksum> {
    let when = line.split(" - ").next().unwrap_or("").trim().to_string();
    let rest = &line[line.find("Local and Drupal ")? + "Local and Drupal ".len()..];
    let (algorithm, rest) = rest.split_once(" checksums for file \"")?;
    if let Some((file, rest)) = rest.split_once("\" (named in CSV row \"") {
        // Local and Drupal md5 checksums for file "P" (named in CSV row "ID") do not match (local: X, Drupal: Y).
        let (id, rest) = rest.split_once("\")")?;
        let local = rest.split_once("(local: ")?.1.split_once(',')?.0.trim().to_string();
        let drupal = rest.split_once("Drupal: ")?.1.trim_end_matches(['.', ')']).trim().to_string();
        return Some(Checksum { when, identifier: id.to_string(), file: file.to_string(), algorithm: algorithm.to_string(), local, drupal, matched: false });
    }
    // Local and Drupal md5 checksums for file "P" (HASH) match.
    let (file, rest) = rest.split_once("\" (")?;
    let (hash, rest) = rest.split_once(')')?;
    if !rest.trim_start().starts_with("match") {
        return None;
    }
    Some(Checksum {
        when,
        identifier: String::new(),
        file: file.to_string(),
        algorithm: algorithm.to_string(),
        local: hash.to_string(),
        drupal: hash.to_string(),
        matched: true,
    })
}

/// File (as named in the CSV, and by its bare name) -> identifier, from a
/// config's input CSV.
fn ids_by_file(dir: &Path, yml: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(csv) = yaml_value(yml, "input_csv") else { return out };
    let id_field = yaml_value(yml, "id_field").unwrap_or_else(|| "id".into());
    let Ok(mut r) = csv::Reader::from_path(dir.join(csv)) else { return out };
    let Ok(h) = r.headers().cloned() else { return out };
    let (Some(fi), Some(ii)) = (h.iter().position(|c| c == "file"), h.iter().position(|c| c == id_field)) else { return out };
    for rec in r.records().flatten() {
        let (f, id) = (rec.get(fi).unwrap_or("").trim(), rec.get(ii).unwrap_or("").trim());
        if f.is_empty() || id.is_empty() {
            continue;
        }
        out.insert(f.to_string(), id.to_string());
        if let Some(name) = Path::new(f).file_name() {
            out.entry(name.to_string_lossy().to_lowercase()).or_insert_with(|| id.to_string());
        }
    }
    out
}

/// Every checksum the logs got since `mark`.
pub fn collect(dir: &Path, marks: &[(PathBuf, PathBuf, u64)]) -> Vec<Checksum> {
    let mut out = Vec::new();
    for (config, log, from) in marks {
        let Ok(bytes) = std::fs::read(log) else { continue };
        // A log that got shorter was started over: all of it is this run's.
        let start = if (*from as usize) <= bytes.len() { *from as usize } else { 0 };
        let text = String::from_utf8_lossy(&bytes[start..]);
        let yml = std::fs::read_to_string(config).unwrap_or_default();
        let ids = ids_by_file(dir, &yml);
        for line in text.lines() {
            if let Some(mut c) = parse_line(line) {
                if c.identifier.is_empty() {
                    let name = Path::new(&c.file).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                    c.identifier = ids
                        .iter()
                        .find(|(f, _)| c.file == **f || c.file.ends_with(&format!("/{f}")) || c.file.ends_with(&format!("\\{f}")))
                        .map(|(_, id)| id.clone())
                        .or_else(|| ids.get(&name).cloned())
                        .unwrap_or_default();
                }
                out.push(c);
            }
        }
    }
    out
}

/// Add to the bundle's checksums file (a header when it's new).
pub fn save(path: &Path, rows: &[Checksum]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let new = !path.exists();
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    let mut w = csv::Writer::from_writer(f);
    if new {
        w.write_record(["identifier", "file", "algorithm", "local_checksum", "drupal_checksum", "result", "logged"])?;
    }
    for c in rows {
        w.write_record([
            c.identifier.as_str(),
            c.file.as_str(),
            c.algorithm.as_str(),
            c.local.as_str(),
            c.drupal.as_str(),
            if c.matched { "match" } else { "MISMATCH" },
            c.when.as_str(),
        ])?;
    }
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_matches_and_mismatches() {
        let ok = r#"23-Nov-26 21:58:46 - INFO - workbench_utils.py - 5482 - Local and Drupal md5 checksums for file "/a/access/2099_01_01_001.tif" (0cc175b9c0f1b6a831c399e269772661) match."#;
        let c = parse_line(ok).unwrap();
        assert_eq!((c.when.as_str(), c.algorithm.as_str(), c.file.as_str(), c.local.as_str(), c.matched), ("23-Nov-26 21:58:46", "md5", "/a/access/2099_01_01_001.tif", "0cc175b9c0f1b6a831c399e269772661", true));
        let bad = r#"23-Nov-26 21:58:47 - WARNING - workbench_utils.py - 5497 - Local and Drupal md5 checksums for file "/a/b.tif" (named in CSV row "2099_01_02") do not match (local: aaa, Drupal: bbb)."#;
        let c = parse_line(bad).unwrap();
        assert_eq!((c.identifier.as_str(), c.local.as_str(), c.drupal.as_str(), c.matched), ("2099_01_02", "aaa", "bbb", false));
        assert!(parse_line("23-Nov-26 - INFO - x - 1 - Node created.").is_none());
    }

    #[test]
    fn only_this_runs_lines_get_saved_with_their_identifier() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("items.log");
        std::fs::write(&log, "old - INFO - x - 1 - Local and Drupal md5 checksums for file \"/x/old.tif\" (111) match.\n").unwrap();
        let cfg = d.path().join("items.config.yml");
        std::fs::write(&cfg, format!("input_csv: items.csv\nid_field: id\nlog_file_path: \"{}\"\n", log.display())).unwrap();
        std::fs::write(d.path().join("items.csv"), "id,file\n2099_01_01_001,/x/access/2099_01_01_001.tif\n").unwrap();
        let marks = mark(std::slice::from_ref(&cfg));
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        std::io::Write::write_all(&mut f, b"new - INFO - x - 1 - Local and Drupal md5 checksums for file \"/x/access/2099_01_01_001.tif\" (222) match.\n").unwrap();
        let got = collect(d.path(), &marks);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].identifier.as_str(), got[0].local.as_str()), ("2099_01_01_001", "222"));
        let out = d.path().join("sums.csv");
        save(&out, &got).unwrap();
        save(&out, &got).unwrap();
        let t = std::fs::read_to_string(&out).unwrap();
        assert_eq!(t.lines().count(), 3, "one header, then a row per save: {t}");
    }
}
