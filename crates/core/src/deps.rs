//! Reverse dependency index over a whole workbook: given a cell that
//! changed, find the formulas that read it, on any sheet, without scanning
//! every formula.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::addr::{CellRef, Range};
use crate::formula::Precedent;
use crate::sheet::SheetId;

/// A cell on a particular sheet.
pub(crate) type Node = (SheetId, CellRef);

/// Ranges are indexed in buckets of this many rows and columns.
const BUCKET_ROWS: u32 = 256;
const BUCKET_COLS: u32 = 16;
/// Ranges touching more buckets than this (whole columns, say) go on a
/// separate list that is scanned linearly instead.
const MAX_BUCKETS: u64 = 64;

type Bucket = (SheetId, u32, u32);

#[derive(Default, Debug)]
pub(crate) struct DepGraph {
    /// What each formula cell reads. Every formula cell has an entry, even
    /// one without references, so this doubles as the set of formulas.
    precedents: FxHashMap<Node, Vec<Precedent>>,
    /// Single-cell references, reversed.
    cell_dependents: FxHashMap<Node, FxHashSet<Node>>,
    /// Range references by bucket, then by the formula cell holding them.
    buckets: FxHashMap<Bucket, FxHashMap<Node, Vec<Range>>>,
    /// Range references too large for the bucket index, with their sheet.
    large: FxHashMap<Node, Vec<(SheetId, Range)>>,
}

fn buckets_of(r: &Range) -> (std::ops::RangeInclusive<u32>, std::ops::RangeInclusive<u32>) {
    (r.start.row / BUCKET_ROWS..=r.end.row / BUCKET_ROWS, r.start.col / BUCKET_COLS..=r.end.col / BUCKET_COLS)
}

fn bucket_count(r: &Range) -> u64 {
    let (rows, cols) = buckets_of(r);
    u64::from(rows.end() - rows.start() + 1) * u64::from(cols.end() - cols.start() + 1)
}

impl DepGraph {
    pub fn is_formula(&self, cell: Node) -> bool {
        self.precedents.contains_key(&cell)
    }

    pub fn clear(&mut self) {
        *self = DepGraph::default();
    }

    pub fn formula_cells(&self) -> impl Iterator<Item = Node> + '_ {
        self.precedents.keys().copied()
    }

    pub fn formula_count(&self) -> usize {
        self.precedents.len()
    }

    pub fn precedents(&self, cell: Node) -> &[Precedent] {
        self.precedents.get(&cell).map_or(&[], Vec::as_slice)
    }

    /// Registers `cell` as a formula reading `precedents`, replacing what it read before.
    pub fn set(&mut self, cell: Node, mut precedents: Vec<Precedent>) {
        self.remove(cell);
        precedents.sort_unstable_by_key(|p| match p {
            Precedent::Cell(s, c) => (0, *s, c.row, c.col, 0, 0),
            Precedent::Range(s, r) => (1, *s, r.start.row, r.start.col, r.end.row, r.end.col),
        });
        precedents.dedup();
        for p in &precedents {
            match *p {
                Precedent::Cell(s, c) => {
                    self.cell_dependents.entry((s, c)).or_default().insert(cell);
                }
                Precedent::Range(s, r) if bucket_count(&r) > MAX_BUCKETS => {
                    self.large.entry(cell).or_default().push((s, r));
                }
                Precedent::Range(s, r) => {
                    let (rows, cols) = buckets_of(&r);
                    for br in rows {
                        for bc in cols.clone() {
                            self.buckets.entry((s, br, bc)).or_default().entry(cell).or_default().push(r);
                        }
                    }
                }
            }
        }
        self.precedents.insert(cell, precedents);
    }

    /// Forgets that `cell` is a formula.
    pub fn remove(&mut self, cell: Node) {
        let Some(old) = self.precedents.remove(&cell) else { return };
        for p in old {
            match p {
                Precedent::Cell(s, c) => {
                    if let Some(set) = self.cell_dependents.get_mut(&(s, c)) {
                        set.remove(&cell);
                        if set.is_empty() {
                            self.cell_dependents.remove(&(s, c));
                        }
                    }
                }
                Precedent::Range(_, r) if bucket_count(&r) > MAX_BUCKETS => {
                    self.large.remove(&cell);
                }
                Precedent::Range(s, r) => {
                    let (rows, cols) = buckets_of(&r);
                    for br in rows {
                        for bc in cols.clone() {
                            if let Some(map) = self.buckets.get_mut(&(s, br, bc)) {
                                map.remove(&cell);
                                if map.is_empty() {
                                    self.buckets.remove(&(s, br, bc));
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
    pub fn dependents(&self, node: Node, mut f: impl FnMut(Node)) {
        let (sheet, at) = node;
        if let Some(set) = self.cell_dependents.get(&node) {
            set.iter().copied().for_each(&mut f);
        }
        if let Some(map) = self.buckets.get(&(sheet, at.row / BUCKET_ROWS, at.col / BUCKET_COLS)) {
            for (cell, ranges) in map {
                if ranges.iter().any(|r| r.contains(at)) {
                    f(*cell);
                }
            }
        }
        for (cell, ranges) in &self.large {
            if ranges.iter().any(|(s, r)| *s == sheet && r.contains(at)) {
                f(*cell);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Node {
        (0, CellRef::parse(s).unwrap())
    }

    fn cell(s: &str) -> Precedent {
        Precedent::Cell(0, CellRef::parse(s).unwrap())
    }

    fn range(a: &str, b: &str) -> Precedent {
        Precedent::Range(0, Range::new(CellRef::parse(a).unwrap(), CellRef::parse(b).unwrap()))
    }

    fn deps(g: &DepGraph, at: &str) -> Vec<String> {
        let mut out = Vec::new();
        g.dependents(c(at), |d| out.push(format!("{}{}", if d.0 == 0 { "" } else { "S1!" }, d.1)));
        out.sort();
        out.dedup();
        out
    }

    #[test]
    fn finds_cell_bucket_and_large_dependents() {
        let mut g = DepGraph::default();
        g.set(c("C1"), vec![cell("A1"), cell("B1")]);
        g.set(c("D1"), vec![range("A1", "A1000")]);
        g.set(c("E1"), vec![range("A1", "A1048576")]);
        // A formula on another sheet reading A1 of sheet 0.
        g.set((1, CellRef::new(0, 0)), vec![cell("A1")]);
        assert_eq!(deps(&g, "A1"), ["C1", "D1", "E1", "S1!A1"]);
        assert_eq!(deps(&g, "A999"), ["D1", "E1"]);
        assert_eq!(deps(&g, "A5000"), ["E1"]);
        assert_eq!(deps(&g, "B1"), ["C1"]);
        assert!(deps(&g, "B2").is_empty());

        // Replacing a formula drops its old edges.
        g.set(c("D1"), vec![cell("B2")]);
        assert_eq!(deps(&g, "A999"), ["E1"]);
        assert_eq!(deps(&g, "B2"), ["D1"]);

        g.remove(c("E1"));
        g.remove(c("C1"));
        g.remove((1, CellRef::new(0, 0)));
        assert!(deps(&g, "A1").is_empty());
        assert_eq!(g.formula_count(), 1);
    }
}
