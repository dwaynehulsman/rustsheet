//! Sorting rows of a range, and finding the block of data around a cell.

use std::cmp::Ordering;

use crate::addr::{CellRef, MAX_COLS, MAX_ROWS, Range};
use crate::input::Content;
use crate::sheet::{CellState, Sheet};
use crate::value::Value;

fn filled(sheet: &Sheet, at: CellRef) -> bool {
    sheet.cell(at).is_some_and(|c| !c.content.is_empty())
}

/// Excel's "current region": the rectangle of data around `at`, bounded by
/// empty rows and columns.
pub fn current_region(sheet: &Sheet, at: CellRef) -> Range {
    let mut r = Range::single(at);
    // Grow while any cell just outside an edge (diagonals included) is filled.
    for _ in 0..10_000 {
        let top = r.start.row.checked_sub(1);
        let bottom = (r.end.row + 1 < MAX_ROWS).then_some(r.end.row + 1);
        let left = r.start.col.checked_sub(1);
        let right = (r.end.col + 1 < MAX_COLS).then_some(r.end.col + 1);
        let c0 = left.unwrap_or(r.start.col);
        let c1 = right.unwrap_or(r.end.col);
        let r0 = top.unwrap_or(r.start.row);
        let r1 = bottom.unwrap_or(r.end.row);
        let row_has = |row: u32| {
            sheet
                .cells_in(Range::new(CellRef::new(row, c0), CellRef::new(row, c1)))
                .any(|(_, c)| !c.content.is_empty())
        };
        let col_has = |col: u32| {
            sheet
                .cells_in(Range::new(CellRef::new(r0, col), CellRef::new(r1, col)))
                .any(|(_, c)| !c.content.is_empty())
        };
        let mut grown = r;
        if let Some(t) = top.filter(|&t| row_has(t)) {
            grown.start.row = t;
        }
        if let Some(b) = bottom.filter(|&b| row_has(b)) {
            grown.end.row = b;
        }
        if let Some(l) = left.filter(|&l| col_has(l)) {
            grown.start.col = l;
        }
        if let Some(rt) = right.filter(|&rt| col_has(rt)) {
            grown.end.col = rt;
        }
        if grown == r {
            break;
        }
        r = grown;
    }
    if r.len() == 1 && !filled(sheet, at) { Range::single(at) } else { r }
}

/// Whether the first row of `range` looks like column headings: all text,
/// while some column has a non-text value right below.
pub fn has_header(sheet: &Sheet, range: Range) -> bool {
    if range.rows() < 2 {
        return false;
    }
    let first = range.start.row;
    let cols = range.start.col..=range.end.col;
    let all_text = cols
        .clone()
        .all(|col| matches!(sheet.value(CellRef::new(first, col)), Value::Text(_) | Value::Empty))
        && cols.clone().any(|col| matches!(sheet.value(CellRef::new(first, col)), Value::Text(_)));
    let bold = cols
        .clone()
        .all(|col| !filled(sheet, CellRef::new(first, col)) || sheet.format(CellRef::new(first, col)).bold);
    let below_differs = cols
        .into_iter()
        .any(|col| matches!(sheet.value(CellRef::new(first + 1, col)), Value::Number(_) | Value::Bool(_)));
    all_text && (below_differs || bold)
}

/// Sort order of values: numbers, text (ignoring case), booleans, errors;
/// empty cells always last, whichever direction.
fn compare(a: &Value, b: &Value, descending: bool) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    let ord = a.compare(b);
    if descending { ord.reverse() } else { ord }
}

/// New states for sorting the rows of `range` by column `key`. With
/// `header` the first row stays put. Formulas move with their row and their
/// relative references shift like a copy, as in Excel.
pub fn sort_rows(
    sheet: &Sheet,
    range: Range,
    key: u32,
    descending: bool,
    header: bool,
) -> Vec<(CellRef, CellState)> {
    // Never sort the empty rows of whole columns.
    let range = match sheet.used_range().and_then(|u| u.intersect(&range)) {
        Some(r) => Range::new(range.start, CellRef::new(r.end.row.max(range.start.row), range.end.col)),
        None => return Vec::new(),
    };
    let first = range.start.row + u32::from(header);
    if first > range.end.row {
        return Vec::new();
    }
    let mut rows: Vec<u32> = (first..=range.end.row).collect();
    rows.sort_by(|&a, &b| {
        compare(sheet.value(CellRef::new(a, key)), sheet.value(CellRef::new(b, key)), descending)
    });
    let mut out = Vec::new();
    for (i, &src) in rows.iter().enumerate() {
        let dest = first + i as u32;
        if dest == src {
            continue;
        }
        let drow = i64::from(dest) - i64::from(src);
        for col in range.start.col..=range.end.col {
            let mut state = sheet.state(CellRef::new(src, col));
            if let Content::Formula(e) = &state.content {
                state.content = Content::Formula(e.shifted(drow, 0));
            }
            out.push((CellRef::new(dest, col), state));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::parse_input;
    use crate::workbook::Workbook;

    fn c(s: &str) -> CellRef {
        CellRef::parse(s).unwrap()
    }

    fn book(rows: &[&[&str]]) -> Workbook {
        let mut b = Workbook::new();
        for (r, row) in rows.iter().enumerate() {
            for (col, text) in row.iter().enumerate() {
                if text.is_empty() {
                    continue;
                }
                let content = parse_input(text).unwrap().content;
                b.set_states(
                    0,
                    vec![(CellRef::new(r as u32, col as u32), CellState { content, ..Default::default() })],
                );
            }
        }
        b
    }

    fn column(b: &Workbook, col: &str, rows: std::ops::RangeInclusive<u32>) -> Vec<String> {
        rows.map(|r| b.sheet(0).display(c(&format!("{col}{r}")))).collect()
    }

    #[test]
    fn sorts_by_a_column_keeping_rows_together() {
        let mut b = book(&[
            &["Naam", "Leeftijd", "Dubbel"],
            &["Sanne", "34", "=B2*2"],
            &["anna", "28", "=B3*2"],
            &["Bram", "", "=B4*2"],
            &["Ceren", "45", "=B5*2"],
        ]);
        let region = current_region(b.sheet(0), c("B3"));
        assert_eq!(region.to_string(), "A1:C5");
        assert!(has_header(b.sheet(0), region));

        let changes = sort_rows(b.sheet(0), region, 1, false, true);
        b.set_states(0, changes);
        assert_eq!(column(&b, "A", 1..=5), ["Naam", "anna", "Sanne", "Ceren", "Bram"]);
        // Formulas moved with their rows and still point at their own row.
        assert_eq!(column(&b, "C", 2..=5), ["56", "68", "90", "0"]);
        assert_eq!(b.sheet(0).state(c("C2")).content.source(), "=B2*2");

        let changes = sort_rows(b.sheet(0), region, 0, true, true);
        b.set_states(0, changes);
        assert_eq!(column(&b, "A", 2..=5), ["Sanne", "Ceren", "Bram", "anna"]);
    }

    #[test]
    fn regions_stop_at_empty_rows_and_columns() {
        let b = book(&[&["1", "2", "", "x"], &["3", "", "", ""], &["", "", "", ""], &["9", "", "", ""]]);
        assert_eq!(current_region(b.sheet(0), c("A1")).to_string(), "A1:B2");
        assert_eq!(current_region(b.sheet(0), c("A4")).to_string(), "A4");
        assert!(!has_header(b.sheet(0), Range::new(c("A1"), c("B2"))));
    }
}
