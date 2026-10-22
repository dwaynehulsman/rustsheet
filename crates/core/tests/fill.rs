//! The fill handle and Fill Down/Right.

use rustsheet_core::fill::{fill, fill_target};
use rustsheet_core::{CellRef, CellState, Range, Workbook, parse_input};

fn c(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

fn r(a: &str, b: &str) -> Range {
    Range::new(c(a), c(b))
}

fn book(cells: &[(&str, &str)]) -> Workbook {
    let mut b = Workbook::new();
    for (at, text) in cells {
        let p = parse_input(text).unwrap();
        let format = rustsheet_core::CellFormat { num: p.format.unwrap_or_default(), ..Default::default() };
        b.set_states(0, vec![(c(at), CellState { content: p.content, format })]);
    }
    b
}

/// Fills and returns the displayed values of `target`, row by row.
fn run(b: &mut Workbook, source: Range, target: Range, series: bool) -> Vec<String> {
    let changes = fill(b.sheet(0), source, target, series);
    b.set_states(0, changes);
    target.iter().map(|at| b.sheet(0).display(at)).collect()
}

#[test]
fn number_series_and_copies() {
    let mut b = book(&[("A1", "1"), ("A2", "2"), ("B1", "5"), ("C1", "10"), ("C2", "7.5")]);
    assert_eq!(run(&mut b, r("A1", "A2"), r("A3", "A6"), true), ["3", "4", "5", "6"]);
    // A single number is copied, like Excel.
    assert_eq!(run(&mut b, r("B1", "B1"), r("B2", "B3"), true), ["5", "5"]);
    assert_eq!(run(&mut b, r("C1", "C2"), r("C3", "C4"), true), ["5", "2.5"]);
    // Upwards continues backwards.
    let mut b = book(&[("A5", "10"), ("A6", "20")]);
    assert_eq!(run(&mut b, r("A5", "A6"), r("A2", "A4"), true), ["-20", "-10", "0"]);
}

#[test]
fn dates_names_and_numbered_text() {
    let mut b = book(&[
        ("A1", "30-12-2026"),
        ("B1", "Week 1"),
        ("C1", "januari"),
        ("D1", "Ma"),
        ("E1", "Q1-2026"),
        ("F1", "Item 007"),
    ]);
    assert_eq!(run(&mut b, r("A1", "A1"), r("A2", "A4"), true), ["31-12-2026", "01-01-2027", "02-01-2027"]);
    assert_eq!(run(&mut b, r("B1", "B1"), r("B2", "B3"), true), ["Week 2", "Week 3"]);
    assert_eq!(run(&mut b, r("C1", "C1"), r("C2", "C3"), true), ["februari", "maart"]);
    assert_eq!(run(&mut b, r("D1", "D1"), r("D2", "D8"), true), ["Di", "Wo", "Do", "Vr", "Za", "Zo", "Ma"]);
    assert_eq!(run(&mut b, r("E1", "E1"), r("E2", "E2"), true), ["Q1-2027"]);
    assert_eq!(run(&mut b, r("F1", "F1"), r("F2", "F2"), true), ["Item 008"]);
    let mut b = book(&[("A1", "jan"), ("A2", "mrt")]);
    assert_eq!(run(&mut b, r("A1", "A2"), r("A3", "A4"), true), ["mei", "jul"]);
}

#[test]
fn formulas_shift_and_patterns_repeat() {
    let mut b = book(&[("A1", "1"), ("A2", "2"), ("A3", "3"), ("B1", "=A1*10"), ("C1", "x"), ("C2", "y")]);
    assert_eq!(run(&mut b, r("B1", "B1"), r("B2", "B3"), true), ["20", "30"]);
    assert_eq!(b.sheet(0).state(c("B3")).content.source(), "=A3*10");
    assert_eq!(run(&mut b, r("C1", "C2"), r("C3", "C5"), true), ["x", "y", "x"]);
    // Horizontally, several rows at once.
    let mut b = book(&[("A1", "1"), ("B1", "3"), ("A2", "ma"), ("B2", "wo")]);
    assert_eq!(run(&mut b, r("A1", "B2"), r("C1", "D2"), true), ["5", "7", "vr", "zo"]);
}

#[test]
fn fill_down_copies_without_series() {
    let mut b = book(&[("A1", "1"), ("B1", "=A1+1")]);
    assert_eq!(run(&mut b, r("A1", "B1"), r("A2", "B3"), false), ["1", "2", "1", "2"]);
    assert_eq!(b.sheet(0).state(c("B3")).content.source(), "=A3+1");
}

#[test]
fn targets_follow_the_drag_direction() {
    let s = r("B2", "C3");
    assert_eq!(fill_target(s, r("B2", "C6")), Some(r("B4", "C6")));
    assert_eq!(fill_target(s, r("B1", "C3")), Some(r("B1", "C1")));
    assert_eq!(fill_target(s, r("B2", "E3")), Some(r("D2", "E3")));
    assert_eq!(fill_target(s, r("A2", "C3")), Some(r("A2", "A3")));
    assert_eq!(fill_target(s, s), None);
}
