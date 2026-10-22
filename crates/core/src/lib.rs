//! Spreadsheet engine for Rustsheet: cell storage, formulas with
//! incremental recalculation, formatting, clipboard and file formats.
//! Nothing in here depends on the GUI.

pub mod addr;
pub mod clipboard;
mod column;
mod deps;
pub mod document;
pub mod fill;
pub mod find;
pub mod format;
pub mod formula;
pub mod input;
pub mod io;
pub mod sheet;
pub mod value;
pub mod workbook;

pub use addr::{CellRef, MAX_COLS, MAX_ROWS, Range};
pub use document::{Change, Document, Spot};
pub use format::{Align, CellFormat, NumFmt};
pub use input::{Content, edit_text, parse_input};
pub use sheet::{Cell, CellState, Sheet, SheetId};
pub use value::{ErrorKind, Value};
pub use workbook::Workbook;
