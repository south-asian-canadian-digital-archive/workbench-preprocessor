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

        let mut parent_data: HashMap<String, (String, usize)> = HashMap::new();
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

                parent_data
                    .entry(parent_id_clean.to_string())
                    .and_modify(|(existing_title, count)| {
                        if existing_title.is_empty() && !file_title_clean.is_empty() {
                            *existing_title = file_title_clean.to_string();
                        }
                        *count += 1;
                    })
                    .or_insert((file_title_clean.to_string(), 1));
            }
        }

        stats.unique_parents = parent_data.len();

        let output_file = File::create(output_path).context("Failed to create output file")?;
        let mut writer = Writer::from_writer(output_file);

    writer.write_record(["file_identifier", "title", "# of items", "field_member_of"])?;

        let mut sorted_data: Vec<_> = parent_data.into_iter().collect();
        sorted_data.sort_by(|a, b| a.0.cmp(&b.0));

        let node_value = node.unwrap_or("");

        for (file_identifier, (title, count)) in sorted_data {
            let count_str = count.to_string();
            writer.write_record([
                file_identifier.as_str(),
                title.as_str(),
                count_str.as_str(),
                node_value,
            ])?;
        }

        writer.flush()?;
        Ok(stats)
    }
}
