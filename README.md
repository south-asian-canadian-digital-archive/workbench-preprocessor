# CSV Organiser

A high-performance, trait-based CSV processing library and CLI tool written in Rust that supports streaming operations for large datasets and can process data from both local files and Google Sheets.

## Tools Included

This project ships a single CLI binary, **`organise`**, which can:

- Process local CSV files with configurable column modifiers
- Stream Google Sheets data directly into the modifier pipeline
- Generate collection-level `items.csv` summaries (either from an existing CSV or directly from a Google Sheet)
- Run the full pipeline end-to-end with a single flag

## Features

- **Streaming Processing**: Efficiently handles large CSV files (400k+ rows) with minimal memory usage
- **Trait-Based Architecture**: Extensible design using the `ColumnModifier` trait
- **Cross-Column Access**: Modifiers can access values from other columns in the same row
- **Built-in Modifiers**: Ready-to-use modifiers for common tasks
- **Google Sheets Integration**: Process CSV data directly from Google Sheets URLs
- **Comprehensive Statistics**: Detailed processing statistics and validation reporting
- **Command Line Interface**: Easy-to-use CLI for batch processing
- **Error Handling**: Robust error handling with context information

## Installation

Build the project:

```bash
cargo build --release
```

This creates one binary in `target/release/`:
- **`organise`** – combined processor and items generator

The binary will be available at `target/release/organise`.

## Quick Start

```bash
# Process a local CSV (auto-writes input-modified.csv)
./target/release/organise input.csv

# Process a Google Sheet (auto-writes sheets-output-modified.csv)
./target/release/organise --url 'https://docs.google.com/spreadsheets/d/...'

# Generate items from an existing CSV
./target/release/organise generate-items output.csv

# Generate items and set a node ID for field_member_of
./target/release/organise generate-items output.csv --node 19

# End-to-end: process input.csv then build items CSV alongside it
./target/release/organise --full input.csv

# End-to-end with node reference prefilled
./target/release/organise --full input.csv --node 19
```

## Command Line Usage

### Process Local CSV Files

```bash
# Basic file processing (applies all modifiers, writes input-modified.csv)
./target/release/organise input.csv

# Custom output filename
./target/release/organise input.csv --output custom-output.csv

# Only run specific modifiers
./target/release/organise input.csv --only-run parent-id

# Skip specific modifiers
./target/release/organise input.csv --ignore-run file-extension

# Show detailed stats after processing
./target/release/organise input.csv --stats

# Process and immediately build items CSV alongside the result
./target/release/organise --full input.csv
```

### Process Google Sheets

```bash
# Download + process Google Sheet (writes sheets-output-modified.csv)
./target/release/organise --url 'https://docs.google.com/spreadsheets/d/SHEET_ID/edit#gid=0'

# Custom output filename
./target/release/organise --url 'https://docs.google.com/spreadsheets/d/SHEET_ID/edit#gid=0' \
    --output custom-output.csv

# Apply only certain modifiers and show stats
./target/release/organise --url 'https://docs.google.com/spreadsheets/d/SHEET_ID/edit' \
    --only-run parent-id --stats

# Full pipeline: process Sheets data then generate items summary
./target/release/organise --url 'https://docs.google.com/spreadsheets/d/SHEET_ID/edit' --full
```

### Generate Items CSV

Generate a summary `items.csv` file containing unique parent IDs, their titles, item counts, and a `field_member_of` column that can optionally be pre-populated with a node identifier.

```bash
# From an existing CSV on disk (default output: items.csv)
./target/release/organise generate-items input-modified.csv

# Specify custom output
./target/release/organise generate-items input-modified.csv --output custom-items.csv

# Generate directly from a Google Sheet
./target/release/organise generate-items --url 'https://docs.google.com/spreadsheets/d/SHEET_ID/edit#gid=0'

# Generate items and set the node reference in field_member_of
./target/release/organise generate-items input-modified.csv --node 19
```

**Input Requirements**: The input CSV must contain:
- `parent_id`: Column with parent identifiers
- `fileTitle`: Column with titles for each item

**Output Format**: The generated `items.csv` will have the following columns:
- `file_identifier`: Unique parent IDs from the `parent_id` column
- `title`: Title associated with each parent ID (from `fileTitle`)
- `# of items`: Count of how many times each parent ID appears
- `field_member_of`: Empty by default, or populated with the value passed via `--node`

**Example**:
```bash
# First, process your CSV to add parent_id
./target/release/organise file -i data.csv -o data-modified.csv

# Then generate the items file
./target/release/organise generate-items -i data-modified.csv -o items.csv

# Include a node ID in field_member_of
./target/release/organise generate-items -i data-modified.csv --node 19
```

This will create an `items.csv` like:
```csv
file_identifier,title,# of items,field_member_of
2024_19_01,Annual Report 2024,3,
2024_19_02,Photo Gallery Spring,2,
2024_20_01,Monthly Financial Report,1,
```

### Command Line Options

#### Command Structure

- **Default mode**: Provide a positional input path or `--url` (mutually exclusive). Optional flags configure modifiers, output, statistics, `--full`, and `--node` (when paired with `--full`).
- **`generate-items` subcommand**: Provide either a positional input path or `--url` (mutually exclusive). Optional `--output` overrides the destination, and `--node` populates every `field_member_of` value.

#### Common Flags

- `--url <URL>`: Treat the input as a Google Sheet (auto-converted to CSV).
- `--output <FILE>`: Override the generated output filename.
- `--only-run <MODIFIER>`: Only apply the specified modifier(s); repeatable.
- `--ignore-run <MODIFIER>`: Skip the specified modifier(s); repeatable.
- `--stats`: Print detailed processing statistics after the run.
- `--full`: After processing, immediately generate the items summary next to the output CSV.
- `-n, --node <NODE>` (_generate-items_ or `--full`): Populate the `field_member_of` column with the provided value.

#### Output Filename Generation

- **For local files**: If no output is specified, the output filename is generated by appending `-modified` to the input filename before the extension
  - `data.csv` → `data-modified.csv`
  - `path/to/file.xlsx` → `path/to/file-modified.xlsx`
  - `document` → `document-modified.csv`

- **For Google Sheets**: If no output is specified, defaults to `sheets-output-modified.csv`

- `parent-id`: Extract parent ID from accessIdentifier column
- `file-extension`: Create file paths with parent directory and extensions

#### Modifier Behavior

#### Available Modifiers

- **Default**: All available modifiers are applied
- **--only-run**: Only the specified modifiers are applied (overrides default)
- **--ignore-run**: The specified modifiers are excluded from the active set
- **Precedence**: `--ignore-run` takes precedence over `--only-run` (if both specify the same modifier)

#### Example Output

```bash
$ ./target/release/organise file -i data.csv --stats

Processing file: data.csv
Applying parent_id modifier
Applying file_extension modifier
Processing complete!
Processed 1000 rows
Modified 2000 cells
Output written to: data-modified.csv

Detailed Statistics:
- Total rows processed: 1000
- Cells modified: 2000
- Validation failures: 0
- Columns processed: 2
  Columns: parent_id, file

$ ./target/release/organise file -i data.csv --only-run parent-id

Processing file: data.csv
Applying parent_id modifier
Skipping modifiers: file-extension
Processing complete!
Processed 1000 rows
Modified 1000 cells
Output written to: data-modified.csv
```

## Built-in Modifiers

### ParentIdModifier
Extracts parent IDs from `accessIdentifier` columns by removing the last underscore segment.

**Example**: `2024_19_01_001` → `2024_19_01`

**Requirements**:
- Target column: `parent_id`
- Source column: `accessIdentifier` (must not be empty)

**Placeholder Handling**: Values like `#VALUE!` (commonly produced by Google Sheets errors) are treated as empty and cleared during processing.

### FileExtensionModifier  
Creates file paths using parent ID directories and file extensions from other columns.

**Example**: With `accessIdentifier="2024_19_01_001"`, `file="document"`, `file_extension="pdf"`
**Result**: `2024_19_01/document.pdf`

**Requirements**:
- Target column: `file`
- Source columns: `accessIdentifier`, `file_extension` (or legacy `file_extention`), all must not be empty

**Placeholder Handling**: If `file`, `file_extension`, or `accessIdentifier` contain `#VALUE!`, the row is left unchanged for that modifier, the offending cell is cleared, and a warning is emitted.
- Handles existing file extensions by replacing them

## Library Usage

### Basic Usage

```rust
use organise::{CsvModifier, ParentIdModifier, FileExtensionModifier};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create modifier with built-in modifiers
    let modifier = CsvModifier::new()
        .add_column_modifier("parent_id", ParentIdModifier)
        .add_column_modifier("file", FileExtensionModifier);
    
    // Process local CSV file
    let stats = modifier.process_file("input.csv", "output.csv")?;
    
    println!("Processed {} rows successfully!", stats.total_rows);
    println!("Modified {} cells", stats.cells_modified);
    
    Ok(())
}
```

### Custom Modifiers

```rust
use organise::{CsvModifier, ColumnModifier, RowContext};

struct PrefixModifier {
    prefix: String,
}

impl ColumnModifier for PrefixModifier {
    fn modify(&self, value: &str, _context: &RowContext) -> String {
        format!("{}{}", self.prefix, value)
    }

    fn description(&self) -> &str {
        "Adds a prefix to the column value"
    }
    
    // Optional: add validation
    fn validate(&self, value: &str, _context: &RowContext) -> bool {
        !value.is_empty()
    }
}

// Usage
let modifier = CsvModifier::new()
    .add_column_modifier("title", PrefixModifier { 
        prefix: "DOC_".to_string() 
    });
```

### Cross-Column Access

```rust
use organise::{ColumnModifier, RowContext};

struct CrossColumnModifier;

impl ColumnModifier for CrossColumnModifier {
    fn modify(&self, value: &str, context: &RowContext) -> String {
        // Access other columns in the same row
        let other_value = context.get_or_empty("other_column");
        let another_value = context.get("another_column").unwrap_or("default");
        
        format!("{}-{}-{}", value, other_value, another_value)
    }
    
    fn description(&self) -> &str {
        "Combines values from multiple columns"
    }
}
```

### Google Sheets Integration

```rust
use organise::CsvModifier;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let modifier = CsvModifier::new()
        .add_column_modifier("parent_id", ParentIdModifier);
    
    // Process from Google Sheets URL
    let sheets_url = "https://docs.google.com/spreadsheets/d/YOUR_SHEET_ID/edit#gid=0";
    let stats = modifier.process_google_sheets(sheets_url, "output.csv")?;
    
    println!("Processed {} rows from Google Sheets!", stats.total_rows);
    
    // Or just convert the URL
    let csv_url = CsvModifier::google_sheets_to_csv_url(sheets_url)?;
    println!("CSV export URL: {}", csv_url);
    
    Ok(())
}
```

### Generating Items Summary

```rust
use organise::{CsvModifier, ItemCsvGenerator};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // First, process the CSV to add parent_id and other modifications
    let modifier = CsvModifier::new()
        .add_column_modifier("parent_id", ParentIdModifier)
        .add_column_modifier("file", FileExtensionModifier);
    
    modifier.process_file("input.csv", "modified.csv")?;
    
    // Then generate the items.csv summary
    ItemCsvGenerator::generate("modified.csv", "items.csv", None)?;
    
    println!("Items summary generated!");
    
    Ok(())
}
```

`ItemCsvGenerator::generate`:
- Reads a CSV file with `parent_id` and `fileTitle` columns
- Groups rows by unique `parent_id` values
- Counts occurrences of each parent ID
- Associates each parent ID with its corresponding title
- Outputs a summary CSV with columns: `file_identifier`, `title`, `# of items`, `field_member_of`
- Ignores rows where `parent_id` is empty or resolves to `#VALUE!`
- Optionally populates `field_member_of` with a provided node identifier

## Processing Statistics

The `ProcessingStats` struct provides detailed information about the processing operation:

```rust
pub struct ProcessingStats {
    pub total_rows: usize,           // Total number of data rows processed
    pub cells_modified: usize,       // Number of cells that were modified
    pub validation_failures: usize, // Number of validation failures
    pub columns_processed: HashSet<String>, // Set of columns that were processed
}
```

### Logging Validation Failures

Validation failures now emit structured warnings through the standard Rust logging system. By default, the first 25 failures are printed; additional failures are summarized to avoid flooding the console.

To surface the warnings, run the CLI with the desired log level (they use the `warn` level by default):

```bash
RUST_LOG=warn ./target/release/organise --url 'https://docs.google.com/spreadsheets/d/SHEET_ID/edit#gid=0'
```

## Error Handling

The library uses the `anyhow` crate for comprehensive error handling:

- **File I/O errors**: Missing files, permission issues
- **CSV parsing errors**: Malformed CSV data, encoding issues
- **Network errors**: Failed HTTP requests for Google Sheets
- **URL parsing errors**: Invalid Google Sheets URLs
- **Validation errors**: Data that fails modifier validation

## Performance

The library is designed for high performance:

- **Streaming Processing**: Processes CSV files row-by-row without loading entire file into memory
- **Minimal Allocations**: Reuses string buffers and minimizes heap allocations
- **Efficient I/O**: Uses buffered readers and writers for optimal disk performance
- **Zero-Copy Operations**: Where possible, avoids unnecessary string copying

**Benchmarks**: Successfully processes 400,000+ row CSV files with minimal memory usage.

## Dependencies

- `csv`: CSV reading and writing
- `serde`: Serialization framework  
- `anyhow`: Error handling
- `reqwest`: HTTP client for Google Sheets integration
- `url`: URL parsing and manipulation

## Examples

Run the included examples:

```bash
# Basic functionality test
cargo run --example test_basic

# Comprehensive test with built-in modifiers
cargo run --example test_full
```

## Google Sheets Setup

To use Google Sheets integration:

1. Make your Google Sheet public (viewable by anyone with the link)
2. Copy the share URL
3. The library automatically converts it to the CSV export format

**Supported URL formats**:
- `https://docs.google.com/spreadsheets/d/SHEET_ID/edit#gid=0`
- `https://docs.google.com/spreadsheets/d/SHEET_ID/edit?usp=sharing`

## License

This project is licensed under the MIT License.
