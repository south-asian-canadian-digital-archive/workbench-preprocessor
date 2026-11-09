//! Loads the archivist's spreadsheet into memory, cleaned: cells trimmed,
//! `#VALUE!` blanked, non-breaking spaces and common mojibake repaired, and old
//! header names renamed. Every rename is recorded so the report can tell the
//! archivist to fix the sheet itself.

use crate::csv_modifier::{normalize_cell, sanitize_text_in_place, CsvModifier};
use crate::spreadsheet::csv_bytes_from_path;
use anyhow::{bail, Context, Result};
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
