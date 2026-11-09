# organise - SACDA ingest tool

One spreadsheet in, a checked [Islandora Workbench](https://github.com/mjordan/islandora_workbench) bundle out.

```
organise                       # interactive: pick a sheet and a site, review, write, ingest
organise sheet.xlsx            # same, starting from that sheet (dev by default)
organise sheet.csv --site test --plain   # no screen: print the report, write the bundle if clean
organise ingest <bundle-dir>   # run a written bundle through Workbench (all --check runs first)
organise doctor                # is this machine ready? can it reach and sign in to each site?
```

## Install

macOS / Linux:

```bash
curl -fsSL https://github.com/south-asian-canadian-digital-archive/workbench-preprocessor/releases/latest/download/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://github.com/south-asian-canadian-digital-archive/workbench-preprocessor/releases/latest/download/install.ps1 | iex
```

The installer picks the right build, verifies it against `SHASUMS256.txt`, puts `organise` on PATH, finds the
workbench `g/` folder (it holds `config/sacda.toml`) and remembers it, then runs `organise doctor`.
Re-run it to update. From source: `cargo build --release`.

## What it does

1. **Reads the sheet** - CSV, TSV, Excel, ODS or a Google Sheets link. Cells are trimmed, `#VALUE!` blanked,
   non-breaking spaces and mojibake repaired, and old headers renamed (`boxIdentifier` -> `field_boxidentifier`...).
2. **Builds the tree from identifiers.** `2024_19` is a collection, `2024_19_01` a component inside it,
   `2024_19_01_02` a component inside that (any depth), and the deepest identifiers are items.
   - A row ending `_00`/`_000`, or whose identifier other rows extend, describes that container.
   - Containers no row describes are created anyway, titled from the items' `fileTitle`.
   - Levels: the root is `Collection`, a component holding items is `File`, anything between is `Series`
     (a `field_archival_level` column overrides).
   - Several rows with one item identifier are one item with several files: a `Compound Object` with a part per file.
   - Item models come from the file's media type: Image, Audio, Video, Digital Document, Binary.
   - A collection not in the sheet is looked up on the site and added to; if it isn't there either, it needs a
     title (`t` on the screen, or `--collection-title`).
3. **Prechecks** - always on, against the live site through JSON:API:
   - site reachable, JSON:API on (with how to turn it on), login works (403 flood-lock explained), site converted
     to archival_component collections
   - columns: unknown columns get a "did you mean" from `suggest.rs` (case, `_`, missing `field_`, typos, plurals)
   - structure: identifiers valid, titles present, nothing already on the site
   - files: in the access folder (`access/` next to the sheet), extension accepted by the site's media types,
     no derivatives
   - dates: EDTF, with fixes for what people actually type (`05/1978`, `1970s`, `c. 1975`, `1975-1979`, `May 1979`)
   - terms: every value matched to the site's vocabularies (by name, or by authority URI); near misses suggested;
     new terms get their LCSH / LC name link (exact heading lookup, or the heading for an `sh...`/`n...` identifier
     in the sheet); protected vocabularies and the SACDA Thesaurus are never added to - a missing thesaurus term
     blocks the run until the thesaurus job has run
4. **Fixes.** Safe ones (EDTF, capitalisation, identifier -> heading) are on; guesses (typos) are offered and off
   until accepted (`Space` / `a` on the screen, `--accept-suggestions`). Fixes change the bundle, never the sheet.
5. **Writes the bundle** (`ingest-<collection>-<site>/` next to the sheet) from `g/config/templates/`:
   `terms-<vocab>.csv` + config per vocabulary with new terms, `components.csv` (nested archival_components,
   parents first) with `items.csv` as its secondary task, `bundle.toml` (steps in order) and `precheck-report.txt`.
6. **Ingests** (optional): Workbench `--check` on every step, then the terms jobs, then components + items.

## Configuration (in g/)

- `config/sacda.toml` - sites (dev `isle.dvnl.work`, test `sacda-test.ufv.ca`, prod `sacda.ufv.ca`, which asks
  for confirmation), ingest defaults, and per-field rules: which vocabulary a new term goes to, relators,
  authorities, `never_create`.
- `config/schema.toml` - generated from the site's `config/sync` by `scripts/gen_schema.py`: fields per content type,
  required flags, target vocabularies, media types and their extensions.
- `config/templates/*.yml` - the Workbench configs organise renders.
- Examples covering each scenario: `g/data/examples/` (`make_examples.py` rebuilds them).

## Code map

`src/sacda/`: `config` · `source` (load/clean) · `tree` (identifiers -> tree, models) · `precheck/` (checks, vocab,
fixes, `Session::settle`) · `authority/` (LC lookups, disk cache) · `suggest` · `edtf` · `site` (read-only JSON:API) ·
`bundle` · `run` · `doctor`. `src/tui/` is the screen. Tests: `cargo test` (live LC test: `cargo test -- --ignored`).

---

# Legacy: `organise legacy ...`

The pre-2026 cleaner is still here for old scripts, under `organise legacy`.

Rust CLI and library for streaming CSV cleanup and Islandora-oriented field shaping: local files or Google Sheets in, modified CSV (and optional `items.csv`) out.

### Features

- **Streaming CSV** — large files without loading everything into memory  
- **Google Sheets** — paste an edit URL; the tool fetches CSV export  
- **Modifiers** — `parent_id`, `file` paths, `field_model`, language code → taxonomy ID, plus built-in `accessIdentifier` checks  
- **Items summary** — optional `items.csv` with parent groupings for collections  
- **Validation** — duplicate / empty access IDs, container rows (`_00` / `_000`), title checks  
- **Text cleanup** — common mojibake, NBSPs; sane handling of `field_description` and `;` in cells  
- **File discovery** — `--access-dir` checks every `file` path against the staged files, rewrites the ones that moved, and lists the ones that are missing  
- **Output control** — `--output`, `--output-dir`, `--full`, `--items-output`, `--node`  

**Using it from Rust?** See **[LIBRARY.md](LIBRARY.md)** for the `organise` crate API, pipeline helpers, and examples.

---

### Installation

#### Pre-built binaries

From [Releases](https://github.com/south-asian-canadian-digital-archive/workbench-preprocessor):

| Platform | Archive |
|----------|---------|
| Linux x86_64 | `workbench-preprocessor-linux-x86_64.tar.gz` |
| Windows x86_64 | `workbench-preprocessor-windows-x86_64.zip` |

Extract and keep **`organise`** (or **`organise.exe`**) and **`field_model_mappings.toml`** in the **same folder** — the binary loads that TOML for `field_model` mappings.

#### Build from source

```bash
cargo build --release
# Binary: target/release/organise  (or organise.exe on Windows)
```

---

### How to use

Replace `./target/release/organise` with `organise` (or `.\organise.exe`) if you use a release build on your `PATH`.

#### Process a local table

Writes `<input-stem>-modified.csv` next to the input unless you set `--output` / `--output-dir`.

```bash
organise legacy data.csv
organise legacy data.tsv
organise legacy metadata.xlsx --sheet "Description"
organise legacy catalogue.ods
organise legacy data.csv --output out.csv
organise legacy data.csv --output-dir ./build
organise legacy data.csv --stats
```

#### Process a Google Sheet

Sheet must be reachable as CSV (typically “anyone with the link can view”). Default output name: `sheets-output-modified.csv`.

```bash
organise legacy --url 'https://docs.google.com/spreadsheets/d/SHEET_ID/edit#gid=0'
organise legacy --url 'https://docs.google.com/...' --output-dir ./out --full
```

Supported URL shapes include `/edit`, `/edit#gid=…`, and `?usp=sharing`.

#### Generate `items.csv` only

Input must include **`parent_id`** and **`fileTitle`**.

```bash
organise legacy generate-items modified.csv
organise legacy generate-items modified.csv --output items.csv --node 19
organise legacy generate-items --url 'https://docs.google.com/spreadsheets/d/SHEET_ID/edit#gid=0'
```

#### Full run (process + items)

```bash
organise legacy --full data.csv
organise legacy --full data.csv --node 19 --output-dir ./out
organise legacy --full data.csv --output processed.csv --items-output items.csv
```

#### Common flags

| Flag | Purpose |
|------|---------|
| `--url <URL>` | Input is a Google Sheet (instead of a file path) |
| `--sheet <NAME>` | Worksheet to read from Excel or ODS input; defaults to the first worksheet |
| `-o, --output <FILE>` | Processed CSV path |
| `--output-dir <DIR>` | Put default or relative outputs under this directory |
| `--only-run <MODIFIER>` | Run only these modifiers (repeatable) |
| `--ignore-run <MODIFIER>` | Skip these modifiers (repeatable; wins over `--only-run`) |
| `--access-dir <DIR>` | Resolve `file` paths against the staged access files; rewrite moved ones, report missing ones |
| `--stats` | Print extra processing stats |
| `--full` | After processing, also write `items.csv` |
| `--items-output <FILE>` | With `--full`, path for items file |
| `-n, --node <ID>` | With `--full` or `generate-items`, fill `field_member_of` |
| `--language-url <URL>` | Override language mapping JSON URL (see below) |

**Modifier names** for `--only-run` / `--ignore-run`: `parent-id`, `file-extension`, `field-model`, `language`.

#### Output naming

- **Local file** — `name.csv` → `name-modified.csv` by default.  
- **Google Sheets** — default `sheets-output-modified.csv`.  
- **`--full`** — items file defaults to `<processed-stem>-items.csv` unless `--items-output` is set.  
- **`--output-dir`** — relative paths (including defaults) go under that directory; absolute `--output` wins.

#### `field_language` column (`language` modifier)

Maps values in the **`field_language`** column (ISO-style codes) to taxonomy term IDs using a JSON export. If the modifier runs, the binary **must** fetch that JSON first; on failure it exits without writing output.

- **`--language-url`** — full URL to the JSON.  
- Or **`ISLANDORA_LANGUAGE_URL`** (full URL).  
- Or **`ISLANDORA_BASE_URL`** + path `/lang-code` (default base `http://localhost:8000`).  

Unknown codes are left unchanged. To skip the call entirely: `--ignore-run language`.

#### Built-in rules (always on)

- **`accessIdentifier`** validated: non-empty, no duplicates, rows ending in `_00` / `_000` skipped (containers).  
- **`accessIdentifier` → `field_accessIdentifier`** copy when the source column exists.  
- **`boxIdentifier` → `field_boxIdentifier`**, **`envelopeIdentifier` → `field_envelopeIdentifier`** when targets are missing.  
- Rows with empty **`title`** / **`fileTitle`** after normalisation are skipped and marked in the first column for review.

#### Modifier summary

- **parent-id** — `parent_id` from last segment of `accessIdentifier` (e.g. `2024_19_01_001` → `2024_19_01`).  
- **file-extension** — `file` becomes `parent_id/basename.ext` using `file_extension` or `file_extention`.  
- **field-model** — fills `field_model` from extension via `field_model_mappings.toml`.  
- **language** — replaces **`field_language`** cells with term IDs from JSON (see above).  

`#VALUE!`-style placeholders are treated as empty where applicable.

#### `items.csv` columns

| Column | Meaning |
|--------|---------|
| `file_identifier` | Unique `parent_id` |
| `title` | From `fileTitle` |
| `# of items` | Row count per parent |
| `field_member_of` | From `--node` if set |
| `field_edtf_date` | Derived when date fields exist |
| `field_identifier` | Same as `file_identifier` for Drupal-style mapping |
| `field_archival_level` | Archival level for the parent node (constant `File`) |

---

### Logging

```bash
RUST_LOG=warn organise legacy --url 'https://docs.google.com/spreadsheets/d/SHEET_ID/edit#gid=0'
```

---

### License

MIT — see [LICENSE](LICENSE) if present in the repo.

### Releases

Version tags trigger the release workflow (Linux `.tar.gz`, Windows `.zip`). Draft multi-platform builds are available as a separate manual workflow in `.github/workflows/`.
