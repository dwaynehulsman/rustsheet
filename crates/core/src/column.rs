//! Storage for one column: rows split into fixed-size pages held in a
//! vector, so finding a cell is two array lookups and walking a range skips
//! pages that hold nothing. Pages with few cells stay small sorted lists.

use crate::sheet::Cell;

const PAGE_BITS: u32 = 6;
const PAGE: usize = 1 << PAGE_BITS;
const MASK: u32 = PAGE as u32 - 1;
/// A page switches from a short sorted list to a full array beyond this
/// many cells, so scattered cells cost about their own size, not a page each.
const SPARSE_MAX: usize = 8;

#[derive(Debug, Clone)]
enum Page {
    /// Up to [`SPARSE_MAX`] cells as (slot, cell), sorted by slot.
    Sparse(Vec<(u8, Cell)>),
    Dense(Box<[Option<Cell>; PAGE]>, u32),
}

impl Page {
    fn len(&self) -> usize {
        match self {
            Page::Sparse(v) => v.len(),
            Page::Dense(_, n) => *n as usize,
        }
    }

    fn get(&self, slot: u8) -> Option<&Cell> {
        match self {
            Page::Sparse(v) => v.binary_search_by_key(&slot, |(s, _)| *s).ok().map(|i| &v[i].1),
            Page::Dense(cells, _) => cells[slot as usize].as_ref(),
        }
    }

    fn get_mut(&mut self, slot: u8) -> Option<&mut Cell> {
        match self {
            Page::Sparse(v) => v.binary_search_by_key(&slot, |(s, _)| *s).ok().map(|i| &mut v[i].1),
            Page::Dense(cells, _) => cells[slot as usize].as_mut(),
        }
    }

    /// Stores a cell; returns true when the slot was empty.
    fn insert(&mut self, slot: u8, cell: Cell) -> bool {
        match self {
            Page::Sparse(v) => match v.binary_search_by_key(&slot, |(s, _)| *s) {
                Ok(i) => {
                    v[i].1 = cell;
                    false
                }
                Err(i) if v.len() < SPARSE_MAX => {
                    v.insert(i, (slot, cell));
                    true
                }
                Err(_) => {
                    let mut cells: Box<[Option<Cell>; PAGE]> = Box::new(std::array::from_fn(|_| None));
                    let n = v.len() as u32 + 1;
                    for (s, c) in v.drain(..) {
                        cells[s as usize] = Some(c);
                    }
                    cells[slot as usize] = Some(cell);
                    *self = Page::Dense(cells, n);
                    true
                }
            },
            Page::Dense(cells, n) => {
                let was_empty = cells[slot as usize].is_none();
                cells[slot as usize] = Some(cell);
                *n += u32::from(was_empty);
                was_empty
            }
        }
    }

    /// Removes a cell; returns true when there was one.
    fn remove(&mut self, slot: u8) -> bool {
        match self {
            Page::Sparse(v) => match v.binary_search_by_key(&slot, |(s, _)| *s) {
                Ok(i) => {
                    v.remove(i);
                    true
                }
                Err(_) => false,
            },
            Page::Dense(cells, n) => {
                let had = cells[slot as usize].take().is_some();
                *n -= u32::from(had);
                had
            }
        }
    }

    /// Cells in slot order.
    fn iter(&self) -> Box<dyn Iterator<Item = (u8, &Cell)> + '_> {
        match self {
            Page::Sparse(v) => Box::new(v.iter().map(|(s, c)| (*s, c))),
            Page::Dense(cells, _) => {
                Box::new(cells.iter().enumerate().filter_map(|(i, c)| c.as_ref().map(|c| (i as u8, c))))
            }
        }
    }
}

#[derive(Debug, Default, Clone)]
pub(crate) struct Column {
    pages: Vec<Option<Page>>,
    len: usize,
}

fn split(row: u32) -> (usize, u8) {
    ((row >> PAGE_BITS) as usize, (row & MASK) as u8)
}

impl Column {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn get(&self, row: u32) -> Option<&Cell> {
        let (p, slot) = split(row);
        self.pages.get(p)?.as_ref()?.get(slot)
    }

    pub fn get_mut(&mut self, row: u32) -> Option<&mut Cell> {
        let (p, slot) = split(row);
        self.pages.get_mut(p)?.as_mut()?.get_mut(slot)
    }

    pub fn insert(&mut self, row: u32, cell: Cell) {
        let (p, slot) = split(row);
        if self.pages.len() <= p {
            self.pages.resize_with(p + 1, || None);
        }
        let page = self.pages[p].get_or_insert_with(|| Page::Sparse(Vec::new()));
        if page.insert(slot, cell) {
            self.len += 1;
        }
    }

    pub fn remove(&mut self, row: u32) {
        let (p, slot) = split(row);
        let Some(Some(page)) = self.pages.get_mut(p) else { return };
        if page.remove(slot) {
            self.len -= 1;
            if page.len() == 0 {
                self.pages[p] = None;
                while self.pages.last().is_some_and(Option::is_none) {
                    self.pages.pop();
                }
            }
        }
    }

    /// Existing cells with `from <= row <= to`, in row order.
    pub fn range(&self, from: u32, to: u32) -> impl Iterator<Item = (u32, &Cell)> + '_ {
        let first = (from >> PAGE_BITS) as usize;
        let last = ((to >> PAGE_BITS) as usize).min(self.pages.len().saturating_sub(1));
        let pages = if first < self.pages.len() { &self.pages[first..=last] } else { &[][..] };
        pages.iter().enumerate().filter_map(move |(i, page)| Some((first + i, page.as_ref()?))).flat_map(
            move |(p, page)| {
                let base = (p as u32) << PAGE_BITS;
                page.iter().filter_map(move |(slot, cell)| {
                    let row = base + u32::from(slot);
                    (from..=to).contains(&row).then_some((row, cell))
                })
            },
        )
    }

    /// Moves every cell at or below `at` by `delta` rows. For a negative
    /// `delta` the rows `at + delta .. at` must already be empty; cells that
    /// would land past `limit` are dropped (callers check that first).
    pub fn shift(&mut self, at: u32, delta: i64, limit: u32) {
        if delta == 0 {
            return;
        }
        let first_page = (at >> PAGE_BITS) as usize;
        let mut moved = Vec::new();
        for p in first_page..self.pages.len() {
            let Some(page) = &self.pages[p] else { continue };
            let base = (p as u32) << PAGE_BITS;
            let rows: Vec<u32> =
                page.iter().map(|(slot, _)| base + u32::from(slot)).filter(|&r| r >= at).collect();
            moved.extend(rows);
        }
        let mut cells = Vec::with_capacity(moved.len());
        for row in moved {
            let (p, slot) = split(row);
            if let Some(page) = self.pages[p].as_mut() {
                let cell = match page {
                    Page::Sparse(v) => v.binary_search_by_key(&slot, |(s, _)| *s).ok().map(|i| v.remove(i).1),
                    Page::Dense(c, n) => {
                        let cell = c[slot as usize].take();
                        *n -= u32::from(cell.is_some());
                        cell
                    }
                };
                if let Some(cell) = cell {
                    self.len -= 1;
                    cells.push((row, cell));
                }
                if page.len() == 0 {
                    self.pages[p] = None;
                }
            }
        }
        while self.pages.last().is_some_and(Option::is_none) {
            self.pages.pop();
        }
        for (row, cell) in cells {
            let target = i64::from(row) + delta;
            if target >= 0 && target < i64::from(limit) {
                self.insert(target as u32, cell);
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &Cell)> + '_ {
        self.range(0, u32::MAX)
    }

    pub fn first_row(&self) -> Option<u32> {
        self.iter().next().map(|(r, _)| r)
    }

    pub fn last_row(&self) -> Option<u32> {
        let (p, page) = self.pages.iter().enumerate().rev().find_map(|(i, p)| Some((i, p.as_ref()?)))?;
        let slot = page.iter().last()?.0;
        Some(((p as u32) << PAGE_BITS) + u32::from(slot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(n: f64) -> Cell {
        Cell { value: crate::value::Value::Number(n), ..Cell::default() }
    }

    #[test]
    fn pages_grow_from_sparse_to_dense_and_back() {
        let mut col = Column::default();
        for row in (0..64).rev() {
            col.insert(row, cell(f64::from(row)));
            assert_eq!(
                col.get(row).map(|c| c.value.clone()),
                Some(crate::value::Value::Number(f64::from(row)))
            );
        }
        assert!(matches!(col.pages[0], Some(Page::Dense(_, 64))));
        let rows: Vec<u32> = col.iter().map(|(r, _)| r).collect();
        assert_eq!(rows, (0..64).collect::<Vec<_>>());
        for row in 0..64 {
            col.remove(row);
        }
        assert!(col.pages.is_empty());

        // Scattered cells stay sparse: one small list per page.
        for row in (0..64 * 100).step_by(64) {
            col.insert(row, cell(1.0));
        }
        assert!(col.pages.iter().all(|p| matches!(p, Some(Page::Sparse(v)) if v.len() == 1)));
    }

    #[test]
    fn shifting_moves_cells_below_a_row() {
        let mut col = Column::default();
        for row in [0, 5, 6, 100, 1000] {
            col.insert(row, cell(f64::from(row)));
        }
        col.shift(5, 3, 1_048_576);
        let rows: Vec<u32> = col.iter().map(|(r, _)| r).collect();
        assert_eq!(rows, [0, 8, 9, 103, 1003]);
        col.shift(8, -3, 1_048_576);
        let rows: Vec<u32> = col.iter().map(|(r, _)| r).collect();
        assert_eq!(rows, [0, 5, 6, 100, 1000]);
        assert_eq!(col.len(), 5);
    }

    #[test]
    fn insert_get_remove_and_ranges() {
        let mut col = Column::default();
        for row in [5, 63, 64, 1000, 1_048_575] {
            col.insert(row, cell(f64::from(row)));
        }
        col.insert(5, cell(55.0));
        assert_eq!(col.len(), 5);
        assert_eq!(col.get(5).map(|c| c.value.clone()), Some(crate::value::Value::Number(55.0)));
        assert!(col.get(6).is_none());
        assert!(col.get(2_000_000).is_none());
        let rows: Vec<u32> = col.range(6, 1000).map(|(r, _)| r).collect();
        assert_eq!(rows, [63, 64, 1000]);
        assert_eq!(col.first_row(), Some(5));
        assert_eq!(col.last_row(), Some(1_048_575));

        col.remove(1_048_575);
        assert_eq!(col.last_row(), Some(1000));
        assert_eq!(col.pages.len(), 1000 / PAGE + 1);
        for row in [5, 63, 64, 1000] {
            col.remove(row);
        }
        assert_eq!(col.len(), 0);
        assert!(col.pages.is_empty());
        assert_eq!(col.first_row(), None);
    }
}
