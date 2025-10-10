use anyhow::{Context, Result};
use csv::{Reader, Writer};
use encoding_rs::WINDOWS_1252;
use log::warn;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;

fn normalize_cell(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("#value!") {
        ""
    } else {
        trimmed
    }
}

fn is_effectively_empty(value: &str) -> bool {
    normalize_cell(value).is_empty()
}

fn contains_mojibake_markers(value: &str) -> bool {
    value
        .chars()
        .any(|c| matches!(c, 'Ã' | 'â' | '€' | '™' | 'œ' | '¢' | '‰' | 'Š' | 'ž' | 'Â'))
}

fn fix_common_mojibake(value: &str) -> Option<String> {
    if !contains_mojibake_markers(value) {
        return None;
    }

    let (encoded, _, had_errors) = WINDOWS_1252.encode(value);
    if had_errors {
        return None;
    }

    let candidate = encoded.into_owned();
    match String::from_utf8(candidate) {
        Ok(decoded) if decoded != value => Some(decoded),
        _ => None,
    }
}

fn sanitize_text_in_place(value: &mut String) -> bool {
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

fn ensure_wrapped_in_quotes(value: &mut String) -> bool {
    if value.is_empty() {
        *value = "\"\"".to_string();
        return true;
    }

    if value.starts_with('"') && value.ends_with('"') {
        return false;
    }

    let mut escaped = value.replace('"', "\"\"");
    escaped.insert(0, '"');
    escaped.push('"');
    *value = escaped;
    true
}

pub trait ColumnModifier {
    fn modify(&self, value: &str, row: &RowContext) -> String;
    fn description(&self) -> &str;
    fn validate(&self, _value: &str, _row: &RowContext) -> bool {
        true
    }
}

#[derive(Debug)]
pub struct RowContext<'a> {
    headers: &'a [String],
    values: &'a [String],
    row_index: usize,
}

impl<'a> RowContext<'a> {
    pub fn new(headers: &'a [String], values: &'a [String], row_index: usize) -> Self {
        Self {
            headers,
            values,
            row_index,
        }
    }

    /// Get the current row index (0-based, excluding header)
    pub fn row_index(&self) -> usize {
        self.row_index
    }

    pub fn get(&self, column: &str) -> Option<&str> {
        self.headers
            .iter()
            .position(|h| h == column)
            .and_then(|i| self.values.get(i).map(|s| s.as_str()))
    }

    pub fn get_or_empty(&self, column: &str) -> &str {
        self.get(column).map(normalize_cell).unwrap_or("")
    }

    pub fn get_first_non_empty(&self, columns: &[&str]) -> Option<&str> {
        columns
            .iter()
            .filter_map(|column| self.get(column))
            .map(normalize_cell)
            .find(|value| !value.is_empty())
    }
}

pub struct CsvModifier {
    column_modifiers: BTreeMap<String, Box<dyn ColumnModifier>>,
}

impl CsvModifier {
    pub fn new() -> Self {
        let mut column_modifiers: BTreeMap<String, Box<dyn ColumnModifier>> = BTreeMap::new();
        column_modifiers.insert(
            "accessIdentifier".to_string(),
            Box::new(AccessIdentifierValidator),
        );
        column_modifiers.insert(
            "field_description".to_string(),
            Box::new(FieldDescriptionSemicolonEscaper),
        );

        Self { column_modifiers }
    }

    pub fn add_column_modifier<M>(mut self, column: &str, modifier: M) -> Self
    where
        M: ColumnModifier + 'static,
    {
        self.column_modifiers
            .insert(column.to_string(), Box::new(modifier));
        self
    }

    /// Process CSV from a file path
    pub fn process_file(&self, input_path: &str, output_path: &str) -> Result<ProcessingStats> {
        let mut reader =
            Reader::from_reader(File::open(input_path).context("Failed to open input file")?);
        self.process_csv_reader(&mut reader, output_path)
    }

    /// Internal method to process CSV from any reader
    pub(crate) fn process_csv_reader<R: std::io::Read>(
        &self,
        reader: &mut Reader<R>,
        output_path: &str,
    ) -> Result<ProcessingStats> {
        let headers = reader.headers()?.clone();
        let headers: Vec<String> = headers.iter().map(|h| h.to_string()).collect();

        let header_map: HashMap<String, usize> = headers
            .iter()
            .enumerate()
            .map(|(i, h)| (h.clone(), i))
            .collect();

        let title_column = ["title", "fileTitle"]
            .iter()
            .find_map(|name| header_map.get(*name).copied().map(|index| (index, *name)));
        let field_description_column = header_map.get("field_description").copied();

        let output_file = File::create(output_path).context("Failed to create output file")?;
        let mut writer = Writer::from_writer(output_file);

        // Write headers to output
        writer.write_record(&headers)?;

        let mut stats = ProcessingStats::new();

        // Stream processing for column modifiers
        let mut validation_logging_suppressed = false;
        let mut seen_access_identifiers: HashSet<String> = HashSet::new();
        for (row_idx, result) in reader.records().enumerate() {
            let record = result?;
            let mut row_values: Vec<String> = record.iter().map(|s| s.to_string()).collect();
            let mut row_valid = true;
            let mut current_access_identifier: Option<String> = None;
            let mut sanitized_cells = 0;

            for cell in row_values.iter_mut() {
                if sanitize_text_in_place(cell) {
                    sanitized_cells += 1;
                }
            }
            if sanitized_cells > 0 {
                stats.cells_modified += sanitized_cells;
            }

            if let Some((title_idx, title_name)) = title_column {
                let title_value = row_values
                    .get(title_idx)
                    .map(|value| normalize_cell(value.as_str()))
                    .unwrap_or("");

                if title_value.is_empty() {
                    stats.validation_failures += 1;

                    if let Some(first_cell) = row_values.get_mut(0) {
                        if first_cell.starts_with('#') {
                            // already marked
                        } else if first_cell.is_empty() {
                            first_cell.push('#');
                        } else {
                            first_cell.insert(0, '#');
                        }
                    }

                    if stats.validation_failures <= 25 {
                        warn!(
                            "Validation failed for column '{}' at row {}. Reason: empty value detected; row marked and skipped.",
                            title_name,
                            row_idx + 1
                        );
                    } else if !validation_logging_suppressed {
                        warn!(
                            "More than 25 validation failures encountered. Suppressing additional validation logs to avoid noise."
                        );
                        validation_logging_suppressed = true;
                    }

                    stats.skipped_rows += 1;
                    continue;
                }
            }

            for (column_name, modifier) in &self.column_modifiers {
                if let Some(&col_index) = header_map.get(column_name) {
                    if let Some(cell) = row_values.get(col_index) {
                        let row_context = RowContext::new(&headers, &row_values, row_idx);

                        if modifier.validate(cell, &row_context) {
                            let mut duplicate_detected = false;

                            if column_name.as_str() == "accessIdentifier" {
                                let normalized_value = normalize_cell(cell.as_str());
                                if !normalized_value.is_empty() {
                                    if seen_access_identifiers.contains(normalized_value) {
                                        stats.validation_failures += 1;

                                        if stats.validation_failures <= 25 {
                                            warn!(
                                                "Duplicate accessIdentifier '{}' detected at row {}. Skipping row.",
                                                normalized_value,
                                                row_idx + 1
                                            );
                                        } else if !validation_logging_suppressed {
                                            warn!(
                                                "More than 25 validation failures encountered. Suppressing additional validation logs to avoid noise."
                                            );
                                            validation_logging_suppressed = true;
                                        }

                                        duplicate_detected = true;
                                    } else {
                                        current_access_identifier =
                                            Some(normalized_value.to_string());
                                    }
                                }
                            }

                            if duplicate_detected {
                                drop(row_context);
                                row_valid = false;
                                break;
                            }

                            let original = cell.clone();
                            let new_value = modifier.modify(cell, &row_context);
                            drop(row_context);

                            if let Some(cell_mut) = row_values.get_mut(col_index) {
                                if original != new_value {
                                    stats.cells_modified += 1;
                                }
                                *cell_mut = new_value;
                            }
                        } else {
                            stats.validation_failures += 1;
                            let row_number = row_idx + 1;
                            let original_cell_value = cell.clone();
                            let sanitized_cell = normalize_cell(&original_cell_value).to_string();
                            let access_identifier_raw = row_context
                                .get("accessIdentifier")
                                .map(|value| value.to_string())
                                .unwrap_or_default();
                            let access_identifier_clean =
                                normalize_cell(&access_identifier_raw).to_string();
                            let file_extension_primary_raw = row_context
                                .get("file_extension")
                                .map(|value| value.to_string())
                                .unwrap_or_default();
                            let file_extension_primary_clean =
                                normalize_cell(&file_extension_primary_raw).to_string();
                            let file_extension_alt_raw = row_context
                                .get("file_extention")
                                .map(|value| value.to_string())
                                .unwrap_or_default();
                            let file_extension_alt_clean =
                                normalize_cell(&file_extension_alt_raw).to_string();
                            let effective_file_extension =
                                if !file_extension_primary_clean.is_empty() {
                                    file_extension_primary_clean.as_str()
                                } else {
                                    file_extension_alt_clean.as_str()
                                };

                            drop(row_context);

                            if sanitized_cell.is_empty()
                                && !original_cell_value.trim().is_empty()
                                && column_name.as_str() == "parent_id"
                            {
                                if let Some(cell_mut) = row_values.get_mut(col_index) {
                                    if !cell_mut.is_empty() {
                                        cell_mut.clear();
                                        stats.cells_modified += 1;
                                    }
                                }
                            }

                            if sanitized_cell.is_empty()
                                && !original_cell_value.trim().is_empty()
                                && column_name.as_str() == "file"
                            {
                                if let Some(cell_mut) = row_values.get_mut(col_index) {
                                    if !cell_mut.is_empty() {
                                        cell_mut.clear();
                                        stats.cells_modified += 1;
                                    }
                                }
                            }

                            if stats.validation_failures <= 25 {
                                let mut missing_fields = Vec::new();

                                if is_effectively_empty(&original_cell_value) {
                                    missing_fields.push(column_name.as_str());
                                }

                                if effective_file_extension.is_empty() {
                                    if file_extension_primary_clean.is_empty()
                                        && file_extension_alt_clean.is_empty()
                                    {
                                        missing_fields.push("file_extension/file_extention");
                                    } else if file_extension_primary_clean.is_empty() {
                                        missing_fields.push("file_extension");
                                    } else {
                                        missing_fields.push("file_extention");
                                    }
                                }

                                if access_identifier_clean.is_empty() {
                                    missing_fields.push("accessIdentifier");
                                }

                                let reason = if missing_fields.is_empty() {
                                    "validation predicate returned false without missing fields"
                                        .to_string()
                                } else {
                                    format!("missing {}", missing_fields.join(", "))
                                };

                                warn!(
                                    "Validation failed for column '{}' at row {} using modifier '{}'. Current value='{}' (normalized='{}'). accessIdentifier='{}', file_extension='{}', file_extention='{}'. Reason: {}",
                                    column_name,
                                    row_number,
                                    modifier.description(),
                                    original_cell_value,
                                    sanitized_cell,
                                    access_identifier_raw,
                                    file_extension_primary_raw,
                                    file_extension_alt_raw,
                                    reason
                                );
                            } else if !validation_logging_suppressed {
                                warn!(
                                    "More than 25 validation failures encountered. Suppressing additional validation logs to avoid noise."
                                );
                                validation_logging_suppressed = true;
                            }

                            if column_name == "accessIdentifier" {
                                row_valid = false;
                                break;
                            }
                        }
                    }
                }
                if !row_valid {
                    break;
                }
            }

            if !row_valid {
                stats.skipped_rows += 1;
                continue;
            }

            if let Some(identifier) = current_access_identifier {
                seen_access_identifiers.insert(identifier);
            }

            if let Some(field_idx) = field_description_column {
                if let Some(cell) = row_values.get_mut(field_idx) {
                    if ensure_wrapped_in_quotes(cell) {
                        stats.cells_modified += 1;
                    }
                }
            }

            writer.write_record(&row_values)?;
            stats.total_rows += 1;
        }

        for column_name in self.column_modifiers.keys() {
            stats.columns_processed.insert(column_name.clone());
        }

        writer.flush()?;
        Ok(stats)
    }
}

#[derive(Debug)]
pub struct ProcessingStats {
    pub total_rows: usize,
    pub cells_modified: usize,
    pub validation_failures: usize,
    pub skipped_rows: usize, // Track skipped rows
    pub columns_processed: std::collections::HashSet<String>,
}

impl ProcessingStats {
    pub fn new() -> Self {
        Self {
            total_rows: 0,
            cells_modified: 0,
            validation_failures: 0,
            skipped_rows: 0, // Initialize skipped_rows
            columns_processed: std::collections::HashSet::new(),
        }
    }
}

pub struct AccessIdentifierValidator;

impl ColumnModifier for AccessIdentifierValidator {
    fn modify(&self, value: &str, _row: &RowContext) -> String {
        normalize_cell(value).to_string()
    }

    fn description(&self) -> &str {
        "Validates accessIdentifier for item-level suitability"
    }

    fn validate(&self, value: &str, _row: &RowContext) -> bool {
        let clean = normalize_cell(value);

        if clean.is_empty() {
            return false;
        }

        !(clean.ends_with("_00") || clean.ends_with("_000"))
    }
}

pub struct FieldDescriptionSemicolonEscaper;

impl ColumnModifier for FieldDescriptionSemicolonEscaper {
    fn modify(&self, value: &str, _row: &RowContext) -> String {
        if value.is_empty() {
            return String::new();
        }

        let mut result = String::with_capacity(value.len());
        let mut changed = false;
        let mut previous = None;

        for ch in value.chars() {
            if ch == ';' {
                if previous != Some('\\') {
                    result.push('\\');
                    changed = true;
                }
                result.push(';');
            } else {
                result.push(ch);
            }
            previous = Some(ch);
        }

        if changed {
            result
        } else {
            value.to_string()
        }
    }

    fn description(&self) -> &str {
        "Escapes unescaped semicolons in field_description"
    }
}

pub struct ParentIdModifier;

impl ColumnModifier for ParentIdModifier {
    fn modify(&self, _value: &str, row: &RowContext) -> String {
        let access_identifier = row.get_or_empty("accessIdentifier");

        if !access_identifier.is_empty() {
            // Extract parent_id by removing the last part after the last underscore
            // Example: "2024_19_01_001" -> "2024_19_01"
            if let Some(last_underscore) = access_identifier.rfind('_') {
                access_identifier[..last_underscore].to_string()
            } else {
                access_identifier.to_string()
            }
        } else {
            String::new()
        }
    }

    fn description(&self) -> &str {
        "Extracts parent_id from accessIdentifier by removing the last underscore segment"
    }

    fn validate(&self, _value: &str, row: &RowContext) -> bool {
        !row.get_or_empty("accessIdentifier").is_empty()
    }
}

pub struct FileExtensionModifier;

impl ColumnModifier for FileExtensionModifier {
    fn modify(&self, value: &str, row: &RowContext) -> String {
        let file_extension = row
            .get_first_non_empty(&["file_extension", "file_extention"])
            .unwrap_or("");
        let access_identifier = row.get_or_empty("accessIdentifier");
        let value_clean = normalize_cell(value);

        if file_extension.is_empty() || value_clean.is_empty() || access_identifier.is_empty() {
            return value_clean.to_string();
        }

        // Extract parent_id from accessIdentifier
        let parent_id = if let Some(last_underscore) = access_identifier.rfind('_') {
            &access_identifier[..last_underscore]
        } else {
            access_identifier
        };

        // Remove existing extension from file if present
        let base_name = if let Some(dot_pos) = value_clean.rfind('.') {
            &value_clean[..dot_pos]
        } else {
            value_clean
        };

        // Create path: parent_id/filename.extension
        format!("{}/{}.{}", parent_id, base_name, file_extension)
    }

    fn description(&self) -> &str {
        "Creates file path with parent_id directory and file extension from accessIdentifier"
    }

    fn validate(&self, value: &str, row: &RowContext) -> bool {
        let has_value = !normalize_cell(value).is_empty();
        let has_extension = row
            .get_first_non_empty(&["file_extension", "file_extention"])
            .is_some();
        let has_access_identifier = !row.get_or_empty("accessIdentifier").is_empty();

        has_value && has_extension && has_access_identifier
    }
}
