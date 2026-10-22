//! Autofilter: a header row with a list of allowed values per column;
//! data rows with any other value are hidden.

use std::collections::{BTreeMap, BTreeSet};

use crate::addr::{CellRef, Range};
use crate::sheet::Sheet;

#[derive(Clone, Debug, PartialEq)]
pub struct AutoFilter {
    /// Header row plus the data rows below it.
    pub range: Range,
    /// Per column, the displayed values that stay visible. Columns without
    /// an entry show everything. An empty cell is the empty string.
    pub criteria: BTreeMap<u32, BTreeSet<String>>,
}

impl AutoFilter {
    pub fn new(range: Range) -> Self {
        Self { range, criteria: BTreeMap::new() }
    }

    fn data_rows(&self) -> std::ops::RangeInclusive<u32> {
        self.range.start.row + 1..=self.range.end.row
    }

    /// The data rows the criteria hide.
    pub fn hidden_rows(&self, sheet: &Sheet) -> BTreeSet<u32> {
        self.data_rows()
            .filter(|&row| {
                self.criteria
                    .iter()
                    .any(|(&col, allowed)| !allowed.contains(&sheet.display(CellRef::new(row, col))))
            })
            .collect()
    }

    /// Distinct displayed values in a column of the data rows, in sort order
    /// (numbers by value, then text), empty last.
    pub fn values(&self, sheet: &Sheet, col: u32) -> Vec<String> {
        let mut seen: BTreeMap<String, crate::value::Value> = BTreeMap::new();
        for row in self.data_rows() {
            let at = CellRef::new(row, col);
            seen.entry(sheet.display(at)).or_insert_with(|| sheet.value(at).clone());
        }
        let mut values: Vec<(String, crate::value::Value)> = seen.into_iter().collect();
        values.sort_by(|a, b| match (a.0.is_empty(), b.0.is_empty()) {
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => a.1.compare(&b.1).then_with(|| a.0.cmp(&b.0)),
        });
        values.into_iter().map(|(text, _)| text).collect()
    }

    pub fn is_filtered(&self, col: u32) -> bool {
        self.criteria.contains_key(&col)
    }

    /// The same filter after rows or columns moved (see `Sheet::shift_lines`);
    /// `None` when its header row was deleted.
    pub(crate) fn shifted(&self, rows: bool, at: u32, delta: i64) -> Option<AutoFilter> {
        let moved = |i: u32| -> Option<u32> {
            let removed = delta < 0 && i64::from(i) >= i64::from(at) + delta && i < at;
            let target = if i >= at { i64::from(i) + delta } else { i64::from(i) };
            (!removed && target >= 0).then_some(target as u32)
        };
        let mut f = self.clone();
        if rows {
            let start = moved(f.range.start.row)?;
            let end = moved(f.range.end.row)
                .unwrap_or_else(|| (i64::from(at) + delta - 1).max(i64::from(start)) as u32);
            f.range = Range::new(
                CellRef::new(start, f.range.start.col),
                CellRef::new(end.max(start), f.range.end.col),
            );
        } else {
            let start = moved(f.range.start.col).unwrap_or(at.saturating_add_signed(delta as i32));
            let end = moved(f.range.end.col).unwrap_or(start);
            f.range = Range::new(
                CellRef::new(f.range.start.row, start),
                CellRef::new(f.range.end.row, end.max(start)),
            );
            f.criteria = std::mem::take(&mut f.criteria)
                .into_iter()
                .filter_map(|(c, v)| Some((moved(c)?, v)))
                .collect();
        }
        Some(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::parse_input;
    use crate::sheet::CellState;
    use crate::workbook::Workbook;

    fn book() -> Workbook {
        let mut b = Workbook::new();
        let rows =
            [["Naam", "Stad"], ["Anna", "Utrecht"], ["Bram", "Delft"], ["Cem", "Utrecht"], ["Dirk", ""]];
        for (r, row) in rows.iter().enumerate() {
            for (c, text) in row.iter().enumerate() {
                let content = parse_input(text).unwrap().content;
                b.set_states(
                    0,
                    vec![(CellRef::new(r as u32, c as u32), CellState { content, ..Default::default() })],
                );
            }
        }
        b
    }

    #[test]
    fn hides_rows_with_other_values() {
        let b = book();
        let mut f = AutoFilter::new(Range::new(CellRef::new(0, 0), CellRef::new(4, 1)));
        assert_eq!(f.values(b.sheet(0), 1), ["Delft", "Utrecht", ""]);
        assert!(f.hidden_rows(b.sheet(0)).is_empty());
        f.criteria.insert(1, ["Utrecht".to_string()].into_iter().collect());
        assert_eq!(f.hidden_rows(b.sheet(0)), [2, 4].into_iter().collect());
        f.criteria.insert(0, ["Cem".to_string(), "Bram".to_string()].into_iter().collect());
        assert_eq!(f.hidden_rows(b.sheet(0)), [1, 2, 4].into_iter().collect());
    }

    #[test]
    fn follows_inserted_and_deleted_lines() {
        let mut f = AutoFilter::new(Range::new(CellRef::new(2, 1), CellRef::new(9, 3)));
        f.criteria.insert(3, BTreeSet::new());
        let down = f.shifted(true, 0, 2).unwrap();
        assert_eq!(down.range.to_string(), "B5:D12");
        let shorter = f.shifted(true, 6, -2).unwrap();
        assert_eq!(shorter.range.to_string(), "B3:D8");
        assert!(f.shifted(true, 3, -1).is_none());
        let wider = f.shifted(false, 2, 1).unwrap();
        assert_eq!(wider.range.to_string(), "B3:E10");
        assert!(wider.criteria.contains_key(&4));
    }
}
