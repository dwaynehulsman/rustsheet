//! Find and replace within a sheet.

use crate::addr::CellRef;
use crate::input::{Content, edit_text, parse_input};
use crate::sheet::{CellState, Sheet};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FindOptions {
    pub match_case: bool,
    /// The whole cell must equal the query, not just contain it.
    pub whole_cell: bool,
    /// Search the values shown instead of what was typed (formulas).
    pub in_values: bool,
}

fn haystack(sheet: &Sheet, at: CellRef, opts: FindOptions) -> String {
    let Some(cell) = sheet.cell(at) else { return String::new() };
    if opts.in_values { sheet.display(at) } else { edit_text(&cell.content, cell.format.num) }
}

fn fold(s: &str, opts: FindOptions) -> String {
    if opts.match_case { s.to_string() } else { s.to_lowercase() }
}

fn hit(text: &str, query: &str, opts: FindOptions) -> bool {
    let (text, query) = (fold(text, opts), fold(query, opts));
    if opts.whole_cell { text == query } else { text.contains(&query) }
}

/// Every cell matching `query`, row by row like Excel.
pub fn find_all(sheet: &Sheet, query: &str, opts: FindOptions) -> Vec<CellRef> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<CellRef> = sheet
        .cells()
        .filter(|(_, c)| !c.content.is_empty())
        .map(|(at, _)| at)
        .filter(|&at| hit(&haystack(sheet, at, opts), query, opts))
        .collect();
    found.sort_by_key(|at| (at.row, at.col));
    found
}

/// The first match after (or, backwards, before) `from`, wrapping around.
pub fn find_next(
    sheet: &Sheet,
    query: &str,
    opts: FindOptions,
    from: CellRef,
    backwards: bool,
) -> Option<CellRef> {
    let all = find_all(sheet, query, opts);
    let key = (from.row, from.col);
    if backwards {
        all.iter().rev().find(|a| (a.row, a.col) < key).or(all.last()).copied()
    } else {
        all.iter().find(|a| (a.row, a.col) > key).or(all.first()).copied()
    }
}

/// `text` with matches of `query` replaced, ignoring case unless asked.
fn replace_in(text: &str, query: &str, with: &str, opts: FindOptions) -> String {
    if opts.whole_cell {
        return with.to_string();
    }
    if opts.match_case {
        return text.replace(query, with);
    }
    let lower = text.to_lowercase();
    let q = query.to_lowercase();
    // Lowercasing can change byte lengths for some scripts; fall back to a
    // char-by-char comparison so indices stay valid.
    if lower.len() != text.len() || q.len() != query.len() {
        return text.replace(query, with);
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (i, _) in lower.match_indices(&q) {
        out.push_str(&text[last..i]);
        out.push_str(with);
        last = i + q.len();
    }
    out.push_str(&text[last..]);
    out
}

/// New states for replacing `query` with `with` in the given cells. Cells
/// whose replaced text would not parse (a broken formula) are skipped.
pub fn replace(
    sheet: &Sheet,
    cells: &[CellRef],
    query: &str,
    with: &str,
    opts: FindOptions,
) -> Vec<(CellRef, CellState)> {
    cells
        .iter()
        .filter_map(|&at| {
            let cell = sheet.cell(at)?;
            let text = edit_text(&cell.content, cell.format.num);
            if !hit(&text, query, opts) {
                return None;
            }
            let parsed = parse_input(&replace_in(&text, query, with, opts)).ok()?;
            let content = if parsed.content.is_empty() { Content::Empty } else { parsed.content };
            Some((at, CellState { content, format: cell.format }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbook::Workbook;

    fn book() -> Workbook {
        let mut b = Workbook::new();
        for (at, text) in [
            ("A1", "Appel"),
            ("B1", "appeltaart"),
            ("A2", "Peer"),
            ("B2", "=SUM(A3:A4)"),
            ("A3", "1"),
            ("A4", "2"),
        ] {
            let content = parse_input(text).unwrap().content;
            b.set_states(0, vec![(CellRef::parse(at).unwrap(), CellState { content, ..Default::default() })]);
        }
        b
    }

    fn names(cells: &[CellRef]) -> Vec<String> {
        cells.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn finds_in_row_order_with_options() {
        let b = book();
        let s = b.sheet(0);
        let o = FindOptions::default();
        assert_eq!(names(&find_all(s, "appel", o)), ["A1", "B1"]);
        assert_eq!(names(&find_all(s, "appel", FindOptions { match_case: true, ..o })), ["B1"]);
        assert_eq!(names(&find_all(s, "appel", FindOptions { whole_cell: true, ..o })), ["A1"]);
        assert_eq!(names(&find_all(s, "sum", o)), ["B2"]);
        assert_eq!(names(&find_all(s, "3", FindOptions { in_values: true, ..o })), ["B2"]);
        assert_eq!(find_next(s, "appel", o, CellRef::parse("B1").unwrap(), false), CellRef::parse("A1"));
        assert_eq!(find_next(s, "appel", o, CellRef::parse("A1").unwrap(), true), CellRef::parse("B1"));
    }

    #[test]
    fn replaces_text_and_formulas() {
        let mut b = book();
        let o = FindOptions::default();
        let found = find_all(b.sheet(0), "appel", o);
        let changes = replace(b.sheet(0), &found, "appel", "Peer", o);
        b.set_states(0, changes);
        assert_eq!(b.sheet(0).display(CellRef::parse("A1").unwrap()), "Peer");
        assert_eq!(b.sheet(0).display(CellRef::parse("B1").unwrap()), "Peertaart");

        let found = find_all(b.sheet(0), "A3:", o);
        let changes = replace(b.sheet(0), &found, "A3:", "A4:", o);
        b.set_states(0, changes);
        assert_eq!(b.sheet(0).display(CellRef::parse("B2").unwrap()), "2");
        // A replacement that breaks a formula is skipped.
        let found = find_all(b.sheet(0), "SUM(", o);
        assert!(replace(b.sheet(0), &found, "SUM(", "SUM((", o).is_empty());
    }
}
