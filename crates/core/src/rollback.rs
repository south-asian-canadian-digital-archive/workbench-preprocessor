//! Undoing what an ingest created. Nodes (with their media and files) go
//! through Workbench's own rollback files - a `task: delete` config and a CSV
//! of the node IDs it wrote while creating them. Terms have no rollback file:
//! their IDs are in the terms job's log ("Term 123 (...) added to vocabulary"),
//! and they're deleted through the site's REST API.
//!
//! Order matters: items before the components they sit in, and both before
//! the terms they use.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Items,
    Components,
    /// New terms in this vocabulary.
    Terms(String),
}

impl Kind {
    /// Rollback order: what depends on something goes first.
    pub fn order(&self) -> u8 {
        match self {
            Kind::Items => 0,
            Kind::Components => 1,
            Kind::Terms(_) => 2,
        }
    }
    pub fn describe(&self, n: usize) -> String {
        match self {
            Kind::Items => format!("Items - {n} node(s), with their media and files"),
            Kind::Components => format!("Collections, series and files - {n} node(s)"),
            Kind::Terms(v) => format!("New {v} terms - {n} term(s)"),
        }
    }
}

/// What one Workbench job of an ingest created.
#[derive(Debug, Clone)]
pub struct Created {
    pub kind: Kind,
    /// Nodes: Workbench's rollback config for them (task: delete).
    pub rollback: Option<PathBuf>,
    /// Nodes: (node ID, identifier); terms: (term ID, name).
    pub entries: Vec<(String, String)>,
}

/// Which groups a choice really needs, so nothing is left pointing at what's
/// gone: components take their items with them, and terms take every node
/// (items and components use them). `chosen` is indexed like `groups`.
pub fn with_dependencies(groups: &[Created], chosen: &[bool]) -> Vec<bool> {
    let any = |pred: &dyn Fn(&Kind) -> bool| groups.iter().zip(chosen).any(|(g, c)| *c && pred(&g.kind));
    let terms = any(&|k| matches!(k, Kind::Terms(_)));
    let components = terms || any(&|k| matches!(k, Kind::Components));
    groups
        .iter()
        .zip(chosen)
        .map(|(g, c)| {
            *c || match g.kind {
                Kind::Items => components,
                Kind::Components => terms,
                Kind::Terms(_) => false,
            }
        })
        .collect()
}

/// The mirror of `with_dependencies`: keeping a group keeps what it needs
/// kept. Unchoosing items unchooses components and terms; unchoosing
/// components unchooses terms.
pub fn without(groups: &[Created], chosen: &[bool], dropped: usize) -> Vec<bool> {
    let mut out = chosen.to_vec();
    out[dropped] = false;
    let level = groups[dropped].kind.order();
    for (i, g) in groups.iter().enumerate() {
        if g.kind.order() > level {
            out[i] = false;
        }
    }
    out
}

/// A `key: value` from a Workbench config or rollback file, also when the
/// value is on the next, indented line (Workbench writes long paths so).
pub fn yaml_value(text: &str, key: &str) -> Option<String> {
    let mut lines = text.lines();
    while let Some(l) = lines.next() {
        if let Some(rest) = l.strip_prefix(key).and_then(|r| r.strip_prefix(':')) {
            let v = rest.trim();
            let v = if v.is_empty() { lines.next()?.trim() } else { v };
            return Some(v.trim_matches(|c| c == '"' || c == '\'').to_string());
        }
    }
    None
}

/// The rollback file Workbench wrote for a config whose
/// `rollback_config_file_path` is `configured`, during a run that started
/// at `since`. Workbench adds a timestamp to the name (`stem.<time>.yml`).
pub fn rollback_file(configured: &Path, since: SystemTime) -> Option<PathBuf> {
    let dir = configured.parent()?;
    let stem = configured.file_stem()?.to_string_lossy().to_string();
    let newer = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).is_ok_and(|t| t >= since);
    let mut found: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            (n == format!("{stem}.yml") || (n.starts_with(&format!("{stem}.")) && n.ends_with(".yml"))) && newer(p)
        })
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    found.sort();
    found.pop().map(|(_, p)| p)
}

/// The nodes in a rollback file's CSV: (node ID, identifier).
pub fn rollback_nodes(rollback: &Path) -> Vec<(String, String)> {
    let Some(csv) = std::fs::read_to_string(rollback).ok().and_then(|t| yaml_value(&t, "input_csv")) else { return vec![] };
    let Ok(text) = std::fs::read_to_string(&csv) else { return vec![] };
    // Workbench starts the CSV with # comment lines.
    let body: String = text.lines().filter(|l| !l.starts_with('#')).map(|l| format!("{l}\n")).collect();
    let mut r = csv::Reader::from_reader(body.as_bytes());
    let Ok(h) = r.headers().cloned() else { return vec![] };
    let (Some(nid), id) = (h.iter().position(|c| c == "node_id"), h.iter().position(|c| c == "id")) else { return vec![] };
    r.records()
        .flatten()
        .filter_map(|rec| {
            let n = rec.get(nid)?.trim().to_string();
            (!n.is_empty()).then(|| (n, id.and_then(|i| rec.get(i)).unwrap_or("").to_string()))
        })
        .collect()
}

/// Terms a terms job logged as created, from byte `from` of its log: (term ID, name).
/// A log shorter than `from` (replaced during the run) gives nothing: its
/// older lines could name terms another ingest made.
pub fn logged_terms(log: &Path, from: u64) -> Vec<(String, String)> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(log) else { return vec![] };
    if f.metadata().map(|m| m.len()).unwrap_or(0) < from || f.seek(SeekFrom::Start(from)).is_err() {
        return vec![];
    }
    let mut bytes = Vec::new();
    if f.read_to_end(&mut bytes).is_err() {
        return vec![];
    }
    String::from_utf8_lossy(&bytes).lines().filter_map(term_line).collect()
}

/// 'Term 123 ("Name") added to vocabulary "subject".'
fn term_line(line: &str) -> Option<(String, String)> {
    let rest = &line[line.find("Term ")? + 5..];
    let (tid, rest) = rest.split_once(" (\"")?;
    if !tid.chars().all(|c| c.is_ascii_digit()) || tid.is_empty() {
        return None;
    }
    let (name, rest) = rest.split_once("\") added to vocabulary")?;
    let _ = rest;
    Some((tid.to_string(), name.to_string()))
}

/// A copy of Workbench's rollback config that logs where organise's logs are,
/// so a rollback run has a log next to the ingest's - and deletes children
/// before their parents (its CSV lists them in the order they were made).
pub fn runnable_config(rollback: &Path, log: &Path) -> std::io::Result<PathBuf> {
    let text = std::fs::read_to_string(rollback)?;
    let mut out = text.clone();
    if let Some(csv) = yaml_value(&text, "input_csv") {
        if let Ok(rows) = std::fs::read_to_string(&csv) {
            let mut keep: Vec<&str> = Vec::new();
            let mut data: Vec<&str> = Vec::new();
            for (i, l) in rows.lines().enumerate() {
                if i == 0 || l.starts_with('#') {
                    keep.push(l);
                } else {
                    data.push(l);
                }
            }
            data.reverse();
            let p = Path::new(&csv);
            let name = format!("organise-{}", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
            let reversed = p.with_file_name(name);
            let body: String = keep.iter().chain(data.iter()).map(|l| format!("{l}\n")).collect();
            if std::fs::write(&reversed, body).is_ok() {
                out = out.replace(&csv, &reversed.to_string_lossy());
            }
        }
    }
    if yaml_value(&text, "log_file_path").is_none() {
        out.push_str(&format!("\nlog_file_path: \"{}\"\n", log.to_string_lossy().replace('\\', "/").replace('"', "\\\"")));
    }
    let name = format!("organise-{}", rollback.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "rollback.yml".into()));
    let p = rollback.with_file_name(name);
    std::fs::write(&p, out)?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(kind: Kind) -> Created {
        Created { kind, rollback: None, entries: vec![("1".into(), "a".into())] }
    }

    #[test]
    fn dependencies_follow_the_order() {
        let groups = vec![g(Kind::Terms("subject".into())), g(Kind::Components), g(Kind::Items)];
        // Terms alone take every node with them.
        assert_eq!(with_dependencies(&groups, &[true, false, false]), vec![true, true, true]);
        // Components take their items.
        assert_eq!(with_dependencies(&groups, &[false, true, false]), vec![false, true, true]);
        // Items alone are fine.
        assert_eq!(with_dependencies(&groups, &[false, false, true]), vec![false, false, true]);
        // Leaving items out leaves out what needs them gone first.
        assert_eq!(without(&groups, &[true, true, true], 2), vec![false, false, false]);
        assert_eq!(without(&groups, &[true, true, true], 1), vec![false, false, true]);
        assert_eq!(without(&groups, &[true, true, true], 0), vec![false, true, true]);
    }

    #[test]
    fn reads_workbench_rollback_files_and_term_logs() {
        let t = tempfile::tempdir().unwrap();
        let csv = t.path().join("rollback.2026_11_24_10_50_05.csv");
        std::fs::write(&csv, "node_id,id,title,field_member_of,file\n# Generated by a \"create\" task\n1560,2099_12,Demo,,\n1561,2099_12_01,Series,1560,\n").unwrap();
        let conf = t.path().join("rollback-ingest-x-dev-components.2026_11_24_10_50_05.yml");
        std::fs::write(&conf, format!("task: delete\ninput_csv: \n  {}\n", csv.display())).unwrap();
        let since = SystemTime::now() - std::time::Duration::from_secs(60);
        let found = rollback_file(&t.path().join("rollback-ingest-x-dev-components.yml"), since).unwrap();
        assert_eq!(found, conf);
        assert_eq!(rollback_nodes(&found), vec![("1560".into(), "2099_12".into()), ("1561".into(), "2099_12_01".into())]);
        // Another step's file isn't this one's.
        assert!(rollback_file(&t.path().join("rollback-ingest-x-dev-items.yml"), since).is_none());

        let log = t.path().join("terms.log");
        std::fs::write(&log, "old - INFO - Term 5 (\"Old\") added to vocabulary \"subject\".\n").unwrap();
        let from = std::fs::metadata(&log).unwrap().len();
        let mut more = std::fs::read_to_string(&log).unwrap();
        more.push_str("x - INFO - Term 812 (\"Anniversarys\") added to vocabulary \"subject\".\nx - INFO - Term \"Y\" already exists\n");
        std::fs::write(&log, more).unwrap();
        assert_eq!(logged_terms(&log, from), vec![("812".into(), "Anniversarys".into())]);
    }
}
