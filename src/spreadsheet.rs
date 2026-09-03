//! Convert supported local table formats into CSV bytes for the existing pipeline.

use anyhow::{bail, Context, Result};
use calamine::{open_workbook_auto, Reader};
use csv::{ReaderBuilder, WriterBuilder};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;

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
    let input = File::open(path).with_context(|| format!("Failed to open input file: {}", path.display()))?;
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
    let mut writer = WriterBuilder::new().from_writer(Vec::new());
    for row in range.rows() {
        writer.write_record(row.iter().map(ToString::to_string))?;
    }
    Ok(writer.into_inner()?)
}

pub fn csv_reader_from_path(path: &str, sheet_name: Option<&str>) -> Result<csv::Reader<Cursor<Vec<u8>>>> {
    Ok(csv::Reader::from_reader(Cursor::new(csv_bytes_from_path(path, sheet_name)?)))
}
