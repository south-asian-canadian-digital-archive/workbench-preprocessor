//! Writes the ingest bundle from a clean report: the CSVs Workbench reads, the
//! configs rendered for the chosen site, a manifest of the steps in run order,
//! and the precheck report.

use super::config::{Config, FieldDef};
use super::edtf;
use super::precheck::{vocab_render, Fix, Gap, Report, Severity};
use super::source::Table;
use super::tree::{Kind, Node};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    /// Config file name inside the bundle.
    pub config: String,
    /// Other tasks Workbench runs as this one's secondary tasks.
    #[serde(default)]
    pub covers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub site: String,
    pub host: String,
    pub source: String,
    pub created: String,
    pub collections: Vec<String>,
    pub components: usize,
    pub items: usize,
    pub files: usize,
    pub new_terms: usize,
    /// Rows left out because their files didn't pair up (skipped-rows.csv).
    #[serde(default)]
    pub skipped_rows: usize,
    /// Files no row used, left out (unmatched-files.csv).
    #[serde(default)]
    pub unmatched_files: usize,
    /// Items already on the site whose fields are updated (update.csv).
    #[serde(default)]
    pub updates: usize,
    /// Files added to items already on the site, replacing or adding media (add-media.csv).
    #[serde(default)]
    pub media: usize,
    pub steps: Vec<Step>,
}

impl Manifest {
    pub fn read(dir: &Path) -> Result<Self> {
        let p = dir.join("bundle.toml");
        let text = fs::read_to_string(&p).with_context(|| format!("{} isn't an ingest bundle", dir.display()))?;
        let m: Self = toml::from_str(&text)?;
        // Steps run the configs they name, and a Workbench config can name
        // scripts to run: only files inside this bundle folder.
        for step in &m.steps {
            for name in std::iter::once(&step.config).chain(step.covers.iter()) {
                let plain = !name.is_empty()
                    && !name.contains(['/', '\\', ':'])
                    && name != ".."
                    && Path::new(name).components().count() == 1;
                if !plain {
                    anyhow::bail!("{} names \"{name}\" - a bundle's steps must be files in the bundle folder", p.display());
                }
            }
        }
        Ok(m)
    }
}

/// Default bundle folder: next to the sheet, named after the collection(s) and site.
pub fn default_dir(input: &str, report: &Report, site: &str) -> PathBuf {
    let base = if super::source::is_url(input) {
        std::env::current_dir().unwrap_or_default()
    } else {
        Path::new(input).parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let roots: Vec<String> = report.tree.roots().iter().map(|n| n.id.clone()).collect();
    let name = if roots.is_empty() { "bundle".to_string() } else { roots.join("+") };
    base.join(format!("ingest-{name}-{site}"))
}

fn yaml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A file's path for the CSV's `file` column. Windows paths get `/`:
/// Workbench sends the path as the upload's `filename*`, and Drupal keeps
/// what's after the last `/` - with `\` it kept the whole path as the file's
/// name ("C:\Users\...\photo.tif"), which broke derivatives. Python on
/// Windows opens `C:/Users/...` the same.
fn csv_path(p: &Path) -> String {
    slashes(&p.to_string_lossy(), cfg!(windows))
}

fn slashes(s: &str, windows: bool) -> String {
    if windows { s.replace('\\', "/") } else { s.to_string() }
}

fn path_str(p: &Path) -> String {
    yaml_str(&p.to_string_lossy())
}

struct Ctx<'a> {
    cfg: &'a Config,
    table: &'a Table,
    report: &'a Report,
    subdelim: String,
}

impl Ctx<'_> {
    /// Columns of `table` that are fields of `bundle` and not handled specially.
    fn field_columns(&self, bundle: &str, skip: &[&str]) -> Vec<(String, FieldDef)> {
        let fields = self.cfg.schema.fields(bundle);
        self.table
            .headers
            .iter()
            .filter(|h| !skip.contains(&h.as_str()) && !self.report.dropped_columns.contains(h))
            .filter_map(|h| fields.get(h).map(|d| (h.clone(), d.clone())))
            .collect()
    }

    fn cell(&self, row: usize, col: &str, def: &FieldDef) -> String {
        let raw = self.table.get(&self.table.rows[row], col);
        if raw.is_empty() {
            return String::new();
        }
        if def.is_taxonomy() {
            if let Some(vals) = self.report.values.get(&(row, col.to_string())) {
                let rule = self.cfg.sacda.fields.get(col);
                // The same non-empty pieces the check resolved, so each value
                // keeps its own relator (`a||b` has no third value).
                let rendered: Vec<String> = raw
                    .split(self.subdelim.as_str())
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .zip(vals.iter())
                    .filter_map(|(raw_v, v)| {
                        // Keep an explicit relator from the sheet, else the field's default.
                        let relator = raw_v
                            .trim()
                            .strip_prefix("relators:")
                            .and_then(|r| r.split_once(':'))
                            .map(|(code, _)| format!("relators:{code}"))
                            .or_else(|| rule.and_then(|r| r.relator.clone()));
                        vocab_render(v, def, relator.as_deref())
                    })
                    .collect();
                return rendered.join(&self.subdelim);
            }
        }
        raw.to_string()
    }

    fn node_date(&self, node: &Node) -> Option<String> {
        let mut dates: Vec<String> = Vec::new();
        self.collect_dates(&node.id, &mut dates);
        let refs: Vec<&str> = dates.iter().map(String::as_str).collect();
        edtf::span(&refs)
    }

    fn collect_dates(&self, id: &str, out: &mut Vec<String>) {
        for child in self.report.tree.children(id) {
            if let Some(r) = child.row {
                let row = &self.table.rows[r];
                let col = if child.kind == Kind::Item {
                    ["field_edtf_date_created", "field_edtf_date"]
                } else {
                    ["field_edtf_date", "field_edtf_date"]
                };
                if let Some(d) = col.iter().map(|c| self.table.get(row, c)).find(|d| !d.is_empty()) {
                    out.extend(d.split(self.subdelim.as_str()).map(|s| s.trim().to_string()));
                    continue;
                }
            }
            self.collect_dates(&child.id, out);
        }
    }
}

/// Parent link for a node: (parent_id, field_member_of).
fn parent_link(report: &Report, node: &Node) -> (String, String) {
    match &node.parent {
        Some(p) => match report.attach.get(p) {
            Some(existing) => (String::new(), existing.nid.clone()),
            None => (p.clone(), String::new()),
        },
        None => (String::new(), String::new()),
    }
}

pub struct Written {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

pub fn write(cfg: &Config, site: &str, table: &Table, report: &Report, applied: &[Fix], dir: &Path) -> Result<Written> {
    if report.blocked() {
        bail!("the precheck has {} error(s); nothing was written", report.count(Severity::Error));
    }
    let (_, site_cfg) = cfg.site(Some(site))?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let dir = dunce::canonicalize(dir)?;
    // The default folder is reused for the same collection and site: clear
    // what an earlier bundle wrote, so nothing stale sits beside the new one.
    // Only organise's own files; logs, rollback and anything else stay.
    if let Ok(entries) = fs::read_dir(&dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let ours = name == "bundle.toml"
                || name == "precheck-report.txt"
                || name == "items.config.yml"
                || name == "components.config.yml"
                || ["update", "delete-media"].iter().any(|s| name == format!("{s}.csv") || name == format!("{s}.config.yml"))
                || (name.starts_with("add-media") && (name.ends_with(".csv") || name.ends_with(".config.yml")))
                || (name.starts_with("terms-") && (name.ends_with(".csv") || name.ends_with(".config.yml")));
            if ours && e.file_type().is_ok_and(|t| t.is_file()) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    let ctx = Ctx {
        cfg,
        table,
        report,
        subdelim: cfg.sacda.ingest.subdelimiter.clone(),
    };

    // --- components.csv ---------------------------------------------------
    let comp_cols = ctx.field_columns(
        "archival_component",
        &["title", "field_identifier", "field_archival_level", "field_model", "field_member_of", "field_edtf_date"],
    );
    let containers: Vec<&Node> = report
        .tree
        .containers_in_order()
        .into_iter()
        .filter(|n| !report.attach.contains_key(&n.id))
        .collect();
    let mut w = csv::Writer::from_path(dir.join("components.csv"))?;
    let mut header = vec![
        "id", "parent_id", "field_member_of", "title", "field_identifier", "field_archival_level", "field_model",
        "field_edtf_date",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    header.extend(comp_cols.iter().map(|(c, _)| c.clone()));
    w.write_record(&header)?;
    for n in &containers {
        let (parent_id, member_of) = parent_link(report, n);
        let explicit_date = n
            .row
            .map(|r| ctx.table.get(&ctx.table.rows[r], "field_edtf_date").to_string())
            .unwrap_or_default();
        let date = if explicit_date.is_empty() { ctx.node_date(n).unwrap_or_default() } else { explicit_date };
        let mut rec = vec![
            n.id.clone(),
            parent_id,
            member_of,
            n.title.clone(),
            n.id.clone(),
            n.level.clone(),
            "Collection".to_string(),
            date,
        ];
        for (c, d) in &comp_cols {
            rec.push(n.row.map(|r| ctx.cell(r, c, d)).unwrap_or_default());
        }
        w.write_record(&rec)?;
    }
    w.flush()?;

    // --- items.csv ----------------------------------------------------------
    let item_cols = ctx.field_columns(
        "islandora_object",
        &["title", "field_identifier", "field_model", "field_member_of", "field_weight"],
    );
    let mut w = csv::Writer::from_path(dir.join("items.csv"))?;
    let mut header: Vec<String> = ["id", "parent_id", "field_member_of", "title", "field_identifier", "field_model", "file", "field_weight"]
        .into_iter()
        .map(String::from)
        .collect();
    header.extend(item_cols.iter().map(|(c, _)| c.clone()));
    let has_date_created = item_cols.iter().any(|(c, _)| c == "field_edtf_date_created");
    if !has_date_created && table.col("field_edtf_date").is_some() {
        header.push("field_edtf_date_created".into());
    }
    w.write_record(&header)?;
    let mut n_items = 0;
    let mut n_files = 0;
    // Rows and files that don't pair up: left out when that choice is on.
    let gap_on = |g: Gap| applied.contains(&Fix::FileGap(g));
    let mut skipped: Vec<(usize, String)> = Vec::new();
    for n in report.tree.of_kind(Kind::Item).filter(|n| !report.on_site_items.contains(&n.id)) {
        let rows_of = |n: &Node| {
            let mut r: Vec<usize> = n.row.into_iter().chain(n.files.iter().map(|f| f.row)).collect();
            r.sort_unstable();
            r.dedup();
            r
        };
        let missing: Vec<&str> = n.files.iter().filter(|f| f.path.is_none()).map(|f| f.wanted.as_str()).collect();
        if n.files.is_empty() && gap_on(Gap::NoFile) {
            skipped.extend(rows_of(n).into_iter().map(|r| (r, "no file in the access folder".to_string())));
            continue;
        }
        if !missing.is_empty() && gap_on(Gap::MissingFile) {
            let why = format!("file not in the access folder: {}", missing.join(", "));
            skipped.extend(rows_of(n).into_iter().map(|r| (r, why.clone())));
            continue;
        }
        // Ingesting it anyway: without the files that aren't there.
        let trimmed;
        let n = if missing.is_empty() {
            n
        } else {
            let mut t = n.clone();
            t.files.retain(|f| f.path.is_some());
            t.model = match t.files.as_slice() {
                [] => cfg.sacda.ingest.no_file_model.clone(),
                [one] => super::tree::media_for_extension(cfg, &one.extension).map(|(_, m)| m.to_string()).unwrap_or(t.model),
                _ => t.model,
            };
            trimmed = t;
            &trimmed
        };
        n_items += 1;
        let (parent_id, member_of) = parent_link(report, n);
        let row = n.row.expect("items always come from a row");
        let fields: Vec<String> = item_cols.iter().map(|(c, d)| ctx.cell(row, c, d)).collect();
        let extra_date = (!has_date_created && table.col("field_edtf_date").is_some())
            .then(|| table.get(&table.rows[row], "field_edtf_date").to_string());
        let path_of = |i: usize| {
            n.files[i]
                .path
                .as_ref()
                .map(|p| csv_path(p))
                .unwrap_or_default()
        };

        let single = n.files.len() <= 1;
        let mut rec = vec![
            n.id.clone(),
            parent_id,
            member_of,
            n.title.clone(),
            n.id.clone(),
            n.model.clone(),
            if single && !n.files.is_empty() { path_of(0) } else { String::new() },
            String::new(),
        ];
        rec.extend(fields);
        rec.extend(extra_date.clone());
        w.write_record(&rec)?;
        n_files += n.files.len();

        if !single {
            // Each file becomes a part of the compound object, in sheet order.
            for (k, f) in n.files.iter().enumerate() {
                let part_row = &table.rows[f.row];
                let own_title = table.get(part_row, "title");
                let title = if own_title.is_empty() || (k == 0 && own_title == n.title) {
                    format!("{}, part {} of {}", n.title, k + 1, n.files.len())
                } else {
                    own_title.to_string()
                };
                let model = super::tree::media_for_extension(cfg, &f.extension)
                    .map(|(_, m)| m.to_string())
                    .unwrap_or_else(|| "Binary".into());
                let mut rec = vec![
                    format!("{}-{:02}", n.id, k + 1),
                    n.id.clone(),
                    String::new(),
                    title,
                    format!("{}-{:02}", n.id, k + 1),
                    model,
                    path_of(k),
                    (k + 1).to_string(),
                ];
                rec.extend(std::iter::repeat_n(String::new(), item_cols.len() + extra_date.iter().count()));
                w.write_record(&rec)?;
            }
        }
    }
    // Files no row used, as items of their own - only when that was chosen.
    let add_stubs = !report.stub_items.is_empty() && !gap_on(Gap::Unmatched);
    if add_stubs {
        let extra_cols = item_cols.len() + usize::from(!has_date_created && table.col("field_edtf_date").is_some());
        for (id, parent) in &report.stub_items {
            let Some(path) = report.unmatched_files.iter().find(|p| p.file_stem().is_some_and(|s| s.to_string_lossy() == *id)) else { continue };
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let model = super::tree::media_for_extension(cfg, &ext).map(|(_, m)| m.to_string()).unwrap_or_else(|| "Binary".into());
            let stub = Node {
                id: id.clone(),
                parent: Some(parent.clone()),
                kind: Kind::Item,
                row: None,
                title: id.clone(),
                level: String::new(),
                model: model.clone(),
                files: vec![],
            };
            let (parent_id, member_of) = parent_link(report, &stub);
            let mut rec = vec![
                id.clone(),
                parent_id,
                member_of,
                id.clone(),
                id.clone(),
                model,
                csv_path(path),
                String::new(),
            ];
            rec.extend(std::iter::repeat_n(String::new(), extra_cols));
            w.write_record(&rec)?;
            n_items += 1;
            n_files += 1;
        }
    }
    w.flush()?;

    // --- what was left out, to sort out later ---------------------------------
    let _ = fs::remove_file(dir.join(Gap::NoFile.csv()));
    let _ = fs::remove_file(dir.join(Gap::Unmatched.csv()));
    if !skipped.is_empty() {
        let mut w = csv::Writer::from_path(dir.join(Gap::NoFile.csv()))?;
        let mut header = vec!["organise_reason".to_string(), "sheet_row".to_string()];
        header.extend(table.headers.iter().cloned());
        w.write_record(&header)?;
        for (r, why) in &skipped {
            let row = &table.rows[*r];
            let mut rec = vec![why.clone(), row.line.to_string()];
            rec.extend((0..table.headers.len()).map(|c| row.cells.get(c).cloned().unwrap_or_default()));
            w.write_record(&rec)?;
        }
        w.flush()?;
    }
    let n_unmatched = if add_stubs { 0 } else { report.unmatched_files.len() };
    if n_unmatched > 0 {
        let mut w = csv::Writer::from_path(dir.join(Gap::Unmatched.csv()))?;
        w.write_record(["file", "path", "organise_reason"])?;
        for p in &report.unmatched_files {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            w.write_record([name, p.to_string_lossy().to_string(), "no row in the sheet uses this file".to_string()])?;
        }
        w.flush()?;
    }

    // --- terms-<vocab>.csv --------------------------------------------------
    let mut n_terms = 0;
    for (vocab, terms) in &report.new_terms {
        let mut w = csv::Writer::from_path(dir.join(format!("terms-{vocab}.csv")))?;
        // Only with links: Workbench stops on a column the vocabulary lacks.
        let links = terms.iter().any(|t| t.link.is_some());
        w.write_record(if links { vec!["term_name", "field_authority_link"] } else { vec!["term_name"] })?;
        for t in terms {
            n_terms += 1;
            let mut rec = vec![t.name.clone()];
            if links {
                rec.push(t.link.as_ref().map(|l| l.workbench_cell()).unwrap_or_default());
            }
            w.write_record(rec)?;
        }
        w.flush()?;
    }

    // --- configs -------------------------------------------------------------
    let tag = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "bundle".into());
    let templates = cfg.g_root.join("config/templates");
    let common = fs::read_to_string(templates.join("common.yml")).context("config/templates/common.yml")?;
    let logs = cfg.g_root.join("logs/organise");
    fs::create_dir_all(&logs)?;
    fs::create_dir_all(cfg.g_root.join("rollback/csv"))?;
    fs::create_dir_all(cfg.g_root.join("rollback/config"))?;
    let common = common
        .replace("{{host}}", &yaml_str(&site_cfg.host))
        .replace("{{credentials}}", &path_str(&cfg.g_root.join(&site_cfg.credentials)))
        .replace("{{input_dir}}", &path_str(&dir))
        .replace("{{subdelimiter}}", &yaml_str(&ctx.subdelim))
        .replace("{{id_map}}", &path_str(&logs.join(format!("{site}-id-map.db"))))
        .replace("{{rollback_dir}}", &path_str(&cfg.g_root.join("rollback/csv")));
    let render = |template: &str, name: &str, vars: &[(&str, String)]| -> Result<String> {
        let mut text = fs::read_to_string(templates.join(template)).with_context(|| format!("config/templates/{template}"))?;
        text = text
            .replace("{{common}}", &common)
            .replace("{{log}}", &path_str(&logs.join(format!("{tag}-{name}.log"))))
            .replace(
                "{{rollback_config}}",
                &path_str(&cfg.g_root.join(format!("rollback/config/rollback-{tag}-{name}.yml"))),
            );
        for (k, v) in vars {
            text = text.replace(&format!("{{{{{k}}}}}"), v);
        }
        Ok(text)
    };

    let mut steps = Vec::new();
    for vocab in report.new_terms.keys() {
        let name = format!("terms-{vocab}");
        let text = render(
            "terms.config.yml",
            &name,
            &[("vocab", vocab.clone()), ("input_csv", format!("{name}.csv"))],
        )?;
        fs::write(dir.join(format!("{name}.config.yml")), text)?;
        steps.push(Step { name: name.clone(), config: format!("{name}.config.yml"), covers: vec![] });
    }

    let has_components = !containers.is_empty();
    let has_items = n_items > 0;
    let items_cfg = dir.join("items.config.yml");
    if has_items {
        let text = render("items.config.yml", "items", &[("secondary_tasks", String::new())])?;
        fs::write(&items_cfg, text)?;
    }
    if has_components {
        let secondary = if has_items {
            format!("secondary_tasks:\n  - {}", path_str(&items_cfg))
        } else {
            String::new()
        };
        let text = render("components.config.yml", "components", &[("secondary_tasks", secondary)])?;
        fs::write(dir.join("components.config.yml"), text)?;
        steps.push(Step {
            name: "components".into(),
            config: "components.config.yml".into(),
            covers: if has_items { vec!["items".into()] } else { vec![] },
        });
    } else if has_items {
        steps.push(Step { name: "items".into(), config: "items.config.yml".into(), covers: vec![] });
    }
    if !has_components {
        let _ = fs::remove_file(dir.join("components.config.yml"));
    }

    // --- items already on the site: update fields, replace or add media ------
    let existing = existing_steps(&ctx, applied, &dir)?;
    for (name, task_lines) in &existing.steps {
        let text = format!(
            "# Items already on the site - generated by organise.\n{task_lines}\nlog_file_path: {}\n{common}\n",
            path_str(&logs.join(format!("{tag}-{name}.log")))
        );
        fs::write(dir.join(format!("{name}.config.yml")), text)?;
        steps.push(Step { name: name.clone(), config: format!("{name}.config.yml"), covers: vec![] });
    }
    if steps.is_empty() {
        bail!("nothing to ingest: no new collections, items or terms, and nothing to change on the items already on the site");
    }

    let manifest = Manifest {
        site: site.to_string(),
        host: site_cfg.host.clone(),
        source: table.source.clone(),
        created: now_stamp(),
        collections: report.tree.roots().iter().map(|n| n.id.clone()).collect(),
        components: containers.len(),
        items: n_items,
        files: n_files,
        new_terms: n_terms,
        skipped_rows: skipped.iter().map(|(r, _)| *r).collect::<std::collections::BTreeSet<_>>().len(),
        unmatched_files: n_unmatched,
        updates: existing.updates,
        media: existing.media,
        steps,
    };
    fs::write(dir.join("bundle.toml"), toml::to_string_pretty(&manifest)?)?;
    fs::write(dir.join("precheck-report.txt"), report_text(report, applied, table))?;
    Ok(Written { dir, manifest })
}

/// The extra Workbench jobs for items already on the site.
struct Existing {
    /// (step name, its task lines), in run order.
    steps: Vec<(String, String)>,
    updates: usize,
    media: usize,
}

/// update.csv (the chosen columns, by node ID), delete-media.csv (media being
/// replaced) and add-media.csv (the file for each, replaced or missing).
fn existing_steps(ctx: &Ctx, applied: &[Fix], dir: &Path) -> Result<Existing> {
    let report = ctx.report;
    let mut out = Existing { steps: vec![], updates: 0, media: 0 };
    let on_site: Vec<&Node> = report.tree.of_kind(Kind::Item).filter(|n| report.on_site_items.contains(&n.id)).collect();
    let nid = |n: &Node| report.on_site_nids.get(&n.id).cloned();
    if !report.update_columns.is_empty() {
        let cols: Vec<(String, FieldDef)> = ctx
            .field_columns("islandora_object", &["title", "field_identifier", "field_model", "field_member_of", "field_weight"])
            .into_iter()
            .filter(|(c, _)| report.update_columns.contains(c))
            .collect();
        let title = report.update_columns.iter().any(|c| c == "title");
        let mut w = csv::Writer::from_path(dir.join("update.csv"))?;
        let mut header = vec!["node_id".to_string()];
        if title {
            header.push("title".into());
        }
        header.extend(cols.iter().map(|(c, _)| c.clone()));
        w.write_record(&header)?;
        for n in &on_site {
            let (Some(id), Some(row)) = (nid(n), n.row) else { continue };
            let mut rec = vec![id];
            if title {
                rec.push(n.title.clone());
            }
            rec.extend(cols.iter().map(|(c, d)| ctx.cell(row, c, d)));
            w.write_record(&rec)?;
            out.updates += 1;
        }
        w.flush()?;
        if out.updates > 0 {
            out.steps.push(("update".into(), "task: update\ncontent_type: islandora_object\ninput_csv: update.csv\nupdate_mode: replace\nallow_adding_terms: false".into()));
        }
    }
    let one_file = |n: &Node| -> Option<String> { (n.files.len() == 1).then(|| n.files[0].path.as_ref().map(|p| csv_path(p))).flatten() };
    let mut add: Vec<(String, String)> = Vec::new();
    if report.replace_media {
        let replace: Vec<(String, String)> = on_site.iter().filter_map(|n| Some((nid(n)?, one_file(n)?))).collect();
        if !replace.is_empty() {
            let mut w = csv::Writer::from_path(dir.join("delete-media.csv"))?;
            w.write_record(["node_id"])?;
            for (id, _) in &replace {
                w.write_record([id])?;
            }
            w.flush()?;
            out.steps.push(("delete-media".into(), "task: delete_media_by_node\ninput_csv: delete-media.csv".into()));
            add.extend(replace);
        }
    } else if applied.contains(&Fix::AddMedia) {
        add.extend(on_site.iter().filter(|n| report.missing_media.contains(&n.id)).filter_map(|n| Some((nid(n)?, one_file(n)?))));
    }
    // Workbench's add_media takes one media type per job: one job per type.
    let mut by_type: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    for (id, f) in add {
        let ext = Path::new(&f).extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        let media = workbench_media_type(&ext);
        by_type.entry(media).or_default().push((id, f));
    }
    for (media, rows) in by_type {
        let name = format!("add-media-{media}");
        let mut w = csv::Writer::from_path(dir.join(format!("{name}.csv")))?;
        w.write_record(["node_id", "file"])?;
        for (id, f) in &rows {
            w.write_record([id, f])?;
        }
        w.flush()?;
        out.media += rows.len();
        out.steps.push((
            name.clone(),
            format!("task: add_media\nmedia_type: {media}\ninput_csv: {name}.csv\nallow_missing_files: false\nfixity_algorithm: md5"),
        ));
    }
    Ok(out)
}

/// The media type Workbench gives a file when it creates an item (its own
/// default list, WorkbenchConfig.get_media_types), so a file added later lands
/// where the first one would have: a TIFF is a `file`, not an `image` (the
/// image field takes web formats only).
fn workbench_media_type(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "png" | "gif" | "jpg" | "jpeg" => "image",
        "pdf" | "doc" | "docx" | "ppt" | "pptx" => "document",
        "mp3" | "wav" | "aac" => "audio",
        "mp4" | "mov" | "wmv" | "avi" | "mts" | "flv" | "f4v" | "swf" | "mkv" | "webm" | "ogv" | "mpeg" => "video",
        "txt" => "extracted_text",
        _ => "file",
    }
}

/// The sheet's columns that can overwrite the same fields on items already on
/// the site: the title and the item fields (not the structural ones).
pub fn updatable_columns(cfg: &Config, table: &Table, report: &Report) -> Vec<String> {
    let ctx = Ctx { cfg, table, report, subdelim: cfg.sacda.ingest.subdelimiter.clone() };
    let mut cols = vec!["title".to_string()];
    cols.extend(
        ctx.field_columns("islandora_object", &["title", "field_identifier", "field_model", "field_member_of", "field_weight"])
            .into_iter()
            .map(|(c, _)| c),
    );
    cols
}

pub fn now_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

/// Plain-text precheck report, grouped by severity then check.
/// "row 2", "rows 2-5, 9 (12 rows)" for the grouped report lines.
fn rows_label(lines: &[usize]) -> String {
    let mut ls = lines.to_vec();
    ls.sort_unstable();
    ls.dedup();
    if ls.len() == 1 {
        return format!("row {}", ls[0]);
    }
    let mut spans: Vec<String> = Vec::new();
    let mut start = ls[0];
    let mut prev = ls[0];
    for &l in &ls[1..] {
        if l != prev + 1 {
            spans.push(if start == prev { start.to_string() } else { format!("{start}-{prev}") });
            start = l;
        }
        prev = l;
    }
    spans.push(if start == prev { start.to_string() } else { format!("{start}-{prev}") });
    let shown = if spans.len() > 6 { format!("{}, ...", spans[..6].join(", ")) } else { spans.join(", ") };
    format!("rows {shown} ({} rows)", ls.len())
}

pub fn report_text(report: &Report, applied: &[Fix], table: &Table) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Precheck: {} error(s), {} warning(s), {} note(s)\n",
        report.count(Severity::Error),
        report.count(Severity::Warning),
        report.count(Severity::Info)
    ));
    // Same message (and same fix) on many rows -> one line listing the rows.
    type Key<'a> = (Severity, &'a str);
    type Lines = Vec<(String, Vec<usize>, Vec<String>)>; // (text, rows, ids)
    let mut groups: BTreeMap<Key, Lines> = BTreeMap::new();
    for f in &report.findings {
        let fix = match (&f.fix, f.auto) {
            (Some(fix), true) => format!("  [fixed: {}]", fix.describe()),
            (Some(fix), false) => format!("  [suggested: {}]", fix.describe()),
            _ => String::new(),
        };
        let text = format!("{}{fix}", f.message);
        let g = groups.entry((f.severity, f.check)).or_default();
        let slot = match g.iter().position(|(t, _, _)| *t == text) {
            Some(p) => p,
            None => {
                g.push((text, Vec::new(), Vec::new()));
                g.len() - 1
            }
        };
        if let Some(l) = f.line {
            g[slot].1.push(l);
        }
        if let Some(id) = &f.id {
            g[slot].2.push(id.clone());
        }
    }
    for ((sev, check), lines) in groups {
        out.push_str(&format!("\n{} - {check}\n", sev.label().to_uppercase()));
        for (text, ls, ids) in lines {
            let loc = match (ls.len(), ids.first()) {
                (0, Some(id)) if ids.len() == 1 => format!("{id}: "),
                (0, _) => String::new(),
                (1, Some(id)) => format!("row {} ({id}): ", ls[0]),
                _ => format!("{}: ", rows_label(&ls)),
            };
            out.push_str(&format!("  - {loc}{text}\n"));
        }
    }
    if !applied.is_empty() {
        out.push_str(&format!("\nFIXES APPLIED to the bundle ({}; the source sheet is unchanged)\n", applied.len()));
        let mut by: Vec<(String, Vec<usize>)> = Vec::new();
        for f in applied {
            let line = match f {
                Fix::SetCell { row, .. } => table.rows.get(*row).map(|r| r.line),
                _ => None,
            };
            let d = f.describe();
            match (line, by.iter().position(|(x, _)| *x == d)) {
                (Some(l), Some(p)) => by[p].1.push(l),
                (Some(l), None) => by.push((d, vec![l])),
                (None, _) => by.push((f.describe_at(table), Vec::new())),
            }
        }
        for (d, ls) in by {
            if ls.is_empty() {
                out.push_str(&format!("  - {d}\n"));
            } else {
                out.push_str(&format!("  - {}: {d}\n", rows_label(&ls)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_file_paths_use_forward_slashes() {
        assert_eq!(slashes(r"C:\Users\a\access\photo 1.tif", true), "C:/Users/a/access/photo 1.tif");
        assert_eq!(slashes(r"\\server\share\x.jpg", true), "//server/share/x.jpg");
        // Elsewhere a backslash is part of the name: left alone.
        assert_eq!(slashes(r"/a/odd\name.jpg", false), r"/a/odd\name.jpg");
    }

    #[test]
    fn added_media_types_follow_workbench() {
        assert_eq!(workbench_media_type("TIF"), "file");
        assert_eq!(workbench_media_type("jpg"), "image");
        assert_eq!(workbench_media_type("pdf"), "document");
        assert_eq!(workbench_media_type("mp4"), "video");
        assert_eq!(workbench_media_type("xyz"), "file");
    }
}
