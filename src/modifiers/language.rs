use crate::csv_modifier::{ColumnModifier, RowContext};
use anyhow::{Context, Result};
use log::{debug, info, warn};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Path segment for the language code → term JSON export (appended to `ISLANDORA_BASE_URL` when no full URL is set).
pub const DEFAULT_LANGUAGE_CODE_PATH: &str = "/lang-code";

const DEFAULT_BASE_URL: &str = "http://localhost:8000";

#[derive(Debug, Deserialize)]
struct LanguageTermJson {
    #[allow(dead_code)]
    name: String,
    tid: String,
    field_code: String,
}

/// Resolves the URL to fetch language mappings.
///
/// Precedence: `cli_override` → `ISLANDORA_LANGUAGE_URL` → `{ISLANDORA_BASE_URL or default}{DEFAULT_LANGUAGE_CODE_PATH}`.
pub fn resolve_language_mapping_url(cli_override: Option<&str>) -> String {
    if let Some(url) = cli_override {
        let t = url.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Ok(url) = std::env::var("ISLANDORA_LANGUAGE_URL") {
        let t = url.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    let base = std::env::var("ISLANDORA_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
    let base = base.trim_end_matches('/');
    format!("{}{}", base, DEFAULT_LANGUAGE_CODE_PATH)
}

pub struct LanguageModifier {
    code_to_tid: HashMap<String, String>,
    matched: AtomicU64,
    unmatched: AtomicU64,
    empty_skips: AtomicU64,
}

impl LanguageModifier {
    /// Fetches the vocabulary export. On network failure, non-success HTTP status, or invalid JSON,
    /// returns an error so the caller does not run CSV processing without a mapping table.
    pub fn new(api_url: &str) -> Result<Self> {
        let response = reqwest::blocking::get(api_url)
            .with_context(|| format!("Failed to connect to language mapping URL {}", api_url))?;

        let response = response
            .error_for_status()
            .with_context(|| format!("Language mapping request failed (check URL and server): {}", api_url))?;

        let terms: Vec<LanguageTermJson> = response
            .json()
            .with_context(|| "Failed to parse language mapping JSON")?;

        let mut code_to_tid = HashMap::new();
        for term in terms {
            let key = term.field_code.trim().to_lowercase();
            if key.is_empty() {
                continue;
            }
            let tid = term.tid.trim();
            if tid.is_empty() {
                continue;
            }
            code_to_tid.entry(key).or_insert_with(|| tid.to_string());
        }

        let n = code_to_tid.len();
        if n == 0 {
            warn!(
                "Language modifier: JSON loaded from {} but no usable field_code→tid pairs (check JSON shape).",
                api_url
            );
        } else {
            info!(
                "Language modifier: loaded {} code→term mappings from {}",
                n, api_url
            );
            debug!(
                "Language modifier: sample codes in map: {:?}",
                code_to_tid.keys().take(8).collect::<Vec<_>>()
            );
        }

        Ok(Self {
            code_to_tid,
            matched: AtomicU64::new(0),
            unmatched: AtomicU64::new(0),
            empty_skips: AtomicU64::new(0),
        })
    }
}

impl Drop for LanguageModifier {
    fn drop(&mut self) {
        let m = self.matched.load(Ordering::Relaxed);
        let u = self.unmatched.load(Ordering::Relaxed);
        let e = self.empty_skips.load(Ordering::Relaxed);
        let total = m + u + e;
        if total == 0 {
            warn!(
                "Language modifier: no cells processed in column `field_language` — add that column to the CSV/Sheet, or use --ignore-run language if you do not need it."
            );
        } else if u > 0 {
            warn!(
                "Language modifier: {} cells mapped to term ID, {} unknown codes left unchanged, {} empty cells ({} `field_language` cells total).",
                m, u, e, total
            );
        } else {
            info!(
                "Language modifier: {} cells mapped to term ID, {} empty cells skipped ({} `field_language` cells total).",
                m, e, total
            );
        }
    }
}

impl ColumnModifier for LanguageModifier {
    fn modify(&self, value: &str, _row: &RowContext) -> String {
        let normalized = value.trim().to_lowercase();
        if normalized.is_empty() {
            self.empty_skips.fetch_add(1, Ordering::Relaxed);
            return value.to_string();
        }

        if let Some(tid) = self.code_to_tid.get(&normalized) {
            self.matched.fetch_add(1, Ordering::Relaxed);
            debug!(
                "Language modifier: {:?} → tid {}",
                value.trim(),
                tid
            );
            tid.clone()
        } else {
            self.unmatched.fetch_add(1, Ordering::Relaxed);
            debug!(
                "Language modifier: no map entry for code {:?} (left unchanged)",
                value.trim()
            );
            value.to_string()
        }
    }

    fn description(&self) -> &str {
        "Replaces field_language codes (field_code) with taxonomy term IDs from Islandora"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_url_cli_wins() {
        assert_eq!(
            resolve_language_mapping_url(Some("https://x.example/full")),
            "https://x.example/full"
        );
    }
}
