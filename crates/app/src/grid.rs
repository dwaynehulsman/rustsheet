//! The spreadsheet grid: virtual rendering of only the visible cells,
//! headers, selection, column resizing, scrollbars, mouse and keyboard.

use egui::{
    Align2, Color32, CornerRadius, CursorIcon, Event, EventFilter, FontId, Key, Pos2, Rect, Sense, Shape,
    Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};
use rustsheet_core::format::{Align as CellAlign, format_value};
use rustsheet_core::sheet::{DEFAULT_COL_WIDTH, DEFAULT_ROW_HEIGHT, MIN_COL_WIDTH, MIN_ROW_HEIGHT};
use rustsheet_core::{CellRef, MAX_COLS, MAX_ROWS, Range, Value, addr::col_name};

use crate::menu::Command;
use crate::view::{Env, Selection, SheetView, take_edit_keys, take_key};

pub const ROW_H: f32 = DEFAULT_ROW_HEIGHT;
pub const HEADER_H: f32 = 22.0;
pub const CELL_FONT: f32 = 13.0;
const PAD: f32 = 5.0;
const SCROLLBAR: f32 = 11.0;
const RESIZE_GRAB: f32 = 4.0;

pub struct Palette {
    pub background: Color32,
    pub chrome: Color32,
    grid: Color32,
    header_bg: Color32,
    header_text: Color32,
    header_active_bg: Color32,
    header_active_text: Color32,
    header_line: Color32,
    accent: Color32,
    selection: Color32,
    text: Color32,
    thumb: Color32,
}

impl Palette {
    pub fn new(dark: bool) -> Self {
        let accent = Color32::from_rgb(10, 132, 255);
        if dark {
            Self {
                background: Color32::from_rgb(30, 30, 32),
                chrome: Color32::from_rgb(44, 44, 47),
                grid: Color32::from_rgb(58, 58, 62),
                header_bg: Color32::from_rgb(40, 40, 43),
                header_text: Color32::from_rgb(170, 170, 176),
                header_active_bg: Color32::from_rgb(38, 64, 102),
                header_active_text: Color32::from_rgb(200, 225, 255),
                header_line: Color32::from_rgb(64, 64, 68),
                accent,
                selection: Color32::from_rgba_unmultiplied(10, 132, 255, 46),
                text: Color32::from_rgb(232, 232, 236),
                thumb: Color32::from_rgba_unmultiplied(255, 255, 255, 80),
            }
        } else {
            Self {
                background: Color32::WHITE,
                chrome: Color32::from_rgb(246, 246, 247),
                grid: Color32::from_rgb(226, 226, 229),
                header_bg: Color32::from_rgb(246, 246, 247),
                header_text: Color32::from_rgb(100, 100, 106),
                header_active_bg: Color32::from_rgb(220, 234, 252),
                header_active_text: Color32::from_rgb(10, 90, 190),
                header_line: Color32::from_rgb(214, 214, 218),
                accent,
                selection: Color32::from_rgba_unmultiplied(10, 132, 255, 30),
                text: Color32::from_rgb(28, 28, 30),
                thumb: Color32::from_rgba_unmultiplied(0, 0, 0, 90),
            }
        }
    }
}

/// What the pointer is doing in the grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drag {
    None,
    Cells,
    /// Clicking cells while typing a formula inserts references.
    Point,
    Cols,
    Rows,
    Resize {
        col: u32,
        original: Option<f32>,
        start_width: f32,
        grab_x: f32,
    },
    ResizeRow {
        row: u32,
        original: Option<f32>,
        start_height: f32,
        grab_y: f32,
    },
    ScrollV {
        grab: f32,
    },
    ScrollH {
        grab: f32,
    },
    /// Dragging the fill handle; `target` is the area that will be filled.
    Fill {
        target: Option<Range>,
    },
}

/// Screen layout of the visible part of the grid for one frame.
struct Geometry {
    /// Area for cells, right of the row header and below the column header.
    cells: Rect,
    col_header: Rect,
    row_header: Rect,
    corner: Rect,
    /// Visible columns: (index, left x, width).
    cols: Vec<(u32, f32, f32)>,
    /// Visible rows: (index, top y, height); hidden rows are left out.
    rows: Vec<(u32, f32, f32)>,
    vbar: Rect,
    hbar: Rect,
    /// Scrollable extents in points (below and right of frozen panes).
    extent: (f64, f64),
    /// Frozen rows and columns, and where the scrolled part starts on screen.
    frozen: (u32, u32),
    split: Pos2,
}

impl Geometry {
    /// The part of the cell area where cells of a frozen or scrolled row and
    /// column are drawn, so scrolled cells never cover frozen ones.
    fn band(&self, row: u32, col: u32) -> Rect {
        let (y0, y1) = if row < self.frozen.0 {
            (self.cells.top(), self.split.y)
        } else {
            (self.split.y, self.cells.bottom())
        };
        let (x0, x1) = if col < self.frozen.1 {
            (self.cells.left(), self.split.x)
        } else {
            (self.split.x, self.cells.right())
        };
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
    }
}

enum Zone {
    Corner,
    /// The small square at the bottom right of the selection.
    FillHandle,
    ColBorder(u32),
    RowBorder(u32),
    ColHeader(u32),
    RowHeader(u32),
    Cell(CellRef),
    VBar,
    HBar,
}

impl SheetView {
    /// Height and width of the frozen rows and columns.
    fn frozen_size(&self) -> (f64, f64) {
        let sheet = self.sheet();
        let (fr, fc) = sheet.freeze();
        (sheet.row_y(fr), sheet.col_x(fc))
    }

    /// Screen y of the top of `row`, given the cell area's top.
    fn screen_y(&self, top: f32, row: u32) -> f64 {
        let sheet = self.sheet();
        let fr = sheet.freeze().0;
        if row < fr {
            f64::from(top) + sheet.row_y(row)
        } else {
            f64::from(top) + sheet.row_y(fr) + sheet.row_y(row) - sheet.row_y(fr) - self.scroll.1
        }
    }

    /// Screen x of the left of `col`, given the cell area's left.
    fn screen_x(&self, left: f32, col: u32) -> f64 {
        let sheet = self.sheet();
        let fc = sheet.freeze().1;
        if col < fc {
            f64::from(left) + sheet.col_x(col)
        } else {
            f64::from(left) + sheet.col_x(fc) + sheet.col_x(col) - sheet.col_x(fc) - self.scroll.0
        }
    }

    fn row_header_width(&self) -> f32 {
        let sheet = self.sheet();
        let first = sheet.row_at_y(sheet.row_y(sheet.freeze().0) + self.scroll.1);
        let last = (first + 80).min(MAX_ROWS);
        let digits = last.to_string().len() as f32;
        (digits * 7.6 + 16.0).max(40.0)
    }

    /// Screen centre of the fill handle as laid out last frame.
    #[cfg(test)]
    pub fn fill_handle_center(&self) -> Option<Pos2> {
        self.fill_handle(&self.geometry(self.grid_rect)).map(|r| r.center())
    }

    /// Screen centre of a cell as laid out last frame.
    #[cfg(test)]
    pub fn cell_center(&self, at: CellRef) -> Pos2 {
        let g = self.geometry(self.grid_rect);
        self.range_rect(&g, Range::single(at)).center()
    }

    fn geometry(&self, rect: Rect) -> Geometry {
        let sheet = self.sheet();
        let rh = self.row_header_width();
        let cells = Rect::from_min_max(pos2(rect.left() + rh, rect.top() + HEADER_H), rect.max);
        let col_header = Rect::from_min_max(pos2(cells.left(), rect.top()), pos2(rect.right(), cells.top()));
        let row_header =
            Rect::from_min_max(pos2(rect.left(), cells.top()), pos2(cells.left(), rect.bottom()));
        let corner = Rect::from_min_max(rect.min, pos2(cells.left(), cells.top()));
        let (fr, fc) = sheet.freeze();
        let (fh, fw) = self.frozen_size();
        let split = pos2(
            (f64::from(cells.left()) + fw).min(f64::from(cells.right())) as f32,
            (f64::from(cells.top()) + fh).min(f64::from(cells.bottom())) as f32,
        );

        let mut cols = Vec::new();
        for col in 0..fc {
            let x = self.screen_x(cells.left(), col) as f32;
            if x >= cells.right() {
                break;
            }
            cols.push((col, x, sheet.col_width(col)));
        }
        let mut col = sheet.col_at_x(sheet.col_x(fc) + self.scroll.0).max(fc);
        let mut x = self.screen_x(cells.left(), col) as f32;
        while x < cells.right() && col < MAX_COLS {
            let w = sheet.col_width(col);
            cols.push((col, x, w));
            x += w;
            col += 1;
        }

        let mut rows = Vec::new();
        for row in 0..fr {
            let h = sheet.row_height(row);
            let y = self.screen_y(cells.top(), row) as f32;
            if y >= cells.bottom() {
                break;
            }
            if h > 0.0 {
                rows.push((row, y, h));
            }
        }
        let mut row = sheet.row_at_y(sheet.row_y(fr) + self.scroll.1).max(fr);
        let mut y = self.screen_y(cells.top(), row) as f32;
        while y < cells.bottom() && row < MAX_ROWS {
            let h = sheet.row_height(row);
            if h > 0.0 {
                rows.push((row, y, h));
            }
            y += h;
            row += 1;
        }

        // Scrollbars cover the data plus some room, growing as you scroll,
        // like Excel; the wheel can always go further.
        let used = sheet.used_range().map_or((0, 0), |r| (r.end.row, r.end.col));
        let last_row = rows.last().map_or(0, |r| r.0);
        let last_col = cols.last().map_or(0, |c| c.0);
        let extent_rows = (used.0 + 100).max(last_row + 1).min(MAX_ROWS);
        let extent_cols = (used.1 + 10).max(last_col + 1).min(MAX_COLS);
        let extent = (sheet.col_x(extent_cols) - fw, sheet.row_y(extent_rows) - fh);

        let vbar = Rect::from_min_max(
            pos2(cells.right() - SCROLLBAR, split.y),
            pos2(cells.right(), cells.bottom() - SCROLLBAR),
        );
        let hbar = Rect::from_min_max(
            pos2(split.x, cells.bottom() - SCROLLBAR),
            pos2(cells.right() - SCROLLBAR, cells.bottom()),
        );
        Geometry {
            cells,
            col_header,
            row_header,
            corner,
            cols,
            rows,
            vbar,
            hbar,
            extent,
            frozen: (fr, fc),
            split,
        }
    }

    /// Size of the scrolled part of the cell area.
    fn scroll_view(&self, g: &Geometry) -> (f64, f64) {
        (
            f64::from(g.cells.right() - g.split.x - SCROLLBAR).max(1.0),
            f64::from(g.cells.bottom() - g.split.y - SCROLLBAR).max(1.0),
        )
    }

    fn max_scroll(&self, g: &Geometry) -> (f64, f64) {
        let sheet = self.sheet();
        let (fh, fw) = self.frozen_size();
        let (view_w, view_h) = self.scroll_view(g);
        ((sheet.col_x(MAX_COLS) - fw - view_w).max(0.0), (sheet.row_y(MAX_ROWS) - fh - view_h).max(0.0))
    }

    fn clamp_scroll(&mut self, g: &Geometry) {
        let (mx, my) = self.max_scroll(g);
        self.scroll.0 = self.scroll.0.clamp(0.0, mx);
        self.scroll.1 = self.scroll.1.clamp(0.0, my);
    }

    /// Scrolls the minimum needed to show `at` (frozen cells are always shown).
    fn reveal(&mut self, at: CellRef, g: &Geometry) {
        let (fr, fc) = g.frozen;
        let (fh, fw) = self.frozen_size();
        let (view_w, view_h) = self.scroll_view(g);
        let sheet = self.sheet();
        let x0 = sheet.col_x(at.col) - fw;
        let x1 = x0 + f64::from(sheet.col_width(at.col));
        let y0 = sheet.row_y(at.row) - fh;
        let y1 = y0 + f64::from(sheet.row_height(at.row));
        if at.col >= fc {
            if x0 < self.scroll.0 {
                self.scroll.0 = x0;
            } else if x1 > self.scroll.0 + view_w {
                self.scroll.0 = (x1 - view_w).min(x0);
            }
        }
        if at.row >= fr {
            if y0 < self.scroll.1 {
                self.scroll.1 = y0;
            } else if y1 > self.scroll.1 + view_h {
                self.scroll.1 = y1 - view_h;
            }
        }
    }

    /// Cell under a screen position, clamped to the grid.
    fn cell_at(&self, g: &Geometry, pos: Pos2) -> CellRef {
        let sheet = self.sheet();
        let (fr, fc) = g.frozen;
        let (fh, fw) = self.frozen_size();
        let dy = f64::from(pos.y - g.cells.top());
        let row = if pos.y < g.split.y {
            sheet.row_at_y(dy).min(fr.saturating_sub(1))
        } else {
            sheet.row_at_y(sheet.row_y(fr) + self.scroll.1 + dy - fh).max(fr)
        };
        let dx = f64::from(pos.x - g.cells.left());
        let col = if pos.x < g.split.x {
            sheet.col_at_x(dx).min(fc.saturating_sub(1))
        } else {
            sheet.col_at_x(sheet.col_x(fc) + self.scroll.0 + dx - fw).max(fc)
        };
        CellRef::new(row, col)
    }

    /// Screen rectangle of a range, clamped near the visible area so huge
    /// ranges stay within f32 precision.
    fn range_rect(&self, g: &Geometry, r: Range) -> Rect {
        let sheet = self.sheet();
        let margin = 8.0;
        let clamp_x =
            |x: f64| x.clamp(f64::from(g.cells.left()) - margin, f64::from(g.cells.right()) + margin) as f32;
        let clamp_y =
            |y: f64| y.clamp(f64::from(g.cells.top()) - margin, f64::from(g.cells.bottom()) + margin) as f32;
        let x0 = self.screen_x(g.cells.left(), r.start.col);
        let x1 = self.screen_x(g.cells.left(), r.end.col) + f64::from(sheet.col_width(r.end.col));
        let y0 = self.screen_y(g.cells.top(), r.start.row);
        let y1 = self.screen_y(g.cells.top(), r.end.row) + f64::from(sheet.row_height(r.end.row));
        Rect::from_min_max(pos2(clamp_x(x0), clamp_y(y0)), pos2(clamp_x(x1), clamp_y(y1)))
    }

    /// Where drawing for range `r` may appear: a range entirely in the
    /// scrolled part must not cover the frozen panes, and the other way round.
    fn range_clip(&self, g: &Geometry, r: Range) -> Rect {
        let (fr, fc) = g.frozen;
        let (y0, y1) = if r.start.row >= fr {
            (g.split.y, g.cells.bottom())
        } else if r.end.row < fr {
            (g.cells.top(), g.split.y)
        } else {
            (g.cells.top(), g.cells.bottom())
        };
        let (x0, x1) = if r.start.col >= fc {
            (g.split.x, g.cells.right())
        } else if r.end.col < fc {
            (g.cells.left(), g.split.x)
        } else {
            (g.cells.left(), g.cells.right())
        };
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
    }

    fn zone(&self, g: &Geometry, pos: Pos2) -> Option<Zone> {
        if g.corner.contains(pos) {
            return Some(Zone::Corner);
        }
        if g.col_header.contains(pos) {
            for &(col, x, w) in &g.cols {
                if (pos.x - (x + w)).abs() <= RESIZE_GRAB {
                    return Some(Zone::ColBorder(col));
                }
            }
            return Some(Zone::ColHeader(self.cell_at(g, pos).col));
        }
        if g.row_header.contains(pos) {
            for &(row, y, h) in &g.rows {
                if (pos.y - (y + h)).abs() <= RESIZE_GRAB.min(h / 3.0) {
                    return Some(Zone::RowBorder(row));
                }
            }
            return Some(Zone::RowHeader(self.cell_at(g, pos).row));
        }
        if g.vbar.contains(pos) {
            return Some(Zone::VBar);
        }
        if g.hbar.contains(pos) {
            return Some(Zone::HBar);
        }
        if self.fill_handle(g).is_some_and(|h| h.expand(3.0).contains(pos)) {
            return Some(Zone::FillHandle);
        }
        g.cells.contains(pos).then(|| Zone::Cell(self.cell_at(g, pos)))
    }

    /// Where the fill handle is drawn, when the selection's corner is visible.
    fn fill_handle(&self, g: &Geometry) -> Option<Rect> {
        let sel = self.sel.range();
        if sel.rows() == MAX_ROWS || sel.cols() == MAX_COLS || self.edit.is_some() {
            return None;
        }
        let corner = self.range_rect(g, sel).right_bottom();
        let rect = Rect::from_center_size(corner - vec2(1.0, 1.0), vec2(7.0, 7.0));
        g.cells.contains(corner - vec2(2.0, 2.0)).then_some(rect)
    }

    /// Thumb rectangle and the scroll range it maps to.
    fn thumb(&self, g: &Geometry, vertical: bool) -> (Rect, f64) {
        let (track, extent, view, pos) = if vertical {
            (
                g.vbar,
                g.extent.1.max(self.scroll.1 + f64::from(g.cells.height())),
                f64::from(g.cells.height()),
                self.scroll.1,
            )
        } else {
            (
                g.hbar,
                g.extent.0.max(self.scroll.0 + f64::from(g.cells.width())),
                f64::from(g.cells.width()),
                self.scroll.0,
            )
        };
        let len = if vertical { track.height() } else { track.width() };
        let thumb_len = ((view / extent) as f32 * len).clamp(24.0, len);
        let range = (extent - view).max(1.0);
        let start = (pos / range) as f32 * (len - thumb_len);
        let rect = if vertical {
            Rect::from_min_size(
                pos2(track.left() + 2.0, track.top() + start),
                vec2(SCROLLBAR - 4.0, thumb_len),
            )
        } else {
            Rect::from_min_size(
                pos2(track.left() + start, track.top() + 2.0),
                vec2(thumb_len, SCROLLBAR - 4.0),
            )
        };
        (rect, range)
    }

    pub fn grid(&mut self, ui: &mut Ui, env: &mut Env<'_>) {
        let (grid_id, editor_id, bar_id, _) = self.ids(env.window);
        let rect = ui.available_rect_before_wrap();
        self.grid_rect = rect;
        let response = ui.interact(rect, grid_id, Sense::click_and_drag());
        ui.advance_cursor_after_rect(rect);
        crate::view::accessible_name(ui, &response, "Werkblad");

        if self.grid_focus_pending && self.edit.is_none() {
            response.request_focus();
            self.grid_focus_pending = false;
        }
        let focused = response.has_focus();
        if focused {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    grid_id,
                    EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
                )
            });
        }

        // Trackpad and wheel scrolling.
        if response.contains_pointer() {
            let delta = ui.input(|i| i.smooth_scroll_delta());
            if delta != Vec2::ZERO {
                self.scroll.0 -= f64::from(delta.x);
                self.scroll.1 -= f64::from(delta.y);
            }
        }

        if focused && self.edit.is_none() {
            self.keyboard(ui, env);
        }

        let mut g = self.geometry(rect);
        self.clamp_scroll(&g);
        self.page_rows = (g.cells.height() / ROW_H).max(1.0) as u32 - 1;
        if self.scroll_into_view {
            let target = if self.sel.corner != self.sel.active { self.sel.corner } else { self.sel.active };
            self.reveal(target, &g);
            self.clamp_scroll(&g);
            self.scroll_into_view = false;
        }
        g = self.geometry(rect);

        self.pointer(ui, &response, &g, env);
        g = self.geometry(rect);

        self.context_menu(&response);

        let palette = Palette::new(ui.visuals().dark_mode);
        self.paint(ui, &g, &palette, env);
        self.editor(ui, &g, &palette, editor_id, bar_id);
    }

    fn keyboard(&mut self, ui: &mut Ui, env: &mut Env<'_>) {
        let events = ui.input(|i| i.events.clone());
        for event in events {
            match event {
                Event::Key { key, pressed: true, modifiers: m, .. } => match key {
                    Key::ArrowUp => self.move_by(-1, 0, m.shift, m.command),
                    Key::ArrowDown => self.move_by(1, 0, m.shift, m.command),
                    Key::ArrowLeft => self.move_by(0, -1, m.shift, m.command),
                    Key::ArrowRight => self.move_by(0, 1, m.shift, m.command),
                    Key::Tab => self.advance(0, if m.shift { -1 } else { 1 }),
                    Key::Enter if m.shift => self.advance(-1, 0),
                    Key::Enter | Key::F2 => {
                        take_key(ui, key);
                        self.begin_edit(None, false);
                        return;
                    }
                    Key::Delete | Key::Backspace => self.push_command(crate::menu::Command::Delete),
                    Key::Escape => env.clip.cancel_cut(),
                    Key::PageDown => self.move_by(i64::from(self.page_rows), 0, m.shift, false),
                    Key::PageUp => self.move_by(-i64::from(self.page_rows), 0, m.shift, false),
                    Key::Home if m.command => self.select(CellRef::default(), m.shift),
                    Key::Home => {
                        let at = if m.shift { self.sel.corner } else { self.sel.active };
                        self.select(CellRef::new(at.row, 0), m.shift);
                    }
                    Key::End if m.command => {
                        if let Some(used) = self.sheet().used_range() {
                            self.select(used.end, m.shift);
                        }
                    }
                    _ => {}
                },
                Event::Text(text) if !text.chars().all(char::is_control) => {
                    // Typing over a cell starts editing with what was typed.
                    ui.ctx().input_mut(|i| i.events.retain(|e| !matches!(e, Event::Text(_))));
                    self.begin_edit(Some(text), false);
                    return;
                }
                _ => {}
            }
        }
    }

    fn pointer(&mut self, ui: &mut Ui, response: &egui::Response, g: &Geometry, env: &mut Env<'_>) {
        let (pos, pressed, down, released, shift) = ui.input(|i| {
            (
                i.pointer.interact_pos(),
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.modifiers.shift,
            )
        });
        let Some(pos) = pos else { return };
        let zone =
            if response.contains_pointer() || self.drag != Drag::None { self.zone(g, pos) } else { None };

        // Cursor feedback.
        match (&self.drag, &zone) {
            (Drag::Resize { .. }, _) | (Drag::None, Some(Zone::ColBorder(_))) => {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal)
            }
            (Drag::ResizeRow { .. }, _) | (Drag::None, Some(Zone::RowBorder(_))) => {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical)
            }
            (Drag::None, Some(Zone::FillHandle)) | (Drag::Fill { .. }, _) => {
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair)
            }
            (Drag::None, Some(Zone::Cell(_))) | (Drag::Cells, _) => {
                ui.ctx().set_cursor_icon(CursorIcon::Cell)
            }
            _ => {}
        }

        // Right-click outside the selection selects what was clicked first.
        let secondary = ui.input(|i| i.pointer.secondary_pressed());
        if secondary && response.contains_pointer() && self.edit.is_none() {
            let sel = self.sel.range();
            match zone {
                Some(Zone::Cell(at)) if !sel.contains(at) => self.select(at, false),
                Some(Zone::ColHeader(col))
                    if !(sel.start.col..=sel.end.col).contains(&col) || sel.rows() < MAX_ROWS =>
                {
                    let top = g.rows.first().map_or(0, |r| r.0);
                    self.sel = Selection {
                        active: CellRef::new(top, col),
                        anchor: CellRef::new(0, col),
                        corner: CellRef::new(MAX_ROWS - 1, col),
                    };
                }
                Some(Zone::RowHeader(row))
                    if !(sel.start.row..=sel.end.row).contains(&row) || sel.cols() < MAX_COLS =>
                {
                    let left = g.cols.first().map_or(0, |c| c.0);
                    self.sel = Selection {
                        active: CellRef::new(row, left),
                        anchor: CellRef::new(row, 0),
                        corner: CellRef::new(row, MAX_COLS - 1),
                    };
                }
                _ => {}
            }
            self.scroll_into_view = false;
        }

        if pressed && response.contains_pointer() {
            self.press(ui, zone, pos, shift, g, env);
        } else if down && self.drag != Drag::None {
            self.drag_to(pos, g);
        }
        if released {
            if let Drag::ResizeRow { row, original, .. } = self.drag {
                let height = self.sheet().custom_row_height(row);
                if height != original {
                    self.set_row_height(row, original, height);
                }
            }
            if let Drag::Resize { col, original, .. } = self.drag {
                let width = self.sheet().custom_col_width(col);
                if width != original {
                    self.set_column_width(col, original, width);
                }
            }
            if self.drag == Drag::Point {
                self.end_point();
            }
            if let Drag::Fill { target: Some(target) } = self.drag {
                let copy_only = ui.input(|i| i.modifiers.alt);
                self.fill_into(target, !copy_only);
            }
            self.drag = Drag::None;
        }

        let double = ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary));
        if double && response.contains_pointer() {
            match self.zone(g, pos) {
                Some(Zone::Cell(at)) if self.edit.is_none() && at == self.sel.active => {
                    self.begin_edit(None, false)
                }
                Some(Zone::ColBorder(col)) => self.autofit(col),
                Some(Zone::RowBorder(row)) => {
                    let original = self.sheet().custom_row_height(row);
                    if original.is_some() {
                        self.set_row_height(row, original, None);
                    }
                }
                _ => {}
            }
        }
    }

    /// Right-click menu: clipboard, inserting and deleting rows and columns.
    fn context_menu(&mut self, response: &egui::Response) {
        let (row_at, rows) = self.line_span(true, true);
        let (col_at, cols) = self.line_span(false, true);
        let (_, del_rows) = self.line_span(true, false);
        let (_, del_cols) = self.line_span(false, false);
        let plural =
            |n: u32, one: &str, many: &str| if n == 1 { one.to_string() } else { format!("{n} {many}") };
        let mut chosen = None;
        response.context_menu(|ui| {
            let mut entry = |ui: &mut Ui, label: String, command: Command| {
                if ui.button(label).clicked() {
                    chosen = Some(command);
                    ui.close();
                }
            };
            entry(ui, "Knip".into(), Command::Cut);
            entry(ui, "Kopieer".into(), Command::Copy);
            entry(ui, "Plak".into(), Command::Paste);
            ui.separator();
            entry(
                ui,
                format!("Voeg {} in boven rij {}", plural(rows, "rij", "rijen"), row_at + 1),
                Command::InsertRows,
            );
            entry(
                ui,
                format!("Voeg {} in links van {}", plural(cols, "kolom", "kolommen"), col_name(col_at)),
                Command::InsertCols,
            );
            entry(ui, format!("Verwijder {}", plural(del_rows, "rij", "rijen")), Command::DeleteRows);
            entry(ui, format!("Verwijder {}", plural(del_cols, "kolom", "kolommen")), Command::DeleteCols);
            ui.separator();
            entry(ui, "Sorteer oplopend".into(), Command::SortAsc);
            entry(ui, "Sorteer aflopend".into(), Command::SortDesc);
            ui.separator();
            entry(ui, "Wis inhoud".into(), Command::Delete);
        });
        if let Some(command) = chosen {
            self.push_command(command);
        }
    }

    fn press(
        &mut self,
        ui: &mut Ui,
        zone: Option<Zone>,
        pos: Pos2,
        shift: bool,
        g: &Geometry,
        env: &mut Env<'_>,
    ) {
        let Some(zone) = zone else { return };
        let first_visible_row = g.rows.first().map_or(0, |r| r.0);
        let first_visible_col = g.cols.first().map_or(0, |c| c.0);
        // Clicking anywhere but a cell reference target ends an edit.
        let pointing = matches!(zone, Zone::Cell(at) if self.wants_reference()
            && self.edit.as_ref().is_some_and(|e| e.cell != at || e.sheet != self.sheet));
        if !pointing
            && !matches!(zone, Zone::VBar | Zone::HBar | Zone::ColBorder(_) | Zone::RowBorder(_))
            && self.edit.is_some()
            && !self.commit_edit()
        {
            return;
        }
        match zone {
            Zone::Corner => {
                self.sel = Selection::span(CellRef::default(), CellRef::new(MAX_ROWS - 1, MAX_COLS - 1));
            }
            Zone::RowBorder(row) => {
                let height = self.sheet().row_height(row);
                self.drag = Drag::ResizeRow {
                    row,
                    original: self.sheet().custom_row_height(row),
                    start_height: height,
                    grab_y: pos.y,
                };
            }
            Zone::ColBorder(col) => {
                let width = self.sheet().col_width(col);
                self.drag = Drag::Resize {
                    col,
                    original: self.sheet().custom_col_width(col),
                    start_width: width,
                    grab_x: pos.x,
                };
            }
            Zone::ColHeader(col) => {
                // Whole columns; the active cell is the top visible one, like Excel.
                if shift {
                    self.sel.anchor.row = 0;
                    self.sel.corner = CellRef::new(MAX_ROWS - 1, col);
                } else {
                    self.sel = Selection {
                        active: CellRef::new(first_visible_row, col),
                        anchor: CellRef::new(0, col),
                        corner: CellRef::new(MAX_ROWS - 1, col),
                    };
                }
                self.drag = Drag::Cols;
            }
            Zone::RowHeader(row) => {
                if shift {
                    self.sel.anchor.col = 0;
                    self.sel.corner = CellRef::new(row, MAX_COLS - 1);
                } else {
                    self.sel = Selection {
                        active: CellRef::new(row, first_visible_col),
                        anchor: CellRef::new(row, 0),
                        corner: CellRef::new(row, MAX_COLS - 1),
                    };
                }
                self.drag = Drag::Rows;
            }
            Zone::Cell(at) => {
                if pointing {
                    self.point_at(at, at);
                    self.drag = Drag::Point;
                    return;
                }
                self.select(at, shift);
                self.scroll_into_view = false;
                self.drag = Drag::Cells;
                env.clip.cut();
            }
            Zone::FillHandle => self.drag = Drag::Fill { target: None },
            Zone::VBar | Zone::HBar => {
                let vertical = matches!(zone, Zone::VBar);
                let (thumb, range) = self.thumb(g, vertical);
                let grab = if thumb.contains(pos) {
                    if vertical { pos.y - thumb.top() } else { pos.x - thumb.left() }
                } else {
                    // Jump so the thumb centres under the pointer.
                    let (len, track_start, thumb_len) = if vertical {
                        (g.vbar.height(), g.vbar.top(), thumb.height())
                    } else {
                        (g.hbar.width(), g.hbar.left(), thumb.width())
                    };
                    let p = if vertical { pos.y } else { pos.x };
                    let frac =
                        ((p - track_start - thumb_len / 2.0) / (len - thumb_len).max(1.0)).clamp(0.0, 1.0);
                    if vertical {
                        self.scroll.1 = f64::from(frac) * range;
                    } else {
                        self.scroll.0 = f64::from(frac) * range;
                    }
                    thumb_len / 2.0
                };
                self.drag = if vertical { Drag::ScrollV { grab } } else { Drag::ScrollH { grab } };
            }
        }
        if !matches!(self.drag, Drag::Point) && self.edit.is_none() {
            ui.memory_mut(|m| m.request_focus(egui::Id::unique(("grid", env.window))));
        }
    }

    fn drag_to(&mut self, pos: Pos2, g: &Geometry) {
        // Auto-scroll when dragging past the edge.
        let auto = |p: f32, lo: f32, hi: f32| -> f64 {
            if p < lo {
                f64::from(p - lo)
            } else if p > hi {
                f64::from(p - hi)
            } else {
                0.0
            }
        };
        let selecting =
            matches!(self.drag, Drag::Cells | Drag::Point | Drag::Cols | Drag::Rows | Drag::Fill { .. });
        if selecting {
            self.scroll.0 += auto(pos.x, g.cells.left(), g.cells.right()) * 0.5;
            self.scroll.1 += auto(pos.y, g.cells.top(), g.cells.bottom()) * 0.5;
        }
        let clamped = pos2(
            pos.x.clamp(g.cells.left() + 1.0, g.cells.right() - 1.0),
            pos.y.clamp(g.cells.top() + 1.0, g.cells.bottom() - 1.0),
        );
        let at = self.cell_at(g, clamped);
        match self.drag {
            Drag::Cells => self.sel.corner = at,
            Drag::Point => {
                if let Some((_, start)) = self.edit.as_ref().and_then(|e| e.point) {
                    self.point_at(start, at);
                }
            }
            Drag::Cols => self.sel.corner = CellRef::new(MAX_ROWS - 1, at.col),
            Drag::Rows => self.sel.corner = CellRef::new(at.row, MAX_COLS - 1),
            Drag::Resize { col, start_width, grab_x, .. } => {
                let width = (start_width + pos.x - grab_x).max(MIN_COL_WIDTH);
                self.preview_column_width(col, Some(width));
            }
            Drag::ResizeRow { row, start_height, grab_y, .. } => {
                let height = (start_height + pos.y - grab_y).max(MIN_ROW_HEIGHT);
                self.preview_row_height(row, Some(height));
            }
            Drag::ScrollV { grab } => {
                let (thumb, range) = self.thumb(g, true);
                let frac = (pos.y - grab - g.vbar.top()) / (g.vbar.height() - thumb.height()).max(1.0);
                self.scroll.1 = f64::from(frac.clamp(0.0, 1.0)) * range;
            }
            Drag::ScrollH { grab } => {
                let (thumb, range) = self.thumb(g, false);
                let frac = (pos.x - grab - g.hbar.left()) / (g.hbar.width() - thumb.width()).max(1.0);
                self.scroll.0 = f64::from(frac.clamp(0.0, 1.0)) * range;
            }
            Drag::Fill { .. } => {
                // Grow along whichever axis the pointer left the selection furthest.
                let sel = self.sel.range();
                let below = i64::from(at.row) - i64::from(sel.end.row);
                let above = i64::from(sel.start.row) - i64::from(at.row);
                let right = i64::from(at.col) - i64::from(sel.end.col);
                let left = i64::from(sel.start.col) - i64::from(at.col);
                let best = below.max(above).max(right).max(left);
                let extended = if best <= 0 {
                    None
                } else if best == below {
                    Some(Range::new(sel.start, CellRef::new(at.row, sel.end.col)))
                } else if best == above {
                    Some(Range::new(CellRef::new(at.row, sel.start.col), sel.end))
                } else if best == right {
                    Some(Range::new(sel.start, CellRef::new(sel.end.row, at.col)))
                } else {
                    Some(Range::new(CellRef::new(sel.start.row, at.col), sel.end))
                };
                self.drag =
                    Drag::Fill { target: extended.and_then(|e| rustsheet_core::fill::fill_target(sel, e)) };
            }
            Drag::None => {}
        }
    }

    /// Fits a column to its widest value, looking at a bounded number of cells.
    fn autofit(&mut self, col: u32) {
        let range = Range::new(CellRef::new(0, col), CellRef::new(MAX_ROWS - 1, col));
        let mut widest: f32 = 0.0;
        for (_, cell) in self.sheet().cells_in(range).take(5000) {
            let text = format_value(&cell.value, cell.format.num);
            // Average SF glyph width at 13pt; avoids laying out thousands of strings.
            widest =
                widest.max(text.chars().count() as f32 * 7.2 * if cell.format.bold { 1.08 } else { 1.0 });
        }
        let original = self.sheet().custom_col_width(col);
        let width = if widest == 0.0 { None } else { Some((widest + 2.0 * PAD + 4.0).max(MIN_COL_WIDTH)) };
        self.preview_column_width(col, width);
        self.set_column_width(col, original, width.filter(|w| (*w - DEFAULT_COL_WIDTH).abs() > 0.5));
    }

    // ----- painting -----

    fn paint(&self, ui: &Ui, g: &Geometry, p: &Palette, env: &Env<'_>) {
        let painter = ui.painter().with_clip_rect(g.cells.union(g.col_header).union(g.row_header));
        let ppp = ui.ctx().pixels_per_point();
        let hair = Stroke::new(1.0 / ppp, p.grid);
        let snap = |v: f32| (v * ppp).round() / ppp - 0.5 / ppp;
        let sheet = self.sheet();
        let cells_painter = painter.with_clip_rect(g.cells);
        let (fr, fc) = g.frozen;

        cells_painter.rect_filled(g.cells, 0.0, p.background);

        // Gridlines; scrolled lines never cross the frozen panes.
        for &(col, x, w) in &g.cols {
            let x = snap(x + w);
            if col >= fc && x < g.split.x {
                continue;
            }
            cells_painter.line_segment([pos2(x, g.cells.top()), pos2(x, g.cells.bottom())], hair);
        }
        for &(row, y, h) in &g.rows {
            let y = snap(y + h);
            if row >= fr && y < g.split.y {
                continue;
            }
            cells_painter.line_segment([pos2(g.cells.left(), y), pos2(g.cells.right(), y)], hair);
        }

        // Background fills cover the gridlines, as in Excel.
        let fill_of = |at: CellRef| {
            sheet.cell(at).and_then(|c| c.format.fill).map(|f| Color32::from_rgb(f.0, f.1, f.2))
        };
        for &(row, y, h) in &g.rows {
            for &(col, x, w) in &g.cols {
                if let Some(fill) = fill_of(CellRef::new(row, col)) {
                    let r = Rect::from_min_size(pos2(x, y), vec2(w, h));
                    cells_painter.with_clip_rect(g.band(row, col)).rect_filled(r, 0.0, fill);
                }
            }
        }

        // Selection tint, with the active cell left clear like Excel.
        let sel = self.sel.range();
        let sel_rect = self.range_rect(g, sel);
        let sel_painter = cells_painter.with_clip_rect(self.range_clip(g, sel).intersect(g.cells));
        if sel.len() > 1 {
            sel_painter.rect_filled(sel_rect, 0.0, p.selection);
            let active = self.range_rect(g, Range::single(self.sel.active));
            let under = fill_of(self.sel.active).unwrap_or(p.background);
            cells_painter.with_clip_rect(g.band(self.sel.active.row, self.sel.active.col)).rect_filled(
                active.shrink(1.0),
                0.0,
                under,
            );
        }

        // Cell text.
        let here = |e: &&crate::view::Edit| e.sheet == self.sheet;
        let editing_cell = self.edit.as_ref().filter(here).filter(|e| !e.in_bar).map(|e| e.cell);
        let last_col = g.cols.last().map_or(0, |c| c.0);
        for &(row, y, h) in &g.rows {
            for (ci, &(col, x, w)) in g.cols.iter().enumerate() {
                let at = CellRef::new(row, col);
                if Some(at) == editing_cell {
                    continue;
                }
                let band = g.band(row, col);
                let bar_text = self
                    .edit
                    .as_ref()
                    .filter(here)
                    .filter(|e| e.in_bar && e.cell == at)
                    .map(|e| e.text.clone());
                let Some(cell) = sheet.cell(at) else {
                    if let Some(text) = bar_text {
                        let galley = painter.layout_no_wrap(text, FontId::proportional(CELL_FONT), p.text);
                        let pos = pos2(x + PAD, y + (h - galley.size().y) / 2.0);
                        let clip = Rect::from_min_size(pos2(x, y), vec2(w, h)).intersect(band);
                        cells_painter.with_clip_rect(clip).galley(pos, galley, p.text);
                    }
                    continue;
                };
                if cell.value.is_empty() && bar_text.is_none() {
                    continue;
                }
                let family = crate::fonts::family(cell.format.bold, cell.format.italic);
                let text_color = cell.format.color.map_or(p.text, |c| Color32::from_rgb(c.0, c.1, c.2));
                let font = FontId::new(CELL_FONT, family);
                let (text, align) = match bar_text {
                    Some(t) => (t, CellAlign::Left),
                    None => {
                        (format_value(&cell.value, cell.format.num), cell.format.effective_align(&cell.value))
                    }
                };
                let mut galley = painter.layout_no_wrap(text, font.clone(), text_color);
                let avail = w - 2.0 * PAD;
                if matches!(cell.value, Value::Number(_)) && galley.size().x > avail {
                    // Numbers never get cut off silently; Excel shows hashes.
                    let n = (avail / 7.0).max(1.0) as usize;
                    galley = painter.layout_no_wrap("#".repeat(n), font, text_color);
                }
                let gw = galley.size().x;
                let tx = match align {
                    CellAlign::Right => x + w - PAD - gw,
                    CellAlign::Center => x + (w - gw) / 2.0,
                    _ => x + PAD,
                };
                let cell_rect = Rect::from_min_size(pos2(x, y), vec2(w, h));
                let mut clip = cell_rect;
                // Left-aligned text spills into empty cells on the right.
                if matches!(align, CellAlign::Left | CellAlign::General) && tx + gw > x + w {
                    let mut right = x + w;
                    for &(next_col, nx, nw) in &g.cols[ci + 1..] {
                        if right >= tx + gw + PAD || next_col > last_col || (col < fc) != (next_col < fc) {
                            break;
                        }
                        let next = sheet.cell(CellRef::new(row, next_col));
                        if next.is_some_and(|c| !c.value.is_empty() || c.format.fill.is_some()) {
                            break;
                        }
                        right = nx + nw;
                    }
                    if right > x + w {
                        clip.max.x = right;
                        let cover =
                            Rect::from_min_max(pos2(x + w - 1.0, y + 0.5), pos2(right - 1.0, y + h - 1.0));
                        cells_painter.with_clip_rect(band).rect_filled(cover, 0.0, p.background);
                        if sel.len() > 1 {
                            cells_painter.with_clip_rect(sel_rect.intersect(band)).rect_filled(
                                cover,
                                0.0,
                                p.selection,
                            );
                        }
                    }
                }
                let pos = pos2(tx, y + (h - galley.size().y) / 2.0);
                cells_painter.with_clip_rect(clip.intersect(band)).galley(pos, galley, text_color);
            }
        }

        // Lines where the panes are frozen.
        let freeze_line = Stroke::new(1.0, p.header_line);
        if fr > 0 {
            cells_painter.line_segment(
                [pos2(g.cells.left(), g.split.y), pos2(g.cells.right(), g.split.y)],
                freeze_line,
            );
        }
        if fc > 0 {
            cells_painter.line_segment(
                [pos2(g.split.x, g.cells.top()), pos2(g.split.x, g.cells.bottom())],
                freeze_line,
            );
        }

        // Selection border and active cell.
        let border = Stroke::new(2.0, p.accent);
        sel_painter.rect_stroke(sel_rect, 0.0, border, StrokeKind::Inside);
        if sel.len() > 1 {
            let active = self.range_rect(g, Range::single(self.sel.active));
            cells_painter.with_clip_rect(g.band(self.sel.active.row, self.sel.active.col)).rect_stroke(
                active,
                0.0,
                Stroke::new(1.0, p.accent),
                StrokeKind::Inside,
            );
        }

        // Fill handle, and the area a fill drag will cover.
        if let Some(handle) = self.fill_handle(g) {
            sel_painter.rect_filled(handle, 1.0, p.accent);
            sel_painter.rect_stroke(handle, 1.0, Stroke::new(1.0, p.background), StrokeKind::Outside);
        }
        if let Drag::Fill { target: Some(target) } = self.drag {
            let all = sel.union(&target);
            let r = self.range_rect(g, all).shrink(1.0);
            let path = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
            cells_painter.with_clip_rect(self.range_clip(g, all)).extend(Shape::dashed_line(
                &path,
                Stroke::new(1.5, p.accent),
                3.0,
                3.0,
            ));
        }

        // Marching border around a pending cut or copy source.
        if let Some(cut) = env.clip.cut().filter(|c| c.window == env.window && c.sheet == self.sheet) {
            let r = self.range_rect(g, cut.range).shrink(1.0);
            let path = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
            cells_painter.with_clip_rect(self.range_clip(g, cut.range)).extend(Shape::dashed_line(
                &path,
                Stroke::new(1.5, p.accent),
                4.0,
                3.0,
            ));
        }

        self.paint_headers(ui, g, p, sel, hair);
        self.paint_scrollbars(ui, g, p);
    }

    fn paint_headers(&self, ui: &Ui, g: &Geometry, p: &Palette, sel: Range, hair: Stroke) {
        let painter = ui.painter();
        let font = FontId::proportional(11.5);
        let line = Stroke::new(hair.width, p.header_line);
        let (fr, fc) = g.frozen;

        let hp = painter.with_clip_rect(g.col_header);
        hp.rect_filled(g.col_header, 0.0, p.header_bg);
        for &(col, x, w) in &g.cols {
            let r = Rect::from_min_size(pos2(x, g.col_header.top()), vec2(w, HEADER_H));
            let band = if col < fc {
                Rect::from_min_max(g.col_header.min, pos2(g.split.x, g.col_header.bottom()))
            } else {
                Rect::from_min_max(pos2(g.split.x, g.col_header.top()), g.col_header.max)
            };
            let hp = hp.with_clip_rect(band);
            let active = (sel.start.col..=sel.end.col).contains(&col);
            if active {
                hp.rect_filled(r, 0.0, p.header_active_bg);
            }
            let color = if active { p.header_active_text } else { p.header_text };
            hp.text(r.center(), Align2::CENTER_CENTER, col_name(col), font.clone(), color);
            hp.line_segment([pos2(r.right(), r.top() + 4.0), pos2(r.right(), r.bottom() - 4.0)], line);
        }
        hp.line_segment([g.col_header.left_bottom(), g.col_header.right_bottom()], line);

        let rp = painter.with_clip_rect(g.row_header);
        rp.rect_filled(g.row_header, 0.0, p.header_bg);
        for &(row, y, h) in &g.rows {
            let r = Rect::from_min_size(pos2(g.row_header.left(), y), vec2(g.row_header.width(), h));
            let band = if row < fr {
                Rect::from_min_max(g.row_header.min, pos2(g.row_header.right(), g.split.y))
            } else {
                Rect::from_min_max(pos2(g.row_header.left(), g.split.y), g.row_header.max)
            };
            let rp = rp.with_clip_rect(band);
            let active = (sel.start.row..=sel.end.row).contains(&row);
            if active {
                rp.rect_filled(r, 0.0, p.header_active_bg);
            }
            let color = if active { p.header_active_text } else { p.header_text };
            rp.text(r.center(), Align2::CENTER_CENTER, (row + 1).to_string(), font.clone(), color);
            // A hidden row above shows as a double line, like Excel.
            let stroke = if self.sheet().is_row_hidden(row + 1) { Stroke::new(2.0, p.accent) } else { line };
            rp.line_segment([pos2(r.left() + 4.0, r.bottom()), pos2(r.right() - 4.0, r.bottom())], stroke);
        }
        rp.line_segment([g.row_header.right_top(), g.row_header.right_bottom()], line);

        painter.rect_filled(g.corner, 0.0, p.header_bg);
        painter.line_segment([g.corner.right_top(), g.corner.right_bottom()], line);
        painter.line_segment([g.corner.left_bottom(), g.corner.right_bottom()], line);
        // A small triangle like Excel's select-all button.
        let c = g.corner.right_bottom() - vec2(4.0, 4.0);
        painter.add(Shape::convex_polygon(
            vec![c, c - vec2(9.0, 0.0), c - vec2(0.0, 9.0)],
            p.header_line,
            Stroke::NONE,
        ));
    }

    fn paint_scrollbars(&self, ui: &Ui, g: &Geometry, p: &Palette) {
        let painter = ui.painter();
        for vertical in [true, false] {
            let (thumb, _) = self.thumb(g, vertical);
            painter.rect_filled(thumb, CornerRadius::same(4), p.thumb);
        }
    }

    fn editor(&mut self, ui: &mut Ui, g: &Geometry, p: &Palette, editor_id: egui::Id, _bar_id: egui::Id) {
        let Some(edit) = &self.edit else { return };
        // While pointing at another sheet the formula bar does the editing.
        if edit.in_bar || edit.sheet != self.sheet {
            return;
        }
        take_edit_keys(ui, self, editor_id);
        let Some(edit) = &self.edit else { return };
        let cell = edit.cell;
        let format = self.sheet().format(cell);
        let family = crate::fonts::family(format.bold, format.italic);
        let font = FontId::new(CELL_FONT, family);
        let text_w = ui.painter().layout_no_wrap(edit.text.clone(), font.clone(), p.text).size().x;

        let mut rect = self.range_rect(g, Range::single(cell));
        // Keep the editor on screen even if its cell scrolled away.
        if !g.cells.intersects(rect) {
            rect = Rect::from_min_size(g.cells.min, rect.size());
        }
        let width = (text_w + 2.0 * PAD + 12.0).max(rect.width()).min(g.cells.right() - rect.left());
        let rect = Rect::from_min_size(rect.min, vec2(width, self.sheet().row_height(cell.row).max(ROW_H)));

        ui.painter().rect_filled(rect, 0.0, p.background);
        let edit = self.edit.as_mut().expect("editing");
        // Clicking a cell to insert a reference takes focus away; take it
        // back before the field runs so this frame's typing lands in it.
        if edit.focus_pending || ui.memory(|m| m.focused().is_none()) {
            ui.memory_mut(|m| m.request_focus(editor_id));
            edit.focus_pending = false;
        }
        if edit.cursor_to_end {
            crate::view::cursor_to_end(ui, editor_id, &edit.text);
            edit.cursor_to_end = false;
        }
        let response = ui.put(
            rect.shrink2(vec2(PAD - 2.0, 2.0)),
            egui::TextEdit::singleline(&mut edit.text)
                .id(editor_id)
                .frame(egui::Frame::NONE)
                .font(font)
                .text_color(p.text)
                .desired_width(rect.width() - 2.0 * PAD)
                .cursor_at_end(true)
                .event_filter(crate::view::EDIT_KEYS),
        );
        crate::view::accessible_name(ui, &response, "Celinhoud");
        ui.painter().rect_stroke(rect, 0.0, Stroke::new(2.0, p.accent), StrokeKind::Outside);
    }
}
