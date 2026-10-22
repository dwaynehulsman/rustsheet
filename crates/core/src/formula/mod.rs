//! Formulas: parsing, printing, reference shifting and evaluation.

mod ast;
mod eval;
mod parse;

pub use ast::{
    BinOp, CellAddr, Coord, Expr, LineChange, Precedent, RangeAddr, RangeKind, UnaryOp, same_sheet_name,
    sheet_prefix,
};
pub use eval::{CellSource, FUNCTIONS, evaluate, round_half_away};
pub use parse::{ParseError, parse_formula};
