//! Writes a large test workbook and times saving, opening and recalculating.
//!
//! cargo run --release -p rustsheet-core --example generate -- [rows] [out.xlsx]

use std::time::{Duration, Instant};

use rustsheet_core::{Align, CellFormat, CellRef, CellState, NumFmt, Sheet, io, parse_input};

/// CPU time used by this thread. On a busy machine wall-clock time mostly
/// measures waiting for a core, so the benchmark reports both.
fn cpu() -> Duration {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime only writes to the timespec we pass.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// Runs `f`, returning its result with (wall, cpu) milliseconds.
fn timed<T>(f: impl FnOnce() -> T) -> (T, f64, f64) {
    let (wall, c) = (Instant::now(), cpu());
    let out = f();
    (out, wall.elapsed().as_secs_f64() * 1000.0, (cpu() - c).as_secs_f64() * 1000.0)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let rows: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let out = args.next().unwrap_or_else(|| format!("bench-{rows}.xlsx"));

    let (mut sheet, build_wall, build_cpu) = timed(|| {
        let mut sheet = Sheet::new("Bestellingen");
        let bold = CellFormat { bold: true, align: Align::Center, ..CellFormat::default() };
        let currency = CellFormat { num: NumFmt::Currency, ..CellFormat::default() };
        let put = |sheet: &mut Sheet, row: u32, col: u32, text: &str, format: CellFormat| {
            let content = parse_input(text).expect("valid input").content;
            sheet.put(CellRef::new(row, col), CellState { content, format });
        };
        for (col, title) in
            ["Nr", "Product", "Aantal", "Prijs", "Bedrag", "Cumulatief", "Klasse", "Incl. btw"]
                .iter()
                .enumerate()
        {
            put(&mut sheet, 0, col as u32, title, bold);
        }
        let products = ["Koffie", "Thee", "Melk", "Suiker", "Koekjes", "Water", "Sap", "Brood"];
        for i in 1..=rows {
            let r = i + 1; // 1-based row number in formulas
            put(&mut sheet, i, 0, &i.to_string(), CellFormat::default());
            put(&mut sheet, i, 1, products[i as usize % products.len()], CellFormat::default());
            put(&mut sheet, i, 2, &((i * 7) % 23 + 1).to_string(), CellFormat::default());
            put(&mut sheet, i, 3, &format!("{}.{:02}", (i * 13) % 90 + 1, (i * 37) % 100), currency);
            put(&mut sheet, i, 4, &format!("=C{r}*D{r}"), currency);
            let running = if i == 1 { format!("=E{r}") } else { format!("=F{}+E{r}", r - 1) };
            put(&mut sheet, i, 5, &running, currency);
            put(&mut sheet, i, 6, &format!("=IF(E{r}>500,\"groot\",\"klein\")"), CellFormat::default());
            put(&mut sheet, i, 7, &format!("=ROUND(E{r}*1.21,2)"), currency);
        }
        sheet.set_col_width(1, Some(110.0));
        sheet.set_col_width(5, Some(120.0));
        sheet
    });

    let ((), recalc_wall, recalc_cpu) = timed(|| sheet.recalc_all());
    let path = std::path::Path::new(&out);
    let ((), save_wall, save_cpu) = timed(|| io::save(&sheet, path).expect("save"));
    let (mut loaded, open_wall, open_cpu) = timed(|| io::open(path).expect("open"));

    // One edit at the top of the running-total chain touches every row below.
    let content = parse_input("999").unwrap().content;
    let (_, edit_wall, edit_cpu) = timed(|| {
        loaded.set_states(vec![(CellRef::new(1, 2), CellState { content, ..CellState::default() })])
    });
    let touched = loaded.last_recalc_count();

    // An edit at the bottom touches only its own row.
    let content = parse_input("5").unwrap().content;
    let (_, last_wall, last_cpu) = timed(|| {
        loaded.set_states(vec![(CellRef::new(rows, 2), CellState { content, ..CellState::default() })])
    });

    println!("rows: {rows}, cells: {}, formulas: {}", loaded.cell_count(), loaded.formula_count());
    println!("{:<44} {:>10} {:>10}", "step", "wall ms", "cpu ms");
    for (name, wall, cpu) in [
        ("build sheet".to_string(), build_wall, build_cpu),
        ("full recalc".to_string(), recalc_wall, recalc_cpu),
        ("save xlsx".to_string(), save_wall, save_cpu),
        ("open xlsx (read + full recalc)".to_string(), open_wall, open_cpu),
        (format!("edit C2 (recalcs {touched} formulas)"), edit_wall, edit_cpu),
        (format!("edit last row (recalcs {} formulas)", loaded.last_recalc_count()), last_wall, last_cpu),
    ] {
        println!("{name:<44} {wall:>10.1} {cpu:>10.1}");
    }
    println!("file: {out}");
}
