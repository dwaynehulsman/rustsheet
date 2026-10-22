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
    pub italic: bool,
    pub align: Align,
    /// Text colour; `None` is the default (black, or white in dark mode).
    pub color: Option<Rgb>,
    /// Background fill; `None` is no fill.
    pub fill: Option<Rgb>,
}

/// An sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Parses `RRGGBB` or Excel's `AARRGGBB`.
    pub fn from_hex(s: &str) -> Option<Rgb> {
        let hex = match s.len() {
            8 => &s[2..],
            6 => s,
            _ => return None,
        };
        let v = u32::from_str_radix(hex, 16).ok()?;
        Some(Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }

    pub fn to_u32(self) -> u32 {
        (u32::from(self.0) << 16) | (u32::from(self.1) << 8) | u32::from(self.2)
    }
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

const DAYS: [&str; 7] = ["zondag", "maandag", "dinsdag", "woensdag", "donderdag", "vrijdag", "zaterdag"];
const MONTHS: [&str; 12] = [
    "januari",
    "februari",
    "maart",
    "april",
    "mei",
    "juni",
    "juli",
    "augustus",
    "september",
    "oktober",
    "november",
    "december",
];
const MONTHS_SHORT: [&str; 12] =
    ["jan", "feb", "mrt", "apr", "mei", "jun", "jul", "aug", "sep", "okt", "nov", "dec"];

/// Day of the week for an Excel serial date: 0 is Sunday.
pub fn weekday(serial: f64) -> u32 {
    ((serial.floor() as i64 - 1).rem_euclid(7)) as u32
}

/// One token of a format code section.
#[derive(Debug, PartialEq)]
enum Tok {
    Lit(String),
    /// Quoted or backslash-escaped text: never a placeholder.
    Quoted(String),
    /// A run of one date/time letter, e.g. `dd` or `yyyy`.
    Date(char, usize),
    AmPm,
}

/// Splits on `;` outside quotes and brackets.
fn sections(code: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (i, c) in code.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                out.push(&code[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&code[start..]);
    out
}

/// Literal text of a section: quoted parts, escaped characters, the rest as is.
fn literal_tokens(section: &str, date: bool) -> Vec<Tok> {
    let mut toks = Vec::new();
    let chars: Vec<char> = section.chars().collect();
    let mut i = 0;
    let push_lit = |toks: &mut Vec<Tok>, s: &str| match toks.last_mut() {
        Some(Tok::Lit(l)) => l.push_str(s),
        _ => toks.push(Tok::Lit(s.to_string())),
    };
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                let end = chars[i + 1..].iter().position(|&x| x == '"').map_or(chars.len(), |p| i + 1 + p);
                toks.push(Tok::Quoted(chars[i + 1..end.min(chars.len())].iter().collect()));
                i = end + 1;
            }
            '\\' if i + 1 < chars.len() => {
                toks.push(Tok::Quoted(chars[i + 1].to_string()));
                i += 2;
            }
            '[' => {
                // Colours and conditions are ignored.
                let end = chars[i..].iter().position(|&x| x == ']').map_or(chars.len(), |p| i + p);
                i = end + 1;
            }
            _ if date && section[char_offset(&chars, i)..].to_ascii_uppercase().starts_with("AM/PM") => {
                toks.push(Tok::AmPm);
                i += 5;
            }
            _ if date && "dmyhsDMYHS".contains(c) => {
                let lower = c.to_ascii_lowercase();
                let mut n = 1;
                while i + n < chars.len() && chars[i + n].to_ascii_lowercase() == lower {
                    n += 1;
                }
                toks.push(Tok::Date(lower, n));
                i += n;
            }
            _ => {
                push_lit(&mut toks, &c.to_string());
                i += 1;
            }
        }
    }
    toks
}

fn char_offset(chars: &[char], i: usize) -> usize {
    chars[..i].iter().map(|c| c.len_utf8()).sum()
}

fn format_date_section(serial: f64, section: &str) -> String {
    let toks = literal_tokens(section, true);
    let Some((y, mo, d)) = serial_to_date(serial) else { return "#####".to_string() };
    let secs = ((serial - serial.floor()) * 86_400.0).round() as u32;
    let (mut h, mi, s) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    let ampm = toks.contains(&Tok::AmPm);
    let pm = h >= 12;
    if ampm {
        h %= 12;
        if h == 0 {
            h = 12;
        }
    }
    let mut out = String::new();
    for (i, tok) in toks.iter().enumerate() {
        match tok {
            Tok::Lit(l) | Tok::Quoted(l) => out.push_str(l),
            Tok::AmPm => out.push_str(if pm { "PM" } else { "AM" }),
            Tok::Date('y', n) => {
                if *n <= 2 {
                    out.push_str(&format!("{:02}", y.rem_euclid(100)))
                } else {
                    out.push_str(&format!("{y:04}"))
                }
            }
            Tok::Date('d', n) => match n {
                1 => out.push_str(&d.to_string()),
                2 => out.push_str(&format!("{d:02}")),
                3 => out.push_str(&DAYS[weekday(serial) as usize][..2]),
                _ => out.push_str(DAYS[weekday(serial) as usize]),
            },
            Tok::Date('m', n) => {
                // m means minutes right after an hour or right before seconds.
                let prev = toks[..i].iter().rev().find(|t| matches!(t, Tok::Date(..)));
                let next = toks[i + 1..].iter().find(|t| matches!(t, Tok::Date(..)));
                let minutes =
                    matches!(prev, Some(Tok::Date('h', _))) || matches!(next, Some(Tok::Date('s', _)));
                if minutes {
                    out.push_str(&if *n >= 2 { format!("{mi:02}") } else { mi.to_string() });
                } else {
                    match n {
                        1 => out.push_str(&mo.to_string()),
                        2 => out.push_str(&format!("{mo:02}")),
                        3 => out.push_str(MONTHS_SHORT[mo as usize - 1]),
                        _ => out.push_str(MONTHS[mo as usize - 1]),
                    }
                }
            }
            Tok::Date('h', n) => out.push_str(&if *n >= 2 { format!("{h:02}") } else { h.to_string() }),
            Tok::Date('s', n) => out.push_str(&if *n >= 2 { format!("{s:02}") } else { s.to_string() }),
            Tok::Date(..) => {}
        }
    }
    out
}

fn format_number_section(n: f64, section: &str) -> String {
    // The numeric pattern runs from the first to the last digit placeholder.
    let toks = literal_tokens(section, false);
    let mut prefix = String::new();
    let mut pattern = String::new();
    let mut suffix = String::new();
    for tok in &toks {
        let l = match tok {
            Tok::Quoted(q) => {
                if pattern.is_empty() {
                    prefix.push_str(q)
                } else {
                    suffix.push_str(q)
                }
                continue;
            }
            Tok::Lit(l) => l,
            _ => continue,
        };
        for c in l.chars() {
            let placeholder = "0#?.,".contains(c);
            if placeholder && suffix.is_empty() {
                pattern.push(c);
            } else if pattern.is_empty() {
                prefix.push(c);
            } else {
                suffix.push(c);
            }
        }
    }
    let percent = prefix.contains('%') || suffix.contains('%');
    let n = if percent { n * 100.0 } else { n };
    if pattern.is_empty() {
        return format!("{prefix}{suffix}");
    }
    let (int_pat, frac_pat) = pattern.split_once('.').unwrap_or((&pattern, ""));
    let decimals = frac_pat.chars().filter(|c| matches!(c, '0' | '#' | '?')).count();
    let min_decimals = frac_pat.chars().filter(|&c| c == '0').count();
    let min_int = int_pat.chars().filter(|&c| c == '0').count();
    let group = int_pat.contains(',');
    let rounded = format!("{:.*}", decimals, n.abs());
    let (int_digits, frac_digits) = rounded.split_once('.').unwrap_or((&rounded, ""));
    let mut frac = frac_digits.to_string();
    while frac.len() > min_decimals && frac.ends_with('0') {
        frac.pop();
    }
    let int_digits = int_digits.trim_start_matches('0');
    let int_digits = if int_digits.len() < min_int {
        format!("{}{int_digits}", "0".repeat(min_int - int_digits.len()))
    } else {
        int_digits.to_string()
    };
    let mut int = String::new();
    for (i, c) in int_digits.chars().enumerate() {
        if group && i > 0 && (int_digits.len() - i) % 3 == 0 {
            int.push(',');
        }
        int.push(c);
    }
    let mut out = prefix;
    if n < 0.0 && (int_digits.chars().chain(frac.chars()).any(|c| c != '0')) {
        out.insert(0, '-');
    }
    out.push_str(&int);
    if !frac.is_empty() {
        out.push('.');
        out.push_str(&frac);
    }
    out.push_str(&suffix);
    out
}

/// Formats a number with an Excel format code, as the TEXT function does:
/// `0.00`, `#,##0`, `0%`, `"€" 0.00`, `dd-mm-yyyy`, `dddd d mmmm`, `hh:mm`,
/// with up to three sections (positive;negative;zero).
pub fn format_with_code(n: f64, code: &str) -> String {
    let parts = sections(code);
    let (section, value) = match parts.len() {
        1 => (parts[0], n),
        2 if n < 0.0 => (parts[1], -n),
        _ if n < 0.0 && parts.len() >= 2 => (parts[1], -n),
        3.. if n == 0.0 => (parts[2], n),
        _ => (parts[0], n),
    };
    if section.eq_ignore_ascii_case("general") || section.is_empty() {
        return format_general(value);
    }
    let lower = section.to_ascii_lowercase();
    let unquoted: String = {
        let mut q = false;
        lower
            .chars()
            .filter(|&c| {
                if c == '"' {
                    q = !q;
                }
                !q
            })
            .collect()
    };
    if is_date_code(&lower) || unquoted.contains('h') || unquoted.contains('s') {
        format_date_section(value, section)
    } else {
        format_number_section(value, section)
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
    fn colours_parse_from_excel_hex() {
        assert_eq!(Rgb::from_hex("FFC00000"), Some(Rgb(0xC0, 0, 0)));
        assert_eq!(Rgb::from_hex("0070C0"), Some(Rgb(0, 0x70, 0xC0)));
        assert_eq!(Rgb::from_hex("xyz"), None);
        assert_eq!(Rgb(0x12, 0x34, 0x56).to_u32(), 0x123456);
    }

    #[test]
    fn excel_format_codes_for_text() {
        assert_eq!(format_with_code(1234.567, "0.00"), "1234.57");
        assert_eq!(format_with_code(1234.567, "#,##0"), "1,235");
        assert_eq!(format_with_code(1234.5, "#,##0.00"), "1,234.50");
        assert_eq!(format_with_code(0.256, "0%"), "26%");
        assert_eq!(format_with_code(0.256, "0.0%"), "25.6%");
        assert_eq!(format_with_code(7.0, "000"), "007");
        assert_eq!(format_with_code(7.0, "\"Nr 0\"0"), "Nr 07");
        assert_eq!(format_with_code(3.5, "\"€\" 0.00"), "€ 3.50");
        assert_eq!(format_with_code(-5.0, "0;(0)"), "(5)");
        assert_eq!(format_with_code(0.0, "0;-0;\"nul\""), "nul");
        assert_eq!(format_with_code(1.5, "#.##"), "1.5");
        assert_eq!(format_with_code(-1.234, "0.0"), "-1.2");
        assert_eq!(format_with_code(46317.0, "dd-mm-yyyy"), "22-10-2026");
        assert_eq!(format_with_code(46317.0, "dddd d mmmm yyyy"), "donderdag 22 oktober 2026");
        assert_eq!(format_with_code(46317.0, "ddd d mmm yy"), "do 22 okt 26");
        assert_eq!(format_with_code(46317.75, "hh:mm"), "18:00");
        assert_eq!(format_with_code(46317.75, "h:mm AM/PM"), "6:00 PM");
        assert_eq!(format_with_code(46317.5 + 61.0 / 86400.0, "hh:mm:ss"), "12:01:01");
        assert_eq!(format_with_code(46317.0, "yyyy"), "2026");
        assert_eq!(weekday(46317.0), 4);
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
