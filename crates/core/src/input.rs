//! Turning typed text into cell content, and cell content back into text
//! for the editor.

use crate::format::{NumFmt, date_to_serial, serial_to_date};
use crate::formula::{Expr, ParseError, parse_formula};
use crate::value::{ErrorKind, Value, format_general, parse_number};

/// What the user put in a cell, as opposed to what it evaluates to.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Content {
    #[default]
    Empty,
    /// A literal number, text, boolean or error.
    Value(Value),
    Formula(Expr),
}

impl Content {
    pub fn is_empty(&self) -> bool {
        matches!(self, Content::Empty)
    }

    pub fn formula(&self) -> Option<&Expr> {
        match self {
            Content::Formula(e) => Some(e),
            _ => None,
        }
    }

    /// The formula text with leading `=`, or the literal as typed.
    pub fn source(&self) -> String {
        match self {
            Content::Empty => String::new(),
            Content::Value(v) => v.to_string(),
            Content::Formula(e) => format!("={e}"),
        }
    }
}

/// Result of interpreting typed text.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub content: Content,
    /// A number format implied by the text, such as `12%` or `€ 5`.
    pub format: Option<NumFmt>,
}

impl Parsed {
    fn plain(content: Content) -> Self {
        Self { content, format: None }
    }
}

/// Interprets text typed into a cell. Fails only for malformed formulas.
pub fn parse_input(text: &str) -> Result<Parsed, ParseError> {
    if text.is_empty() {
        return Ok(Parsed::plain(Content::Empty));
    }
    if let Some(rest) = text.strip_prefix('\'') {
        return Ok(Parsed::plain(Content::Value(Value::Text(rest.to_string()))));
    }
    if text.len() > 1 && text.starts_with('=') {
        return Ok(Parsed::plain(Content::Formula(parse_formula(text)?)));
    }
    Ok(parse_literal(text))
}

/// Interprets text as a literal, never as a formula. Used for pasted text
/// and CSV values where a bad formula should simply stay text.
pub fn parse_literal(text: &str) -> Parsed {
    let t = text.trim();
    if t.is_empty() {
        return Parsed::plain(if text.is_empty() {
            Content::Empty
        } else {
            Content::Value(Value::Text(text.to_string()))
        });
    }
    if let Some((n, format)) = parse_formatted_number(t) {
        return Parsed { content: Content::Value(Value::Number(n)), format };
    }
    if let Some(serial) = parse_date(t) {
        return Parsed { content: Content::Value(Value::Number(serial)), format: Some(NumFmt::Date) };
    }
    if t.eq_ignore_ascii_case("TRUE") {
        return Parsed::plain(Content::Value(Value::Bool(true)));
    }
    if t.eq_ignore_ascii_case("FALSE") {
        return Parsed::plain(Content::Value(Value::Bool(false)));
    }
    if let Some(e) = ErrorKind::parse(t) {
        return Parsed::plain(Content::Value(Value::Error(e)));
    }
    Parsed::plain(Content::Value(Value::Text(text.to_string())))
}

/// Numbers with optional percent sign, euro sign and thousands separators.
fn parse_formatted_number(t: &str) -> Option<(f64, Option<NumFmt>)> {
    if let Some(body) = t.strip_suffix('%') {
        return plain_number(body.trim_end()).map(|n| (n / 100.0, Some(NumFmt::Percent)));
    }
    let euro = t.starts_with('€') || t.ends_with('€');
    if euro {
        let body = t.trim_matches('€').trim();
        return plain_number(body).map(|n| (n, Some(NumFmt::Currency)));
    }
    if t.contains(',') {
        return plain_number(t).map(|n| (n, Some(NumFmt::Number)));
    }
    plain_number(t).map(|n| (n, None))
}

/// A number, allowing `1,234,567.8` style grouping.
fn plain_number(t: &str) -> Option<f64> {
    if !t.contains(',') {
        return parse_number(t);
    }
    let (sign, digits) = match t.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, t),
    };
    let (int, frac) = digits.split_once('.').map_or((digits, None), |(a, b)| (a, Some(b)));
    let mut groups = int.split(',');
    let first = groups.next()?;
    if first.is_empty() || first.len() > 3 || !first.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut joined = first.to_string();
    for g in groups {
        if g.len() != 3 || !g.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        joined.push_str(g);
    }
    if let Some(frac) = frac {
        if frac.is_empty() || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        joined.push('.');
        joined.push_str(frac);
    }
    joined.parse::<f64>().ok().map(|n| sign * n)
}

/// `22-10-2026`, `22/10/2026` (day first, the Dutch order) or `2026-10-22`.
fn parse_date(t: &str) -> Option<f64> {
    let sep = if t.contains('-') {
        '-'
    } else if t.contains('/') {
        '/'
    } else {
        return None;
    };
    let parts: Vec<&str> = t.split(sep).collect();
    if parts.len() != 3 || !parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let nums: Vec<i64> = parts.iter().map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let (y, m, d) = if parts[0].len() == 4 {
        (nums[0], nums[1], nums[2])
    } else if parts[2].len() == 4 {
        (nums[2], nums[1], nums[0])
    } else {
        return None;
    };
    date_to_serial(y, u32::try_from(m).ok()?, u32::try_from(d).ok()?)
}

/// The text the editor shows for a cell, chosen so that typing it back in
/// gives the same content and format.
pub fn edit_text(content: &Content, fmt: NumFmt) -> String {
    match content {
        Content::Empty => String::new(),
        Content::Formula(e) => format!("={e}"),
        Content::Value(Value::Number(n)) => match fmt {
            NumFmt::Percent => format!("{}%", format_general(n * 100.0)),
            NumFmt::Date => match serial_to_date(*n) {
                Some((y, m, d)) if n.fract() == 0.0 => format!("{d:02}-{m:02}-{y:04}"),
                _ => format_general(*n),
            },
            _ => format_general(*n),
        },
        Content::Value(Value::Text(s)) => {
            // Text that would read back as something else needs the ' marker.
            let reparsed = parse_input(s).ok().map(|p| p.content);
            if reparsed == Some(Content::Value(Value::Text(s.clone()))) { s.clone() } else { format!("'{s}") }
        }
        Content::Value(v) => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(text: &str) -> (Content, Option<NumFmt>) {
        let p = parse_input(text).unwrap();
        (p.content, p.format)
    }

    #[test]
    fn recognises_literals() {
        assert_eq!(value("42"), (Content::Value(Value::Number(42.0)), None));
        assert_eq!(value("-1.5e2"), (Content::Value(Value::Number(-150.0)), None));
        assert_eq!(value("12.5%"), (Content::Value(Value::Number(0.125)), Some(NumFmt::Percent)));
        assert_eq!(value("€ 1,250.75"), (Content::Value(Value::Number(1250.75)), Some(NumFmt::Currency)));
        assert_eq!(value("1,234"), (Content::Value(Value::Number(1234.0)), Some(NumFmt::Number)));
        assert_eq!(value("22-10-2026"), (Content::Value(Value::Number(46317.0)), Some(NumFmt::Date)));
        assert_eq!(value("2026-10-22"), (Content::Value(Value::Number(46317.0)), Some(NumFmt::Date)));
        assert_eq!(value("true"), (Content::Value(Value::Bool(true)), None));
        assert_eq!(value("#N/A"), (Content::Value(Value::Error(ErrorKind::NA)), None));
        assert_eq!(value("12,34"), (Content::Value(Value::Text("12,34".into())), None));
        assert_eq!(value("'123"), (Content::Value(Value::Text("123".into())), None));
        assert_eq!(value("="), (Content::Value(Value::Text("=".into())), None));
        assert_eq!(value(""), (Content::Empty, None));
        assert!(matches!(value("=A1*2").0, Content::Formula(_)));
        assert!(parse_input("=1+").is_err());
    }

    #[test]
    fn edit_text_round_trips() {
        for (text, fmt) in [
            ("42", NumFmt::General),
            ("12.5%", NumFmt::Percent),
            ("22-10-2026", NumFmt::Date),
            ("'007", NumFmt::General),
            ("'=not a formula", NumFmt::General),
            ("hello", NumFmt::General),
            ("=SUM(A1:A3)", NumFmt::General),
        ] {
            let p = parse_input(text).unwrap();
            assert_eq!(edit_text(&p.content, fmt), text);
        }
    }
}
