//! Opening and saving files: xlsx (and other workbooks calamine reads) and CSV.

mod csv;
mod styles;
mod xlsx;

use std::fmt;
use std::path::Path;

use crate::sheet::Sheet;

pub use self::csv::{read_csv, read_csv_str, write_csv, write_csv_string};
pub use self::xlsx::{read_workbook, write_xlsx};

#[derive(Debug)]
pub enum IoError {
    Io(std::io::Error),
    Format(String),
    Unsupported(String),
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IoError::Io(e) => write!(f, "{e}"),
            IoError::Format(msg) => f.write_str(msg),
            IoError::Unsupported(ext) => write!(f, "bestandstype '{ext}' wordt niet ondersteund"),
        }
    }
}

impl std::error::Error for IoError {}

impl From<std::io::Error> for IoError {
    fn from(e: std::io::Error) -> Self {
        IoError::Io(e)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Xlsx,
    Csv,
    /// Read-only formats calamine understands (xls, xlsm, xlsb, ods).
    OtherWorkbook,
}

impl FileKind {
    pub fn of(path: &Path) -> Option<FileKind> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "xlsx" => FileKind::Xlsx,
            "csv" | "tsv" | "txt" => FileKind::Csv,
            "xlsm" | "xlsb" | "xls" | "ods" => FileKind::OtherWorkbook,
            _ => return None,
        })
    }
}

/// Extensions the open dialog offers.
pub const OPEN_EXTENSIONS: &[&str] = &["xlsx", "xlsm", "xlsb", "xls", "ods", "csv", "tsv", "txt"];

fn sheet_name_from(path: &Path) -> String {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("Blad1").to_string()
}

/// Opens a file by extension. Workbooks load their first worksheet.
pub fn open(path: &Path) -> Result<Sheet, IoError> {
    match FileKind::of(path) {
        Some(FileKind::Csv) => read_csv(path, &sheet_name_from(path)),
        Some(FileKind::Xlsx | FileKind::OtherWorkbook) => read_workbook(path),
        None => {
            Err(IoError::Unsupported(path.extension().and_then(|e| e.to_str()).unwrap_or("").to_string()))
        }
    }
}

/// Saves by extension: `.csv` as CSV, anything else as xlsx.
pub fn save(sheet: &Sheet, path: &Path) -> Result<(), IoError> {
    match FileKind::of(path) {
        Some(FileKind::Csv) => write_csv(sheet, path),
        _ => write_xlsx(sheet, path),
    }
}
