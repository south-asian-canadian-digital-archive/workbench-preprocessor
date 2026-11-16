//! Loads the archivist's spreadsheet into memory, cleaned: cells trimmed,
//! `#VALUE!` blanked, non-breaking spaces and common mojibake repaired, and old
//! header names renamed. Every rename is recorded so the report can tell the
//! archivist to fix the sheet itself.

use crate::csv_modifier::{normalize_cell, sanitize_text_in_place, CsvModifier};
use crate::spreadsheet::csv_bytes_from_path;
use anyhow::{bail, Context, Result};
use super::config::Config;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Table {
    pub source: String,
    pub headers: Vec<String>,
    /// Data rows; `line` is the 1-based spreadsheet row (header is row 1).
    pub rows: Vec<Row>,
    /// (old header, new header) pairs that were applied.
    pub renamed: Vec<(String, String)>,
    pub cells_repaired: usize,
    /// What `prepare` did, for the report: (is_warning, line or 0, message).
    pub notes: Vec<(bool, usize, String)>,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub line: usize,
    pub cells: Vec<String>,
}

impl Table {
    pub fn col(&self, name: &str) -> Option<usize> {
        self.headers.iter().position(|h| h == name)
    }

    pub fn get<'a>(&self, row: &'a Row, name: &str) -> &'a str {
        self.col(name)
            .and_then(|i| row.cells.get(i))
            .map(String::as_str)
            .unwrap_or("")
    }
}

pub fn is_url(input: &str) -> bool {
    input.starts_with("http://") || input.starts_with("https://")
}

pub fn load(input: &str, sheet: Option<&str>, aliases: &HashMap<String, String>) -> Result<Table> {
    let bytes = if is_url(input) {
        if !input.contains("docs.google.com/spreadsheets") {
            bail!("Only Google Sheets links are supported as URLs: {input}");
        }
        CsvModifier::fetch_google_sheets_csv(input)?.into_bytes()
    } else {
        csv_bytes_from_path(input, sheet)?
    };
    parse(input, &bytes, aliases)
}

pub fn parse(source: &str, bytes: &[u8], aliases: &HashMap<String, String>) -> Result<Table> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}');
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(text.as_bytes());

    let raw_headers: Vec<String> = reader
        .headers()
        .context("the sheet has no header row")?
        .iter()
        .map(|h| h.trim().to_string())
        .collect();

    let mut renamed = Vec::new();
    let mut headers = raw_headers.clone();
    for h in headers.iter_mut() {
        if let Some(to) = aliases.get(h.as_str()) {
            // Only rename when the target isn't already a column of its own.
            if to != h && !raw_headers.contains(to) {
                renamed.push((h.clone(), to.clone()));
                *h = to.clone();
            }
        }
    }

    let mut rows = Vec::new();
    let mut repaired = 0;
    for (i, rec) in reader.records().enumerate() {
        let rec = rec.with_context(|| format!("row {} can't be read", i + 2))?;
        let mut cells: Vec<String> = (0..headers.len())
            .map(|c| normalize_cell(rec.get(c).unwrap_or("")).to_string())
            .collect();
        for cell in cells.iter_mut() {
            if sanitize_text_in_place(cell) {
                repaired += 1;
            }
        }
        if cells.iter().all(String::is_empty) {
            continue;
        }
        rows.push(Row { line: i + 2, cells });
    }

    Ok(Table {
        source: source.to_string(),
        headers,
        rows,
        renamed,
        cells_repaired: repaired,
        notes: Vec::new(),
    })
}

/// Load a sheet and bring it into the shape the checks expect (see `prepare`).
pub fn load_for(input: &str, sheet: Option<&str>, cfg: &Config) -> Result<Table> {
    let mut t = load(input, sheet, &cfg.sacda.aliases)?;
    prepare(&mut t, cfg);
    Ok(t)
}

/// `localNote` / `Local Note` / `local-note` -> `local_note`.
fn to_snake(h: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in h.chars() {
        if c.is_ascii_uppercase() {
            if prev_lower {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            prev_lower = false;
        } else if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        } else {
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
            prev_lower = false;
        }
    }
    out.trim_matches('_').to_string()
}

fn drop_column(t: &mut Table, i: usize) {
    t.headers.remove(i);
    for r in t.rows.iter_mut() {
        if i < r.cells.len() {
            r.cells.remove(i);
        }
    }
}

fn split_values<'a>(cell: &'a str, seps: &[String], sub: &str) -> Vec<&'a str> {
    let mut parts = vec![cell];
    for sep in seps.iter().map(String::as_str).chain(std::iter::once(sub)) {
        parts = parts.into_iter().flat_map(|p| p.split(sep)).collect();
    }
    parts.into_iter().map(str::trim).filter(|p| !p.is_empty()).collect()
}

/// Archivist sheets -> Workbench shape:
/// - working columns (`[sheet] ignore_columns`, blank headers) are dropped;
/// - identifier columns are folded into the term column they pair with;
/// - `;` (and friends) between values in multi-value term cells become the
///   Workbench subdelimiter.
pub fn prepare(t: &mut Table, cfg: &Config) {
    let sc = &cfg.sacda.sheet;
    let sub = cfg.sacda.ingest.subdelimiter.clone();

    let mut dropped = Vec::new();
    while let Some(i) = t
        .headers
        .iter()
        .position(|h| h.is_empty() || sc.ignore_columns.iter().any(|c| c.eq_ignore_ascii_case(h)))
    {
        dropped.push(if t.headers[i].is_empty() { "(no header)".to_string() } else { t.headers[i].clone() });
        drop_column(t, i);
    }
    if !dropped.is_empty() {
        t.notes.push((false, 0, format!("working columns left out: {}", dropped.join(", "))));
    }

    // Headers written without the `field_` prefix (extent, rights), in
    // camelCase (localNote), or as the field's label ("Subject (Topical)").
    let mut fields: Vec<(String, String)> = Vec::new(); // (field, label)
    for b in ["islandora_object", "archival_component"] {
        for (k, f) in cfg.schema.fields(b) {
            if !fields.iter().any(|(x, _)| x == k) {
                fields.push((k.clone(), f.label.clone()));
            }
        }
    }
    let reserved = super::precheck::checks::RESERVED;
    let skip = |h: &str| {
        reserved.contains(&h)
            || h.starts_with("field_")
            || sc.identifier_columns.contains_key(h)
            || h == sc.collection_title_column
            || fields.iter().any(|(k, _)| k == h)
    };
    let mut auto = Vec::new();
    for i in 0..t.headers.len() {
        let h = t.headers[i].clone();
        if h.is_empty() || skip(&h) {
            continue;
        }
        let snake = to_snake(&h);
        let hit = fields
            .iter()
            .find(|(k, _)| *k == format!("field_{snake}") || *k == format!("field_{}", snake.replace('_', "")))
            .or_else(|| fields.iter().find(|(_, l)| l.eq_ignore_ascii_case(&h)))
            .map(|(k, _)| k.clone());
        if let Some(to) = hit {
            if t.col(&to).is_none() {
                auto.push(format!("{h} -> {to}"));
                t.renamed.push((h, to.clone()));
                t.headers[i] = to;
            }
        }
    }
    // (reported together with the alias renames)
    let _ = auto;

    let mut pairs: Vec<(&String, &String)> = sc.identifier_columns.iter().collect();
    pairs.sort();
    for (idcol, target) in pairs {
        let Some(ic) = t.col(idcol) else { continue };
        let Some(tc) = t.col(target) else {
            t.headers[ic] = target.clone();
            t.notes.push((false, 0, format!("column '{idcol}' read as '{target}'")));
            continue;
        };
        let (mut used, mut kept) = (0, Vec::new());
        for r in t.rows.iter_mut() {
            let ids = split_values(&r.cells[ic], &sc.value_separators, &sub);
            let terms = split_values(&r.cells[tc], &sc.value_separators, &sub);
            if ids.is_empty() {
                continue;
            }
            if terms.is_empty() || ids.len() == terms.len() {
                r.cells[tc] = ids.join(&sub);
                used += 1;
            } else {
                kept.push(r.line);
            }
        }
        if used > 0 {
            t.notes.push((false, 0, format!("{idcol}: identifiers used for {target} on {used} row(s) (headings come from the authority)")));
        }
        if !kept.is_empty() {
            let shown: Vec<String> = kept.iter().take(8).map(|l| l.to_string()).collect();
            t.notes.push((
                true,
                0,
                format!(
                    "{idcol} and {target} list a different number of values on {} row(s) ({}{}) - the headings were used",
                    kept.len(),
                    shown.join(", "),
                    if kept.len() > 8 { ", ..." } else { "" }
                ),
            ));
        }
        drop_column(t, ic);
    }

    let multi: Vec<usize> = t
        .headers
        .iter()
        .enumerate()
        .filter(|(_, h)| {
            ["islandora_object", "archival_component"].iter().any(|b| {
                cfg.schema
                    .fields(b)
                    .get(h.as_str())
                    .is_some_and(|f| f.cardinality != 1 && (f.is_taxonomy() || f.kind == "typed_relation"))
            })
        })
        .map(|(i, _)| i)
        .collect();
    let mut changed = 0;
    for r in t.rows.iter_mut() {
        for &i in &multi {
            if r.cells[i].is_empty() {
                continue;
            }
            let joined = split_values(&r.cells[i], &sc.value_separators, &sub).join(&sub);
            if joined != r.cells[i] {
                r.cells[i] = joined;
                changed += 1;
            }
        }
    }
    if changed > 0 {
        t.notes.push((false, 0, format!("{changed} multi-value cell(s) re-separated with '{sub}'")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_cases_headers() {
        assert_eq!(to_snake("localNote"), "local_note");
        assert_eq!(to_snake("Custodial History"), "custodial_history");
        assert_eq!(to_snake("extent"), "extent");
        assert_eq!(to_snake("boxIdentifier"), "box_identifier");
    }

    #[test]
    fn renames_aliases_and_skips_blank_rows() {
        let mut aliases = HashMap::new();
        aliases.insert("file_extention".to_string(), "file_extension".to_string());
        let t = parse(
            "x",
            b"\xef\xbb\xbfaccessIdentifier,file_extention\n a_01 ,jpg\n,\n",
            &aliases,
        )
        .unwrap();
        assert_eq!(t.headers, vec!["accessIdentifier", "file_extension"]);
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.get(&t.rows[0], "accessIdentifier"), "a_01");
        assert_eq!(t.renamed.len(), 1);
    }
}
