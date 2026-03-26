use crate::csv_modifier::{normalize_cell, ColumnModifier, RowContext};

/// Fills the target column with the normalized value from another column (duplicate field).
pub struct CopyFromColumnModifier {
    pub source_column: &'static str,
}

impl CopyFromColumnModifier {
    pub const fn new(source_column: &'static str) -> Self {
        Self { source_column }
    }
}

impl ColumnModifier for CopyFromColumnModifier {
    fn modify(&self, _value: &str, row: &RowContext) -> String {
        row.get(self.source_column)
            .map(|s| normalize_cell(s).to_string())
            .unwrap_or_default()
    }

    fn description(&self) -> &str {
        "Copies normalized value from a source column"
    }
}
