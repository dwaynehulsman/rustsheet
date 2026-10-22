//! Formula syntax tree, printing back to text, and reference shifting.

use std::fmt;

use crate::addr::{CellRef, MAX_COLS, MAX_ROWS, Range, col_name};
use crate::value::ErrorKind;

/// One axis of a reference: an index plus the `$` marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coord {
    pub index: u32,
    pub abs: bool,
}

impl Coord {
    pub const fn rel(index: u32) -> Self {
        Self { index, abs: false }
    }

    /// Moves a relative coordinate; absolute ones stay put. `None` when it
    /// would leave the grid, which callers turn into `#REF!`.
    fn shifted(self, delta: i64, limit: u32) -> Option<Self> {
        if self.abs || delta == 0 {
            return Some(self);
        }
        let n = i64::from(self.index) + delta;
        (0..i64::from(limit)).contains(&n).then_some(Coord { index: n as u32, abs: false })
    }
}

/// A single cell reference such as `B$3`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellAddr {
    pub row: Coord,
    pub col: Coord,
}

impl CellAddr {
    pub fn cell(&self) -> CellRef {
        CellRef::new(self.row.index, self.col.index)
    }

    fn shifted(self, drow: i64, dcol: i64) -> Option<Self> {
        Some(CellAddr { row: self.row.shifted(drow, MAX_ROWS)?, col: self.col.shifted(dcol, MAX_COLS)? })
    }
}

impl fmt::Display for CellAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_col(f, self.col)?;
        write_row(f, self.row)
    }
}

fn write_col(f: &mut fmt::Formatter<'_>, c: Coord) -> fmt::Result {
    write!(f, "{}{}", if c.abs { "$" } else { "" }, col_name(c.index))
}

fn write_row(f: &mut fmt::Formatter<'_>, r: Coord) -> fmt::Result {
    write!(f, "{}{}", if r.abs { "$" } else { "" }, r.index + 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeKind {
    /// `A1:B10`
    Cells(CellAddr, CellAddr),
    /// `A:C`
    Cols(Coord, Coord),
    /// `3:5`
    Rows(Coord, Coord),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RangeAddr {
    pub sheet: Option<String>,
    pub kind: RangeKind,
}

impl RangeAddr {
    /// The rectangle this reference covers on the grid.
    pub fn resolve(&self) -> Range {
        match self.kind {
            RangeKind::Cells(a, b) => Range::new(a.cell(), b.cell()),
            RangeKind::Cols(a, b) => {
                Range::new(CellRef::new(0, a.index), CellRef::new(MAX_ROWS - 1, b.index))
            }
            RangeKind::Rows(a, b) => {
                Range::new(CellRef::new(a.index, 0), CellRef::new(b.index, MAX_COLS - 1))
            }
        }
    }

    fn shifted(&self, drow: i64, dcol: i64) -> Option<RangeAddr> {
        let kind = match self.kind {
            RangeKind::Cells(a, b) => RangeKind::Cells(a.shifted(drow, dcol)?, b.shifted(drow, dcol)?),
            RangeKind::Cols(a, b) => RangeKind::Cols(a.shifted(dcol, MAX_COLS)?, b.shifted(dcol, MAX_COLS)?),
            RangeKind::Rows(a, b) => RangeKind::Rows(a.shifted(drow, MAX_ROWS)?, b.shifted(drow, MAX_ROWS)?),
        };
        Some(RangeAddr { sheet: self.sheet.clone(), kind })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Plus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Pow => "^",
            BinOp::Concat => "&",
            BinOp::Eq => "=",
            BinOp::Ne => "<>",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Number(f64),
    Text(String),
    Bool(bool),
    Error(ErrorKind),
    Ref {
        sheet: Option<String>,
        addr: CellAddr,
    },
    Range(RangeAddr),
    Unary(UnaryOp, Box<Expr>),
    Percent(Box<Expr>),
    /// A left-associative run of binary operators: `first op1 r1 op2 r2 ...`
    /// means `((first op1 r1) op2 r2)`. Kept flat so long chains such as
    /// `A1+A2+...+A4000` never make the tree (and recursion over it) deep.
    Binary(Box<Expr>, Vec<(BinOp, Expr)>),
    /// Kept so a formula prints back exactly as it was grouped.
    Paren(Box<Expr>),
    /// Function call; the name is upper case without `_xlfn.` prefixes.
    Call(String, Vec<Expr>),
    /// An identifier that is not a function, reference or boolean.
    Name(String),
}

/// A cell or range a formula reads, used by the dependency graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Precedent {
    Cell(CellRef),
    Range(Range),
}

impl Expr {
    /// Copy of this expression as if the formula moved by (`drow`, `dcol`),
    /// the way Excel adjusts relative references when pasting.
    pub fn shifted(&self, drow: i64, dcol: i64) -> Expr {
        match self {
            Expr::Ref { sheet, addr } => match addr.shifted(drow, dcol) {
                Some(addr) => Expr::Ref { sheet: sheet.clone(), addr },
                None => Expr::Error(ErrorKind::Ref),
            },
            Expr::Range(r) => r.shifted(drow, dcol).map_or(Expr::Error(ErrorKind::Ref), Expr::Range),
            Expr::Unary(op, e) => Expr::Unary(*op, Box::new(e.shifted(drow, dcol))),
            Expr::Percent(e) => Expr::Percent(Box::new(e.shifted(drow, dcol))),
            Expr::Binary(first, rest) => Expr::Binary(
                Box::new(first.shifted(drow, dcol)),
                rest.iter().map(|(op, e)| (*op, e.shifted(drow, dcol))).collect(),
            ),
            Expr::Paren(e) => Expr::Paren(Box::new(e.shifted(drow, dcol))),
            Expr::Call(name, args) => {
                Expr::Call(name.clone(), args.iter().map(|a| a.shifted(drow, dcol)).collect())
            }
            Expr::Number(_) | Expr::Text(_) | Expr::Bool(_) | Expr::Error(_) | Expr::Name(_) => self.clone(),
        }
    }

    /// Collects every cell and range on `own_sheet` (or unqualified) that
    /// this expression reads.
    pub fn precedents(&self, own_sheet: &str, out: &mut Vec<Precedent>) {
        let local =
            |sheet: &Option<String>| sheet.as_deref().is_none_or(|s| s.eq_ignore_ascii_case(own_sheet));
        match self {
            Expr::Ref { sheet, addr } if local(sheet) => out.push(Precedent::Cell(addr.cell())),
            Expr::Range(r) if local(&r.sheet) => {
                let range = r.resolve();
                out.push(if range.start == range.end {
                    Precedent::Cell(range.start)
                } else {
                    Precedent::Range(range)
                });
            }
            Expr::Unary(_, e) | Expr::Percent(e) | Expr::Paren(e) => e.precedents(own_sheet, out),
            Expr::Binary(first, rest) => {
                first.precedents(own_sheet, out);
                rest.iter().for_each(|(_, e)| e.precedents(own_sheet, out));
            }
            Expr::Call(_, args) => args.iter().for_each(|a| a.precedents(own_sheet, out)),
            _ => {}
        }
    }
}

fn write_sheet(f: &mut fmt::Formatter<'_>, sheet: &Option<String>) -> fmt::Result {
    let Some(name) = sheet else { return Ok(()) };
    let plain = name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        && !looks_like_cell(name);
    if plain { write!(f, "{name}!") } else { write!(f, "'{}'!", name.replace('\'', "''")) }
}

fn looks_like_cell(s: &str) -> bool {
    CellRef::parse(s).is_some() || s.eq_ignore_ascii_case("TRUE") || s.eq_ignore_ascii_case("FALSE")
}

fn write_number(f: &mut fmt::Formatter<'_>, n: f64) -> fmt::Result {
    // Rust prints the shortest text that parses back to the same float.
    let s = n.to_string();
    if s.len() > 16 && !s.contains('.') { write!(f, "{n:E}") } else { f.write_str(&s) }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Number(n) => write_number(f, *n),
            Expr::Text(s) => write!(f, "\"{}\"", s.replace('"', "\"\"")),
            Expr::Bool(b) => f.write_str(if *b { "TRUE" } else { "FALSE" }),
            Expr::Error(e) => f.write_str(e.as_str()),
            Expr::Ref { sheet, addr } => {
                write_sheet(f, sheet)?;
                write!(f, "{addr}")
            }
            Expr::Range(r) => {
                write_sheet(f, &r.sheet)?;
                match r.kind {
                    RangeKind::Cells(a, b) => write!(f, "{a}:{b}"),
                    RangeKind::Cols(a, b) => {
                        write_col(f, a)?;
                        f.write_str(":")?;
                        write_col(f, b)
                    }
                    RangeKind::Rows(a, b) => {
                        write_row(f, a)?;
                        f.write_str(":")?;
                        write_row(f, b)
                    }
                }
            }
            Expr::Unary(UnaryOp::Neg, e) => write!(f, "-{e}"),
            Expr::Unary(UnaryOp::Plus, e) => write!(f, "+{e}"),
            Expr::Percent(e) => write!(f, "{e}%"),
            Expr::Binary(first, rest) => {
                write!(f, "{first}")?;
                for (op, rhs) in rest {
                    write!(f, "{}{rhs}", op.symbol())?;
                }
                Ok(())
            }
            Expr::Paren(e) => write!(f, "({e})"),
            Expr::Call(name, args) => {
                write!(f, "{name}(")?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{a}")?;
                }
                f.write_str(")")
            }
            Expr::Name(n) => f.write_str(n),
        }
    }
}
