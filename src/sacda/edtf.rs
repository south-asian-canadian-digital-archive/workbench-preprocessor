//! EDTF (Extended Date/Time Format) checks for Islandora's `edtf` fields.
//!
//! `is_valid` accepts EDTF levels 0 and 1 (plus sets), which is what the site's
//! EDTF widget accepts. `normalize` rewrites the forms archivists actually type
//! - `05/1978`, `1970-1975`, `1970s`, `c. 1970`, `May 1975` - into EDTF so the
//!   precheck can offer a fix instead of just failing the row.

fn is_digits(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_digit())
}

/// Digits with `X` allowed for unspecified positions (level 1).
fn is_digits_x(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_digit() || b == b'X')
}

fn strip_qualifier(s: &str) -> &str {
    s.trim_end_matches(['?', '~', '%'])
}

fn valid_year(y: &str) -> bool {
    let y = y.strip_prefix('-').unwrap_or(y);
    // Level 1 long years: Y170000002
    if let Some(long) = y.strip_prefix('Y') {
        return long.len() > 4 && long.bytes().all(|b| b.is_ascii_digit());
    }
    is_digits_x(y, 4)
}

fn valid_month(m: &str) -> bool {
    if m == "XX" {
        return true;
    }
    if !is_digits_x(m, 2) {
        return false;
    }
    if m.contains('X') {
        return true;
    }
    let n: u32 = m.parse().unwrap_or(0);
    (1..=12).contains(&n) || (21..=24).contains(&n) // 21-24 are seasons
}

fn valid_day(d: &str) -> bool {
    if !is_digits_x(d, 2) {
        return false;
    }
    if d.contains('X') {
        return true;
    }
    let n: u32 = d.parse().unwrap_or(0);
    (1..=31).contains(&n)
}

fn valid_date(s: &str) -> bool {
    let s = strip_qualifier(s);
    if s.is_empty() {
        return false;
    }
    // Leading '-' is a negative year; split the rest on '-'.
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => ("-", b),
        None => ("", s),
    };
    let parts: Vec<&str> = body.split('-').collect();
    let year = format!("{neg}{}", parts[0]);
    match parts.len() {
        1 => valid_year(&year),
        2 => valid_year(&year) && valid_month(parts[1]),
        3 => {
            let m = parts[1];
            valid_year(&year)
                && valid_month(m)
                && !(21..=24).contains(&m.parse::<u32>().unwrap_or(0))
                && valid_day(parts[2])
        }
        _ => false,
    }
}

/// Date-only EDTF (no times - Islandora's EDTF fields are configured date-only).
pub fn is_valid(value: &str) -> bool {
    let v = value.trim();
    if v.is_empty() {
        return true;
    }
    // Sets: [a,b..c] (one of) and {a,b} (all of)
    if (v.starts_with('[') && v.ends_with(']')) || (v.starts_with('{') && v.ends_with('}')) {
        let inner = &v[1..v.len() - 1];
        return !inner.is_empty()
            && inner.split(',').all(|p| {
                let p = p.trim();
                match p.split_once("..") {
                    Some((a, b)) => {
                        (a.is_empty() || valid_date(a)) && (b.is_empty() || valid_date(b)) && !(a.is_empty() && b.is_empty())
                    }
                    None => valid_date(p),
                }
            });
    }
    if let Some((a, b)) = v.split_once('/') {
        let side = |x: &str| x.is_empty() || x == ".." || valid_date(x);
        return side(a) && side(b) && !(a.is_empty() && b.is_empty());
    }
    valid_date(v)
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

fn month_number(word: &str) -> Option<u32> {
    let w = word.trim_end_matches('.').to_ascii_lowercase();
    if w.len() < 3 {
        return None;
    }
    MONTHS
        .iter()
        .position(|m| w.starts_with(m))
        .map(|i| i as u32 + 1)
}

/// An EDTF rewrite of a common non-EDTF date, or None if it isn't recognised.
/// Returns `Some("")` for "no date" spellings like `n.d.` / `undated`.
pub fn normalize(value: &str) -> Option<String> {
    let v = value.trim();
    if v.is_empty() || is_valid(v) {
        return None;
    }
    let lower = v.to_ascii_lowercase();
    if matches!(lower.as_str(), "n.d." | "nd" | "n.d" | "undated" | "unknown" | "no date") {
        return Some(String::new());
    }

    // c. 1970 / ca. 1970 / circa 1970 -> 1970~
    for prefix in ["circa ", "circa", "ca. ", "ca.", "ca ", "c. ", "c.", "c "] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let rest = rest.trim();
            let inner = normalize(rest).unwrap_or_else(|| rest.to_string());
            if is_valid(&inner) && !inner.contains('/') && !inner.is_empty() {
                return Some(format!("{}~", strip_qualifier(&inner)));
            }
        }
    }

    // 1970s -> 197X ; 1900s -> 19XX
    if let Some(dec) = lower.strip_suffix('s').or_else(|| lower.strip_suffix("'s")) {
        if is_digits(dec, 4) {
            return Some(if dec.ends_with("00") {
                format!("{}XX", &dec[..2])
            } else if dec.ends_with('0') {
                format!("{}X", &dec[..3])
            } else {
                dec.to_string()
            });
        }
    }

    // 1970-1975 / 1970 - 1975 / 1970–1975 -> 1970/1975
    let dashed = v.replace(['–', '—'], "-");
    if let Some((a, b)) = dashed.split_once('-') {
        let (a, b) = (a.trim(), b.trim());
        if is_digits(a, 4) && is_digits(b, 4) {
            return Some(format!("{a}/{b}"));
        }
    }

    // MM/YYYY -> YYYY-MM ; YYYY/MM/DD and YYYY.MM.DD -> YYYY-MM-DD
    let parts: Vec<&str> = v.split(['/', '.']).map(str::trim).collect();
    match parts.as_slice() {
        [m, y] if is_digits(y, 4) && (m.len() == 1 || m.len() == 2) && m.bytes().all(|b| b.is_ascii_digit()) => {
            let cand = format!("{y}-{:02}", m.parse::<u32>().ok()?);
            if is_valid(&cand) {
                return Some(cand);
            }
        }
        [y, m, d] if is_digits(y, 4) => {
            let cand = format!(
                "{y}-{:02}-{:02}",
                m.parse::<u32>().ok()?,
                d.parse::<u32>().ok()?
            );
            if is_valid(&cand) {
                return Some(cand);
            }
        }
        _ => {}
    }

    // "May 1975", "May 5, 1975", "5 May 1975"
    let words: Vec<&str> = v
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .collect();
    match words.as_slice() {
        [m, y] if is_digits(y, 4) => {
            if let Some(mn) = month_number(m) {
                return Some(format!("{y}-{mn:02}"));
            }
        }
        [m, d, y] if is_digits(y, 4) && month_number(m).is_some() => {
            let cand = format!("{y}-{:02}-{:02}", month_number(m)?, d.parse::<u32>().ok()?);
            if is_valid(&cand) {
                return Some(cand);
            }
        }
        [d, m, y] if is_digits(y, 4) && month_number(m).is_some() => {
            let cand = format!("{y}-{:02}-{:02}", month_number(m)?, d.parse::<u32>().ok()?);
            if is_valid(&cand) {
                return Some(cand);
            }
        }
        _ => {}
    }
    None
}

/// Year (and month, when the date has one) of a simple EDTF date, for
/// summarising a component's span from its items.
pub fn year_month(value: &str) -> Option<(i32, Option<u32>)> {
    let v = strip_qualifier(value.trim());
    let first = v.split('/').next()?;
    let mut parts = first.split('-');
    let y: i32 = parts.next()?.parse().ok()?;
    let m = parts.next().and_then(|m| m.parse::<u32>().ok()).filter(|m| (1..=12).contains(m));
    Some((y, m))
}

/// A component's date from its items' dates: the shared month or year if they
/// all agree, otherwise the year span as an EDTF interval.
pub fn span(dates: &[&str]) -> Option<String> {
    let parsed: Vec<(i32, Option<u32>)> = dates.iter().filter_map(|d| year_month(d)).collect();
    if parsed.is_empty() {
        return None;
    }
    let min = parsed.iter().map(|p| p.0).min()?;
    let max = parsed.iter().map(|p| p.0).max()?;
    if min != max {
        return Some(format!("{min:04}/{max:04}"));
    }
    let first_m = parsed[0].1;
    if first_m.is_some() && parsed.iter().all(|p| p.1 == first_m) {
        return Some(format!("{min:04}-{:02}", first_m?));
    }
    Some(format!("{min:04}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_edtf() {
        for v in [
            "1978", "1978-05", "1978-05-17", "1978?", "1978~", "1978-05%", "197X", "19XX",
            "1978-XX", "1978/1982", "1978-05/..", "../1980", "/1980", "1978-21", "[1978,1980]",
            "{1978..1980}", "-0100",
        ] {
            assert!(is_valid(v), "{v} should be valid");
        }
    }

    #[test]
    fn rejects_non_edtf() {
        for v in ["05/1978", "1978-13", "1978-05-32", "c. 1970", "1970s", "May 1975", "78", "1978-21-01"] {
            assert!(!is_valid(v), "{v} should be invalid");
        }
    }

    #[test]
    fn normalizes_common_forms() {
        let cases = [
            ("05/1978", "1978-05"),
            ("5/1978", "1978-05"),
            ("1970-1975", "1970/1975"),
            ("1970 – 1975", "1970/1975"),
            ("1970s", "197X"),
            ("1900s", "19XX"),
            ("c. 1970", "1970~"),
            ("circa 1970s", "197X~"),
            ("May 1975", "1975-05"),
            ("May 5, 1975", "1975-05-05"),
            ("5 May 1975", "1975-05-05"),
            ("1975/05/05", "1975-05-05"),
            ("n.d.", ""),
        ];
        for (input, want) in cases {
            assert_eq!(normalize(input).as_deref(), Some(want), "{input}");
        }
        assert_eq!(normalize("1978"), None);
        assert_eq!(normalize("garbage"), None);
    }

    #[test]
    fn spans() {
        assert_eq!(span(&["1978-05-01", "1978-05"]).as_deref(), Some("1978-05"));
        assert_eq!(span(&["1978-05", "1978-06"]).as_deref(), Some("1978"));
        assert_eq!(span(&["1975", "1982~"]).as_deref(), Some("1975/1982"));
        assert_eq!(span(&[]), None);
    }
}
