//! End-to-end formula behaviour: typed input in, displayed value out.

use rustsheet_core::{CellRef, CellState, Sheet, Value, parse_input};

fn c(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

fn set(sheet: &mut Sheet, at: &str, text: &str) {
    let parsed = parse_input(text).unwrap_or_else(|e| panic!("{text}: {e}"));
    let format = rustsheet_core::CellFormat { num: parsed.format.unwrap_or_default(), ..Default::default() };
    sheet.set_states(vec![(c(at), CellState { content: parsed.content, format })]);
}

/// Evaluates a formula in a scratch cell of `sheet`.
fn eval_in(sheet: &mut Sheet, formula: &str) -> String {
    set(sheet, "Z100", formula);
    sheet.display(c("Z100"))
}

fn eval(formula: &str) -> String {
    eval_in(&mut sample(), formula)
}

/// A1:A5 = 1..5, B1 = "tekst", B2 = TRUE, B3 = "7", C1 = 0
fn sample() -> Sheet {
    let mut s = Sheet::new("Blad1");
    for i in 1..=5 {
        set(&mut s, &format!("A{i}"), &i.to_string());
    }
    set(&mut s, "B1", "tekst");
    set(&mut s, "B2", "TRUE");
    set(&mut s, "B3", "'7");
    set(&mut s, "C1", "0");
    s
}

#[test]
fn arithmetic_and_precedence() {
    assert_eq!(eval("=1+2*3"), "7");
    assert_eq!(eval("=(1+2)*3"), "9");
    assert_eq!(eval("=2^3^2"), "64");
    assert_eq!(eval("=-2^2"), "4");
    assert_eq!(eval("=10/4"), "2.5");
    assert_eq!(eval("=7-2-1"), "4");
    assert_eq!(eval("=50%*A4"), "2");
    assert_eq!(eval("=A1+A2*A3"), "7");
    assert_eq!(eval("=$A$5-A$1"), "4");
    assert_eq!(eval("=0.1+0.2"), "0.3");
    assert_eq!(eval("=\"a\"&1&TRUE"), "a1TRUE");
    assert_eq!(eval("=B3*2"), "14");
    assert_eq!(eval("=B2+1"), "2");
    assert_eq!(eval("=Q50+1"), "1");
}

#[test]
fn comparisons() {
    assert_eq!(eval("=A1<A2"), "TRUE");
    assert_eq!(eval("=\"abc\"=\"ABC\""), "TRUE");
    assert_eq!(eval("=A1<>1"), "FALSE");
    assert_eq!(eval("=1<\"a\""), "TRUE");
    assert_eq!(eval("=A5>=5"), "TRUE");
    assert_eq!(eval("=1+1=2"), "TRUE");
}

#[test]
fn functions() {
    assert_eq!(eval("=SUM(A1:A5)"), "15");
    assert_eq!(eval("=SUM(A1:A5,10,B1:B3)"), "25");
    assert_eq!(eval("=SUM(A:A)"), "15");
    assert_eq!(eval("=SUM(1:2)"), "3");
    assert_eq!(eval("=SUM(\"2\",TRUE)"), "3");
    assert_eq!(eval("=AVERAGE(A1:A5)"), "3");
    assert_eq!(eval("=AVERAGE(A1:B5)"), "3");
    assert_eq!(eval("=MIN(A1:A5,-1)"), "-1");
    assert_eq!(eval("=MAX(A1:A5)"), "5");
    assert_eq!(eval("=MAX(B1:B3)"), "0");
    assert_eq!(eval("=COUNT(A1:B5)"), "5");
    assert_eq!(eval("=COUNT(1,\"2\",\"x\")"), "2");
    assert_eq!(eval("=COUNTA(A1:B5)"), "8");
    assert_eq!(eval("=IF(A1>0,\"ja\",\"nee\")"), "ja");
    assert_eq!(eval("=IF(C1,\"ja\",\"nee\")"), "nee");
    assert_eq!(eval("=IF(C1,1)"), "FALSE");
    assert_eq!(eval("=IF(A1=1,A2,1/0)"), "2");
    assert_eq!(eval("=ROUND(2.675,2)"), "2.68");
    assert_eq!(eval("=ROUND(-2.5,0)"), "-3");
    assert_eq!(eval("=ROUND(1234.5678,-2)"), "1200");
    assert_eq!(eval("=ROUND(1/3,3)"), "0.333");
    assert_eq!(eval("=CONCAT(\"x\",A1:A3,B1)"), "x123tekst");
    assert_eq!(eval("=CONCATENATE(\"a\",\"b\")"), "ab");
    assert_eq!(eval("=AND(A1>0,B2)"), "TRUE");
    assert_eq!(eval("=OR(C1,FALSE)"), "FALSE");
    assert_eq!(eval("=NOT(C1)"), "TRUE");
    assert_eq!(eval("=ABS(-3)"), "3");
    assert_eq!(eval("=IFERROR(1/0,\"oeps\")"), "oeps");
    assert_eq!(eval("=LEN(B1)"), "5");
    assert_eq!(eval("=UPPER(B1)&LOWER(\"X\")"), "TEKSTx");
    assert_eq!(eval("=sum(a1:a2)"), "3");
}

#[test]
fn error_values() {
    assert_eq!(eval("=1/0"), "#DIV/0!");
    assert_eq!(eval("=A1/C1"), "#DIV/0!");
    assert_eq!(eval("=AVERAGE(B1)"), "#DIV/0!");
    assert_eq!(eval("=B1+1"), "#VALUE!");
    assert_eq!(eval("=SUM(A1,\"x\")"), "#VALUE!");
    assert_eq!(eval("=ONBEKEND(1)"), "#NAME?");
    assert_eq!(eval("=foo+1"), "#NAME?");
    assert_eq!(eval("=Blad2!A1"), "#REF!");
    assert_eq!(eval("=Blad1!A2"), "2");
    assert_eq!(eval("=#REF!+1"), "#REF!");
    assert_eq!(eval("=0^0"), "#NUM!");
    assert_eq!(eval("=10^400"), "#NUM!");
    assert_eq!(eval("=IF(1,2,3,4)"), "#VALUE!");
    assert_eq!(eval("=SUM(A1:A3)+A1:A3"), "#VALUE!");
    // Errors flow through ranges and references.
    let mut s = sample();
    set(&mut s, "D1", "=1/0");
    assert_eq!(eval_in(&mut s, "=SUM(D1:D3)"), "#DIV/0!");
    assert_eq!(eval_in(&mut s, "=D1&\"x\""), "#DIV/0!");
    assert_eq!(eval_in(&mut s, "=COUNT(D1,A1)"), "1");
    assert_eq!(eval_in(&mut s, "=IFERROR(D1,0)"), "0");
}

#[test]
fn circular_references() {
    let mut s = Sheet::new("Blad1");
    set(&mut s, "A1", "=A1+1");
    assert_eq!(s.display(c("A1")), "#CIRC!");

    set(&mut s, "B1", "=B3");
    set(&mut s, "B2", "=B1+1");
    set(&mut s, "B3", "=B2*2");
    for at in ["B1", "B2", "B3"] {
        assert_eq!(s.display(c(at)), "#CIRC!", "{at}");
    }
    // A cell that only reads the cycle gets the error, but is not part of it.
    set(&mut s, "C1", "=B1");
    assert_eq!(s.display(c("C1")), "#CIRC!");
    set(&mut s, "C2", "=SUM(B1:B3)");
    assert_eq!(s.display(c("C2")), "#CIRC!");

    // Breaking the cycle recovers every cell.
    set(&mut s, "B1", "5");
    assert_eq!(s.display(c("B2")), "6");
    assert_eq!(s.display(c("B3")), "12");
    assert_eq!(s.display(c("C1")), "5");
    assert_eq!(s.display(c("C2")), "23");

    // A range that includes its own cell is circular too.
    set(&mut s, "D5", "=SUM(D1:D10)");
    assert_eq!(s.display(c("D5")), "#CIRC!");
}

#[test]
fn only_affected_cells_recalculate() {
    let mut s = Sheet::new("Blad1");
    for i in 1..=100 {
        set(&mut s, &format!("A{i}"), &i.to_string());
        set(&mut s, &format!("B{i}"), &format!("=A{i}*2"));
    }
    set(&mut s, "C1", "=SUM(B1:B100)");
    set(&mut s, "D1", "=C1+1");
    set(&mut s, "E1", "=A100");
    assert_eq!(s.display(c("D1")), "10101");

    // Changing A50 touches B50, C1 and D1 and nothing else.
    set(&mut s, "A50", "0");
    assert_eq!(s.last_recalc_count(), 3);
    assert_eq!(s.display(c("D1")), "10001");

    // A value nobody reads triggers no formulas at all.
    set(&mut s, "Z1", "42");
    assert_eq!(s.last_recalc_count(), 0);

    // Editing a formula evaluates it and its dependents only.
    set(&mut s, "C1", "=SUM(B1:B10)");
    assert_eq!(s.last_recalc_count(), 2);
    assert_eq!(s.display(c("D1")), "111");
    // The old range no longer triggers C1.
    set(&mut s, "A60", "1");
    assert_eq!(s.last_recalc_count(), 1);
    assert_eq!(s.display(c("D1")), "111");
}

#[test]
fn long_chains_do_not_overflow_the_stack() {
    let mut s = Sheet::new("Blad1");
    set(&mut s, "A1", "1");
    for i in 2..=100_000 {
        let at = CellRef::new(i - 1, 0);
        let content = parse_input(&format!("=A{}+1", i - 1)).unwrap().content;
        s.put(at, CellState { content, ..Default::default() });
    }
    s.recalc_all();
    assert_eq!(s.value(c("A100000")), &Value::Number(100_000.0));
    set(&mut s, "A1", "10");
    assert_eq!(s.last_recalc_count(), 99_999);
    assert_eq!(s.value(c("A100000")), &Value::Number(100_009.0));
}

#[test]
fn deleting_a_cell_updates_dependents() {
    let mut s = sample();
    set(&mut s, "D1", "=A1+A2");
    set(&mut s, "A1", "");
    assert_eq!(s.display(c("D1")), "2");
}
