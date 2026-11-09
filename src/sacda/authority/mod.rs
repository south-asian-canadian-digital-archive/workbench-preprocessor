//! Authority lookups that give new taxonomy terms their source link.
//!
//! An `Authority` turns a sheet value into a label + URI: either the value is
//! already an identifier for that authority (`sh85005357`, an id.loc.gov URL)
//! and we fetch its label, or it's a heading and we look it up by exact label.
//! New authorities (Wikidata, GeoNames...) implement the same trait and get a
//! name in `config/sacda.toml` `[fields.*] authority`.

pub mod lc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// A resolved heading: what to name the term, and its link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolved {
    pub label: String,
    pub uri: String,
    /// The authority_link `source` key the site expects (e.g. `lcsh`, `naf`).
    pub source: String,
    /// True when the sheet held an identifier rather than the heading.
    pub from_identifier: bool,
}

impl Resolved {
    /// Workbench's `field_authority_link` cell format: `source%%uri%%title`.
    pub fn workbench_cell(&self) -> String {
        format!("{}%%{}%%", self.source, self.uri)
    }
}

pub trait Authority {
    fn name(&self) -> &str;
    /// Whether this value looks like one of this authority's identifiers.
    fn is_identifier(&self, value: &str) -> bool;
    /// Ok(None) means the authority answered but has no such heading.
    fn resolve(&self, value: &str) -> Result<Option<Resolved>>;
}

pub fn by_name(name: &str) -> Option<Box<dyn Authority>> {
    match name {
        "lcsh" => Some(Box::new(lc::Lc::subjects())),
        "lcnaf" => Some(Box::new(lc::Lc::names())),
        _ => None,
    }
}

/// Lookups are slow and the answers don't change, so they're cached on disk
/// across runs (negative answers too, so a typo isn't re-queried every run).
#[derive(Default, Serialize, Deserialize)]
pub struct Cache {
    entries: HashMap<String, Option<Resolved>>,
    #[serde(skip)]
    path: Option<PathBuf>,
    #[serde(skip)]
    dirty: bool,
}

impl Cache {
    pub fn open() -> Self {
        let path = cache_dir().map(|d| d.join("authority-cache.json"));
        let mut cache: Cache = path
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        cache.path = path;
        cache
    }

    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Resolve through the cache. Network errors aren't cached.
    pub fn resolve(&mut self, auth: &dyn Authority, value: &str) -> Result<Option<Resolved>> {
        let key = format!("{}\u{1f}{}", auth.name(), value.trim().to_lowercase());
        if let Some(hit) = self.entries.get(&key) {
            return Ok(hit.clone());
        }
        let got = auth.resolve(value)?;
        self.entries.insert(key, got.clone());
        self.dirty = true;
        Ok(got)
    }

    pub fn save(&self) {
        if !self.dirty {
            return;
        }
        if let Some(p) = &self.path {
            if let Some(dir) = p.parent() {
                let _ = fs::create_dir_all(dir);
            }
            if let Ok(text) = serde_json::to_string(self) {
                let _ = fs::write(p, text);
            }
        }
    }
}

pub fn cache_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("sacda"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .map(|d| d.join("sacda"))
    }
}
