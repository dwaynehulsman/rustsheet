//! Filling: dragging the fill handle (series 1, 2, 3, dates, "Week 1",
//! month names) and Fill Down / Fill Right (plain copies).

use crate::addr::{CellRef, Range};
use crate::format::NumFmt;
use crate::input::Content;
use crate::sheet::{CellState, Sheet};
use crate::value::Value;

const NAME_LISTS: &[&[&str]] = &[
    &[
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
    ],
    &["jan", "feb", "mrt", "apr", "mei", "jun", "jul", "aug", "sep", "okt", "nov", "dec"],
    &["maandag", "dinsdag", "woensdag", "donderdag", "vrijdag", "zaterdag", "zondag"],
    &["ma", "di", "wo", "do", "vr", "za", "zo"],
    &[
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ],
    &["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"],
    &["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"],
    &["mon", "tue", "wed", "thu", "fri", "sat", "sun"],
];

/// How one line of source cells continues.
enum Pattern {
    /// value(p) = first + step * p
    Linear { first: f64, step: f64 },
    /// "Week 1": prefix, number at position 0, step, suffix, zero padding.
    Numbered { prefix: String, first: i64, step: i64, suffix: String, width: usize },
    /// Names from a list (months, days), keeping the capitalisation style.
    Names { list: &'static [&'static str], first: i64, step: i64, case: Case },
    /// Repeat the source cells (formulas shift).
    Copy,
}

#[derive(Clone, Copy)]
enum Case {
    Lower,
    Upper,
    Title,
}

fn case_of(s: &str) -> Case {
    if s.chars().all(|c| !c.is_lowercase()) && s.chars().any(char::is_uppercase) {
        Case::Upper
    } else if s.chars().next().is_some_and(char::is_uppercase) {
        Case::Title
    } else {
        Case::Lower
    }
}

fn apply_case(s: &str, case: Case) -> String {
    match case {
        Case::Lower => s.to_string(),
        Case::Upper => s.to_uppercase(),
        Case::Title => {
            let mut c = s.chars();
            c.next().map_or_else(String::new, |f| f.to_uppercase().collect::<String>() + c.as_str())
        }
    }
}

/// Splits "Week 12b" into ("Week ", 12, "b", width 2) on the last number.
fn split_number(s: &str) -> Option<(String, i64, String, usize)> {
    let bytes = s.as_bytes();
    let end = bytes.iter().rposition(u8::is_ascii_digit)? + 1;
    let start = bytes[..end].iter().rposition(|b| !b.is_ascii_digit()).map_or(0, |i| i + 1);
    let digits = &s[start..end];
    if digits.len() > 15 {
        return None;
    }
    Some((s[..start].to_string(), digits.parse().ok()?, s[end..].to_string(), digits.len()))
}

fn name_index(s: &str) -> Option<(&'static [&'static str], i64)> {
    let lower = s.to_lowercase();
    NAME_LISTS.iter().find_map(|list| list.iter().position(|n| *n == lower).map(|i| (*list, i as i64)))
}

fn number(state: &CellState) -> Option<f64> {
    match &state.content {
        Content::Value(Value::Number(n)) => Some(*n),
        _ => None,
    }
}

fn text(state: &CellState) -> Option<&str> {
    match &state.content {
        Content::Value(Value::Text(t)) => Some(t),
        _ => None,
    }
}

fn pattern(cells: &[CellState]) -> Pattern {
    let k = cells.len();
    if let Some(nums) = cells.iter().map(number).collect::<Option<Vec<f64>>>() {
        if k >= 2 {
            return Pattern::Linear { first: nums[0], step: (nums[k - 1] - nums[0]) / (k - 1) as f64 };
        }
        // A single date counts on by days; a single plain number is copied.
        if cells[0].format.num == NumFmt::Date {
            return Pattern::Linear { first: nums[0], step: 1.0 };
        }
        return Pattern::Copy;
    }
    let Some(texts) = cells.iter().map(text).collect::<Option<Vec<&str>>>() else { return Pattern::Copy };
    if let Some(names) = texts.iter().map(|t| name_index(t)).collect::<Option<Vec<_>>>()
        && names.iter().all(|(list, _)| std::ptr::eq(*list, names[0].0))
    {
        let step = if k >= 2 { names[1].1 - names[0].1 } else { 1 };
        return Pattern::Names { list: names[0].0, first: names[0].1, step, case: case_of(texts[0]) };
    }
    if let Some(parts) = texts.iter().map(|t| split_number(t)).collect::<Option<Vec<_>>>() {
        let (prefix, first, suffix, width) = parts[0].clone();
        let same = parts.iter().all(|(p, _, s, _)| *p == prefix && *s == suffix);
        if same {
            let step = if k >= 2 { (parts[k - 1].1 - first) / (k as i64 - 1) } else { 1 };
            let width = if digits_padded(&parts) { width } else { 0 };
            return Pattern::Numbered { prefix, first, step, suffix, width };
        }
    }
    Pattern::Copy
}

/// Whether numbers were written with leading zeros ("Item 007").
fn digits_padded(parts: &[(String, i64, String, usize)]) -> bool {
    parts.iter().any(|(_, n, _, w)| n.to_string().len() < *w)
}

/// States for `target` when the fill handle drags `source` over it.
/// `target` must touch `source` below, above, right or left of it and span
/// the same columns (vertical) or rows (horizontal). With `series` false
/// every cell is a copy (Fill Down / Fill Right).
pub fn fill(sheet: &Sheet, source: Range, target: Range, series: bool) -> Vec<(CellRef, CellState)> {
    let vertical = target.start.col == source.start.col && target.end.col == source.end.col;
    let (lines, len) = if vertical { (source.cols(), source.rows()) } else { (source.rows(), source.cols()) };
    let mut out = Vec::new();
    for line in 0..lines {
        let at = |p: u32| {
            if vertical {
                CellRef::new(source.start.row + p, source.start.col + line)
            } else {
                CellRef::new(source.start.row + line, source.start.col + p)
            }
        };
        let cells: Vec<CellState> = (0..len).map(|p| sheet.state(at(p))).collect();
        let pat = if series { pattern(&cells) } else { Pattern::Copy };
        let (t0, t1) =
            if vertical { (target.start.row, target.end.row) } else { (target.start.col, target.end.col) };
        let s0 = if vertical { source.start.row } else { source.start.col };
        for t in t0..=t1 {
            // Position relative to the first source cell; negative above/left.
            let p = i64::from(t) - i64::from(s0);
            let index = p.rem_euclid(i64::from(len));
            let src = &cells[index as usize];
            let dest = if vertical {
                CellRef::new(t, source.start.col + line)
            } else {
                CellRef::new(source.start.row + line, t)
            };
            let content = match &pat {
                Pattern::Linear { first, step } => Content::Value(Value::number(first + step * p as f64)),
                Pattern::Numbered { prefix, first, step, suffix, width } => {
                    let n = first + step * p;
                    let digits = if *width > 0 && n >= 0 {
                        format!("{n:0width$}", width = *width)
                    } else {
                        n.to_string()
                    };
                    Content::Value(Value::Text(format!("{prefix}{digits}{suffix}")))
                }
                Pattern::Names { list, first, step, case } => {
                    let i = (first + step * p).rem_euclid(list.len() as i64) as usize;
                    Content::Value(Value::Text(apply_case(list[i], *case)))
                }
                Pattern::Copy => match &src.content {
                    Content::Formula(e) => {
                        // Shift by the distance from the source cell repeated here.
                        let shift = i64::from(t) - (i64::from(s0) + index);
                        Content::Formula(if vertical { e.shifted(shift, 0) } else { e.shifted(0, shift) })
                    }
                    other => other.clone(),
                },
            };
            out.push((dest, CellState { content, format: src.format }));
        }
    }
    out
}

/// The part of `extended` outside `source` along one axis, if `extended`
/// grows `source` in exactly one direction.
pub fn fill_target(source: Range, extended: Range) -> Option<Range> {
    let same_cols = extended.start.col == source.start.col && extended.end.col == source.end.col;
    let same_rows = extended.start.row == source.start.row && extended.end.row == source.end.row;
    if same_cols && extended.end.row > source.end.row && extended.start.row == source.start.row {
        Some(Range::new(CellRef::new(source.end.row + 1, source.start.col), extended.end))
    } else if same_cols && extended.start.row < source.start.row && extended.end.row == source.end.row {
        Some(Range::new(extended.start, CellRef::new(source.start.row - 1, source.end.col)))
    } else if same_rows && extended.end.col > source.end.col && extended.start.col == source.start.col {
        Some(Range::new(CellRef::new(source.start.row, source.end.col + 1), extended.end))
    } else if same_rows && extended.start.col < source.start.col && extended.end.col == source.end.col {
        Some(Range::new(extended.start, CellRef::new(source.end.row, source.start.col - 1)))
    } else {
        None
    }
}
