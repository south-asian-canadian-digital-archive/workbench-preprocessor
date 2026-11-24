//! Resolves every taxonomy cell against the vocabularies on the site.
//!
//! Each value becomes a `Val`: an existing term, a new term (with its
//! authority link when one was found), a term ID, or blocked. New terms are
//! collected per vocabulary for the terms jobs, which run before the ingest -
//! so the ingest itself never creates a term.

use super::{Finding, Fix, Report, Session, Severity::*, TermHint};
use crate::authority::{self, Resolved};
use crate::config::FieldDef;
use crate::source::Table;
use crate::suggest;
use crate::tree::Kind;
use std::collections::BTreeMap;

/// Why a vocabulary takes no new terms, for the message and the screen.
pub fn locked_reason(vocab: &str) -> String {
    format!(
        "{vocab} is in protected_vocabularies ({}), so an ingest can't add terms to it - Workbench would stop on it too.",
        crate::config::PROTECTED_SOURCE
    )
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Val {
    Existing { vocab: String, name: String },
    New { vocab: String, name: String, link: Option<Resolved> },
    Tid(String),
    /// Couldn't be resolved; an error finding explains why.
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTerm {
    pub name: String,
    pub link: Option<Resolved>,
    /// (field, spreadsheet line) for every use.
    pub uses: Vec<(String, usize)>,
}

#[derive(Debug, Clone)]
struct Parsed {
    /// `relators:cre:person:` - whatever preceded the name.
    prefix: String,
    relator: Option<String>,
    vocab: Option<String>,
    name: String,
}

/// `relators:cre:person:Name`, `person:Name` or `Name`.
fn parse_value(raw: &str, def: &FieldDef) -> Parsed {
    let mut rest = raw.trim();
    let mut relator = None;
    if def.kind == "typed_relation" {
        if let Some(r) = rest.strip_prefix("relators:") {
            if let Some((code, tail)) = r.split_once(':') {
                relator = Some(format!("relators:{code}"));
                rest = tail;
            }
        }
    }
    let mut vocab = None;
    if let Some((v, tail)) = rest.split_once(':') {
        if def.targets.iter().any(|t| t == v) {
            vocab = Some(v.to_string());
            rest = tail;
        }
    }
    let t = raw.trim();
    let name = rest.trim().to_string();
    Parsed {
        prefix: t[..t.len() - rest.len()].to_string(),
        relator,
        vocab,
        name,
    }
}

/// The Workbench cell text for a resolved value.
pub fn render(val: &Val, def: &FieldDef, relator: Option<&str>) -> Option<String> {
    let (vocab, name) = match val {
        Val::Existing { vocab, name } | Val::New { vocab, name, .. } => (vocab, name),
        Val::Tid(t) => {
            return Some(match (def.kind.as_str(), relator) {
                ("typed_relation", Some(r)) => format!("{r}:{t}"),
                _ => t.clone(),
            })
        }
        Val::Blocked => return None,
    };
    let base = if def.targets.len() > 1 || def.kind == "typed_relation" {
        format!("{vocab}:{name}")
    } else {
        name.clone()
    };
    Some(match (def.kind.as_str(), relator) {
        ("typed_relation", Some(r)) => format!("{r}:{base}"),
        _ => base,
    })
}

pub fn check(s: &mut Session, table: &Table, report: &mut Report) {
    let (_, site_cfg) = match s.cfg.site(Some(&s.site_name)) {
        Ok(x) => x,
        Err(_) => return,
    };
    let id_col = report.tree.id_column.clone().unwrap_or_default();
    let subdelim = s.cfg.sacda.ingest.subdelimiter.clone();

    // Which bundle each row lands in decides which field definitions apply.
    let mut row_bundle: Vec<Option<&'static str>> = vec![None; table.rows.len()];
    // Rows of items already on the site: only the columns being updated count.
    let mut update_only = vec![false; table.rows.len()];
    for n in report.tree.nodes.values() {
        // Already on the site: the row isn't ingested, so its terms don't matter
        // - unless its fields are being updated from the sheet.
        let updating = !s.opts.update_columns.is_empty() && report.on_site_items.contains(&n.id);
        if (report.on_site_items.contains(&n.id) && !updating) || report.attach.contains_key(&n.id) {
            continue;
        }
        if let Some(r) = n.row {
            row_bundle[r] = Some(if n.kind == Kind::Item { "islandora_object" } else { "archival_component" });
            update_only[r] = updating;
        }
    }
    for &r in &report.tree.extra_file_rows {
        row_bundle[r] = None; // metadata comes from the item's first row
    }

    // Every value the loop below will look up, asked of the site in one go.
    let mut asked: Vec<(String, String)> = Vec::new();
    let mut uris: Vec<String> = Vec::new();
    for col in &table.headers {
        for (i, row) in table.rows.iter().enumerate() {
            let Some(bundle) = row_bundle[i] else { continue };
            if update_only[i] && !s.opts.update_columns.contains(col) {
                continue;
            }
            let Some(def) = s.cfg.schema.fields(bundle).get(col).cloned() else { continue };
            if !def.is_taxonomy() || col == "field_model" || col == "field_archival_level" || col == "field_language" {
                continue;
            }
            let auth = s.cfg.sacda.fields.get(col).and_then(|r| r.authority.as_deref()).and_then(authority::by_name);
            for (idx, raw) in table.get(row, col).split(subdelim.as_str()).map(str::trim).filter(|v| !v.is_empty()).enumerate() {
                let p = parse_value(raw, &def);
                if p.name.is_empty() {
                    continue;
                }
                let mut names = Vec::new();
                // An authority ID: the check uses its heading, or the sheet's beside it.
                if let Some(a) = auth.as_deref().filter(|a| a.is_identifier(&p.name)) {
                    if let Ok(Some(res)) = s.authorities.resolve(a, &p.name) {
                        names.push(res.label.clone());
                        uris.push(res.uri.clone());
                    }
                    // Any heading of the cell can end up paired with this ID.
                    for h in table.headings.get(&(row.line, col.clone())).into_iter().flatten() {
                        names.push(parse_value(h, &def).name);
                    }
                    let _ = idx;
                } else {
                    names.push(p.name.clone());
                }
                // A number may be a term ID: it's asked as it is.
                if names.is_empty() {
                    names.push(p.name.clone());
                }
                let targets = p.vocab.clone().map(|v| vec![v]).unwrap_or_else(|| def.targets.clone());
                for v in targets {
                    asked.extend(names.iter().map(|n| (v.clone(), n.clone())));
                }
            }
        }
    }
    s.prefetch_matches(asked, uris);

    let mut new_terms: BTreeMap<String, Vec<NewTerm>> = BTreeMap::new();

    for col in table.headers.clone() {
        for (i, row) in table.rows.iter().enumerate() {
            if s.job.cancelled() {
                return;
            }
            let Some(bundle) = row_bundle[i] else { continue };
            if update_only[i] && !s.opts.update_columns.contains(&col) {
                continue;
            }
            let Some(def) = s.cfg.schema.fields(bundle).get(&col).cloned() else { continue };
            if !def.is_taxonomy() || col == "field_model" || col == "field_archival_level" {
                continue;
            }
            let cell = table.get(row, &col).to_string();
            if cell.is_empty() {
                continue;
            }
            let id = table.get(row, &id_col).to_string();
            let rule = s.cfg.sacda.fields.get(&col).cloned().unwrap_or_default();
            if i % 25 == 0 {
                let asked = s.authorities.asked();
                s.job.say(format!(
                    "checking {col}, row {} of {}{}",
                    row.line,
                    table.rows.len() + 1,
                    if asked > 0 { format!(" - {asked} lookup(s) at id.loc.gov so far") } else { String::new() }
                ));
            }
            let mut vals = Vec::new();
            let mut rewritten: Vec<String> = Vec::new();
            let mut changed = false;
            // Which of the sheet's headings each ID in this cell goes with.
            let ids: Vec<String> = cell.split(subdelim.as_str()).map(str::trim).filter(|v| !v.is_empty()).map(|v| parse_value(v, &def).name).collect();
            let sheet_headings = table.headings.get(&(row.line, col.clone()));
            let pairing = pair_headings(s, sheet_headings, &ids, rule.authority.as_deref());

            for (idx, raw) in cell.split(subdelim.as_str()).map(str::trim).filter(|v| !v.is_empty()).enumerate() {
                let mut p = parse_value(raw, &def);
                let relator = p.relator.clone().or_else(|| rule.relator.clone());

                if col == "field_language" {
                    let v = resolve_language(s, &p.name, row.line, &id, report);
                    vals.push(v);
                    rewritten.push(raw.to_string());
                    continue;
                }
                if p.name.is_empty() {
                    report.findings.push(
                        Finding::new(Error, "terms", format!("{col}: \"{raw}\" has no name after its prefix"))
                            .at(row.line, id.clone()),
                    );
                    vals.push(Val::Blocked);
                    rewritten.push(raw.to_string());
                    continue;
                }
                // A number is a term ID only if a vocabulary the field takes has
                // that ID; otherwise it's a name ("1947" as a subject).
                if p.name.bytes().all(|b| b.is_ascii_digit()) {
                    let targets = p.vocab.clone().map(|v| vec![v]).unwrap_or_else(|| def.targets.clone());
                    let known = targets
                        .iter()
                        .any(|v| s.terms_for(v, &p.name).is_ok_and(|terms| terms.iter().any(|t| t.tid == p.name)));
                    if known {
                        vals.push(Val::Tid(p.name.clone()));
                        rewritten.push(raw.to_string());
                        continue;
                    }
                }

                // An authority identifier in the sheet: look up its heading.
                let auth = rule.authority.as_deref().and_then(authority::by_name);
                let mut link: Option<Resolved> = None;
                if let Some(a) = auth.as_deref() {
                    if a.is_identifier(&p.name) {
                        let typed_for_id = pairing.get(idx).copied().flatten().and_then(|h| sheet_headings?.get(h)).cloned();
                        let resolved = s.authorities.resolve(a, &p.name);
                        match resolved.as_ref().map(Option::clone).map_err(|e| anyhow::anyhow!("{e:#}")) {
                            Ok(Some(res)) => {
                                // The ID and the heading typed beside it (same position) should
                                // name the same thing. When they don't, the sheet's heading wins:
                                // a mistyped ID lands on another real heading.
                                let typed = typed_for_id.as_ref();
                                match typed {
                                    Some(h) if !suggest::headings_agree(h, &res.label) => {
                                        s.stick(
                                            Finding::new(
                                                Warning,
                                                "terms",
                                                format!(
                                                    "{col}: {} is \"{}\" at {}, which none of the sheet's headings match - skipped the ID and kept the sheet's heading \"{h}\"; correct or remove the ID",
                                                    p.name,
                                                    res.label,
                                                    a.name().to_uppercase()
                                                ),
                                            )
                                            .at(row.line, id.clone())
                                            .with_link(res.uri.clone()),
                                        );
                                        // The heading as a name (a typed vocabulary or relator prefix aside).
                                        p.name = parse_value(h, &def).name;
                                        changed = true;
                                    }
                                    _ => {
                                        report.findings.push(
                                            Finding::new(
                                                Info,
                                                "terms",
                                                format!("{col}: {} is {} \"{}\"", p.name, a.name().to_uppercase(), res.label),
                                            )
                                            .at(row.line, id.clone())
                                            .with_link(res.uri.clone()),
                                        );
                                        p.name = res.label.clone();
                                        link = Some(res);
                                        changed = true;
                                    }
                                }
                            }
                            Ok(None) | Err(_) => {
                                let why = match &resolved {
                                    Err(e) => format!("couldn't look up {} ({e:#})", p.name),
                                    _ => format!("id.loc.gov has no heading for {} (a typo, or a withdrawn ID?)", p.name),
                                };
                                // The written heading is kept; a bad ID is skipped.
                                match typed_for_id.clone() {
                                    Some(h) => {
                                        s.stick(
                                            Finding::new(Warning, "terms", format!("{col}: {why} - skipped the ID and kept the sheet's heading \"{h}\"; correct or remove the ID"))
                                                .at(row.line, id.clone())
                                                .with_link(format!("https://id.loc.gov/search/?q={}", crate::site::urlencode(&p.name))),
                                        );
                                        p.name = parse_value(&h, &def).name;
                                        changed = true;
                                    }
                                    None => {
                                        report.findings.push(
                                            Finding::new(Error, "terms", format!("{col}: {why} - use the current ID or the heading"))
                                                .at(row.line, id.clone())
                                                .with_link(format!("https://id.loc.gov/search/?q={}", crate::site::urlencode(&p.name))),
                                        );
                                        vals.push(Val::Blocked);
                                        rewritten.push(raw.to_string());
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }

                // Existing term, by name (any target vocabulary) or by authority URI.
                let targets: Vec<String> = match &p.vocab {
                    Some(v) => vec![v.clone()],
                    None => def.targets.clone(),
                };
                let mut found: Option<(String, String)> = None;
                let mut near: Option<(String, String, String)> = None;
                for vid in &targets {
                    let memo_key = (vid.clone(), p.name.clone());
                    let memo = s.term_memo.get(&memo_key).cloned();
                    let terms = match s.terms_for(vid, &p.name) {
                        Ok(t) => t,
                        Err(e) => {
                            report.findings.push(Finding::new(Error, "terms", format!("can't read vocabulary {vid}: {e}")));
                            continue;
                        }
                    };
                    let (hit, close) = match memo {
                        Some(m) => m,
                        None => {
                            let names = || terms.iter().map(|t| t.name.as_str());
                            let hit = suggest::exact_ci(&p.name, names())
                                .or_else(|| suggest::same_letters(&p.name, names()))
                                .map(String::from);
                            let close = if hit.is_none() { suggest::closest(&p.name, names()).map(String::from) } else { None };
                            (hit, close)
                        }
                    };
                    let by_uri = link.as_ref().and_then(|l| s.term_with_uri(vid, &l.uri, same_uri));
                    s.term_memo.insert(memo_key, (hit.clone(), close.clone()));
                    if let Some(name) = hit.or(by_uri) {
                        found = Some((vid.clone(), name));
                        break;
                    }
                    if near.is_none() {
                        if let Some(n) = close {
                            let tid = terms.iter().find(|t| t.name == n).map(|t| t.tid.clone()).unwrap_or_default();
                            near = Some((vid.clone(), n, tid));
                        }
                    }
                }

                if let Some((vocab, name)) = found {
                    if name != p.name {
                        changed = true;
                    }
                    let v = Val::Existing { vocab, name };
                    rewritten.push(render(&v, &def, relator.as_deref()).unwrap_or_default());
                    vals.push(v);
                    continue;
                }

                // Not on the site. Where would it go?
                let new_vocab = match &p.vocab {
                    Some(v) => v.clone(),
                    None => s
                        .cfg
                        .new_term_vocab(&col, &site_cfg)
                        .unwrap_or_else(|| def.targets.first().cloned().unwrap_or_default()),
                };
                // A new term has to go somewhere the field can point at, or the
                // item would link to nothing (or to a same-named term elsewhere).
                if !def.targets.is_empty() && !def.targets.contains(&new_vocab) {
                    report.findings.push(
                        Finding::new(
                            Error,
                            "terms",
                            format!(
                                "{col}: \"{}\" isn't on the site, and a new term would go to {new_vocab}, which {col} can't use \
                                 (it takes {}) - fix the vocabulary for this field in config/sacda.toml",
                                p.name,
                                def.targets.join(", ")
                            ),
                        )
                        .at(row.line, id.clone()),
                    );
                    vals.push(Val::Blocked);
                    rewritten.push(raw.to_string());
                    continue;
                }
                let locked = s.cfg.protected_vocabularies.contains(&new_vocab);
                let creatable = !locked;

                // A new term: find its authority link by heading if it has none yet.
                // A heading the authority knows is deliberate, so no "did you mean".
                if creatable && link.is_none() {
                    if let Some(a) = auth.as_deref() {
                        match s.authorities.resolve(a, &p.name) {
                            Ok(Some(res)) => {
                                if res.label != p.name {
                                    changed = true;
                                    p.name = res.label.clone();
                                }
                                link = Some(res);
                            }
                            Ok(None) => {}
                            Err(e) => report.findings.push(
                                Finding::new(Warning, "terms", format!("{col}: {} lookup failed for \"{}\": {e:#}", a.name(), p.name))
                                    .at(row.line, id.clone()),
                            ),
                        }
                    }
                }

                if let Some((near_vocab, near_name, tid)) = near.as_ref().filter(|_| link.is_none()) {
                    let tid = tid.clone();
                    let fixed_cell = replace_value(&cell, raw, &render_name(&p, near_name), &subdelim);
                    let why = suggest::why(&p.name, near_name);
                    let mut f = Finding::new(
                        if creatable { Warning } else { Error },
                        "terms",
                        format!(
                            "{col}: \"{}\" isn't on the site; existing {near_vocab} term \"{near_name}\" is close ({why}). {}",
                            p.name,
                            if creatable {
                                format!("Use \"{near_name}\", or keep \"{}\" as a new term.", p.name)
                            } else {
                                format!("Use \"{near_name}\" - {}", locked_reason(&new_vocab))
                            }
                        ),
                    )
                    .at(row.line, id.clone())
                    .with_fix(Fix::SetCell { row: i, column: col.clone(), value: fixed_cell }, false);
                    if let Some(u) = s.term_url(&tid).filter(|_| !tid.is_empty()) {
                        f = f.with_link(u);
                    }
                    f.hint = Some(TermHint {
                        column: col.clone(),
                        typed: p.name.clone(),
                        existing: near_name.clone(),
                        vocab: near_vocab.clone(),
                        tid,
                        why,
                        creatable,
                    });
                    report.findings.push(f);
                }

                if !creatable {
                    if near.is_none() {
                        report.findings.push(
                            Finding::new(
                                Error,
                                "terms",
                                format!("{col}: \"{}\" isn't in {new_vocab}. {}", p.name, locked_reason(&new_vocab)),
                            )
                            .at(row.line, id.clone()),
                        );
                    }
                    vals.push(Val::Blocked);
                    rewritten.push(raw.to_string());
                    continue;
                }

                let list = new_terms.entry(new_vocab.clone()).or_default();
                match list.iter_mut().find(|t| t.name.eq_ignore_ascii_case(&p.name)) {
                    Some(t) => t.uses.push((col.clone(), row.line)),
                    None => list.push(NewTerm {
                        name: p.name.clone(),
                        link: link.clone(),
                        uses: vec![(col.clone(), row.line)],
                    }),
                }
                let v = Val::New { vocab: new_vocab, name: p.name.clone(), link };
                rewritten.push(render(&v, &def, relator.as_deref()).unwrap_or_default());
                vals.push(v);
            }

            if changed {
                let value = rewritten.join(&subdelim);
                if value != cell {
                    report.findings.push(
                        Finding::new(Info, "terms", format!("{col}: \"{cell}\" -> \"{value}\""))
                            .at(row.line, id.clone())
                            .with_fix(Fix::SetCell { row: i, column: col.clone(), value }, true),
                    );
                }
            }
            report.values.insert((i, col.clone()), vals);
        }
    }

    // Workbench stops on a column the vocabulary doesn't have, so links only go
    // to vocabularies with an authority link field.
    for (vocab, terms) in &new_terms {
        let linked = terms.iter().filter(|t| t.link.is_some()).count();
        if linked == 0 {
            continue;
        }
        match s.term_has_field(vocab, "field_authority_link") {
            Ok(true) => {}
            Ok(false) => report.findings.push(Finding::new(
                Error,
                "terms",
                format!(
                    "{linked} new {vocab} term(s) have an authority link, but Workbench can't see field_authority_link on {vocab} on {} \
                     (it reads the vocabulary's form display, core.entity_form_display.taxonomy_term.{vocab}.default) - \
                     the site's config needs that form display; then press r",
                    s.site_name
                ),
            )),
            Err(e) => report.findings.push(Finding::new(
                Error,
                "terms",
                format!("couldn't read which fields {vocab} has, so its terms can't be written safely: {e} - press r to try again"),
            )),
        }
    }

    for (vocab, terms) in &new_terms {
        let linked = terms.iter().filter(|t| t.link.is_some()).count();
        report.findings.push(Finding::new(
            Info,
            "terms",
            format!("{} new {vocab} term(s), {linked} with an authority link - created by a terms job before the ingest", terms.len()),
        ));
        for t in terms.iter().filter(|t| t.link.is_none()) {
            let mut f = Finding::new(
                Info,
                "terms",
                format!("new {vocab} term \"{}\" has no authority link - it will be created with just a name", t.name),
            )
            .at(t.uses[0].1, "");
            // Somewhere to look for the heading by hand.
            let auth = s.cfg.sacda.fields.get(&t.uses[0].0).and_then(|r| r.authority.clone());
            if let Some(scheme) = auth.as_deref().and_then(|a| match a {
                "lcsh" => Some("subjects"),
                "lcnaf" => Some("names"),
                _ => None,
            }) {
                f = f.with_link(format!(
                    "https://id.loc.gov/search/?q={}&q=cs:http://id.loc.gov/authorities/{scheme}",
                    crate::site::urlencode(&t.name)
                ));
            }
            report.findings.push(f);
        }
    }
    report.new_terms = new_terms;
}

/// For a cell of authority IDs with the sheet's headings beside them: which
/// heading each ID goes with. An ID whose heading (at id.loc.gov) agrees with
/// one of the sheet's headings is paired with it wherever it's listed - the
/// two columns needn't be in the same order; the same position wins a tie.
/// IDs that agree with none take the headings left over, in order (and the
/// check then uses that heading, not the ID). None: no heading for it.
fn pair_headings(s: &mut Session, headings: Option<&Vec<String>>, ids: &[String], authority: Option<&str>) -> Vec<Option<usize>> {
    let Some(headings) = headings else { return vec![None; ids.len()] };
    let Some(a) = authority.and_then(authority::by_name) else { return vec![None; ids.len()] };
    // agrees[i][j]: ID i's heading agrees with sheet heading j.
    let agrees: Vec<Vec<bool>> = ids
        .iter()
        .map(|id| {
            let label = if a.is_identifier(id) { s.authorities.resolve(a.as_ref(), id).ok().flatten().map(|r| r.label) } else { None };
            headings.iter().map(|h| label.as_deref().is_some_and(|l| suggest::headings_agree(h, l))).collect()
        })
        .collect();
    let matched = match_by_agreement(&agrees, headings.len());
    // Left-over headings go to left-over IDs, in order.
    let used: std::collections::HashSet<usize> = matched.iter().flatten().copied().collect();
    let mut spare = (0..headings.len()).filter(|j| !used.contains(j));
    matched.into_iter().map(|m| m.or_else(|| spare.next())).collect()
}

/// The most IDs paired with headings they agree with, one heading each
/// (Kuhn's augmenting paths - a cell holds a handful). Same positions are
/// tried first, so a cell in order keeps its order.
fn match_by_agreement(agrees: &[Vec<bool>], headings: usize) -> Vec<Option<usize>> {
    fn try_assign(i: usize, agrees: &[Vec<bool>], owner: &mut [Option<usize>], seen: &mut [bool]) -> bool {
        let n = owner.len();
        // Own position first, then the rest.
        let order = std::iter::once(i).filter(|j| *j < n).chain((0..n).filter(move |j| *j != i));
        for j in order {
            if agrees[i].get(j).copied().unwrap_or(false) && !seen[j] {
                seen[j] = true;
                if owner[j].is_none_or(|k| try_assign(k, agrees, owner, seen)) {
                    owner[j] = Some(i);
                    return true;
                }
            }
        }
        false
    }
    let mut owner: Vec<Option<usize>> = vec![None; headings];
    for i in 0..agrees.len() {
        let mut seen = vec![false; headings];
        try_assign(i, agrees, &mut owner, &mut seen);
    }
    let mut out = vec![None; agrees.len()];
    for (j, o) in owner.iter().enumerate() {
        if let Some(i) = o {
            out[*i] = Some(j);
        }
    }
    out
}

fn same_uri(a: &str, b: &str) -> bool {
    let norm = |u: &str| u.trim().trim_end_matches('/').replace("https://", "http://");
    norm(a) == norm(b)
}

fn render_name(p: &Parsed, name: &str) -> String {
    format!("{}{}", p.prefix, name)
}

fn replace_value(cell: &str, old: &str, new: &str, subdelim: &str) -> String {
    cell.split(subdelim)
        .map(|v| if v.trim() == old { new.to_string() } else { v.trim().to_string() })
        .collect::<Vec<_>>()
        .join(subdelim)
}

fn resolve_language(s: &mut Session, value: &str, line: usize, id: &str, report: &mut Report) -> Val {
    let codes = match s.language_codes() {
        Ok(c) => c.clone(),
        Err(e) => {
            report.findings.push(Finding::new(Error, "terms", format!("can't read the language codes: {e}")));
            return Val::Blocked;
        }
    };
    if let Some(tid) = codes.get(&value.to_lowercase()) {
        return Val::Tid(tid.clone());
    }
    // A language name instead of a code.
    if let Ok(terms) = s.vocabulary("language") {
        if let Some(name) = suggest::exact_ci(value, terms.iter().map(|t| t.name.as_str())) {
            return Val::Existing { vocab: "language".into(), name: name.to_string() };
        }
    }
    let names: Vec<String> = s
        .vocabulary("language")
        .map(|t| t.iter().map(|t| t.name.clone()).collect())
        .unwrap_or_default();
    let hint = suggest::closest(value, codes.keys().map(String::as_str))
        .or_else(|| suggest::closest(value, names.iter().map(String::as_str)))
        .map(|c| format!(" - did you mean '{c}'?"))
        .unwrap_or_default();
    report.findings.push(
        Finding::new(Error, "terms", format!("field_language: '{value}' isn't a language code on the site{hint}"))
            .at(line, id),
    );
    Val::Blocked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(kind: &str, targets: &[&str]) -> FieldDef {
        FieldDef {
            label: String::new(),
            kind: kind.into(),
            required: false,
            cardinality: -1,
            target_type: "taxonomy_term".into(),
            targets: targets.iter().map(|s| s.to_string()).collect(),
            allowed_values: vec![],
        }
    }

    #[test]
    fn ids_pair_with_the_heading_they_agree_with() {
        // In order: each with its own.
        let a = vec![vec![true, false], vec![false, true]];
        assert_eq!(match_by_agreement(&a, 2), vec![Some(0), Some(1)]);
        // Listed in another order: found anyway.
        let a = vec![vec![false, false, true], vec![true, false, false], vec![false, true, false]];
        assert_eq!(match_by_agreement(&a, 3), vec![Some(2), Some(0), Some(1)]);
        // Both agree with heading 0; ID 1 also with 1: everyone gets one.
        let a = vec![vec![true, false], vec![true, true]];
        assert_eq!(match_by_agreement(&a, 2), vec![Some(0), Some(1)]);
        // An ID that agrees with nothing stays unpaired here.
        let a = vec![vec![false, false], vec![false, true]];
        assert_eq!(match_by_agreement(&a, 2), vec![None, Some(1)]);
    }

    #[test]
    fn parses_and_renders_typed_relations() {
        let d = def("typed_relation", &["person", "corporate_body"]);
        let p = parse_value("relators:aut:person:Jane Doe", &d);
        assert_eq!(p.relator.as_deref(), Some("relators:aut"));
        assert_eq!(p.vocab.as_deref(), Some("person"));
        assert_eq!(p.name, "Jane Doe");
        let v = Val::Existing { vocab: "person".into(), name: "Jane Doe".into() };
        assert_eq!(render(&v, &d, Some("relators:cre")).unwrap(), "relators:cre:person:Jane Doe");
    }

    #[test]
    fn single_target_renders_bare_name() {
        let d = def("entity_reference", &["subject"]);
        let v = Val::New { vocab: "subject".into(), name: "Anniversaries".into(), link: None };
        assert_eq!(render(&v, &d, None).unwrap(), "Anniversaries");
        let multi = def("entity_reference", &["sacda_thesaurus", "subject"]);
        assert_eq!(render(&v, &multi, None).unwrap(), "subject:Anniversaries");
    }
}
