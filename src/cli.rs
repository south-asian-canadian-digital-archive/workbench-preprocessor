use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "organise", version)]
#[command(about = "Check a SACDA spreadsheet against the site and build its Workbench ingest bundle")]
#[command(after_help = "With no arguments in a terminal, organise opens its interactive screen.\n\
Examples:\n  organise                       pick a sheet interactively\n  organise sheet.xlsx            check and review it (dev by default)\n  organise sheet.xlsx --site test\n  organise doctor                is this machine ready?")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Spreadsheet (CSV, TSV, Excel, ODS) or a Google Sheets link
    #[arg(value_name = "SHEET")]
    pub input: Option<String>,

    /// Site to check against and ingest into (dev, test, prod)
    #[arg(long, short)]
    pub site: Option<String>,

    /// Worksheet inside an Excel/ODS file (default: the first)
    #[arg(long, value_name = "NAME")]
    pub sheet: Option<String>,

    /// Folder of staged access files (default: `access` next to the sheet)
    #[arg(long, value_name = "DIR")]
    pub access_dir: Option<String>,

    /// Where to write the bundle (default: ingest-<collection>-<site> next to the sheet)
    #[arg(long, short, value_name = "DIR")]
    pub out: Option<String>,

    /// Title for a collection that's in neither the sheet nor the site
    #[arg(long, value_name = "TITLE")]
    pub collection_title: Option<String>,

    /// Apply suggested fixes too (misspelt columns/terms), not just the safe ones
    #[arg(long)]
    pub accept_suggestions: bool,

    /// Ingest rows without their files and files without rows anyway (default: leave them out and list them in
    /// skipped-rows.csv / unmatched-files.csv in the bundle)
    #[arg(long)]
    pub keep_unpaired: bool,

    /// Items already on the site: overwrite these of their fields from the sheet, by column (comma-separated,
    /// or `all`); the items are otherwise left alone
    #[arg(long, value_name = "COLUMNS")]
    pub update: Option<String>,

    /// Items already on the site: replace their media with the file in the files folder
    #[arg(long)]
    pub replace_media: bool,

    /// Items already on the site with no media: upload their file from the files folder
    #[arg(long)]
    pub add_missing_media: bool,

    /// No interactive screen: print the report and write the bundle if it's clean
    #[arg(long)]
    pub plain: bool,

    /// After writing the bundle, run it through Workbench (check, then ingest)
    #[arg(long)]
    pub ingest: bool,

    /// Confirm a site marked `confirm = true` (prod) without the typed prompt
    #[arg(long, global = true)]
    pub yes: bool,

    /// Ignore the sheet's file column: every access file is <identifier>.<ext>
    #[arg(long)]
    pub files_by_identifier: bool,

    /// Only part of the sheet, picked like PDF pages by sheet row: 12 | 12,15 | -40 | 100- | 12-40, or by identifier
    /// (2024_19_1545 = it and everything under it). The rows of their collections/series always come along.
    #[arg(long, value_name = "SPEC")]
    pub only: Option<String>,

    /// Skip all site lookups (quick structural check only; never writes a bundle)
    #[arg(long)]
    pub offline: bool,

    /// Path to the g/ folder, if organise can't find it
    #[arg(long, value_name = "DIR", global = true)]
    pub g: Option<String>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Check this machine and every site's reachability, JSON:API and login
    Doctor {
        #[arg(long, short)]
        site: Option<String>,
    },
    /// Run an existing bundle through Workbench (all --check runs first)
    Ingest {
        #[arg(value_name = "BUNDLE_DIR")]
        bundle: String,
        /// Only run Workbench's checks
        #[arg(long)]
        check_only: bool,
    },
}
