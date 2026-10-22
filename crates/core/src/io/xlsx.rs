//! xlsx: reading with calamine (plus our own style reader), writing with
//! rust_xlsxwriter.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use calamine::{Data, Reader, open_workbook_auto};
use rust_xlsxwriter::{Format, FormatAlign, Formula, Workbook};

use super::IoError;
use super::styles::read_sheet_styles;
use crate::addr::{CellRef, MAX_COLS, MAX_ROWS};
use crate::format::{Align, CellFormat, NumFmt};
use crate::formula::parse_formula;
use crate::input::Content;
use crate::sheet::{CellState, DEFAULT_COL_WIDTH, Sheet};
use crate::value::{ErrorKind, Value};

/// Excel's default column is 64 pixels wide; ours is [`DEFAULT_COL_WIDTH`].
/// Widths scale by this factor so a default column stays default.
const WIDTH_SCALE: f64 = DEFAULT_COL_WIDTH as f64 / 64.0;

fn data_to_value(data: &Data) -> Option<(Value, Option<NumFmt>)> {
    Some(match data {
        Data::Empty => return None,
        Data::Int(i) => (Value::Number(*i as f64), None),
        Data::Float(f) => (Value::number(*f), None),
        Data::String(s) => (Value::Text(s.clone()), None),
        Data::Bool(b) => (Value::Bool(*b), None),
        Data::DateTime(dt) => (Value::Number(dt.as_f64()), Some(NumFmt::Date)),
        Data::DateTimeIso(s) | Data::DurationIso(s) => (Value::Text(s.clone()), None),
        Data::Error(e) => (
            Value::Error(match e {
                calamine::CellErrorType::Div0 => ErrorKind::Div0,
                calamine::CellErrorType::NA => ErrorKind::NA,
                calamine::CellErrorType::Name => ErrorKind::Name,
                calamine::CellErrorType::Null => ErrorKind::Null,
                calamine::CellErrorType::Num => ErrorKind::Num,
                calamine::CellErrorType::Ref => ErrorKind::Ref,
                _ => ErrorKind::Value,
            }),
            None,
        ),
    })
}

fn format_err(e: impl std::fmt::Display) -> IoError {
    IoError::Format(format!("kan werkmap niet lezen: {e}"))
}

/// Reads the first worksheet of a workbook. Formulas are re-evaluated by
/// our engine; a formula we cannot parse keeps its cached value.
pub fn read_workbook(path: &Path) -> Result<Sheet, IoError> {
    let mut workbook = open_workbook_auto(path).map_err(format_err)?;
    let name = workbook
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| IoError::Format("de werkmap bevat geen werkbladen".into()))?;
    let values = workbook.worksheet_range(&name).map_err(format_err)?;
    let formulas = workbook.worksheet_formula(&name).map_err(format_err)?;
    let styles = File::open(path).map(|f| read_sheet_styles(f, 0)).unwrap_or_default();

    let mut sheet = Sheet::new(name);
    let mut formats: HashMap<CellRef, CellFormat> = styles.cells.into_iter().collect();
    let in_grid = |r: u32, c: u32| r < MAX_ROWS && c < MAX_COLS;

    if let Some((r0, c0)) = values.start() {
        for (r, c, data) in values.used_cells() {
            let (r, c) = (r0 + r as u32, c0 + c as u32);
            let Some((value, implied)) = data_to_value(data) else { continue };
            if !in_grid(r, c) {
                continue;
            }
            let at = CellRef::new(r, c);
            let mut format = formats.remove(&at).unwrap_or_default();
            if format.num == NumFmt::General
                && let Some(num) = implied
            {
                format.num = num;
            }
            sheet.put(at, CellState { content: Content::Value(value), format });
        }
    }
    if let Some((r0, c0)) = formulas.start() {
        for (r, c, text) in formulas.used_cells() {
            let (r, c) = (r0 + r as u32, c0 + c as u32);
            if text.is_empty() || !in_grid(r, c) {
                continue;
            }
            let at = CellRef::new(r, c);
            if let Ok(expr) = parse_formula(text) {
                let format = sheet.format(at);
                let format = formats.remove(&at).unwrap_or(format);
                sheet.put(at, CellState { content: Content::Formula(expr), format });
            }
        }
    }
    // Styled cells without a value.
    for (at, format) in formats {
        if sheet.cell(at).is_none() && in_grid(at.row, at.col) {
            sheet.put(at, CellState { content: Content::Empty, format });
        }
    }
    for (col, chars) in styles.col_widths {
        if col < MAX_COLS {
            // The stored width already includes Excel's padding, so at the
            // default font one unit is seven pixels.
            let px = (chars * 7.0).round().max(0.0);
            sheet.set_col_width(col, Some((px * WIDTH_SCALE) as f32));
        }
    }
    sheet.recalc_all();
    Ok(sheet)
}

fn xlsx_format(f: &CellFormat) -> Format {
    let mut format = Format::new();
    if f.num != NumFmt::General {
        format = format.set_num_format(f.num.excel_code());
    }
    if f.bold {
        format = format.set_bold();
    }
    match f.align {
        Align::General => {}
        Align::Left => format = format.set_align(FormatAlign::Left),
        Align::Center => format = format.set_align(FormatAlign::Center),
        Align::Right => format = format.set_align(FormatAlign::Right),
    }
    format
}

/// Excel rejects sheet names that are empty, longer than 31 characters or
/// contain `[]:*?/\`.
fn valid_sheet_name(name: &str) -> Option<&str> {
    let ok = !name.is_empty()
        && name.chars().count() <= 31
        && !name.contains(['[', ']', ':', '*', '?', '/', '\\'])
        && !name.starts_with('\'')
        && !name.ends_with('\'');
    ok.then_some(name)
}

fn xlsx_err(e: rust_xlsxwriter::XlsxError) -> IoError {
    IoError::Format(format!("kan xlsx niet schrijven: {e}"))
}

pub fn write_xlsx_bytes(sheet: &Sheet) -> Result<Vec<u8>, IoError> {
    let mut workbook = Workbook::new();
    let ws = workbook.add_worksheet();
    ws.set_name(valid_sheet_name(sheet.name()).unwrap_or("Blad1")).map_err(xlsx_err)?;
    let mut formats: HashMap<CellFormat, Format> = HashMap::new();
    for (at, cell) in sheet.cells() {
        let format = formats.entry(cell.format).or_insert_with(|| xlsx_format(&cell.format));
        let (row, col) = (at.row, at.col as u16);
        match &cell.content {
            Content::Empty => {
                ws.write_blank(row, col, format).map_err(xlsx_err)?;
            }
            Content::Value(Value::Number(n)) => {
                ws.write_number_with_format(row, col, *n, format).map_err(xlsx_err)?;
            }
            Content::Value(Value::Text(s)) => {
                ws.write_string_with_format(row, col, s, format).map_err(xlsx_err)?;
            }
            Content::Value(Value::Bool(b)) => {
                ws.write_boolean_with_format(row, col, *b, format).map_err(xlsx_err)?;
            }
            Content::Value(Value::Error(e)) => {
                let formula = Formula::new(format!("={e}")).set_result(e.as_str());
                ws.write_formula_with_format(row, col, formula, format).map_err(xlsx_err)?;
            }
            Content::Value(Value::Empty) => {}
            Content::Formula(expr) => {
                let result = match &cell.value {
                    Value::Bool(true) => "TRUE".to_string(),
                    Value::Bool(false) => "FALSE".to_string(),
                    // Excel has no circular-reference error value.
                    Value::Error(ErrorKind::Circ) => "0".to_string(),
                    v => v.to_string(),
                };
                let formula = Formula::new(format!("={expr}")).set_result(result);
                ws.write_formula_with_format(row, col, formula, format).map_err(xlsx_err)?;
            }
        }
    }
    for (col, width) in sheet.col_widths() {
        let px = (f64::from(width) / WIDTH_SCALE).round().max(0.0) as u32;
        ws.set_column_width_pixels(col as u16, px).map_err(xlsx_err)?;
    }
    workbook.save_to_buffer().map_err(xlsx_err)
}

pub fn write_xlsx(sheet: &Sheet, path: &Path) -> Result<(), IoError> {
    write_atomic(path, &write_xlsx_bytes(sheet)?)
}

/// Writes to a temporary file next to `path` and renames it into place, so
/// a failed save never leaves a half-written file behind.
pub(super) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), IoError> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("bestand");
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(IoError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::parse_input;

    fn c(s: &str) -> CellRef {
        CellRef::parse(s).unwrap()
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rustsheet-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn xlsx_round_trip_keeps_values_formulas_and_formats() {
        let mut sheet = Sheet::new("Begroting");
        let mut set = |at: &str, text: &str, format: CellFormat| {
            let p = parse_input(text).unwrap();
            let format = CellFormat { num: p.format.unwrap_or(format.num), ..format };
            sheet.set_states(vec![(c(at), CellState { content: p.content, format })]);
        };
        let bold = CellFormat { bold: true, ..CellFormat::default() };
        set("A1", "Post", bold);
        set("B1", "Bedrag", CellFormat { align: Align::Center, ..bold });
        set("A2", "Huur", CellFormat::default());
        set("B2", "€ 950", CellFormat::default());
        set("A3", "Energie", CellFormat::default());
        set("B3", "125.5", CellFormat { num: NumFmt::Currency, ..CellFormat::default() });
        set("B4", "=SUM(B2:B3)", CellFormat { num: NumFmt::Currency, align: Align::Right, ..bold });
        set("C4", "=CONCAT(\"Totaal \",ROUND(B4,0))", CellFormat::default());
        set("D2", "22-10-2026", CellFormat::default());
        set("D3", "12.5%", CellFormat::default());
        set("E2", "TRUE", CellFormat::default());
        set("E3", "=1/0", CellFormat::default());
        set("F9", "", CellFormat { bold: true, ..CellFormat::default() });
        sheet.set_col_width(0, Some(150.0));

        let path = temp_path("roundtrip.xlsx");
        write_xlsx(&sheet, &path).unwrap();
        let back = read_workbook(&path).unwrap();

        assert_eq!(back.name(), "Begroting");
        for at in ["A1", "B1", "A2", "B2", "B3", "B4", "C4", "D2", "D3", "E2", "E3"] {
            assert_eq!(back.display(c(at)), sheet.display(c(at)), "display {at}");
            assert_eq!(back.format(c(at)), sheet.format(c(at)), "format {at}");
            assert_eq!(back.state(c(at)).content, sheet.state(c(at)).content, "content {at}");
        }
        assert_eq!(back.display(c("B4")), "€ 1,075.50");
        assert_eq!(back.display(c("C4")), "Totaal 1076");
        assert_eq!(back.format(c("F9")), sheet.format(c("F9")));
        let width = back.col_width(0);
        assert!((width - 150.0).abs() < 2.0, "width {width}");
        assert_eq!(back.custom_col_width(1), None);
    }

    #[test]
    fn written_file_uses_excel_function_prefixes() {
        let mut sheet = Sheet::new("Blad1");
        sheet.set_states(vec![(
            c("A1"),
            CellState { content: parse_input("=CONCAT(\"a\",\"b\")").unwrap().content, ..Default::default() },
        )]);
        let bytes = write_xlsx_bytes(&sheet).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("xl/worksheets/sheet1.xml").unwrap(), &mut xml)
            .unwrap();
        assert!(xml.contains("_xlfn.CONCAT"), "{xml}");
    }

    #[test]
    fn invalid_sheet_names_fall_back() {
        assert_eq!(valid_sheet_name("ok"), Some("ok"));
        assert_eq!(valid_sheet_name("a/b"), None);
        assert_eq!(valid_sheet_name(&"x".repeat(32)), None);
    }
}
