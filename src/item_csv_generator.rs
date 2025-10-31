use anyhow::{Context, Result};
use csv::{Reader, Writer};
use std::collections::HashMap;
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

/// Attempt to extract a (year, optional month) from a free-form date string.
/// Heuristics (no external crates):
/// - Find first 4-digit year (1000..=2999)
/// - Prefer month adjacent to the year with '-' or '/' as delimiter
///   - After the year (YYYY[-/]MM)
///   - Or before the year (MM[-/]YYYY)
/// - If no adjacent month found, returns (year, None)
fn parse_year_and_month(value: &str) -> Option<(u16, Option<u8>)> {
    let s = value.trim();
    if s.is_empty() {
        return None;
    }

    let bytes = s.as_bytes();

    // Helper to parse 1-2 digit month at position range [start, end)
    fn parse_month_slice(bytes: &[u8], start: usize, end: usize) -> Option<u8> {
        if start >= end || end > bytes.len() {
            return None;
        }
        let slice = &bytes[start..end];
        if slice.iter().all(|b| b.is_ascii_digit()) {
            if let Ok(m) = std::str::from_utf8(slice).ok()?.parse::<u8>() {
                if (1..=12).contains(&m) {
                    return Some(m);
                }
            }
        }
        None
    }

    // Scan for first 4 consecutive digits as year
    let mut year_pos: Option<(usize, u16)> = None;
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..i + 4].iter().all(|b| b.is_ascii_digit()) {
            if let Ok(y) = std::str::from_utf8(&bytes[i..i + 4]).ok()?.parse::<u16>() {
                if (1000..=2999).contains(&y) {
                    year_pos = Some((i, y));
                    break;
                }
            }
            i += 1;
        } else {
            i += 1;
        }
    }

    let (y_idx, year) = year_pos?;

    // Try month after year: YYYY[-/]MM
    if y_idx + 5 <= bytes.len() {
        let delim = bytes.get(y_idx + 4).copied();
        if matches!(delim, Some(b'-' | b'/')) {
            // Try 2-digit then 1-digit
            if let Some(m) = parse_month_slice(bytes, y_idx + 5, (y_idx + 7).min(bytes.len())) {
                return Some((year, Some(m)));
            }
        }
    }

    // Try month before year: MM[-/]YYYY
    if y_idx >= 2 {
        let delim_pos = y_idx.saturating_sub(1);
        let delim = bytes.get(delim_pos).copied();
        if matches!(delim, Some(b'-' | b'/')) {
            // Look 1-2 digits before delimiter
            if delim_pos >= 2 {
                if let Some(m) = parse_month_slice(bytes, delim_pos - 2, delim_pos) {
                    return Some((year, Some(m)));
                }
            }
            if let Some(m) = parse_month_slice(bytes, delim_pos - 1, delim_pos) {
                return Some((year, Some(m)));
            }
        }
    }

    Some((year, None))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ItemGenerationStats {
    pub unique_parents: usize,
    pub total_items: usize,
    pub skipped_rows: usize,
}

pub struct ItemCsvGenerator;

impl ItemCsvGenerator {
    pub fn generate(
        input_path: &str,
        output_path: &str,
        node: Option<&str>,
    ) -> Result<ItemGenerationStats> {
        let file = File::open(input_path).context("Failed to open input file")?;
        let mut reader = Reader::from_reader(file);

        let headers = reader.headers()?.clone();
        let headers: Vec<String> = headers.iter().map(|h| h.to_string()).collect();

        let parent_id_idx = headers
            .iter()
            .position(|h| h == "parent_id")
            .context("Column 'parent_id' not found in CSV. Please ensure the input file has been processed with parent_id modifier.")?;
        let file_title_idx = headers
            .iter()
            .position(|h| h == "fileTitle")
            .context("Column 'fileTitle' not found in CSV. Please ensure the input file contains a fileTitle column.")?;
        let field_date_idx_opt = headers.iter().position(|h| h == "field_date");

        #[derive(Default)]
        struct GroupData {
            title: String,
            count: usize,
            year_month_counts: HashMap<(u16, u8), usize>,
            year_counts: HashMap<u16, usize>,
            total_date_samples: usize,
        }

        let mut parent_data: HashMap<String, GroupData> = HashMap::with_capacity(256); // Pre-allocate
        let mut stats = ItemGenerationStats::default();

        for result in reader.records() {
            let record = result?;
            stats.total_items += 1;

            if let (Some(parent_id_raw), Some(file_title_raw)) =
                (record.get(parent_id_idx), record.get(file_title_idx))
            {
                if is_effectively_empty(parent_id_raw) {
                    stats.skipped_rows += 1;
                    continue;
                }

                let parent_id_clean = normalize_cell(parent_id_raw);
                let file_title_clean = normalize_cell(file_title_raw);

                let entry = parent_data
                    .entry(parent_id_clean.to_string())
                    .or_default();

                if entry.title.is_empty() && !file_title_clean.is_empty() {
                    entry.title = file_title_clean.to_string();
                }
                entry.count += 1;

                // Prefer explicit field_date; fall back to parsing from the file title
                let mut date_source: Option<&str> = None;
                if let Some(idx) = field_date_idx_opt {
                    if let Some(date_raw) = record.get(idx) {
                        let candidate = normalize_cell(date_raw);
                        if !candidate.is_empty() {
                            date_source = Some(candidate);
                        }
                    }
                }
                if date_source.is_none() && !file_title_clean.is_empty() {
                    date_source = Some(file_title_clean);
                }

                if let Some(src) = date_source {
                    if let Some((year, maybe_month)) = parse_year_and_month(src) {
                        entry.total_date_samples += 1;
                        *entry.year_counts.entry(year).or_insert(0) += 1;
                        if let Some(m) = maybe_month {
                            *entry.year_month_counts.entry((year, m)).or_insert(0) += 1;
                        }
                    }
                }
            }
        }

        stats.unique_parents = parent_data.len();

        let output_file = File::create(output_path).context("Failed to create output file")?;
        let mut writer = Writer::from_writer(output_file);

        writer.write_record(["file_identifier", "title", "# of items", "field_member_of", "field_date"])?;

        let mut sorted_data: Vec<_> = parent_data.into_iter().collect();
        sorted_data.sort_by(|a, b| a.0.cmp(&b.0));

        let node_value = node.unwrap_or("");

        for (file_identifier, group) in sorted_data {
            let count_str = group.count.to_string();

            // Decide field_date for the group:
            let field_date_value = if group.total_date_samples == 0 {
                String::new()
            } else {
                // Prefer a dominant month+year if present
                let dominant_ym = group
                    .year_month_counts
                    .iter()
                    .max_by_key(|((_y, _m), c)| *c)
                    .map(|(&(y, m), &c)| (y, m, c));

                if let Some((y, m, c)) = dominant_ym {
                    if c * 2 > group.total_date_samples {
                        // Format MM/YYYY
                        format!("{:02}/{}", m, y)
                    } else {
                        // Fallback to average year
                        let (sum, total): (u32, u32) = group
                            .year_counts
                            .iter()
                            .fold((0u32, 0u32), |(s, t), (&yy, &cnt)| (s + (yy as u32) * (cnt as u32), t + cnt as u32));
                        let avg = if total > 0 {
                            ((sum as f64) / (total as f64)).round() as u16
                        } else {
                            y
                        };
                        avg.to_string()
                    }
                } else {
                    // No month info found; average the year
                    let (sum, total): (u32, u32) = group
                        .year_counts
                        .iter()
                        .fold((0u32, 0u32), |(s, t), (&yy, &cnt)| (s + (yy as u32) * (cnt as u32), t + cnt as u32));
                    if total > 0 {
                        (((sum as f64) / (total as f64)).round() as u16).to_string()
                    } else {
                        String::new()
                    }
                }
            };

            writer.write_record([
                file_identifier.as_str(),
                group.title.as_str(),
                count_str.as_str(),
                node_value,
                field_date_value.as_str(),
            ])?;
        }

        writer.flush()?;
        Ok(stats)
    }
}
