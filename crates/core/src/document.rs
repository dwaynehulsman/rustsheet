//! A sheet plus its undo history and saved/unsaved bookkeeping.

use crate::addr::{CellRef, Range};
use crate::sheet::{CellState, Sheet};

/// One undoable step.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Cells(Vec<(CellRef, CellState)>),
    ColWidths(Vec<(u32, Option<f32>)>),
}

impl Change {
    /// Bounding box of the cells involved, to reselect after undo.
    fn area(&self) -> Option<Range> {
        match self {
            Change::Cells(cells) => cells.iter().fold(None, |acc: Option<Range>, (at, _)| {
                Some(match acc {
                    None => Range::single(*at),
                    Some(r) => Range::new(
                        CellRef::new(r.start.row.min(at.row), r.start.col.min(at.col)),
                        CellRef::new(r.end.row.max(at.row), r.end.col.max(at.col)),
                    ),
                })
            }),
            Change::ColWidths(_) => None,
        }
    }
}

#[derive(Debug)]
struct Step {
    /// Applying this undoes (or redoes) the step.
    change: Change,
    /// Identifies the document state this step leads to.
    id: u64,
    /// Selection to restore when this step is applied.
    selection: Option<Range>,
}

/// Most steps kept for undo.
const UNDO_LIMIT: usize = 500;

#[derive(Debug, Default)]
pub struct Document {
    pub sheet: Sheet,
    undo: Vec<Step>,
    redo: Vec<Step>,
    next_id: u64,
    /// Id of the state the document is in; 0 is the state it was opened in.
    state_id: u64,
    saved_id: u64,
    /// Bumped by every change, undo and redo; lets views cache derived data.
    revision: u64,
}

impl Document {
    pub fn new(sheet: Sheet) -> Self {
        Self { sheet, next_id: 1, ..Self::default() }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_modified(&self) -> bool {
        self.state_id != self.saved_id
    }

    pub fn mark_saved(&mut self) {
        self.saved_id = self.state_id;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn apply_raw(&mut self, change: Change) -> Change {
        self.revision += 1;
        match change {
            Change::Cells(cells) => Change::Cells(self.sheet.set_states(cells)),
            Change::ColWidths(widths) => Change::ColWidths(
                widths.into_iter().rev().map(|(col, w)| (col, self.sheet.set_col_width(col, w))).collect(),
            ),
        }
    }

    /// Applies an edit and records it for undo. `selection` is restored when
    /// the edit is undone.
    pub fn apply(&mut self, change: Change, selection: Option<Range>) {
        let inverse = self.apply_raw(change);
        if matches!(&inverse, Change::Cells(c) if c.is_empty())
            || matches!(&inverse, Change::ColWidths(w) if w.is_empty())
        {
            return;
        }
        let previous = self.state_id;
        self.state_id = self.next_id;
        self.next_id += 1;
        self.undo.push(Step { change: inverse, id: previous, selection });
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Convenience for the common case of setting cell states.
    pub fn set_cells(&mut self, cells: Vec<(CellRef, CellState)>, selection: Option<Range>) {
        self.apply(Change::Cells(cells), selection);
    }

    /// Undoes the last step; returns the range to select afterwards.
    pub fn undo(&mut self) -> Option<Option<Range>> {
        let step = self.undo.pop()?;
        let area = step.change.area();
        let inverse = self.apply_raw(step.change);
        self.redo.push(Step { change: inverse, id: self.state_id, selection: area });
        self.state_id = step.id;
        Some(step.selection.or(area))
    }

    /// Redoes the last undone step; returns the range to select afterwards.
    pub fn redo(&mut self) -> Option<Option<Range>> {
        let step = self.redo.pop()?;
        let area = step.change.area();
        let inverse = self.apply_raw(step.change);
        self.undo.push(Step { change: inverse, id: self.state_id, selection: area });
        self.state_id = step.id;
        Some(step.selection.or(area))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::parse_input;

    fn c(s: &str) -> CellRef {
        CellRef::parse(s).unwrap()
    }

    fn state(text: &str) -> CellState {
        CellState { content: parse_input(text).unwrap().content, ..Default::default() }
    }

    #[test]
    fn undo_redo_restores_values_and_modified_flag() {
        let mut doc = Document::new(Sheet::new("Blad1"));
        doc.set_cells(vec![(c("A1"), state("2")), (c("B1"), state("=A1*10"))], None);
        doc.set_cells(vec![(c("A1"), state("3"))], None);
        assert_eq!(doc.sheet.display(c("B1")), "30");
        assert!(doc.is_modified());

        assert_eq!(doc.undo(), Some(Some(Range::single(c("A1")))));
        assert_eq!(doc.sheet.display(c("B1")), "20");
        doc.undo();
        assert_eq!(doc.sheet.display(c("B1")), "");
        assert!(!doc.is_modified());
        assert_eq!(doc.undo(), None);

        doc.redo();
        doc.redo();
        assert_eq!(doc.sheet.display(c("B1")), "30");
        doc.mark_saved();
        doc.undo();
        assert!(doc.is_modified());
        doc.redo();
        assert!(!doc.is_modified());

        // A new edit drops the redo stack.
        doc.undo();
        doc.set_cells(vec![(c("C1"), state("x"))], None);
        assert!(!doc.can_redo());
    }

    #[test]
    fn repeated_cells_in_one_step_undo_correctly() {
        let mut doc = Document::new(Sheet::new("Blad1"));
        doc.set_cells(vec![(c("A1"), state("1"))], None);
        doc.set_cells(vec![(c("A1"), state("2")), (c("A1"), state("3"))], None);
        assert_eq!(doc.sheet.display(c("A1")), "3");
        doc.undo();
        assert_eq!(doc.sheet.display(c("A1")), "1");
    }

    #[test]
    fn column_widths_are_undoable() {
        let mut doc = Document::new(Sheet::new("Blad1"));
        doc.apply(Change::ColWidths(vec![(2, Some(150.0))]), None);
        assert_eq!(doc.sheet.col_width(2), 150.0);
        doc.undo();
        assert_eq!(doc.sheet.custom_col_width(2), None);
        doc.redo();
        assert_eq!(doc.sheet.col_width(2), 150.0);
    }

    #[test]
    fn no_op_edits_are_not_recorded() {
        let mut doc = Document::new(Sheet::new("Blad1"));
        doc.set_cells(vec![(c("A1"), CellState::default())], None);
        assert!(!doc.can_undo());
        assert!(!doc.is_modified());
    }
}
