//! One document window: toolbar, formula bar, grid and status bar, plus the
//! selection and editing state behind them.

use std::path::PathBuf;

use egui::{Event, Key, Modifiers, Ui};
use rustsheet_core::clipboard::{Block, to_tsv};
use rustsheet_core::{
    Align, CellFormat, CellRef, CellState, Change, Content, Document, MAX_COLS, MAX_ROWS, NumFmt, Range,
    Sheet, Value, edit_text, parse_input,
};

use crate::clip::{ClipState, CutSource, Pasted};
use crate::menu::Command;

/// Formatting a selection larger than this touches only cells that exist,
/// so formatting a whole column does not create a million cells.
const MAX_FORMAT_CELLS: u64 = 250_000;

/// What the app shell gives a view each frame.
pub struct Env<'a> {
    pub clip: &'a mut ClipState,
    pub window: u64,
    pub requests: &'a mut Vec<Request>,
}

/// Things a view asks the app shell to do.
pub enum Request {
    /// A cut from this window was pasted elsewhere; clear its source cells.
    ClearCut(CutSource),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Selection {
    /// Where typing goes; always inside the range.
    pub active: CellRef,
    /// Fixed corner of the range; Shift+arrows move the other one.
    pub anchor: CellRef,
    pub corner: CellRef,
}

impl Selection {
    pub fn single(at: CellRef) -> Self {
        Self { active: at, anchor: at, corner: at }
    }

    pub fn span(anchor: CellRef, corner: CellRef) -> Self {
        Self { active: anchor, anchor, corner }
    }

    pub fn range(&self) -> Range {
        Range::new(self.anchor, self.corner)
    }
}

pub struct Edit {
    pub cell: CellRef,
    pub text: String,
    /// Started by typing over the cell: arrow keys commit and move, like Excel.
    pub typed: bool,
    /// Being edited in the formula bar rather than in the cell.
    pub in_bar: bool,
    pub focus_pending: bool,
    /// Reference being inserted by clicking cells while typing a formula:
    /// byte offset where it starts and the cell the drag started on.
    pub point: Option<(usize, CellRef)>,
}

/// Cached sum/average/count of the selection for the status bar.
struct Stats {
    key: (Range, u64),
    count: u64,
    numbers: u64,
    sum: f64,
}

pub struct SheetView {
    pub doc: Document,
    pub path: Option<PathBuf>,
    pub untitled: usize,
    pub sel: Selection,
    /// Scroll offset in points; f64 because a million rows exceed f32 precision.
    pub scroll: (f64, f64),
    pub edit: Option<Edit>,
    pub status: Option<String>,
    pending: Vec<Command>,
    bar_text: String,
    name_text: String,
    stats: Option<Stats>,
    pub(crate) drag: crate::grid::Drag,
    /// Visible rows, for Page Up/Down.
    pub(crate) page_rows: u32,
    pub(crate) scroll_into_view: bool,
    pub(crate) grid_focus_pending: bool,
}

impl SheetView {
    pub fn new(doc: Document, path: Option<PathBuf>, untitled: usize) -> Self {
        Self {
            doc,
            path,
            untitled,
            sel: Selection::single(CellRef::default()),
            scroll: (0.0, 0.0),
            edit: None,
            status: None,
            pending: Vec::new(),
            bar_text: String::new(),
            name_text: String::new(),
            stats: None,
            drag: crate::grid::Drag::None,
            page_rows: 20,
            scroll_into_view: false,
            grid_focus_pending: true,
        }
    }

    pub fn sheet(&self) -> &Sheet {
        &self.doc.sheet
    }

    /// The name shown in the title bar.
    pub fn name(&self) -> String {
        match &self.path {
            Some(p) => p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into()),
            None if self.untitled <= 1 => "Naamloos".to_string(),
            None => format!("Naamloos {}", self.untitled),
        }
    }

    pub fn title(&self) -> String {
        if self.doc.is_modified() { format!("{} — bewerkt", self.name()) } else { self.name() }
    }

    /// True for a fresh window nobody typed in, which Open may reuse.
    pub fn is_pristine(&self) -> bool {
        self.path.is_none() && !self.doc.is_modified() && !self.doc.can_undo()
    }

    pub fn push_command(&mut self, command: Command) {
        self.pending.push(command);
    }

    pub fn ids(&self, window: u64) -> (egui::Id, egui::Id, egui::Id, egui::Id) {
        (
            egui::Id::unique(("grid", window)),
            egui::Id::unique(("cell-editor", window)),
            egui::Id::unique(("formula-bar", window)),
            egui::Id::unique(("name-box", window)),
        )
    }

    // ----- frame -----

    pub fn ui(&mut self, ui: &mut Ui, env: &mut Env<'_>) {
        for command in std::mem::take(&mut self.pending) {
            self.run(command, ui, env);
        }
        let palette = crate::grid::Palette::new(ui.visuals().dark_mode);
        egui::Panel::top(("toolbar", env.window))
            .frame(egui::Frame::new().fill(palette.chrome).inner_margin(egui::Margin::symmetric(8, 6)))
            .show_separator_line(false)
            .show(ui, |ui| self.toolbar(ui, env));
        egui::Panel::top(("formula-panel", env.window))
            .frame(egui::Frame::new().fill(palette.chrome).inner_margin(egui::Margin::symmetric(8, 4)))
            .show(ui, |ui| self.formula_bar(ui, env));
        egui::Panel::bottom(("status", env.window))
            .frame(egui::Frame::new().fill(palette.chrome).inner_margin(egui::Margin::symmetric(10, 3)))
            .show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.background))
            .show(ui, |ui| self.grid(ui, env));
    }

    /// Runs a menu or toolbar command. While a cell is being edited, the
    /// text commands go to the text field instead of the grid.
    fn run(&mut self, command: Command, ui: &mut Ui, env: &mut Env<'_>) {
        if self.edit.is_some() {
            let event = match &command {
                Command::Copy => Some(Event::Copy),
                Command::Cut => Some(Event::Cut),
                Command::Paste => env.clip.text().map(Event::Paste),
                Command::SelectAll => Some(key_event(Key::A, Modifiers::COMMAND)),
                Command::Undo => Some(key_event(Key::Z, Modifiers::COMMAND)),
                Command::Redo => Some(key_event(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT)),
                Command::Delete => None,
                _ => {
                    // Formatting applies after the edit is committed.
                    if !self.commit_edit() {
                        return;
                    }
                    self.run(command, ui, env);
                    return;
                }
            };
            if let Some(event) = event {
                ui.ctx().input_mut(|i| i.events.push(event));
            }
            return;
        }
        match command {
            Command::Undo => {
                if let Some(selection) = self.doc.undo() {
                    self.restore_selection(selection);
                }
            }
            Command::Redo => {
                if let Some(selection) = self.doc.redo() {
                    self.restore_selection(selection);
                }
            }
            Command::Copy => self.copy(env, false),
            Command::Cut => self.copy(env, true),
            Command::Paste => self.paste(env),
            Command::SelectAll => {
                self.sel = Selection::span(CellRef::default(), CellRef::new(MAX_ROWS - 1, MAX_COLS - 1));
            }
            Command::Delete => self.clear_selection(),
            Command::Bold => {
                let bold = !self.sheet().format(self.sel.active).bold;
                self.format_selection(|f| f.bold = bold);
            }
            Command::Align(align) => self.format_selection(|f| f.align = align),
            Command::NumFmt(num) => self.format_selection(|f| f.num = num),
            // File commands are handled by the app shell.
            _ => {}
        }
    }

    fn restore_selection(&mut self, selection: Option<Range>) {
        if let Some(r) = selection {
            self.sel = Selection::span(r.start, r.end);
            self.scroll_into_view = true;
        }
    }

    // ----- editing -----

    pub fn begin_edit(&mut self, text: Option<String>, in_bar: bool) {
        let cell = self.sel.active;
        let typed = text.is_some();
        let text = text.unwrap_or_else(|| {
            let state = self.sheet().state(cell);
            edit_text(&state.content, state.format.num)
        });
        self.edit = Some(Edit { cell, text, typed, in_bar, focus_pending: true, point: None });
        self.status = None;
        self.scroll_into_view = true;
    }

    /// Stores the edited text. Returns false (and keeps editing) when the
    /// formula does not parse.
    pub fn commit_edit(&mut self) -> bool {
        let Some(edit) = self.edit.take() else { return true };
        match parse_input(&edit.text) {
            Ok(parsed) => {
                let mut state = self.sheet().state(edit.cell);
                state.content = parsed.content;
                if let Some(num) = parsed.format
                    && state.format.num == NumFmt::General
                {
                    state.format.num = num;
                }
                self.doc.set_cells(vec![(edit.cell, state)], Some(self.sel.range()));
                self.status = None;
                self.grid_focus_pending = true;
                true
            }
            Err(e) => {
                self.status = Some(format!("Formule klopt niet: {e}"));
                self.edit = Some(Edit { focus_pending: true, point: None, ..edit });
                false
            }
        }
    }

    pub fn cancel_edit(&mut self) {
        self.edit = None;
        self.status = None;
        self.grid_focus_pending = true;
    }

    /// Whether clicking a cell should insert a reference into the formula
    /// being typed instead of committing it.
    pub fn wants_reference(&self) -> bool {
        let Some(edit) = &self.edit else { return false };
        if !edit.text.starts_with('=') {
            return false;
        }
        if edit.point.is_some() {
            return true;
        }
        edit.text.trim_end().chars().last().is_some_and(|c| "=+-*/^(,;:&<>".contains(c))
    }

    /// Inserts (or, while dragging, updates) a reference to `from:to`.
    pub fn point_at(&mut self, from: CellRef, to: CellRef) {
        let Some(edit) = &mut self.edit else { return };
        let start = match edit.point {
            Some((start, _)) => start,
            None => {
                let start = edit.text.len();
                edit.point = Some((start, from));
                start
            }
        };
        let anchor = edit.point.map_or(from, |(_, a)| a);
        edit.text.truncate(start);
        edit.text.push_str(&Range::new(anchor, to).to_string());
        edit.focus_pending = true;
    }

    pub fn end_point(&mut self) {
        if let Some(edit) = &mut self.edit {
            edit.point = None;
        }
    }

    // ----- selection -----

    pub fn select(&mut self, at: CellRef, extend: bool) {
        if extend {
            self.sel.corner = at;
        } else {
            self.sel = Selection::single(at);
        }
        self.scroll_into_view = true;
    }

    /// Moves the active cell (or with `extend`, the selection corner).
    pub fn move_by(&mut self, drow: i64, dcol: i64, extend: bool, to_edge: bool) {
        let from = if extend { self.sel.corner } else { self.sel.active };
        let to = if to_edge {
            data_edge(self.sheet(), from, drow.signum(), dcol.signum())
        } else {
            from.offset_clamped(drow, dcol)
        };
        self.select(to, extend);
    }

    /// Enter and Tab move within a multi-cell selection, wrapping like Excel.
    pub fn advance(&mut self, drow: i64, dcol: i64) {
        let range = self.sel.range();
        if range.len() <= 1 {
            self.move_by(drow, dcol, false, false);
            return;
        }
        let a = self.sel.active;
        let (r0, r1, c0, c1) = (range.start.row, range.end.row, range.start.col, range.end.col);
        let next = if drow != 0 {
            let row = i64::from(a.row) + drow;
            if row > i64::from(r1) {
                CellRef::new(r0, if a.col >= c1 { c0 } else { a.col + 1 })
            } else if row < i64::from(r0) {
                CellRef::new(r1, if a.col <= c0 { c1 } else { a.col - 1 })
            } else {
                CellRef::new(row as u32, a.col)
            }
        } else {
            let col = i64::from(a.col) + dcol;
            if col > i64::from(c1) {
                CellRef::new(if a.row >= r1 { r0 } else { a.row + 1 }, c0)
            } else if col < i64::from(c0) {
                CellRef::new(if a.row <= r0 { r1 } else { a.row - 1 }, c1)
            } else {
                CellRef::new(a.row, col as u32)
            }
        };
        self.sel.active = next;
        self.scroll_into_view = true;
    }

    // ----- commands on the selection -----

    /// Cells a selection-wide change should touch.
    fn target_cells(&self) -> Vec<CellRef> {
        let range = self.sel.range();
        if range.len() <= MAX_FORMAT_CELLS {
            range.iter().collect()
        } else {
            self.sheet().cells_in(range).map(|(at, _)| at).collect()
        }
    }

    fn format_selection(&mut self, f: impl Fn(&mut CellFormat)) {
        let changes = self
            .target_cells()
            .into_iter()
            .map(|at| {
                let mut state = self.sheet().state(at);
                f(&mut state.format);
                (at, state)
            })
            .collect();
        self.doc.set_cells(changes, Some(self.sel.range()));
    }

    fn clear_selection(&mut self) {
        let changes = self
            .sheet()
            .cells_in(self.sel.range())
            .filter(|(_, cell)| !cell.content.is_empty())
            .map(|(at, cell)| (at, CellState { content: Content::Empty, format: cell.format }))
            .collect();
        self.doc.set_cells(changes, Some(self.sel.range()));
    }

    fn copy(&mut self, env: &mut Env<'_>, cut: bool) {
        let range = self.sel.range();
        let block = Block::copy(self.sheet(), range);
        let text = to_tsv(self.sheet(), range);
        let source = cut.then_some(CutSource { window: env.window, range });
        env.clip.set(block, text, source);
        self.status = Some(if cut { "Geknipt" } else { "Gekopieerd" }.to_string());
    }

    fn paste(&mut self, env: &mut Env<'_>) {
        let Some(pasted) = env.clip.take_paste() else { return };
        let (block, cut) = match pasted {
            Pasted::Internal(block, cut) => (block, cut),
            Pasted::External(block) => (block, None),
        };
        let mut changes = Vec::new();
        if let Some(cut) = cut {
            if cut.window == env.window {
                changes.extend(self.sheet().cells_in(cut.range).map(|(at, _)| (at, CellState::default())));
            } else {
                env.requests.push(Request::ClearCut(cut));
            }
        }
        let (area, pasted) = block.paste(self.sheet(), self.sel.range(), cut.is_some());
        changes.extend(pasted);
        self.doc.set_cells(changes, Some(self.sel.range()));
        self.sel = Selection::span(area.start, area.end);
        self.status = None;
    }

    /// Clears the source of a cut that was pasted in another window.
    pub fn clear_cut_source(&mut self, range: Range) {
        let changes = self.sheet().cells_in(range).map(|(at, _)| (at, CellState::default())).collect();
        self.doc.set_cells(changes, Some(range));
    }

    pub fn set_column_width(&mut self, col: u32, original: Option<f32>, width: Option<f32>) {
        // The drag already changed the sheet; put the original back so the
        // undo step records the right "before".
        self.doc.sheet.set_col_width(col, original);
        self.doc.apply(Change::ColWidths(vec![(col, width)]), None);
    }

    // ----- bars -----

    fn toolbar(&mut self, ui: &mut Ui, env: &mut Env<'_>) {
        let format = self.sheet().format(self.sel.active);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let mut clicked = None;
            if ui
                .add_enabled(self.doc.can_undo(), egui::Button::new("↶"))
                .on_hover_text("Herstel (⌘Z)")
                .clicked()
            {
                clicked = Some(Command::Undo);
            }
            if ui
                .add_enabled(self.doc.can_redo(), egui::Button::new("↷"))
                .on_hover_text("Opnieuw (⇧⌘Z)")
                .clicked()
            {
                clicked = Some(Command::Redo);
            }
            ui.separator();
            let bold = egui::RichText::new("B").font(egui::FontId::new(14.0, crate::fonts::bold_family()));
            if ui.add(egui::Button::selectable(format.bold, bold)).on_hover_text("Vet (⌘B)").clicked() {
                clicked = Some(Command::Bold);
            }
            ui.separator();
            for (align, label, tip) in [
                (Align::Left, "Links", "Lijn links uit"),
                (Align::Center, "Midden", "Centreer"),
                (Align::Right, "Rechts", "Lijn rechts uit"),
            ] {
                let on = format.align == align;
                if ui.add(egui::Button::selectable(on, label)).on_hover_text(tip).clicked() {
                    clicked = Some(Command::Align(if on { Align::General } else { align }));
                }
            }
            ui.separator();
            let mut num = format.num;
            egui::ComboBox::from_id_salt(("numfmt", env.window))
                .selected_text(num.label())
                .width(110.0)
                .show_ui(ui, |ui| {
                    for f in NumFmt::ALL {
                        ui.selectable_value(&mut num, f, f.label());
                    }
                });
            if num != format.num {
                clicked = Some(Command::NumFmt(num));
            }
            if let Some(command) = clicked {
                self.run(command, ui, env);
                self.grid_focus_pending = self.edit.is_none();
            }
        });
    }

    fn formula_bar(&mut self, ui: &mut Ui, env: &mut Env<'_>) {
        let (_, _, bar_id, name_id) = self.ids(env.window);
        ui.horizontal(|ui| {
            // Name box: shows the selection, accepts an address to jump to.
            let name_focused = ui.memory(|m| m.has_focus(name_id));
            if !name_focused {
                let r = self.sel.range();
                self.name_text = if r.start == r.end { r.start.to_string() } else { r.to_string() };
            }
            let name = ui.add(
                egui::TextEdit::singleline(&mut self.name_text)
                    .id(name_id)
                    .desired_width(90.0)
                    .align(egui::Align2::CENTER_CENTER),
            );
            if name.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                self.jump_to(&self.name_text.clone());
                self.grid_focus_pending = true;
            }
            ui.label(egui::RichText::new("ƒx").italics().weak());

            if self.edit.as_ref().is_some_and(|e| e.in_bar) {
                take_edit_keys(ui, self, bar_id);
            }
            if self.edit.is_none() {
                let state = self.sheet().state(self.sel.active);
                self.bar_text = edit_text(&state.content, state.format.num);
            }
            let text = match &mut self.edit {
                Some(edit) => &mut edit.text,
                None => &mut self.bar_text,
            };
            let response = ui.add(
                egui::TextEdit::singleline(text)
                    .id(bar_id)
                    .desired_width(f32::INFINITY)
                    .font(egui::FontId::monospace(13.0)),
            );
            if let Some(edit) = &mut self.edit
                && edit.in_bar
                && edit.focus_pending
            {
                response.request_focus();
                edit.focus_pending = false;
            }
            if response.gained_focus() && self.edit.as_ref().is_none_or(|e| !e.in_bar) {
                match &mut self.edit {
                    Some(edit) => edit.in_bar = true,
                    None => {
                        let text = std::mem::take(&mut self.bar_text);
                        self.begin_edit(None, true);
                        if let Some(edit) = &mut self.edit {
                            edit.text = text;
                            edit.focus_pending = false;
                        }
                    }
                }
            }
        });
    }

    fn jump_to(&mut self, text: &str) {
        let text = text.trim().to_ascii_uppercase();
        let target = match text.split_once(':') {
            Some((a, b)) => CellRef::parse(a).zip(CellRef::parse(b)).map(|(a, b)| Selection::span(a, b)),
            None => CellRef::parse(&text).map(Selection::single),
        };
        match target {
            Some(sel) => {
                self.sel = sel;
                self.scroll_into_view = true;
            }
            None => self.status = Some(format!("Onbekend adres: {text}")),
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let range = self.sel.range();
        let key = (range, self.doc.revision());
        if self.stats.as_ref().is_none_or(|s| s.key != key) {
            let (mut count, mut numbers, mut sum) = (0u64, 0u64, 0.0);
            for (_, cell) in self.sheet().cells_in(range) {
                match &cell.value {
                    Value::Empty => {}
                    Value::Number(n) => {
                        count += 1;
                        numbers += 1;
                        sum += n;
                    }
                    _ => count += 1,
                }
            }
            self.stats = Some(Stats { key, count, numbers, sum });
        }
        ui.horizontal(|ui| {
            match &self.status {
                Some(msg) => ui.label(msg),
                None => ui.weak("Gereed"),
            };
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let s = self.stats.as_ref().expect("computed above");
                if s.count > 1 {
                    let fmt =
                        |n: f64| rustsheet_core::format::format_value(&Value::Number(n), NumFmt::General);
                    if s.numbers > 0 {
                        ui.label(format!("Som: {}", fmt(s.sum)));
                        ui.label(format!("Gemiddelde: {}", fmt(s.sum / s.numbers as f64)));
                    }
                    ui.label(format!("Aantal: {}", s.count));
                }
            });
        });
    }
}

fn key_event(key: Key, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

/// Removes the first press of `key` from this frame's input and returns its modifiers.
pub fn take_key(ui: &Ui, key: Key) -> Option<Modifiers> {
    ui.ctx().input_mut(|i| {
        let pos = i
            .events
            .iter()
            .position(|e| matches!(e, Event::Key { key: k, pressed: true, .. } if *k == key))?;
        match i.events.remove(pos) {
            Event::Key { modifiers, .. } => Some(modifiers),
            _ => None,
        }
    })
}

/// Handles Enter, Tab, Escape (and arrows when typing over a cell) for the
/// text field `id` before the field itself sees them.
pub fn take_edit_keys(ui: &Ui, view: &mut SheetView, id: egui::Id) {
    if !ui.memory(|m| m.has_focus(id)) {
        return;
    }
    if take_key(ui, Key::Escape).is_some() {
        view.cancel_edit();
        return;
    }
    if let Some(m) = take_key(ui, Key::Enter) {
        if view.commit_edit() {
            view.advance(if m.shift { -1 } else { 1 }, 0);
        }
        return;
    }
    if let Some(m) = take_key(ui, Key::Tab) {
        if view.commit_edit() {
            view.advance(0, if m.shift { -1 } else { 1 });
        }
        return;
    }
    let typed = view.edit.as_ref().is_some_and(|e| e.typed && !e.in_bar && e.point.is_none());
    if typed {
        for (key, dr, dc) in
            [(Key::ArrowUp, -1, 0), (Key::ArrowDown, 1, 0), (Key::ArrowLeft, 0, -1), (Key::ArrowRight, 0, 1)]
        {
            if let Some(m) = take_key(ui, key) {
                if view.commit_edit() {
                    view.move_by(dr, dc, m.shift, m.command);
                }
                return;
            }
        }
    }
}

/// Excel's Cmd+arrow: jump to the edge of the current block of data, or to
/// the next non-empty cell, or to the edge of the sheet.
pub fn data_edge(sheet: &Sheet, from: CellRef, drow: i64, dcol: i64) -> CellRef {
    let filled = |at: CellRef| sheet.cell(at).is_some_and(|c| !c.content.is_empty());
    let Some(next) = from.offset(drow, dcol) else { return from };
    if filled(from) && filled(next) {
        let mut cur = next;
        while let Some(n) = cur.offset(drow, dcol) {
            if !filled(n) {
                break;
            }
            cur = n;
        }
        return cur;
    }
    // Vertical searches use the column index instead of probing a million rows.
    if dcol == 0 {
        let col = from.col;
        let found = if drow > 0 {
            sheet
                .cells_in(Range::new(next, CellRef::new(MAX_ROWS - 1, col)))
                .find(|(_, c)| !c.content.is_empty())
                .map(|(at, _)| at)
        } else {
            sheet
                .cells_in(Range::new(CellRef::new(0, col), next))
                .filter(|(_, c)| !c.content.is_empty())
                .last()
                .map(|(at, _)| at)
        };
        return found.unwrap_or(CellRef::new(if drow > 0 { MAX_ROWS - 1 } else { 0 }, col));
    }
    let mut cur = next;
    loop {
        if filled(cur) {
            return cur;
        }
        match cur.offset(drow, dcol) {
            Some(n) => cur = n,
            None => return cur,
        }
    }
}
