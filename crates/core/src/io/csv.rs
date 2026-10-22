//! CSV import and export.

use std::path::Path;

use super::IoError;
use crate::addr::{CellRef, MAX_COLS, MAX_ROWS};
use crate::format::CellFormat;
use crate::input::{Content, Parsed, parse_input, parse_literal};
use crate::sheet::{CellState, Sheet};
use crate::value::Value;
use crate::workbook::Workbook;

/// Reads a CSV file. The delimiter (`,`, `;` or tab) is detected from the
/// first line; Dutch Excel writes `;`. Text that is not valid UTF-8 is read
/// as Windows-1252.
pub fn read_csv(path: &Path, sheet_name: &str) -> Result<Workbook, IoError> {
    let bytes = std::fs::read(path)?;
    let text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => decode_cp1252(e.as_bytes()),
    };
    // A .tsv file is tab-separated even if commas outnumber tabs in its first line.
    let tsv = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("tsv"));
    read_delimited(&text, sheet_name, tsv.then_some(b'\t'))
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

/// Reads CSV text into a one-sheet workbook.
pub fn read_csv_str(text: &str, sheet_name: &str) -> Result<Workbook, IoError> {
    read_delimited(text, sheet_name, None)
}

/// Reads delimited text; `delimiter` `None` detects it from the first line.
pub fn read_delimited(text: &str, sheet_name: &str, delimiter: Option<u8>) -> Result<Workbook, IoError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut reader = ::csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter.unwrap_or_else(|| detect_delimiter(text)))
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
    Ok(Workbook::from_sheets(vec![sheet]))
}

/// Characters that make Excel, Numbers and LibreOffice treat a CSV field as
/// a formula.
/// The full-width forms count too: some Excel locales accept them.
const FORMULA_TRIGGERS: [char; 10] = ['=', '+', '-', '@', '\t', '\r', '＝', '＋', '－', '＠'];

/// The displayed value of a cell as a CSV field. Text that another
/// spreadsheet would execute as a formula gets a leading `'`, the usual
/// defence against CSV formula injection; [`read_csv`] strips it again.
fn csv_field(sheet: &Sheet, at: CellRef) -> String {
    let text = sheet.display(at);
    let is_text = matches!(sheet.value(at), Value::Text(_));
    // Some programs trim leading whitespace before deciding, so look past it.
    let lead = text.trim_start();
    if is_text
        && (text.starts_with(FORMULA_TRIGGERS)
            || lead.starts_with(FORMULA_TRIGGERS)
            || text.starts_with('\''))
    {
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
        let book =
            read_csv_str("\u{feff}naam;bedrag\r\n\"Jansen; B.\";12.5\r\ntotaal;=SUM(B2:B2)*2\r\n", "test")
                .unwrap();
        let sheet = book.sheet(0);
        assert_eq!(sheet.name(), "test");
        assert_eq!(sheet.display(CellRef::new(1, 0)), "Jansen; B.");
        assert_eq!(sheet.value(CellRef::new(1, 1)), &Value::Number(12.5));
        assert_eq!(sheet.value(CellRef::new(2, 1)), &Value::Number(25.0));
    }

    #[test]
    fn round_trips_through_csv() {
        let mut book = Workbook::new();
        let mut set = |at: &str, text: &str| {
            let p = parse_input(text).unwrap();
            let format = CellFormat { num: p.format.unwrap_or_default(), ..CellFormat::default() };
            book.set_states(0, vec![(CellRef::parse(at).unwrap(), CellState { content: p.content, format })]);
        };
        set("A1", "Product");
        set("B1", "Prijs, incl.");
        set("A2", "Thee \"groen\"");
        set("B2", "€ 3.50");
        set("C2", "=B2*2");
        set("B3", "22-10-2026");
        let sheet = book.sheet(0);
        let text = write_csv_string(sheet);
        assert_eq!(text, "Product,\"Prijs, incl.\",\r\n\"Thee \"\"groen\"\"\",€ 3.50,7\r\n,22-10-2026,\r\n");
        let back = read_csv_str(&text, "Blad1").unwrap();
        let back = back.sheet(0);
        for at in ["A1", "B1", "A2", "B2", "C2", "B3"] {
            let at = CellRef::parse(at).unwrap();
            assert_eq!(back.display(at), sheet.display(at), "{at}");
        }
        assert_eq!(back.value(CellRef::parse("B2").unwrap()), &Value::Number(3.5));
    }

    #[test]
    fn escapes_text_that_would_run_as_a_formula() {
        let mut book = Workbook::new();
        let inputs = [
            "'=HYPERLINK(\"http://x\")",
            "'+1",
            "-tekst",
            "@SUM(1)",
            "''quote",
            "=1+1",
            "-5",
            "  =1+1",
            "\tx",
            "\n=1",
            "＝1+1",
        ];
        for (i, text) in inputs.iter().enumerate() {
            let p = parse_input(text).unwrap();
            book.set_states(
                0,
                vec![(CellRef::new(i as u32, 0), CellState { content: p.content, ..Default::default() })],
            );
        }
        let sheet = book.sheet(0);
        let text = write_csv_string(sheet);
        assert_eq!(
            text,
            "\"'=HYPERLINK(\"\"http://x\"\")\"\r\n'+1\r\n'-tekst\r\n'@SUM(1)\r\n''quote\r\n2\r\n-5\r\n'  =1+1\r\n'\tx\r\n\"'\n=1\"\r\n'＝1+1\r\n"
        );
        let back = read_csv_str(&text, "Blad1").unwrap();
        let back = back.sheet(0);
        for i in 0..inputs.len() as u32 {
            let at = CellRef::new(i, 0);
            assert_eq!(back.value(at), sheet.value(at), "row {i}");
        }
    }

    fn grid(book: &Workbook, rows: u32, cols: u32) -> Vec<Vec<String>> {
        (0..rows).map(|r| (0..cols).map(|c| book.sheet(0).display(CellRef::new(r, c))).collect()).collect()
    }

    #[test]
    fn reads_every_common_variant() {
        let expected = vec![
            vec!["Naam".to_string(), "Prijs".to_string(), "Opmerking".to_string()],
            vec!["Thee".to_string(), "2.5".to_string(), "groen, los".to_string()],
            vec!["Kaas".to_string(), "4".to_string(), "".to_string()],
        ];
        for (label, text) in [
            ("comma", "Naam,Prijs,Opmerking\nThee,2.5,\"groen, los\"\nKaas,4,\n"),
            ("semicolon", "Naam;Prijs;Opmerking\r\nThee;2.5;groen, los\r\nKaas;4;\r\n"),
            ("tab", "Naam\tPrijs\tOpmerking\nThee\t2.5\tgroen, los\nKaas\t4\t\n"),
            ("bom", "\u{feff}Naam,Prijs,Opmerking\nThee,2.5,\"groen, los\"\nKaas,4,"),
        ] {
            let book = read_csv_str(text, "t").unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_eq!(grid(&book, 3, 3), expected, "{label}");
        }
        // Ragged rows and a missing final newline are fine.
        let book = read_csv_str("a,b,c\n1\n2,3", "t").unwrap();
        assert_eq!(grid(&book, 3, 3)[2], ["2", "3", ""]);
    }

    #[test]
    fn tsv_files_are_always_tab_separated_and_legacy_files_decode() {
        let dir = std::env::temp_dir().join(format!("rustsheet-csv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tsv = dir.join("lijst.tsv");
        std::fs::write(&tsv, "Naam, voluit\tStad\nJansen, P.\tDelft\n").unwrap();
        let book = crate::io::open(&tsv).unwrap();
        assert_eq!(book.sheet(0).name(), "lijst");
        assert_eq!(grid(&book, 2, 2), [["Naam, voluit", "Stad"], ["Jansen, P.", "Delft"]]);

        let legacy = dir.join("oud.csv");
        std::fs::write(&legacy, b"Plaats;Prijs\nCaf\xe9;\x80 3\n").unwrap();
        let book = crate::io::open(&legacy).unwrap();
        assert_eq!(book.sheet(0).display(CellRef::new(1, 0)), "Café");
        assert_eq!(book.sheet(0).value(CellRef::new(1, 1)), &Value::Number(3.0));
    }

    #[test]
    fn falls_back_to_windows_1252() {
        assert_eq!(decode_cp1252(b"caf\xe9 \x80"), "café €");
    }
}
