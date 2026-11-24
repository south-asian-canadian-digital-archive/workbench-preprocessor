//! Reading a sheet in: local tables (CSV, TSV, Excel, ODS) as CSV bytes, a
//! Google Sheets link downloaded as CSV, and the cell clean-up every value
//! gets on the way in.

use anyhow::{bail, Context, Result};
use calamine::{open_workbook_auto, Reader};
use csv::{ReaderBuilder, WriterBuilder};
use encoding_rs::WINDOWS_1252;
use std::fs::File;
use std::io::Read;
use std::path::Path;

// --- cells -------------------------------------------------------------------

/// A cell's value, trimmed; Excel's `#VALUE!` counts as empty.
pub fn normalize_cell(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("#value!") {
        ""
    } else {
        trimmed
    }
}
fn contains_mojibake_markers(value: &str) -> bool {
    value.chars().any(|c| {
        matches!(
            c,
            'Ã' | 'â'
                | '€'
                | '™'
                | 'œ'
                | 'Â'
                | 'Î'
                | '¢'
                | '‰'
                | 'Š'
                | 'ž'
                | '¡'
                | '«'
                | '»'
                | 'š'
                | '‚'
                | '„'
                | '¬'
        )
    })
}

fn fix_common_mojibake(value: &str) -> Option<String> {
    if !contains_mojibake_markers(value) {
        return None;
    }

    let (encoded, _, encode_had_errors) = WINDOWS_1252.encode(value);

    if encode_had_errors {
        return None;
    }

    match String::from_utf8(encoded.into_owned()) {
        Ok(decoded_string)
            if decoded_string != value && !contains_mojibake_markers(&decoded_string) =>
        {
            Some(decoded_string)
        }
        _ => None,
    }
}

pub fn sanitize_text_in_place(value: &mut String) -> bool {
    let mut changed = false;

    if value.contains('\u{00A0}') {
        *value = value.replace('\u{00A0}', " ");
        changed = true;
    }

    if let Some(decoded) = fix_common_mojibake(value) {
        if decoded != *value {
            *value = decoded;
            changed = true;
        }
    }

    changed
}

// --- local tables --------------------------------------------------------------

const WORKBOOK_EXTENSIONS: &[&str] = &["xls", "xlsx", "xlsb", "ods"];

pub fn supported_input_formats() -> &'static str {
    "CSV, TSV, Excel (.xls/.xlsx/.xlsb), or ODS"
}

pub fn csv_bytes_from_path(path: &str, sheet_name: Option<&str>) -> Result<Vec<u8>> {
    let path_ref = Path::new(path);
    let extension = path_ref
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    match extension.as_str() {
        "csv" => {
            if sheet_name.is_some() {
                bail!("--sheet is only valid for Excel and ODS workbooks");
            }
            let mut bytes = Vec::new();
            File::open(path_ref)
                .with_context(|| format!("Failed to open input file: {path}"))?
                .read_to_end(&mut bytes)?;
            Ok(bytes)
        }
        "tsv" => text_table_to_csv(path_ref, b'\t', sheet_name),
        ext if WORKBOOK_EXTENSIONS.contains(&ext) => workbook_to_csv(path_ref, sheet_name),
        _ => bail!(
            "Unsupported table format for {}. Use {}.",
            path_ref.display(),
            supported_input_formats()
        ),
    }
}

fn text_table_to_csv(path: &Path, delimiter: u8, sheet_name: Option<&str>) -> Result<Vec<u8>> {
    if sheet_name.is_some() {
        bail!("--sheet is only valid for Excel and ODS workbooks");
    }
    let input = File::open(path)
        .with_context(|| format!("Failed to open input file: {}", path.display()))?;
    let mut reader = ReaderBuilder::new()
        .has_headers(false)
        .delimiter(delimiter)
        .from_reader(input);
    let mut writer = WriterBuilder::new().from_writer(Vec::new());
    for row in reader.records() {
        writer.write_record(&row?)?;
    }
    Ok(writer.into_inner()?)
}

fn workbook_to_csv(path: &Path, sheet_name: Option<&str>) -> Result<Vec<u8>> {
    let mut workbook = open_workbook_auto(path)
        .with_context(|| format!("Failed to open workbook: {}", path.display()))?;
    let names = workbook.sheet_names().to_owned();
    let selected = sheet_name
        .map(str::to_owned)
        .or_else(|| names.first().cloned())
        .context("Workbook has no worksheets")?;
    if !names.iter().any(|name| name == &selected) {
        bail!(
            "Worksheet '{}' was not found in {}. Available worksheets: {}",
            selected,
            path.display(),
            names.join(", ")
        );
    }
    let range = workbook
        .worksheet_range(&selected)
        .with_context(|| format!("Failed to read worksheet '{selected}'"))?;
    // The range starts at the first cell with something in it: blank rows
    // above it still count, so row numbers match what Excel shows.
    let blank_above = range.start().map(|(r, _)| r as usize).unwrap_or(0);
    let mut writer = WriterBuilder::new().from_writer(b"\n".repeat(blank_above));
    for row in range.rows() {
        writer.write_record(row.iter().map(cell_text))?;
    }
    Ok(writer.into_inner()?)
}

/// A cell as text. Date cells become ISO dates (`1913-05-04`, or with the time
/// when there is one): their plain text is Excel's serial number, and one
/// before 1927 (`4872`) would pass as a year.
fn cell_text(cell: &calamine::Data) -> String {
    if let calamine::Data::DateTime(d) = cell {
        if d.is_datetime() {
            if let Some(dt) = d.as_datetime() {
                use chrono::{Datelike, Timelike};
                let date = format!("{:04}-{:02}-{:02}", dt.year(), dt.month(), dt.day());
                return if dt.time() == chrono::NaiveTime::MIN {
                    date
                } else {
                    format!("{date}T{:02}:{:02}:{:02}", dt.hour(), dt.minute(), dt.second())
                };
            }
        }
    }
    cell.to_string()
}

// --- Google Sheets ---------------------------------------------------------------

/// A Google Sheets link: the spreadsheet and, when the link names one, the tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetLink {
    pub id: String,
    /// `#gid=N` (what the address bar shows) or `?gid=N`. Without it the
    /// export is the first tab, whichever tab the link was for.
    pub gid: Option<String>,
}

impl SheetLink {
    pub fn parse(link: &str) -> Result<Self> {
        let url = url::Url::parse(link.trim()).context("that isn't a link")?;
        if url.host_str() != Some("docs.google.com") {
            bail!("only Google Sheets links (docs.google.com) are supported, not {}", url.host_str().unwrap_or("this"));
        }
        let id = url
            .path()
            .split_once("/spreadsheets/d/")
            .map(|(_, rest)| rest.split('/').next().unwrap_or_default())
            .filter(|id| id.len() >= 2 && *id != "edit" && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
            .context("that isn't a Google Sheets link (docs.google.com/spreadsheets/d/...)")?
            .to_string();
        let gid = url
            .fragment()
            .into_iter()
            .flat_map(|f| f.split('&'))
            .chain(url.query().into_iter().flat_map(|q| q.split('&')))
            .find_map(|kv| kv.strip_prefix("gid="))
            .filter(|g| !g.is_empty() && g.bytes().all(|b| b.is_ascii_digit()))
            .map(String::from);
        Ok(Self { id, gid })
    }

    /// The CSV export of the sheet (its tab, if the link named one).
    pub fn export_url(&self) -> String {
        let tab = self.gid.as_ref().map(|g| format!("&gid={g}")).unwrap_or_default();
        format!("https://docs.google.com/spreadsheets/d/{}/export?format=csv{tab}", self.id)
    }
}

/// Each downloaded sheet's own name, by export link, from the last download.
static TITLES: std::sync::Mutex<std::collections::BTreeMap<String, String>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

/// The sheet's name as Google gave it with the last download of `link`
/// ("ingest_box002 - primary": the document, then the tab).
pub fn sheet_title(link: &str) -> Option<String> {
    let key = SheetLink::parse(link).ok()?.export_url();
    TITLES.lock().ok()?.get(&key).cloned()
}

/// The file name in a Content-Disposition header, without `.csv`. The
/// `filename*` form is the exact one (`filename` has spaces etc. replaced).
fn disposition_title(v: &str) -> Option<String> {
    let param = |name: &str| {
        v.split(';').map(str::trim).find_map(|p| {
            let (k, val) = p.split_once('=')?;
            k.trim().eq_ignore_ascii_case(name).then(|| val.trim().trim_matches('"').to_string())
        })
    };
    let name = match param("filename*") {
        Some(enc) => {
            let enc = enc.split_once("''").map(|(_, rest)| rest.to_string()).unwrap_or(enc);
            let b = enc.as_bytes();
            let mut out = Vec::with_capacity(b.len());
            let mut i = 0;
            while i < b.len() {
                let hex = |c: u8| (c as char).to_digit(16);
                match (b[i], b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex)) {
                    (b'%', Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                    }
                    (c, _, _) => {
                        out.push(c);
                        i += 1;
                    }
                }
            }
            String::from_utf8_lossy(&out).to_string()
        }
        None => param("filename")?,
    };
    let name = name.strip_suffix(".csv").or_else(|| name.strip_suffix(".CSV")).unwrap_or(&name).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Download a Google Sheet (the tab the link names) as CSV text. Its name
/// is kept for `sheet_title`.
pub fn fetch_sheet(link: &str) -> Result<String> {
    let url = SheetLink::parse(link)?.export_url();
    let response = reqwest::blocking::get(&url).with_context(|| format!("couldn't download {url}"))?;
    if !response.status().is_success() {
        bail!("Google answered {} for the sheet", response.status());
    }
    let html = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"));
    let title = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(disposition_title);
    let text = response.text().context("couldn't read the sheet Google sent")?;
    // A sheet that isn't shared answers 200 with Google's sign-in page.
    if html || text.trim_start().starts_with("<!DOCTYPE html") || text.trim_start().starts_with("<html") {
        bail!("Google sent a web page instead of the sheet - share it as \"Anyone with the link can view\" and try again");
    }
    if let (Some(t), Ok(mut titles)) = (title, TITLES.lock()) {
        titles.insert(url, t);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_name_from_the_download() {
        let h = r#"attachment; filename="ingest_box002-primary.csv"; filename*=UTF-8''ingest_box002%20-%20primary.csv"#;
        assert_eq!(disposition_title(h).as_deref(), Some("ingest_box002 - primary"));
        assert_eq!(disposition_title(r#"attachment; filename="Box 3.csv""#).as_deref(), Some("Box 3"));
        assert_eq!(disposition_title("attachment; filename*=UTF-8''Gurdw%C4%81r%C4%81%20list.csv").as_deref(), Some("Gurdwārā list"));
        assert_eq!(disposition_title("attachment"), None);
    }

    #[test]
    fn sheet_links() {
        let l = SheetLink::parse("https://docs.google.com/spreadsheets/d/1Bxi-MV_s0XRA/edit?usp=sharing").unwrap();
        assert_eq!(l, SheetLink { id: "1Bxi-MV_s0XRA".into(), gid: None });
        assert_eq!(l.export_url(), "https://docs.google.com/spreadsheets/d/1Bxi-MV_s0XRA/export?format=csv");
        let l = SheetLink::parse("https://docs.google.com/spreadsheets/d/abc123/edit?usp=sharing#gid=789").unwrap();
        assert_eq!(l.export_url(), "https://docs.google.com/spreadsheets/d/abc123/export?format=csv&gid=789");
        assert_eq!(SheetLink::parse("https://docs.google.com/spreadsheets/d/abc123").unwrap().gid, None);
        for bad in [
            "https://example.com/spreadsheet",
            "https://sheets.google.com/spreadsheets/d/123/edit",
            "https://docs.google.com/spreadsheets/d/",
            "https://docs.google.com/spreadsheets/d/edit",
            "https://docs.google.com/document/d/123/edit",
            "https://docs.google.com/spreadsheets/d/../x",
            "not a link",
            "",
        ] {
            assert!(SheetLink::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn cells() {
        assert_eq!(normalize_cell("  #VALUE!  "), "");
        assert_eq!(normalize_cell(" x "), "x");
        let mut s = "CafÃ©\u{00A0}Bar".to_string();
        assert!(sanitize_text_in_place(&mut s));
        assert_eq!(s, "Café Bar");
    }
}
