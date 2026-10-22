//! Computed cell values and the coercion rules formulas use.

use std::cmp::Ordering;
use std::fmt;

/// Error values a cell can hold, matching Excel's error literals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    Div0,
    Ref,
    Name,
    Value,
    Num,
    NA,
    Null,
    /// Circular reference. Excel shows a warning instead; a visible error is clearer.
    Circ,
}

impl ErrorKind {
    pub const ALL: [ErrorKind; 8] = [
        ErrorKind::Div0,
        ErrorKind::Ref,
        ErrorKind::Name,
        ErrorKind::Value,
        ErrorKind::Num,
        ErrorKind::NA,
        ErrorKind::Null,
        ErrorKind::Circ,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Div0 => "#DIV/0!",
            ErrorKind::Ref => "#REF!",
            ErrorKind::Name => "#NAME?",
            ErrorKind::Value => "#VALUE!",
            ErrorKind::Num => "#NUM!",
            ErrorKind::NA => "#N/A",
            ErrorKind::Null => "#NULL!",
            ErrorKind::Circ => "#CIRC!",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str().eq_ignore_ascii_case(s))
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The value of a cell after evaluation.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Empty,
    Number(f64),
    Text(String),
    Bool(bool),
    Error(ErrorKind),
}

impl Value {
    pub fn is_empty(&self) -> bool {
        matches!(self, Value::Empty)
    }

    /// Wraps a float, turning NaN and infinities into `#NUM!`.
    pub fn number(n: f64) -> Value {
        if n.is_finite() {
            // Normalise -0 so it never displays as "-0".
            Value::Number(if n == 0.0 { 0.0 } else { n })
        } else {
            Value::Error(ErrorKind::Num)
        }
    }

    /// Numeric coercion used by arithmetic operators.
    pub fn to_number(&self) -> Result<f64, ErrorKind> {
        match self {
            Value::Empty => Ok(0.0),
            Value::Number(n) => Ok(*n),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::Text(s) => parse_number(s).ok_or(ErrorKind::Value),
            Value::Error(e) => Err(*e),
        }
    }

    /// Boolean coercion used by `IF`, `AND`, `OR` and `NOT`.
    pub fn to_bool(&self) -> Result<bool, ErrorKind> {
        match self {
            Value::Empty => Ok(false),
            Value::Number(n) => Ok(*n != 0.0),
            Value::Bool(b) => Ok(*b),
            Value::Text(s) if s.eq_ignore_ascii_case("TRUE") => Ok(true),
            Value::Text(s) if s.eq_ignore_ascii_case("FALSE") => Ok(false),
            Value::Text(_) => Err(ErrorKind::Value),
            Value::Error(e) => Err(*e),
        }
    }

    /// Text coercion used by `&` and `CONCAT`.
    pub fn to_text(&self) -> Result<String, ErrorKind> {
        match self {
            Value::Error(e) => Err(*e),
            other => Ok(other.to_string()),
        }
    }

    /// Excel ordering for comparison operators: numbers < text < booleans,
    /// text compared case-insensitively, empty acting as 0, "" or FALSE.
    pub fn compare(&self, other: &Value) -> Ordering {
        fn rank(v: &Value) -> u8 {
            match v {
                Value::Number(_) | Value::Empty => 0,
                Value::Text(_) => 1,
                Value::Bool(_) => 2,
                Value::Error(_) => 3,
            }
        }
        match (self, other) {
            (Value::Empty, Value::Empty) => Ordering::Equal,
            (Value::Empty, Value::Text(t)) => "".cmp(t.to_lowercase().as_str()),
            (Value::Text(t), Value::Empty) => t.to_lowercase().as_str().cmp(""),
            (Value::Empty, Value::Bool(b)) => false.cmp(b),
            (Value::Bool(b), Value::Empty) => b.cmp(&false),
            (Value::Empty, Value::Number(n)) => 0.0f64.total_cmp(n),
            (Value::Number(n), Value::Empty) => n.total_cmp(&0.0),
            (Value::Number(a), Value::Number(b)) => a.total_cmp(b),
            (Value::Text(a), Value::Text(b)) => a.to_lowercase().cmp(&b.to_lowercase()),
            (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
            (a, b) => rank(a).cmp(&rank(b)),
        }
    }
}

impl fmt::Display for Value {
    /// The "General" rendering, also used when converting numbers to text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Empty => Ok(()),
            Value::Number(n) => f.write_str(&format_general(*n)),
            Value::Text(s) => f.write_str(s),
            Value::Bool(true) => f.write_str("TRUE"),
            Value::Bool(false) => f.write_str("FALSE"),
            Value::Error(e) => f.write_str(e.as_str()),
        }
    }
}

/// Parses the number syntax accepted in cells and numeric text.
pub fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    // Reject the words Rust accepts but a spreadsheet must not ("inf", "nan").
    let first = t.trim_start_matches(['+', '-']).chars().next()?;
    if !(first.is_ascii_digit() || first == '.') {
        return None;
    }
    t.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// Formats a number like Excel's General format: at most 15 significant
/// digits, trailing zeros trimmed, scientific notation for very large or
/// very small magnitudes.
pub fn format_general(n: f64) -> String {
    if n == 0.0 {
        return "0".to_string();
    }
    let abs = n.abs();
    if !(1e-9..1e15).contains(&abs) {
        let s = format!("{:.5E}", n);
        // "1.23450E3" -> "1.2345E+03"
        let (mantissa, exp) = s.split_once('E').expect("scientific");
        let mantissa = trim_zeros(mantissa);
        let exp: i32 = exp.parse().expect("exponent");
        return format!("{}E{}{:02}", mantissa, if exp < 0 { '-' } else { '+' }, exp.abs());
    }
    let digits_before = abs.log10().floor() as i32 + 1;
    let decimals = (15 - digits_before).clamp(0, 15) as usize;
    trim_zeros(&format!("{:.*}", decimals, n)).to_string()
}

fn trim_zeros(s: &str) -> &str {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn general_format() {
        assert_eq!(format_general(3.0), "3");
        assert_eq!(format_general(-2.5), "-2.5");
        assert_eq!(format_general(0.1 + 0.2), "0.3");
        assert_eq!(format_general(1.0 / 3.0), "0.333333333333333");
        assert_eq!(format_general(123456789.0), "123456789");
        assert_eq!(format_general(1e20), "1E+20");
        assert_eq!(format_general(1.5e-12), "1.5E-12");
    }

    #[test]
    fn coercions() {
        assert_eq!(Value::Text(" 4.5 ".into()).to_number(), Ok(4.5));
        assert_eq!(Value::Text("abc".into()).to_number(), Err(ErrorKind::Value));
        assert_eq!(Value::Text("inf".into()).to_number(), Err(ErrorKind::Value));
        assert_eq!(Value::Bool(true).to_number(), Ok(1.0));
        assert_eq!(Value::Empty.to_bool(), Ok(false));
        assert_eq!(Value::Number(2.0).to_text(), Ok("2".to_string()));
    }

    #[test]
    fn excel_ordering() {
        assert_eq!(Value::Number(100.0).compare(&Value::Text("a".into())), Ordering::Less);
        assert_eq!(Value::Text("ABC".into()).compare(&Value::Text("abc".into())), Ordering::Equal);
        assert_eq!(Value::Empty.compare(&Value::Number(0.0)), Ordering::Equal);
        assert_eq!(Value::Bool(false).compare(&Value::Text("z".into())), Ordering::Greater);
    }

    #[test]
    fn error_literals() {
        for e in ErrorKind::ALL {
            assert_eq!(ErrorKind::parse(e.as_str()), Some(e));
        }
    }
}
