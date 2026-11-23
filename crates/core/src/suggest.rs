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
        4..=7 => 1,
        8..=12 => 2,
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
    one_word_typo(input, cand).then_some(cand)
}

/// For phrases, a suggestion may only fix a typo inside one word (or the
/// spacing): `Diwali festival` -> `Divali festival` yes, but `men poets` ->
/// `men models` or `women labour activists` -> `men labour activists` no -
/// those are different real words, not typos.
fn one_word_typo(input: &str, cand: &str) -> bool {
    let words = |s: &str| -> Vec<String> { s.split_whitespace().map(letters).filter(|w| !w.is_empty()).collect() };
    let (a, b) = (words(input), words(cand));
    // Single-token candidates (field names, one-word terms) keep the plain
    // distance rule.
    if b.len() < 2 {
        return true;
    }
    if a.len() != b.len() {
        // Only a spacing slip: `Bharatanatyam` / `Bharata natyam`.
        return distance(&a.concat(), &b.concat()) <= 1;
    }
    let diff: Vec<(&String, &String)> = a.iter().zip(b.iter()).filter(|(x, y)| x != y).collect();
    match diff.as_slice() {
        [] => true,
        [(x, y)] => {
            x.trim_end_matches('s') == y.trim_end_matches('s')
                || distance(x, y) <= budget(x.chars().count().max(y.chars().count()))
        }
        _ => false,
    }
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

/// The one candidate that's the same once case, spaces and punctuation are
/// ignored (`newwestminster` = `New Westminster`, `Isma'ilism` = `Ismailism`,
/// curly vs straight apostrophes). None if there's no such candidate or more
/// than one.
pub fn same_letters<'a, I>(input: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let key = letters(input);
    if key.is_empty() {
        return None;
    }
    // Accents the sheet typed are meant: `Ḍal` and `Dal` are different words.
    // Only a value typed without them takes the accented term (`Giddha` ->
    // `Giddhā`); anything else is left to the "did you mean" suggestions.
    let marked = has_marks(input);
    let mut hits = candidates
        .into_iter()
        .filter(|c| letters(c) == key && (!marked || accented(c) == accented(input)));
    let first = hits.next()?;
    hits.all(|h| h == first).then_some(first)
}

/// Does the text carry accents (combining marks once decomposed)?
fn has_marks(s: &str) -> bool {
    use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};
    s.nfd().any(is_combining_mark)
}

/// Like `letters`, but keeping the accents.
fn accented(s: &str) -> String {
    use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};
    s.nfd()
        .filter(|c| (c.is_alphanumeric() || is_combining_mark(*c)) && !('\u{02B0}'..='\u{02FF}').contains(c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Letters and digits only, lower-cased, accents and modifier letters dropped
/// (`Giddhā` = `Giddha`, `ʻĪd` = `Id`).
pub fn letters(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    s.nfd()
        .filter(|c| c.is_alphanumeric() && !('\u{02B0}'..='\u{02FF}').contains(c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Does an authority's heading plausibly name what the sheet typed? Equal
/// ignoring case/accents/punctuation, or a typo apart - either against the
/// whole heading or the heading without its parenthetical qualifier (`Holi` /
/// `Holī (Hindu festival)`). A subdivision is a different subject:
/// `Muslims` does not agree with `Muslims--Dietary laws`.
pub fn headings_agree(typed: &str, authority: &str) -> bool {
    let a = letters(typed);
    if a.is_empty() {
        return true;
    }
    let whole = letters(authority);
    let core = letters(authority.split(" (").next().unwrap_or(authority));
    let room = budget(a.chars().count());
    if [&whole, &core].iter().any(|b| **b == a || distance(&a, b) <= room) {
        return true;
    }
    // Inverted headings: `Group portraits` = `Portraits, Group`.
    let bag = |s: &str| {
        let mut w: Vec<String> = s.split(|c: char| c.is_whitespace() || c == ',').map(letters).filter(|w| !w.is_empty()).collect();
        w.sort();
        w
    };
    bag(typed) == bag(authority)
}

/// In plain words, why `typed` looks like `existing` - shown next to a
/// suggestion so it's clear whether it's a typo fix or a different term.
pub fn why(typed: &str, existing: &str) -> String {
    let (a, b) = (letters(typed), letters(existing));
    if a == b {
        return "same letters - only case, accents, spacing or punctuation differ".into();
    }
    if a.trim_end_matches('s') == b.trim_end_matches('s') {
        return "singular / plural of the same word".into();
    }
    let bag = |s: &str| {
        let mut w: Vec<String> = s.split(|c: char| c.is_whitespace() || c == ',').map(letters).filter(|w| !w.is_empty()).collect();
        w.sort();
        w
    };
    if bag(typed) == bag(existing) {
        return "same words in a different order".into();
    }
    let d = distance(&a, &b);
    format!("looks like a spelling mistake - {d} letter{} different", if d == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_letters_ignores_spacing_and_punctuation() {
        let c = ["New Westminster", "Ismailism or Ismaili Muslims", "Vancouver"];
        assert_eq!(same_letters("Newwestminster", c), Some("New Westminster"));
        assert_eq!(same_letters("Isma'ilism or Ismaili Muslims", c), Some("Ismailism or Ismaili Muslims"));
        assert_eq!(same_letters("Surrey", c), None);
        assert_eq!(same_letters("Giddha (Dance)", ["Giddhā (Dance)"]), Some("Giddhā (Dance)"));
        // Accents typed in the sheet are meant: a retroflex isn't a dental.
        assert_eq!(same_letters("Ḍal", ["Dal"]), None);
        assert_eq!(same_letters("Naṭ", ["Nat", "Naṭ"]), Some("Naṭ"));
        assert_eq!(closest("Sangat", ["Langar"]), None);
        assert!(headings_agree("Holi", "Holī (Hindu festival)"));
        assert!(headings_agree("Diwali", "Dīvālī (Hindu festival)"));
        assert!(headings_agree("Id al-Adha", "ʻĪd al-Aḍḥā"));
        assert!(!headings_agree("Festivals", "Muslims"));
        assert!(!headings_agree("Muslims", "Muslims--Dietary laws"));
        assert!(headings_agree("Group portraits", "Portraits, Group"));
        let th = ["South Asian Canadian men models", "South Asian Canadian men labour activists", "Divali", "Bharata natyam", "Masjids"];
        assert_eq!(closest("South Asian Canadian men poets", th), None);
        assert_eq!(closest("South Asian Canadian women labour activists", th), None);
        assert_eq!(closest("Diwali", th), Some("Divali"));
        assert_eq!(closest("Bharatanatayam", th), Some("Bharata natyam"));
        assert_eq!(closest("Masjid", th), Some("Masjids"));
    }

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

    #[test]
    fn explains_suggestions() {
        assert!(why("Garbha", "Garba").contains("1 letter"));
        assert!(why("Masjid", "Masjids").contains("plural"));
        assert!(why("Giddhā", "Giddha").contains("same letters"));
        assert!(why("Portraits, Group", "Group portraits").contains("order"));
    }
}
