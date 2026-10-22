//! Writes a PDF of the first sheet of a workbook.
//!
//! cargo run -p rustsheet-core --example topdf -- map.xlsx uit.pdf
fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("workbook");
    let output = args.next().unwrap_or_else(|| "uit.pdf".into());
    let book = rustsheet_core::io::open(std::path::Path::new(&input)).expect("open");
    std::fs::write(&output, rustsheet_core::pdf::sheet_to_pdf(book.sheet(0), None)).expect("write");
    println!("{output}");
}
