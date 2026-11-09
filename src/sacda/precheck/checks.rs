//! The non-vocabulary checks: site, columns, structure, files, dates.

use super::{Finding, Fix, Report, Session, Severity::*};
use crate::sacda::edtf;
use crate::sacda::site::{Login, JSONAPI_HELP};
use crate::sacda::source::Table;
use crate::sacda::suggest;
use crate::sacda::tree::{media_for_extension, Kind, ID_COLUMNS};

/// Columns organise itself understands; not Drupal fields.
pub const RESERVED: &[&str] = &[
    "accessIdentifier",
    "id",
    "title",
    "file",
    "file_extension",
    "fileTitle",
    "parent_id",
    "field_identifier",
    "field_model",
    "field_archival_level",
    "field_member_of",
    "# of items",
];

pub fn site(s: &mut Session, report: &mut Report) {
    if let Some((cached, ok)) = &s.site_findings {
        report.findings.extend(cached.iter().cloned());
        report.site_ok = *ok;
        return;
    }
    let mut out = Vec::new();
    let mut ok = false;
    match &s.site {
        None => out.push(Finding::new(
            Warning,
            "site",
            "offline: vocabularies, existing collections and logins weren't checked",
        )),
        Some(site) => {
            let host = site.host().to_string();
            if let Err(e) = site.reachable() {
                out.push(Finding::new(Error, "site", format!("{e:#}. Check the internet connection.")));
            } else {
                match site.jsonapi_enabled() {
                    Ok(true) => ok = true,
                    Ok(false) => out.push(Finding::new(Error, "site", format!("{host}: {JSONAPI_HELP}"))),
                    Err(e) => out.push(Finding::new(Error, "site", format!("{e:#}"))),
                }
                match site.login() {
                    Login::Ok => out.push(Finding::new(Info, "site", format!("signed in to {host} ({})", s.site_name))),
                    Login::NoCredentials(f) => out.push(Finding::new(
                        Error,
                        "site",
                        format!("no credentials for {} - put {f} in the g/ folder (ask the SACDA admin)", s.site_name),
                    )),
                    Login::BadPassword => out.push(Finding::new(
                        Error,
                        "site",
                        format!("{host} didn't accept the username/password in {}", site.cfg.credentials),
                    )),
                    Login::Locked => out.push(Finding::new(
                        Error,
                        "site",
                        format!(
                            "{host} refused the login (403). If the password is right, the account is probably \
                             locked by Drupal's flood control after failed attempts - an admin can clear it with \
                             `drush sql:query \"DELETE FROM flood\"`, or wait an hour."
                        ),
                    )),
                }
            }
        }
    }
    // The new standard only ever creates archival_component collections.
    if ok {
        match s.vocabulary("archival_levels") {
            Ok(levels) if levels.iter().any(|t| t.name == "Collection") => {}
            Ok(_) => out.push(Finding::new(
                Error,
                "site",
                "this site has no 'Collection' archival level yet - it still uses the old collection \
                 content type. Run the collection conversion on it first.",
            )),
            Err(e) => out.push(Finding::new(Error, "site", format!("can't read archival_levels: {e}"))),
        }
    }
    s.site_findings = Some((out.clone(), ok));
    report.findings.extend(out);
    report.site_ok = ok;
}

pub fn columns(s: &mut Session, table: &Table, report: &mut Report) {
    for (from, to) in &table.renamed {
        report.findings.push(Finding::new(
            Info,
            "columns",
            format!("column '{from}' read as '{to}' - rename it in the sheet when you can"),
        ));
    }
    if !ID_COLUMNS.iter().any(|c| table.col(c).is_some()) {
        let fix = table
            .headers
            .iter()
            .find_map(|h| suggest::closest(h, ID_COLUMNS.iter().copied()).map(|to| (h.clone(), to)));
        let mut f = Finding::new(Error, "columns", "no accessIdentifier column, so nothing can be placed");
        if let Some((from, to)) = fix {
            f.message = format!("no accessIdentifier column - did you mean '{from}'?");
            f = f.with_fix(Fix::RenameColumn { from, to: to.to_string() }, false);
        }
        report.findings.push(f);
    }
    if table.col("title").is_none() {
        report.findings.push(Finding::new(Error, "columns", "no title column - every node needs a title"));
    }

    let item_fields = s.cfg.schema.fields("islandora_object");
    let comp_fields = s.cfg.schema.fields("archival_component");
    let known: Vec<&str> = RESERVED
        .iter()
        .copied()
        .chain(item_fields.keys().map(String::as_str))
        .chain(comp_fields.keys().map(String::as_str))
        .collect();
    for h in &table.headers {
        if h.is_empty() || known.contains(&h.as_str()) {
            continue;
        }
        let used = table.rows.iter().any(|r| !table.get(r, h).is_empty());
        match suggest::closest(h, known.iter().copied()) {
            Some(to) if table.col(to).is_none() => report.findings.push(
                Finding::new(
                    Warning,
                    "columns",
                    format!("unknown column '{h}' - did you mean '{to}'? It's left out until renamed."),
                )
                .with_fix(Fix::RenameColumn { from: h.clone(), to: to.to_string() }, false),
            ),
            _ => {
                report.dropped_columns.push(h.clone());
                if used {
                    report.findings.push(Finding::new(
                        Warning,
                        "columns",
                        format!("unknown column '{h}' isn't a field on the site - it's left out"),
                    ));
                }
            }
        }
    }
}

pub fn structure(s: &mut Session, table: &Table, report: &mut Report) {
    for (i, reason) in report.tree.rejected.clone() {
        let row = &table.rows[i];
        let id = report
            .tree
            .id_column
            .as_deref()
            .map(|c| table.get(row, c).to_string())
            .unwrap_or_default();
        report.findings.push(Finding::new(Error, "structure", reason).at(row.line, id));
    }

    // Roots: in the sheet, on the site, or neither.
    let roots: Vec<(String, bool, String)> = report
        .tree
        .roots()
        .iter()
        .map(|n| (n.id.clone(), n.row.is_some(), n.title.clone()))
        .collect();
    for (id, described, title) in roots {
        if described {
            if s.site.is_some() {
                if let Some(existing) = s.find_node(&id) {
                    report.findings.push(Finding::new(
                        Error,
                        "structure",
                        format!(
                            "collection {id} is already on the site (node {}, \"{}\"). Remove its row to add \
                             to it, or change the identifier.",
                            existing.nid, existing.title
                        ),
                    ));
                }
            }
            continue;
        }
        match s.find_node(&id) {
            Some(node) => {
                report.findings.push(Finding::new(
                    Info,
                    "structure",
                    format!("collection {id} isn't in the sheet - adding to the one on the site (node {}, \"{}\")", node.nid, node.title),
                ));
                report.attach.insert(id.clone(), node);
            }
            None if !title.is_empty() => report.findings.push(Finding::new(
                Info,
                "structure",
                format!("collection {id} isn't in the sheet or on the site - it will be created as \"{title}\""),
            )),
            None => {
                let mut f = Finding::new(
                    Error,
                    "structure",
                    format!("collection {id} isn't in the sheet or on the site - add a row for it, or give it a title"),
                );
                if let Some(t) = s.opts.collection_title.clone() {
                    f = Finding::new(Info, "structure", format!("collection {id} will be created as \"{t}\""))
                        .with_fix(Fix::SetTitle { id: id.clone(), title: t }, true);
                }
                report.findings.push(f);
            }
        }
    }

    // Components that are already on the site would be created twice.
    let attached_roots: Vec<String> = report.attach.keys().cloned().collect();
    let containers: Vec<(String, Option<usize>, String, Kind)> = report
        .tree
        .containers_in_order()
        .iter()
        .map(|n| (n.id.clone(), n.row, n.title.clone(), n.kind))
        .collect();
    for (id, row, title, kind) in containers {
        if kind == Kind::Component && attached_roots.iter().any(|r| id.starts_with(&format!("{r}_"))) {
            if let Some(existing) = s.find_node(&id) {
                report.findings.push(Finding::new(
                    Info,
                    "structure",
                    format!("{id} is already on the site (node {}) - new items go into it", existing.nid),
                ));
                report.attach.insert(id.clone(), existing);
                continue;
            }
        }
        if title.is_empty() && kind == Kind::Component {
            let line = row.map(|r| table.rows[r].line);
            let mut f = Finding::new(
                Warning,
                "structure",
                format!("component {id} has no title (no row for it and no fileTitle) - it will be titled \"{id}\""),
            )
            .with_fix(Fix::SetTitle { id: id.clone(), title: id.clone() }, true);
            if let Some(l) = line {
                f = f.at(l, id.clone());
            }
            report.findings.push(f);
        }
    }

    let items: Vec<(String, Option<usize>, String)> = report
        .tree
        .of_kind(Kind::Item)
        .map(|n| (n.id.clone(), n.row, n.title.clone()))
        .collect();
    for (id, row, title) in items {
        let line = row.map(|r| table.rows[r].line).unwrap_or(0);
        if title.is_empty() {
            report
                .findings
                .push(Finding::new(Error, "structure", "item has no title").at(line, id.clone()));
        }
        if s.site.is_some() && report.attach.keys().any(|r| id.starts_with(&format!("{r}_"))) {
            if let Some(existing) = s.find_node(&id) {
                report.findings.push(
                    Finding::new(
                        Error,
                        "structure",
                        format!("item is already on the site (node {}) - remove the row or change its identifier", existing.nid),
                    )
                    .at(line, id),
                );
            }
        }
    }
    for i in report.tree.extra_file_rows.clone() {
        let row = &table.rows[i];
        let id = report.tree.id_column.as_deref().map(|c| table.get(row, c)).unwrap_or("");
        report.findings.push(
            Finding::new(Info, "structure", "repeated identifier - this row's file is another part of the same item")
                .at(row.line, id),
        );
    }
}

/// Names that look like derivatives Islandora makes itself.
fn looks_like_derivative(name: &str) -> bool {
    let n = name.to_lowercase();
    ["_tn.", "-tn.", "thumbnail", "_service.", "servicefile", ".hocr", "_ocr.", "fits.xml"]
        .iter()
        .any(|m| n.contains(m))
}

pub fn files(s: &mut Session, table: &Table, report: &mut Report) {
    if !s.files.exists && report.tree.of_kind(Kind::Item).any(|n| !n.files.is_empty()) {
        report.findings.push(Finding::new(
            Error,
            "files",
            format!(
                "the access folder {} doesn't exist - stage the files there (or pass --access-dir)",
                s.files.root.display()
            ),
        ));
        return;
    }
    let mut out = Vec::new();
    for node in report.tree.of_kind(Kind::Item) {
        for f in &node.files {
            let line = table.rows[f.row].line;
            if f.path.is_none() {
                out.push(
                    Finding::new(Error, "files", format!("file '{}' isn't in the access folder", f.wanted))
                        .at(line, node.id.clone()),
                );
                continue;
            }
            if f.extension.is_empty() {
                out.push(
                    Finding::new(Error, "files", format!("file '{}' has no extension and no file_extension", f.wanted))
                        .at(line, node.id.clone()),
                );
            } else if media_for_extension(&s.cfg, &f.extension).is_none() {
                let all: Vec<&str> = s
                    .cfg
                    .schema
                    .media_types
                    .values()
                    .flat_map(|m| m.extensions.iter().map(String::as_str))
                    .collect();
                let hint = suggest::closest(&f.extension, all.iter().copied())
                    .map(|e| format!(" (did you mean .{e}?)"))
                    .unwrap_or_else(|| " - convert it (e.g. to tif, jpg, pdf, mp3 or mp4) first".into());
                out.push(
                    Finding::new(Error, "files", format!("the site doesn't accept .{} files{hint}", f.extension))
                        .at(line, node.id.clone()),
                );
            }
            if looks_like_derivative(&f.wanted) {
                out.push(
                    Finding::new(
                        Warning,
                        "files",
                        format!("'{}' looks like a derivative (thumbnail/service/OCR) - the site makes those itself", f.wanted),
                    )
                    .at(line, node.id.clone()),
                );
            }
        }
    }
    report.findings.extend(out);
}

pub fn dates(s: &mut Session, table: &Table, report: &mut Report) {
    let edtf_cols: Vec<String> = table
        .headers
        .iter()
        .filter(|h| {
            ["islandora_object", "archival_component"]
                .iter()
                .any(|b| s.cfg.schema.fields(b).get(h.as_str()).is_some_and(|f| f.kind == "edtf"))
        })
        .cloned()
        .collect();
    let id_col = report.tree.id_column.clone().unwrap_or_default();
    let subdelim = s.cfg.sacda.ingest.subdelimiter.clone();
    for col in &edtf_cols {
        for (i, row) in table.rows.iter().enumerate() {
            let v = table.get(row, col);
            if v.is_empty() {
                continue;
            }
            let parts: Vec<&str> = v.split(subdelim.as_str()).map(str::trim).collect();
            if parts.iter().all(|p| edtf::is_valid(p)) {
                continue;
            }
            let fixed: Option<Vec<String>> = parts
                .iter()
                .map(|p| if edtf::is_valid(p) { Some(p.to_string()) } else { edtf::normalize(p) })
                .collect();
            let id = table.get(row, &id_col);
            match fixed {
                Some(f) => {
                    let new = f.into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(&subdelim);
                    report.findings.push(
                        Finding::new(Warning, "dates", format!("{col} '{v}' isn't EDTF - using '{new}'"))
                            .at(row.line, id)
                            .with_fix(Fix::SetCell { row: i, column: col.clone(), value: new }, true),
                    );
                }
                None => report.findings.push(
                    Finding::new(
                        Error,
                        "dates",
                        format!("{col} '{v}' isn't an EDTF date (e.g. 1978, 1978-05, 1978-05-17, 1975/1982, 197X, 1978~)"),
                    )
                    .at(row.line, id),
                ),
            }
        }
    }
}
