//! Loads the ingest settings (`config/sacda.toml`) and the site schema
//! (`config/schema.toml`) from the g/ checkout.
//!
//! The g/ folder is found without flags: `SACDA_G` env var, then the path saved
//! by the installer, then walking up from the current directory and from the
//! executable looking for `config/sacda.toml`.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct SiteCfg {
    pub host: String,
    pub credentials: String,
    #[serde(default = "default_thesaurus")]
    pub thesaurus_vocabulary: String,
    #[serde(default)]
    pub confirm: bool,
}

fn default_thesaurus() -> String {
    "subject".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct IngestCfg {
    #[serde(default = "d_access")]
    pub access_dir: String,
    #[serde(default = "d_subdelim")]
    pub subdelimiter: String,
    #[serde(default = "d_intermediate")]
    pub intermediate_level: String,
    #[serde(default = "d_multi")]
    pub multi_file_model: String,
    #[serde(default = "d_nofile")]
    pub no_file_model: String,
}

fn d_access() -> String {
    "access".into()
}
fn d_subdelim() -> String {
    "|".into()
}
fn d_intermediate() -> String {
    "Series".into()
}
fn d_multi() -> String {
    "Compound Object".into()
}
fn d_nofile() -> String {
    "Binary".into()
}

impl Default for IngestCfg {
    fn default() -> Self {
        Self {
            access_dir: d_access(),
            subdelimiter: d_subdelim(),
            intermediate_level: d_intermediate(),
            multi_file_model: d_multi(),
            no_file_model: d_nofile(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FieldRule {
    /// Vocabulary a new term goes to. `@thesaurus` means the site's thesaurus vocabulary.
    pub vocab: Option<String>,
    pub relator: Option<String>,
    pub authority: Option<String>,
    #[serde(default)]
    pub never_create: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SacdaToml {
    pub default_site: String,
    pub sites: BTreeMap<String, SiteCfg>,
    #[serde(default)]
    pub ingest: IngestCfg,
    #[serde(default)]
    pub fields: HashMap<String, FieldRule>,
    #[serde(default)]
    pub aliases: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FieldDef {
    pub label: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default = "one")]
    pub cardinality: i64,
    #[serde(default)]
    pub target_type: String,
    #[serde(default)]
    pub targets: Vec<String>,
}

fn one() -> i64 {
    1
}

impl FieldDef {
    pub fn is_taxonomy(&self) -> bool {
        self.target_type == "taxonomy_term"
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContentType {
    pub fields: BTreeMap<String, FieldDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MediaType {
    #[serde(default)]
    pub source_field: String,
    #[serde(default)]
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Schema {
    pub content_types: BTreeMap<String, ContentType>,
    pub media_types: BTreeMap<String, MediaType>,
}

impl Schema {
    pub fn fields(&self, bundle: &str) -> &BTreeMap<String, FieldDef> {
        static EMPTY: std::sync::OnceLock<BTreeMap<String, FieldDef>> = std::sync::OnceLock::new();
        self.content_types
            .get(bundle)
            .map(|c| &c.fields)
            .unwrap_or_else(|| EMPTY.get_or_init(BTreeMap::new))
    }
}

/// Everything `organise` knows before talking to a site.
#[derive(Debug, Clone)]
pub struct Config {
    pub g_root: PathBuf,
    pub sacda: SacdaToml,
    pub schema: Schema,
}

impl Config {
    pub fn load(explicit: Option<&Path>) -> Result<Self> {
        let g_root = match explicit {
            Some(p) => p.to_path_buf(),
            None => find_g_root().context(
                "Couldn't find the g/ folder (looked for config/sacda.toml). Run organise from \
                 inside the workbench checkout, set SACDA_G, or re-run the installer.",
            )?,
        };
        Self::load_from(&g_root)
    }

    pub fn load_from(g_root: &Path) -> Result<Self> {
        let sacda_path = g_root.join("config/sacda.toml");
        let schema_path = g_root.join("config/schema.toml");
        let sacda: SacdaToml = toml::from_str(
            &fs::read_to_string(&sacda_path)
                .with_context(|| format!("reading {}", sacda_path.display()))?,
        )
        .with_context(|| format!("parsing {}", sacda_path.display()))?;
        let schema: Schema = toml::from_str(
            &fs::read_to_string(&schema_path)
                .with_context(|| format!("reading {}", schema_path.display()))?,
        )
        .with_context(|| format!("parsing {}", schema_path.display()))?;
        if !sacda.sites.contains_key(&sacda.default_site) {
            bail!("default_site '{}' isn't one of the [sites]", sacda.default_site);
        }
        Ok(Self {
            g_root: g_root.to_path_buf(),
            sacda,
            schema,
        })
    }

    pub fn site(&self, name: Option<&str>) -> Result<(String, SiteCfg)> {
        let name = name.unwrap_or(&self.sacda.default_site);
        match self.sacda.sites.get(name) {
            Some(s) => Ok((name.to_string(), s.clone())),
            None => bail!(
                "Unknown site '{}'. Known: {}",
                name,
                self.sacda.sites.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
        }
    }

    /// The workbench checkout that holds g/ (where `workbench` itself lives).
    pub fn workbench_root(&self) -> PathBuf {
        self.g_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.g_root.clone())
    }

    /// The vocabulary a NEW value for `field` goes to on `site`.
    pub fn new_term_vocab(&self, field: &str, site: &SiteCfg) -> Option<String> {
        let rule = self.sacda.fields.get(field)?;
        let v = rule.vocab.as_deref()?;
        Some(if v == "@thesaurus" {
            site.thesaurus_vocabulary.clone()
        } else {
            v.to_string()
        })
    }
}

/// The installer records where g/ is so organise works from any folder.
pub fn user_config_path() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("sacda").join("organise.toml"))
    } else {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("sacda").join("organise.toml"))
    }
}

#[derive(Deserialize)]
struct UserCfg {
    g_root: Option<String>,
}

fn is_g_root(p: &Path) -> bool {
    p.join("config/sacda.toml").is_file()
}

fn walk_up(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(dir) = cur {
        if is_g_root(dir) {
            return Some(dir.to_path_buf());
        }
        // Also accept the workbench checkout itself, which holds g/.
        if is_g_root(&dir.join("g")) {
            return Some(dir.join("g"));
        }
        cur = dir.parent();
    }
    None
}

pub fn find_g_root() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SACDA_G").map(PathBuf::from) {
        if is_g_root(&p) {
            return Some(p);
        }
    }
    if let Some(cfg) = user_config_path().filter(|p| p.is_file()) {
        if let Ok(text) = fs::read_to_string(&cfg) {
            if let Ok(UserCfg { g_root: Some(g) }) = toml::from_str::<UserCfg>(&text) {
                let p = PathBuf::from(g);
                if is_g_root(&p) {
                    return Some(p);
                }
            }
        }
    }
    if let Some(found) = std::env::current_dir().ok().and_then(|d| walk_up(&d)) {
        return Some(found);
    }
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().and_then(walk_up))
}

/// `username:` / `password:` from a Workbench credentials YAML. Only those two
/// keys exist in these files, so a line parser avoids a YAML dependency.
pub fn read_credentials(path: &Path) -> Result<(String, String)> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("credentials file {} isn't there", path.display()))?;
    let mut user = None;
    let mut pass = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
            match k.trim() {
                "username" => user = Some(v),
                "password" => pass = Some(v),
                _ => {}
            }
        }
    }
    match (user, pass) {
        (Some(u), Some(p)) if !u.is_empty() && !p.is_empty() => Ok((u, p)),
        _ => bail!("{} has no username/password", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_real_g_config() {
        // The submodule sits at g/scripts/workbench-preprocessor.
        let g = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        if !is_g_root(&g) {
            return; // standalone checkout of the preprocessor
        }
        let cfg = Config::load_from(&g).unwrap();
        assert!(cfg.sacda.sites.contains_key("dev"));
        let ac = cfg.schema.fields("archival_component");
        assert!(ac["field_archival_level"].required);
        assert!(cfg.schema.media_types["image"].extensions.contains(&"jpg".to_string()));
    }

    #[test]
    fn parses_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.yml");
        fs::write(&p, "username: admin\npassword: \"s3:cret\"\n").unwrap();
        assert_eq!(read_credentials(&p).unwrap(), ("admin".into(), "s3:cret".into()));
    }
}
