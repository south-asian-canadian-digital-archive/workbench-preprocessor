use crate::csv_modifier::{ColumnModifier, RowContext};

fn ensure_wrapped_in_quotes(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".to_string();
    }

    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value.to_string()
    } else {
        let mut wrapped = String::with_capacity(value.len() + 2);
        wrapped.push('"');
        wrapped.push_str(value);
        wrapped.push('"');
        wrapped
    }
}

pub struct FieldDescriptionSemicolonEscaper;

impl ColumnModifier for FieldDescriptionSemicolonEscaper {
    fn modify(&self, value: &str, _row: &RowContext) -> String {
        if value.is_empty() {
            return ensure_wrapped_in_quotes(value);
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

        let escaped = if changed { result } else { value.to_string() };

        ensure_wrapped_in_quotes(&escaped)
    }

    fn description(&self) -> &str {
        "Escapes unescaped semicolons in field_description"
    }
}
