//! Rustsheet: a native macOS spreadsheet.

mod app;
mod bench;
mod clip;
mod fonts;
mod grid;
mod menu;
mod open_files;
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
    let mut sheet = None;
    let mut screenshot_wait = None;
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
            Some("--screenshot-wait") => {
                let secs: f64 =
                    args.next().and_then(|s| s.to_str().and_then(|s| s.parse().ok())).unwrap_or(0.0);
                screenshot_wait = Some(std::time::Duration::from_secs_f64(secs.max(0.0)));
            }
            Some("--sheet") => sheet = args.next().and_then(|s| s.into_string().ok()),
            // Finder passes a process serial number on older systems.
            Some(s) if s.starts_with("-psn_") => {}
            _ => files.push(PathBuf::from(arg)),
        }
    }
    if let (Some(shot), Some(wait)) = (&mut shot, screenshot_wait) {
        shot.not_before = Some(started + wait);
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
    // Own the event loop (instead of eframe::run_native) so the app delegate
    // can be installed between creating it and running it; that is the only
    // moment that catches the files Finder asks a fresh launch to open.
    let event_loop = winit::event_loop::EventLoop::<eframe::UserEvent>::with_user_event()
        .build()
        .map_err(eframe::Error::WinitEventLoop)?;
    open_files::install();
    let mut app = eframe::create_native(
        "Rustsheet",
        options,
        Box::new(move |cc| Ok(Box::new(app::RustsheetApp::new(cc, files, bench, shot, select, sheet)))),
        &event_loop,
    );
    event_loop.run_app(&mut app).map_err(eframe::Error::WinitEventLoop)
}
