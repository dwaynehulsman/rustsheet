//! Sparse cell storage with incremental recalculation.

use std::collections::BTreeMap;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::addr::{CellRef, MAX_COLS, Range};
use crate::column::Column;
use crate::deps::DepGraph;
use crate::format::{CellFormat, format_value};
use crate::formula::{CellSource, Precedent, evaluate};
use crate::input::Content;
use crate::value::{ErrorKind, Value};

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

#[derive(Debug, Default)]
pub struct Sheet {
    name: String,
    /// One paged column per sheet column, so ranges iterate only the
    /// cells that exist.
    columns: Vec<Column>,
    deps: DepGraph,
    col_widths: BTreeMap<u32, f32>,
    /// Formulas evaluated by the most recent recalculation.
    last_recalc: usize,
}

impl Sheet {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), ..Self::default() }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
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

    /// How many formulas the most recent recalculation evaluated.
    pub fn last_recalc_count(&self) -> usize {
        self.last_recalc
    }

    pub fn formula_count(&self) -> usize {
        self.deps.formula_count()
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

    /// Applies new states to cells, recalculates whatever depends on them and
    /// returns the undo list. Applying the returned list in order restores
    /// the previous states exactly, even when `changes` repeats a cell.
    pub fn set_states(&mut self, changes: Vec<(CellRef, CellState)>) -> Vec<(CellRef, CellState)> {
        let mut undo = Vec::with_capacity(changes.len());
        let mut changed = Vec::new();
        for (at, state) in changes {
            let old = self.state(at);
            if old == state {
                continue;
            }
            if old.content != state.content {
                changed.push(at);
            }
            self.put(at, state);
            undo.push((at, old));
        }
        undo.reverse();
        self.recalc(&changed);
        undo
    }

    /// Stores a cell without recalculating. Use for bulk loads followed by
    /// [`Sheet::recalc_all`].
    pub fn put(&mut self, at: CellRef, state: CellState) {
        let col = at.col as usize;
        match &state.content {
            Content::Formula(expr) => {
                let mut precedents = Vec::new();
                expr.precedents(&self.name, &mut precedents);
                self.deps.set(at, precedents);
            }
            _ => self.deps.remove(at),
        }
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

    /// Recalculates every formula in the sheet.
    pub fn recalc_all(&mut self) {
        let dirty: FxHashSet<CellRef> = self.deps.formula_cells().collect();
        self.evaluate_set(dirty);
    }

    /// Recalculates the formulas that (transitively) read any of `changed`.
    pub fn recalc(&mut self, changed: &[CellRef]) {
        if changed.is_empty() {
            self.last_recalc = 0;
            return;
        }
        let mut dirty = FxHashSet::default();
        let mut queue = Vec::new();
        for &c in changed {
            if self.deps.is_formula(c) {
                dirty.insert(c);
            }
            queue.push(c);
        }
        while let Some(c) = queue.pop() {
            self.deps.dependents(c, |d| {
                if dirty.insert(d) {
                    queue.push(d);
                }
            });
        }
        self.evaluate_set(dirty);
    }

    /// Evaluates `dirty` in dependency order. Cells on a reference cycle get
    /// `#CIRC!`; formulas reading them then see that error.
    fn evaluate_set(&mut self, dirty: FxHashSet<CellRef>) {
        self.last_recalc = dirty.len();
        if dirty.is_empty() {
            return;
        }
        let order = self.evaluation_order(dirty);
        for (at, circular) in order {
            let value = if circular {
                Value::Error(ErrorKind::Circ)
            } else {
                match self.cell(at).and_then(|c| c.content.formula()) {
                    Some(expr) => evaluate(expr, self),
                    None => continue,
                }
            };
            self.put_value(at, value);
        }
    }

    /// Tarjan's strongly connected components over the dirty formulas, with
    /// edges from a formula to the dirty formulas it reads. Components come
    /// out precedents-first, which is exactly evaluation order.
    fn evaluation_order(&self, dirty: FxHashSet<CellRef>) -> Vec<(CellRef, bool)> {
        let nodes: Vec<CellRef> = dirty.iter().copied().collect();
        let id: FxHashMap<CellRef, u32> = nodes.iter().enumerate().map(|(i, &c)| (c, i as u32)).collect();
        let n = nodes.len();

        let edges_of = |v: u32| -> Vec<u32> {
            let mut out = Vec::new();
            for p in self.deps.precedents(nodes[v as usize]) {
                match *p {
                    Precedent::Cell(c) => {
                        if let Some(&w) = id.get(&c) {
                            out.push(w);
                        }
                    }
                    Precedent::Range(r) => {
                        if (n as u64) < r.len() / 8 {
                            out.extend(
                                nodes
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, c)| r.contains(**c))
                                    .map(|(i, _)| i as u32),
                            );
                        } else {
                            out.extend(self.cells_in(r).filter_map(|(c, _)| id.get(&c).copied()));
                        }
                    }
                }
            }
            out
        };

        const UNSEEN: u32 = u32::MAX;
        let mut index = vec![UNSEEN; n];
        let mut low = vec![0u32; n];
        let mut on_stack = vec![false; n];
        let mut stack: Vec<u32> = Vec::new();
        let mut order = Vec::with_capacity(n);
        let mut counter = 0u32;
        // Explicit call stack: (node, its edges, next edge to visit).
        let mut calls: Vec<(u32, Vec<u32>, usize)> = Vec::new();

        for root in 0..n as u32 {
            if index[root as usize] != UNSEEN {
                continue;
            }
            index[root as usize] = counter;
            low[root as usize] = counter;
            counter += 1;
            stack.push(root);
            on_stack[root as usize] = true;
            calls.push((root, edges_of(root), 0));

            while let Some((v, edges, pos)) = calls.last_mut() {
                let v = *v;
                if let Some(&w) = edges.get(*pos) {
                    *pos += 1;
                    if index[w as usize] == UNSEEN {
                        index[w as usize] = counter;
                        low[w as usize] = counter;
                        counter += 1;
                        stack.push(w);
                        on_stack[w as usize] = true;
                        calls.push((w, edges_of(w), 0));
                    } else if on_stack[w as usize] {
                        low[v as usize] = low[v as usize].min(index[w as usize]);
                    }
                    continue;
                }
                let (_, edges, _) = calls.pop().expect("frame");
                if let Some((parent, _, _)) = calls.last() {
                    low[*parent as usize] = low[*parent as usize].min(low[v as usize]);
                }
                if low[v as usize] == index[v as usize] {
                    let mut component = Vec::new();
                    loop {
                        let w = stack.pop().expect("tarjan stack");
                        on_stack[w as usize] = false;
                        component.push(w);
                        if w == v {
                            break;
                        }
                    }
                    let circular = component.len() > 1 || edges.contains(&v);
                    order.extend(component.into_iter().map(|w| (nodes[w as usize], circular)));
                }
            }
        }
        order
    }
}

impl CellSource for Sheet {
    fn sheet_name(&self) -> &str {
        &self.name
    }

    fn value(&self, at: CellRef) -> &Value {
        Sheet::value(self, at)
    }

    fn for_each_value(&self, range: Range, f: &mut dyn FnMut(&Value) -> bool) {
        for (_, cell) in self.cells_in(range) {
            if !cell.value.is_empty() && !f(&cell.value) {
                return;
            }
        }
    }
}
