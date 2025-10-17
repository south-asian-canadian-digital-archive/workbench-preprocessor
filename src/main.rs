use anyhow::{Context, Result};
use clap::Parser;
use env_logger::Env;
use organise::{
    Cli, Commands, CsvModifier, FieldModelModifier, FileExtensionModifier, ItemCsvGenerator,
    ItemGenerationStats, Modifier, ParentIdModifier, ProcessingStats,
};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;

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

fn main() -> Result<()> {
    init_logging();
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::GenerateItems {
            input,
            url,
            output,
            node,
        }) => {
            let output_path = output.unwrap_or_else(|| "items.csv".to_string());
            generate_items(
                input.as_deref(),
                url.as_deref(),
                &output_path,
                node.as_deref(),
            )?;
        }
        None => match (cli.input.as_deref(), cli.url.as_deref()) {
            (Some(input_path), None) => {
                let processed_output = determine_processed_output_path(
                    input_path,
                    cli.output.as_deref(),
                    cli.output_dir.as_deref(),
                )?;

                process_file(
                    input_path,
                    &processed_output,
                    &cli.only_run,
                    &cli.ignore_run,
                    cli.stats,
                )?;

                if cli.full {
                    let items_output = determine_items_output_path(
                        &processed_output,
                        cli.items_output.as_deref(),
                        cli.output_dir.as_deref(),
                    )?;
                    run_full_pipeline(&processed_output, Some(&items_output), cli.node.as_deref())?;
                }
            }
            (None, Some(url)) => {
                let processed_output = determine_processed_output_path_for_sheets(
                    cli.output.as_deref(),
                    cli.output_dir.as_deref(),
                )?;

                process_sheets(
                    url,
                    &processed_output,
                    &cli.only_run,
                    &cli.ignore_run,
                    cli.stats,
                )?;

                if cli.full {
                    let items_output = determine_items_output_path(
                        &processed_output,
                        cli.items_output.as_deref(),
                        cli.output_dir.as_deref(),
                    )?;
                    run_full_pipeline(&processed_output, Some(&items_output), cli.node.as_deref())?;
                }
            }
            (Some(_), Some(_)) => {
                anyhow::bail!("Specify either a file path or --url, not both");
            }
            (None, None) => {
                anyhow::bail!(
                    "No input provided. Pass a file path or use --url with a Google Sheets link"
                );
            }
        },
    }

    Ok(())
}

fn init_logging() {
    let env = Env::default().filter_or("RUST_LOG", "warn");
    let _ = env_logger::Builder::from_env(env)
        .format_timestamp_secs()
        .format_target(false)
        .try_init();
}
fn determine_modifiers_to_run(only_run: &[Modifier], ignore_run: &[Modifier]) -> HashSet<Modifier> {
    let all_modifiers = [Modifier::ParentId, Modifier::FileExtension, Modifier::FieldModel];

    let mut active_modifiers: HashSet<Modifier> = if only_run.is_empty() {
        // Default behavior: run all modifiers
        all_modifiers.into_iter().collect()
    } else {
        // Only run specified modifiers
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

    if active_modifiers.is_empty() {
        println!("WARNING: No modifiers will be applied - file will be copied without changes");
        return Ok(modifier);
    }

    if active_modifiers.contains(&Modifier::ParentId) {
        println!("Applying parent_id modifier");
        modifier = modifier.add_column_modifier("parent_id", ParentIdModifier);
    }

    if active_modifiers.contains(&Modifier::FileExtension) {
        println!("Applying file_extension modifier");
        modifier = modifier.add_column_modifier("file", FileExtensionModifier);
    }

    if active_modifiers.contains(&Modifier::FieldModel) {
        println!("Applying field_model modifier");
        let field_model_modifier = FieldModelModifier::from_default_config()?;
        modifier = modifier.add_column_modifier("field_model", field_model_modifier);
    }

    // Show which modifiers were ignored/excluded
    let all_modifiers = [Modifier::ParentId, Modifier::FileExtension, Modifier::FieldModel];
    let excluded_modifiers: Vec<&Modifier> = all_modifiers
        .iter()
        .filter(|m| !active_modifiers.contains(m))
        .collect();

    if !excluded_modifiers.is_empty() {
        let excluded_names: Vec<String> = excluded_modifiers
            .iter()
            .map(|m| format!("{:?}", m).to_lowercase().replace("_", "-"))
            .collect();
        println!("Skipping modifiers: {}", excluded_names.join(", "));
    }

    Ok(modifier)
}

fn process_file(
    input: &str,
    output: &str,
    only_run: &[Modifier],
    ignore_run: &[Modifier],
    show_stats: bool,
) -> Result<()> {
    // Validate input file exists
    if !Path::new(input).exists() {
        anyhow::bail!("Input file does not exist: {}", input);
    }

    println!("Processing file: {}", input);

    let modifier = create_modifier(only_run, ignore_run)?;
    let stats = modifier.process_file(input, output)?;

    println!("Processing complete!");
    println!("Processed {} rows", stats.total_rows);
    println!("Modified {} cells", stats.cells_modified);

    if stats.validation_failures > 0 {
        println!("WARNING: {} validation failures", stats.validation_failures);
    }

    println!("Output written to: {}", output);

    if show_stats {
        print_detailed_stats(&stats);
    }

    Ok(())
}

fn process_sheets(
    url: &str,
    output: &str,
    only_run: &[Modifier],
    ignore_run: &[Modifier],
    show_stats: bool,
) -> Result<()> {
    println!("Processing Google Sheets URL: {}", url);

    // Show the converted CSV URL for transparency
    let csv_url = CsvModifier::google_sheets_to_csv_url(url)?;
    println!("CSV export URL: {}", csv_url);

    let modifier = create_modifier(only_run, ignore_run)?;
    let stats = modifier.process_google_sheets(url, output)?;

    println!("Processing complete!");
    println!("Processed {} rows", stats.total_rows);
    println!("Modified {} cells", stats.cells_modified);

    if stats.validation_failures > 0 {
        println!("WARNING: {} validation failures", stats.validation_failures);
    }

    println!("Output written to: {}", output);

    if show_stats {
        print_detailed_stats(&stats);
    }

    Ok(())
}

fn generate_items(
    input: Option<&str>,
    url: Option<&str>,
    output: &str,
    node: Option<&str>,
) -> Result<()> {
    match (input, url) {
        (Some(path), None) => generate_items_from_path(path, output, node),
        (None, Some(link)) => generate_items_from_url(link, output, node),
        (Some(_), Some(_)) => {
            anyhow::bail!("Specify either a file path or --url for generate-items, not both")
        }
        (None, None) => {
            anyhow::bail!("No input provided for generate-items. Pass a file path or use --url.")
        }
    }
}

fn generate_items_from_path(input: &str, output: &str, node: Option<&str>) -> Result<()> {
    if !Path::new(input).exists() {
        anyhow::bail!("Input file does not exist: {}", input);
    }

    println!("Generating items.csv from: {}", input);

    let stats = ItemCsvGenerator::generate(input, output, node)?;
    print_item_generation_summary(&stats, output);

    Ok(())
}

fn generate_items_from_url(url: &str, output: &str, node: Option<&str>) -> Result<()> {
    println!("Generating items.csv from Google Sheets URL: {}", url);

    let csv_data = CsvModifier::fetch_google_sheets_csv(url)?;
    let mut temp_file = NamedTempFile::new()?;
    temp_file.write_all(csv_data.as_bytes())?;

    let temp_path = temp_file.path().to_path_buf();
    let path_str = temp_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Temporary file path contains invalid UTF-8"))?;

    let stats = ItemCsvGenerator::generate(path_str, output, node)?;
    print_item_generation_summary(&stats, output);

    Ok(())
}

fn run_full_pipeline(
    processed_path: &str,
    items_output: Option<&str>,
    node: Option<&str>,
) -> Result<()> {
    let items_output_path = if let Some(path) = items_output {
        path.to_string()
    } else {
        generate_items_output_filename(processed_path)
    };

    generate_items_from_path(processed_path, &items_output_path, node)
}

fn print_item_generation_summary(stats: &ItemGenerationStats, output: &str) {
    println!("\u{2713} Items file generated successfully!");
    println!("  - Unique parent IDs: {}", stats.unique_parents);
    println!("  - Total items processed: {}", stats.total_items);
    println!("  - Output written to: {}", output);

    if stats.skipped_rows > 0 {
        println!(
            "  \u{26a0} Skipped {} rows with empty parent_id",
            stats.skipped_rows
        );
    }
}

fn print_detailed_stats(stats: &ProcessingStats) {
    println!("\nDetailed Statistics:");
    println!("- Total rows processed: {}", stats.total_rows);
    println!("- Rows skipped: {}", stats.skipped_rows);
    println!("- Cells modified: {}", stats.cells_modified);
    println!("- Validation failures: {}", stats.validation_failures);
    println!("- Columns processed: {}", stats.columns_processed.len());

    if !stats.columns_processed.is_empty() {
        println!(
            "  Columns: {}",
            stats
                .columns_processed
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

fn determine_processed_output_path(
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

fn determine_processed_output_path_for_sheets(
    explicit_output: Option<&str>,
    output_dir: Option<&str>,
) -> Result<String> {
    if let Some(path) = explicit_output {
        return finalize_output_path(path, output_dir);
    }

    let default = generate_sheets_output_filename();
    finalize_output_path(&default, output_dir)
}

fn determine_items_output_path(
    processed_output: &str,
    explicit_output: Option<&str>,
    output_dir: Option<&str>,
) -> Result<String> {
    if let Some(path) = explicit_output {
        return finalize_output_path(path, output_dir);
    }

    let default_path = generate_items_output_filename(processed_output);

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

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
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
