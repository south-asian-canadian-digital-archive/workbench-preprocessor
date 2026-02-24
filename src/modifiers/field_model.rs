use crate::csv_modifier::{normalize_cell, ColumnModifier, RowContext};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

const DEFAULT_TOML_STR: &str = include_str!("field_model_mappings.toml");

#[derive(Debug, Deserialize)]
struct ModelCategory {
    model: String,
    #[serde(default)]
    extensions: Vec<String>,
    #[serde(default)]
    _description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DefaultModel {
    model: String,
    #[serde(default)]
    _description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FieldModelConfig {
    #[serde(default)]
    extension_lookup: HashMap<String, String>,
    #[serde(default)]
    default: Option<DefaultModel>,
    #[serde(flatten)]
    categories: HashMap<String, ModelCategory>,
}

pub struct FieldModelModifier {
    mappings: HashMap<String, String>,
    default_model: String,
}

impl FieldModelModifier {
    pub fn from_default_config() -> Result<Self> {
        Self::from_toml_str(DEFAULT_TOML_STR)
    }

    pub fn from_toml_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        use std::fs;
        let contents = fs::read_to_string(&path).with_context(|| {
            format!(
                "Failed to read field model mapping configuration from {}",
                path.as_ref().display()
            )
        })?;
        Self::from_toml_str(&contents)
    }

    fn from_toml_str(toml_str: &str) -> Result<Self> {
        let config: FieldModelConfig = toml::from_str(toml_str)
            .context("Failed to parse field model mapping configuration")?;

        let mut mappings: HashMap<String, String> = HashMap::new();

        for (ext, model) in config.extension_lookup {
            mappings.insert(normalize_extension(&ext), model);
        }

        for category in config.categories.values() {
            for ext in &category.extensions {
                mappings
                    .entry(normalize_extension(ext))
                    .or_insert_with(|| category.model.clone());
            }
        }

        let default_model = config
            .default
            .map(|d| d.model)
            .unwrap_or_else(|| "Binary".to_string());

        Ok(Self {
            mappings,
            default_model,
        })
    }

    fn model_for_extension(&self, extension: &str) -> &str {
        let key = normalize_extension(extension);
        if key.is_empty() {
            return &self.default_model;
        }

        self.mappings
            .get(&key)
            .map(|s| s.as_str())
            .unwrap_or(&self.default_model)
    }
}

impl ColumnModifier for FieldModelModifier {
    fn modify(&self, value: &str, row: &RowContext) -> String {
        let extension = row
            .get_first_non_empty(&["file_extension", "file_extention"])
            .unwrap_or("");
        let target_model = self.model_for_extension(extension);
        let current_value = normalize_cell(value);

        if current_value == target_model {
            current_value.to_string()
        } else {
            target_model.to_string()
        }
    }

    fn description(&self) -> &str {
        "Populates field_model based on configured file extension mappings"
    }
}

fn normalize_extension(value: &str) -> String {
    value
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_default_mappings() {
        let modifier = FieldModelModifier::from_default_config().unwrap();
        assert_eq!(modifier.model_for_extension("jpg"), "Image");
        assert_eq!(modifier.model_for_extension("mp3"), "Audio");
    }

    #[test]
    fn uses_default_for_unknown_extension() {
        let modifier = FieldModelModifier::from_default_config().unwrap();
        assert_eq!(modifier.model_for_extension("unknown"), "Binary");
    }

    #[test]
    fn normalize_extension_handles_dots_and_case() {
        assert_eq!(normalize_extension(".JPG"), "jpg");
    }

    #[test]
    fn modifies_field_model_based_on_extension() {
        let modifier = FieldModelModifier::from_default_config().unwrap();
        let headers = vec!["file_extension".to_string(), "field_model".to_string()];
        let values = vec!["jpg".to_string(), String::new()];
        let context = RowContext::new(&headers, &values, 0);

        let updated = modifier.modify("", &context);
        assert_eq!(updated, "Image");
    }

    #[test]
    fn falls_back_to_default_when_extension_missing() {
        let modifier = FieldModelModifier::from_default_config().unwrap();
        let headers = vec!["field_model".to_string()];
        let values = vec![String::new()];
        let context = RowContext::new(&headers, &values, 0);

        let updated = modifier.modify("", &context);
        assert_eq!(updated, "Binary");
    }
}
