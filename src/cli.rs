use clap::{Parser, Subcommand, ValueEnum};

/// The pre-2026 command line, kept as `organise legacy ...` for old scripts.
#[derive(Parser)]
#[command(author, version, long_about = None)]
#[command(name = "organise legacy")]
#[command(about = "Legacy CSV cleaner (pre-2026 flow). New ingests: run `organise` with no subcommand.")]
#[command(subcommand_negates_reqs = true)]
pub struct LegacyCli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Path to input CSV file
    #[arg(
        value_name = "INPUT",
        conflicts_with = "url",
        required_unless_present_any = ["url"]
    )]
    pub input: Option<String>,

    /// Worksheet to read from an Excel or ODS input (defaults to the first sheet)
    #[arg(long, value_name = "NAME")]
    pub sheet: Option<String>,

    /// Google Sheets URL (edit URL will be converted to CSV export URL)
    #[arg(long, value_name = "URL", conflicts_with = "input")]
    pub url: Option<String>,

    /// Path to output CSV file (defaults vary based on input type)
    #[arg(short, long)]
    pub output: Option<String>,

    /// Directory to write processed and generated output files
    #[arg(long, value_name = "DIR")]
    pub output_dir: Option<String>,

    /// File name or path for the generated items CSV when using --full
    #[arg(long, value_name = "FILE", requires = "full")]
    pub items_output: Option<String>,

    /// Only run the specified modifiers (overrides default behavior)
    #[arg(long, value_enum)]
    pub only_run: Vec<Modifier>,

    /// Ignore the specified modifiers (applied after default modifiers)
    #[arg(long, value_enum)]
    pub ignore_run: Vec<Modifier>,

    /// Full URL for the language code → taxonomy term JSON (`field_code` → `tid`). Overrides `ISLANDORA_LANGUAGE_URL` and the default base + `/lang-code`.
    #[arg(long, value_name = "URL")]
    pub language_url: Option<String>,

    /// Staged access directory. When set, every `file` path is checked against
    /// disk, rewritten to where the file actually is, and missing files reported.
    #[arg(long, value_name = "DIR")]
    pub access_dir: Option<String>,

    /// Show detailed processing statistics
    #[arg(long)]
    pub stats: bool,

    /// Run both processing and item generation in a single pass
    #[arg(long)]
    pub full: bool,

    /// Node identifier to use when running --full
    #[arg(short = 'n', long = "node", value_name = "NODE", requires = "full")]
    pub node: Option<String>,
}

#[derive(Clone, Debug, ValueEnum, PartialEq, Eq, Hash)]
pub enum Modifier {
    /// Extract parent ID from accessIdentifier column
    ParentId,
    /// Create file paths with parent directory and extensions
    FileExtension,
    /// Populate field_model based on file extension mappings
    FieldModel,
    /// Map field_language column codes to taxonomy term IDs via JSON export
    Language,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Generate items.csv from a modified CSV file
    GenerateItems {
        /// Path to input CSV file (typically the modified file)
        #[arg(
            value_name = "INPUT",
            conflicts_with = "url",
            required_unless_present = "url"
        )]
        input: Option<String>,

        /// Worksheet to read from an Excel or ODS input (defaults to the first sheet)
        #[arg(long, value_name = "NAME")]
        sheet: Option<String>,

        /// Google Sheets URL to source the data from
        #[arg(
            long,
            value_name = "URL",
            conflicts_with = "input",
            required_unless_present = "input"
        )]
        url: Option<String>,

        /// Path to output items.csv file (defaults to 'items.csv')
        #[arg(short, long)]
        output: Option<String>,

        /// Node identifier to populate the field_member_of column
        #[arg(short = 'n', long = "node", value_name = "NODE")]
        node: Option<String>,
    },
}

/// `organise`: spreadsheet in, checked Workbench bundle out.
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

    /// No interactive screen: print the report and write the bundle if it's clean
    #[arg(long)]
    pub plain: bool,

    /// After writing the bundle, run it through Workbench (check, then ingest)
    #[arg(long)]
    pub ingest: bool,

    /// Confirm a site marked `confirm = true` (prod) without the typed prompt
    #[arg(long)]
    pub yes: bool,

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
    /// The pre-2026 cleaner: `organise legacy --help`
    #[command(disable_help_flag = true)]
    Legacy {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}
