//! Storage for one worksheet: sparse cells and column widths. Formulas are
//! evaluated by the [`Workbook`](crate::workbook::Workbook) that owns the sheet.

use std::collections::{BTreeMap, BTreeSet};

use crate::addr::{CellRef, MAX_COLS, MAX_ROWS, Range};
use crate::column::Column;
use crate::format::{CellFormat, format_value};
use crate::input::Content;
use crate::value::Value;

/// Stable identity of a sheet within a workbook. Survives renames and
/// reordering, so undo steps and references stay valid.
pub type SheetId = u32;

/// Column width used when none was set, in points.
pub const DEFAULT_COL_WIDTH: f32 = 84.0;
/// Row height used when none was set, in points.
pub const DEFAULT_ROW_HEIGHT: f32 = 22.0;
/// Lowest a row can be dragged.
pub const MIN_ROW_HEIGHT: f32 = 8.0;
/// Narrowest a column can be dragged.
pub const MIN_COL_WIDTH: f32 = 8.0;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Cell {
    pub content: Content,
    pub value: Value,
    pub format: CellFormat,
}

/// Everything about a cell that a user edits; what undo stores.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CellState {
    pub content: Content,
    pub format: CellFormat,
}

impl CellState {
    pub fn is_default(&self) -> bool {
        self.content.is_empty() && self.format.is_default()
    }
}

static EMPTY: Value = Value::Empty;

#[derive(Debug, Default, Clone)]
pub struct Sheet {
    pub(crate) id: SheetId,
    name: String,
    /// One paged column per sheet column, so ranges iterate only the
    /// cells that exist.
    columns: Vec<Column>,
    col_widths: BTreeMap<u32, f32>,
    row_heights: BTreeMap<u32, f32>,
    /// Rows hidden by a filter (or hidden in an opened file).
    hidden_rows: BTreeSet<u32>,
    /// Rows whose height differs from the default, with the total extra
    /// height up to and including each, for fast row positions.
    row_extra: Vec<(u32, f64)>,
    /// Rows and columns kept in view at the top and left.
    freeze: (u32, u32),
}

impl Sheet {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), ..Self::default() }
    }

    pub fn id(&self) -> SheetId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    pub fn cell(&self, at: CellRef) -> Option<&Cell> {
        self.columns.get(at.col as usize)?.get(at.row)
    }

    pub fn value(&self, at: CellRef) -> &Value {
        self.cell(at).map_or(&EMPTY, |c| &c.value)
    }

    pub fn format(&self, at: CellRef) -> CellFormat {
        self.cell(at).map(|c| c.format).unwrap_or_default()
    }

    pub fn state(&self, at: CellRef) -> CellState {
        self.cell(at)
            .map_or_else(CellState::default, |c| CellState { content: c.content.clone(), format: c.format })
    }

    /// The formatted text a cell displays.
    pub fn display(&self, at: CellRef) -> String {
        self.cell(at).map_or_else(String::new, |c| format_value(&c.value, c.format.num))
    }

    pub fn cell_count(&self) -> usize {
        self.columns.iter().map(Column::len).sum()
    }

    /// Iterates existing cells inside `range`, column by column.
    pub fn cells_in(&self, range: Range) -> impl Iterator<Item = (CellRef, &Cell)> + '_ {
        let last = (range.end.col as usize).min(self.columns.len().saturating_sub(1));
        let cols = if (range.start.col as usize) < self.columns.len() {
            &self.columns[range.start.col as usize..=last]
        } else {
            &[][..]
        };
        cols.iter().enumerate().flat_map(move |(i, col)| {
            let c = range.start.col + i as u32;
            col.range(range.start.row, range.end.row).map(move |(r, cell)| (CellRef::new(r, c), cell))
        })
    }

    /// Every existing cell, column by column.
    pub fn cells(&self) -> impl Iterator<Item = (CellRef, &Cell)> + '_ {
        self.columns
            .iter()
            .enumerate()
            .flat_map(|(c, col)| col.iter().map(move |(r, cell)| (CellRef::new(r, c as u32), cell)))
    }

    /// The smallest range holding every non-default cell.
    pub fn used_range(&self) -> Option<Range> {
        let mut bounds: Option<(u32, u32, u32, u32)> = None;
        for (c, col) in self.columns.iter().enumerate() {
            let (Some(first), Some(last)) = (col.first_row(), col.last_row()) else {
                continue;
            };
            let c = c as u32;
            bounds = Some(match bounds {
                None => (first, c, last, c),
                Some((r0, c0, r1, _)) => (r0.min(first), c0, r1.max(last), c),
            });
        }
        bounds.map(|(r0, c0, r1, c1)| Range::new(CellRef::new(r0, c0), CellRef::new(r1, c1)))
    }

    pub fn col_width(&self, col: u32) -> f32 {
        self.col_widths.get(&col).copied().unwrap_or(DEFAULT_COL_WIDTH)
    }

    pub fn custom_col_width(&self, col: u32) -> Option<f32> {
        self.col_widths.get(&col).copied()
    }

    pub fn col_widths(&self) -> impl Iterator<Item = (u32, f32)> + '_ {
        self.col_widths.iter().map(|(&c, &w)| (c, w))
    }

    /// Sets or clears a custom width and returns the previous one.
    pub fn set_col_width(&mut self, col: u32, width: Option<f32>) -> Option<f32> {
        match width {
            Some(w) => self.col_widths.insert(col, w.max(MIN_COL_WIDTH)),
            None => self.col_widths.remove(&col),
        }
    }

    /// Moves rows (or columns) at and after `at` by `delta`. For a negative
    /// `delta` the lines `at + delta .. at` are removed and must already be
    /// empty of cells; lines pushed off the grid are dropped (callers check
    /// that first).
    pub(crate) fn shift_lines(&mut self, rows: bool, at: u32, delta: i64) {
        if rows {
            for col in &mut self.columns {
                col.shift(at, delta, MAX_ROWS);
            }
            let moved = |r: u32| -> Option<u32> {
                let removed = delta < 0 && i64::from(r) >= i64::from(at) + delta && r < at;
                let target = if r >= at { i64::from(r) + delta } else { i64::from(r) };
                (!removed && (0..i64::from(MAX_ROWS)).contains(&target)).then_some(target as u32)
            };
            self.row_heights = std::mem::take(&mut self.row_heights)
                .into_iter()
                .filter_map(|(r, h)| Some((moved(r)?, h)))
                .collect();
            self.hidden_rows = std::mem::take(&mut self.hidden_rows).into_iter().filter_map(moved).collect();
            self.rebuild_rows();
            return;
        }
        let at_i = at as usize;
        if delta > 0 {
            if at_i < self.columns.len() {
                for _ in 0..delta {
                    self.columns.insert(at_i, Column::default());
                }
                self.columns.truncate(MAX_COLS as usize);
            }
        } else {
            let start = (i64::from(at) + delta).max(0) as usize;
            let end = at_i.min(self.columns.len());
            if start < end {
                self.columns.drain(start..end);
            }
        }
        let widths = std::mem::take(&mut self.col_widths);
        for (col, w) in widths {
            let removed = delta < 0 && i64::from(col) >= i64::from(at) + delta && col < at;
            let target = if col >= at { i64::from(col) + delta } else { i64::from(col) };
            if !removed && (0..i64::from(MAX_COLS)).contains(&target) {
                self.col_widths.insert(target as u32, w);
            }
        }
    }

    /// Whether the last `count` rows (or columns) of the grid hold anything,
    /// in which case inserting would push data off the sheet.
    pub fn edge_occupied(&self, rows: bool, count: u32) -> bool {
        let Some(used) = self.used_range() else { return false };
        if rows { used.end.row >= MAX_ROWS - count } else { used.end.col >= MAX_COLS - count }
    }

    /// Height of `row` as drawn: 0 when hidden.
    pub fn row_height(&self, row: u32) -> f32 {
        if self.hidden_rows.contains(&row) {
            0.0
        } else {
            self.row_heights.get(&row).copied().unwrap_or(DEFAULT_ROW_HEIGHT)
        }
    }

    pub fn custom_row_height(&self, row: u32) -> Option<f32> {
        self.row_heights.get(&row).copied()
    }

    pub fn row_heights(&self) -> impl Iterator<Item = (u32, f32)> + '_ {
        self.row_heights.iter().map(|(&r, &h)| (r, h))
    }

    /// Sets or clears a custom height and returns the previous one.
    pub fn set_row_height(&mut self, row: u32, height: Option<f32>) -> Option<f32> {
        let old = match height {
            Some(h) => self.row_heights.insert(row, h.max(MIN_ROW_HEIGHT)),
            None => self.row_heights.remove(&row),
        };
        self.rebuild_rows();
        old
    }

    pub fn is_row_hidden(&self, row: u32) -> bool {
        self.hidden_rows.contains(&row)
    }

    pub fn hidden_rows(&self) -> &BTreeSet<u32> {
        &self.hidden_rows
    }

    /// Replaces the set of hidden rows, returning the old one.
    pub fn set_hidden_rows(&mut self, rows: BTreeSet<u32>) -> BTreeSet<u32> {
        let old = std::mem::replace(&mut self.hidden_rows, rows);
        self.rebuild_rows();
        old
    }

    fn rebuild_rows(&mut self) {
        let rows: BTreeSet<u32> = self.row_heights.keys().chain(self.hidden_rows.iter()).copied().collect();
        let mut total = 0.0;
        self.row_extra = rows
            .into_iter()
            .map(|r| {
                total += f64::from(self.row_height(r) - DEFAULT_ROW_HEIGHT);
                (r, total)
            })
            .collect();
    }

    /// Y offset of the top edge of `row`.
    pub fn row_y(&self, row: u32) -> f64 {
        // Extra height of all exceptional rows above `row`.
        let i = self.row_extra.partition_point(|(r, _)| *r < row);
        let extra = if i == 0 { 0.0 } else { self.row_extra[i - 1].1 };
        f64::from(row) * f64::from(DEFAULT_ROW_HEIGHT) + extra
    }

    /// The row whose span contains offset `y` (clamped to the grid; hidden
    /// rows are skipped).
    pub fn row_at_y(&self, y: f64) -> u32 {
        if y <= 0.0 {
            return self.first_visible_row(0);
        }
        let (mut lo, mut hi) = (0u32, MAX_ROWS - 1);
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if self.row_y(mid) <= y { lo = mid } else { hi = mid - 1 }
        }
        lo
    }

    /// `row` or the first row after it that is not hidden.
    pub fn first_visible_row(&self, row: u32) -> u32 {
        let mut r = row;
        while self.hidden_rows.contains(&r) && r < MAX_ROWS - 1 {
            r += 1;
        }
        r
    }

    pub fn freeze(&self) -> (u32, u32) {
        self.freeze
    }

    /// Keeps the first `rows` rows and `cols` columns in view; returns the
    /// previous setting.
    pub fn set_freeze(&mut self, rows: u32, cols: u32) -> (u32, u32) {
        std::mem::replace(&mut self.freeze, (rows.min(MAX_ROWS - 1), cols.min(MAX_COLS - 1)))
    }

    /// X offset of the left edge of `col`, honouring custom widths.
    pub fn col_x(&self, col: u32) -> f64 {
        let mut x = f64::from(col) * f64::from(DEFAULT_COL_WIDTH);
        for (_, w) in self.col_widths.range(..col) {
            x += f64::from(*w - DEFAULT_COL_WIDTH);
        }
        x
    }

    /// The column whose span contains offset `x` (clamped to the grid).
    pub fn col_at_x(&self, x: f64) -> u32 {
        if x <= 0.0 {
            return 0;
        }
        // Binary search on the monotonic col_x.
        let (mut lo, mut hi) = (0u32, MAX_COLS - 1);
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if self.col_x(mid) <= x { lo = mid } else { hi = mid - 1 }
        }
        lo
    }

    /// Stores a cell. A formula gets its value when the workbook next
    /// recalculates; use this for bulk loads, then build a
    /// [`Workbook`](crate::workbook::Workbook) from the sheets.
    pub fn put(&mut self, at: CellRef, state: CellState) {
        let col = at.col as usize;
        if state.is_default() {
            if let Some(column) = self.columns.get_mut(col) {
                column.remove(at.row);
            }
            return;
        }
        if self.columns.len() <= col {
            self.columns.resize_with(col + 1, Column::default);
        }
        let value = match &state.content {
            Content::Value(v) => v.clone(),
            // Computed by the next recalculation.
            Content::Formula(_) | Content::Empty => Value::Empty,
        };
        self.columns[col].insert(at.row, Cell { content: state.content, value, format: state.format });
    }

    /// Stores a computed value for a formula cell, e.g. a cached result read
    /// from a file, without recalculating.
    pub fn put_value(&mut self, at: CellRef, value: Value) {
        if let Some(cell) = self.columns.get_mut(at.col as usize).and_then(|c| c.get_mut(at.row)) {
            cell.value = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_positions_follow_heights_and_hidden_rows() {
        let mut s = Sheet::new("Blad1");
        let d = f64::from(DEFAULT_ROW_HEIGHT);
        assert_eq!(s.row_y(10), 10.0 * d);
        assert_eq!(s.row_at_y(10.0 * d + 1.0), 10);
        s.set_row_height(2, Some(50.0));
        assert_eq!(s.row_y(3), 2.0 * d + 50.0);
        assert_eq!(s.row_at_y(2.0 * d + 49.0), 2);
        assert_eq!(s.row_at_y(2.0 * d + 51.0), 3);
        s.set_hidden_rows([4, 5].into_iter().collect());
        assert_eq!(s.row_height(4), 0.0);
        assert_eq!(s.row_y(6), s.row_y(4));
        // A y inside the row after hidden rows lands on that row, not a hidden one.
        assert_eq!(s.row_at_y(s.row_y(6) + 1.0), 6);
        assert_eq!(s.row_y(1_000_000), 1_000_000.0 * d + 50.0 - d - 2.0 * d);
        assert_eq!(s.first_visible_row(4), 6);

        // Inserting rows above moves heights and hidden rows along.
        s.shift_lines(true, 0, 3);
        assert_eq!(s.custom_row_height(5), Some(50.0));
        assert!(s.is_row_hidden(7) && s.is_row_hidden(8) && !s.is_row_hidden(4));
        s.shift_lines(true, 3, -3);
        assert_eq!(s.custom_row_height(2), Some(50.0));
    }
}
