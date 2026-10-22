//! A workbook plus its undo history and saved/unsaved bookkeeping.

use crate::addr::{CellRef, Range};
use crate::input::Content;
use crate::sheet::{CellState, Sheet, SheetId};
use crate::workbook::Workbook;

/// One undoable step.
#[derive(Debug)]
pub enum Change {
    Cells(SheetId, Vec<(CellRef, CellState)>),
    ColWidths(SheetId, Vec<(u32, Option<f32>)>),
    /// Puts a sheet (with its id) back at a position.
    InsertSheet(usize, Box<Sheet>),
    RemoveSheet(SheetId),
    RenameSheet(SheetId, String),
    /// Several changes as one step, applied in order.
    Batch(Vec<Change>),
}

/// Where to look after an undo or redo: a sheet and, if known, cells on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub sheet: SheetId,
    pub range: Option<Range>,
}

impl Change {
    fn is_noop(&self) -> bool {
        match self {
            Change::Cells(_, c) => c.is_empty(),
            Change::ColWidths(_, w) => w.is_empty(),
            Change::Batch(parts) => parts.iter().all(Change::is_noop),
            _ => false,
        }
    }

    /// What the change touches, to show it after undo.
    fn spot(&self) -> Option<Spot> {
        match self {
            Change::Cells(sheet, cells) => {
                let range = cells.iter().fold(None, |acc: Option<Range>, (at, _)| {
                    Some(match acc {
                        None => Range::single(*at),
                        Some(r) => Range::new(
                            CellRef::new(r.start.row.min(at.row), r.start.col.min(at.col)),
                            CellRef::new(r.end.row.max(at.row), r.end.col.max(at.col)),
                        ),
                    })
                })?;
                Some(Spot { sheet: *sheet, range: Some(range) })
            }
            Change::ColWidths(sheet, _) | Change::RenameSheet(sheet, _) => {
                Some(Spot { sheet: *sheet, range: None })
            }
            Change::InsertSheet(_, sheet) => Some(Spot { sheet: sheet.id(), range: None }),
            Change::RemoveSheet(_) => None,
            // Structural parts say more about where to look than cell rewrites.
            Change::Batch(parts) => parts
                .iter()
                .find(|p| matches!(p, Change::InsertSheet(..) | Change::RenameSheet(..)))
                .or_else(|| parts.iter().find(|p| p.spot().is_some()))
                .and_then(Change::spot),
        }
    }
}

#[derive(Debug)]
struct Step {
    /// Applying this undoes (or redoes) the step.
    change: Change,
    /// Identifies the document state this step leads to.
    id: u64,
    /// Where the user was when the step was made.
    spot: Option<Spot>,
}

/// Most steps kept for undo.
const UNDO_LIMIT: usize = 500;

#[derive(Debug, Default)]
pub struct Document {
    pub book: Workbook,
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
    pub fn new(book: Workbook) -> Self {
        Self { book, next_id: 1, ..Self::default() }
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
            Change::Cells(sheet, cells) => Change::Cells(sheet, self.book.set_states(sheet, cells)),
            Change::ColWidths(sheet, widths) => Change::ColWidths(
                sheet,
                widths
                    .into_iter()
                    .rev()
                    .filter_map(|(col, w)| {
                        self.book.index_of(sheet)?;
                        Some((col, self.book.set_col_width(sheet, col, w)))
                    })
                    .collect(),
            ),
            Change::InsertSheet(index, sheet) => {
                let id = sheet.id();
                self.book.insert_sheet(index, *sheet);
                Change::RemoveSheet(id)
            }
            Change::RemoveSheet(id) => match self.book.remove_sheet(id) {
                Some((index, sheet)) => Change::InsertSheet(index, Box::new(sheet)),
                None => Change::Batch(Vec::new()),
            },
            Change::RenameSheet(id, name) => match self.book.rename_sheet(id, name) {
                Some(old) => Change::RenameSheet(id, old),
                None => Change::Batch(Vec::new()),
            },
            Change::Batch(parts) => {
                let mut inverse: Vec<Change> = parts.into_iter().map(|p| self.apply_raw(p)).collect();
                inverse.reverse();
                Change::Batch(inverse)
            }
        }
    }

    /// Applies an edit and records it for undo. `spot` is where the user
    /// was, restored when the edit is undone.
    pub fn apply(&mut self, change: Change, spot: Option<Spot>) {
        let inverse = self.apply_raw(change);
        if inverse.is_noop() {
            return;
        }
        let previous = self.state_id;
        self.state_id = self.next_id;
        self.next_id += 1;
        self.undo.push(Step { change: inverse, id: previous, spot });
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Convenience for the common case of setting cell states on one sheet.
    pub fn set_cells(&mut self, sheet: SheetId, cells: Vec<(CellRef, CellState)>, selection: Option<Range>) {
        self.apply(Change::Cells(sheet, cells), Some(Spot { sheet, range: selection }));
    }

    /// Undoes the last step; returns where to look afterwards.
    pub fn undo(&mut self) -> Option<Option<Spot>> {
        let step = self.undo.pop()?;
        let spot = step.change.spot();
        let inverse = self.apply_raw(step.change);
        self.redo.push(Step { change: inverse, id: self.state_id, spot });
        self.state_id = step.id;
        Some(self.where_to_look(step.spot, spot))
    }

    /// Redoes the last undone step; returns where to look afterwards.
    pub fn redo(&mut self) -> Option<Option<Spot>> {
        let step = self.redo.pop()?;
        let spot = step.change.spot();
        let inverse = self.apply_raw(step.change);
        self.undo.push(Step { change: inverse, id: self.state_id, spot });
        self.state_id = step.id;
        Some(self.where_to_look(step.spot, spot))
    }

    /// Prefers where the user was, filling in the cells the change touched
    /// when that is on the same sheet; ignores sheets that are gone.
    fn where_to_look(&self, saved: Option<Spot>, touched: Option<Spot>) -> Option<Spot> {
        let exists = |s: &Spot| self.book.index_of(s.sheet).is_some();
        match (saved.filter(exists), touched.filter(exists)) {
            (Some(s), Some(t)) if s.range.is_none() && s.sheet == t.sheet => Some(t),
            (Some(s), _) => Some(s),
            (None, t) => t,
        }
    }

    // ----- sheets -----

    /// Adds an empty sheet at `index` and returns its id.
    pub fn add_sheet(&mut self, index: usize) -> SheetId {
        let name = self.book.next_sheet_name();
        let sheet = self.book.new_sheet(name);
        let id = sheet.id();
        self.apply(Change::InsertSheet(index, Box::new(sheet)), None);
        id
    }

    /// Copies sheet `id` to just after it, named like "Blad1 (2)".
    pub fn duplicate_sheet(&mut self, id: SheetId) -> Option<SheetId> {
        let index = self.book.index_of(id)?;
        let name = self.book.unique_name(self.book.sheet(index).name());
        let copy = self.book.copy_sheet(id, name)?;
        let new_id = copy.id();
        self.apply(Change::InsertSheet(index + 1, Box::new(copy)), Some(Spot { sheet: id, range: None }));
        Some(new_id)
    }

    /// Renames a sheet and rewrites every formula that names it, on all
    /// sheets, so references follow. Undoable as one step.
    pub fn rename_sheet(&mut self, id: SheetId, name: &str) -> Result<(), String> {
        let index = self.book.index_of(id).ok_or("Dit blad bestaat niet meer.")?;
        let name = self.book.check_name(name, Some(id))?;
        let old = self.book.sheet(index).name().to_string();
        if old == name {
            return Ok(());
        }
        let mut parts = vec![Change::RenameSheet(id, name.clone())];
        parts.extend(self.rewrite_references(&old, Some(&name)));
        self.apply(Change::Batch(parts), Some(Spot { sheet: id, range: None }));
        Ok(())
    }

    /// Deletes a sheet. Formulas on other sheets that named it get `#REF!`,
    /// as in Excel. Undoable as one step, which also restores those formulas.
    pub fn delete_sheet(&mut self, id: SheetId) -> Result<(), String> {
        if self.book.len() <= 1 {
            return Err("Een werkmap heeft minstens één blad nodig.".into());
        }
        let index = self.book.index_of(id).ok_or("Dit blad bestaat niet meer.")?;
        let name = self.book.sheet(index).name().to_string();
        let mut parts: Vec<Change> = self
            .rewrite_references(&name, None)
            .into_iter()
            .filter(|c| !matches!(c, Change::Cells(s, _) if *s == id))
            .collect();
        parts.push(Change::RemoveSheet(id));
        self.apply(Change::Batch(parts), Some(Spot { sheet: id, range: None }));
        Ok(())
    }

    /// Cell changes that point formulas naming sheet `old` at `new`, or at
    /// `#REF!` when `new` is `None`.
    fn rewrite_references(&self, old: &str, new: Option<&str>) -> Vec<Change> {
        let mut changes = Vec::new();
        for sheet in self.book.sheets() {
            let cells: Vec<(CellRef, CellState)> = sheet
                .cells()
                .filter_map(|(at, cell)| {
                    let expr = cell.content.formula().filter(|e| e.mentions_sheet(old))?;
                    let content = Content::Formula(expr.with_sheet_renamed(old, new));
                    Some((at, CellState { content, format: cell.format }))
                })
                .collect();
            if !cells.is_empty() {
                changes.push(Change::Cells(sheet.id(), cells));
            }
        }
        changes
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

    /// A document whose first sheet has id 0.
    fn doc() -> Document {
        Document::new(Workbook::new())
    }

    fn shown(doc: &Document, sheet: SheetId, at: &str) -> String {
        doc.book.sheet_by_id(sheet).unwrap().display(c(at))
    }

    fn source(doc: &Document, sheet: SheetId, at: &str) -> String {
        doc.book.sheet_by_id(sheet).unwrap().state(c(at)).content.source()
    }

    #[test]
    fn undo_redo_restores_values_and_modified_flag() {
        let mut doc = doc();
        doc.set_cells(0, vec![(c("A1"), state("2")), (c("B1"), state("=A1*10"))], None);
        doc.set_cells(0, vec![(c("A1"), state("3"))], None);
        assert_eq!(shown(&doc, 0, "B1"), "30");
        assert!(doc.is_modified());

        assert_eq!(doc.undo(), Some(Some(Spot { sheet: 0, range: Some(Range::single(c("A1"))) })));
        assert_eq!(shown(&doc, 0, "B1"), "20");
        doc.undo();
        assert_eq!(shown(&doc, 0, "B1"), "");
        assert!(!doc.is_modified());
        assert_eq!(doc.undo(), None);

        doc.redo();
        doc.redo();
        assert_eq!(shown(&doc, 0, "B1"), "30");
        doc.mark_saved();
        doc.undo();
        assert!(doc.is_modified());
        doc.redo();
        assert!(!doc.is_modified());

        // A new edit drops the redo stack.
        doc.undo();
        doc.set_cells(0, vec![(c("C1"), state("x"))], None);
        assert!(!doc.can_redo());
    }

    #[test]
    fn repeated_cells_in_one_step_undo_correctly() {
        let mut doc = doc();
        doc.set_cells(0, vec![(c("A1"), state("1"))], None);
        doc.set_cells(0, vec![(c("A1"), state("2")), (c("A1"), state("3"))], None);
        assert_eq!(shown(&doc, 0, "A1"), "3");
        doc.undo();
        assert_eq!(shown(&doc, 0, "A1"), "1");
    }

    #[test]
    fn column_widths_are_undoable() {
        let mut doc = doc();
        doc.apply(Change::ColWidths(0, vec![(2, Some(150.0))]), None);
        assert_eq!(doc.book.sheet(0).col_width(2), 150.0);
        doc.undo();
        assert_eq!(doc.book.sheet(0).custom_col_width(2), None);
        doc.redo();
        assert_eq!(doc.book.sheet(0).col_width(2), 150.0);
    }

    #[test]
    fn no_op_edits_are_not_recorded() {
        let mut doc = doc();
        doc.set_cells(0, vec![(c("A1"), CellState::default())], None);
        assert!(!doc.can_undo());
        assert!(!doc.is_modified());
    }

    #[test]
    fn adding_and_duplicating_sheets() {
        let mut doc = doc();
        doc.set_cells(0, vec![(c("A1"), state("5")), (c("A2"), state("=A1*2"))], None);
        let b2 = doc.add_sheet(1);
        assert_eq!(doc.book.sheet(1).name(), "Blad2");
        doc.set_cells(b2, vec![(c("A1"), state("=Blad1!A2+1"))], None);
        assert_eq!(shown(&doc, b2, "A1"), "11");

        // A copy's unqualified references point at the copy itself.
        let copy = doc.duplicate_sheet(0).unwrap();
        assert_eq!(doc.book.index_of(copy), Some(1));
        assert_eq!(doc.book.sheet(1).name(), "Blad1 (2)");
        doc.set_cells(copy, vec![(c("A1"), state("50"))], None);
        assert_eq!(shown(&doc, copy, "A2"), "100");
        assert_eq!(shown(&doc, 0, "A2"), "10");

        doc.undo();
        doc.undo();
        assert_eq!(doc.book.len(), 2);
        assert_eq!(doc.undo(), Some(Some(Spot { sheet: b2, range: Some(Range::single(c("A1"))) })));
        doc.undo();
        assert_eq!(doc.book.len(), 1);
        doc.redo();
        assert_eq!(doc.book.sheet(1).name(), "Blad2");
    }

    #[test]
    fn renaming_rewrites_references_everywhere_and_undoes() {
        let mut doc = doc();
        let kosten = doc.add_sheet(1);
        doc.set_cells(kosten, vec![(c("A1"), state("40")), (c("A2"), state("=Blad2!A1/2"))], None);
        doc.set_cells(0, vec![(c("B1"), state("=SUM(Blad2!A1:A2)")), (c("B2"), state("='Blad2'!A1"))], None);
        assert_eq!(shown(&doc, 0, "B1"), "60");

        doc.rename_sheet(kosten, "Kosten 2026").unwrap();
        assert_eq!(doc.book.sheet(1).name(), "Kosten 2026");
        assert_eq!(source(&doc, 0, "B1"), "=SUM('Kosten 2026'!A1:A2)");
        assert_eq!(source(&doc, 0, "B2"), "='Kosten 2026'!A1");
        assert_eq!(source(&doc, kosten, "A2"), "='Kosten 2026'!A1/2");
        assert_eq!(shown(&doc, 0, "B1"), "60");

        // References keep working and recalculating under the new name.
        doc.set_cells(kosten, vec![(c("A1"), state("100"))], None);
        assert_eq!(shown(&doc, 0, "B1"), "150");

        doc.undo();
        doc.undo();
        assert_eq!(doc.book.sheet(1).name(), "Blad2");
        assert_eq!(source(&doc, 0, "B1"), "=SUM(Blad2!A1:A2)");
        assert_eq!(shown(&doc, 0, "B1"), "60");
        doc.redo();
        assert_eq!(source(&doc, 0, "B1"), "=SUM('Kosten 2026'!A1:A2)");
    }

    #[test]
    fn rename_rejects_bad_and_duplicate_names() {
        let mut doc = doc();
        let b2 = doc.add_sheet(1);
        assert!(doc.rename_sheet(b2, "blad1").is_err());
        assert!(doc.rename_sheet(b2, "").is_err());
        assert!(doc.rename_sheet(b2, "a/b").is_err());
        assert!(doc.rename_sheet(b2, &"x".repeat(32)).is_err());
        assert!(doc.rename_sheet(b2, "'quote").is_err());
        // Changing only the case of its own name is fine.
        assert!(doc.rename_sheet(b2, "BLAD2").is_ok());
    }

    #[test]
    fn deleting_a_sheet_turns_references_into_ref_errors_and_undo_restores() {
        let mut doc = doc();
        let b2 = doc.add_sheet(1);
        doc.set_cells(b2, vec![(c("A1"), state("7"))], None);
        doc.set_cells(0, vec![(c("A1"), state("=Blad2!A1*2"))], None);
        assert_eq!(shown(&doc, 0, "A1"), "14");

        doc.delete_sheet(b2).unwrap();
        assert_eq!(doc.book.len(), 1);
        assert_eq!(source(&doc, 0, "A1"), "=#REF!*2");
        assert_eq!(shown(&doc, 0, "A1"), "#REF!");
        assert!(doc.delete_sheet(0).is_err());

        let spot = doc.undo().unwrap();
        assert_eq!(spot.map(|s| s.sheet), Some(b2));
        assert_eq!(source(&doc, 0, "A1"), "=Blad2!A1*2");
        assert_eq!(shown(&doc, 0, "A1"), "14");
        assert_eq!(shown(&doc, b2, "A1"), "7");
        // The restored sheet keeps its id, so older steps still apply to it.
        doc.undo();
        doc.undo();
        assert_eq!(shown(&doc, b2, "A1"), "");
    }

    #[test]
    fn a_reference_to_a_missing_sheet_starts_working_when_it_appears() {
        let mut doc = doc();
        doc.set_cells(0, vec![(c("A1"), state("=Later!B2+1"))], None);
        assert_eq!(shown(&doc, 0, "A1"), "#REF!");
        let id = doc.add_sheet(1);
        doc.rename_sheet(id, "Later").unwrap();
        assert_eq!(shown(&doc, 0, "A1"), "1");
        doc.set_cells(id, vec![(c("B2"), state("41"))], None);
        assert_eq!(shown(&doc, 0, "A1"), "42");
    }
}
