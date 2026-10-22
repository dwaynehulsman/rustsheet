//! Draws the Rustsheet icon as vectors at every size of a macOS iconset.
//!
//! cargo run -p rustsheet-icon -- <out.iconset>
//! iconutil -c icns <out.iconset> -o assets/AppIcon.icns

use std::path::PathBuf;

use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point, Rect, SpreadMode,
    Transform,
};

/// Design canvas; macOS icons put an 824 pt body inside a 1024 pt canvas.
const CANVAS: f32 = 1024.0;

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> tiny_skia::Path {
    // Cubic approximation of a continuous-corner rounded rectangle.
    let k = r * 0.552_284_8;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish().expect("valid path")
}

fn solid(r: u8, g: u8, b: u8, a: u8) -> Paint<'static> {
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    paint.set_color_rgba8(r, g, b, a);
    paint
}

fn gradient(top: Color, bottom: Color, y0: f32, y1: f32) -> Paint<'static> {
    let shader = LinearGradient::new(
        Point::from_xy(0.0, y0),
        Point::from_xy(0.0, y1),
        vec![GradientStop::new(0.0, top), GradientStop::new(1.0, bottom)],
        SpreadMode::Pad,
        Transform::identity(),
    )
    .expect("gradient");
    Paint { shader, anti_alias: true, ..Paint::default() }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> tiny_skia::Path {
    PathBuilder::from_rect(Rect::from_xywh(x, y, w, h).expect("rect"))
}

fn draw(size: u32) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("pixmap");
    let t = Transform::from_scale(size as f32 / CANVAS, size as f32 / CANVAS);
    let fill = |pm: &mut Pixmap, path: &tiny_skia::Path, paint: &Paint<'_>| {
        pm.fill_path(path, paint, FillRule::Winding, t, None);
    };

    // Body: deep spreadsheet green with a soft shadow below.
    let body = rounded_rect(100.0, 100.0, 824.0, 824.0, 186.0);
    let shadow = rounded_rect(100.0, 112.0, 824.0, 824.0, 186.0);
    fill(&mut pm, &shadow, &solid(0, 0, 0, 60));
    fill(
        &mut pm,
        &body,
        &gradient(Color::from_rgba8(46, 170, 98, 255), Color::from_rgba8(14, 112, 62, 255), 100.0, 924.0),
    );

    // The sheet: a white card with a header row and column.
    let (sx, sy, sw, sh) = (210.0, 230.0, 604.0, 520.0);
    fill(&mut pm, &rounded_rect(sx, sy + 10.0, sw, sh, 36.0), &solid(0, 50, 20, 70));
    fill(&mut pm, &rounded_rect(sx, sy, sw, sh, 36.0), &solid(255, 255, 255, 255));
    let header = 92.0;
    let left = 110.0;
    fill(&mut pm, &rect(sx + 18.0, sy + 18.0, sw - 36.0, header - 18.0), &solid(232, 244, 236, 255));
    fill(&mut pm, &rect(sx + 18.0, sy + header, left - 18.0, sh - header - 18.0), &solid(232, 244, 236, 255));

    // Grid lines.
    let line = solid(196, 214, 202, 255);
    let rows = 4;
    let cols = 3;
    let cell_h = (sh - header - 18.0) / rows as f32;
    let cell_w = (sw - left - 18.0) / cols as f32;
    for r in 0..=rows {
        let y = sy + header + r as f32 * cell_h;
        fill(&mut pm, &rect(sx + 18.0, y - 3.0, sw - 36.0, 6.0), &line);
    }
    for c in 0..=cols {
        let x = sx + left + c as f32 * cell_w;
        fill(&mut pm, &rect(x - 3.0, sy + 18.0, 6.0, sh - 36.0), &line);
    }

    // A selected cell in Rust orange, with a thicker outline.
    let (cx, cy) = (sx + left + cell_w, sy + header + cell_h);
    fill(&mut pm, &rect(cx, cy, cell_w, cell_h), &solid(247, 225, 210, 255));
    let outline = solid(222, 98, 40, 255);
    let wgt = 14.0;
    fill(&mut pm, &rect(cx - wgt / 2.0, cy - wgt / 2.0, cell_w + wgt, wgt), &outline);
    fill(&mut pm, &rect(cx - wgt / 2.0, cy + cell_h - wgt / 2.0, cell_w + wgt, wgt), &outline);
    fill(&mut pm, &rect(cx - wgt / 2.0, cy - wgt / 2.0, wgt, cell_h + wgt), &outline);
    fill(&mut pm, &rect(cx + cell_w - wgt / 2.0, cy - wgt / 2.0, wgt, cell_h + wgt), &outline);
    // Fill handle.
    fill(&mut pm, &rect(cx + cell_w - 16.0, cy + cell_h - 16.0, 32.0, 32.0), &outline);

    // Value bars in the cells below, like numbers in a column.
    let bar = solid(110, 140, 120, 255);
    for (r, frac) in [(0, 0.55), (2, 0.7), (3, 0.4)] {
        let y = sy + header + r as f32 * cell_h + cell_h / 2.0 - 9.0;
        let w = (cell_w - 50.0) * frac;
        fill(&mut pm, &rounded_rect(cx + cell_w - 25.0 - w, y, w, 18.0, 9.0), &bar);
    }
    pm
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "AppIcon.iconset".into()));
    std::fs::create_dir_all(&out).expect("create iconset dir");
    for base in [16u32, 32, 128, 256, 512] {
        for (scale, suffix) in [(1, ""), (2, "@2x")] {
            let px = base * scale;
            draw(px).save_png(out.join(format!("icon_{base}x{base}{suffix}.png"))).expect("write png");
        }
    }
    println!("{}", out.display());
}
