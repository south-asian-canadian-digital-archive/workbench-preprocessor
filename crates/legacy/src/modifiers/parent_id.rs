use crate::csv_modifier::{ColumnModifier, RowContext};

pub struct ParentIdModifier;

impl ColumnModifier for ParentIdModifier {
    fn modify(&self, _value: &str, row: &RowContext) -> String {
        let access_identifier = row.get_or_empty("accessIdentifier");

        if !access_identifier.is_empty() {
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
