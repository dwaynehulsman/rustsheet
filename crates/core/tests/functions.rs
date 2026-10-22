//! The function library, one test per family, on a small product table.

use rustsheet_core::{CellRef, CellState, Workbook, parse_input};

fn c(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

fn set(book: &mut Workbook, at: &str, text: &str) {
    let parsed = parse_input(text).unwrap_or_else(|e| panic!("{text}: {e}"));
    let format = rustsheet_core::CellFormat { num: parsed.format.unwrap_or_default(), ..Default::default() };
    book.set_states(0, vec![(c(at), CellState { content: parsed.content, format })]);
}

/// A1:D6
///   Product  Categorie  Prijs  Voorraad
///   Appel    Fruit      0.5    100
///   Peer     Fruit      0.75   0
///   Kaas     Zuivel     4.2    12
///   Melk     Zuivel     1.1    30
///   Ananas   Fruit      2.5    7
fn table() -> Workbook {
    let mut b = Workbook::new();
    let rows = [
        ["Product", "Categorie", "Prijs", "Voorraad"],
        ["Appel", "Fruit", "0.5", "100"],
        ["Peer", "Fruit", "0.75", "0"],
        ["Kaas", "Zuivel", "4.2", "12"],
        ["Melk", "Zuivel", "1.1", "30"],
        ["Ananas", "Fruit", "2.5", "7"],
    ];
    for (r, row) in rows.iter().enumerate() {
        for (col, text) in row.iter().enumerate() {
            set(&mut b, &format!("{}{}", ["A", "B", "C", "D"][col], r + 1), text);
        }
    }
    b
}

fn eval(book: &mut Workbook, formula: &str) -> String {
    set(book, "Z1", formula);
    book.sheet(0).display(c("Z1"))
}

#[test]
fn lookups() {
    let mut b = table();
    assert_eq!(eval(&mut b, "=VLOOKUP(\"Kaas\",A2:D6,3,FALSE)"), "4.2");
    assert_eq!(eval(&mut b, "=VLOOKUP(\"kaas\",A2:D6,4,0)"), "12");
    assert_eq!(eval(&mut b, "=VLOOKUP(\"Ana*\",A2:D6,2,FALSE)"), "Fruit");
    assert_eq!(eval(&mut b, "=VLOOKUP(\"Brood\",A2:D6,3,FALSE)"), "#N/A");
    assert_eq!(eval(&mut b, "=VLOOKUP(\"Kaas\",A2:D6,5,FALSE)"), "#REF!");
    assert_eq!(eval(&mut b, "=VLOOKUP(\"Kaas\",A2:D6,0,FALSE)"), "#VALUE!");
    assert_eq!(eval(&mut b, "=HLOOKUP(\"Prijs\",A1:D6,3,FALSE)"), "0.75");
    assert_eq!(eval(&mut b, "=MATCH(\"Melk\",A1:A6,0)"), "5");
    assert_eq!(eval(&mut b, "=INDEX(A1:D6,4,3)"), "4.2");
    assert_eq!(eval(&mut b, "=INDEX(C2:C6,2)"), "0.75");
    assert_eq!(eval(&mut b, "=INDEX(A1:D6,9,1)"), "#REF!");
    assert_eq!(eval(&mut b, "=INDEX(C2:C6,MATCH(\"Peer\",A2:A6,0))"), "0.75");
    assert_eq!(eval(&mut b, "=XLOOKUP(\"Melk\",A2:A6,D2:D6)"), "30");
    assert_eq!(eval(&mut b, "=XLOOKUP(\"Brood\",A2:A6,D2:D6,\"geen\")"), "geen");
    assert_eq!(eval(&mut b, "=XLOOKUP(\"Fruit\",B2:B6,A2:A6,,0,-1)"), "Ananas");
    // Next smaller and next larger, without sorted data.
    assert_eq!(eval(&mut b, "=XLOOKUP(3,C2:C6,A2:A6,,-1)"), "Ananas");
    assert_eq!(eval(&mut b, "=XLOOKUP(3,C2:C6,A2:A6,,1)"), "Kaas");

    // Approximate match on a sorted table (tax brackets).
    let mut t = Workbook::new();
    for (r, (from, rate)) in
        [("0", "0%"), ("1000", "10%"), ("5000", "20%"), ("20000", "40%")].iter().enumerate()
    {
        set(&mut t, &format!("A{}", r + 1), from);
        set(&mut t, &format!("B{}", r + 1), rate);
    }
    assert_eq!(eval(&mut t, "=VLOOKUP(7500,A1:B4,2)"), "0.2");
    assert_eq!(eval(&mut t, "=VLOOKUP(20000,A1:B4,2,TRUE)"), "0.4");
    assert_eq!(eval(&mut t, "=MATCH(4999,A1:A4)"), "2");
    assert_eq!(eval(&mut t, "=VLOOKUP(-1,A1:B4,2)"), "#N/A");
}

#[test]
fn conditional_aggregates() {
    let mut b = table();
    assert_eq!(eval(&mut b, "=COUNTIF(B2:B6,\"Fruit\")"), "3");
    assert_eq!(eval(&mut b, "=COUNTIF(D2:D6,\">10\")"), "3");
    assert_eq!(eval(&mut b, "=COUNTIF(D2:D6,0)"), "1");
    assert_eq!(eval(&mut b, "=COUNTIF(A2:A6,\"*a*\")"), "3");
    assert_eq!(eval(&mut b, "=COUNTIF(A2:A6,\"<>Peer\")"), "4");
    assert_eq!(eval(&mut b, "=COUNTIF(A2:A9,\"\")"), "3");
    assert_eq!(eval(&mut b, "=SUMIF(B2:B6,\"Fruit\",D2:D6)"), "107");
    assert_eq!(eval(&mut b, "=SUMIF(C2:C6,\">=1\")"), "7.8");
    assert_eq!(eval(&mut b, "=AVERAGEIF(B2:B6,\"Zuivel\",C2:C6)"), "2.65");
    assert_eq!(eval(&mut b, "=AVERAGEIF(B2:B6,\"Vlees\",C2:C6)"), "#DIV/0!");
    assert_eq!(eval(&mut b, "=SUMIFS(D2:D6,B2:B6,\"Fruit\",C2:C6,\">1\")"), "7");
    assert_eq!(eval(&mut b, "=COUNTIFS(B2:B6,\"Fruit\",D2:D6,\">0\")"), "2");
    assert_eq!(eval(&mut b, "=AVERAGEIFS(C2:C6,B2:B6,\"Fruit\")"), "1.25");
    assert_eq!(eval(&mut b, "=MAXIFS(C2:C6,B2:B6,\"Zuivel\")"), "4.2");
    assert_eq!(eval(&mut b, "=MINIFS(C2:C6,B2:B6,\"Zuivel\")"), "1.1");
    assert_eq!(eval(&mut b, "=SUMIFS(D2:D6,B2:B4,\"Fruit\")"), "#VALUE!");
    assert_eq!(eval(&mut b, "=COUNTIF(D:D,\">=12\")"), "3");
    assert_eq!(eval(&mut b, "=SUMPRODUCT(C2:C6,D2:D6)"), "150.9");
    assert_eq!(eval(&mut b, "=COUNTBLANK(A1:B8)"), "4");
    assert_eq!(eval(&mut b, "=MEDIAN(D2:D6)"), "12");
    assert_eq!(eval(&mut b, "=MEDIAN(C2:C5)"), "0.925");

    // Changing data recalculates the conditional sums.
    set(&mut b, "B3", "Zuivel");
    assert_eq!(b.sheet(0).display(c("Z1")), "0.925");
    assert_eq!(eval(&mut b, "=COUNTIF(B2:B6,\"Fruit\")"), "2");
}

#[test]
fn text_functions() {
    let mut b = table();
    assert_eq!(eval(&mut b, "=TEXT(1234.5,\"#,##0.00\")"), "1,234.50");
    assert_eq!(eval(&mut b, "=TEXT(0.256,\"0%\")"), "26%");
    assert_eq!(eval(&mut b, "=TEXT(DATE(2026,10,22),\"dddd d mmmm\")"), "donderdag 22 oktober");
    assert_eq!(eval(&mut b, "=\"Totaal: \"&TEXT(SUM(C2:C6),\"0.00\")"), "Totaal: 9.05");
    assert_eq!(eval(&mut b, "=LEFT(A2,3)&RIGHT(A3,2)&MID(A4,2,2)"), "Apperaa");
    assert_eq!(eval(&mut b, "=LEFT(A2)"), "A");
    assert_eq!(eval(&mut b, "=TRIM(\"  veel   spaties  \")"), "veel spaties");
    assert_eq!(eval(&mut b, "=SUBSTITUTE(\"a-b-c\",\"-\",\"+\")"), "a+b+c");
    assert_eq!(eval(&mut b, "=SUBSTITUTE(\"a-b-c\",\"-\",\"+\",2)"), "a-b+c");
    assert_eq!(eval(&mut b, "=FIND(\"n\",\"Ananas\")"), "2");
    assert_eq!(eval(&mut b, "=FIND(\"N\",\"Ananas\")"), "#VALUE!");
    assert_eq!(eval(&mut b, "=SEARCH(\"N\",\"Ananas\",3)"), "4");
    assert_eq!(eval(&mut b, "=SEARCH(\"a?a\",\"xxanaxx\")"), "3");
    assert_eq!(eval(&mut b, "=REPT(\"ab\",3)"), "ababab");
    assert_eq!(eval(&mut b, "=VALUE(\"€ 1,250.50\")+1"), "1251.5");
    assert_eq!(eval(&mut b, "=VALUE(\"abc\")"), "#VALUE!");
}

#[test]
fn dates_and_now() {
    let mut b = table();
    assert_eq!(eval(&mut b, "=DATE(2026,10,22)"), "46317");
    assert_eq!(eval(&mut b, "=DATE(2026,14,1)"), "46419");
    assert_eq!(eval(&mut b, "=YEAR(46317)&\"-\"&MONTH(46317)&\"-\"&DAY(46317)"), "2026-10-22");
    assert_eq!(eval(&mut b, "=WEEKDAY(DATE(2026,10,22))"), "5");
    assert_eq!(eval(&mut b, "=WEEKDAY(DATE(2026,10,22),2)"), "4");
    let today: f64 = eval(&mut b, "=TODAY()").parse().unwrap();
    assert!(today > 46000.0 && today.fract() == 0.0, "{today}");
    let now: f64 = eval(&mut b, "=NOW()").parse().unwrap();
    assert!(now >= today && now < today + 1.0, "{now} {today}");
    // Volatile: a formula using TODAY is refreshed on any change.
    set(&mut b, "Y1", "=TODAY()-DATE(2026,1,1)");
    set(&mut b, "Y2", "x");
    assert!(b.last_recalc_count() >= 1);
}

#[test]
fn maths_and_logic() {
    let mut b = table();
    assert_eq!(eval(&mut b, "=ROUNDUP(2.121,2)"), "2.13");
    assert_eq!(eval(&mut b, "=ROUNDDOWN(-2.129,2)"), "-2.12");
    assert_eq!(eval(&mut b, "=ROUNDUP(0.1+0.2,1)"), "0.3");
    assert_eq!(eval(&mut b, "=INT(-2.5)"), "-3");
    assert_eq!(eval(&mut b, "=MOD(-7,3)"), "2");
    assert_eq!(eval(&mut b, "=MOD(5,0)"), "#DIV/0!");
    assert_eq!(eval(&mut b, "=POWER(2,10)"), "1024");
    assert_eq!(eval(&mut b, "=SQRT(-1)"), "#NUM!");
    assert_eq!(eval(&mut b, "=IFS(D3>10,\"veel\",D3>0,\"weinig\",TRUE,\"op\")"), "op");
    assert_eq!(eval(&mut b, "=IFS(FALSE,1)"), "#N/A");
    assert_eq!(eval(&mut b, "=IFNA(VLOOKUP(\"x\",A2:D6,2,FALSE),\"onbekend\")"), "onbekend");
    assert_eq!(
        eval(&mut b, "=ISBLANK(F1)&ISNUMBER(C2)&ISTEXT(A2)&ISERROR(1/0)&ISNA(NA())"),
        "TRUETRUETRUETRUETRUE"
    );
}
