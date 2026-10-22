//! Cell formatting: number formats, bold and horizontal alignment.

use crate::value::{Value, format_general};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum NumFmt {
    #[default]
    General,
    /// `#,##0.00`
    Number,
    /// `€ #,##0.00`
    Currency,
    /// `0.00%`
    Percent,
    /// `dd-mm-yyyy`
    Date,
}

impl NumFmt {
    pub const ALL: [NumFmt; 5] =
        [NumFmt::General, NumFmt::Number, NumFmt::Currency, NumFmt::Percent, NumFmt::Date];

    pub fn label(self) -> &'static str {
        match self {
            NumFmt::General => "Standaard",
            NumFmt::Number => "Getal",
            NumFmt::Currency => "Valuta",
            NumFmt::Percent => "Percentage",
            NumFmt::Date => "Datum",
        }
    }

    /// The Excel format code written to xlsx files.
    pub fn excel_code(self) -> &'static str {
        match self {
            NumFmt::General => "General",
            NumFmt::Number => "#,##0.00",
            NumFmt::Currency => "\"€\" #,##0.00",
            NumFmt::Percent => "0.00%",
            NumFmt::Date => "dd-mm-yyyy",
        }
    }

    /// Maps an Excel format code read from a file onto the closest format.
    pub fn from_excel_code(code: &str) -> NumFmt {
        let lower = code.to_ascii_lowercase();
        if lower.is_empty() || lower == "general" || lower == "@" {
            NumFmt::General
        } else if lower.contains('%') {
            NumFmt::Percent
        } else if lower.contains('€') || lower.contains('$') || lower.contains("[$") {
            NumFmt::Currency
        } else if is_date_code(&lower) {
            NumFmt::Date
        } else if lower.contains('0') || lower.contains('#') {
            NumFmt::Number
        } else {
            NumFmt::General
        }
    }
}

fn is_date_code(lower: &str) -> bool {
    // Ignore quoted literals and bracketed sections such as colours.
    let mut plain = String::new();
    let mut quoted = false;
    let mut bracket = false;
    for c in lower.chars() {
        match c {
            '"' => quoted = !quoted,
            '[' if !quoted => bracket = true,
            ']' if !quoted => bracket = false,
            _ if !quoted && !bracket => plain.push(c),
            _ => {}
        }
    }
    plain.contains('d') || plain.contains('y') || (plain.contains('m') && !plain.contains('0'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Align {
    /// Numbers right, text left, booleans and errors centred.
    #[default]
    General,
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct CellFormat {
    pub num: NumFmt,
    pub bold: bool,
    pub align: Align,
}

impl CellFormat {
    pub fn is_default(&self) -> bool {
        *self == CellFormat::default()
    }

    /// The alignment actually used for `value`.
    pub fn effective_align(&self, value: &Value) -> Align {
        match self.align {
            Align::General => match value {
                Value::Number(_) => Align::Right,
                Value::Bool(_) | Value::Error(_) => Align::Center,
                _ => Align::Left,
            },
            a => a,
        }
    }
}

/// Text shown in a cell for `value` with number format `fmt`.
pub fn format_value(value: &Value, fmt: NumFmt) -> String {
    let Value::Number(n) = value else { return value.to_string() };
    let n = *n;
    match fmt {
        NumFmt::General => format_general(n),
        NumFmt::Number => group_thousands(n, 2),
        NumFmt::Currency => {
            let body = group_thousands(n.abs(), 2);
            if n < 0.0 { format!("€ -{body}") } else { format!("€ {body}") }
        }
        NumFmt::Percent => format!("{:.2}%", n * 100.0),
        NumFmt::Date => match serial_to_date(n) {
            Some((y, m, d)) => format!("{d:02}-{m:02}-{y:04}"),
            None => "#####".to_string(),
        },
    }
}

/// `1234567.891` -> `1,234,567.89`
fn group_thousands(n: f64, decimals: usize) -> String {
    let s = format!("{:.*}", decimals, n.abs());
    let (int, frac) = s.split_once('.').map_or((s.as_str(), ""), |(a, b)| (a, b));
    let mut out = String::with_capacity(s.len() + int.len() / 3 + 1);
    if n < 0.0 && s.bytes().any(|b| (b'1'..=b'9').contains(&b)) {
        out.push('-');
    }
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    out
}

/// Days from 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Excel's day zero in the 1900 date system, as days from 1970-01-01.
const EXCEL_EPOCH: i64 = -25_569; // 1899-12-30

/// Converts an Excel serial date (1900 system) to (year, month, day).
pub fn serial_to_date(serial: f64) -> Option<(i64, u32, u32)> {
    if !(0.0..2_958_466.0).contains(&serial) {
        return None;
    }
    let days = serial.floor() as i64;
    // Excel pretends 1900 was a leap year; serial 60 is the fictional 29 Feb 1900.
    if days == 60 {
        return Some((1900, 2, 29));
    }
    let days = if days < 60 { days + 1 } else { days };
    Some(civil_from_days(EXCEL_EPOCH + days))
}

/// Converts a calendar date to an Excel serial number (1900 system).
pub fn date_to_serial(y: i64, m: u32, d: u32) -> Option<f64> {
    if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) || !(1900..=9999).contains(&y) {
        return None;
    }
    let days = days_from_civil(y, m, d) - EXCEL_EPOCH;
    // Undo the fake leap day for dates before 1 March 1900.
    Some((if days < 61 { days - 1 } else { days }) as f64)
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 31,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formats() {
        let v = Value::Number(1234567.891);
        assert_eq!(format_value(&v, NumFmt::General), "1234567.891");
        assert_eq!(format_value(&v, NumFmt::Number), "1,234,567.89");
        assert_eq!(format_value(&Value::Number(-1234.5), NumFmt::Currency), "€ -1,234.50");
        assert_eq!(format_value(&Value::Number(0.125), NumFmt::Percent), "12.50%");
        assert_eq!(format_value(&Value::Number(-0.001), NumFmt::Number), "0.00");
        assert_eq!(format_value(&Value::Text("x".into()), NumFmt::Number), "x");
    }

    #[test]
    fn excel_dates() {
        assert_eq!(serial_to_date(1.0), Some((1900, 1, 1)));
        assert_eq!(serial_to_date(59.0), Some((1900, 2, 28)));
        assert_eq!(serial_to_date(60.0), Some((1900, 2, 29)));
        assert_eq!(serial_to_date(61.0), Some((1900, 3, 1)));
        assert_eq!(serial_to_date(45000.0), Some((2023, 3, 15)));
        assert_eq!(date_to_serial(2023, 3, 15), Some(45000.0));
        assert_eq!(date_to_serial(1900, 1, 1), Some(1.0));
        assert_eq!(date_to_serial(1900, 3, 1), Some(61.0));
        assert_eq!(date_to_serial(2023, 2, 29), None);
        assert_eq!(format_value(&Value::Number(46317.0), NumFmt::Date), "22-10-2026");
    }

    #[test]
    fn reads_excel_format_codes() {
        assert_eq!(NumFmt::from_excel_code("0.00%"), NumFmt::Percent);
        assert_eq!(NumFmt::from_excel_code("[$€-413] #,##0.00"), NumFmt::Currency);
        assert_eq!(NumFmt::from_excel_code("d-m-yyyy"), NumFmt::Date);
        assert_eq!(NumFmt::from_excel_code("[Red]#,##0"), NumFmt::Number);
        assert_eq!(NumFmt::from_excel_code("General"), NumFmt::General);
        for f in NumFmt::ALL {
            assert_eq!(NumFmt::from_excel_code(f.excel_code()), f);
        }
    }
}
