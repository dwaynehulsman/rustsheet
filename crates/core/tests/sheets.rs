//! Formulas across sheets: references, recalculation and renaming.

use rustsheet_core::{CellRef, CellState, Document, SheetId, Workbook, parse_input};

fn c(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

fn set(doc: &mut Document, sheet: SheetId, at: &str, text: &str) {
    let content = parse_input(text).unwrap_or_else(|e| panic!("{text}: {e}")).content;
    doc.set_cells(sheet, vec![(c(at), CellState { content, ..Default::default() })], None);
}

fn shown(doc: &Document, sheet: SheetId, at: &str) -> String {
    doc.book.sheet_by_id(sheet).unwrap().display(c(at))
}

fn source(doc: &Document, sheet: SheetId, at: &str) -> String {
    doc.book.sheet_by_id(sheet).unwrap().state(c(at)).content.source()
}

/// Blad1 (id 0), Blad2 and 'Mijn kosten'.
fn three_sheets() -> (Document, SheetId, SheetId) {
    let mut doc = Document::new(Workbook::new());
    let b2 = doc.add_sheet(1);
    let kosten = doc.add_sheet(2);
    doc.rename_sheet(kosten, "Mijn kosten").unwrap();
    (doc, b2, kosten)
}

#[test]
fn references_to_other_sheets_evaluate_and_recalculate() {
    let (mut doc, b2, kosten) = three_sheets();
    set(&mut doc, b2, "A1", "10");
    set(&mut doc, kosten, "B2", "5");
    set(&mut doc, 0, "A1", "=Blad2!A1*2");
    set(&mut doc, 0, "A2", "='Mijn kosten'!B2+blad2!A1");
    set(&mut doc, 0, "A3", "=SUM(Blad2!A1:A3,'Mijn kosten'!B:B)");
    assert_eq!(shown(&doc, 0, "A1"), "20");
    assert_eq!(shown(&doc, 0, "A2"), "15");
    assert_eq!(shown(&doc, 0, "A3"), "15");

    // Changing a cell on Blad2 recalculates exactly the formulas on Blad1 that read it.
    set(&mut doc, b2, "A1", "100");
    assert_eq!(doc.book.last_recalc_count(), 3);
    assert_eq!(shown(&doc, 0, "A1"), "200");
    assert_eq!(shown(&doc, 0, "A2"), "105");
    set(&mut doc, b2, "A3", "1");
    assert_eq!(doc.book.last_recalc_count(), 1);
    assert_eq!(shown(&doc, 0, "A3"), "106");
    set(&mut doc, kosten, "B900", "4");
    assert_eq!(shown(&doc, 0, "A3"), "110");
}

#[test]
fn chains_across_sheets_and_cycles() {
    let (mut doc, b2, kosten) = three_sheets();
    set(&mut doc, 0, "A1", "1");
    set(&mut doc, b2, "A1", "=Blad1!A1+1");
    set(&mut doc, kosten, "A1", "=Blad2!A1+1");
    set(&mut doc, 0, "B1", "='Mijn kosten'!A1+1");
    assert_eq!(shown(&doc, 0, "B1"), "4");
    set(&mut doc, 0, "A1", "10");
    assert_eq!(shown(&doc, 0, "B1"), "13");

    // A cycle through three sheets.
    set(&mut doc, 0, "A1", "=B1");
    for (sheet, at) in [(0, "A1"), (b2, "A1"), (kosten, "A1"), (0, "B1")] {
        assert_eq!(shown(&doc, sheet, at), "#CIRC!");
    }
    set(&mut doc, 0, "A1", "0");
    assert_eq!(shown(&doc, 0, "B1"), "3");
}

#[test]
fn missing_sheets_give_ref_errors() {
    let (mut doc, _, _) = three_sheets();
    set(&mut doc, 0, "A1", "=Bestaatniet!A1");
    set(&mut doc, 0, "A2", "=SUM('Ook niet'!A1:A9)");
    assert_eq!(shown(&doc, 0, "A1"), "#REF!");
    assert_eq!(shown(&doc, 0, "A2"), "#REF!");
}

#[test]
fn renaming_updates_formulas_on_every_sheet() {
    let (mut doc, b2, kosten) = three_sheets();
    set(&mut doc, b2, "A1", "3");
    set(&mut doc, 0, "A1", "=Blad2!A1+BLAD2!A1");
    set(&mut doc, kosten, "A1", "=SUM(Blad2!A1:B2)*2");
    // A reference to its own sheet by name follows too.
    set(&mut doc, b2, "B1", "=Blad2!A1*10");

    doc.rename_sheet(b2, "Omzet Q4").unwrap();
    assert_eq!(source(&doc, 0, "A1"), "='Omzet Q4'!A1+'Omzet Q4'!A1");
    assert_eq!(source(&doc, kosten, "A1"), "=SUM('Omzet Q4'!A1:B2)*2");
    assert_eq!(source(&doc, b2, "B1"), "='Omzet Q4'!A1*10");
    assert_eq!(shown(&doc, kosten, "A1"), "66");

    // And again, to a name that needs no quotes, then undo both.
    doc.rename_sheet(b2, "Omzet").unwrap();
    assert_eq!(source(&doc, 0, "A1"), "=Omzet!A1+Omzet!A1");
    set(&mut doc, b2, "A1", "4");
    assert_eq!(shown(&doc, 0, "A1"), "8");
    doc.undo();
    doc.undo();
    doc.undo();
    // Undo restores the text exactly as it was typed.
    assert_eq!(source(&doc, 0, "A1"), "=Blad2!A1+BLAD2!A1");
    assert_eq!(doc.book.sheet_by_id(b2).unwrap().name(), "Blad2");
    assert_eq!(shown(&doc, 0, "A1"), "6");
}

#[test]
fn renaming_to_a_name_with_quotes_round_trips() {
    let (mut doc, b2, _) = three_sheets();
    set(&mut doc, b2, "A1", "2");
    set(&mut doc, 0, "A1", "=Blad2!A1");
    doc.rename_sheet(b2, "Jan's blad").unwrap();
    assert_eq!(source(&doc, 0, "A1"), "='Jan''s blad'!A1");
    assert_eq!(shown(&doc, 0, "A1"), "2");
}

#[test]
fn pasting_a_formula_shifts_references_to_other_sheets() {
    let (mut doc, b2, _) = three_sheets();
    set(&mut doc, b2, "A2", "9");
    set(&mut doc, 0, "B1", "=Blad2!A1+$A$1");
    let block =
        rustsheet_core::clipboard::Block::copy(doc.book.sheet(0), rustsheet_core::Range::single(c("B1")));
    let (_, changes) = block.paste(doc.book.sheet(0), rustsheet_core::Range::single(c("B2")), false);
    doc.set_cells(0, changes, None);
    assert_eq!(source(&doc, 0, "B2"), "=Blad2!A2+$A$1");
    assert_eq!(shown(&doc, 0, "B2"), "9");
}
