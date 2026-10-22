//! Writes a small workbook that shows the basics: formats, formulas,
//! errors and overflowing text. Used for screenshots and manual testing.
//!
//! cargo run -p rustsheet-core --example demo -- demo.xlsx

use rustsheet_core::{Align, CellFormat, CellRef, CellState, NumFmt, Sheet, Workbook, io, parse_input};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "demo.xlsx".into());
    let mut sheet = Sheet::new("Begroting");
    let header = CellFormat { bold: true, ..CellFormat::default() };
    let rows: &[&[(&str, CellFormat)]] = &[
        &[("Huishoudbegroting oktober 2026 (alle bedragen incl. btw)", header)],
        &[],
        &[
            ("Post", header),
            ("Datum", header),
            ("Bedrag", CellFormat { align: Align::Right, ..header }),
            ("Aandeel", CellFormat { align: Align::Right, ..header }),
            ("Opmerking", header),
        ],
        &[
            ("Huur", CellFormat::default()),
            ("01-10-2026", CellFormat::default()),
            ("€ 1,250", CellFormat::default()),
            ("=C4/$C$10", CellFormat { num: NumFmt::Percent, ..Default::default() }),
            ("vast", CellFormat::default()),
        ],
        &[
            ("Energie", CellFormat::default()),
            ("03-10-2026", CellFormat::default()),
            ("€ 187.40", CellFormat::default()),
            ("=C5/$C$10", CellFormat { num: NumFmt::Percent, ..Default::default() }),
            ("=IF(C5>150,\"hoog\",\"ok\")", CellFormat::default()),
        ],
        &[
            ("Boodschappen", CellFormat::default()),
            ("12-10-2026", CellFormat::default()),
            ("€ 412.95", CellFormat::default()),
            ("=C6/$C$10", CellFormat { num: NumFmt::Percent, ..Default::default() }),
            ("", CellFormat::default()),
        ],
        &[
            ("Verzekering", CellFormat::default()),
            ("15-10-2026", CellFormat::default()),
            ("€ 96.10", CellFormat::default()),
            ("=C7/$C$10", CellFormat { num: NumFmt::Percent, ..Default::default() }),
            ("", CellFormat::default()),
        ],
        &[
            ("Abonnementen", CellFormat::default()),
            ("20-10-2026", CellFormat::default()),
            ("€ 42.97", CellFormat::default()),
            ("=C8/$C$10", CellFormat { num: NumFmt::Percent, ..Default::default() }),
            ("=CONCAT(\"aantal: \",COUNT(C4:C8))", CellFormat::default()),
        ],
        &[],
        &[
            ("Totaal", header),
            ("", CellFormat::default()),
            ("=SUM(C4:C8)", CellFormat { num: NumFmt::Currency, ..header }),
            ("=SUM(D4:D8)", CellFormat { num: NumFmt::Percent, ..header }),
            ("", CellFormat::default()),
        ],
        &[
            ("Gemiddeld", CellFormat::default()),
            ("", CellFormat::default()),
            ("=ROUND(AVERAGE(C4:C8),2)", CellFormat { num: NumFmt::Currency, ..Default::default() }),
        ],
        &[
            ("Hoogste", CellFormat::default()),
            ("", CellFormat::default()),
            ("=MAX(C4:C8)", CellFormat { num: NumFmt::Number, ..Default::default() }),
        ],
        &[
            ("Fout", CellFormat::default()),
            ("", CellFormat::default()),
            ("=C12/0", CellFormat::default()),
            ("=onbekend(1)", CellFormat::default()),
            ("=E13+1", CellFormat::default()),
        ],
    ];
    for (r, row) in rows.iter().enumerate() {
        for (c, (text, format)) in row.iter().enumerate() {
            if text.is_empty() {
                continue;
            }
            let parsed = parse_input(text).expect("valid");
            let num =
                if format.num == NumFmt::General { parsed.format.unwrap_or_default() } else { format.num };
            sheet.put(
                CellRef::new(r as u32, c as u32),
                CellState { content: parsed.content, format: CellFormat { num, ..*format } },
            );
        }
    }
    sheet.set_col_width(0, Some(120.0));
    sheet.set_col_width(4, Some(150.0));

    // A second sheet that reads totals from the first.
    let mut notes = Sheet::new("Toelichting");
    let lines = [
        ("Totaal uitgaven", "=Begroting!C10"),
        ("Grootste post", "=MAX(Begroting!C4:C8)"),
        ("Huur als aandeel", "=Begroting!D4"),
        ("Budget", "€ 2,250"),
        ("Over", "=B4-B1"),
    ];
    for (r, (label, value)) in lines.iter().enumerate() {
        notes.put(
            CellRef::new(r as u32, 0),
            CellState { content: parse_input(label).unwrap().content, ..Default::default() },
        );
        let parsed = parse_input(value).expect("valid");
        let num = parsed.format.unwrap_or(if r == 2 { NumFmt::Percent } else { NumFmt::Currency });
        notes.put(
            CellRef::new(r as u32, 1),
            CellState { content: parsed.content, format: CellFormat { num, ..Default::default() } },
        );
    }
    notes.set_col_width(0, Some(140.0));
    notes.set_col_width(1, Some(110.0));

    let book = Workbook::from_sheets(vec![sheet, notes, Sheet::new("Blad3")]);
    io::save(&book, std::path::Path::new(&out), 0).expect("save");
    println!("{out}");
}
