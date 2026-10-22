//! Copy, cut and paste: internal blocks that keep formulas, and
//! tab-separated text for the system clipboard (Excel, Numbers).

use crate::addr::{CellRef, Range};
use crate::input::{Content, parse_literal};
use crate::sheet::{CellState, Sheet};

/// A copied rectangle of cells.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// Where the block was copied from; relative references shift by the
    /// distance from here to the paste target.
    pub origin: CellRef,
    pub rows: u32,
    pub cols: u32,
    /// Non-default cells as (row offset, column offset, state).
    pub cells: Vec<(u32, u32, CellState)>,
}

impl Block {
    /// Copies the cells in `range`, trimmed to the used part of the sheet.
    pub fn copy(sheet: &Sheet, range: Range) -> Block {
        let range = trim_to_used(sheet, range);
        let cells = sheet
            .cells_in(range)
            .map(|(at, cell)| {
                let state = CellState { content: cell.content.clone(), format: cell.format };
                (at.row - range.start.row, at.col - range.start.col, state)
            })
            .collect();
        Block { origin: range.start, rows: range.rows(), cols: range.cols(), cells }
    }

    /// Builds a block from pasted text. Values are interpreted like typed
    /// input, except that text starting with `=` is not turned into a formula.
    pub fn from_tsv(text: &str) -> Block {
        let rows = parse_tsv(text);
        let height = rows.len().max(1) as u32;
        let width = rows.iter().map(Vec::len).max().unwrap_or(1).max(1) as u32;
        let mut cells = Vec::new();
        for (r, row) in rows.into_iter().enumerate() {
            for (c, field) in row.into_iter().enumerate() {
                let parsed = parse_literal(&field);
                if parsed.content.is_empty() {
                    continue;
                }
                let format = crate::format::CellFormat {
                    num: parsed.format.unwrap_or_default(),
                    ..Default::default()
                };
                cells.push((r as u32, c as u32, CellState { content: parsed.content, format }));
            }
        }
        Block { origin: CellRef::default(), rows: height, cols: width, cells }
    }

    /// The states to apply when pasting at `target`. When the selection is
    /// an exact multiple of the block size the block is tiled to fill it,
    /// like Excel. Relative references shift unless `keep_refs` is set,
    /// which is what a cut-and-paste wants.
    pub fn paste(&self, sheet: &Sheet, target: Range, keep_refs: bool) -> (Range, Vec<(CellRef, CellState)>) {
        let tiles_down = if target.rows().is_multiple_of(self.rows) { target.rows() / self.rows } else { 1 };
        let tiles_across =
            if target.cols().is_multiple_of(self.cols) { target.cols() / self.cols } else { 1 };
        let start = target.start;
        let end = start
            .offset_clamped(i64::from(self.rows * tiles_down) - 1, i64::from(self.cols * tiles_across) - 1);
        let area = Range::new(start, end);

        let mut out: Vec<(CellRef, CellState)> = Vec::new();
        // Clear what is there first; later entries for the same cell win.
        for (at, _) in sheet.cells_in(area) {
            out.push((at, CellState::default()));
        }
        for ty in 0..tiles_down {
            for tx in 0..tiles_across {
                let tile = CellRef::new(start.row + ty * self.rows, start.col + tx * self.cols);
                let drow = i64::from(tile.row) - i64::from(self.origin.row);
                let dcol = i64::from(tile.col) - i64::from(self.origin.col);
                for (r, c, state) in &self.cells {
                    let Some(at) = tile.offset(i64::from(*r), i64::from(*c)) else { continue };
                    let content = match &state.content {
                        Content::Formula(e) if !keep_refs => Content::Formula(e.shifted(drow, dcol)),
                        other => other.clone(),
                    };
                    out.push((at, CellState { content, format: state.format }));
                }
            }
        }
        (area, dedup_last_wins(out))
    }
}

/// Keeps only the last state per cell, preserving first-seen order.
fn dedup_last_wins(changes: Vec<(CellRef, CellState)>) -> Vec<(CellRef, CellState)> {
    let mut seen = rustc_hash::FxHashMap::default();
    let mut out: Vec<(CellRef, CellState)> = Vec::with_capacity(changes.len());
    for (at, state) in changes {
        match seen.get(&at) {
            Some(&i) => out[i] = (at, state),
            None => {
                seen.insert(at, out.len());
                out.push((at, state));
            }
        }
    }
    out
}

fn trim_to_used(sheet: &Sheet, range: Range) -> Range {
    match sheet.used_range() {
        Some(used) => Range::new(
            range.start,
            CellRef::new(
                range.end.row.min(used.end.row.max(range.start.row)),
                range.end.col.min(used.end.col.max(range.start.col)),
            ),
        ),
        None => Range::single(range.start),
    }
}

/// Displayed values of `range` as tab-separated text, the format Excel and
/// Numbers put on the clipboard.
pub fn to_tsv(sheet: &Sheet, range: Range) -> String {
    let range = trim_to_used(sheet, range);
    let mut out = String::new();
    for row in range.start.row..=range.end.row {
        for col in range.start.col..=range.end.col {
            if col > range.start.col {
                out.push('\t');
            }
            let text = sheet.display(CellRef::new(row, col));
            if text.contains(['\t', '\n', '\r', '"']) {
                out.push('"');
                out.push_str(&text.replace('"', "\"\""));
                out.push('"');
            } else {
                out.push_str(&text);
            }
        }
        out.push('\n');
    }
    out
}

/// Splits tab-separated text into rows of fields. Fields may be quoted with
/// `"` to hold tabs, newlines and doubled quotes; one trailing line break is
/// ignored.
pub fn parse_tsv(text: &str) -> Vec<Vec<String>> {
    let text = text.strip_suffix("\r\n").or_else(|| text.strip_suffix('\n')).unwrap_or(text);
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut at_field_start = true;
    while let Some(c) = chars.next() {
        match c {
            '"' if at_field_start => {
                // Quoted field: read to the closing quote.
                while let Some(q) = chars.next() {
                    if q == '"' {
                        if chars.peek() == Some(&'"') {
                            chars.next();
                            field.push('"');
                        } else {
                            break;
                        }
                    } else {
                        field.push(q);
                    }
                }
                at_field_start = false;
            }
            '\t' => {
                row.push(std::mem::take(&mut field));
                at_field_start = true;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                at_field_start = true;
            }
            _ => {
                field.push(c);
                at_field_start = false;
            }
        }
    }
    row.push(field);
    rows.push(row);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::parse_input;

    fn c(s: &str) -> CellRef {
        CellRef::parse(s).unwrap()
    }

    fn set(sheet: &mut Sheet, at: &str, text: &str) {
        let p = parse_input(text).unwrap();
        let format = crate::format::CellFormat { num: p.format.unwrap_or_default(), ..Default::default() };
        sheet.set_states(vec![(c(at), CellState { content: p.content, format })]);
    }

    fn source(sheet: &Sheet, at: &str) -> String {
        sheet.state(c(at)).content.source()
    }

    #[test]
    fn paste_shifts_relative_references() {
        let mut s = Sheet::new("Blad1");
        set(&mut s, "A1", "1");
        set(&mut s, "A2", "2");
        set(&mut s, "B1", "=A1*$A$1+A$2+$A2");
        let block = Block::copy(&s, Range::single(c("B1")));
        let (_, changes) = block.paste(&s, Range::single(c("C3")), false);
        s.set_states(changes);
        assert_eq!(source(&s, "C3"), "=B3*$A$1+B$2+$A4");
    }

    #[test]
    fn paste_off_the_grid_gives_ref_error() {
        let mut s = Sheet::new("Blad1");
        set(&mut s, "B2", "=A1+SUM(A1:A2)");
        let block = Block::copy(&s, Range::single(c("B2")));
        let (_, changes) = block.paste(&s, Range::single(c("A1")), false);
        s.set_states(changes);
        assert_eq!(source(&s, "A1"), "=#REF!+SUM(#REF!)");
        assert_eq!(s.display(c("A1")), "#REF!");
    }

    #[test]
    fn cut_keeps_references_and_tiles_fill_selection() {
        let mut s = Sheet::new("Blad1");
        set(&mut s, "A1", "5");
        set(&mut s, "B1", "=A1+1");
        let block = Block::copy(&s, Range::single(c("B1")));
        let (_, changes) = block.paste(&s, Range::single(c("D4")), true);
        s.set_states(changes);
        assert_eq!(source(&s, "D4"), "=A1+1");

        let (area, changes) = block.paste(&s, Range::new(c("F1"), c("F3")), false);
        assert_eq!(area.to_string(), "F1:F3");
        s.set_states(changes);
        assert_eq!(source(&s, "F3"), "=E3+1");
    }

    #[test]
    fn paste_clears_cells_the_block_leaves_empty() {
        let mut s = Sheet::new("Blad1");
        set(&mut s, "A1", "1");
        set(&mut s, "C1", "old");
        set(&mut s, "D1", "keep");
        let block = Block::copy(&s, Range::new(c("A1"), c("B1")));
        let (_, changes) = block.paste(&s, Range::single(c("B1")), false);
        s.set_states(changes);
        assert_eq!(s.display(c("B1")), "1");
        assert_eq!(s.display(c("C1")), "");
        assert_eq!(s.display(c("D1")), "keep");
    }

    #[test]
    fn tsv_round_trip_with_quotes() {
        let rows = parse_tsv("a\tb\r\n\"x\ty\"\t\"say \"\"hi\"\"\"\n\"multi\nline\"\t3\n");
        assert_eq!(
            rows,
            vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["x\ty".to_string(), "say \"hi\"".to_string()],
                vec!["multi\nline".to_string(), "3".to_string()],
            ]
        );

        let mut s = Sheet::new("Blad1");
        set(&mut s, "A1", "x\ty");
        set(&mut s, "B1", "=2*3");
        set(&mut s, "A2", "say \"hi\"");
        let tsv = to_tsv(&s, Range::new(c("A1"), c("C9")));
        assert_eq!(tsv, "\"x\ty\"\t6\n\"say \"\"hi\"\"\"\t\n");
        assert_eq!(parse_tsv(&tsv), vec![vec!["x\ty", "6"], vec!["say \"hi\"", ""]]);
    }

    #[test]
    fn tsv_from_excel_becomes_values() {
        let block = Block::from_tsv("Naam\tBedrag\nKoffie\t€ 1,250.50\nThee\t12%\n");
        let mut s = Sheet::new("Blad1");
        let (area, changes) = block.paste(&s, Range::single(c("B2")), false);
        assert_eq!(area.to_string(), "B2:C4");
        s.set_states(changes);
        assert_eq!(s.value(c("C3")), &crate::value::Value::Number(1250.5));
        assert_eq!(s.display(c("C3")), "€ 1,250.50");
        assert_eq!(s.display(c("C4")), "12.00%");
        assert_eq!(s.display(c("B2")), "Naam");
    }
}
