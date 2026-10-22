//! `--bench-scroll`: scrolls the first window programmatically and reports
//! startup, load and per-frame timings, used for the numbers in the README.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustsheet_core::Sheet;

use crate::view::SheetView;

pub struct Bench {
    process_start: Instant,
    frames: u32,
    out: Option<PathBuf>,
    seen: u32,
    first_frame: Option<Duration>,
    load: Option<(String, Duration, usize, usize)>,
    cpu_ms: Vec<f32>,
    interval_ms: Vec<f32>,
    last: Option<Instant>,
    finished: bool,
}

const WARMUP: u32 = 20;

impl Bench {
    pub fn new(process_start: Instant, frames: u32, out: Option<PathBuf>) -> Self {
        Self {
            process_start,
            frames,
            out,
            seen: 0,
            first_frame: None,
            load: None,
            cpu_ms: Vec::new(),
            interval_ms: Vec::new(),
            last: None,
            finished: false,
        }
    }

    pub fn loaded(&mut self, path: &Path, took: Duration, sheet: &Sheet) {
        self.load = Some((path.display().to_string(), took, sheet.cell_count(), sheet.formula_count()));
    }

    /// Advances one frame; returns true when the run is finished.
    /// `ready` is false while files are still loading; measuring starts after.
    pub fn step(&mut self, ctx: &egui::Context, frame: &eframe::Frame, view: &mut SheetView, ready: bool) -> bool {
        if self.finished {
            return true;
        }
        let now = Instant::now();
        if self.first_frame.is_none() {
            self.first_frame = Some(self.process_start.elapsed());
        }
        if !ready {
            ctx.request_repaint();
            return false;
        }
        self.seen += 1;
        if self.seen > WARMUP {
            if let Some(cpu) = frame.info().cpu_usage {
                self.cpu_ms.push(cpu * 1000.0);
            }
            if let Some(last) = self.last {
                self.interval_ms.push((now - last).as_secs_f32() * 1000.0);
            }
            let n = self.seen - WARMUP;
            if n < self.frames / 2 {
                // Smooth trackpad-like scrolling: about two rows per frame.
                view.scroll.1 += 45.0;
                if n.is_multiple_of(25) {
                    view.scroll.0 += 60.0;
                }
            } else {
                // Large jumps, like dragging the scrollbar.
                view.scroll.1 += 2_400.0;
            }
        }
        self.last = Some(now);
        ctx.request_repaint();
        if self.seen < self.frames + WARMUP {
            return false;
        }
        self.report();
        self.finished = true;
        true
    }

    fn report(&self) {
        let stats = |v: &[f32]| {
            if v.is_empty() {
                return "{}".to_string();
            }
            let mut s = v.to_vec();
            s.sort_by(f32::total_cmp);
            let pct = |p: f32| s[((s.len() - 1) as f32 * p).round() as usize];
            let mean = s.iter().sum::<f32>() / s.len() as f32;
            format!(
                "{{\"mean\": {mean:.2}, \"p50\": {:.2}, \"p95\": {:.2}, \"p99\": {:.2}, \"max\": {:.2}}}",
                pct(0.5),
                pct(0.95),
                pct(0.99),
                s[s.len() - 1]
            )
        };
        let load = self.load.as_ref().map_or("null".to_string(), |(path, took, cells, formulas)| {
            format!(
                "{{\"file\": {path:?}, \"ms\": {:.1}, \"cells\": {cells}, \"formulas\": {formulas}}}",
                took.as_secs_f64() * 1000.0
            )
        });
        let report = format!(
            "{{\n  \"first_frame_ms\": {:.1},\n  \"load\": {load},\n  \"frames\": {},\n  \"frame_cpu_ms\": {},\n  \"frame_interval_ms\": {}\n}}\n",
            self.first_frame.unwrap_or_default().as_secs_f64() * 1000.0,
            self.cpu_ms.len(),
            stats(&self.cpu_ms),
            stats(&self.interval_ms),
        );
        print!("{report}");
        if let Some(out) = &self.out {
            let _ = std::fs::write(out, &report);
        }
    }
}

/// `--screenshot <path>`: saves what the first window rendered, read back
/// from the GPU, once the window has settled.
pub struct Shot {
    pub path: PathBuf,
    pub after_frames: u32,
    pub quit: bool,
    seen: u32,
    requested: bool,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Shot {
    pub fn new(path: PathBuf, quit: bool) -> Self {
        Self { path, after_frames: 30, quit, seen: 0, requested: false, done: Default::default() }
    }

    /// Call once per frame; returns true when the app may quit. Frames are
    /// only counted once `ready` (files loaded).
    pub fn step(&mut self, ctx: &egui::Context, ready: bool) -> bool {
        if ready {
            self.seen += 1;
        }
        if !self.requested && self.seen >= self.after_frames {
            self.requested = true;
            let path = self.path.clone();
            let done = self.done.clone();
            let ctx2 = ctx.clone();
            ctx.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |image| {
                    if let Err(e) = save_png(&path, &image) {
                        eprintln!("schermafbeelding mislukt: {e}");
                    } else {
                        println!("schermafbeelding: {}", path.display());
                    }
                    done.store(true, std::sync::atomic::Ordering::SeqCst);
                    ctx2.request_repaint();
                })),
            );
        }
        // The screenshot arrives with a later frame, so keep frames coming.
        if !self.done.load(std::sync::atomic::Ordering::SeqCst) {
            ctx.request_repaint();
        }
        self.quit && self.done.load(std::sync::atomic::Ordering::SeqCst)
    }
}

fn save_png(path: &Path, image: &egui::ColorImage) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let [w, h] = image.size;
    let mut encoder = png::Encoder::new(file, w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect();
    writer.write_image_data(&bytes)?;
    Ok(())
}
