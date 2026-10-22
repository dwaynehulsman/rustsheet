//! A workbook: ordered sheets, plus formula evaluation and incremental
//! recalculation across all of them.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::addr::{CellRef, Range};
use crate::deps::{DepGraph, Node};
use crate::formula::{CellSource, Precedent, evaluate, same_sheet_name};
use crate::input::Content;
use crate::sheet::{CellState, Sheet, SheetId};
use crate::value::{ErrorKind, Value};

/// Longest sheet name Excel accepts.
pub const MAX_SHEET_NAME: usize = 31;

static EMPTY: Value = Value::Empty;

#[derive(Debug)]
pub struct Workbook {
    sheets: Vec<Sheet>,
    /// Position of each sheet id in `sheets`; `u32::MAX` once removed.
    index: Vec<u32>,
    next_id: SheetId,
    deps: DepGraph,
    /// Formulas using TODAY or NOW; recalculated with every change.
    volatile: FxHashSet<Node>,
    /// Formulas evaluated by the most recent recalculation.
    last_recalc: usize,
}

impl Default for Workbook {
    fn default() -> Self {
        Self::new()
    }
}

fn find_in(sheets: &[Sheet], name: &str) -> Option<SheetId> {
    sheets.iter().find(|s| same_sheet_name(s.name(), name)).map(Sheet::id)
}

impl Workbook {
    /// An empty workbook with one sheet, "Blad1".
    pub fn new() -> Self {
        Self::from_sheets(vec![Sheet::new("Blad1")])
    }

    /// Builds a workbook from loaded sheets and calculates every formula.
    /// Sheets get fresh ids in order.
    pub fn from_sheets(mut sheets: Vec<Sheet>) -> Self {
        if sheets.is_empty() {
            sheets.push(Sheet::new("Blad1"));
        }
        for (i, sheet) in sheets.iter_mut().enumerate() {
            sheet.id = i as SheetId;
        }
        let next_id = sheets.len() as SheetId;
        let mut book = Workbook {
            sheets,
            index: Vec::new(),
            next_id,
            deps: DepGraph::default(),
            volatile: FxHashSet::default(),
            last_recalc: 0,
        };
        book.rebuild();
        book
    }

    pub fn sheets(&self) -> &[Sheet] {
        &self.sheets
    }

    pub fn len(&self) -> usize {
        self.sheets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sheets.is_empty()
    }

    pub fn sheet(&self, index: usize) -> &Sheet {
        &self.sheets[index]
    }

    pub fn index_of(&self, id: SheetId) -> Option<usize> {
        self.index.get(id as usize).filter(|&&i| i != u32::MAX).map(|&i| i as usize)
    }

    pub fn sheet_by_id(&self, id: SheetId) -> Option<&Sheet> {
        self.index_of(id).map(|i| &self.sheets[i])
    }

    /// The sheet called `name`, ignoring case.
    pub fn find(&self, name: &str) -> Option<SheetId> {
        find_in(&self.sheets, name)
    }

    pub fn formula_count(&self) -> usize {
        self.deps.formula_count()
    }

    pub fn cell_count(&self) -> usize {
        self.sheets.iter().map(Sheet::cell_count).sum()
    }

    /// How many formulas the most recent recalculation evaluated.
    pub fn last_recalc_count(&self) -> usize {
        self.last_recalc
    }

    // ----- cells -----

    /// Applies new states to cells of `sheet`, recalculates whatever depends
    /// on them (on any sheet) and returns the undo list. Applying the
    /// returned list in order restores the previous states exactly, even
    /// when `changes` repeats a cell.
    pub fn set_states(
        &mut self,
        sheet: SheetId,
        changes: Vec<(CellRef, CellState)>,
    ) -> Vec<(CellRef, CellState)> {
        let Some(index) = self.index_of(sheet) else { return Vec::new() };
        let mut undo = Vec::with_capacity(changes.len());
        let mut changed = Vec::new();
        for (at, state) in changes {
            let old = self.sheets[index].state(at);
            if old == state {
                continue;
            }
            if old.content != state.content {
                changed.push((sheet, at));
            }
            self.put(index, at, state);
            undo.push((at, old));
        }
        undo.reverse();
        self.recalc(&changed);
        undo
    }

    /// Stores a cell and registers its formula, without recalculating.
    fn put(&mut self, index: usize, at: CellRef, state: CellState) {
        let id = self.sheets[index].id;
        match &state.content {
            Content::Formula(expr) => {
                let mut precedents = Vec::new();
                let sheets = &self.sheets;
                expr.precedents(id, &|name| find_in(sheets, name), &mut precedents);
                self.deps.set((id, at), precedents);
                if expr.is_volatile() {
                    self.volatile.insert((id, at));
                } else {
                    self.volatile.remove(&(id, at));
                }
            }
            _ => {
                self.deps.remove((id, at));
                self.volatile.remove(&(id, at));
            }
        }
        self.sheets[index].put(at, state);
    }

    /// Sets or clears a custom column width; returns the previous one.
    pub fn set_col_width(&mut self, sheet: SheetId, col: u32, width: Option<f32>) -> Option<f32> {
        let index = self.index_of(sheet)?;
        self.sheets[index].set_col_width(col, width)
    }

    /// Sets or clears a custom row height; returns the previous one.
    pub fn set_row_height(&mut self, sheet: SheetId, row: u32, height: Option<f32>) -> Option<f32> {
        let index = self.index_of(sheet)?;
        self.sheets[index].set_row_height(row, height)
    }

    /// Replaces the hidden rows of a sheet; returns the previous set.
    pub fn set_hidden_rows(
        &mut self,
        sheet: SheetId,
        rows: std::collections::BTreeSet<u32>,
    ) -> std::collections::BTreeSet<u32> {
        match self.index_of(sheet) {
            Some(index) => self.sheets[index].set_hidden_rows(rows),
            None => rows,
        }
    }

    /// Sets or removes a sheet's autofilter; returns the previous one.
    pub fn set_filter(
        &mut self,
        sheet: SheetId,
        filter: Option<crate::filter::AutoFilter>,
    ) -> Option<crate::filter::AutoFilter> {
        let index = self.index_of(sheet)?;
        self.sheets[index].set_filter(filter)
    }

    /// Sets the frozen rows and columns; returns the previous setting.
    pub fn set_freeze(&mut self, sheet: SheetId, rows: u32, cols: u32) -> (u32, u32) {
        match self.index_of(sheet) {
            Some(index) => self.sheets[index].set_freeze(rows, cols),
            None => (rows, cols),
        }
    }

    // ----- sheets -----

    /// A new empty sheet with a fresh id, not yet part of the workbook.
    pub fn new_sheet(&mut self, name: impl Into<String>) -> Sheet {
        let mut sheet = Sheet::new(name);
        sheet.id = self.next_id;
        self.next_id += 1;
        sheet
    }

    /// A copy of sheet `id` under a fresh id and the given name.
    pub fn copy_sheet(&mut self, id: SheetId, name: impl Into<String>) -> Option<Sheet> {
        let mut copy = self.sheet_by_id(id)?.clone();
        copy.id = self.next_id;
        self.next_id += 1;
        copy.set_name(name);
        Some(copy)
    }

    /// Inserts `sheet` (keeping its id) at `index` and recalculates.
    pub fn insert_sheet(&mut self, index: usize, sheet: Sheet) {
        debug_assert!(self.index_of(sheet.id).is_none(), "sheet id already present");
        self.next_id = self.next_id.max(sheet.id + 1);
        self.sheets.insert(index.min(self.sheets.len()), sheet);
        self.rebuild();
    }

    /// Removes sheet `id`, returning where it was and its contents.
    pub fn remove_sheet(&mut self, id: SheetId) -> Option<(usize, Sheet)> {
        let index = self.index_of(id)?;
        let sheet = self.sheets.remove(index);
        self.rebuild();
        Some((index, sheet))
    }

    /// Renames sheet `id` and returns the old name. Formulas are not
    /// rewritten here; [`crate::Document::rename_sheet`] does that.
    pub fn rename_sheet(&mut self, id: SheetId, name: impl Into<String>) -> Option<String> {
        let index = self.index_of(id)?;
        let old = self.sheets[index].name().to_string();
        self.sheets[index].set_name(name);
        self.rebuild();
        Some(old)
    }

    /// Moves rows or columns of sheet `id` at and after `at` by `delta`,
    /// without touching formulas (see [`crate::Document::insert_lines`]).
    pub fn shift_lines(&mut self, id: SheetId, rows: bool, at: u32, delta: i64) {
        let Some(index) = self.index_of(id) else { return };
        self.sheets[index].shift_lines(rows, at, delta);
        self.rebuild();
    }

    /// Checks a sheet name against Excel's rules; returns it trimmed.
    /// `except` is the sheet being renamed, which may keep its own name.
    pub fn check_name(&self, name: &str, except: Option<SheetId>) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Een bladnaam mag niet leeg zijn.".into());
        }
        if name.chars().count() > MAX_SHEET_NAME {
            return Err(format!("Een bladnaam mag hoogstens {MAX_SHEET_NAME} tekens hebben."));
        }
        if let Some(c) = name.chars().find(|c| ":\\/?*[]".contains(*c)) {
            return Err(format!("Een bladnaam mag geen “{c}” bevatten."));
        }
        if name.starts_with('\'') || name.ends_with('\'') {
            return Err("Een bladnaam mag niet met een apostrof beginnen of eindigen.".into());
        }
        if let Some(other) = self.find(name)
            && Some(other) != except
        {
            return Err(format!("Er is al een blad met de naam “{name}”."));
        }
        Ok(name.to_string())
    }

    /// `base` if free, otherwise `base (2)`, `base (3)`, ...
    pub fn unique_name(&self, base: &str) -> String {
        if self.find(base).is_none() {
            return base.to_string();
        }
        (2..).map(|n| format!("{base} ({n})")).find(|n| self.find(n).is_none()).expect("a free name")
    }

    /// The next free `Blad<n>` name.
    pub fn next_sheet_name(&self) -> String {
        (1..).map(|n| format!("Blad{n}")).find(|n| self.find(n).is_none()).expect("a free name")
    }

    /// Re-registers every formula and recalculates everything. Used after
    /// sheets are added, removed or renamed, since that changes what sheet
    /// names in formulas resolve to.
    fn rebuild(&mut self) {
        self.index.clear();
        for (i, sheet) in self.sheets.iter().enumerate() {
            let id = sheet.id as usize;
            if self.index.len() <= id {
                self.index.resize(id + 1, u32::MAX);
            }
            self.index[id] = i as u32;
        }
        self.deps.clear();
        self.volatile.clear();
        let sheets = &self.sheets;
        let resolve = |name: &str| find_in(sheets, name);
        for sheet in sheets {
            for (at, cell) in sheet.cells() {
                if let Content::Formula(expr) = &cell.content {
                    let mut precedents = Vec::new();
                    expr.precedents(sheet.id, &resolve, &mut precedents);
                    self.deps.set((sheet.id, at), precedents);
                    if expr.is_volatile() {
                        self.volatile.insert((sheet.id, at));
                    }
                }
            }
        }
        self.recalc_all();
    }

    // ----- recalculation -----

    /// Recalculates every formula in the workbook.
    pub fn recalc_all(&mut self) {
        let dirty: FxHashSet<Node> = self.deps.formula_cells().collect();
        self.evaluate_set(dirty);
    }

    /// Recalculates the formulas that (transitively) read any of `changed`.
    fn recalc(&mut self, changed: &[Node]) {
        if changed.is_empty() {
            self.last_recalc = 0;
            return;
        }
        // Volatile formulas (TODAY, NOW) and what reads them refresh with every change.
        let mut queue: Vec<Node> = self.volatile.iter().copied().collect();
        let mut dirty: FxHashSet<Node> = queue.iter().copied().collect();
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
    fn evaluate_set(&mut self, dirty: FxHashSet<Node>) {
        self.last_recalc = dirty.len();
        if dirty.is_empty() {
            return;
        }
        let order = self.evaluation_order(dirty);
        for ((sheet, at), circular) in order {
            let Some(index) = self.index_of(sheet) else { continue };
            let value = if circular {
                Value::Error(ErrorKind::Circ)
            } else {
                match self.sheets[index].cell(at).and_then(|c| c.content.formula()) {
                    Some(expr) => evaluate(expr, self, sheet),
                    None => continue,
                }
            };
            self.sheets[index].put_value(at, value);
        }
    }

    /// Tarjan's strongly connected components over the dirty formulas, with
    /// edges from a formula to the dirty formulas it reads. Components come
    /// out precedents-first, which is exactly evaluation order.
    fn evaluation_order(&self, dirty: FxHashSet<Node>) -> Vec<(Node, bool)> {
        let nodes: Vec<Node> = dirty.iter().copied().collect();
        let id: FxHashMap<Node, u32> = nodes.iter().enumerate().map(|(i, &c)| (c, i as u32)).collect();
        let n = nodes.len();

        let edges_of = |v: u32| -> Vec<u32> {
            let mut out = Vec::new();
            for p in self.deps.precedents(nodes[v as usize]) {
                match *p {
                    Precedent::Cell(s, c) => {
                        if let Some(&w) = id.get(&(s, c)) {
                            out.push(w);
                        }
                    }
                    Precedent::Range(s, r) => {
                        if (n as u64) < r.len() / 8 {
                            out.extend(
                                nodes
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, (ns, c))| *ns == s && r.contains(*c))
                                    .map(|(i, _)| i as u32),
                            );
                        } else if let Some(sheet) = self.sheet_by_id(s) {
                            out.extend(sheet.cells_in(r).filter_map(|(c, _)| id.get(&(s, c)).copied()));
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

impl CellSource for Workbook {
    fn resolve_sheet(&self, name: &str) -> Option<SheetId> {
        self.find(name)
    }

    fn value(&self, sheet: SheetId, at: CellRef) -> &Value {
        self.sheet_by_id(sheet).map_or(&EMPTY, |s| s.value(at))
    }

    fn for_each_cell(&self, sheet: SheetId, range: Range, f: &mut dyn FnMut(CellRef, &Value) -> bool) {
        let Some(sheet) = self.sheet_by_id(sheet) else { return };
        for (at, cell) in sheet.cells_in(range) {
            if !cell.value.is_empty() && !f(at, &cell.value) {
                return;
            }
        }
    }

    fn for_each_value(&self, sheet: SheetId, range: Range, f: &mut dyn FnMut(&Value) -> bool) {
        let Some(sheet) = self.sheet_by_id(sheet) else { return };
        for (_, cell) in sheet.cells_in(range) {
            if !cell.value.is_empty() && !f(&cell.value) {
                return;
            }
        }
    }
}
