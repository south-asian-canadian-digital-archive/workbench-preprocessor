use anyhow::Result;
use clap::Parser;
use env_logger::Env;
use organise::{
    Cli, Commands, CsvModifier, ItemGenerationStats, ProcessingStats,
    generate_items_from_source,
    process_csv_and_maybe_generate_items,
    process_google_sheets_and_maybe_generate_items,
};

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
            let stats = generate_items_from_source(
                input.as_deref(),
                url.as_deref(),
                output.as_deref(),
                node.as_deref(),
            )?;
            print_item_generation_summary(&stats, output.as_deref().unwrap_or("items.csv"));
        }
        None => match (cli.input.as_deref(), cli.url.as_deref()) {
            (Some(input_path), None) => {
                println!("Processing file: {}", input_path);
                let res = process_csv_and_maybe_generate_items(
                    input_path,
                    cli.output.as_deref(),
                    cli.output_dir.as_deref(),
                    &cli.only_run,
                    &cli.ignore_run,
                    cli.language_url.as_deref(),
                    cli.full,
                    cli.items_output.as_deref(),
                    cli.node.as_deref(),
                )?;
                print_processing_summary(&res.processing_stats, &res.processed_output_path, cli.stats);

                if let (Some(items_stats), Some(items_path)) = (res.items_stats.as_ref(), res.items_output_path.as_ref()) {
                    print_item_generation_summary(items_stats, items_path);
                }
            }
            (None, Some(url)) => {
                println!("Processing Google Sheets URL: {}", url);
                let csv_url = CsvModifier::google_sheets_to_csv_url(url)?;
                println!("CSV export URL: {}", csv_url);

                let res = process_google_sheets_and_maybe_generate_items(
                    url,
                    cli.output.as_deref(),
                    cli.output_dir.as_deref(),
                    &cli.only_run,
                    &cli.ignore_run,
                    cli.language_url.as_deref(),
                    cli.full,
                    cli.items_output.as_deref(),
                    cli.node.as_deref(),
                )?;
                print_processing_summary(&res.processing_stats, &res.processed_output_path, cli.stats);

                if let (Some(items_stats), Some(items_path)) = (res.items_stats.as_ref(), res.items_output_path.as_ref()) {
                    print_item_generation_summary(items_stats, items_path);
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

fn print_processing_summary(stats: &ProcessingStats, output: &str, show_stats: bool) {
    println!("Processing complete!");
    println!("Processed {} rows", stats.total_rows);
    println!("Modified {} cells", stats.cells_modified);

    if stats.validation_failures > 0 {
        println!("WARNING: {} validation failures", stats.validation_failures);
    }

    println!("Output written to: {}", output);

    if show_stats {
        print_detailed_stats(stats);
    }
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
