//! The larger part of the function library: lookups, conditional
//! aggregates, text, dates and extra maths.

use std::cmp::Ordering;

use super::{Arg, Eval, Res};
use crate::addr::{CellRef, Range};
use crate::format::{date_to_serial, format_with_code, serial_to_date, weekday};
use crate::formula::Expr;
use crate::input::{Content, parse_literal};
use crate::sheet::SheetId;
use crate::value::{ErrorKind, Value, parse_number};

/// Ranges an aggregate may walk densely (cell by cell, empty ones too).
const MAX_DENSE: u64 = 4_000_000;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// A COUNTIF-style condition such as `5`, `">=10"`, `"<>"` or `"app*"`.
#[derive(Clone, Debug)]
struct Criterion {
    op: Op,
    value: Value,
}

impl Criterion {
    fn new(v: &Value) -> Criterion {
        let Value::Text(text) = v else {
            return Criterion { op: Op::Eq, value: v.clone() };
        };
        let (op, rest) =
            [("<=", Op::Le), (">=", Op::Ge), ("<>", Op::Ne), ("<", Op::Lt), (">", Op::Gt), ("=", Op::Eq)]
                .into_iter()
                .find_map(|(p, op)| text.strip_prefix(p).map(|r| (op, r)))
                .unwrap_or((Op::Eq, text.as_str()));
        let value = if rest.is_empty() {
            Value::Empty
        } else if let Some(n) = parse_number(rest) {
            Value::Number(n)
        } else if rest.eq_ignore_ascii_case("TRUE") {
            Value::Bool(true)
        } else if rest.eq_ignore_ascii_case("FALSE") {
            Value::Bool(false)
        } else {
            Value::Text(rest.to_string())
        };
        Criterion { op, value }
    }

    fn equal(&self, cell: &Value) -> bool {
        match (&self.value, cell) {
            (Value::Empty, Value::Empty) => true,
            (Value::Empty, Value::Text(t)) => t.is_empty(),
            (Value::Empty, _) => false,
            (Value::Number(n), Value::Number(x)) => n == x,
            (Value::Number(n), Value::Text(t)) => parse_number(t) == Some(*n),
            (Value::Text(p), Value::Text(t)) => wildcard(p, t),
            (Value::Bool(b), Value::Bool(x)) => b == x,
            (Value::Error(e), Value::Error(x)) => e == x,
            _ => false,
        }
    }

    fn matches(&self, cell: &Value) -> bool {
        match self.op {
            Op::Eq => self.equal(cell),
            Op::Ne => !self.equal(cell),
            op => {
                let ord = match (&self.value, cell) {
                    (Value::Number(n), Value::Number(x)) => x.total_cmp(n),
                    (Value::Text(p), Value::Text(t)) => t.to_lowercase().cmp(&p.to_lowercase()),
                    _ => return false,
                };
                match op {
                    Op::Lt => ord.is_lt(),
                    Op::Le => ord.is_le(),
                    Op::Gt => ord.is_gt(),
                    _ => ord.is_ge(),
                }
            }
        }
    }
}

/// Case-insensitive match with Excel wildcards: `*` any run, `?` one
/// character, `~` escapes the next character.
pub(crate) fn wildcard(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    if !p.iter().any(|c| matches!(c, '*' | '?' | '~')) {
        return p == t;
    }
    // Tokens: Some(c) literal, None for `?`, and star positions.
    let mut toks: Vec<(bool, Option<char>)> = Vec::new(); // (is_star, char)
    let mut i = 0;
    while i < p.len() {
        match p[i] {
            '~' if i + 1 < p.len() => {
                toks.push((false, Some(p[i + 1])));
                i += 2;
                continue;
            }
            '*' => toks.push((true, None)),
            '?' => toks.push((false, None)),
            c => toks.push((false, Some(c))),
        }
        i += 1;
    }
    // Greedy matching with backtracking to the last star.
    let (mut ti, mut pi) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < toks.len() && !toks[pi].0 && toks[pi].1.is_none_or(|c| c == t[ti]) {
            ti += 1;
            pi += 1;
        } else if pi < toks.len() && toks[pi].0 {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    toks[pi..].iter().all(|t| t.0)
}

/// Argument `i` unless it is absent or left empty.
fn present(args: &[Expr], i: usize) -> Option<&Expr> {
    args.get(i).filter(|e| !matches!(e, Expr::Missing))
}

/// Lookup equality: numbers by value, text case-insensitively (with
/// wildcards when `wild`), booleans by value.
fn lookup_eq(want: &Value, have: &Value, wild: bool) -> bool {
    match (want, have) {
        (Value::Text(w), Value::Text(h)) => {
            if wild {
                wildcard(w, h)
            } else {
                w.to_lowercase() == h.to_lowercase()
            }
        }
        (Value::Number(a), Value::Number(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        _ => false,
    }
}

fn same_kind(a: &Value, b: &Value) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// The current local date and time as an Excel serial number.
pub fn now_serial() -> f64 {
    let secs =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let offset = local_offset_secs(secs);
    let local = secs + offset;
    // 25569 is 1970-01-01 in the 1900 date system.
    25_569.0 + local as f64 / 86_400.0
}

#[cfg(unix)]
fn local_offset_secs(secs: i64) -> i64 {
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: localtime_r writes into the tm we own; t is a valid time_t.
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    if ok { tm.tm_gmtoff as i64 } else { 0 }
}

#[cfg(not(unix))]
fn local_offset_secs(_secs: i64) -> i64 {
    0
}

/// Excel's DATE: months and days outside their range roll over, years
/// below 1900 count from 1900.
pub(crate) fn date(year: f64, month: f64, day: f64) -> Res<f64> {
    let mut y = year.trunc() as i64;
    if (0..1900).contains(&y) {
        y += 1900;
    }
    let total = y * 12 + (month.trunc() as i64 - 1);
    let (y, m) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    let first = date_to_serial(y, m, 1).ok_or(ErrorKind::Num)?;
    let serial = first + day.trunc() - 1.0;
    if (0.0..2_958_466.0).contains(&serial) { Ok(serial) } else { Err(ErrorKind::Num) }
}

impl Eval<'_> {
    fn range_arg(&self, expr: &Expr) -> Res<(SheetId, Range)> {
        match self.arg(expr) {
            Arg::Range(s, r) => Ok((s, r)),
            Arg::Value(Value::Error(e)) => Err(e),
            Arg::Value(_) => Err(ErrorKind::Value),
        }
    }

    fn at(&self, sheet: SheetId, r: Range, drow: u32, dcol: u32) -> Value {
        self.ctx.value(sheet, CellRef::new(r.start.row + drow, r.start.col + dcol)).clone()
    }

    fn text_arg(&self, expr: &Expr) -> Res<String> {
        self.scalar(expr).to_text()
    }

    fn opt_number(&self, args: &[Expr], i: usize, default: f64) -> Res<f64> {
        present(args, i).map_or(Ok(default), |e| self.number_arg(e))
    }

    /// Offsets (row, col) within `shape` where every (range, criterion)
    /// pair matches; ranges must all have the shape of the first.
    fn matching_offsets(&self, pairs: &[(SheetId, Range, Criterion)]) -> Res<Vec<(u32, u32)>> {
        let Some(&(sheet0, shape, ref c0)) = pairs.first() else { return Ok(Vec::new()) };
        if pairs.iter().any(|(_, r, _)| r.rows() != shape.rows() || r.cols() != shape.cols()) {
            return Err(ErrorKind::Value);
        }
        let check_rest =
            |dr: u32, dc: u32| pairs[1..].iter().all(|(s, r, c)| c.matches(&self.at(*s, *r, dr, dc)));
        let mut out = Vec::new();
        if pairs.iter().any(|(_, _, c)| c.matches(&Value::Empty)) {
            // Empty cells can match, so every position counts.
            if shape.len() > MAX_DENSE {
                return Err(ErrorKind::Value);
            }
            for dr in 0..shape.rows() {
                for dc in 0..shape.cols() {
                    if c0.matches(&self.at(sheet0, shape, dr, dc)) && check_rest(dr, dc) {
                        out.push((dr, dc));
                    }
                }
            }
        } else {
            // Only filled cells can match: walk those of the first range.
            self.ctx.for_each_cell(sheet0, shape, &mut |at, v| {
                let (dr, dc) = (at.row - shape.start.row, at.col - shape.start.col);
                if c0.matches(v) && check_rest(dr, dc) {
                    out.push((dr, dc));
                }
                true
            });
        }
        Ok(out)
    }

    /// Parses `range, criterion, range, criterion, ...` from `args`.
    fn criteria_pairs(&self, args: &[Expr]) -> Res<Vec<(SheetId, Range, Criterion)>> {
        if args.is_empty() || !args.len().is_multiple_of(2) {
            return Err(ErrorKind::Value);
        }
        args.chunks(2)
            .map(|pair| {
                let (s, r) = self.range_arg(&pair[0])?;
                let crit = self.scalar(&pair[1]);
                if let Value::Error(e) = crit {
                    return Err(e);
                }
                Ok((s, r, Criterion::new(&crit)))
            })
            .collect()
    }

    /// Numbers at `offsets` within `target`.
    fn numbers_at(&self, target: (SheetId, Range), offsets: &[(u32, u32)]) -> Res<Vec<f64>> {
        let (s, r) = target;
        let mut out = Vec::new();
        for &(dr, dc) in offsets {
            if dr >= r.rows() && r.rows() > 1 || dc >= r.cols() && r.cols() > 1 {
                continue;
            }
            match self.at(s, r, dr, dc) {
                Value::Number(n) => out.push(n),
                Value::Error(e) => return Err(e),
                _ => {}
            }
        }
        Ok(out)
    }

    /// Position (0-based) of `want` along a one-row or one-column range.
    fn find_in_line(&self, want: &Value, sheet: SheetId, line: Range, mode: i32, last: bool) -> Option<u32> {
        let vertical = line.cols() == 1;
        let index = |at: CellRef| if vertical { at.row - line.start.row } else { at.col - line.start.col };
        let wild = mode == 2 || (mode == 0 && matches!(want, Value::Text(t) if t.contains(['*', '?', '~'])));
        let mut exact: Option<u32> = None;
        // Best "next smaller" / "next larger" so far: (position, value).
        let mut near: Option<(u32, Value)> = None;
        self.ctx.for_each_cell(sheet, line, &mut |at, v| {
            let i = index(at);
            if lookup_eq(want, v, wild) {
                exact = Some(i);
                return last;
            }
            if (mode == -1 || mode == 1) && same_kind(want, v) {
                let ord = v.compare(want);
                let better = |old: &Value| {
                    let c = v.compare(old);
                    if mode == -1 { c == Ordering::Greater } else { c == Ordering::Less }
                };
                let candidate =
                    (mode == -1 && ord == Ordering::Less) || (mode == 1 && ord == Ordering::Greater);
                if candidate && near.as_ref().is_none_or(|(_, old)| better(old)) {
                    near = Some((i, v.clone()));
                }
            }
            true
        });
        exact.or(near.map(|(i, _)| i))
    }

    /// VLOOKUP/HLOOKUP/MATCH approximate mode on data sorted ascending:
    /// the last position whose value is at most `want`.
    fn sorted_position(&self, want: &Value, sheet: SheetId, line: Range) -> Option<u32> {
        let vertical = line.cols() == 1;
        let mut best = None;
        self.ctx.for_each_cell(sheet, line, &mut |at, v| {
            if !same_kind(want, v) {
                return true;
            }
            if v.compare(want) == Ordering::Greater {
                return false;
            }
            best = Some(if vertical { at.row - line.start.row } else { at.col - line.start.col });
            true
        });
        best
    }

    fn lookup(&self, args: &[Expr], vertical: bool) -> Res<Value> {
        if !(3..=4).contains(&args.len()) {
            return Err(ErrorKind::Value);
        }
        let want = self.scalar(&args[0]);
        if let Value::Error(e) = want {
            return Err(e);
        }
        let (sheet, table) = self.range_arg(&args[1])?;
        let index = self.number_arg(&args[2])?.trunc();
        let approximate = match present(args, 3) {
            Some(e) => self.scalar(e).to_bool()?,
            None => true,
        };
        let span = if vertical { table.cols() } else { table.rows() };
        if index < 1.0 {
            return Err(ErrorKind::Value);
        }
        if index > f64::from(span) {
            return Err(ErrorKind::Ref);
        }
        let line = if vertical {
            Range::new(table.start, CellRef::new(table.end.row, table.start.col))
        } else {
            Range::new(table.start, CellRef::new(table.start.row, table.end.col))
        };
        let pos = if approximate {
            self.sorted_position(&want, sheet, line)
        } else {
            self.find_in_line(&want, sheet, line, 0, false)
        };
        let pos = pos.ok_or(ErrorKind::NA)?;
        let k = index as u32 - 1;
        Ok(if vertical { self.at(sheet, table, pos, k) } else { self.at(sheet, table, k, pos) })
    }

    pub(super) fn call_more(&self, name: &str, args: &[Expr]) -> Res<Value> {
        let arity = |min: usize, max: usize| {
            if (min..=max).contains(&args.len()) { Ok(()) } else { Err(ErrorKind::Value) }
        };
        match name {
            // ----- lookups -----
            "VLOOKUP" => self.lookup(args, true),
            "HLOOKUP" => self.lookup(args, false),
            "MATCH" => {
                arity(2, 3)?;
                let want = self.scalar(&args[0]);
                if let Value::Error(e) = want {
                    return Err(e);
                }
                let (sheet, line) = self.range_arg(&args[1])?;
                if line.rows() > 1 && line.cols() > 1 {
                    return Err(ErrorKind::NA);
                }
                let kind = self.opt_number(args, 2, 1.0)?;
                let pos = if kind == 0.0 {
                    self.find_in_line(&want, sheet, line, 0, false)
                } else if kind > 0.0 {
                    self.sorted_position(&want, sheet, line)
                } else {
                    // Sorted descending: the last value that is still >= want.
                    let mut best = None;
                    let vertical = line.cols() == 1;
                    self.ctx.for_each_cell(sheet, line, &mut |at, v| {
                        if !same_kind(&want, v) {
                            return true;
                        }
                        if v.compare(&want) == Ordering::Less {
                            return false;
                        }
                        best = Some(if vertical { at.row - line.start.row } else { at.col - line.start.col });
                        true
                    });
                    best
                };
                pos.map(|p| Value::Number(f64::from(p + 1))).ok_or(ErrorKind::NA)
            }
            "INDEX" => {
                arity(2, 3)?;
                let (sheet, r) = self.range_arg(&args[0])?;
                let a = self.number_arg(&args[1])?.trunc();
                let (row, col) = match present(args, 2) {
                    Some(e) => (a, self.number_arg(e)?.trunc()),
                    None if r.rows() == 1 => (1.0, a),
                    None => (a, 1.0),
                };
                if row < 1.0 || col < 1.0 {
                    return Err(ErrorKind::Value);
                }
                if row > f64::from(r.rows()) || col > f64::from(r.cols()) {
                    return Err(ErrorKind::Ref);
                }
                Ok(self.at(sheet, r, row as u32 - 1, col as u32 - 1))
            }
            "XLOOKUP" => {
                arity(3, 6)?;
                let want = self.scalar(&args[0]);
                if let Value::Error(e) = want {
                    return Err(e);
                }
                let (ls, line) = self.range_arg(&args[1])?;
                let (rs, ret) = self.range_arg(&args[2])?;
                if line.rows() > 1 && line.cols() > 1 {
                    return Err(ErrorKind::Value);
                }
                let vertical = line.cols() == 1;
                let len = if vertical { line.rows() } else { line.cols() };
                if (if vertical { ret.rows() } else { ret.cols() }) != len {
                    return Err(ErrorKind::Value);
                }
                let mode = self.opt_number(args, 4, 0.0)? as i32;
                let last = self.opt_number(args, 5, 1.0)? < 0.0;
                match self.find_in_line(&want, ls, line, mode, last) {
                    Some(p) => Ok(if vertical { self.at(rs, ret, p, 0) } else { self.at(rs, ret, 0, p) }),
                    None => match present(args, 3) {
                        Some(e) => Ok(self.scalar(e)),
                        None => Err(ErrorKind::NA),
                    },
                }
            }

            // ----- conditional aggregates -----
            "COUNTIF" | "COUNTIFS" => {
                if name == "COUNTIF" {
                    arity(2, 2)?;
                }
                Ok(Value::Number(self.matching_offsets(&self.criteria_pairs(args)?)?.len() as f64))
            }
            "SUMIF" | "AVERAGEIF" => {
                arity(2, 3)?;
                let pairs = self.criteria_pairs(&args[..2])?;
                let target = match present(args, 2) {
                    Some(e) => self.range_arg(e)?,
                    None => (pairs[0].0, pairs[0].1),
                };
                // Like Excel, the target takes the shape of the criteria range.
                let shape = pairs[0].1;
                let target = (
                    target.0,
                    Range::new(
                        target.1.start,
                        target
                            .1
                            .start
                            .offset_clamped(i64::from(shape.rows()) - 1, i64::from(shape.cols()) - 1),
                    ),
                );
                let nums = self.numbers_at(target, &self.matching_offsets(&pairs)?)?;
                self.sum_or_average(nums, name == "AVERAGEIF")
            }
            "SUMIFS" | "AVERAGEIFS" | "MAXIFS" | "MINIFS" => {
                if args.len() < 3 {
                    return Err(ErrorKind::Value);
                }
                let target = self.range_arg(&args[0])?;
                let pairs = self.criteria_pairs(&args[1..])?;
                if target.1.rows() != pairs[0].1.rows() || target.1.cols() != pairs[0].1.cols() {
                    return Err(ErrorKind::Value);
                }
                let nums = self.numbers_at(target, &self.matching_offsets(&pairs)?)?;
                match name {
                    "MAXIFS" => Ok(Value::number(if nums.is_empty() {
                        0.0
                    } else {
                        nums.iter().copied().fold(f64::NEG_INFINITY, f64::max)
                    })),
                    "MINIFS" => Ok(Value::number(if nums.is_empty() {
                        0.0
                    } else {
                        nums.iter().copied().fold(f64::INFINITY, f64::min)
                    })),
                    _ => self.sum_or_average(nums, name == "AVERAGEIFS"),
                }
            }
            "COUNTBLANK" => {
                arity(1, 1)?;
                let (s, r) = self.range_arg(&args[0])?;
                let mut filled = 0u64;
                self.ctx.for_each_cell(s, r, &mut |_, v| {
                    if !matches!(v, Value::Text(t) if t.is_empty()) {
                        filled += 1;
                    }
                    true
                });
                Ok(Value::Number((r.len() - filled) as f64))
            }
            "SUMPRODUCT" => {
                if args.is_empty() {
                    return Err(ErrorKind::Value);
                }
                let ranges: Vec<(SheetId, Range)> =
                    args.iter().map(|a| self.range_arg(a)).collect::<Res<_>>()?;
                let shape = ranges[0].1;
                if ranges.iter().any(|(_, r)| r.rows() != shape.rows() || r.cols() != shape.cols()) {
                    return Err(ErrorKind::Value);
                }
                let mut sum = 0.0;
                let mut err = None;
                // A product is zero unless the first range has a number there.
                self.ctx.for_each_cell(ranges[0].0, shape, &mut |at, _| {
                    let (dr, dc) = (at.row - shape.start.row, at.col - shape.start.col);
                    let mut product = 1.0;
                    for &(s, r) in &ranges {
                        match self.at(s, r, dr, dc) {
                            Value::Number(n) => product *= n,
                            Value::Error(e) => {
                                err = Some(e);
                                return false;
                            }
                            _ => product = 0.0,
                        }
                    }
                    sum += product;
                    true
                });
                match err {
                    Some(e) => Err(e),
                    None => Ok(Value::number(sum)),
                }
            }
            "MEDIAN" => {
                let mut nums = Vec::new();
                self.numbers(args, &mut |n| nums.push(n))?;
                if nums.is_empty() {
                    return Err(ErrorKind::Num);
                }
                nums.sort_by(f64::total_cmp);
                let mid = nums.len() / 2;
                Ok(Value::number(if nums.len() % 2 == 1 {
                    nums[mid]
                } else {
                    (nums[mid - 1] + nums[mid]) / 2.0
                }))
            }

            // ----- logic and information -----
            "IFS" => {
                if args.is_empty() || !args.len().is_multiple_of(2) {
                    return Err(ErrorKind::Value);
                }
                for pair in args.chunks(2) {
                    if self.scalar(&pair[0]).to_bool()? {
                        return Ok(self.scalar(&pair[1]));
                    }
                }
                Err(ErrorKind::NA)
            }
            "IFNA" => {
                arity(2, 2)?;
                match self.scalar(&args[0]) {
                    Value::Error(ErrorKind::NA) => Ok(self.scalar(&args[1])),
                    v => Ok(v),
                }
            }
            "NA" => {
                arity(0, 0)?;
                Err(ErrorKind::NA)
            }
            "ISBLANK" | "ISNUMBER" | "ISTEXT" | "ISERROR" | "ISNA" | "ISLOGICAL" => {
                arity(1, 1)?;
                let v = self.scalar(&args[0]);
                Ok(Value::Bool(match name {
                    "ISBLANK" => v.is_empty(),
                    "ISNUMBER" => matches!(v, Value::Number(_)),
                    "ISTEXT" => matches!(v, Value::Text(_)),
                    "ISERROR" => matches!(v, Value::Error(_)),
                    "ISNA" => v == Value::Error(ErrorKind::NA),
                    _ => matches!(v, Value::Bool(_)),
                }))
            }

            // ----- maths -----
            "ROUNDUP" | "ROUNDDOWN" => {
                arity(2, 2)?;
                let x = self.number_arg(&args[0])?;
                let d = self.number_arg(&args[1])?.trunc().clamp(-308.0, 308.0) as i32;
                let factor = 10f64.powi(d.abs());
                let scaled = if d >= 0 { x * factor } else { x / factor };
                // Clean representation noise first so ROUNDUP(0.1+0.2,1) is 0.3.
                let scaled: f64 = format!("{scaled:.14e}").parse().unwrap_or(scaled);
                let r = if name == "ROUNDUP" { scaled.abs().ceil() } else { scaled.abs().floor() }
                    * scaled.signum();
                Ok(Value::number(if d >= 0 { r / factor } else { r * factor }))
            }
            "INT" => {
                arity(1, 1)?;
                Ok(Value::number(self.number_arg(&args[0])?.floor()))
            }
            "MOD" => {
                arity(2, 2)?;
                let (a, b) = (self.number_arg(&args[0])?, self.number_arg(&args[1])?);
                if b == 0.0 {
                    return Err(ErrorKind::Div0);
                }
                Ok(Value::number(a - b * (a / b).floor()))
            }
            "POWER" => {
                arity(2, 2)?;
                let (a, b) = (self.number_arg(&args[0])?, self.number_arg(&args[1])?);
                if a == 0.0 && b <= 0.0 {
                    return Err(if b == 0.0 { ErrorKind::Num } else { ErrorKind::Div0 });
                }
                Ok(Value::number(a.powf(b)))
            }
            "SQRT" => {
                arity(1, 1)?;
                let x = self.number_arg(&args[0])?;
                if x < 0.0 { Err(ErrorKind::Num) } else { Ok(Value::number(x.sqrt())) }
            }

            // ----- text -----
            "TEXT" => {
                arity(2, 2)?;
                let v = self.scalar(&args[0]);
                let code = self.text_arg(&args[1])?;
                match v {
                    Value::Error(e) => Err(e),
                    Value::Number(n) => Ok(Value::Text(format_with_code(n, &code))),
                    Value::Empty => Ok(Value::Text(format_with_code(0.0, &code))),
                    Value::Text(t) => match parse_number(&t) {
                        Some(n) => Ok(Value::Text(format_with_code(n, &code))),
                        None => Ok(Value::Text(t)),
                    },
                    other => Ok(Value::Text(other.to_string())),
                }
            }
            "LEFT" | "RIGHT" => {
                arity(1, 2)?;
                let text = self.text_arg(&args[0])?;
                let n = self.opt_number(args, 1, 1.0)?;
                if n < 0.0 {
                    return Err(ErrorKind::Value);
                }
                let n = n as usize;
                let chars: Vec<char> = text.chars().collect();
                let n = n.min(chars.len());
                let part = if name == "LEFT" { &chars[..n] } else { &chars[chars.len() - n..] };
                Ok(Value::Text(part.iter().collect()))
            }
            "MID" => {
                arity(3, 3)?;
                let text = self.text_arg(&args[0])?;
                let (start, n) = (self.number_arg(&args[1])?.trunc(), self.number_arg(&args[2])?.trunc());
                if start < 1.0 || n < 0.0 {
                    return Err(ErrorKind::Value);
                }
                Ok(Value::Text(text.chars().skip(start as usize - 1).take(n as usize).collect()))
            }
            "TRIM" => {
                arity(1, 1)?;
                Ok(Value::Text(
                    self.text_arg(&args[0])?
                        .split(' ')
                        .filter(|w| !w.is_empty())
                        .collect::<Vec<_>>()
                        .join(" "),
                ))
            }
            "SUBSTITUTE" => {
                arity(3, 4)?;
                let (text, old, new) =
                    (self.text_arg(&args[0])?, self.text_arg(&args[1])?, self.text_arg(&args[2])?);
                if old.is_empty() {
                    return Ok(Value::Text(text));
                }
                match present(args, 3) {
                    None => Ok(Value::Text(text.replace(&old, &new))),
                    Some(e) => {
                        let nth = self.number_arg(e)?.trunc();
                        if nth < 1.0 {
                            return Err(ErrorKind::Value);
                        }
                        match text.match_indices(&old).nth(nth as usize - 1) {
                            Some((i, _)) => {
                                Ok(Value::Text(format!("{}{new}{}", &text[..i], &text[i + old.len()..])))
                            }
                            None => Ok(Value::Text(text)),
                        }
                    }
                }
            }
            "FIND" | "SEARCH" => {
                arity(2, 3)?;
                let (needle, hay) = (self.text_arg(&args[0])?, self.text_arg(&args[1])?);
                let start = self.opt_number(args, 2, 1.0)?.trunc();
                let chars: Vec<char> = hay.chars().collect();
                if start < 1.0 || start as usize > chars.len() + 1 {
                    return Err(ErrorKind::Value);
                }
                let from = start as usize - 1;
                let found = (from..=chars.len()).find(|&i| {
                    let rest: String = chars[i..].iter().collect();
                    if name == "FIND" {
                        rest.starts_with(&needle)
                    } else if needle.contains(['*', '?', '~']) {
                        wildcard(&format!("{needle}*"), &rest)
                    } else {
                        rest.to_lowercase().starts_with(&needle.to_lowercase())
                    }
                });
                found.map(|i| Value::Number((i + 1) as f64)).ok_or(ErrorKind::Value)
            }
            "REPT" => {
                arity(2, 2)?;
                let text = self.text_arg(&args[0])?;
                let n = self.number_arg(&args[1])?.trunc();
                if n < 0.0 || text.len() as f64 * n > 32_767.0 {
                    return Err(ErrorKind::Value);
                }
                Ok(Value::Text(text.repeat(n as usize)))
            }
            "VALUE" => {
                arity(1, 1)?;
                match self.scalar(&args[0]) {
                    Value::Number(n) => Ok(Value::Number(n)),
                    Value::Error(e) => Err(e),
                    Value::Empty => Ok(Value::Number(0.0)),
                    v => match parse_literal(&v.to_string()).content {
                        Content::Value(Value::Number(n)) => Ok(Value::Number(n)),
                        _ => Err(ErrorKind::Value),
                    },
                }
            }

            // ----- dates -----
            "TODAY" => {
                arity(0, 0)?;
                Ok(Value::Number(now_serial().floor()))
            }
            "NOW" => {
                arity(0, 0)?;
                Ok(Value::Number(now_serial()))
            }
            "DATE" => {
                arity(3, 3)?;
                let (y, m, d) =
                    (self.number_arg(&args[0])?, self.number_arg(&args[1])?, self.number_arg(&args[2])?);
                Ok(Value::Number(date(y, m, d)?))
            }
            "YEAR" | "MONTH" | "DAY" => {
                arity(1, 1)?;
                let serial = self.number_arg(&args[0])?;
                let (y, m, d) = serial_to_date(serial).ok_or(ErrorKind::Num)?;
                Ok(Value::Number(match name {
                    "YEAR" => y as f64,
                    "MONTH" => f64::from(m),
                    _ => f64::from(d),
                }))
            }
            "WEEKDAY" => {
                arity(1, 2)?;
                let serial = self.number_arg(&args[0])?;
                if serial < 0.0 {
                    return Err(ErrorKind::Num);
                }
                let sunday0 = weekday(serial);
                let n = match self.opt_number(args, 1, 1.0)? as i32 {
                    1 => sunday0 + 1,
                    2 => (sunday0 + 6) % 7 + 1,
                    3 => (sunday0 + 6) % 7,
                    _ => return Err(ErrorKind::Num),
                };
                Ok(Value::Number(f64::from(n)))
            }
            _ => Err(ErrorKind::Name),
        }
    }

    fn sum_or_average(&self, nums: Vec<f64>, average: bool) -> Res<Value> {
        let sum: f64 = nums.iter().sum();
        if !average {
            return Ok(Value::number(sum));
        }
        if nums.is_empty() { Err(ErrorKind::Div0) } else { Ok(Value::number(sum / nums.len() as f64)) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard("app*", "Appel"));
        assert!(wildcard("*el", "appel"));
        assert!(wildcard("a?pel", "appel"));
        assert!(!wildcard("a?pel", "apel"));
        assert!(wildcard("*", ""));
        assert!(wildcard("~*", "*"));
        assert!(!wildcard("~*", "x"));
        assert!(wildcard("a*b*c", "aXXbYYc"));
        assert!(!wildcard("a*b*c", "aXXbYY"));
    }

    #[test]
    fn criteria() {
        let c = Criterion::new(&Value::Text(">=10".into()));
        assert!(c.matches(&Value::Number(10.0)));
        assert!(!c.matches(&Value::Number(9.0)));
        assert!(!c.matches(&Value::Text("abc".into())));
        let c = Criterion::new(&Value::Text("<>".into()));
        assert!(c.matches(&Value::Number(1.0)));
        assert!(!c.matches(&Value::Empty));
        let c = Criterion::new(&Value::Number(5.0));
        assert!(c.matches(&Value::Text("5".into())));
        assert!(Criterion::new(&Value::Text("".into())).matches(&Value::Empty));
    }

    #[test]
    fn dates_roll_over() {
        assert_eq!(date(2026.0, 10.0, 22.0), Ok(46317.0));
        assert_eq!(date(2026.0, 13.0, 1.0), date(2027.0, 1.0, 1.0));
        assert_eq!(date(2026.0, 3.0, 0.0), date(2026.0, 2.0, 28.0));
        // Years below 1900 count from 1900, like Excel.
        assert_eq!(date(126.0, 10.0, 22.0), Ok(46317.0));
    }
}
