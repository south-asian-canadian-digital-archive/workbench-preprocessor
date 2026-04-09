# CSV Organiser

Rust CLI and library for streaming CSV cleanup and Islandora-oriented field shaping: local files or Google Sheets in, modified CSV (and optional `items.csv`) out.

## Features

- **Streaming CSV** — large files without loading everything into memory  
- **Google Sheets** — paste an edit URL; the tool fetches CSV export  
- **Modifiers** — `parent_id`, `file` paths, `field_model`, language code → taxonomy ID, plus built-in `accessIdentifier` checks  
- **Items summary** — optional `items.csv` with parent groupings for collections  
- **Validation** — duplicate / empty access IDs, container rows (`_00` / `_000`), title checks  
- **Text cleanup** — common mojibake, NBSPs; sane handling of `field_description` and `;` in cells  
- **Output control** — `--output`, `--output-dir`, `--full`, `--items-output`, `--node`  

**Using it from Rust?** See **[LIBRARY.md](LIBRARY.md)** for the `organise` crate API, pipeline helpers, and examples.

---

## Installation

### Pre-built binaries

From [Releases](https://github.com/south-asian-canadian-digital-archive/workbench-preprocessor):

| Platform | Archive |
|----------|---------|
| Linux x86_64 | `workbench-preprocessor-linux-x86_64.tar.gz` |
| Windows x86_64 | `workbench-preprocessor-windows-x86_64.zip` |

Extract and keep **`organise`** (or **`organise.exe`**) and **`field_model_mappings.toml`** in the **same folder** — the binary loads that TOML for `field_model` mappings.

### Build from source

```bash
cargo build --release
# Binary: target/release/organise  (or organise.exe on Windows)
```

---

## How to use

Replace `./target/release/organise` with `organise` (or `.\organise.exe`) if you use a release build on your `PATH`.

### Process a local CSV

Writes `<input-stem>-modified.csv` next to the input unless you set `--output` / `--output-dir`.

```bash
organise data.csv
organise data.csv --output out.csv
organise data.csv --output-dir ./build
organise data.csv --stats
```

### Process a Google Sheet

Sheet must be reachable as CSV (typically “anyone with the link can view”). Default output name: `sheets-output-modified.csv`.

```bash
organise --url 'https://docs.google.com/spreadsheets/d/SHEET_ID/edit#gid=0'
organise --url 'https://docs.google.com/...' --output-dir ./out --full
```

Supported URL shapes include `/edit`, `/edit#gid=…`, and `?usp=sharing`.

### Generate `items.csv` only

Input must include **`parent_id`** and **`fileTitle`**.

```bash
organise generate-items modified.csv
organise generate-items modified.csv --output items.csv --node 19
organise generate-items --url 'https://docs.google.com/spreadsheets/d/SHEET_ID/edit#gid=0'
```

### Full run (process + items)

```bash
organise --full data.csv
organise --full data.csv --node 19 --output-dir ./out
organise --full data.csv --output processed.csv --items-output items.csv
```

### Common flags

| Flag | Purpose |
|------|---------|
| `--url <URL>` | Input is a Google Sheet (instead of a file path) |
| `-o, --output <FILE>` | Processed CSV path |
| `--output-dir <DIR>` | Put default or relative outputs under this directory |
| `--only-run <MODIFIER>` | Run only these modifiers (repeatable) |
| `--ignore-run <MODIFIER>` | Skip these modifiers (repeatable; wins over `--only-run`) |
| `--stats` | Print extra processing stats |
| `--full` | After processing, also write `items.csv` |
| `--items-output <FILE>` | With `--full`, path for items file |
| `-n, --node <ID>` | With `--full` or `generate-items`, fill `field_member_of` |
| `--language-url <URL>` | Override language mapping JSON URL (see below) |

**Modifier names** for `--only-run` / `--ignore-run`: `parent-id`, `file-extension`, `field-model`, `language`.

### Output naming

- **Local file** — `name.csv` → `name-modified.csv` by default.  
- **Google Sheets** — default `sheets-output-modified.csv`.  
- **`--full`** — items file defaults to `<processed-stem>-items.csv` unless `--items-output` is set.  
- **`--output-dir`** — relative paths (including defaults) go under that directory; absolute `--output` wins.

### `field_language` column (`language` modifier)

Maps values in the **`field_language`** column (ISO-style codes) to taxonomy term IDs using a JSON export. If the modifier runs, the binary **must** fetch that JSON first; on failure it exits without writing output.

- **`--language-url`** — full URL to the JSON.  
- Or **`ISLANDORA_LANGUAGE_URL`** (full URL).  
- Or **`ISLANDORA_BASE_URL`** + path `/lang-code` (default base `http://localhost:8000`).  

Unknown codes are left unchanged. To skip the call entirely: `--ignore-run language`.

### Built-in rules (always on)

- **`accessIdentifier`** validated: non-empty, no duplicates, rows ending in `_00` / `_000` skipped (containers).  
- **`accessIdentifier` → `field_accessIdentifier`** copy when the source column exists.  
- **`boxIdentifier` → `field_boxIdentifier`**, **`envelopeIdentifier` → `field_envelopeIdentifier`** when targets are missing.  
- Rows with empty **`title`** / **`fileTitle`** after normalisation are skipped and marked in the first column for review.

### Modifier summary

- **parent-id** — `parent_id` from last segment of `accessIdentifier` (e.g. `2024_19_01_001` → `2024_19_01`).  
- **file-extension** — `file` becomes `parent_id/basename.ext` using `file_extension` or `file_extention`.  
- **field-model** — fills `field_model` from extension via `field_model_mappings.toml`.  
- **language** — replaces **`field_language`** cells with term IDs from JSON (see above).  

`#VALUE!`-style placeholders are treated as empty where applicable.

### `items.csv` columns

| Column | Meaning |
|--------|---------|
| `file_identifier` | Unique `parent_id` |
| `title` | From `fileTitle` |
| `# of items` | Row count per parent |
| `field_member_of` | From `--node` if set |
| `field_edtf_date` | Derived when date fields exist |
| `field_fileidentifier` | Same as `file_identifier` for Drupal-style mapping |

---

## Logging

```bash
RUST_LOG=warn organise --url 'https://docs.google.com/spreadsheets/d/SHEET_ID/edit#gid=0'
```

---

## License

MIT — see [LICENSE](LICENSE) if present in the repo.

## Releases

Version tags trigger the release workflow (Linux `.tar.gz`, Windows `.zip`). Draft multi-platform builds are available as a separate manual workflow in `.github/workflows/`.
