//! Small "did you mean" engine for column names and term values.
//!
//! Names are compared after normalising away the differences archivists
//! actually make - case, `_`/`-`/space, a missing `field_` prefix, plurals -
//! and then by Damerau-Levenshtein distance (a swap of two letters counts as
//! one edit). A suggestion is only offered when it's clearly the closest one.

/// Lowercase, drop `field_`, and squash separators: `Field Subject` and
/// `field_subject` both become `subject`.
pub fn normalize(s: &str) -> String {
    let lower = s.trim().to_lowercase();
    let lower = lower.strip_prefix("field_").unwrap_or(&lower).to_string();
    lower
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Lowercase alphanumerics only, keeping any prefix.
fn squash(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Optimal-string-alignment distance (Damerau-Levenshtein without
/// substring moves), over chars.
pub fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n][m]
}

/// How many edits a name of this length may be off by and still count as a typo.
fn budget(len: usize) -> usize {
    match len {
        0..=3 => 0,
        4..=5 => 1,
        6..=10 => 2,
        _ => 3,
    }
}

/// The closest candidate to `input`, if it's a plausible typo and not tied
/// with another candidate. Exact matches after normalising (case, prefix,
/// separators) always win.
pub fn closest<'a, I>(input: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let key = normalize(input);
    let full = squash(input);
    if key.is_empty() {
        return None;
    }
    let mut best: Option<(&str, usize)> = None;
    let mut tied = false;
    for cand in candidates {
        let ck = normalize(cand);
        if ck.is_empty() {
            continue;
        }
        // A missing or extra trailing `s` is a plural, not a typo.
        let d = if ck == key || ck.trim_end_matches('s') == key.trim_end_matches('s') {
            0
        } else {
            // Also compare with prefixes kept, so a misspelt `feild_` still matches.
            distance(&key, &ck).min(distance(&full, &squash(cand)))
        };
        match best {
            Some((_, bd)) if d > bd => {}
            Some((prev, bd)) if d == bd => {
                if normalize(prev) != ck {
                    tied = true;
                }
            }
            _ => {
                best = Some((cand, d));
                tied = false;
            }
        }
    }
    let (cand, d) = best?;
    if d == 0 {
        return Some(cand);
    }
    if tied || d > budget(key.chars().count().max(normalize(cand).chars().count())) {
        return None;
    }
    Some(cand)
}

/// The case-insensitive exact match, if any (used for term names, where
/// `photographs` and `Photographs` are the same term to Workbench).
pub fn exact_ci<'a, I>(input: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let lower = input.trim().to_lowercase();
    candidates.into_iter().find(|c| c.to_lowercase() == lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIELDS: &[&str] = &[
        "field_subject",
        "field_description",
        "field_edtf_date_created",
        "field_linked_agent",
        "field_language",
        "field_sacda_thesaurus",
        "title",
    ];

    #[test]
    fn suggests_typos() {
        assert_eq!(closest("field_subjet", FIELDS.iter().copied()), Some("field_subject"));
        assert_eq!(closest("feild_description", FIELDS.iter().copied()), Some("field_description"));
        assert_eq!(closest("Subject", FIELDS.iter().copied()), Some("field_subject"));
        assert_eq!(closest("field_subjects", FIELDS.iter().copied()), Some("field_subject"));
        assert_eq!(closest("Linked Agent", FIELDS.iter().copied()), Some("field_linked_agent"));
        assert_eq!(closest("titel", FIELDS.iter().copied()), Some("title"));
        assert_eq!(closest("sacda thesarus", FIELDS.iter().copied()), Some("field_sacda_thesaurus"));
    }

    #[test]
    fn no_suggestion_for_unrelated() {
        assert_eq!(closest("Collection ID", FIELDS.iter().copied()), None);
        assert_eq!(closest("xyz", FIELDS.iter().copied()), None);
    }

    #[test]
    fn transposition_is_one_edit() {
        assert_eq!(distance("ab", "ba"), 1);
        assert_eq!(distance("kitten", "sitting"), 3);
    }

    #[test]
    fn term_typos() {
        let terms = ["Photographs", "Newspapers", "Anniversaries"];
        assert_eq!(closest("Photogrpahs", terms.iter().copied()), Some("Photographs"));
        assert_eq!(exact_ci("photographs", terms.iter().copied()), Some("Photographs"));
    }
}
