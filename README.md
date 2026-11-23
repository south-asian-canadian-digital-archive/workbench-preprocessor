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

### The screen

- **Pick:** `u` pastes a Google Sheets link. It's downloaded straight away and added to a **Google Sheets**
  section at the top of the list (marked "just pulled"), with the collection name, row count and identifier range
  so batches of one fonds can be told apart. Links stay there between runs (`x` removes one); below them are the
  spreadsheets in the current folder.
- **Each sheet has its own files folder and file-name rule**, remembered between runs (`sheets.json` next to
  organise's settings file): select it, `f` to set the folder (type it, or drag the folder into the window; the
  panel shows how many files it holds - counted in the background), `m` to switch file names between "the `file`
  column, else `<identifier>.<ext>`" and "always `<identifier>.<ext>`" (SACDA staging; also
  `--files-by-identifier`). Then Enter checks it (a link is downloaded fresh for the check).
- **Part of a sheet:** `s` on a sheet opens a box for which rows to use, written like PDF pages with the sheet's
  own row numbers: `12`, `12,15`, `-40` (up to 40), `100-` (100 to the end), `12-40`, or an identifier such as
  `2024_19_1545` (it and everything under it). The rows of the picked items' collection and series always come
  along. It's remembered per sheet and shown in the header ("only rows 12-40"); `--only` does the same on the
  command line, and `s` works on the review screen too (even mid-check): the new rows are cut from the sheet already read and
  re-checked with what's already fetched from the site (about a second), and the decisions already made carry over, following each row by its identifier, so rows added or removed in the
  sheet don't move a fix onto another item. Collections and series already on the site are added to, and items already there are left out
  (identifiers are unique), so overlapping or repeated subsets just ingest what's new.
- **Ingesting** (`i`, `organise ingest`, `--ingest`) streams Workbench's output live, the items secondary task's
  too, with a progress bar pinned to the bottom: step n of N, records done out of the step's CSV rows, time so far
  and roughly how long is left. Each step's full log is in `g/logs/organise/<bundle>-<step>.log`.
- **Temporary files:** `c` on the first screen lists what organise keeps in its cache folder (downloaded Google
  Sheets, the id.loc.gov lookup cache) with sizes, and deletes them on Enter. Nothing is cleaned automatically.
  Bundles, logs, rollback files and the id map are never touched - they record what was ingested.
- **Checks run in the background.** Pulling a sheet and its first check show a progress dialog (step, a bar when
  it knows how far along it is, time so far; Esc or its button cancels). Re-checks stay in the status line, so you
  can keep reading and deciding findings while one runs.
- **Rows and files that don't pair up are decisions too**, each listing exactly which rows (row number and
  identifier) or files: rows with no file, rows naming a file the folder doesn't have, and files no row uses.
  `y` leaves them out of the ingest and lists them in the bundle (`skipped-rows.csv` - the full sheet rows plus why
  - or `unmatched-files.csv`) to sort out later; `n` ingests them anyway (without media / without the missing file /
  each unused file as its own item titled by its identifier, offered when every such file is named after an
  identifier whose parent is known). `--plain` leaves them out unless `--keep-unpaired`.
- **Each suggestion is a decision:** `y` uses the fix, `n` keeps the sheet's value (for a term: add it as a new
  term). In a vocabulary listed in Workbench's `protected_vocabularies` (`config/templates/common.yml`) there's no
  `n`: the pane says why, and the value has to be the existing term or fixed in the sheet. The detail pane says what the sheet has,
  which existing term it's close to (with its term number and a link to it on the site), and why: a spelling
  mistake (n letters), singular/plural, accents/case, word order. `[?]` = not decided yet, `[✓]` on, `[–]` off.
  A choice on a term suggestion applies to every row with the same value and the same suggested term (52 rows of
  "Masjid" -> "Masjids" is one keypress). `a` uses every undecided suggestion.
- **Nothing is re-checked until you press `c`.** Write (`w`) and write+ingest (`i`) only appear once the last
  check covers every decision, nothing is undecided and there are no errors. `r` re-reads the sheet and the site
  (after editing the sheet, or ingesting thesaurus terms).
- **Links:** `o` (or the link line under a finding) opens the LCSH/LC heading, the site term, or an id.loc.gov
  search in the browser. On the New terms tab, each term with an authority link has a box: Space leaves that link
  out of the created term, `L` toggles all of them.
- **Mouse:** click sheets (again to open), sites, tabs, findings, their `[?]` box, the `y`/`n`/`o` buttons and
  the footer chips; the wheel scrolls. Mouse capture means the terminal's own text selection needs Shift (Option
  in some terminals) held while dragging.

## What it does

1. **Reads the sheet** - CSV, TSV, Excel, ODS or a Google Sheets link (shared as "anyone with the link"). Cells are
   trimmed, `#VALUE!` blanked, non-breaking spaces and mojibake repaired. The archivist layout is read as-is:
   - headers map to fields: the `[aliases]` in `sacda.toml` (`itemTitle` -> `title`, `date` -> `field_edtf_date`,
     `creator` -> `field_linked_agent`, `lcsh_terms` -> `field_subject`, `subjectPersonal` -> `field_subjects_name`...),
     and otherwise any header that is a field without `field_`, in camelCase, or the field's label
     (`extent`, `custodialHistory`, `Subject (Topical)`)
   - working columns (`notes`, `Cataloguer's Notes`, `SB`, `datetype`, `levelOfDescription`, blank headers) are left
     out - `[sheet] ignore_columns`
   - `lcsh_identifier` pairs value-for-value with `lcsh_terms`: the IDs are used (heading and link come from
     id.loc.gov) when the counts match, the headings otherwise (with a warning)
   - `;` between values in term cells is read like `|`; CA place keys (`vancouver_bc`) are read as names (`Vancouver`)
   - `fondsCollectionTitle` titles a collection that's neither in the sheet nor on the site
   - access files are found by `accessIdentifier` (`2024_19_118_001.tif` anywhere under `access/`), or by a `file`
     column
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
   - files: rows and files pair up (see above), in the access folder (`access/` next to the sheet, or the one chosen with `f` / `--access-dir`), extension accepted by the site's media types,
     no derivatives
   - dates: EDTF, with fixes for what people actually type (`05/1978`, `1970s`, `c. 1975`, `1975-1979`, `May 1979`)
   - terms: every value matched to the site's vocabularies (by name, or by authority URI); near misses suggested;
     new terms get their LCSH / LC name link (exact heading lookup, or the heading for an `sh...`/`n...` identifier
     in the sheet); vocabularies in Workbench's `protected_vocabularies` are never added to. New terms without a
     link are a note, not a warning - most SACDA terms have none
4. **Fixes.** Safe ones (EDTF, capitalisation, identifier -> heading) are on; guesses (typos) are offered and off
   until accepted (`y` / `a` on the screen, `--accept-suggestions`). Fixes change the bundle, never the sheet.
   Failed id.loc.gov lookups (throttling) aren't retried within a run, so a re-check takes about a second;
   `r` tries them again.
5. **Writes the bundle** (`ingest-<collection>-<site>/` next to the sheet) from `g/config/templates/`:
   `terms-<vocab>.csv` + config per vocabulary with new terms, `components.csv` (nested archival_components,
   parents first) with `items.csv` as its secondary task, `bundle.toml` (steps in order) and `precheck-report.txt`.
6. **Ingests** (optional): Workbench `--check` on every step, then the terms jobs, then components + items.

## Configuration (in g/)

- `config/sacda.toml` - sites (dev `isle.dvnl.work`, test `sacda-test.ufv.ca`, prod `sacda.ufv.ca`, which asks
  for confirmation), ingest defaults, and per-field rules: which vocabulary a new term goes to, relators,
  authorities.
- `config/templates/common.yml` - also holds Workbench's `protected_vocabularies`. organise reads the same list:
  a misspelt value in one of those can only be fixed in the sheet, not kept as a new term.
- `config/schema.toml` - generated from the site's `config/sync` by `scripts/gen_schema.py`: fields per content type,
  required flags, target vocabularies, media types and their extensions.
- `config/templates/*.yml` - the Workbench configs organise renders.
- Examples covering each scenario: `g/data/examples/` (`make_examples.py` rebuilds them).

## Code map

A Cargo workspace, so a change to the screen doesn't rebuild the checks:

| Crate | | |
|---|---|---|
| `crates/core` | `sacda-core` (`use sacda::...`) | Everything that reads or changes data, no UI: `config` · `input` (tables, Google Sheets, cell clean-up) · `source` (load/clean) · `tree` (identifiers -> tree, models) · `precheck/` (checks, vocab, fixes, `Session::settle`) · `authority/` (LC lookups, disk cache) · `suggest` · `edtf` · `site` (read-only JSON:API) · `bundle` · `run` · `doctor` |
| `crates/tui` | `sacda-tui` | The interactive screen; only calls into `sacda-core` |
| `.` | `workbench-preprocessor` | The `organise` binary: CLI + plain mode |

Tests: `cargo test --workspace` (live LC test: `cargo test --workspace -- --ignored`).

---

### License

MIT — see [LICENSE](LICENSE) if present in the repo.

### Releases

Actions → **Draft release (multi-platform)** → Run workflow. It runs clippy and the tests, builds Linux, Windows and both macOS targets with checksums, and leaves a draft release to publish.
