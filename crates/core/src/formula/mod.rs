//! Formulas: parsing, printing, reference shifting and evaluation.

mod ast;
mod eval;
mod parse;

pub use ast::{BinOp, CellAddr, Coord, Expr, Precedent, RangeAddr, RangeKind, UnaryOp};
pub use eval::{CellSource, FUNCTIONS, evaluate, round_half_away};
pub use parse::{ParseError, parse_formula};
