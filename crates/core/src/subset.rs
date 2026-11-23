//! Ingest part of a sheet, picked like PDF pages - "12", "12,15", "-40",
//! "100-", "12-40" - or by identifier ("2024_19_1545"). The rows that describe the chosen items' collections and
//! series are always kept, so titles and the hierarchy still work.

use crate::source::Table;
use crate::tree::{parent_of, ID_COLUMNS};
use anyhow::{bail, Result};
use std::collections::HashSet;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subset {
    /// Identifiers whose whole subtree is kept.
    pub ids: Vec<String>,
    /// Sheet row ranges (1-based, header = 1), inclusive.
    pub rows: Vec<(usize, usize)>,
}

impl Subset {
    /// Parse page-style rows - "12", "12,15", "-40", "100-", "12-40" - and/or
    /// identifiers ("2024_19_1545"), comma-separated. A row is picked when any
    /// of them picks it. Row numbers are the
    /// sheet's own (row 1 is the header). Empty = the whole sheet (None).
    pub fn parse(spec: &str) -> Result<Option<Self>> {
        let mut s = Subset::default();
        let num = |t: &str, tok: &str| -> Result<usize> {
            match t.trim().parse::<usize>() {
                Ok(n) if n >= 2 => Ok(n),
                Ok(_) => bail!("'{tok}': rows start at 2 (row 1 is the header)"),
                Err(_) => bail!("'{tok}' isn't a row (12), a range (12-40, -40, 100-) or an identifier (2024_19_1545)"),
            }
        };
        for tok in spec.split([',', ' ', ';']).map(str::trim).filter(|t| !t.is_empty()) {
            if tok.contains('_') {
                s.ids.push(tok.to_string());
            } else if let Some((a, b)) = tok.split_once('-') {
                let from = if a.trim().is_empty() { 2 } else { num(a, tok)? };
                let to = if b.trim().is_empty() { usize::MAX } else { num(b, tok)? };
                if from > to {
                    bail!("'{tok}': the range runs backwards");
                }
                s.rows.push((from, to));
            } else {
                let n = num(tok, tok)?;
                s.rows.push((n, n));
            }
        }
        Ok((s != Subset::default()).then_some(s))
    }

    /// The short form shown on screen and in the report.
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = self
            .rows
            .iter()
            .map(|&(a, b)| match (a, b) {
                (a, b) if a == b => format!("row {a}"),
                (a, usize::MAX) => format!("rows {a}-end"),
                (a, b) => format!("rows {a}-{b}"),
            })
            .collect();
        parts.extend(self.ids.iter().cloned());
        parts.join(", ")
    }

    /// Keep only the chosen rows (plus their ancestors' rows). Returns how
    /// many items were picked; errors when nothing matches.
    pub fn apply(&self, table: &mut Table) -> Result<usize> {
        let Some(id_col) = ID_COLUMNS.iter().find_map(|c| table.col(c)) else {
            bail!("the sheet has no identifier column ({}), so a subset can't be picked", ID_COLUMNS.join(" / "));
        };
        let id_of = |i: usize| table.rows[i].cells.get(id_col).map(|c| c.trim().to_string()).unwrap_or_default();
        let ids: Vec<String> = (0..table.rows.len()).map(id_of).collect();
        // A container is anything another row sits under.
        let all: HashSet<&str> = ids.iter().map(String::as_str).filter(|s| !s.is_empty()).collect();
        let containers: HashSet<String> = all.iter().filter_map(|id| parent_of(id)).flat_map(ancestors_and_self).collect();
        let is_item = |id: &str| !id.is_empty() && !containers.contains(id);

        let under = |id: &str, root: &str| id == root || id.starts_with(&format!("{root}_"));
        let picked: Vec<usize> = (0..table.rows.len())
            .filter(|&i| {
                let line = table.rows[i].line;
                // Either picks a row: "2024_19_2, 40-" is that subtree and
                // everything from row 40, like pages in a print dialog.
                self.ids.iter().any(|r| under(&ids[i], r)) || self.rows.iter().any(|(a, b)| (*a..=*b).contains(&line))
            })
            .collect();
        let items: HashSet<&str> = picked.iter().map(|&i| ids[i].as_str()).filter(|id| is_item(id)).collect();
        if items.is_empty() {
            bail!("no items match the subset \"{}\"", self.describe());
        }
        // Containers only come along when something under them was picked.
        let needed: HashSet<String> = items.iter().filter_map(|id| parent_of(id)).flat_map(ancestors_and_self).collect();
        let keep: HashSet<usize> = (0..table.rows.len())
            .filter(|&i| {
                let id = ids[i].as_str();
                (picked.contains(&i) && is_item(id)) || needed.contains(id)
            })
            .collect();
        table.subset = Some(self.describe());
        let before = table.rows.len();
        let mut i = 0;
        table.rows.retain(|_| {
            let k = keep.contains(&i);
            i += 1;
            k
        });
        table.notes.push((
            false,
            0,
            format!(
                "subset \"{}\": {} item(s), {} of {} row(s) kept (with their collection/series rows)",
                self.describe(),
                items.len(),
                table.rows.len(),
                before
            ),
        ));
        Ok(items.len())
    }
}

fn ancestors_and_self(id: String) -> Vec<String> {
    let mut v = vec![id.clone()];
    let mut cur = id;
    while let Some(p) = parent_of(&cur) {
        v.push(p.clone());
        cur = p;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Row;

    fn table(ids: &[&str]) -> Table {
        Table {
            source: "t".into(),
            headers: vec!["accessIdentifier".into()],
            rows: ids.iter().enumerate().map(|(i, id)| Row { line: i + 2, cells: vec![id.to_string()] }).collect(),
            renamed: vec![],
            cells_repaired: 0,
            notes: vec![],
            headings: Default::default(),
            subset: None,
        }
    }

    fn kept(t: &Table) -> Vec<String> {
        t.rows.iter().map(|r| r.cells[0].clone()).collect()
    }

    const SHEET: &[&str] = &["2024_19", "2024_19_1", "2024_19_1_001", "2024_19_1_002", "2024_19_2", "2024_19_2_001", "2024_19_2_002"];

    #[test]
    fn parses() {
        assert_eq!(Subset::parse("  ").unwrap(), None);
        let s = Subset::parse("3, 5-6,-4 ,7-, 2024_19_2").unwrap().unwrap();
        assert_eq!(s.rows, [(3, 3), (5, 6), (2, 4), (7, usize::MAX)]);
        assert_eq!(s.ids, ["2024_19_2"]);
        assert_eq!(s.describe(), "row 3, rows 5-6, rows 2-4, rows 7-end, 2024_19_2");
        assert!(Subset::parse("1").is_err());
        assert!(Subset::parse("9-4").is_err());
        assert!(Subset::parse("hello").is_err());
    }

    #[test]
    fn subtree_keeps_its_parents() {
        let mut t = table(SHEET);
        assert_eq!(Subset::parse("2024_19_2").unwrap().unwrap().apply(&mut t).unwrap(), 2);
        assert_eq!(kept(&t), ["2024_19", "2024_19_2", "2024_19_2_001", "2024_19_2_002"]);
    }

    #[test]
    fn rows_bring_their_parents() {
        // Row 5 is 2024_19_1_002; the series row (3) and collection row (2) come along.
        let mut t = table(SHEET);
        Subset::parse("5").unwrap().unwrap().apply(&mut t).unwrap();
        assert_eq!(kept(&t), ["2024_19", "2024_19_1", "2024_19_1_002"]);
        // Open-ended: row 7 on is 2024_19_2_001 and _002.
        let mut t = table(SHEET);
        assert_eq!(Subset::parse("7-").unwrap().unwrap().apply(&mut t).unwrap(), 2);
        assert_eq!(kept(&t), ["2024_19", "2024_19_2", "2024_19_2_001", "2024_19_2_002"]);
        let mut t = table(SHEET);
        assert_eq!(Subset::parse("-4").unwrap().unwrap().apply(&mut t).unwrap(), 1);
        assert_eq!(kept(&t), ["2024_19", "2024_19_1", "2024_19_1_001"]);
    }

    #[test]
    fn ids_and_rows_add_up() {
        // 2024_19_1's items, plus row 8 (2024_19_2_002).
        let mut t = table(SHEET);
        assert_eq!(Subset::parse("2024_19_1, 8").unwrap().unwrap().apply(&mut t).unwrap(), 3);
        assert_eq!(kept(&t), ["2024_19", "2024_19_1", "2024_19_1_001", "2024_19_1_002", "2024_19_2", "2024_19_2_002"]);
    }

    #[test]
    fn nothing_matching_is_an_error() {
        let mut t = table(SHEET);
        assert!(Subset::parse("2024_20").unwrap().unwrap().apply(&mut t).is_err());
    }
}
