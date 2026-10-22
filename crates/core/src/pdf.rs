//! Exporting a sheet (or part of it) to PDF, for sharing and printing.
//!
//! The PDF uses the standard Helvetica fonts, which every viewer has, so
//! nothing is embedded. Text is measured with Helvetica's metrics so
//! alignment matches what the viewer draws.

use std::fmt::Write as _;

use crate::addr::{CellRef, Range};
use crate::format::{Align, format_value};
use crate::sheet::Sheet;
use crate::value::Value;

/// A4 in points.
const PAGE_W: f64 = 595.0;
const PAGE_H: f64 = 842.0;
const MARGIN: f64 = 36.0;
/// Our layout units (screen points) to PDF points; Excel prints a 64 px
/// column as 48 pt, so a default column lands at the same size.
const SCALE: f64 = 0.75;
const FONT_SIZE: f64 = 9.0;
const PAD: f64 = 3.0;
const HEADER_SPACE: f64 = 22.0;

/// Helvetica advance widths (per 1000 em) for ASCII 32..=126.
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556,
    556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667, 611, 778, 722, 278,
    500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469,
    556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500,
    278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];
/// Helvetica-Bold advance widths for ASCII 32..=126.
const HELVETICA_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556,
    556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667, 611, 778, 722, 278,
    556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584,
    556, 333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611, 611, 611, 389, 556,
    333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

/// Width of `text` in points at `size`.
fn text_width(text: &str, bold: bool, size: f64) -> f64 {
    let table = if bold { &HELVETICA_BOLD } else { &HELVETICA };
    let units: u32 = text
        .chars()
        .map(|c| match c as u32 {
            32..=126 => u32::from(table[c as usize - 32]),
            _ => 556,
        })
        .sum();
    f64::from(units) * size / 1000.0
}

/// Text as a PDF string in WinAnsi encoding: Latin-1 as is, the euro and
/// typographic quotes mapped, anything else as `?`.
fn pdf_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('(');
    for c in text.chars() {
        let byte: u32 = match c {
            '€' => 0x80,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '–' => 0x96,
            '—' => 0x97,
            '…' => 0x85,
            c if (c as u32) < 0x80 || (0xA0..=0xFF).contains(&(c as u32)) => c as u32,
            _ => u32::from(b'?'),
        };
        match byte {
            0x28 | 0x29 | 0x5C => {
                out.push('\\');
                out.push(char::from_u32(byte).expect("ascii"));
            }
            0x20..=0x7E => out.push(char::from_u32(byte).expect("ascii")),
            _ => {
                let _ = write!(out, "\\{byte:03o}");
            }
        }
    }
    out.push(')');
    out
}

fn rgb(c: crate::format::Rgb) -> (f64, f64, f64) {
    (f64::from(c.0) / 255.0, f64::from(c.1) / 255.0, f64::from(c.2) / 255.0)
}

/// Splits `lines` (with sizes) into consecutive groups that fit `space`.
fn paginate(lines: &[(u32, f64)], space: f64) -> Vec<Vec<(u32, f64)>> {
    let mut pages = Vec::new();
    let mut page = Vec::new();
    let mut used = 0.0;
    for &(i, size) in lines {
        if !page.is_empty() && used + size > space {
            pages.push(std::mem::take(&mut page));
            used = 0.0;
        }
        page.push((i, size));
        used += size;
    }
    if !page.is_empty() {
        pages.push(page);
    }
    pages
}

/// The PDF for `range` of `sheet` (the used range when `None`). Rows hidden
/// by a filter are left out; frozen rows repeat at the top of every page.
pub fn sheet_to_pdf(sheet: &Sheet, range: Option<Range>) -> Vec<u8> {
    let range = range.or_else(|| sheet.used_range()).unwrap_or(Range::single(CellRef::default()));
    let cols: Vec<(u32, f64)> =
        (range.start.col..=range.end.col).map(|c| (c, f64::from(sheet.col_width(c)) * SCALE)).collect();
    let rows: Vec<(u32, f64)> = (range.start.row..=range.end.row)
        .filter(|&r| !sheet.is_row_hidden(r))
        .map(|r| (r, f64::from(sheet.row_height(r)) * SCALE))
        .collect();
    let titles: Vec<(u32, f64)> = rows.iter().copied().filter(|(r, _)| *r < sheet.freeze().0).collect();
    let body: Vec<(u32, f64)> = rows.iter().copied().filter(|(r, _)| *r >= sheet.freeze().0).collect();

    // Landscape when the table is wider than a portrait page but fits a landscape one.
    let total_w: f64 = cols.iter().map(|c| c.1).sum();
    let landscape = total_w > PAGE_W - 2.0 * MARGIN && total_w <= PAGE_H - 2.0 * MARGIN;
    let (pw, ph) = if landscape { (PAGE_H, PAGE_W) } else { (PAGE_W, PAGE_H) };
    let title_h: f64 = titles.iter().map(|t| t.1).sum();
    let col_pages = paginate(&cols, pw - 2.0 * MARGIN);
    let row_pages = paginate(&body, (ph - 2.0 * MARGIN - HEADER_SPACE - title_h).max(50.0));
    let row_pages = if row_pages.is_empty() { vec![Vec::new()] } else { row_pages };

    // Pages go down first, then across, like Excel.
    let mut streams = Vec::new();
    for page_cols in &col_pages {
        for page_rows in &row_pages {
            let lines: Vec<(u32, f64)> = titles.iter().chain(page_rows.iter()).copied().collect();
            streams.push(page_content(sheet, page_cols, &lines, pw, ph));
        }
    }
    let count = streams.len();
    let streams: Vec<String> = streams
        .into_iter()
        .enumerate()
        .map(|(i, mut s)| {
            let footer = format!("Pagina {} van {count}", i + 1);
            let x = (pw - text_width(&footer, false, 8.0)) / 2.0;
            let _ =
                write!(s, "BT /F1 8 Tf 0.4 g {x:.2} {:.2} Td {} Tj ET\n", MARGIN / 2.0, pdf_string(&footer));
            let _ = write!(
                s,
                "BT /F2 9 Tf 0.2 g {MARGIN:.2} {:.2} Td {} Tj ET\n",
                ph - MARGIN / 2.0 - 6.0,
                pdf_string(sheet.name())
            );
            s
        })
        .collect();
    assemble(&streams, pw, ph)
}

fn page_content(sheet: &Sheet, cols: &[(u32, f64)], rows: &[(u32, f64)], _pw: f64, ph: f64) -> String {
    let mut s = String::new();
    let left = MARGIN;
    let top = ph - MARGIN - HEADER_SPACE;
    let mut xs = Vec::with_capacity(cols.len());
    let mut x = left;
    for &(_, w) in cols {
        xs.push(x);
        x += w;
    }
    let right = x;
    let mut y = top;
    let mut ys = Vec::with_capacity(rows.len());
    for &(_, h) in rows {
        ys.push(y);
        y -= h;
    }
    let bottom = y;

    // Fills.
    for (ri, &(row, h)) in rows.iter().enumerate() {
        for (ci, &(col, w)) in cols.iter().enumerate() {
            if let Some(fill) = sheet.cell(CellRef::new(row, col)).and_then(|c| c.format.fill) {
                let (r, g, b) = rgb(fill);
                let _ =
                    write!(s, "{r:.3} {g:.3} {b:.3} rg {:.2} {:.2} {w:.2} {h:.2} re f\n", xs[ci], ys[ri] - h);
            }
        }
    }

    // Gridlines.
    let _ = write!(s, "0.82 G 0.4 w\n");
    for &yy in ys.iter().chain(std::iter::once(&bottom)) {
        let _ = write!(s, "{left:.2} {yy:.2} m {right:.2} {yy:.2} l S\n");
    }
    for &xx in xs.iter().chain(std::iter::once(&right)) {
        let _ = write!(s, "{xx:.2} {top:.2} m {xx:.2} {bottom:.2} l S\n");
    }

    // Text.
    let last = cols.len();
    for (ri, &(row, h)) in rows.iter().enumerate() {
        for (ci, &(col, w)) in cols.iter().enumerate() {
            let Some(cell) = sheet.cell(CellRef::new(row, col)) else { continue };
            if cell.value.is_empty() {
                continue;
            }
            let f = cell.format;
            let mut text = format_value(&cell.value, f.num);
            let tw = text_width(&text, f.bold, FONT_SIZE);
            if matches!(cell.value, Value::Number(_)) && tw > w - 2.0 * PAD {
                let n = ((w - 2.0 * PAD) / text_width("#", f.bold, FONT_SIZE)).max(1.0) as usize;
                text = "#".repeat(n);
            }
            let tw = text_width(&text, f.bold, FONT_SIZE);
            let align = f.effective_align(&cell.value);
            let tx = match align {
                Align::Right => xs[ci] + w - PAD - tw,
                Align::Center => xs[ci] + (w - tw) / 2.0,
                _ => xs[ci] + PAD,
            };
            // Left-aligned text may run on into empty cells to the right.
            let mut clip_w = w;
            if matches!(align, Align::Left | Align::General) {
                let mut j = ci + 1;
                while tx + tw > xs[ci] + clip_w && j < last {
                    let next = sheet.cell(CellRef::new(row, cols[j].0));
                    if next.is_some_and(|c| !c.value.is_empty() || c.format.fill.is_some()) {
                        break;
                    }
                    clip_w += cols[j].1;
                    j += 1;
                }
            }
            if clip_w > w {
                // Hide the gridlines the text runs over, as on screen.
                let _ = write!(
                    s,
                    "1 g {:.2} {:.2} {:.2} {:.2} re f\n",
                    xs[ci] + w - 0.3,
                    ys[ri] - h + 0.3,
                    clip_w - w,
                    h - 0.6
                );
            }
            let ty = ys[ri] - h / 2.0 - FONT_SIZE * 0.35;
            let font = match (f.bold, f.italic) {
                (false, false) => "F1",
                (true, false) => "F2",
                (false, true) => "F3",
                (true, true) => "F4",
            };
            let (r, g, b) = f.color.map_or((0.0, 0.0, 0.0), rgb);
            let _ = write!(
                s,
                "q {:.2} {:.2} {clip_w:.2} {h:.2} re W n BT /{font} {FONT_SIZE} Tf {r:.3} {g:.3} {b:.3} rg {tx:.2} {ty:.2} Td {} Tj ET Q\n",
                xs[ci],
                ys[ri] - h,
                pdf_string(&text)
            );
        }
    }
    s
}

/// Writes the PDF file structure around the page content streams.
fn assemble(streams: &[String], pw: f64, ph: f64) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
    let object = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    let n = streams.len();
    // 1 catalog, 2 pages, 3..=6 fonts, then (page, content) pairs.
    let first_page = 7;
    object(&mut out, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", first_page + 2 * i)).collect();
    object(
        &mut out,
        &mut offsets,
        format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")).as_bytes(),
    );
    for base in ["Helvetica", "Helvetica-Bold", "Helvetica-Oblique", "Helvetica-BoldOblique"] {
        object(
            &mut out,
            &mut offsets,
            format!("<< /Type /Font /Subtype /Type1 /BaseFont /{base} /Encoding /WinAnsiEncoding >>")
                .as_bytes(),
        );
    }
    for (i, content) in streams.iter().enumerate() {
        let page = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.0} {ph:.0}] /Resources << /Font << /F1 3 0 R /F2 4 0 R /F3 5 0 R /F4 6 0 R >> >> /Contents {} 0 R >>",
            first_page + 2 * i + 1
        );
        object(&mut out, &mut offsets, page.as_bytes());
        let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        stream.extend_from_slice(content.as_bytes());
        stream.extend_from_slice(b"\nendstream");
        object(&mut out, &mut offsets, &stream);
    }
    let xref = out.len();
    let mut table = format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1);
    for off in &offsets {
        let _ = write!(table, "{off:010} 00000 n \n");
    }
    let _ =
        write!(table, "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1);
    out.extend_from_slice(table.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::parse_input;
    use crate::sheet::CellState;
    use crate::workbook::Workbook;

    fn book(rows: u32) -> Workbook {
        let mut b = Workbook::new();
        let mut changes = Vec::new();
        for r in 0..rows {
            for (c, text) in
                [format!("Rij {}", r + 1), format!("{}", r * 3), "€ 12.50".to_string()].iter().enumerate()
            {
                let p = parse_input(text).unwrap();
                let format =
                    crate::format::CellFormat { num: p.format.unwrap_or_default(), ..Default::default() };
                changes.push((CellRef::new(r, c as u32), CellState { content: p.content, format }));
            }
        }
        b.set_states(0, changes);
        b
    }

    /// Checks the cross-reference table: every offset points at its object.
    fn check_structure(pdf: &[u8]) -> usize {
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        // Offsets are in bytes, so work on the bytes (the header has binary ones).
        let tail = String::from_utf8_lossy(&pdf[pdf.len() - 40..]).to_string();
        let start: usize =
            tail.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        let table = String::from_utf8(pdf[start..].to_vec()).unwrap();
        let mut count = 0;
        for (i, line) in table.lines().skip(3).take_while(|l| l.ends_with(" n ")).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            assert!(pdf[off..].starts_with(format!("{} 0 obj", i + 1).as_bytes()), "object {}", i + 1);
            count += 1;
        }
        count
    }

    #[test]
    fn writes_a_valid_pdf_with_the_cell_text() {
        let b = book(5);
        let pdf = sheet_to_pdf(b.sheet(0), None);
        assert_eq!(check_structure(&pdf), 8); // catalog, pages, 4 fonts, 1 page + content
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains("(Rij 1) Tj"));
        assert!(text.contains("(\\200 12.50) Tj"), "euro sign in WinAnsi");
        assert!(text.contains("/Count 1"));
    }

    #[test]
    fn long_sheets_split_over_pages_and_skip_hidden_rows() {
        let mut b = book(200);
        let pdf = sheet_to_pdf(b.sheet(0), None);
        let pages = String::from_utf8_lossy(&pdf).matches("/Type /Page ").count();
        assert!(pages >= 4, "{pages} pages");
        let id = b.sheet(0).id();
        b.set_hidden_rows(id, (1..200).collect());
        let pdf = sheet_to_pdf(b.sheet(0), None);
        let text = String::from_utf8_lossy(&pdf);
        assert_eq!(text.matches("/Type /Page ").count(), 1);
        assert!(!text.contains("(Rij 2) Tj"));
    }

    #[test]
    fn helvetica_metrics_and_escaping() {
        assert_eq!(text_width("Hi", false, 10.0), (722.0 + 222.0) / 100.0);
        assert!(text_width("Hi", true, 10.0) > text_width("Hi", false, 10.0));
        assert_eq!(pdf_string("a(b)\\c"), "(a\\(b\\)\\\\c)");
        assert_eq!(pdf_string("café"), "(caf\\351)");
        assert_eq!(pdf_string("日"), "(?)");
    }
}
