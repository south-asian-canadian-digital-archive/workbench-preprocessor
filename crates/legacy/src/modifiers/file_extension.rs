use crate::csv_modifier::{normalize_cell, ColumnModifier, RowContext};

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

        let parent_id = if let Some(last_underscore) = access_identifier.rfind('_') {
            &access_identifier[..last_underscore]
        } else {
            access_identifier
        };

        let base_name = if let Some(dot_pos) = value_clean.rfind('.') {
            &value_clean[..dot_pos]
        } else {
            value_clean
        };

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
