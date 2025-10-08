use anyhow::Result;
use clap::Parser;
use env_logger::Env;
use organise::{
    Cli, Commands, CsvModifier, FileExtensionModifier, ItemCsvGenerator, ItemGenerationStats,
    Modifier, ParentIdModifier, ProcessingStats,
};
use std::collections::HashSet;
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
                let output_path = cli
                    .output
                    .clone()
                    .unwrap_or_else(|| generate_output_filename(input_path));
                process_file(
                    input_path,
                    &output_path,
                    &cli.only_run,
                    &cli.ignore_run,
                    cli.stats,
                )?;

                if cli.full {
                    run_full_pipeline(&output_path, cli.node.as_deref())?;
                }
            }
            (None, Some(url)) => {
                let output_path = cli
                    .output
                    .clone()
                    .unwrap_or_else(generate_sheets_output_filename);
                process_sheets(url, &output_path, &cli.only_run, &cli.ignore_run, cli.stats)?;

                if cli.full {
                    run_full_pipeline(&output_path, cli.node.as_deref())?;
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
    let all_modifiers = vec![Modifier::ParentId, Modifier::FileExtension];

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

fn create_modifier(only_run: &[Modifier], ignore_run: &[Modifier]) -> CsvModifier {
    let active_modifiers = determine_modifiers_to_run(only_run, ignore_run);
    let mut modifier = CsvModifier::new();

    if active_modifiers.is_empty() {
        println!("WARNING: No modifiers will be applied - file will be copied without changes");
        return modifier;
    }

    if active_modifiers.contains(&Modifier::ParentId) {
        println!("Applying parent_id modifier");
        modifier = modifier.add_column_modifier("parent_id", ParentIdModifier);
    }

    if active_modifiers.contains(&Modifier::FileExtension) {
        println!("Applying file_extension modifier");
        modifier = modifier.add_column_modifier("file", FileExtensionModifier);
    }

    // Show which modifiers were ignored/excluded
    let all_modifiers = vec![Modifier::ParentId, Modifier::FileExtension];
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

    modifier
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

    let modifier = create_modifier(only_run, ignore_run);
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

    let modifier = create_modifier(only_run, ignore_run);
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

fn run_full_pipeline(processed_path: &str, node: Option<&str>) -> Result<()> {
    let items_output = generate_items_output_filename(processed_path);
    generate_items_from_path(processed_path, &items_output, node)
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
