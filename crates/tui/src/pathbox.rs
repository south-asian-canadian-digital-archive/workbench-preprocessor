//! Tab completion for a typed folder path. Only folders are offered: the box
//! asks for the folder that holds the access files.

use std::path::{Path, PathBuf};

pub struct Completion {
    /// The box's new text (unchanged when nothing matched).
    pub text: String,
    /// Every folder that matched, when there was more than one (or none: empty).
    pub matches: Vec<String>,
}

fn is_sep(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// Complete the last part of `typed` against the folders on disk. A relative
/// path is read from `base` (the folder organise runs in, like Enter does).
pub fn complete(typed: &str, base: &Path) -> Completion {
    // A dragged-in path can come quoted or with escaped spaces: complete the plain form.
    let plain = {
        let t = typed.trim_start().trim_matches(|c| c == '"' || c == '\'');
        if cfg!(windows) { t.to_string() } else { t.replace("\\ ", " ") }
    };
    let cut = plain.rfind(is_sep).map(|i| i + 1).unwrap_or(0);
    let (head, tail) = plain.split_at(cut);
    let dir = if head.is_empty() {
        base.to_path_buf()
    } else {
        let p = super::clean_path(head);
        if p.is_absolute() { p } else { base.join(p) }
    };
    let want = tail.to_lowercase();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                // metadata() follows links, so a link to a folder counts.
                .filter(|e| std::fs::metadata(e.path()).is_ok_and(|m| m.is_dir()))
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| tail.starts_with('.') || !n.starts_with('.'))
                .filter(|n| n.to_lowercase().starts_with(&want))
                .collect()
        })
        .unwrap_or_default();
    names.sort_by_key(|n| n.to_lowercase());
    let sep = if cfg!(windows) && head.contains('\\') { '\\' } else { '/' };
    match names.len() {
        0 => Completion { text: typed.to_string(), matches: vec![] },
        1 => Completion { text: format!("{head}{}{sep}", names[0]), matches: vec![] },
        _ => {
            let first: Vec<char> = names[0].chars().collect();
            let mut n = first.len();
            for other in &names[1..] {
                let o: Vec<char> = other.chars().collect();
                n = n.min(first.iter().zip(&o).take_while(|(a, b)| a.to_lowercase().eq(b.to_lowercase())).count());
            }
            let common: String = first[..n].iter().collect();
            // Only replace the typed part when it gets longer.
            let text = if common.chars().count() > tail.chars().count() { format!("{head}{common}") } else { typed.to_string() };
            Completion { text, matches: names.into_iter().map(|m| format!("{m}{sep}")).collect() }
        }
    }
}

/// Where a typed path points (relative to `base`), for showing in full.
pub fn resolve(typed: &str, base: &Path) -> PathBuf {
    let p = super::clean_path(typed);
    if p.is_absolute() { p } else { base.join(p) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_folders_only() {
        let t = tempfile::tempdir().unwrap();
        for d in ["Access files", "Archive", "beta", ".hidden"] {
            std::fs::create_dir(t.path().join(d)).unwrap();
        }
        std::fs::write(t.path().join("Accounts.csv"), "").unwrap();
        let b = t.path();
        // One folder fits: completed, with a separator to go on typing.
        assert_eq!(complete("be", b).text, "beta/");
        assert_eq!(complete("acc", b).text, "Access files/");
        // Several: as far as they agree, and the list to choose from.
        let c = complete("A", b);
        assert_eq!(c.text, "A");
        assert_eq!(c.matches, vec!["Access files/", "Archive/"]);
        let c = complete("", b);
        assert_eq!(c.matches.len(), 3, "hidden folders stay out");
        assert_eq!(complete(".h", b).text, ".hidden/");
        // Inside a folder, and an absolute path.
        std::fs::create_dir(t.path().join("beta/scans")).unwrap();
        assert_eq!(complete("beta/s", b).text, "beta/scans/");
        let abs = format!("{}/beta/", b.display());
        assert_eq!(complete(&abs, Path::new("/")).text, format!("{abs}scans/"));
        // Nothing matches: left as typed.
        assert_eq!(complete("zzz", b).text, "zzz");
        assert!(complete("zzz", b).matches.is_empty());
    }
}
