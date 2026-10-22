//! Formula evaluation and the built-in function library.

use super::ast::{BinOp, Expr, RangeAddr, UnaryOp};
use crate::addr::{CellRef, Range};
use crate::value::{ErrorKind, Value};

/// Read access to cell values during evaluation.
pub trait CellSource {
    fn sheet_name(&self) -> &str;
    fn value(&self, at: CellRef) -> &Value;
    /// Calls `f` for every non-empty cell in `range`; stops early when it returns `false`.
    fn for_each_value(&self, range: Range, f: &mut dyn FnMut(&Value) -> bool);
}

/// Names of all functions the evaluator knows, for autocompletion and docs.
pub const FUNCTIONS: &[&str] = &[
    "ABS",
    "AND",
    "AVERAGE",
    "CONCAT",
    "CONCATENATE",
    "COUNT",
    "COUNTA",
    "IF",
    "IFERROR",
    "LEN",
    "LOWER",
    "MAX",
    "MIN",
    "NOT",
    "OR",
    "ROUND",
    "SUM",
    "UPPER",
];

pub fn evaluate(expr: &Expr, ctx: &dyn CellSource) -> Value {
    Eval { ctx }.scalar(expr)
}

/// A function argument before it is reduced to a single value.
enum Arg {
    Value(Value),
    Range(Range),
}

struct Eval<'a> {
    ctx: &'a dyn CellSource,
}

type Res<T> = Result<T, ErrorKind>;

impl Eval<'_> {
    fn local(&self, sheet: &Option<String>) -> bool {
        sheet.as_deref().is_none_or(|s| s.eq_ignore_ascii_case(self.ctx.sheet_name()))
    }

    fn range(&self, r: &RangeAddr) -> Res<Range> {
        if self.local(&r.sheet) { Ok(r.resolve()) } else { Err(ErrorKind::Ref) }
    }

    /// Evaluates keeping references as ranges, which aggregate functions need.
    fn arg(&self, expr: &Expr) -> Arg {
        match expr {
            Expr::Ref { sheet, addr } if self.local(sheet) => Arg::Range(Range::single(addr.cell())),
            Expr::Range(r) => match self.range(r) {
                Ok(range) => Arg::Range(range),
                Err(e) => Arg::Value(Value::Error(e)),
            },
            Expr::Paren(inner) => self.arg(inner),
            _ => Arg::Value(self.scalar(expr)),
        }
    }

    fn scalar(&self, expr: &Expr) -> Value {
        match expr {
            Expr::Number(n) => Value::Number(*n),
            Expr::Text(s) => Value::Text(s.clone()),
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Error(e) => Value::Error(*e),
            Expr::Ref { sheet, addr } => {
                if self.local(sheet) {
                    self.ctx.value(addr.cell()).clone()
                } else {
                    Value::Error(ErrorKind::Ref)
                }
            }
            Expr::Range(r) => match self.range(r) {
                // A one-cell range behaves as that cell; larger ranges need
                // implicit intersection, which is not supported.
                Ok(range) if range.start == range.end => self.ctx.value(range.start).clone(),
                Ok(_) => Value::Error(ErrorKind::Value),
                Err(e) => Value::Error(e),
            },
            Expr::Paren(e) => self.scalar(e),
            Expr::Unary(op, e) => match self.scalar(e).to_number() {
                Ok(n) => Value::number(if *op == UnaryOp::Neg { -n } else { n }),
                Err(err) => Value::Error(err),
            },
            Expr::Percent(e) => match self.scalar(e).to_number() {
                Ok(n) => Value::number(n / 100.0),
                Err(err) => Value::Error(err),
            },
            Expr::Binary(op, a, b) => self.binary(*op, a, b).unwrap_or_else(Value::Error),
            Expr::Call(name, args) => self.call(name, args).unwrap_or_else(Value::Error),
            Expr::Name(_) => Value::Error(ErrorKind::Name),
        }
    }

    fn binary(&self, op: BinOp, a: &Expr, b: &Expr) -> Res<Value> {
        let a = self.scalar(a);
        let b = self.scalar(b);
        if let Value::Error(e) = a {
            return Err(e);
        }
        if let Value::Error(e) = b {
            return Err(e);
        }
        let cmp = |pred: fn(std::cmp::Ordering) -> bool| Ok(Value::Bool(pred(a.compare(&b))));
        match op {
            BinOp::Add => Ok(Value::number(a.to_number()? + b.to_number()?)),
            BinOp::Sub => Ok(Value::number(a.to_number()? - b.to_number()?)),
            BinOp::Mul => Ok(Value::number(a.to_number()? * b.to_number()?)),
            BinOp::Div => {
                let (x, y) = (a.to_number()?, b.to_number()?);
                if y == 0.0 { Err(ErrorKind::Div0) } else { Ok(Value::number(x / y)) }
            }
            BinOp::Pow => {
                let (x, y) = (a.to_number()?, b.to_number()?);
                if x == 0.0 && y == 0.0 {
                    Err(ErrorKind::Num)
                } else if x == 0.0 && y < 0.0 {
                    Err(ErrorKind::Div0)
                } else {
                    Ok(Value::number(x.powf(y)))
                }
            }
            BinOp::Concat => Ok(Value::Text(a.to_text()? + &b.to_text()?)),
            BinOp::Eq => cmp(|o| o.is_eq()),
            BinOp::Ne => cmp(|o| o.is_ne()),
            BinOp::Lt => cmp(|o| o.is_lt()),
            BinOp::Le => cmp(|o| o.is_le()),
            BinOp::Gt => cmp(|o| o.is_gt()),
            BinOp::Ge => cmp(|o| o.is_ge()),
        }
    }

    fn number_arg(&self, expr: &Expr) -> Res<f64> {
        self.scalar(expr).to_number()
    }

    /// Feeds every number an aggregate like SUM sees. Direct arguments are
    /// coerced (so `SUM("2", TRUE)` is 3); inside ranges only real numbers
    /// count and text, booleans and blanks are skipped. Errors propagate.
    fn numbers(&self, args: &[Expr], f: &mut dyn FnMut(f64)) -> Res<()> {
        for a in args {
            match self.arg(a) {
                Arg::Value(v) => f(v.to_number()?),
                Arg::Range(r) => {
                    let mut err = None;
                    self.ctx.for_each_value(r, &mut |v| match v {
                        Value::Number(n) => {
                            f(*n);
                            true
                        }
                        Value::Error(e) => {
                            err = Some(*e);
                            false
                        }
                        _ => true,
                    });
                    if let Some(e) = err {
                        return Err(e);
                    }
                }
            }
        }
        Ok(())
    }

    /// Feeds every value, flattening ranges, for CONCAT, AND, OR and COUNTA.
    fn values(&self, args: &[Expr], f: &mut dyn FnMut(&Value) -> Res<()>) -> Res<()> {
        for a in args {
            match self.arg(a) {
                Arg::Value(v) => f(&v)?,
                Arg::Range(r) => {
                    let mut res = Ok(());
                    self.ctx.for_each_value(r, &mut |v| {
                        res = f(v);
                        res.is_ok()
                    });
                    res?;
                }
            }
        }
        Ok(())
    }

    fn call(&self, name: &str, args: &[Expr]) -> Res<Value> {
        let arity = |min: usize, max: usize| {
            if (min..=max).contains(&args.len()) { Ok(()) } else { Err(ErrorKind::Value) }
        };
        match name {
            "SUM" => {
                let mut sum = 0.0;
                self.numbers(args, &mut |n| sum += n)?;
                Ok(Value::number(sum))
            }
            "AVERAGE" => {
                arity(1, usize::MAX)?;
                let (mut sum, mut count) = (0.0, 0u64);
                self.numbers(args, &mut |n| {
                    sum += n;
                    count += 1;
                })?;
                if count == 0 { Err(ErrorKind::Div0) } else { Ok(Value::number(sum / count as f64)) }
            }
            "MIN" | "MAX" => {
                arity(1, usize::MAX)?;
                let is_min = name == "MIN";
                let mut best: Option<f64> = None;
                self.numbers(args, &mut |n| {
                    best = Some(match best {
                        None => n,
                        Some(b) if is_min => b.min(n),
                        Some(b) => b.max(n),
                    })
                })?;
                Ok(Value::number(best.unwrap_or(0.0)))
            }
            "COUNT" => {
                // COUNT never fails: errors and text are simply not counted.
                let mut count = 0u64;
                for a in args {
                    match self.arg(a) {
                        Arg::Value(v) => {
                            if matches!(v, Value::Number(_) | Value::Bool(_))
                                || matches!(&v, Value::Text(t) if crate::value::parse_number(t).is_some())
                            {
                                count += 1;
                            }
                        }
                        Arg::Range(r) => self.ctx.for_each_value(r, &mut |v| {
                            if matches!(v, Value::Number(_)) {
                                count += 1;
                            }
                            true
                        }),
                    }
                }
                Ok(Value::Number(count as f64))
            }
            "COUNTA" => {
                let mut count = 0u64;
                for a in args {
                    match self.arg(a) {
                        Arg::Value(v) => count += u64::from(!v.is_empty()),
                        Arg::Range(r) => self.ctx.for_each_value(r, &mut |v| {
                            count += u64::from(!v.is_empty());
                            true
                        }),
                    }
                }
                Ok(Value::Number(count as f64))
            }
            "IF" => {
                arity(2, 3)?;
                if self.scalar(&args[0]).to_bool()? {
                    Ok(self.scalar(&args[1]))
                } else {
                    Ok(args.get(2).map_or(Value::Bool(false), |e| self.scalar(e)))
                }
            }
            "IFERROR" => {
                arity(2, 2)?;
                match self.scalar(&args[0]) {
                    Value::Error(_) => Ok(self.scalar(&args[1])),
                    v => Ok(v),
                }
            }
            "ROUND" => {
                arity(2, 2)?;
                let x = self.number_arg(&args[0])?;
                let digits = self.number_arg(&args[1])?.trunc();
                Ok(Value::number(round_half_away(x, digits.clamp(-308.0, 308.0) as i32)))
            }
            "ABS" => {
                arity(1, 1)?;
                Ok(Value::number(self.number_arg(&args[0])?.abs()))
            }
            "CONCAT" | "CONCATENATE" => {
                arity(1, usize::MAX)?;
                let mut out = String::new();
                self.values(args, &mut |v| {
                    out.push_str(&v.to_text()?);
                    Ok(())
                })?;
                Ok(Value::Text(out))
            }
            "AND" | "OR" => {
                arity(1, usize::MAX)?;
                let is_and = name == "AND";
                let mut acc: Option<bool> = None;
                self.values(args, &mut |v| {
                    let b = match v {
                        // Text and blanks inside ranges are ignored.
                        Value::Empty | Value::Text(_) => return Ok(()),
                        other => other.to_bool()?,
                    };
                    acc = Some(match acc {
                        None => b,
                        Some(a) if is_and => a && b,
                        Some(a) => a || b,
                    });
                    Ok(())
                })?;
                acc.map(Value::Bool).ok_or(ErrorKind::Value)
            }
            "NOT" => {
                arity(1, 1)?;
                Ok(Value::Bool(!self.scalar(&args[0]).to_bool()?))
            }
            "LEN" => {
                arity(1, 1)?;
                Ok(Value::Number(self.scalar(&args[0]).to_text()?.chars().count() as f64))
            }
            "UPPER" => {
                arity(1, 1)?;
                Ok(Value::Text(self.scalar(&args[0]).to_text()?.to_uppercase()))
            }
            "LOWER" => {
                arity(1, 1)?;
                Ok(Value::Text(self.scalar(&args[0]).to_text()?.to_lowercase()))
            }
            _ => Err(ErrorKind::Name),
        }
    }
}

/// Rounds half away from zero like Excel, correcting binary representation
/// error first so ROUND(2.675, 2) gives 2.68 and not 2.67.
pub fn round_half_away(x: f64, digits: i32) -> f64 {
    let factor = 10f64.powi(digits.abs());
    let scaled = if digits >= 0 { x * factor } else { x / factor };
    if !scaled.is_finite() {
        return x;
    }
    // Round to 15 significant digits to drop representation noise.
    let cleaned: f64 = format!("{scaled:.14e}").parse().unwrap_or(scaled);
    let rounded = cleaned.round();
    if digits >= 0 { rounded / factor } else { rounded * factor }
}
