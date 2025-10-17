use crate::csv_modifier::{normalize_cell, ColumnModifier, RowContext};

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
