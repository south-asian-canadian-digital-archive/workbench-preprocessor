//! Writes the ingest bundle from a clean report: the CSVs Workbench reads, the
//! configs rendered for the chosen site, a manifest of the steps in run order,
//! and the precheck report.

use super::config::{Config, FieldDef};
use super::edtf;
use super::precheck::{vocab_render, Fix, Report, Severity, Val};
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
    pub steps: Vec<Step>,
}

impl Manifest {
    pub fn read(dir: &Path) -> Result<Self> {
        let p = dir.join("bundle.toml");
        let text = fs::read_to_string(&p).with_context(|| format!("{} isn't an ingest bundle", dir.display()))?;
        Ok(toml::from_str(&text)?)
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
                let rendered: Vec<String> = raw
                    .split(self.subdelim.as_str())
                    .zip(vals.iter())
                    .filter_map(|(raw_v, v)| {
                        // Keep an explicit relator from the sheet, else the field's default.
                        let relator = raw_v
                            .trim()
                            .strip_prefix("relators:")
                            .and_then(|r| r.split_once(':'))
                            .map(|(code, _)| format!("relators:{code}"))
                            .or_else(|| rule.and_then(|r| r.relator.clone()));
                        match v {
                            Val::Tid(t) => Some(t.clone()),
                            _ => vocab_render(v, def, relator.as_deref()),
                        }
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
    let dir = fs::canonicalize(dir)?;
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
    for n in report.tree.of_kind(Kind::Item) {
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
                .map(|p| p.to_string_lossy().to_string())
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
    w.flush()?;

    // --- terms-<vocab>.csv --------------------------------------------------
    let mut n_terms = 0;
    for (vocab, terms) in &report.new_terms {
        let mut w = csv::Writer::from_path(dir.join(format!("terms-{vocab}.csv")))?;
        w.write_record(["term_name", "field_authority_link"])?;
        for t in terms {
            n_terms += 1;
            w.write_record([t.name.clone(), t.link.as_ref().map(|l| l.workbench_cell()).unwrap_or_default()])?;
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
        steps,
    };
    fs::write(dir.join("bundle.toml"), toml::to_string_pretty(&manifest)?)?;
    fs::write(dir.join("precheck-report.txt"), report_text(report, applied))?;
    Ok(Written { dir, manifest })
}

pub fn now_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

/// Plain-text precheck report, grouped by severity then check.
pub fn report_text(report: &Report, applied: &[Fix]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Precheck: {} error(s), {} warning(s), {} note(s)\n",
        report.count(Severity::Error),
        report.count(Severity::Warning),
        report.count(Severity::Info)
    ));
    let mut groups: BTreeMap<(Severity, &str), Vec<String>> = BTreeMap::new();
    for f in &report.findings {
        let loc = match (&f.line, &f.id) {
            (Some(l), Some(id)) => format!("row {l} ({id}): "),
            (Some(l), None) => format!("row {l}: "),
            (None, Some(id)) => format!("{id}: "),
            _ => String::new(),
        };
        let fix = match (&f.fix, f.auto) {
            (Some(fix), true) => format!("  [fixed: {}]", fix.describe()),
            (Some(fix), false) => format!("  [suggested: {}]", fix.describe()),
            _ => String::new(),
        };
        groups.entry((f.severity, f.check)).or_default().push(format!("{loc}{}{fix}", f.message));
    }
    for ((sev, check), lines) in groups {
        out.push_str(&format!("\n{} - {check}\n", sev.label().to_uppercase()));
        for l in lines {
            out.push_str(&format!("  - {l}\n"));
        }
    }
    if !applied.is_empty() {
        out.push_str(&format!("\nFIXES APPLIED to the bundle ({}; the source sheet is unchanged)\n", applied.len()));
        for f in applied {
            out.push_str(&format!("  - {}\n", f.describe_at()));
        }
    }
    out
}
