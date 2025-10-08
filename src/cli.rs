use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(name = "organise")]
#[command(about = "A tool for processing and organizing CSV files")]
#[command(subcommand_negates_reqs = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Path to input CSV file
    #[arg(
        value_name = "INPUT",
        conflicts_with = "url",
        required_unless_present_any = ["url", "command"]
    )]
    pub input: Option<String>,

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
    #[arg(
        long,
        value_name = "FILE",
        requires = "full",
        conflicts_with = "command"
    )]
    pub items_output: Option<String>,

    /// Only run the specified modifiers (overrides default behavior)
    #[arg(long, value_enum)]
    pub only_run: Vec<Modifier>,

    /// Ignore the specified modifiers (applied after default modifiers)
    #[arg(long, value_enum)]
    pub ignore_run: Vec<Modifier>,

    /// Show detailed processing statistics
    #[arg(long)]
    pub stats: bool,

    /// Run both processing and item generation in a single pass
    #[arg(long, conflicts_with = "command")]
    pub full: bool,

    /// Node identifier to use when running --full
    #[arg(
        short = 'n',
        long = "node",
        value_name = "NODE",
        requires = "full",
        conflicts_with = "command"
    )]
    pub node: Option<String>,
}

#[derive(Clone, Debug, ValueEnum, PartialEq, Eq, Hash)]
pub enum Modifier {
    /// Extract parent ID from accessIdentifier column
    ParentId,
    /// Create file paths with parent directory and extensions
    FileExtension,
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
