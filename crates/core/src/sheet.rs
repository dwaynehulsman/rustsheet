//! Storage for one worksheet: sparse cells and column widths. Formulas are
//! evaluated by the [`Workbook`](crate::workbook::Workbook) that owns the sheet.

use std::collections::BTreeMap;

use crate::addr::{CellRef, MAX_COLS, Range};
use crate::column::Column;
use crate::format::{CellFormat, format_value};
use crate::input::Content;
use crate::value::Value;

/// Stable identity of a sheet within a workbook. Survives renames and
/// reordering, so undo steps and references stay valid.
pub type SheetId = u32;

/// Column width used when none was set, in points.
pub const DEFAULT_COL_WIDTH: f32 = 84.0;
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
