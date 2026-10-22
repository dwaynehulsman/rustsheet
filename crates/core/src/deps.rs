//! Reverse dependency index: given a cell that changed, find the formulas
//! that read it without scanning every formula in the sheet.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::addr::{CellRef, Range};
use crate::formula::Precedent;

/// Ranges are indexed in buckets of this many rows and columns.
const BUCKET_ROWS: u32 = 256;
const BUCKET_COLS: u32 = 16;
/// Ranges touching more buckets than this (whole columns, say) go on a
/// separate list that is scanned linearly instead.
const MAX_BUCKETS: u64 = 64;

type Bucket = (u32, u32);

#[derive(Default, Debug)]
pub(crate) struct DepGraph {
    /// What each formula cell reads. Every formula cell has an entry, even
    /// one without references, so this doubles as the set of formulas.
    precedents: FxHashMap<CellRef, Vec<Precedent>>,
    /// Single-cell references, reversed.
    cell_dependents: FxHashMap<CellRef, FxHashSet<CellRef>>,
    /// Range references by bucket, then by the formula cell holding them.
    buckets: FxHashMap<Bucket, FxHashMap<CellRef, Vec<Range>>>,
    /// Range references too large for the bucket index.
    large: FxHashMap<CellRef, Vec<Range>>,
}

fn buckets_of(r: &Range) -> (std::ops::RangeInclusive<u32>, std::ops::RangeInclusive<u32>) {
    (r.start.row / BUCKET_ROWS..=r.end.row / BUCKET_ROWS, r.start.col / BUCKET_COLS..=r.end.col / BUCKET_COLS)
}

fn bucket_count(r: &Range) -> u64 {
    let (rows, cols) = buckets_of(r);
    u64::from(rows.end() - rows.start() + 1) * u64::from(cols.end() - cols.start() + 1)
}

impl DepGraph {
    pub fn is_formula(&self, cell: CellRef) -> bool {
        self.precedents.contains_key(&cell)
    }

    pub fn formula_cells(&self) -> impl Iterator<Item = CellRef> + '_ {
        self.precedents.keys().copied()
    }

    pub fn formula_count(&self) -> usize {
        self.precedents.len()
    }

    pub fn precedents(&self, cell: CellRef) -> &[Precedent] {
        self.precedents.get(&cell).map_or(&[], Vec::as_slice)
    }

    /// Registers `cell` as a formula reading `precedents`, replacing what it read before.
    pub fn set(&mut self, cell: CellRef, mut precedents: Vec<Precedent>) {
        self.remove(cell);
        precedents.sort_unstable_by_key(|p| match p {
            Precedent::Cell(c) => (0, c.row, c.col, 0, 0),
            Precedent::Range(r) => (1, r.start.row, r.start.col, r.end.row, r.end.col),
        });
        precedents.dedup();
        for p in &precedents {
            match *p {
                Precedent::Cell(c) => {
                    self.cell_dependents.entry(c).or_default().insert(cell);
                }
                Precedent::Range(r) if bucket_count(&r) > MAX_BUCKETS => {
                    self.large.entry(cell).or_default().push(r);
                }
                Precedent::Range(r) => {
                    let (rows, cols) = buckets_of(&r);
                    for br in rows {
                        for bc in cols.clone() {
                            self.buckets.entry((br, bc)).or_default().entry(cell).or_default().push(r);
                        }
                    }
                }
            }
        }
        self.precedents.insert(cell, precedents);
    }

    /// Forgets that `cell` is a formula.
    pub fn remove(&mut self, cell: CellRef) {
        let Some(old) = self.precedents.remove(&cell) else { return };
        for p in old {
            match p {
                Precedent::Cell(c) => {
                    if let Some(set) = self.cell_dependents.get_mut(&c) {
                        set.remove(&cell);
                        if set.is_empty() {
                            self.cell_dependents.remove(&c);
                        }
                    }
                }
                Precedent::Range(r) if bucket_count(&r) > MAX_BUCKETS => {
                    self.large.remove(&cell);
                }
                Precedent::Range(r) => {
                    let (rows, cols) = buckets_of(&r);
                    for br in rows {
                        for bc in cols.clone() {
                            if let Some(map) = self.buckets.get_mut(&(br, bc)) {
                                map.remove(&cell);
                                if map.is_empty() {
                                    self.buckets.remove(&(br, bc));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Calls `f` for every formula that reads `at` directly. A formula may be
    /// reported more than once; callers deduplicate.
    pub fn dependents(&self, at: CellRef, mut f: impl FnMut(CellRef)) {
        if let Some(set) = self.cell_dependents.get(&at) {
            set.iter().copied().for_each(&mut f);
        }
        if let Some(map) = self.buckets.get(&(at.row / BUCKET_ROWS, at.col / BUCKET_COLS)) {
            for (cell, ranges) in map {
                if ranges.iter().any(|r| r.contains(at)) {
                    f(*cell);
                }
            }
        }
        for (cell, ranges) in &self.large {
            if ranges.iter().any(|r| r.contains(at)) {
                f(*cell);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> CellRef {
        CellRef::parse(s).unwrap()
    }

    fn deps(g: &DepGraph, at: &str) -> Vec<String> {
        let mut out = Vec::new();
        g.dependents(c(at), |d| out.push(d.to_string()));
        out.sort();
        out.dedup();
        out
    }

    #[test]
    fn finds_cell_bucket_and_large_dependents() {
        let mut g = DepGraph::default();
        g.set(c("C1"), vec![Precedent::Cell(c("A1")), Precedent::Cell(c("B1"))]);
        g.set(c("D1"), vec![Precedent::Range(Range::new(c("A1"), c("A1000")))]);
        g.set(c("E1"), vec![Precedent::Range(Range::new(c("A1"), c("A1048576")))]);
        assert_eq!(deps(&g, "A1"), ["C1", "D1", "E1"]);
        assert_eq!(deps(&g, "A999"), ["D1", "E1"]);
        assert_eq!(deps(&g, "A5000"), ["E1"]);
        assert_eq!(deps(&g, "B1"), ["C1"]);
        assert!(deps(&g, "B2").is_empty());

        // Replacing a formula drops its old edges.
        g.set(c("D1"), vec![Precedent::Cell(c("B2"))]);
        assert_eq!(deps(&g, "A999"), ["E1"]);
        assert_eq!(deps(&g, "B2"), ["D1"]);

        g.remove(c("E1"));
        g.remove(c("C1"));
        assert!(deps(&g, "A1").is_empty());
        assert_eq!(g.formula_count(), 1);
    }
}
