//! End-to-end checks of the new flow over g/data/examples, offline (no site
//! lookups), so they run anywhere the g/ checkout is present.

use organise::sacda::bundle;
use organise::sacda::config::Config;
use organise::sacda::precheck::{Fix, Gap, Options, Report, Session, Severity};
use organise::sacda::source::{self, Table};
use organise::sacda::tree::Kind;
use std::path::{Path, PathBuf};

fn g() -> Option<PathBuf> {
    let g = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    g.join("data/examples/full-collection/sheet.csv").is_file().then_some(g)
}

fn check(sheet: &str, title: Option<&str>) -> Option<(Config, Table, Report, Vec<Fix>)> {
    let g = g()?;
    let cfg = Config::load_from(&g).unwrap();
    let input = g.join("data/examples").join(sheet);
    let input = input.to_str().unwrap().to_string();
    let table = source::load_for(&input, None, &cfg).unwrap();
    let mut s = Session::new(
        cfg.clone(),
        &input,
        Options {
            site: Some("dev".into()),
            access_dir: None,
            collection_title: title.map(String::from),
            offline: true,
            ..Default::default()
        },
    )
    .unwrap();
    let (fixed, report, applied) = s.settle(&table, &[], &[]);
    Some((cfg, fixed, report, applied))
}

fn errors(r: &Report) -> Vec<String> {
    r.findings
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .map(|f| f.message.clone())
        .collect()
}

#[test]
fn full_collection_builds_a_clean_bundle() {
    let Some((cfg, table, report, applied)) = check("full-collection/sheet.csv", None) else { return };
    assert!(errors(&report).is_empty(), "{:?}", errors(&report));
    let t = &report.tree;
    assert_eq!(t.nodes["2099_01"].kind, Kind::Collection);
    assert_eq!(t.nodes["2099_01_01"].level, "Series");
    assert_eq!(t.nodes["2099_01_01_01"].title, "TEST File one");
    assert_eq!(t.nodes["2099_01_02_001"].model, "Compound Object");
    assert_eq!(t.nodes["2099_01_01_01_004"].model, "Audio");
    assert!(applied.iter().any(|f| f.describe().contains("1978-05")));

    let dir = tempfile::tempdir().unwrap();
    let w = bundle::write(&cfg, "dev", &table, &report, &applied, dir.path()).unwrap();
    assert_eq!(w.manifest.components, 4);
    assert_eq!(w.manifest.items, 6);
    let comps = std::fs::read_to_string(dir.path().join("components.csv")).unwrap();
    let ids: Vec<&str> = comps.lines().skip(1).map(|l| l.split(',').next().unwrap()).collect();
    // Parents always before children.
    for (i, id) in ids.iter().enumerate() {
        if let Some((p, _)) = id.rsplit_once('_') {
            if let Some(pi) = ids.iter().position(|x| *x == p) {
                assert!(pi < i, "{p} must come before {id}");
            }
        }
    }
    let items = std::fs::read_to_string(dir.path().join("items.csv")).unwrap();
    assert!(items.contains("2099_01_02_001-03,2099_01_02_001,"));
    assert!(items.contains(",1978-05,"));
    let cfg_text = std::fs::read_to_string(dir.path().join("components.config.yml")).unwrap();
    assert!(cfg_text.contains("content_type: archival_component"));
    assert!(cfg_text.contains("secondary_tasks:"));
    assert!(cfg_text.contains("https://isle.dvnl.work"));
}

#[test]
fn blocked_sheet_reports_every_problem() {
    let Some((_, _, report, _)) = check("blocked/sheet.csv", None) else { return };
    let e = errors(&report).join("\n");
    for needle in [".heic", "no title", "spaces or odd characters", "isn't an EDTF date"] {
        assert!(e.contains(needle), "missing '{needle}' in:\n{e}");
    }
    assert!(report.blocked());
    // A missing file is a decision (leave the row out, or ingest without it), not an error.
    let gap = report
        .findings
        .iter()
        .find(|f| f.fix == Some(Fix::FileGap(Gap::MissingFile)))
        .expect("a missing-file decision");
    assert!(gap.message.contains("nowhere.jpg"), "{}", gap.message);
}

/// Copy the full-collection example with one file gone and one extra, so
/// every way rows and files can fail to pair up shows.
fn unpaired() -> Option<(Config, Table, Report, Vec<Fix>, tempfile::TempDir)> {
    let g = g()?;
    let cfg = Config::load_from(&g).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let src = g.join("data/examples/full-collection");
    std::fs::copy(src.join("sheet.csv"), tmp.path().join("sheet.csv")).unwrap();
    let access = tmp.path().join("access");
    let mut stack = vec![(src.join("access"), access.clone())];
    while let Some((from, to)) = stack.pop() {
        std::fs::create_dir_all(&to).unwrap();
        for e in std::fs::read_dir(&from).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push((p.clone(), to.join(e.file_name())));
            } else if !p.to_string_lossy().contains("photo_001") {
                std::fs::copy(&p, to.join(e.file_name())).unwrap();
            }
        }
    }
    std::fs::write(access.join("2099_01_01_01_009.jpg"), b"x").unwrap();
    let input = tmp.path().join("sheet.csv").to_string_lossy().to_string();
    let table = source::load_for(&input, None, &cfg).unwrap();
    let mut s = Session::new(cfg.clone(), &input, Options { site: Some("dev".into()), offline: true, ..Default::default() }).unwrap();
    let (fixed, report, applied) = s.settle(&table, &[], &[]);
    Some((cfg, fixed, report, applied, tmp))
}

#[test]
fn unpaired_rows_and_files_are_decisions() {
    let Some((cfg, table, report, applied, _tmp)) = unpaired() else { return };
    let gaps: Vec<Gap> = report
        .findings
        .iter()
        .filter_map(|f| match f.fix {
            Some(Fix::FileGap(g)) => Some(g),
            _ => None,
        })
        .collect();
    assert!(gaps.contains(&Gap::MissingFile), "{gaps:?}");
    assert!(gaps.contains(&Gap::Unmatched), "{gaps:?}");
    let missing = report.findings.iter().find(|f| f.fix == Some(Fix::FileGap(Gap::MissingFile))).unwrap();
    assert!(missing.message.contains("2099_01_01_01_001"), "{}", missing.message);
    assert_eq!(report.stub_items, vec![("2099_01_01_01_009".to_string(), "2099_01_01_01".to_string())]);

    // Left out: listed for later, not in items.csv.
    let mut skip = applied.clone();
    skip.extend([Fix::FileGap(Gap::MissingFile), Fix::FileGap(Gap::Unmatched)]);
    let out = tempfile::tempdir().unwrap();
    let w = bundle::write(&cfg, "dev", &table, &report, &skip, out.path()).unwrap();
    assert_eq!(w.manifest.items, 5);
    assert_eq!(w.manifest.skipped_rows, 1);
    assert_eq!(w.manifest.unmatched_files, 1);
    let skipped = std::fs::read_to_string(out.path().join("skipped-rows.csv")).unwrap();
    assert!(skipped.contains("2099_01_01_01_001") && skipped.contains("photo_001"), "{skipped}");
    assert!(std::fs::read_to_string(out.path().join("unmatched-files.csv")).unwrap().contains("2099_01_01_01_009.jpg"));
    assert!(!std::fs::read_to_string(out.path().join("items.csv")).unwrap().contains("2099_01_01_01_001,"));

    // Ingested anyway: the row without its file, the file as its own item.
    let out = tempfile::tempdir().unwrap();
    let w = bundle::write(&cfg, "dev", &table, &report, &applied, out.path()).unwrap();
    assert_eq!(w.manifest.items, 7);
    assert_eq!(w.manifest.skipped_rows, 0);
    let items = std::fs::read_to_string(out.path().join("items.csv")).unwrap();
    assert!(items.contains("2099_01_01_01_009,2099_01_01_01,,2099_01_01_01_009,2099_01_01_01_009,Image,"), "{items}");
    assert!(!out.path().join("skipped-rows.csv").exists());
}

#[test]
fn deep_nesting_needs_a_collection_title() {
    let Some((_, _, report, _)) = check("deep-nesting/sheet.xlsx", None) else { return };
    assert!(errors(&report).iter().any(|e| e.contains("2099_04")));

    let Some((_, _, report, _)) = check("deep-nesting/sheet.xlsx", Some("Deep")) else { return };
    assert!(errors(&report).is_empty(), "{:?}", errors(&report));
    let t = &report.tree;
    assert_eq!(t.nodes["2099_04"].title, "Deep");
    assert_eq!(t.nodes["2099_04_01_01"].level, "Sub-series");
    assert_eq!(t.nodes["2099_04_01_01_01"].level, "File");
    assert_eq!(t.nodes["2099_04_02"].level, "File");
}

#[test]
fn ocr_column_takes_always_or_never() {
    let Some(g) = g() else { return };
    let cfg = Config::load_from(&g).unwrap();
    let def = &cfg.schema.fields("islandora_object")["field_ocr"];
    assert_eq!(def.allowed_values, ["always", "never"]);
    // The thesaurus field points at the SACDA thesaurus only.
    assert_eq!(cfg.schema.fields("islandora_object")["field_sacda_thesaurus"].targets, ["sacda_thesaurus"]);

    let tmp = tempfile::tempdir().unwrap();
    let src = std::fs::read_to_string(g.join("data/examples/full-collection/sheet.csv")).unwrap();
    let mut lines: Vec<String> = src.lines().map(String::from).collect();
    lines[0].push_str(",OCR");
    for (i, l) in lines.iter_mut().enumerate().skip(1) {
        l.push_str(match i {
            3 => ",Always",
            4 => ",sometimes",
            _ => ",",
        });
    }
    std::fs::write(tmp.path().join("sheet.csv"), lines.join("\n")).unwrap();
    let input = tmp.path().join("sheet.csv").to_string_lossy().to_string();
    let table = source::load_for(&input, None, &cfg).unwrap();
    assert!(table.col("field_ocr").is_some(), "OCR header maps to field_ocr");
    let mut s = Session::new(cfg.clone(), &input, Options { site: Some("dev".into()), offline: true, ..Default::default() }).unwrap();
    let (fixed, report, _) = s.settle(&table, &[], &[]);
    assert!(errors(&report).iter().any(|e| e.contains("\"sometimes\" isn't one of OCR")), "{:?}", errors(&report));
    assert_eq!(fixed.get(&fixed.rows[2], "field_ocr"), "always");
}

#[test]
fn new_terms_go_where_their_field_can_point_on_every_site() {
    let Some(g) = g() else { return };
    let cfg = Config::load_from(&g).unwrap();
    for (site_name, site) in &cfg.sacda.sites {
        for field in cfg.sacda.fields.keys() {
            let Some(vocab) = cfg.new_term_vocab(field, site) else { continue };
            for bundle in ["islandora_object", "archival_component"] {
                if let Some(def) = cfg.schema.fields(bundle).get(field) {
                    if def.targets.is_empty() {
                        continue;
                    }
                    assert!(
                        def.targets.contains(&vocab),
                        "on {site_name}, a new {field} term would go to {vocab}, but {bundle}.{field} takes {:?}",
                        def.targets
                    );
                }
            }
        }
    }
}
