//! Cell addresses and rectangular ranges.

use std::fmt;

/// Number of rows in a sheet, same as Excel.
pub const MAX_ROWS: u32 = 1_048_576;
/// Number of columns in a sheet, same as Excel (A through XFD).
pub const MAX_COLS: u32 = 16_384;

/// A zero-based cell position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct CellRef {
    pub row: u32,
    pub col: u32,
}

impl CellRef {
    pub const fn new(row: u32, col: u32) -> Self {
        Self { row, col }
    }

    /// Parses an A1-style address such as `B12` (no `$` markers).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let split = s.find(|c: char| c.is_ascii_digit())?;
        let (letters, digits) = s.split_at(split);
        let col = parse_col(letters)?;
        let row = parse_row(digits)?;
        Some(Self { row, col })
    }

    /// Moves by a signed offset, returning `None` when the result leaves the grid.
    pub fn offset(self, drow: i64, dcol: i64) -> Option<Self> {
        let row = i64::from(self.row) + drow;
        let col = i64::from(self.col) + dcol;
        if (0..i64::from(MAX_ROWS)).contains(&row) && (0..i64::from(MAX_COLS)).contains(&col) {
            Some(Self::new(row as u32, col as u32))
        } else {
            None
        }
    }

    /// Like [`CellRef::offset`] but clamps to the grid edges.
    pub fn offset_clamped(self, drow: i64, dcol: i64) -> Self {
        let row = (i64::from(self.row) + drow).clamp(0, i64::from(MAX_ROWS) - 1);
        let col = (i64::from(self.col) + dcol).clamp(0, i64::from(MAX_COLS) - 1);
        Self::new(row as u32, col as u32)
    }
}

impl fmt::Display for CellRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", col_name(self.col), self.row + 1)
    }
}

/// An inclusive rectangle of cells, always stored with `start <= end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Range {
    pub start: CellRef,
    pub end: CellRef,
}

impl Range {
    /// Builds a range from two corners in any order.
    pub fn new(a: CellRef, b: CellRef) -> Self {
        Self {
            start: CellRef::new(a.row.min(b.row), a.col.min(b.col)),
            end: CellRef::new(a.row.max(b.row), a.col.max(b.col)),
        }
    }

    pub fn single(at: CellRef) -> Self {
        Self { start: at, end: at }
    }

    pub fn contains(&self, at: CellRef) -> bool {
        (self.start.row..=self.end.row).contains(&at.row) && (self.start.col..=self.end.col).contains(&at.col)
    }

    pub fn rows(&self) -> u32 {
        self.end.row - self.start.row + 1
    }

    pub fn cols(&self) -> u32 {
        self.end.col - self.start.col + 1
    }

    /// Number of cells, as `u64` because a whole sheet does not fit in `u32`.
    pub fn len(&self) -> u64 {
        u64::from(self.rows()) * u64::from(self.cols())
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    /// Iterates all positions row by row. Only use this on small ranges.
    pub fn iter(&self) -> impl Iterator<Item = CellRef> + use<> {
        let r = *self;
        (r.start.row..=r.end.row)
            .flat_map(move |row| (r.start.col..=r.end.col).map(move |col| CellRef::new(row, col)))
    }

    pub fn intersect(&self, other: &Range) -> Option<Range> {
        let start = CellRef::new(self.start.row.max(other.start.row), self.start.col.max(other.start.col));
        let end = CellRef::new(self.end.row.min(other.end.row), self.end.col.min(other.end.col));
        (start.row <= end.row && start.col <= end.col).then_some(Range { start, end })
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.start == self.end {
            write!(f, "{}", self.start)
        } else {
            write!(f, "{}:{}", self.start, self.end)
        }
    }
}

/// Column letters for a zero-based column index: 0 is `A`, 26 is `AA`.
pub fn col_name(col: u32) -> String {
    let mut n = col + 1;
    let mut out = Vec::with_capacity(3);
    while n > 0 {
        let rem = (n - 1) % 26;
        out.push(b'A' + rem as u8);
        n = (n - 1) / 26;
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

/// Parses column letters (case-insensitive) into a zero-based index.
pub fn parse_col(letters: &str) -> Option<u32> {
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    let mut n: u32 = 0;
    for c in letters.chars() {
        if !c.is_ascii_alphabetic() {
            return None;
        }
        n = n * 26 + (c.to_ascii_uppercase() as u32 - 'A' as u32 + 1);
    }
    (1..=MAX_COLS).contains(&n).then(|| n - 1)
}

/// Parses a one-based row number into a zero-based index.
pub fn parse_row(digits: &str) -> Option<u32> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    (1..=MAX_ROWS).contains(&n).then(|| n - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_names_round_trip() {
        for (i, name) in [
            (0, "A"),
            (25, "Z"),
            (26, "AA"),
            (51, "AZ"),
            (52, "BA"),
            (701, "ZZ"),
            (702, "AAA"),
            (16383, "XFD"),
        ] {
            assert_eq!(col_name(i), name);
            assert_eq!(parse_col(name), Some(i));
        }
        assert_eq!(parse_col("XFE"), None);
        assert_eq!(parse_col(""), None);
    }

    #[test]
    fn parse_cell_refs() {
        assert_eq!(CellRef::parse("a1"), Some(CellRef::new(0, 0)));
        assert_eq!(CellRef::parse("XFD1048576"), Some(CellRef::new(MAX_ROWS - 1, MAX_COLS - 1)));
        assert_eq!(CellRef::parse("A0"), None);
        assert_eq!(CellRef::parse("A1048577"), None);
        assert_eq!(CellRef::new(9, 27).to_string(), "AB10");
    }

    #[test]
    fn range_normalises_and_intersects() {
        let r = Range::new(CellRef::new(5, 3), CellRef::new(1, 1));
        assert_eq!(r.to_string(), "B2:D6");
        assert_eq!(r.len(), 15);
        let other = Range::new(CellRef::new(4, 0), CellRef::new(10, 1));
        assert_eq!(r.intersect(&other).unwrap().to_string(), "B5:B6");
    }
}
