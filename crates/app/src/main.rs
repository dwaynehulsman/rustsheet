//! Rustsheet: a native macOS spreadsheet.

mod app;
mod bench;
mod clip;
mod fonts;
mod grid;
mod menu;
mod recent;
#[cfg(test)]
mod tests;
mod view;

use std::path::PathBuf;
use std::time::Instant;

fn main() -> eframe::Result {
    let started = Instant::now();
    let mut files = Vec::new();
    let mut bench_frames: Option<u32> = None;
    let mut bench_out = None;
    let mut shot = None;
    let mut select = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--bench-scroll") => bench_frames = Some(bench_frames.unwrap_or(600)),
            Some("--bench-frames") => {
                bench_frames = args.next().and_then(|n| n.to_str().and_then(|s| s.parse().ok()));
            }
            Some("--bench-out") => bench_out = args.next().map(PathBuf::from),
            Some("--screenshot") => shot = args.next().map(|p| bench::Shot::new(PathBuf::from(p), false)),
            Some("--screenshot-and-quit") => {
                shot = args.next().map(|p| bench::Shot::new(PathBuf::from(p), true))
            }
            Some("--select") => select = args.next().and_then(|s| s.into_string().ok()),
            // Finder passes a process serial number on older systems.
            Some(s) if s.starts_with("-psn_") => {}
            _ => files.push(PathBuf::from(arg)),
        }
    }
    let bench = bench_frames.map(|n| bench::Bench::new(started, n, bench_out));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Rustsheet")
            .with_app_id("nl.infodatek.rustsheet")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([480.0, 320.0]),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Rustsheet",
        options,
        Box::new(move |cc| Ok(Box::new(app::RustsheetApp::new(cc, files, bench, shot, select)))),
    )
}
