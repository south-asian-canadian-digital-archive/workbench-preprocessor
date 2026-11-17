//! Resolve the `file` column of a processed CSV against the files actually on
//! disk, instead of trusting the `parent_id/name.ext` path the FileExtension
//! modifier builds from metadata alone.
//!
//! Port of `index_access_files` / `staged_match` from scripts/ca-migration.

use anyhow::{Context, Result};
use csv::{Reader, Writer};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct DiscoveryStats {
    /// Rows whose path already pointed at a real file.
    pub matched: usize,
    /// Rows rewritten to where the file actually is.
    pub relocated: usize,
    /// Rows with no file on disk at all: `(id, path that was tried)`.
    pub missing: Vec<(String, String)>,
}

/// Index every file under `root`, keyed by lowercased file name and by
/// lowercased stem, mapping to the path relative to `root`.
///
/// ponytail: first hit wins on a duplicate key; collections with the same base
/// name in two subdirectories need a parent-scoped key, add it when that shows up.
fn index_files(root: &Path) -> Result<HashMap<String, PathBuf>> {
    let mut index = HashMap::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir)
            .with_context(|| format!("Failed to read access directory: {}", dir.display()))?;
        for entry in entries {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            for key in [path.file_name(), path.file_stem()].into_iter().flatten() {
                index
                    .entry(key.to_string_lossy().to_ascii_lowercase())
                    .or_insert_with(|| relative.clone());
            }
        }
    }

    Ok(index)
}

fn to_forward_slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Rewrite `csv_path` in place so every `file` cell points at a real file under
/// `access_dir`. Cells with no match are left alone and reported.
pub fn resolve_file_paths(csv_path: &str, access_dir: &str) -> Result<DiscoveryStats> {
    let root = Path::new(access_dir);
    if !root.is_dir() {
        anyhow::bail!("Access directory does not exist: {}", access_dir);
    }

    let index = index_files(root)?;

    let mut reader = Reader::from_path(csv_path)
        .with_context(|| format!("Failed to open {} for file discovery", csv_path))?;
    let headers = reader.headers()?.clone();
    let file_idx = match headers.iter().position(|h| h == "file") {
        Some(idx) => idx,
        None => return Ok(DiscoveryStats::default()),
    };
    let id_idx = headers.iter().position(|h| h == "accessIdentifier");

    let mut stats = DiscoveryStats::default();
    let mut rows: Vec<Vec<String>> = Vec::new();

    for result in reader.records() {
        let record = result?;
        let mut row: Vec<String> = record.iter().map(|c| c.to_string()).collect();
        let value = row[file_idx].trim().to_string();

        if !value.is_empty() {
            let id = id_idx
                .and_then(|i| row.get(i))
                .map(|s| s.to_string())
                .unwrap_or_default();

            if root.join(&value).is_file() {
                stats.matched += 1;
            } else {
                let name = Path::new(&value);
                let key = [name.file_name(), name.file_stem()]
                    .into_iter()
                    .flatten()
                    .map(|k| k.to_string_lossy().to_ascii_lowercase())
                    .find(|k| index.contains_key(k));

                match key.and_then(|k| index.get(&k)) {
                    Some(found) => {
                        row[file_idx] = to_forward_slashes(found);
                        stats.relocated += 1;
                    }
                    None => stats.missing.push((id, value)),
                }
            }
        }

        rows.push(row);
    }

    let mut writer =
        Writer::from_path(csv_path).with_context(|| format!("Failed to rewrite {}", csv_path))?;
    writer.write_record(headers.iter())?;
    for row in rows {
        writer.write_record(&row)?;
    }
    writer.flush()?;

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn relocates_moved_file_and_reports_missing() -> Result<()> {
        let temp = tempdir()?;
        let access = temp.path().join("access");
        fs::create_dir_all(access.join("2024_19"))?;
        fs::write(access.join("2024_19").join("2024_19_001.jpg"), b"x")?;

        let csv_path = temp.path().join("processed.csv");
        fs::write(
            &csv_path,
            "accessIdentifier,file\n\
             2024_19_001,wrong_dir/2024_19_001.jpg\n\
             2024_19_002,2024_19/2024_19_002.jpg\n",
        )?;

        let stats = resolve_file_paths(csv_path.to_str().unwrap(), access.to_str().unwrap())?;

        assert_eq!(stats.relocated, 1);
        assert_eq!(stats.matched, 0);
        assert_eq!(
            stats.missing,
            vec![(
                "2024_19_002".to_string(),
                "2024_19/2024_19_002.jpg".to_string()
            )]
        );

        let rewritten = fs::read_to_string(&csv_path)?;
        assert!(rewritten.contains("2024_19/2024_19_001.jpg"));
        Ok(())
    }

    #[test]
    fn leaves_correct_paths_alone() -> Result<()> {
        let temp = tempdir()?;
        let access = temp.path().join("access");
        fs::create_dir_all(access.join("2024_19"))?;
        fs::write(access.join("2024_19").join("a.jpg"), b"x")?;

        let csv_path = temp.path().join("processed.csv");
        fs::write(
            &csv_path,
            "accessIdentifier,file\n2024_19_001,2024_19/a.jpg\n",
        )?;

        let stats = resolve_file_paths(csv_path.to_str().unwrap(), access.to_str().unwrap())?;
        assert_eq!(stats.matched, 1);
        assert_eq!(stats.relocated, 0);
        assert!(stats.missing.is_empty());
        Ok(())
    }
}
