//! Builds the collection -> component -> item tree from identifiers alone.
//!
//! SACDA identifiers nest by underscore: `2024_19` is a collection,
//! `2024_19_01` a component inside it (any depth: `2024_19_01_02`...), and the
//! deepest identifiers are items. A row whose identifier ends in `_00`/`_000`
//! describes the container above it, as does a row whose identifier is a
//! prefix of other rows' identifiers. Everything else is an item; several rows
//! with the same item identifier are one item with several files.

use super::config::{Config, IngestCfg};
use super::source::Table;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

/// Segments in a collection identifier (`2024_19`).
pub const COLLECTION_SEGMENTS: usize = 2;

pub const ID_COLUMNS: &[&str] = &["accessIdentifier", "field_identifier", "id"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Collection,
    Component,
    Item,
}

#[derive(Debug, Clone)]
pub struct FileRef {
    /// Row the file came from.
    pub row: usize,
    /// As written in the sheet (plus `file_extension` if it had none).
    pub wanted: String,
    pub extension: String,
    /// Where it is on disk, if found.
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub parent: Option<String>,
    pub kind: Kind,
    /// Index into `Table::rows` of the row that describes this node, if any.
    pub row: Option<usize>,
    pub title: String,
    pub level: String,
    pub model: String,
    pub files: Vec<FileRef>,
}

#[derive(Debug, Default)]
pub struct Tree {
    pub nodes: BTreeMap<String, Node>,
    /// Identifier column used, if one was found.
    pub id_column: Option<String>,
    /// Rows that couldn't be placed: (row index, reason).
    pub rejected: Vec<(usize, String)>,
    /// Rows folded into an item as an extra file.
    pub extra_file_rows: Vec<usize>,
}

impl Tree {
    pub fn roots(&self) -> Vec<&Node> {
        self.nodes.values().filter(|n| n.parent.is_none()).collect()
    }

    pub fn children(&self, id: &str) -> Vec<&Node> {
        self.nodes
            .values()
            .filter(|n| n.parent.as_deref() == Some(id))
            .collect()
    }

    pub fn of_kind(&self, kind: Kind) -> impl Iterator<Item = &Node> {
        self.nodes.values().filter(move |n| n.kind == kind)
    }

    /// Containers parent-first, so Workbench always sees a parent before its
    /// children in components.csv.
    pub fn containers_in_order(&self) -> Vec<&Node> {
        let mut v: Vec<&Node> = self.nodes.values().filter(|n| n.kind != Kind::Item).collect();
        v.sort_by(|a, b| depth(&a.id).cmp(&depth(&b.id)).then(a.id.cmp(&b.id)));
        v
    }
}

pub fn depth(id: &str) -> usize {
    id.split('_').count()
}

pub fn parent_of(id: &str) -> Option<String> {
    id.rsplit_once('_').map(|(p, _)| p.to_string())
}

/// `2024_19_01_00` / `_000` -> `2024_19_01`.
fn container_target(id: &str) -> Option<String> {
    let (head, last) = id.rsplit_once('_')?;
    (last.len() >= 2 && last.bytes().all(|b| b == b'0')).then(|| head.to_string())
}

pub fn valid_identifier(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        && !id.starts_with('_')
        && !id.ends_with('_')
        && !id.contains("__")
}

/// Every file under the access folder, by lowercased name and by lowercased stem.
pub struct FileIndex {
    by_name: HashMap<String, Vec<PathBuf>>,
    by_stem: HashMap<String, Vec<PathBuf>>,
    pub root: PathBuf,
    pub exists: bool,
}

impl FileIndex {
    pub fn build(root: &Path) -> Self {
        let mut by_name: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let mut by_stem: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let exists = root.is_dir();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase());
                if name.as_deref().is_some_and(|n| n.starts_with('.')) {
                    continue;
                }
                let abs = fs::canonicalize(&p).unwrap_or(p.clone());
                if let Some(n) = name {
                    by_name.entry(n).or_default().push(abs.clone());
                }
                if let Some(s) = p.file_stem() {
                    by_stem.entry(s.to_string_lossy().to_lowercase()).or_default().push(abs);
                }
            }
        }
        Self {
            by_name,
            by_stem,
            root: root.to_path_buf(),
            exists,
        }
    }

    /// Find a file the sheet names. Paths are reduced to their file name -
    /// staged folders rarely match the sheet's idea of the layout. When two
    /// files share the name, prefer the one under a folder named `scope`.
    pub fn find(&self, wanted: &str, scope: &str) -> Option<PathBuf> {
        let base = wanted.rsplit(['/', '\\']).next().unwrap_or(wanted).to_lowercase();
        let pick = |hits: &Vec<PathBuf>| {
            hits.iter()
                .find(|p| p.components().any(|c| c.as_os_str().to_string_lossy() == scope))
                .or_else(|| hits.first())
                .cloned()
        };
        if let Some(h) = self.by_name.get(&base) {
            return pick(h);
        }
        let stem = Path::new(&base)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or(base.clone());
        self.by_stem.get(&stem).and_then(pick)
    }
}

/// Media type order when an extension is accepted by several (tif is both an
/// image and a generic file - it should be an Image).
const MEDIA_PRIORITY: &[(&str, &str)] = &[
    ("image", "Image"),
    ("audio", "Audio"),
    ("video", "Video"),
    ("document", "Digital Document"),
    ("file", "Binary"),
];

/// The media type and model for a file extension, or None if no media type
/// on the site accepts it.
pub fn media_for_extension(cfg: &Config, ext: &str) -> Option<(&'static str, &'static str)> {
    let ext = ext.trim_start_matches('.').to_lowercase();
    MEDIA_PRIORITY.iter().copied().find(|(media, _)| {
        cfg.schema
            .media_types
            .get(*media)
            .is_some_and(|m| m.extensions.iter().any(|e| e == &ext))
    })
}

/// Models that describe structure rather than a file. A sheet that sets one
/// explicitly is kept.
pub const STRUCTURAL_MODELS: &[&str] = &["Compound Object", "Paged Content", "Publication Issue", "Newspaper"];

fn extension_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub fn build(table: &Table, cfg: &Config, files: &FileIndex) -> Tree {
    let ingest: &IngestCfg = &cfg.sacda.ingest;
    let mut tree = Tree::default();
    let Some(id_col) = ID_COLUMNS.iter().find(|c| table.col(c).is_some()) else {
        return tree;
    };
    tree.id_column = Some(id_col.to_string());

    // Pass 1: every row's effective identifier.
    let mut ids: Vec<Option<String>> = Vec::with_capacity(table.rows.len());
    for (i, row) in table.rows.iter().enumerate() {
        let raw = table.get(row, id_col);
        if raw.is_empty() {
            tree.rejected.push((i, format!("no {id_col}")));
            ids.push(None);
            continue;
        }
        if !valid_identifier(raw) {
            tree.rejected.push((i, format!("identifier '{raw}' has spaces or odd characters")));
            ids.push(None);
            continue;
        }
        let id = container_target(raw).unwrap_or_else(|| raw.to_string());
        if depth(&id) < COLLECTION_SEGMENTS {
            tree.rejected.push((i, format!("identifier '{raw}' is shorter than a collection identifier (like 2024_19)")));
            ids.push(None);
            continue;
        }
        ids.push(Some(id));
    }

    // A row is a container if it's a `_00` row, sits at collection depth, or
    // some other row's identifier extends it.
    let all: Vec<&String> = ids.iter().flatten().collect();
    let is_prefix_of_other = |id: &str| {
        let p = format!("{id}_");
        all.iter().any(|o| o.starts_with(&p))
    };

    // Pass 2: place rows.
    for (i, row) in table.rows.iter().enumerate() {
        let Some(id) = ids[i].clone() else { continue };
        let raw = table.get(row, id_col);
        let container = raw != id || depth(&id) == COLLECTION_SEGMENTS || is_prefix_of_other(&id);
        let title = table.get(row, "title").to_string();

        if container {
            let kind = if depth(&id) == COLLECTION_SEGMENTS { Kind::Collection } else { Kind::Component };
            if let Some(existing) = tree.nodes.get(&id) {
                if existing.row.is_some() {
                    tree.rejected.push((i, format!("a second row describes container {id}")));
                    continue;
                }
            }
            tree.nodes.insert(
                id.clone(),
                Node {
                    parent: (kind != Kind::Collection).then(|| parent_of(&id)).flatten(),
                    id,
                    kind,
                    row: Some(i),
                    title,
                    level: String::new(),
                    model: "Collection".into(),
                    files: Vec::new(),
                },
            );
            continue;
        }

        // Item row (or another file of an item already seen).
        let file_cell = table.get(row, "file");
        let ext_cell = table.get(row, "file_extension").trim_start_matches('.').to_lowercase();
        let file = (!file_cell.is_empty()).then(|| {
            let has_ext = !extension_of(file_cell).is_empty();
            let wanted = if has_ext || ext_cell.is_empty() {
                file_cell.to_string()
            } else {
                format!("{file_cell}.{ext_cell}")
            };
            let extension = if has_ext { extension_of(file_cell) } else { ext_cell.clone() };
            let scope = parent_of(&id).unwrap_or_default();
            FileRef {
                row: i,
                path: files.find(&wanted, &scope),
                wanted,
                extension,
            }
        });

        if let Some(item) = tree.nodes.get_mut(&id) {
            if item.kind == Kind::Item {
                if let Some(f) = file {
                    item.files.push(f);
                }
                tree.extra_file_rows.push(i);
                continue;
            }
        }
        tree.nodes.insert(
            id.clone(),
            Node {
                parent: parent_of(&id),
                id,
                kind: Kind::Item,
                row: Some(i),
                title,
                level: String::new(),
                model: String::new(),
                files: file.into_iter().collect(),
            },
        );
    }

    // Pass 3: create the containers nobody described, up to the collection.
    let item_ids: Vec<String> = tree.nodes.keys().cloned().collect();
    for id in item_ids {
        let mut cur = parent_of(&id);
        while let Some(p) = cur {
            if depth(&p) < COLLECTION_SEGMENTS {
                break;
            }
            if tree.nodes.contains_key(&p) {
                cur = parent_of(&p);
                continue;
            }
            let kind = if depth(&p) == COLLECTION_SEGMENTS { Kind::Collection } else { Kind::Component };
            tree.nodes.insert(
                p.clone(),
                Node {
                    parent: (kind != Kind::Collection).then(|| parent_of(&p)).flatten(),
                    id: p.clone(),
                    kind,
                    row: None,
                    title: String::new(),
                    level: String::new(),
                    model: "Collection".into(),
                    files: Vec::new(),
                },
            );
            cur = parent_of(&p);
        }
    }

    // Untitled components take the legacy `fileTitle` of their first item.
    let file_titles: HashMap<String, String> = tree
        .nodes
        .values()
        .filter(|n| n.kind == Kind::Item)
        .filter_map(|n| {
            let row = &table.rows[n.row?];
            let ft = table.get(row, "fileTitle");
            let parent = n.parent.clone()?;
            (!ft.is_empty()).then(|| (parent, ft.to_string()))
        })
        .collect();

    // Levels and models.
    let has_item_child: HashMap<String, bool> = tree
        .nodes
        .values()
        .filter(|n| n.kind == Kind::Item)
        .filter_map(|n| n.parent.clone().map(|p| (p, true)))
        .collect();
    for node in tree.nodes.values_mut() {
        match node.kind {
            Kind::Collection | Kind::Component => {
                if node.title.is_empty() {
                    if let Some(t) = file_titles.get(&node.id) {
                        node.title = t.clone();
                    }
                }
                let explicit = node
                    .row
                    .map(|r| table.get(&table.rows[r], "field_archival_level"))
                    .unwrap_or("");
                node.level = if !explicit.is_empty() {
                    explicit.to_string()
                } else if node.kind == Kind::Collection {
                    "Collection".into()
                } else if has_item_child.contains_key(&node.id) {
                    "File".into()
                } else {
                    ingest.intermediate_level.clone()
                };
            }
            Kind::Item => {
                let explicit = node
                    .row
                    .map(|r| table.get(&table.rows[r], "field_model"))
                    .unwrap_or("");
                node.model = if STRUCTURAL_MODELS.iter().any(|m| m.eq_ignore_ascii_case(explicit)) {
                    explicit.to_string()
                } else if node.files.len() > 1 {
                    ingest.multi_file_model.clone()
                } else if let Some(f) = node.files.first() {
                    media_for_extension(cfg, &f.extension)
                        .map(|(_, model)| model.to_string())
                        .unwrap_or_else(|| "Binary".into())
                } else {
                    ingest.no_file_model.clone()
                };
            }
        }
    }
    tree
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sacda::source::parse;

    fn cfg() -> Option<Config> {
        let g = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        Config::load_from(&g).ok()
    }

    fn table(csv: &str) -> Table {
        parse("t", csv.as_bytes(), &HashMap::new()).unwrap()
    }

    #[test]
    fn builds_nested_tree_with_derived_containers() {
        let Some(cfg) = cfg() else { return };
        let t = table(
            "accessIdentifier,title,file,file_extension,fileTitle\n\
             2099_01,My Collection,,,\n\
             2099_01_01_01_001,Photo,p1,jpg,Box one\n\
             2099_01_01_01_002,Song,s1,mp3,Box one\n\
             2099_01_02_000,Second series,,,\n\
             2099_01_02_001,Letter,l1,pdf,\n",
        );
        let tree = build(&t, &cfg, &FileIndex::build(Path::new("/nonexistent")));
        assert!(tree.rejected.is_empty(), "{:?}", tree.rejected);
        assert_eq!(tree.nodes["2099_01"].kind, Kind::Collection);
        assert_eq!(tree.nodes["2099_01"].title, "My Collection");
        assert_eq!(tree.nodes["2099_01_01"].level, "Series");
        assert_eq!(tree.nodes["2099_01_01_01"].level, "File");
        assert_eq!(tree.nodes["2099_01_01_01"].title, "Box one");
        assert_eq!(tree.nodes["2099_01_02"].title, "Second series");
        assert_eq!(tree.nodes["2099_01_02"].level, "File");
        assert_eq!(tree.nodes["2099_01_01_01_001"].model, "Image");
        assert_eq!(tree.nodes["2099_01_01_01_002"].model, "Audio");
        assert_eq!(tree.nodes["2099_01_02_001"].model, "Digital Document");
        let order: Vec<&str> = tree.containers_in_order().iter().map(|n| n.id.as_str()).collect();
        assert_eq!(order, vec!["2099_01", "2099_01_01", "2099_01_02", "2099_01_01_01"]);
    }

    #[test]
    fn repeated_item_rows_make_a_compound_object() {
        let Some(cfg) = cfg() else { return };
        let t = table(
            "accessIdentifier,title,file,file_extension\n\
             2099_02_01_001,Album,a,jpg\n\
             2099_02_01_001,,b,jpg\n",
        );
        let tree = build(&t, &cfg, &FileIndex::build(Path::new("/nonexistent")));
        let item = &tree.nodes["2099_02_01_001"];
        assert_eq!(item.files.len(), 2);
        assert_eq!(item.model, "Compound Object");
        assert_eq!(tree.extra_file_rows, vec![1]);
    }

    #[test]
    fn rejects_bad_identifiers() {
        let Some(cfg) = cfg() else { return };
        let t = table("accessIdentifier,title\n2099,x\nbad id,y\n,z\n");
        let tree = build(&t, &cfg, &FileIndex::build(Path::new("/nonexistent")));
        assert_eq!(tree.rejected.len(), 3);
    }
}
