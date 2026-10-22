//! Inserting and deleting rows and columns, with references that follow.

use rustsheet_core::{CellRef, CellState, Document, SheetId, Workbook, parse_input};

fn c(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

fn set(doc: &mut Document, sheet: SheetId, at: &str, text: &str) {
    let content = parse_input(text).unwrap().content;
    doc.set_cells(sheet, vec![(c(at), CellState { content, ..Default::default() })], None);
}

fn src(doc: &Document, sheet: SheetId, at: &str) -> String {
    doc.book.sheet_by_id(sheet).unwrap().state(c(at)).content.source()
}

fn shown(doc: &Document, sheet: SheetId, at: &str) -> String {
    doc.book.sheet_by_id(sheet).unwrap().display(c(at))
}

/// A1:A5 = 1..5, B1 = SUM(A1:A5), B2 = A3*2, B3 = $A$4, C1 = SUM(A:A)
fn doc() -> Document {
    let mut d = Document::new(Workbook::new());
    for i in 1..=5 {
        set(&mut d, 0, &format!("A{i}"), &i.to_string());
    }
    set(&mut d, 0, "B1", "=SUM(A1:A5)");
    set(&mut d, 0, "B2", "=A3*2");
    set(&mut d, 0, "B3", "=$A$4");
    set(&mut d, 0, "C1", "=SUM(A:A)");
    d
}

#[test]
fn inserting_rows_moves_cells_and_references() {
    let mut d = doc();
    // Two rows before row 3.
    d.insert_lines(0, true, 2, 2).unwrap();
    assert_eq!(shown(&d, 0, "A3"), "");
    assert_eq!(shown(&d, 0, "A5"), "3");
    assert_eq!(src(&d, 0, "B1"), "=SUM(A1:A7)");
    assert_eq!(src(&d, 0, "B2"), "=A5*2");
    // B3 itself moved to B5; absolute references move too.
    assert_eq!(src(&d, 0, "B5"), "=$A$6");
    assert_eq!(src(&d, 0, "C1"), "=SUM(A:A)");
    assert_eq!(shown(&d, 0, "B1"), "15");
    // Values typed into the new rows are picked up by the grown range.
    set(&mut d, 0, "A3", "100");
    assert_eq!(shown(&d, 0, "B1"), "115");

    d.undo();
    d.undo();
    assert_eq!(src(&d, 0, "B1"), "=SUM(A1:A5)");
    assert_eq!(src(&d, 0, "B3"), "=$A$4");
    assert_eq!(shown(&d, 0, "A3"), "3");
    assert_eq!(shown(&d, 0, "B1"), "15");
}

#[test]
fn inserting_at_the_top_of_a_range_shifts_it() {
    let mut d = doc();
    d.insert_lines(0, true, 0, 1).unwrap();
    assert_eq!(src(&d, 0, "B2"), "=SUM(A2:A6)");
    assert_eq!(shown(&d, 0, "B2"), "15");
}

#[test]
fn deleting_rows_shrinks_ranges_and_breaks_references_into_them() {
    let mut d = doc();
    // Delete rows 3 and 4 (A3=3, A4=4, B3).
    d.delete_lines(0, true, 2, 2).unwrap();
    assert_eq!(src(&d, 0, "B1"), "=SUM(A1:A3)");
    assert_eq!(shown(&d, 0, "B1"), "8");
    assert_eq!(src(&d, 0, "B2"), "=#REF!*2");
    assert_eq!(shown(&d, 0, "B2"), "#REF!");
    assert_eq!(shown(&d, 0, "A3"), "5");
    assert_eq!(shown(&d, 0, "B3"), "");

    // Deleting everything a range covers turns it into #REF!.
    set(&mut d, 0, "D1", "=SUM(A2:A2)");
    d.delete_lines(0, true, 1, 1).unwrap();
    assert_eq!(src(&d, 0, "D1"), "=SUM(#REF!)");

    d.undo();
    d.undo();
    d.undo();
    assert_eq!(src(&d, 0, "B1"), "=SUM(A1:A5)");
    assert_eq!(src(&d, 0, "B2"), "=A3*2");
    assert_eq!(src(&d, 0, "B3"), "=$A$4");
    assert_eq!(shown(&d, 0, "B2"), "6");
    assert_eq!(shown(&d, 0, "A4"), "4");
}

#[test]
fn columns_work_the_same_and_keep_widths() {
    let mut d = Document::new(Workbook::new());
    set(&mut d, 0, "A1", "1");
    set(&mut d, 0, "B1", "2");
    set(&mut d, 0, "C1", "3");
    set(&mut d, 0, "D1", "=SUM(A1:C1)");
    set(&mut d, 0, "E1", "=SUM(B:B)");
    d.apply(rustsheet_core::Change::ColWidths(0, vec![(2, Some(150.0))]), None);

    d.insert_lines(0, false, 1, 1).unwrap();
    assert_eq!(src(&d, 0, "E1"), "=SUM(A1:D1)");
    assert_eq!(src(&d, 0, "F1"), "=SUM(C:C)");
    assert_eq!(d.book.sheet(0).custom_col_width(3), Some(150.0));
    assert_eq!(shown(&d, 0, "E1"), "6");

    d.delete_lines(0, false, 3, 1).unwrap();
    assert_eq!(src(&d, 0, "D1"), "=SUM(A1:C1)");
    assert_eq!(shown(&d, 0, "D1"), "3");
    assert_eq!(d.book.sheet(0).custom_col_width(3), None);

    d.undo();
    assert_eq!(d.book.sheet(0).custom_col_width(3), Some(150.0));
    d.undo();
    assert_eq!(src(&d, 0, "D1"), "=SUM(A1:C1)");
    assert_eq!(d.book.sheet(0).custom_col_width(2), Some(150.0));
}

#[test]
fn references_from_other_sheets_follow() {
    let mut d = doc();
    let b2 = d.add_sheet(1);
    set(&mut d, b2, "A1", "=Blad1!A5+SUM(Blad1!A1:A5)");
    set(&mut d, b2, "A2", "=A1");
    d.insert_lines(0, true, 0, 3).unwrap();
    assert_eq!(src(&d, b2, "A1"), "=Blad1!A8+SUM(Blad1!A4:A8)");
    // References within Blad2 itself are untouched.
    assert_eq!(src(&d, b2, "A2"), "=A1");
    assert_eq!(shown(&d, b2, "A1"), "20");
    d.delete_lines(0, true, 7, 1).unwrap();
    assert_eq!(src(&d, b2, "A1"), "=#REF!+SUM(Blad1!A4:A7)");
}

#[test]
fn inserting_refuses_to_push_data_off_the_sheet() {
    let mut d = Document::new(Workbook::new());
    set(&mut d, 0, "A1048576", "laatste");
    assert!(d.insert_lines(0, true, 0, 1).is_err());
    assert_eq!(shown(&d, 0, "A1048576"), "laatste");
    assert!(d.insert_lines(0, false, 0, 1).is_ok());
}
