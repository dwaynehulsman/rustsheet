//! Storage for one column: rows split into fixed-size pages held in a
//! vector, so finding a cell is two array lookups and walking a range skips
//! pages that hold nothing.

use crate::sheet::Cell;

const PAGE_BITS: u32 = 6;
const PAGE: usize = 1 << PAGE_BITS;
const MASK: u32 = PAGE as u32 - 1;

#[derive(Debug)]
struct Page {
    cells: [Option<Cell>; PAGE],
    count: u32,
}

impl Page {
    fn new() -> Box<Page> {
        Box::new(Page { cells: std::array::from_fn(|_| None), count: 0 })
    }
}

#[derive(Debug, Default)]
pub(crate) struct Column {
    pages: Vec<Option<Box<Page>>>,
    len: usize,
}

impl Column {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn get(&self, row: u32) -> Option<&Cell> {
        self.pages.get((row >> PAGE_BITS) as usize)?.as_ref()?.cells[(row & MASK) as usize].as_ref()
    }

    pub fn get_mut(&mut self, row: u32) -> Option<&mut Cell> {
        self.pages.get_mut((row >> PAGE_BITS) as usize)?.as_mut()?.cells[(row & MASK) as usize].as_mut()
    }

    pub fn insert(&mut self, row: u32, cell: Cell) {
        let p = (row >> PAGE_BITS) as usize;
        if self.pages.len() <= p {
            self.pages.resize_with(p + 1, || None);
        }
        let page = self.pages[p].get_or_insert_with(Page::new);
        let slot = &mut page.cells[(row & MASK) as usize];
        if slot.is_none() {
            page.count += 1;
            self.len += 1;
        }
        *slot = Some(cell);
    }

    pub fn remove(&mut self, row: u32) {
        let p = (row >> PAGE_BITS) as usize;
        let Some(Some(page)) = self.pages.get_mut(p) else { return };
        if page.cells[(row & MASK) as usize].take().is_some() {
            page.count -= 1;
            self.len -= 1;
            if page.count == 0 {
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
                page.cells.iter().enumerate().filter_map(move |(i, cell)| {
                    let row = base + i as u32;
                    if row < from || row > to { None } else { cell.as_ref().map(|c| (row, c)) }
                })
            },
        )
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &Cell)> + '_ {
        self.range(0, u32::MAX)
    }

    pub fn first_row(&self) -> Option<u32> {
        self.iter().next().map(|(r, _)| r)
    }

    pub fn last_row(&self) -> Option<u32> {
        let (p, page) = self.pages.iter().enumerate().rev().find_map(|(i, p)| Some((i, p.as_ref()?)))?;
        let slot = page.cells.iter().rposition(Option::is_some)?;
        Some(((p as u32) << PAGE_BITS) + slot as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(n: f64) -> Cell {
        Cell { value: crate::value::Value::Number(n), ..Cell::default() }
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
