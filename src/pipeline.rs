use crate::csv_modifier::{CsvModifier, ProcessingStats};
use crate::item_csv_generator::{ItemCsvGenerator, ItemGenerationStats};
use crate::modifiers::{
    FieldModelModifier, FileExtensionModifier, ParentIdModifier,
};
use crate::{Modifier};
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;

#[derive(Debug)]
pub struct ProcessResult {
    pub processed_output_path: String,
    pub processing_stats: ProcessingStats,
    pub items_output_path: Option<String>,
    pub items_stats: Option<ItemGenerationStats>,
}

fn determine_modifiers_to_run(
    only_run: &[Modifier],
    ignore_run: &[Modifier],
) -> HashSet<Modifier> {
    let all_modifiers = [Modifier::ParentId, Modifier::FileExtension, Modifier::FieldModel];

    let mut active_modifiers: HashSet<Modifier> = if only_run.is_empty() {
        // Default behavior: run all modifiers
        all_modifiers.into_iter().collect()
    } else {
        only_run.iter().cloned().collect()
    };

    // Remove ignored modifiers
    for modifier in ignore_run {
        active_modifiers.remove(modifier);
    }

    active_modifiers
}

fn create_modifier(only_run: &[Modifier], ignore_run: &[Modifier]) -> Result<CsvModifier> {
    let active_modifiers = determine_modifiers_to_run(only_run, ignore_run);
    let mut modifier = CsvModifier::new();

    // Note: CsvModifier::new() always includes the accessIdentifier validator.
    // This wrapper only toggles the additional column modifiers enabled by the CLI.
    if active_modifiers.is_empty() {
        return Ok(modifier);
    }

    if active_modifiers.contains(&Modifier::ParentId) {
        modifier = modifier.add_column_modifier("parent_id", ParentIdModifier);
    }

    if active_modifiers.contains(&Modifier::FileExtension) {
        modifier = modifier.add_column_modifier("file", FileExtensionModifier);
    }

    if active_modifiers.contains(&Modifier::FieldModel) {
        let field_model_modifier = FieldModelModifier::from_default_config()?;
        modifier = modifier.add_column_modifier("field_model", field_model_modifier);
    }

    Ok(modifier)
}

fn generate_output_filename(input: &str) -> String {
    let path = Path::new(input);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("csv");

    if let Some(parent) = path.parent() {
        parent
            .join(format!("{}-modified.{}", stem, extension))
            .to_string_lossy()
            .to_string()
    } else {
        format!("{}-modified.{}", stem, extension)
    }
}

fn generate_sheets_output_filename() -> String {
    "sheets-output-modified.csv".to_string()
}

fn generate_items_output_filename(processed_path: &str) -> String {
    let path = Path::new(processed_path);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("items");
    let file_name = format!("{}-items.csv", stem);

    if let Some(parent) = path.parent() {
        parent.join(file_name).to_string_lossy().to_string()
    } else {
        file_name
    }
}

pub fn determine_processed_output_path(
    input_path: &str,
    explicit_output: Option<&str>,
    output_dir: Option<&str>,
) -> Result<String> {
    if let Some(path) = explicit_output {
        return finalize_output_path(path, output_dir);
    }

    let default_path = generate_output_filename(input_path);
    if let Some(dir) = output_dir {
        if let Some(file_name) = Path::new(&default_path).file_name() {
            let file_name_owned = file_name.to_string_lossy().into_owned();
            return finalize_output_path(&file_name_owned, Some(dir));
        }
    }

    Ok(default_path)
}

pub fn determine_processed_output_path_for_sheets(
    explicit_output: Option<&str>,
    output_dir: Option<&str>,
) -> Result<String> {
    if let Some(path) = explicit_output {
        return finalize_output_path(path, output_dir);
    }

    let default = generate_sheets_output_filename();
    finalize_output_path(&default, output_dir)
}

pub fn determine_items_output_path(
    processed_output: &str,
    explicit_output: Option<&str>,
    output_dir: Option<&str>,
) -> Result<String> {
    if let Some(path) = explicit_output {
        return finalize_output_path(path, output_dir);
    }

    let default_path = generate_items_output_filename(processed_output);

    // Ensure the destination directory exists.
    if let Some(parent) = Path::new(&default_path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create output directory for items file: {}",
                    parent.display()
                )
            })?;
        }
    }

    Ok(default_path)
}

fn finalize_output_path(path: &str, output_dir: Option<&str>) -> Result<String> {
    let candidate = Path::new(path);

    if candidate.is_absolute()
        || candidate
            .parent()
            .map(|p| !p.as_os_str().is_empty())
            .unwrap_or(false)
    {
        if let Some(parent) = candidate.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create output directory: {}", parent.display())
                })?;
            }
        }

        return Ok(candidate.to_string_lossy().into_owned());
    }

    if let Some(dir) = output_dir {
        let dir_path = Path::new(dir);
        fs::create_dir_all(dir_path).with_context(|| {
            format!("Failed to create output directory: {}", dir_path.display())
        })?;
        return Ok(dir_path.join(candidate).to_string_lossy().to_string());
    }

    Ok(path.to_string())
}

pub fn generate_items_from_path(
    input_path: &str,
    output_path: &str,
    node: Option<&str>,
) -> Result<ItemGenerationStats> {
    ItemCsvGenerator::generate(input_path, output_path, node)
}

pub fn generate_items_from_url(
    url: &str,
    output_path: &str,
    node: Option<&str>,
) -> Result<ItemGenerationStats> {
    let csv_data = CsvModifier::fetch_google_sheets_csv(url)?;

    let mut temp_file = NamedTempFile::new()?;
    temp_file.write_all(csv_data.as_bytes())?;

    let temp_path = temp_file.path().to_path_buf();
    let path_str = temp_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Temporary file path contains invalid UTF-8"))?;

    ItemCsvGenerator::generate(path_str, output_path, node)
}

/// Library equivalent of the CLI `generate-items` subcommand.
///
/// - Provide exactly one of `input_path` or `url`.
/// - If `output_path` is `None`, it defaults to `items.csv`.
pub fn generate_items_from_source(
    input_path: Option<&str>,
    url: Option<&str>,
    output_path: Option<&str>,
    node: Option<&str>,
) -> Result<ItemGenerationStats> {
    let output_path = output_path.unwrap_or("items.csv");

    match (input_path, url) {
        (Some(path), None) => generate_items_from_path(path, output_path, node),
        (None, Some(link)) => generate_items_from_url(link, output_path, node),
        (Some(_), Some(_)) => anyhow::bail!("Specify either input_path or url, not both"),
        (None, None) => anyhow::bail!("No input provided. Provide input_path or url."),
    }
}

pub fn process_csv_and_maybe_generate_items(
    input_path: &str,
    explicit_output: Option<&str>,
    output_dir: Option<&str>,
    only_run: &[Modifier],
    ignore_run: &[Modifier],
    full: bool,
    items_output: Option<&str>,
    node: Option<&str>,
) -> Result<ProcessResult> {
    if !Path::new(input_path).exists() {
        anyhow::bail!("Input file does not exist: {}", input_path);
    }

    let processed_output_path = determine_processed_output_path(
        input_path,
        explicit_output,
        output_dir,
    )?;

    let modifier = create_modifier(only_run, ignore_run)?;
    let processing_stats = modifier.process_file(input_path, &processed_output_path)?;

    let (items_output_path, items_stats) = if full {
        let items_output_path = determine_items_output_path(
            &processed_output_path,
            items_output,
            output_dir,
        )?;
        let stats = generate_items_from_path(&processed_output_path, &items_output_path, node)?;
        (Some(items_output_path), Some(stats))
    } else {
        (None, None)
    };

    Ok(ProcessResult {
        processed_output_path,
        processing_stats,
        items_output_path,
        items_stats,
    })
}

pub fn process_google_sheets_and_maybe_generate_items(
    url: &str,
    explicit_output: Option<&str>,
    output_dir: Option<&str>,
    only_run: &[Modifier],
    ignore_run: &[Modifier],
    full: bool,
    items_output: Option<&str>,
    node: Option<&str>,
) -> Result<ProcessResult> {
    let processed_output_path = determine_processed_output_path_for_sheets(
        explicit_output,
        output_dir,
    )?;

    let modifier = create_modifier(only_run, ignore_run)?;
    let processing_stats = modifier.process_google_sheets(url, &processed_output_path)?;

    let (items_output_path, items_stats) = if full {
        let items_output_path = determine_items_output_path(
            &processed_output_path,
            items_output,
            output_dir,
        )?;
        let stats = generate_items_from_path(&processed_output_path, &items_output_path, node)?;
        (Some(items_output_path), Some(stats))
    } else {
        (None, None)
    };

    Ok(ProcessResult {
        processed_output_path,
        processing_stats,
        items_output_path,
        items_stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn processed_output_uses_output_dir_when_unspecified() -> Result<()> {
        let temp = tempdir()?;
        let output_dir = temp.path().join("outputs");
        let path = determine_processed_output_path(
            "/data/source.csv",
            None,
            Some(output_dir.to_str().unwrap()),
        )?;

        assert!(path.ends_with("source-modified.csv"));
        assert!(Path::new(&path).starts_with(&output_dir));
        assert!(output_dir.exists());
        Ok(())
    }

    #[test]
    fn finalize_output_respects_absolute_paths() -> Result<()> {
        let temp = tempdir()?;
        let absolute = temp.path().join("custom.csv");
        let resolved = finalize_output_path(absolute.to_str().unwrap(), Some("ignored"))?;
        assert_eq!(Path::new(&resolved), absolute);
        assert!(absolute.parent().unwrap().exists());
        Ok(())
    }

    #[test]
    fn items_output_defaults_to_processed_directory() -> Result<()> {
        let temp = tempdir()?;
        let processed = temp.path().join("processed.csv");
        fs::write(&processed, b"input")?;
        let items = determine_items_output_path(processed.to_str().unwrap(), None, None)?;
        assert!(items.ends_with("processed-items.csv"));
        assert!(Path::new(&items).parent().unwrap().exists());
        Ok(())
    }
}

