//! CSV import and export.

use std::path::Path;

use super::IoError;
use crate::addr::{CellRef, MAX_COLS, MAX_ROWS};
use crate::format::CellFormat;
use crate::input::{Content, Parsed, parse_input, parse_literal};
use crate::sheet::{CellState, Sheet};
use crate::value::Value;

/// Reads a CSV file. The delimiter (`,`, `;` or tab) is detected from the
/// first line; Dutch Excel writes `;`. Text that is not valid UTF-8 is read
/// as Windows-1252.
pub fn read_csv(path: &Path, sheet_name: &str) -> Result<Sheet, IoError> {
    let bytes = std::fs::read(path)?;
    let text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => decode_cp1252(e.as_bytes()),
    };
    read_csv_str(&text, sheet_name)
}

fn decode_cp1252(bytes: &[u8]) -> String {
    // The 0x80..0x9F block differs from Latin-1; map the common ones.
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
        '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    bytes
        .iter()
        .map(|&b| if (0x80..0xA0).contains(&b) { HIGH[(b - 0x80) as usize] } else { char::from(b) })
        .collect()
}

fn detect_delimiter(text: &str) -> u8 {
    let first = text.lines().next().unwrap_or("");
    let count = |c: char| {
        // Ignore delimiters inside quotes.
        let mut quoted = false;
        first
            .chars()
            .filter(|&x| {
                if x == '"' {
                    quoted = !quoted;
                }
                !quoted && x == c
            })
            .count()
    };
    [(b'\t', count('\t')), (b';', count(';')), (b',', count(','))]
        .into_iter()
        .max_by_key(|&(d, n)| (n, d == b','))
        .map_or(b',', |(d, _)| d)
}

pub fn read_csv_str(text: &str, sheet_name: &str) -> Result<Sheet, IoError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut reader = ::csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(detect_delimiter(text))
        .from_reader(text.as_bytes());
    let mut sheet = Sheet::new(sheet_name);
    for (r, record) in reader.records().enumerate() {
        let record = record.map_err(|e| IoError::Format(format!("CSV-fout: {e}")))?;
        if r as u32 >= MAX_ROWS {
            break;
        }
        for (c, field) in record.iter().enumerate().take(MAX_COLS as usize) {
            // Formulas are kept when they parse; anything else stays a value.
            // A leading ' marks escaped text (see `csv_field`).
            let parsed = if let Some(text) = field.strip_prefix('\'') {
                Parsed { content: Content::Value(Value::Text(text.to_string())), format: None }
            } else if field.starts_with('=') {
                parse_input(field).unwrap_or_else(|_| parse_literal(field))
            } else {
                parse_literal(field)
            };
            if parsed.content.is_empty() {
                continue;
            }
            let format = CellFormat { num: parsed.format.unwrap_or_default(), ..CellFormat::default() };
            sheet.put(CellRef::new(r as u32, c as u32), CellState { content: parsed.content, format });
        }
    }
    sheet.recalc_all();
    Ok(sheet)
}

/// Characters that make Excel, Numbers and LibreOffice treat a CSV field as
/// a formula.
const FORMULA_TRIGGERS: [char; 4] = ['=', '+', '-', '@'];

/// The displayed value of a cell as a CSV field. Text that another
/// spreadsheet would execute as a formula gets a leading `'`, the usual
/// defence against CSV formula injection; [`read_csv`] strips it again.
fn csv_field(sheet: &Sheet, at: CellRef) -> String {
    let text = sheet.display(at);
    let is_text = matches!(sheet.value(at), Value::Text(_));
    if is_text && (text.starts_with(FORMULA_TRIGGERS) || text.starts_with('\'')) {
        format!("'{text}")
    } else {
        text
    }
}

/// CSV text of the used range, with displayed (formatted) values like Excel.
pub fn write_csv_string(sheet: &Sheet) -> String {
    let mut writer = ::csv::WriterBuilder::new().terminator(::csv::Terminator::CRLF).from_writer(Vec::new());
    if let Some(used) = sheet.used_range() {
        // Rows start at row 1 and column A so positions survive a round trip.
        for row in 0..=used.end.row {
            let fields: Vec<String> =
                (0..=used.end.col).map(|col| csv_field(sheet, CellRef::new(row, col))).collect();
            writer.write_record(&fields).expect("writing to memory");
        }
    }
    String::from_utf8(writer.into_inner().expect("flush to memory")).expect("utf-8")
}

pub fn write_csv(sheet: &Sheet, path: &Path) -> Result<(), IoError> {
    super::xlsx::write_atomic(path, write_csv_string(sheet).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_semicolons_quotes_and_formulas() {
        let sheet =
            read_csv_str("\u{feff}naam;bedrag\r\n\"Jansen; B.\";12.5\r\ntotaal;=SUM(B2:B2)*2\r\n", "test")
                .unwrap();
        assert_eq!(sheet.display(CellRef::new(1, 0)), "Jansen; B.");
        assert_eq!(sheet.value(CellRef::new(1, 1)), &Value::Number(12.5));
        assert_eq!(sheet.value(CellRef::new(2, 1)), &Value::Number(25.0));
    }

    #[test]
    fn round_trips_through_csv() {
        let mut sheet = Sheet::new("Blad1");
        let set = |sheet: &mut Sheet, at: &str, text: &str| {
            let p = parse_input(text).unwrap();
            let format = CellFormat { num: p.format.unwrap_or_default(), ..CellFormat::default() };
            sheet.set_states(vec![(CellRef::parse(at).unwrap(), CellState { content: p.content, format })]);
        };
        set(&mut sheet, "A1", "Product");
        set(&mut sheet, "B1", "Prijs, incl.");
        set(&mut sheet, "A2", "Thee \"groen\"");
        set(&mut sheet, "B2", "€ 3.50");
        set(&mut sheet, "C2", "=B2*2");
        set(&mut sheet, "B3", "22-10-2026");
        let text = write_csv_string(&sheet);
        assert_eq!(text, "Product,\"Prijs, incl.\",\r\n\"Thee \"\"groen\"\"\",€ 3.50,7\r\n,22-10-2026,\r\n");
        let back = read_csv_str(&text, "Blad1").unwrap();
        for at in ["A1", "B1", "A2", "B2", "C2", "B3"] {
            let at = CellRef::parse(at).unwrap();
            assert_eq!(back.display(at), sheet.display(at), "{at}");
        }
        assert_eq!(back.value(CellRef::parse("B2").unwrap()), &Value::Number(3.5));
    }

    #[test]
    fn escapes_text_that_would_run_as_a_formula() {
        let mut sheet = Sheet::new("Blad1");
        let inputs = ["'=HYPERLINK(\"http://x\")", "'+1", "-tekst", "@SUM(1)", "''quote", "=1+1", "-5"];
        for (i, text) in inputs.iter().enumerate() {
            let p = parse_input(text).unwrap();
            sheet.set_states(vec![(
                CellRef::new(i as u32, 0),
                CellState { content: p.content, ..Default::default() },
            )]);
        }
        let text = write_csv_string(&sheet);
        assert_eq!(
            text,
            "\"'=HYPERLINK(\"\"http://x\"\")\"\r\n'+1\r\n'-tekst\r\n'@SUM(1)\r\n''quote\r\n2\r\n-5\r\n"
        );
        let back = read_csv_str(&text, "Blad1").unwrap();
        for i in 0..inputs.len() as u32 {
            let at = CellRef::new(i, 0);
            assert_eq!(back.value(at), sheet.value(at), "row {i}");
        }
    }

    #[test]
    fn falls_back_to_windows_1252() {
        assert_eq!(decode_cp1252(b"caf\xe9 \x80"), "café €");
    }
}
