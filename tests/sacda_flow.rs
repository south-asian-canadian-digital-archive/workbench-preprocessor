//! End-to-end checks of the new flow over g/data/examples, offline (no site
//! lookups), so they run anywhere the g/ checkout is present.

use organise::sacda::bundle;
use organise::sacda::config::Config;
use organise::sacda::precheck::{Options, Report, Session, Severity};
use organise::sacda::source::{self, Table};
use organise::sacda::tree::Kind;
use std::path::{Path, PathBuf};

fn g() -> Option<PathBuf> {
    let g = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    g.join("data/examples/full-collection/sheet.csv").is_file().then_some(g)
}

fn check(sheet: &str, title: Option<&str>) -> Option<(Config, Table, Report, Vec<organise::sacda::precheck::Fix>)> {
    let g = g()?;
    let cfg = Config::load_from(&g).unwrap();
    let input = g.join("data/examples").join(sheet);
    let input = input.to_str().unwrap().to_string();
    let table = source::load(&input, None, &cfg.sacda.aliases).unwrap();
    let mut s = Session::new(
        cfg.clone(),
        &input,
        Options {
            site: Some("dev".into()),
            access_dir: None,
            collection_title: title.map(String::from),
            offline: true,
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
    for needle in [".heic", "nowhere.jpg", "no title", "spaces or odd characters", "isn't an EDTF date"] {
        assert!(e.contains(needle), "missing '{needle}' in:\n{e}");
    }
    assert!(report.blocked());
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
