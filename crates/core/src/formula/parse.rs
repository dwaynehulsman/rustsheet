//! Tokenizer and Pratt parser for Excel-style formulas.

use std::fmt;

use super::ast::{BinOp, CellAddr, Coord, Expr, RangeAddr, RangeKind, UnaryOp};
use crate::addr::{parse_col, parse_row};
use crate::value::ErrorKind;

#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub message: String,
    /// Byte offset in the formula text (after the leading `=`).
    pub pos: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (positie {})", self.message, self.pos + 1)
    }
}

impl std::error::Error for ParseError {}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    /// Identifier-like run: names, references, `TRUE`.
    Word(String),
    /// `Sheet1!` or `'My sheet'!`
    Sheet(String),
    Error(ErrorKind),
    Op(&'static str),
    LParen,
    RParen,
    /// `,` or `;` (the separator Dutch Excel uses).
    Sep,
    Colon,
    Eof,
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn err<T>(&self, message: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError { message: message.into(), pos: self.pos })
    }

    fn peek_char(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn tokens(mut self) -> Result<Vec<(Tok, usize)>, ParseError> {
        let mut out = Vec::new();
        loop {
            while self.peek_char().is_some_and(char::is_whitespace) {
                self.pos += self.peek_char().map_or(0, char::len_utf8);
            }
            let start = self.pos;
            let Some(c) = self.peek_char() else {
                out.push((Tok::Eof, start));
                return Ok(out);
            };
            let tok = match c {
                '0'..='9' => self.number()?,
                '.' if self.src[self.pos + 1..].starts_with(|c: char| c.is_ascii_digit()) => self.number()?,
                '"' => self.string()?,
                '\'' => self.quoted_sheet()?,
                '#' => self.error_literal()?,
                c if c.is_alphabetic() || c == '_' || c == '$' || c == '\\' => self.word(),
                _ => {
                    self.pos += c.len_utf8();
                    match c {
                        '(' => Tok::LParen,
                        ')' => Tok::RParen,
                        ',' | ';' => Tok::Sep,
                        ':' => Tok::Colon,
                        '+' => Tok::Op("+"),
                        '-' => Tok::Op("-"),
                        '*' => Tok::Op("*"),
                        '/' => Tok::Op("/"),
                        '^' => Tok::Op("^"),
                        '&' => Tok::Op("&"),
                        '%' => Tok::Op("%"),
                        '=' => Tok::Op("="),
                        '<' if self.eat('>') => Tok::Op("<>"),
                        '<' if self.eat('=') => Tok::Op("<="),
                        '<' => Tok::Op("<"),
                        '>' if self.eat('=') => Tok::Op(">="),
                        '>' => Tok::Op(">"),
                        _ => {
                            self.pos = start;
                            return self.err(format!("onverwacht teken '{c}'"));
                        }
                    }
                }
            };
            out.push((tok, start));
        }
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek_char() == Some(c) {
            self.pos += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn number(&mut self) -> Result<Tok, ParseError> {
        let bytes = self.src.as_bytes();
        let start = self.pos;
        let mut i = self.pos;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
        }
        if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
            let mut j = i + 1;
            if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
                j += 1;
            }
            if j < bytes.len() && bytes[j].is_ascii_digit() {
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
            }
        }
        self.pos = i;
        match self.src[start..i].parse::<f64>() {
            Ok(n) if n.is_finite() => Ok(Tok::Num(n)),
            _ => {
                self.pos = start;
                self.err("ongeldig getal")
            }
        }
    }

    fn string(&mut self) -> Result<Tok, ParseError> {
        let start = self.pos;
        self.pos += 1;
        let mut s = String::new();
        loop {
            match self.peek_char() {
                None => {
                    self.pos = start;
                    return self.err("tekst zonder afsluitend aanhalingsteken");
                }
                Some('"') => {
                    self.pos += 1;
                    if self.eat('"') {
                        s.push('"');
                    } else {
                        return Ok(Tok::Str(s));
                    }
                }
                Some(c) => {
                    s.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
    }

    fn quoted_sheet(&mut self) -> Result<Tok, ParseError> {
        let start = self.pos;
        self.pos += 1;
        let mut s = String::new();
        loop {
            match self.peek_char() {
                None => {
                    self.pos = start;
                    return self.err("bladnaam zonder afsluitend aanhalingsteken");
                }
                Some('\'') => {
                    self.pos += 1;
                    if self.eat('\'') {
                        s.push('\'');
                    } else if self.eat('!') {
                        return Ok(Tok::Sheet(s));
                    } else {
                        return self.err("verwacht '!' na bladnaam");
                    }
                }
                Some(c) => {
                    s.push(c);
                    self.pos += c.len_utf8();
                }
            }
        }
    }

    fn error_literal(&mut self) -> Result<Tok, ParseError> {
        let rest = &self.src[self.pos..];
        for e in ErrorKind::ALL {
            let lit = e.as_str();
            if rest.get(..lit.len()).is_some_and(|head| head.eq_ignore_ascii_case(lit)) {
                self.pos += lit.len();
                return Ok(Tok::Error(e));
            }
        }
        self.err("onbekende foutwaarde")
    }

    fn word(&mut self) -> Tok {
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c.is_alphanumeric() || matches!(c, '_' | '.' | '$' | '\\') {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        let w = self.src[start..self.pos].to_string();
        if self.eat('!') { Tok::Sheet(w) } else { Tok::Word(w) }
    }
}

/// Parses formula text. A leading `=` is optional.
/// Longest formula accepted, the same limit Excel uses.
pub const MAX_FORMULA_LEN: usize = 8192;
/// Deepest nesting of parentheses, function calls and signs accepted. Keeps
/// parsing and evaluation recursion far away from the stack limit.
const MAX_DEPTH: usize = 128;

pub fn parse_formula(text: &str) -> Result<Expr, ParseError> {
    let body = text.strip_prefix('=').unwrap_or(text);
    if body.chars().count() > MAX_FORMULA_LEN {
        return Err(ParseError {
            message: format!("formule is langer dan {MAX_FORMULA_LEN} tekens"),
            pos: 0,
        });
    }
    let tokens = Lexer { src: body, pos: 0 }.tokens()?;
    let mut p = Parser { tokens, i: 0, depth: 0 };
    if p.peek() == &Tok::Eof {
        return p.err("lege formule");
    }
    let expr = p.expr(0)?;
    // The recursion guard in `expr` bounds the parser itself; postfix `%`
    // nests without recursing, so check the depth of the finished tree too.
    // Evaluation and printing recurse over it.
    if tree_depth(&expr) > MAX_DEPTH {
        return Err(ParseError { message: "formule is te diep genest".into(), pos: 0 });
    }
    match p.peek() {
        Tok::Eof => Ok(expr),
        Tok::RParen => p.err("haakje sluiten zonder openen"),
        _ => p.err("onverwachte invoer na formule"),
    }
}

/// Nesting depth of an expression, computed without recursion. A flat
/// operator chain counts as one level.
fn tree_depth(expr: &Expr) -> usize {
    let mut max = 0;
    let mut stack = vec![(expr, 1usize)];
    while let Some((e, depth)) = stack.pop() {
        max = max.max(depth);
        match e {
            Expr::Unary(_, inner) | Expr::Percent(inner) | Expr::Paren(inner) => {
                stack.push((inner, depth + 1))
            }
            Expr::Binary(first, rest) => {
                stack.push((first, depth + 1));
                stack.extend(rest.iter().map(|(_, e)| (e, depth + 1)));
            }
            Expr::Call(_, args) => stack.extend(args.iter().map(|a| (a, depth + 1))),
            _ => {}
        }
    }
    max
}

struct Parser {
    tokens: Vec<(Tok, usize)>,
    i: usize,
    depth: usize,
}

const PREFIX_BP: u8 = 11;
const PERCENT_BP: u8 = 13;

fn infix(op: &str) -> Option<(BinOp, u8, u8)> {
    // Excel precedence, all left-associative (Excel evaluates 2^3^2 as 64).
    Some(match op {
        "=" => (BinOp::Eq, 1, 2),
        "<>" => (BinOp::Ne, 1, 2),
        "<" => (BinOp::Lt, 1, 2),
        "<=" => (BinOp::Le, 1, 2),
        ">" => (BinOp::Gt, 1, 2),
        ">=" => (BinOp::Ge, 1, 2),
        "&" => (BinOp::Concat, 3, 4),
        "+" => (BinOp::Add, 5, 6),
        "-" => (BinOp::Sub, 5, 6),
        "*" => (BinOp::Mul, 7, 8),
        "/" => (BinOp::Div, 7, 8),
        "^" => (BinOp::Pow, 9, 10),
        _ => return None,
    })
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.tokens[self.i].0
    }

    fn next(&mut self) -> Tok {
        let t = self.tokens[self.i].0.clone();
        if self.i < self.tokens.len() - 1 {
            self.i += 1;
        }
        t
    }

    fn err<T>(&self, message: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError { message: message.into(), pos: self.tokens[self.i].1 })
    }

    fn expr(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        if self.depth >= MAX_DEPTH {
            return self.err("formule is te diep genest");
        }
        self.depth += 1;
        let result = self.expr_inner(min_bp);
        self.depth -= 1;
        result
    }

    fn expr_inner(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        let mut lhs = self.prefix()?;
        // Operators of this level accumulate into one flat chain.
        let mut chain: Vec<(BinOp, Expr)> = Vec::new();
        let close = |lhs: Expr, chain: &mut Vec<(BinOp, Expr)>| {
            if chain.is_empty() { lhs } else { Expr::Binary(Box::new(lhs), std::mem::take(chain)) }
        };
        loop {
            match *self.peek() {
                Tok::Op("%") => {
                    if PERCENT_BP < min_bp {
                        break;
                    }
                    self.next();
                    lhs = Expr::Percent(Box::new(close(lhs, &mut chain)));
                }
                Tok::Op(op) => {
                    let Some((bin, l_bp, r_bp)) = infix(op) else { break };
                    if l_bp < min_bp {
                        break;
                    }
                    self.next();
                    let rhs = self.expr(r_bp)?;
                    chain.push((bin, rhs));
                }
                _ => break,
            }
        }
        Ok(close(lhs, &mut chain))
    }

    fn prefix(&mut self) -> Result<Expr, ParseError> {
        match self.next() {
            Tok::Num(n) => {
                if self.peek() == &Tok::Colon && n.fract() == 0.0 {
                    let Some(start) = row_coord(&format!("{n}")) else {
                        return self.err("ongeldig rijnummer");
                    };
                    return self.row_range(None, start);
                }
                Ok(Expr::Number(n))
            }
            Tok::Str(s) => Ok(Expr::Text(s)),
            Tok::Error(e) => Ok(Expr::Error(e)),
            Tok::LParen => {
                let inner = self.expr(0)?;
                if self.next() != Tok::RParen {
                    self.i -= 1;
                    return self.err("verwacht ')'");
                }
                Ok(Expr::Paren(Box::new(inner)))
            }
            Tok::Op("-") => Ok(Expr::Unary(UnaryOp::Neg, Box::new(self.expr(PREFIX_BP)?))),
            Tok::Op("+") => Ok(Expr::Unary(UnaryOp::Plus, Box::new(self.expr(PREFIX_BP)?))),
            Tok::Sheet(name) => match self.next() {
                Tok::Word(w) => self.reference(Some(name), &w),
                Tok::Num(n) if n.fract() == 0.0 && self.peek() == &Tok::Colon => {
                    let Some(start) = row_coord(&format!("{n}")) else {
                        return self.err("ongeldig rijnummer");
                    };
                    self.row_range(Some(name), start)
                }
                _ => {
                    self.i -= 1;
                    self.err("verwacht een verwijzing na de bladnaam")
                }
            },
            Tok::Word(w) => {
                if self.peek() == &Tok::LParen {
                    self.next();
                    return self.call(&w);
                }
                if w.eq_ignore_ascii_case("TRUE") {
                    return Ok(Expr::Bool(true));
                }
                if w.eq_ignore_ascii_case("FALSE") {
                    return Ok(Expr::Bool(false));
                }
                self.reference(None, &w)
            }
            Tok::Eof => {
                self.i = self.tokens.len() - 1;
                self.err("formule is onvolledig")
            }
            _ => {
                self.i -= 1;
                self.err("onverwacht teken")
            }
        }
    }

    fn call(&mut self, raw: &str) -> Result<Expr, ParseError> {
        let mut name = raw.to_ascii_uppercase();
        for prefix in ["_XLFN.", "_XLWS."] {
            if let Some(rest) = name.strip_prefix(prefix) {
                name = rest.to_string();
            }
        }
        let mut args = Vec::new();
        if self.peek() == &Tok::RParen {
            self.next();
        } else {
            loop {
                args.push(self.expr(0)?);
                match self.next() {
                    Tok::Sep => continue,
                    Tok::RParen => break,
                    _ => {
                        self.i -= 1;
                        return self.err("verwacht ',' of ')' in functieaanroep");
                    }
                }
            }
        }
        if args.is_empty() && (name == "TRUE" || name == "FALSE") {
            return Ok(Expr::Bool(name == "TRUE"));
        }
        Ok(Expr::Call(name, args))
    }

    fn reference(&mut self, sheet: Option<String>, word: &str) -> Result<Expr, ParseError> {
        if let Some(start) = cell_addr(word) {
            if self.peek() == &Tok::Colon {
                self.next();
                let end = match self.next() {
                    Tok::Word(w) => cell_addr(&w),
                    _ => None,
                };
                let Some(end) = end else {
                    self.i -= 1;
                    return self.err("verwacht een celverwijzing na ':'");
                };
                return Ok(Expr::Range(RangeAddr { sheet, kind: RangeKind::Cells(start, end) }));
            }
            return Ok(Expr::Ref { sheet, addr: start });
        }
        if self.peek() == &Tok::Colon {
            if let Some(start) = col_coord(word) {
                self.next();
                let end = match self.next() {
                    Tok::Word(w) => col_coord(&w),
                    _ => None,
                };
                let Some(end) = end else {
                    self.i -= 1;
                    return self.err("verwacht een kolom na ':'");
                };
                return Ok(Expr::Range(RangeAddr { sheet, kind: RangeKind::Cols(start, end) }));
            }
            if let Some(start) = row_coord(word) {
                return self.row_range(sheet, start);
            }
        }
        if sheet.is_some() {
            self.i -= 1;
            return self.err("ongeldige verwijzing");
        }
        Ok(Expr::Name(word.to_string()))
    }

    /// Parses the `:end` of a row range whose start was already read.
    fn row_range(&mut self, sheet: Option<String>, start: Coord) -> Result<Expr, ParseError> {
        self.next(); // ':'
        let end = match self.next() {
            Tok::Num(n) if n.fract() == 0.0 => row_coord(&format!("{n}")),
            Tok::Word(w) => row_coord(&w),
            _ => None,
        };
        let Some(end) = end else {
            self.i -= 1;
            return self.err("verwacht een rijnummer na ':'");
        };
        Ok(Expr::Range(RangeAddr { sheet, kind: RangeKind::Rows(start, end) }))
    }
}

fn split_abs(s: &str) -> (bool, &str) {
    match s.strip_prefix('$') {
        Some(rest) => (true, rest),
        None => (false, s),
    }
}

fn col_coord(s: &str) -> Option<Coord> {
    let (abs, letters) = split_abs(s);
    Some(Coord { index: parse_col(letters)?, abs })
}

fn row_coord(s: &str) -> Option<Coord> {
    let (abs, digits) = split_abs(s);
    Some(Coord { index: parse_row(digits)?, abs })
}

fn cell_addr(s: &str) -> Option<CellAddr> {
    let (col_abs, rest) = split_abs(s);
    let letters_end = rest.find(|c: char| !c.is_ascii_alphabetic())?;
    let (letters, rest) = rest.split_at(letters_end);
    let col = parse_col(letters)?;
    let (row_abs, digits) = split_abs(rest);
    let row = parse_row(digits)?;
    Some(CellAddr { row: Coord { index: row, abs: row_abs }, col: Coord { index: col, abs: col_abs } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(src: &str) -> String {
        parse_formula(src).unwrap_or_else(|e| panic!("{src}: {e}")).to_string()
    }

    #[test]
    fn prints_back_canonical_text() {
        assert_eq!(round_trip("=a1+b2"), "A1+B2");
        assert_eq!(round_trip("= sum( A1 : B10 ; 3 )"), "SUM(A1:B10,3)");
        assert_eq!(round_trip("=$A$1*B$2-$C3"), "$A$1*B$2-$C3");
        assert_eq!(round_trip("=(1+2)*3"), "(1+2)*3");
        assert_eq!(round_trip("=\"say \"\"hi\"\"\"&A1"), "\"say \"\"hi\"\"\"&A1");
        assert_eq!(round_trip("=A:A"), "A:A");
        assert_eq!(round_trip("=SUM($2:$3)"), "SUM($2:$3)");
        assert_eq!(round_trip("=_xlfn.CONCAT(A1,\"x\")"), "CONCAT(A1,\"x\")");
        assert_eq!(round_trip("='My Sheet'!B2+Sheet2!A1:A3"), "'My Sheet'!B2+Sheet2!A1:A3");
        assert_eq!(round_trip("=50%"), "50%");
        assert_eq!(round_trip("=1.5e3"), "1500");
        assert_eq!(round_trip("=#div/0!"), "#DIV/0!");
        assert_eq!(round_trip("=TRUE()"), "TRUE");
    }

    #[test]
    fn precedence_matches_excel() {
        use Expr::*;
        // Higher-precedence operators nest inside the right operand.
        let e = parse_formula("=1+2*3^2").unwrap();
        let Binary(_, rest) = &e else { panic!("{e:?}") };
        assert_eq!(rest.len(), 1);
        let (BinOp::Add, Binary(_, mul)) = &rest[0] else { panic!("{e:?}") };
        let (BinOp::Mul, Binary(_, pow)) = &mul[0] else { panic!("{e:?}") };
        assert_eq!(pow[0].0, BinOp::Pow);

        // Unary minus binds tighter than ^ in Excel: -2^2 = 4.
        let e = parse_formula("=-2^2").unwrap();
        assert!(
            matches!(&e, Binary(first, rest) if matches!(**first, Unary(UnaryOp::Neg, _)) && rest[0].0 == BinOp::Pow)
        );

        // Same-level operators form one left-associative chain.
        let e = parse_formula("=2^3^2").unwrap();
        assert!(matches!(&e, Binary(_, rest) if rest.len() == 2));
        let e = parse_formula("=1-2+3").unwrap();
        assert!(matches!(&e, Binary(_, rest) if rest[0].0 == BinOp::Sub && rest[1].0 == BinOp::Add));

        // Comparison is the loosest operator: it never ends up inside an operand.
        let e = parse_formula("=A1&\"x\"=B1").unwrap();
        assert!(matches!(&e, Binary(_, rest) if rest[0].0 == BinOp::Concat && rest[1].0 == BinOp::Eq));
        let e = parse_formula("=1=2&3").unwrap();
        assert!(matches!(&e, Binary(_, rest) if rest.len() == 1 && rest[0].0 == BinOp::Eq));
    }

    #[test]
    fn names_and_refs_are_told_apart() {
        assert!(matches!(parse_formula("=FOO").unwrap(), Expr::Name(_)));
        assert!(matches!(parse_formula("=XFD1").unwrap(), Expr::Ref { .. }));
        // Beyond the last column it is a name, not a reference.
        assert!(matches!(parse_formula("=XFE1").unwrap(), Expr::Name(_)));
    }

    #[test]
    fn rejects_hostile_input_without_panicking() {
        assert!(parse_formula("=#é").is_err());
        assert!(parse_formula("=#DIV/é").is_err());
        assert!(parse_formula(&format!("={}1{}", "(".repeat(10_000), ")".repeat(10_000))).is_err());
        assert!(parse_formula(&format!("={}1", "-".repeat(10_000))).is_err());
        assert!(parse_formula(&format!("=1{}", "%".repeat(8_000))).is_err());
        assert!(parse_formula("=50%%").is_ok());
        // Percent signs spread over many nesting levels add up.
        let nested = "(".repeat(100) + "1" + &")%".repeat(100);
        assert!(parse_formula(&format!("={nested}")).is_err());
        let spread: String = (0..60).fold("1".to_string(), |acc, _| format!("({acc}{})", "%".repeat(60)));
        assert!(parse_formula(&format!("={spread}")).is_err());
        assert!(tree_depth(&parse_formula("=SUM(1,(2+3)%)").unwrap()) == 5);
        assert!(parse_formula(&format!("=SUM({}", "SUM(".repeat(5_000))).is_err());
        assert!(parse_formula(&format!("={}", "1+".repeat(5_000))).is_err());
        assert!(parse_formula(&format!("={}", "(1)+".repeat(100) + "1")).is_ok());
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in ["=", "=1+", "=(1+2", "=1+2)", "=SUM(1,", "=\"abc", "=A1:", "=1 2", "=@", "=Sheet1!"] {
            assert!(parse_formula(bad).is_err(), "{bad} should fail");
        }
    }
}
