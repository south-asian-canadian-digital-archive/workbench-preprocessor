# Library usage (`organise` crate)

Use this crate from another Rust project as a dependency.

```toml
[dependencies]
organise = { path = "../workbench-preprocessor" }  # or git / crates.io when published
```

The library name is **`organise`** (see `Cargo.toml` `[lib] name = "organise"`).

---

## Basic usage

```rust
use organise::{CsvModifier, ParentIdModifier, FileExtensionModifier};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let modifier = CsvModifier::new()
        .add_column_modifier("parent_id", ParentIdModifier)
        .add_column_modifier("file", FileExtensionModifier);

    let stats = modifier.process_file("input.csv", "output.csv")?;

    println!("Processed {} rows", stats.total_rows);
    println!("Modified {} cells", stats.cells_modified);

    Ok(())
}
```

## CLI-equivalent pipeline API

Same behavior as the `organise` binary (default output paths, `--only-run` / `--ignore-run`, `--full`, language URL):

```rust
use organise::{Modifier, ProcessResult};
use organise::{
    process_csv_and_maybe_generate_items,
    process_google_sheets_and_maybe_generate_items,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let res: ProcessResult = process_csv_and_maybe_generate_items(
        "input.csv",
        None,       // explicit output (--output)
        None,       // output directory (--output-dir)
        &[],        // only_run (--only-run)
        &[],        // ignore_run (--ignore-run)
        None,       // language_url (--language-url; or ISLANDORA_LANGUAGE_URL)
        true,       // full (--full)
        None,       // items_output (--items-output)
        Some("19"), // node (-n / --node when full)
    )?;

    println!("processed: {}", res.processed_output_path);
    if let Some(items_path) = res.items_output_path {
        println!("items: {}", items_path);
    }
    Ok(())
}
```

Google Sheets:

```rust
let res = process_google_sheets_and_maybe_generate_items(
    "https://docs.google.com/spreadsheets/d/SHEET_ID/edit#gid=0",
    None,
    None,
    &[],
    &[],
    None,
    true,
    None,
    Some("19"),
)?;
```

`generate-items` wrapper (default output `items.csv`):

```rust
let items_stats = organise::generate_items_from_source(
    Some("input-modified.csv"),
    None,
    None,
    Some("19"),
)?;
println!("total items: {}", items_stats.total_items);
```

## Custom modifiers

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

    fn validate(&self, value: &str, _context: &RowContext) -> bool {
        !value.is_empty()
    }
}

let modifier = CsvModifier::new().add_column_modifier(
    "title",
    PrefixModifier {
        prefix: "DOC_".to_string(),
    },
);
```

## Cross-column access

```rust
use organise::{ColumnModifier, RowContext};

struct CrossColumnModifier;

impl ColumnModifier for CrossColumnModifier {
    fn modify(&self, value: &str, context: &RowContext) -> String {
        let other_value = context.get_or_empty("other_column");
        let another_value = context.get("another_column").unwrap_or("default");
        format!("{}-{}-{}", value, other_value, another_value)
    }

    fn description(&self) -> &str {
        "Combines values from multiple columns"
    }
}
```

## Google Sheets

```rust
use organise::{CsvModifier, ParentIdModifier};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let modifier = CsvModifier::new().add_column_modifier("parent_id", ParentIdModifier);

    let sheets_url = "https://docs.google.com/spreadsheets/d/YOUR_SHEET_ID/edit#gid=0";
    let stats = modifier.process_google_sheets(sheets_url, "output.csv")?;

    println!("Processed {} rows from Google Sheets", stats.total_rows);

    let csv_url = CsvModifier::google_sheets_to_csv_url(sheets_url)?;
    println!("CSV export URL: {}", csv_url);

    Ok(())
}
```

## Items summary (`ItemCsvGenerator`)

```rust
use organise::{CsvModifier, FileExtensionModifier, ItemCsvGenerator, ParentIdModifier};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let modifier = CsvModifier::new()
        .add_column_modifier("parent_id", ParentIdModifier)
        .add_column_modifier("file", FileExtensionModifier);

    modifier.process_file("input.csv", "modified.csv")?;
    ItemCsvGenerator::generate("modified.csv", "items.csv", None)?;
    Ok(())
}
```

`ItemCsvGenerator::generate`:

- Expects `parent_id` and `fileTitle` columns.
- Groups by `parent_id`, counts rows, emits `file_identifier`, `title`, `# of items`, `field_member_of`, `field_edtf_date`, `field_fileidentifier`.
- Skips empty / `#VALUE!` `parent_id` rows.
- Optional node ID fills `field_member_of`.

## `ProcessingStats`

Fields include `total_rows`, `skipped_rows`, `cells_modified`, `validation_failures`, and `columns_processed`. See `ProcessingStats` in `src/csv_modifier.rs`.

`skipped_rows` includes rows dropped by validators (e.g. container-style `accessIdentifier` values ending in `_000`).

## Logging

Validation issues are logged with `log` (default level in the binary is `warn`). In your app, initialize a logger (e.g. `env_logger`) and set `RUST_LOG`.

## Error handling

Errors use `anyhow`: I/O, CSV parsing, HTTP (Sheets), URL parsing, and validation failures surface as `Result` errors.

## Performance

Row-at-a-time CSV processing, buffered I/O, and careful allocation patterns; suitable for very large sheets/files.

## Dependencies (crate)

Key crates: `csv`, `serde`, `anyhow`, `reqwest`, `url`, `clap`, `log`, `toml`, etc. See `Cargo.toml`.

## Examples in this repo

```bash
cargo run --example test_basic
cargo run --example test_full
```
