//! Minimal reader for the xlsx parts calamine skips: cell styles (bold,
//! horizontal alignment, number format) and column widths of a sheet.

use std::collections::HashMap;
use std::io::{BufReader, Read, Seek};

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use zip::ZipArchive;

use crate::addr::CellRef;
use crate::format::{Align, CellFormat, NumFmt};

/// Styling found for one worksheet.
#[derive(Debug, Default)]
pub struct SheetStyles {
    pub cells: Vec<(CellRef, CellFormat)>,
    /// Column widths in Excel character units.
    pub col_widths: Vec<(u32, f64)>,
}

fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == name)
        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok().map(|v| v.into_owned()))
}

fn read_part<'z, R: Read + Seek>(
    zip: &'z mut ZipArchive<R>,
    path: &str,
) -> Option<Reader<BufReader<impl Read + 'z>>> {
    let file = zip.by_name(path).ok()?;
    Some(Reader::from_reader(BufReader::new(file)))
}

/// Walks every start or empty element of a part.
fn each_element<R: std::io::BufRead>(mut reader: Reader<R>, mut f: impl FnMut(&BytesStart<'_>, bool)) {
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => f(&e, false),
            Ok(Event::Empty(e)) => f(&e, true),
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

/// Number format for one of Excel's built-in format ids.
fn builtin_num_fmt(id: u32) -> NumFmt {
    match id {
        0 | 49 => NumFmt::General,
        1..=4 | 37..=40 | 48 => NumFmt::Number,
        5..=8 | 41..=44 => NumFmt::Currency,
        9 | 10 => NumFmt::Percent,
        14..=22 | 27..=36 | 45..=47 | 50..=58 => NumFmt::Date,
        _ => NumFmt::General,
    }
}

/// The path inside the archive of the worksheet with index `index` in workbook order.
fn sheet_path<R: Read + Seek>(zip: &mut ZipArchive<R>, index: usize) -> Option<String> {
    let mut rel_ids = Vec::new();
    each_element(read_part(zip, "xl/workbook.xml")?, |e, _| {
        if e.local_name().as_ref() == "sheet"
            && let Some(id) = attr(e, "id")
        {
            rel_ids.push(id);
        }
    });
    let wanted = rel_ids.get(index)?.clone();
    let mut target = None;
    each_element(read_part(zip, "xl/_rels/workbook.xml.rels")?, |e, _| {
        if e.local_name().as_ref() == "Relationship" && attr(e, "Id").as_deref() == Some(&wanted) {
            target = attr(e, "Target");
        }
    });
    let target = target?;
    Some(match target.strip_prefix('/') {
        Some(abs) => abs.to_string(),
        None => format!("xl/{target}"),
    })
}

/// Cell formats indexed by the `s` attribute of cells.
fn cell_xfs<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Vec<CellFormat> {
    let Some(reader) = read_part(zip, "xl/styles.xml") else { return Vec::new() };
    let mut custom: HashMap<u32, NumFmt> = HashMap::new();
    let mut bold_fonts: Vec<bool> = Vec::new();
    let mut xfs: Vec<CellFormat> = Vec::new();
    let mut section = String::new();
    let mut in_font = false;
    // Track sections by hand: fonts and xfs appear under several parents.
    let mut buf = Vec::new();
    let mut reader = reader;
    loop {
        let event = reader.read_event_into(&mut buf);
        match event {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let empty = matches!(event, Ok(Event::Empty(_)));
                let name = e.local_name().as_ref().to_string();
                match name.as_str() {
                    "numFmts" | "fonts" | "cellXfs" | "cellStyleXfs" if !empty => section = name.clone(),
                    "numFmt" => {
                        if let (Some(id), Some(code)) = (attr(e, "numFmtId"), attr(e, "formatCode"))
                            && let Ok(id) = id.parse()
                        {
                            custom.insert(id, NumFmt::from_excel_code(&code));
                        }
                    }
                    "font" if section == "fonts" => {
                        bold_fonts.push(false);
                        in_font = !empty;
                    }
                    "b" if in_font => {
                        let on = attr(e, "val").is_none_or(|v| v != "0" && v != "false");
                        if let Some(last) = bold_fonts.last_mut() {
                            *last = on;
                        }
                    }
                    "xf" if section == "cellXfs" => {
                        let num_id: u32 = attr(e, "numFmtId").and_then(|v| v.parse().ok()).unwrap_or(0);
                        let font_id: usize = attr(e, "fontId").and_then(|v| v.parse().ok()).unwrap_or(0);
                        xfs.push(CellFormat {
                            num: custom.get(&num_id).copied().unwrap_or_else(|| builtin_num_fmt(num_id)),
                            bold: bold_fonts.get(font_id).copied().unwrap_or(false),
                            align: Align::General,
                        });
                    }
                    "alignment" if section == "cellXfs" => {
                        if let Some(last) = xfs.last_mut() {
                            last.align = match attr(e, "horizontal").as_deref() {
                                Some("left") => Align::Left,
                                Some("center") | Some("centerContinuous") => Align::Center,
                                Some("right") => Align::Right,
                                _ => Align::General,
                            };
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = e.local_name();
                if name.as_ref() == "font" {
                    in_font = false;
                } else if name.as_ref() == section {
                    section.clear();
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    xfs
}

/// Reads cell styles and column widths of the worksheet at `index`.
/// Returns empty styles for files that are not xlsx or lack these parts.
pub fn read_sheet_styles<R: Read + Seek>(reader: R, index: usize) -> SheetStyles {
    let Ok(mut zip) = ZipArchive::new(reader) else { return SheetStyles::default() };
    let xfs = cell_xfs(&mut zip);
    let Some(path) = sheet_path(&mut zip, index) else { return SheetStyles::default() };
    let Some(part) = read_part(&mut zip, &path) else { return SheetStyles::default() };
    let mut styles = SheetStyles::default();
    each_element(part, |e, _| match e.local_name().as_ref() {
        "c" => {
            let Some(style) = attr(e, "s").and_then(|s| s.parse::<usize>().ok()) else { return };
            let Some(format) = xfs.get(style).copied().filter(|f| !f.is_default()) else { return };
            if let Some(at) = attr(e, "r").and_then(|r| CellRef::parse(&r)) {
                styles.cells.push((at, format));
            }
        }
        "col" => {
            let custom = attr(e, "customWidth").is_some_and(|v| v == "1" || v == "true");
            let (Some(min), Some(max), Some(width)) = (
                attr(e, "min").and_then(|v| v.parse::<u32>().ok()),
                attr(e, "max").and_then(|v| v.parse::<u32>().ok()),
                attr(e, "width").and_then(|v| v.parse::<f64>().ok()),
            ) else {
                return;
            };
            // Ranges spanning the whole sheet are default widths in disguise.
            if custom && min >= 1 && max >= min && max - min < 1024 {
                styles.col_widths.extend((min - 1..max).map(|c| (c, width)));
            }
        }
        _ => {}
    });
    styles
}
