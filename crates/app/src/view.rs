//! One document window: toolbar, formula bar, grid and status bar, plus the
//! selection and editing state behind them.

use std::collections::HashMap;
use std::path::PathBuf;

use egui::{Event, Key, Modifiers, Ui};
use rustsheet_core::clipboard::{Block, to_tsv};
use rustsheet_core::format::Rgb;
use rustsheet_core::formula::sheet_prefix;
use rustsheet_core::{
    Align, CellFormat, CellRef, CellState, Change, Content, Document, MAX_COLS, MAX_ROWS, NumFmt, Range,
    Sheet, SheetId, Spot, Value, edit_text, parse_input,
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
    /// The sheet being edited; while pointing at cells for a formula the
    /// view may show another sheet.
    pub sheet: SheetId,
    pub cell: CellRef,
    pub text: String,
    /// Started by typing over the cell: arrow keys commit and move, like Excel.
    pub typed: bool,
    /// Being edited in the formula bar rather than in the cell.
    pub in_bar: bool,
    pub focus_pending: bool,
    /// Move the text cursor to the end on the next frame, after the text
    /// changed outside the text field.
    pub cursor_to_end: bool,
    /// Reference being inserted by clicking cells while typing a formula:
    /// byte offset where it starts and the cell the drag started on.
    pub point: Option<(usize, CellRef)>,
}

/// The find (and replace) bar.
#[derive(Default)]
pub struct FindBar {
    pub query: String,
    pub replacement: String,
    pub replacing: bool,
    pub opts: rustsheet_core::find::FindOptions,
    pub focus_pending: bool,
    /// "3 van 12" or a result message.
    pub info: String,
}

/// A sheet tab being renamed in place.
pub struct Rename {
    pub sheet: SheetId,
    pub text: String,
    pub focus_pending: bool,
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
    /// The sheet shown in this window.
    pub sheet: SheetId,
    /// Its position, to pick a neighbour when it disappears.
    sheet_index: usize,
    /// Selection and scroll position of the other sheets, restored when
    /// switching back to them.
    sheet_views: HashMap<SheetId, (Selection, (f64, f64))>,
    pub rename: Option<Rename>,
    pub find: Option<FindBar>,
    /// Column where a run of Tab presses started; Enter returns there on
    /// the next row, like Excel.
    tab_start: Option<u32>,
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
    /// Where the grid was drawn last frame, for tests and hit testing.
    pub(crate) grid_rect: egui::Rect,
}

impl SheetView {
    pub fn new(doc: Document, path: Option<PathBuf>, untitled: usize) -> Self {
        let sheet = doc.book.sheet(0).id();
        Self {
            doc,
            path,
            untitled,
            sheet,
            sheet_index: 0,
            sheet_views: HashMap::new(),
            rename: None,
            find: None,
            tab_start: None,
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
            grid_rect: egui::Rect::NOTHING,
        }
    }

    pub fn sheet(&self) -> &Sheet {
        self.doc.book.sheet_by_id(self.sheet).unwrap_or_else(|| self.doc.book.sheet(0))
    }

    /// Shows sheet `id`, remembering where we were on the current one.
    pub fn show_sheet(&mut self, id: SheetId) {
        let Some(index) = self.doc.book.index_of(id) else { return };
        self.sheet_index = index;
        if id == self.sheet {
            return;
        }
        self.sheet_views.insert(self.sheet, (self.sel, self.scroll));
        self.sheet = id;
        let (sel, scroll) =
            self.sheet_views.get(&id).copied().unwrap_or((Selection::single(CellRef::default()), (0.0, 0.0)));
        self.sel = sel;
        self.scroll = scroll;
        self.drag = crate::grid::Drag::None;
        self.stats = None;
        if self.edit.is_none() {
            self.grid_focus_pending = true;
        }
    }

    /// Moves to the next (`1`) or previous (`-1`) sheet tab.
    pub fn switch_sheet_by(&mut self, delta: i64) {
        let len = self.doc.book.len() as i64;
        let index = (self.sheet_index as i64 + delta).clamp(0, len - 1) as usize;
        let id = self.doc.book.sheet(index).id();
        self.show_sheet(id);
    }

    /// Picks a neighbouring sheet when the shown one was deleted (or its
    /// creation undone).
    fn sync_sheet(&mut self) {
        match self.doc.book.index_of(self.sheet) {
            Some(index) => self.sheet_index = index,
            None => {
                let index = self.sheet_index.min(self.doc.book.len() - 1);
                let id = self.doc.book.sheet(index).id();
                self.sheet_views.remove(&self.sheet);
                self.sheet = id;
                let (sel, scroll) = self
                    .sheet_views
                    .get(&id)
                    .copied()
                    .unwrap_or((Selection::single(CellRef::default()), (0.0, 0.0)));
                self.sel = sel;
                self.scroll = scroll;
                self.stats = None;
            }
        }
        if self.edit.as_ref().is_some_and(|e| self.doc.book.index_of(e.sheet).is_none()) {
            self.edit = None;
        }
        if self.rename.as_ref().is_some_and(|r| self.doc.book.index_of(r.sheet).is_none()) {
            self.rename = None;
        }
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
        self.sync_sheet();
        self.sheet_keys(ui);
        for command in std::mem::take(&mut self.pending) {
            self.run(command, ui, env);
            self.sync_sheet();
        }
        let palette = crate::grid::Palette::new(ui.visuals().dark_mode);
        egui::Panel::top(("toolbar", env.window))
            .frame(egui::Frame::new().fill(palette.chrome).inner_margin(egui::Margin::symmetric(8, 6)))
            .show_separator_line(false)
            .show(ui, |ui| self.toolbar(ui, env));
        egui::Panel::top(("formula-panel", env.window))
            .frame(egui::Frame::new().fill(palette.chrome).inner_margin(egui::Margin::symmetric(8, 4)))
            .show(ui, |ui| self.formula_bar(ui, env));
        if self.find.is_some() {
            egui::Panel::top(("find-panel", env.window))
                .frame(egui::Frame::new().fill(palette.chrome).inner_margin(egui::Margin::symmetric(8, 4)))
                .show(ui, |ui| self.find_bar(ui, env));
        }
        egui::Panel::bottom(("status", env.window))
            .frame(egui::Frame::new().fill(palette.chrome).inner_margin(egui::Margin::symmetric(10, 3)))
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::bottom(("tabs", env.window))
            .frame(egui::Frame::new().fill(palette.chrome).inner_margin(egui::Margin::symmetric(6, 3)))
            .show(ui, |ui| self.tab_bar(ui, env));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.background))
            .show(ui, |ui| self.grid(ui, env));
    }

    /// Ctrl+Page Up/Down and Cmd+Option+arrows switch sheets. Handled before
    /// any panel so scroll areas cannot swallow them; while typing a formula
    /// they let you point at the other sheet.
    fn sheet_keys(&mut self, ui: &Ui) {
        if self.rename.is_some() || (self.edit.is_some() && !self.wants_reference()) {
            return;
        }
        let delta = ui.ctx().input_mut(|i| {
            let pos = i.events.iter().position(|e| {
                matches!(e, Event::Key { key, pressed: true, modifiers: m, .. }
                    if (m.ctrl && matches!(key, Key::PageUp | Key::PageDown))
                        || (m.command && m.alt && matches!(key, Key::ArrowLeft | Key::ArrowRight)))
            })?;
            match i.events.remove(pos) {
                Event::Key { key: Key::PageUp | Key::ArrowLeft, .. } => Some(-1),
                _ => Some(1),
            }
        });
        let Some(delta) = delta else { return };
        if self.edit.is_some() {
            self.switch_sheet_while_editing(delta);
        } else {
            self.switch_sheet_by(delta);
        }
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
                // Switching sheets while typing a formula lets you point at
                // cells on the other sheet.
                Command::NextSheet | Command::PrevSheet if self.wants_reference() => {
                    self.switch_sheet_while_editing(if command == Command::NextSheet { 1 } else { -1 });
                    return;
                }
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
                if let Some(spot) = self.doc.undo() {
                    self.restore_spot(spot);
                }
            }
            Command::Redo => {
                if let Some(spot) = self.doc.redo() {
                    self.restore_spot(spot);
                }
            }
            Command::AddSheet => {
                let id = self.doc.add_sheet(self.sheet_index + 1);
                self.show_sheet(id);
            }
            Command::DuplicateSheet => {
                if let Some(id) = self.doc.duplicate_sheet(self.sheet) {
                    let (sel, scroll) = (self.sel, self.scroll);
                    self.show_sheet(id);
                    (self.sel, self.scroll) = (sel, scroll);
                }
            }
            Command::DeleteSheet => match self.doc.delete_sheet(self.sheet) {
                Ok(()) => self.sync_sheet(),
                Err(e) => self.status = Some(e),
            },
            Command::RenameSheet => {
                let text = self.sheet().name().to_string();
                self.rename = Some(Rename { sheet: self.sheet, text, focus_pending: true });
            }
            Command::NextSheet => self.switch_sheet_by(1),
            Command::PrevSheet => self.switch_sheet_by(-1),
            Command::Copy => self.copy(env, false),
            Command::Cut => self.copy(env, true),
            Command::Paste => self.paste(env),
            Command::SelectAll => {
                self.sel = Selection::span(CellRef::default(), CellRef::new(MAX_ROWS - 1, MAX_COLS - 1));
            }
            Command::Delete => self.clear_selection(),
            Command::SortAsc | Command::SortDesc => self.sort(command == Command::SortDesc),
            Command::Find | Command::Replace => {
                let bar = self.find.get_or_insert_with(FindBar::default);
                bar.replacing |= command == Command::Replace;
                bar.focus_pending = true;
            }
            Command::FindNext | Command::FindPrev => self.find_step(command == Command::FindPrev),
            Command::FillDown | Command::FillRight => {
                // Copy the first row (or column) of the selection into the rest.
                let r = self.sel.range();
                let down = command == Command::FillDown;
                let (source, target) = if down && r.rows() > 1 {
                    (
                        Range::new(r.start, CellRef::new(r.start.row, r.end.col)),
                        Range::new(CellRef::new(r.start.row + 1, r.start.col), r.end),
                    )
                } else if !down && r.cols() > 1 {
                    (
                        Range::new(r.start, CellRef::new(r.end.row, r.start.col)),
                        Range::new(CellRef::new(r.start.row, r.start.col + 1), r.end),
                    )
                } else {
                    return;
                };
                if target.len() <= MAX_FORMAT_CELLS {
                    let changes = rustsheet_core::fill::fill(self.sheet(), source, target, false);
                    self.doc.set_cells(self.sheet, changes, Some(r));
                }
            }
            Command::InsertRows | Command::InsertCols | Command::DeleteRows | Command::DeleteCols => {
                self.change_lines(command)
            }
            Command::Bold => {
                let bold = !self.sheet().format(self.sel.active).bold;
                self.format_selection(|f| f.bold = bold);
            }
            Command::Italic => {
                let italic = !self.sheet().format(self.sel.active).italic;
                self.format_selection(|f| f.italic = italic);
            }
            Command::TextColor(color) => self.format_selection(|f| f.color = color),
            Command::FillColor(fill) => self.format_selection(|f| f.fill = fill),
            Command::Align(align) => self.format_selection(|f| f.align = align),
            Command::NumFmt(num) => self.format_selection(|f| f.num = num),
            // File commands are handled by the app shell.
            _ => {}
        }
    }

    /// After undo or redo: show the sheet that changed and select the cells.
    fn restore_spot(&mut self, spot: Option<Spot>) {
        self.sync_sheet();
        let Some(spot) = spot else { return };
        self.show_sheet(spot.sheet);
        if let Some(r) = spot.range {
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
        self.edit = Some(Edit {
            sheet: self.sheet,
            cell,
            text,
            typed,
            in_bar,
            focus_pending: true,
            cursor_to_end: true,
            point: None,
        });
        self.status = None;
        self.scroll_into_view = true;
    }

    /// Stores the edited text. Returns false (and keeps editing) when the
    /// formula does not parse.
    pub fn commit_edit(&mut self) -> bool {
        let Some(edit) = self.edit.take() else { return true };
        let Some(sheet) = self.doc.book.sheet_by_id(edit.sheet) else { return true };
        match parse_input(&edit.text) {
            Ok(parsed) => {
                let mut state = sheet.state(edit.cell);
                state.content = parsed.content;
                if let Some(num) = parsed.format
                    && state.format.num == NumFmt::General
                {
                    state.format.num = num;
                }
                // Pointing at another sheet ends back on the edited one.
                self.show_sheet(edit.sheet);
                self.doc.set_cells(edit.sheet, vec![(edit.cell, state)], Some(self.sel.range()));
                self.status = None;
                self.grid_focus_pending = true;
                true
            }
            Err(e) => {
                self.status = Some(format!("Formule klopt niet: {e}"));
                self.edit = Some(Edit { focus_pending: true, cursor_to_end: false, point: None, ..edit });
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
        if edit.sheet != self.sheet {
            let name = self.doc.book.sheet_by_id(self.sheet).map_or("", |s| s.name());
            edit.text.push_str(&sheet_prefix(name));
        }
        edit.text.push_str(&Range::new(anchor, to).to_string());
        edit.focus_pending = true;
        edit.cursor_to_end = true;
    }

    /// Shows another sheet while a formula is being typed; typing goes on
    /// in the formula bar and clicks insert references to that sheet.
    pub fn switch_sheet_while_editing(&mut self, delta: i64) {
        self.switch_sheet_by(delta);
        self.after_sheet_switch_while_editing();
    }

    fn after_sheet_switch_while_editing(&mut self) {
        if let Some(edit) = &mut self.edit {
            edit.in_bar = true;
            edit.focus_pending = true;
            edit.cursor_to_end = true;
            edit.point = None;
        }
    }

    pub fn end_point(&mut self) {
        if let Some(edit) = &mut self.edit {
            edit.point = None;
        }
    }

    // ----- selection -----

    pub fn select(&mut self, at: CellRef, extend: bool) {
        self.tab_start = None;
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
            let active = self.sel.active;
            if dcol != 0 {
                let start = self.tab_start.unwrap_or(active.col);
                self.move_by(drow, dcol, false, false);
                self.tab_start = Some(start);
            } else if let (Some(start), true) = (self.tab_start, drow > 0) {
                self.select(CellRef::new(active.row, start).offset_clamped(1, 0), false);
            } else {
                self.move_by(drow, dcol, false, false);
            }
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
        self.doc.set_cells(self.sheet, changes, Some(self.sel.range()));
    }

    /// Sorts the selection (or, for a single cell, the block of data around
    /// it) by the active cell's column, keeping a header row in place.
    pub fn sort(&mut self, descending: bool) {
        use rustsheet_core::sort::{current_region, has_header, sort_rows};
        let sel = self.sel.range();
        let range = if sel.len() == 1 { current_region(self.sheet(), self.sel.active) } else { sel };
        if range.rows() < 2 {
            return;
        }
        let header = has_header(self.sheet(), range);
        let key = self.sel.active.col.clamp(range.start.col, range.end.col);
        let changes = sort_rows(self.sheet(), range, key, descending, header);
        self.doc.set_cells(self.sheet, changes, Some(sel));
        self.sel = Selection { active: self.sel.active, anchor: range.start, corner: range.end };
        self.status =
            Some(if header { "Gesorteerd (eerste rij is kop)".into() } else { "Gesorteerd".into() });
    }

    /// Selects the next (or previous) match of the find bar's query.
    pub fn find_step(&mut self, backwards: bool) {
        let Some(bar) = &self.find else { return };
        if bar.query.is_empty() {
            return;
        }
        let (query, opts) = (bar.query.clone(), bar.opts);
        let all = rustsheet_core::find::find_all(self.sheet(), &query, opts);
        let next = rustsheet_core::find::find_next(self.sheet(), &query, opts, self.sel.active, backwards);
        let info = match next {
            Some(at) => {
                let i = all.iter().position(|&a| a == at).map_or(0, |i| i + 1);
                self.sel = Selection::single(at);
                self.scroll_into_view = true;
                format!("{i} van {}", all.len())
            }
            None => "Niet gevonden".to_string(),
        };
        if let Some(bar) = &mut self.find {
            bar.info = info;
        }
    }

    /// Replaces in the active cell if it matches, then moves on.
    pub fn replace_one(&mut self) {
        let Some(bar) = &self.find else { return };
        let (query, with, opts) = (bar.query.clone(), bar.replacement.clone(), bar.opts);
        let at = self.sel.active;
        let changes = rustsheet_core::find::replace(self.sheet(), &[at], &query, &with, opts);
        if !changes.is_empty() {
            self.doc.set_cells(self.sheet, changes, Some(Range::single(at)));
        }
        self.find_step(false);
    }

    /// Replaces every match on the sheet as one undoable step.
    pub fn replace_all(&mut self) {
        let Some(bar) = &self.find else { return };
        let (query, with, opts) = (bar.query.clone(), bar.replacement.clone(), bar.opts);
        let all = rustsheet_core::find::find_all(self.sheet(), &query, opts);
        let changes = rustsheet_core::find::replace(self.sheet(), &all, &query, &with, opts);
        let n = changes.len();
        if n > 0 {
            self.doc.set_cells(self.sheet, changes, Some(self.sel.range()));
        }
        if let Some(bar) = &mut self.find {
            bar.info = if n == 1 { "1 vervangen".into() } else { format!("{n} vervangen") };
        }
    }

    fn find_bar(&mut self, ui: &mut Ui, env: &mut Env<'_>) {
        let field_id = egui::Id::unique(("find-field", env.window));
        let replace_id = egui::Id::unique(("replace-field", env.window));
        let mut action: Option<&str> = None;
        let focus_field = ui.memory(|m| m.has_focus(field_id) || m.has_focus(replace_id));
        if focus_field {
            if take_key(ui, Key::Escape).is_some() {
                action = Some("close");
            } else if let Some(m) = take_key(ui, Key::Enter) {
                let in_replace = ui.memory(|mem| mem.has_focus(replace_id));
                action = Some(if in_replace {
                    "replace"
                } else if m.shift {
                    "prev"
                } else {
                    "next"
                });
            }
        }
        let Some(bar) = &mut self.find else { return };
        ui.horizontal(|ui| {
            ui.label("Zoek");
            let field = ui.add(
                egui::TextEdit::singleline(&mut bar.query)
                    .id(field_id)
                    .desired_width(180.0)
                    .event_filter(EDIT_KEYS),
            );
            accessible_name(ui, &field, "Zoekterm");
            if bar.focus_pending {
                field.request_focus();
                bar.focus_pending = false;
            }
            if field.changed() {
                bar.info.clear();
            }
            if ui.button("Vorige").clicked() {
                action = Some("prev");
            }
            if ui.button("Volgende").clicked() {
                action = Some("next");
            }
            if bar.replacing {
                ui.separator();
                ui.label("Vervang door");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut bar.replacement)
                        .id(replace_id)
                        .desired_width(140.0)
                        .event_filter(EDIT_KEYS),
                );
                accessible_name(ui, &r, "Vervangtekst");
                if ui.button("Vervang").clicked() {
                    action = Some("replace");
                }
                if ui.button("Vervang alles").clicked() {
                    action = Some("replace-all");
                }
            }
            ui.separator();
            ui.checkbox(&mut bar.opts.match_case, "Hoofdletters");
            ui.checkbox(&mut bar.opts.whole_cell, "Hele cel");
            ui.checkbox(&mut bar.opts.in_values, "In waarden");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let close = ui.add(egui::Button::new("×").frame(false)).on_hover_text("Sluit (Esc)");
                accessible_name(ui, &close, "Sluit zoeken");
                if close.clicked() {
                    action = Some("close");
                }
                ui.weak(&bar.info);
            });
        });
        match action {
            Some("next") => self.find_step(false),
            Some("prev") => self.find_step(true),
            Some("replace") => self.replace_one(),
            Some("replace-all") => self.replace_all(),
            Some("close") => {
                self.find = None;
                self.grid_focus_pending = true;
            }
            _ => {}
        }
    }

    /// Fills `target` from the selection (the fill handle) and selects both.
    pub fn fill_into(&mut self, target: Range, series: bool) {
        let source = self.sel.range();
        if target.len() > MAX_FORMAT_CELLS {
            self.status = Some("Dat gebied is te groot om te vullen.".into());
            return;
        }
        let changes = rustsheet_core::fill::fill(self.sheet(), source, target, series);
        self.doc.set_cells(self.sheet, changes, Some(source));
        let all = source.union(&target);
        self.sel = Selection { active: self.sel.active, anchor: all.start, corner: all.end };
    }

    /// Rows (or columns) the selection covers; a selection spanning every
    /// row (a whole column) counts as one row when inserting.
    pub fn line_span(&self, rows: bool, inserting: bool) -> (u32, u32) {
        let r = self.sel.range();
        let (start, len, max) =
            if rows { (r.start.row, r.rows(), MAX_ROWS) } else { (r.start.col, r.cols(), MAX_COLS) };
        (start, if inserting && len == max { 1 } else { len })
    }

    fn change_lines(&mut self, command: Command) {
        let rows = matches!(command, Command::InsertRows | Command::DeleteRows);
        let inserting = matches!(command, Command::InsertRows | Command::InsertCols);
        let (at, count) = self.line_span(rows, inserting);
        let result = if inserting {
            self.doc.insert_lines(self.sheet, rows, at, count)
        } else {
            self.doc.delete_lines(self.sheet, rows, at, count)
        };
        match result {
            Ok(()) => {
                if !inserting {
                    let a = self.sel.range().start;
                    self.sel = Selection::single(a);
                }
                self.status = None;
            }
            Err(e) => self.status = Some(e),
        }
    }

    fn clear_selection(&mut self) {
        let changes = self
            .sheet()
            .cells_in(self.sel.range())
            .filter(|(_, cell)| !cell.content.is_empty())
            .map(|(at, cell)| (at, CellState { content: Content::Empty, format: cell.format }))
            .collect();
        self.doc.set_cells(self.sheet, changes, Some(self.sel.range()));
    }

    fn copy(&mut self, env: &mut Env<'_>, cut: bool) {
        let range = self.sel.range();
        let block = Block::copy(self.sheet(), range);
        let text = to_tsv(self.sheet(), range);
        let source = cut.then_some(CutSource { window: env.window, sheet: self.sheet, range });
        env.clip.set(block, text, source);
        self.status = Some(if cut { "Geknipt" } else { "Gekopieerd" }.to_string());
    }

    fn paste(&mut self, env: &mut Env<'_>) {
        let Some(pasted) = env.clip.take_paste() else { return };
        let (block, cut) = match pasted {
            Pasted::Internal(block, cut) => (block, cut),
            Pasted::External(block) => (block, None),
        };
        let mut parts = Vec::new();
        if let Some(cut) = cut {
            if cut.window == env.window {
                // Clear the source first (possibly on another sheet), then
                // paste; one undo step for both.
                if let Some(source) = self.doc.book.sheet_by_id(cut.sheet) {
                    let clear =
                        source.cells_in(cut.range).map(|(at, _)| (at, CellState::default())).collect();
                    parts.push(Change::Cells(cut.sheet, clear));
                }
            } else {
                env.requests.push(Request::ClearCut(cut));
            }
        }
        let (area, pasted) = block.paste(self.sheet(), self.sel.range(), cut.is_some());
        parts.push(Change::Cells(self.sheet, pasted));
        self.doc.apply(Change::Batch(parts), Some(Spot { sheet: self.sheet, range: Some(self.sel.range()) }));
        self.sel = Selection::span(area.start, area.end);
        self.status = None;
    }

    /// Clears the source of a cut that was pasted in another window.
    pub fn clear_cut_source(&mut self, sheet: SheetId, range: Range) {
        let Some(source) = self.doc.book.sheet_by_id(sheet) else { return };
        let changes = source.cells_in(range).map(|(at, _)| (at, CellState::default())).collect();
        self.doc.set_cells(sheet, changes, Some(range));
    }

    pub fn set_column_width(&mut self, col: u32, original: Option<f32>, width: Option<f32>) {
        // The drag already changed the sheet; put the original back so the
        // undo step records the right "before".
        self.doc.book.set_col_width(self.sheet, col, original);
        self.doc.apply(
            Change::ColWidths(self.sheet, vec![(col, width)]),
            Some(Spot { sheet: self.sheet, range: None }),
        );
    }

    /// Changes a column width while dragging, without an undo step.
    pub fn preview_column_width(&mut self, col: u32, width: Option<f32>) {
        self.doc.book.set_col_width(self.sheet, col, width);
    }

    // ----- bars -----

    fn toolbar(&mut self, ui: &mut Ui, env: &mut Env<'_>) {
        let format = self.sheet().format(self.sel.active);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let mut clicked = None;
            if icon_button(ui, self.doc.can_undo(), false).on_hover_text("Herstel (⌘Z)").clicked() {
                clicked = Some(Command::Undo);
            }
            if icon_button(ui, self.doc.can_redo(), true).on_hover_text("Opnieuw (⇧⌘Z)").clicked() {
                clicked = Some(Command::Redo);
            }
            ui.separator();
            let bold = egui::RichText::new("B").font(egui::FontId::new(14.0, crate::fonts::bold_family()));
            if ui.add(egui::Button::selectable(format.bold, bold)).on_hover_text("Vet (⌘B)").clicked() {
                clicked = Some(Command::Bold);
            }
            let italic =
                egui::RichText::new("I").font(egui::FontId::new(14.0, crate::fonts::family(false, true)));
            let italic =
                ui.add(egui::Button::selectable(format.italic, italic)).on_hover_text("Cursief (⌘I)");
            accessible_name(ui, &italic, "Cursief");
            if italic.clicked() {
                clicked = Some(Command::Italic);
            }
            if let Some(c) = color_menu(ui, "Tekstkleur", format.color, false) {
                clicked = Some(Command::TextColor(c));
            }
            if let Some(c) = color_menu(ui, "Opvulkleur", format.fill, true) {
                clicked = Some(Command::FillColor(c));
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
            let combo = egui::ComboBox::from_id_salt(("numfmt", env.window))
                .selected_text(num.label())
                .width(110.0)
                .show_ui(ui, |ui| {
                    for f in NumFmt::ALL {
                        ui.selectable_value(&mut num, f, f.label());
                    }
                });
            accessible_name(ui, &combo.response, "Getalnotatie");
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
            accessible_name(ui, &name, "Celadres");
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
            let (grid_id, ..) = self.ids(env.window);
            if let Some(edit) = &mut self.edit
                && edit.in_bar
            {
                // Clicking a cell to point at it moves focus to the grid;
                // take it back so typing continues here.
                let focus = ui.memory(|m| m.focused());
                if edit.focus_pending || focus.is_none() || focus == Some(grid_id) {
                    ui.memory_mut(|m| m.request_focus(bar_id));
                    edit.focus_pending = false;
                }
                if edit.cursor_to_end {
                    cursor_to_end(ui, bar_id, &edit.text);
                    edit.cursor_to_end = false;
                }
            }
            let text = match &mut self.edit {
                Some(edit) => &mut edit.text,
                None => &mut self.bar_text,
            };
            let response = ui.add(
                egui::TextEdit::singleline(text)
                    .id(bar_id)
                    .desired_width(f32::INFINITY)
                    .font(egui::FontId::monospace(13.0))
                    .event_filter(EDIT_KEYS),
            );
            accessible_name(ui, &response, "Formulebalk");
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

    pub fn jump_to(&mut self, text: &str) {
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

    /// Clicking a tab: while pointing for a formula it shows the sheet and
    /// keeps editing; otherwise any edit is committed first.
    pub fn click_tab(&mut self, id: SheetId) {
        if self.edit.is_some() {
            if self.wants_reference() {
                self.show_sheet(id);
                self.after_sheet_switch_while_editing();
                return;
            }
            if !self.commit_edit() {
                return;
            }
        }
        self.show_sheet(id);
        self.grid_focus_pending = true;
    }

    fn finish_rename(&mut self) {
        let Some(rename) = self.rename.take() else { return };
        match self.doc.rename_sheet(rename.sheet, &rename.text) {
            Ok(()) => {
                self.status = None;
                self.grid_focus_pending = true;
            }
            Err(e) => {
                self.status = Some(e);
                self.rename = Some(Rename { focus_pending: true, ..rename });
            }
        }
    }

    fn tab_bar(&mut self, ui: &mut Ui, env: &mut Env<'_>) {
        enum Action {
            Show(SheetId),
            Rename,
            Run(SheetId, Command),
        }
        let mut action = None;
        let rename_id = egui::Id::unique(("rename-sheet", env.window));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let add = ui.add(egui::Button::new(" + ").frame(false)).on_hover_text("Nieuw blad (⇧F11)");
            accessible_name(ui, &add, "Nieuw blad");
            if add.clicked() {
                action = Some(Action::Run(self.sheet, Command::AddSheet));
            }
            ui.separator();
            egui::ScrollArea::horizontal().id_salt(("tabs", env.window)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for index in 0..self.doc.book.len() {
                        let sheet = self.doc.book.sheet(index);
                        let (id, name) = (sheet.id(), sheet.name().to_string());
                        if let Some(rename) = self.rename.as_mut().filter(|r| r.sheet == id) {
                            let width = (rename.text.chars().count() as f32 * 7.5 + 24.0).max(60.0);
                            let has_focus = ui.memory(|m| m.has_focus(rename_id));
                            let mut done = None;
                            if has_focus {
                                if take_key(ui, Key::Escape).is_some() {
                                    done = Some(false);
                                } else if take_key(ui, Key::Enter).is_some() {
                                    done = Some(true);
                                }
                            }
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut rename.text)
                                    .id(rename_id)
                                    .desired_width(width)
                                    .event_filter(EDIT_KEYS),
                            );
                            accessible_name(ui, &response, "Bladnaam");
                            if rename.focus_pending {
                                response.request_focus();
                                let text = rename.text.clone();
                                let mut state = egui::text_edit::TextEditState::load(ui.ctx(), rename_id)
                                    .unwrap_or_default();
                                state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                                    egui::text::CCursor::new(0),
                                    egui::text::CCursor::new(text.chars().count()),
                                )));
                                state.store(ui.ctx(), rename_id);
                                rename.focus_pending = false;
                            } else if done.is_none() && response.lost_focus() {
                                done = Some(true);
                            }
                            match done {
                                Some(true) => action = Some(Action::Rename),
                                Some(false) => {
                                    self.rename = None;
                                    self.grid_focus_pending = true;
                                }
                                None => {}
                            }
                            continue;
                        }
                        let selected = id == self.sheet;
                        let label = if selected {
                            egui::RichText::new(&name).strong()
                        } else {
                            egui::RichText::new(&name)
                        };
                        let response = ui.add(egui::Button::selectable(selected, label));
                        if response.clicked() {
                            action = Some(Action::Show(id));
                        }
                        if response.double_clicked() {
                            action = Some(Action::Run(id, Command::RenameSheet));
                        }
                        let only_one = self.doc.book.len() <= 1;
                        response.context_menu(|ui| {
                            if ui.button("Wijzig naam…").clicked() {
                                action = Some(Action::Run(id, Command::RenameSheet));
                                ui.close();
                            }
                            if ui.button("Dupliceer").clicked() {
                                action = Some(Action::Run(id, Command::DuplicateSheet));
                                ui.close();
                            }
                            if ui.add_enabled(!only_one, egui::Button::new("Verwijder")).clicked() {
                                action = Some(Action::Run(id, Command::DeleteSheet));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Nieuw blad").clicked() {
                                action = Some(Action::Run(id, Command::AddSheet));
                                ui.close();
                            }
                        });
                    }
                });
            });
        });
        match action {
            Some(Action::Show(id)) => self.click_tab(id),
            Some(Action::Rename) => self.finish_rename(),
            Some(Action::Run(id, command)) => {
                // Sheet commands apply to the tab they were invoked on.
                if self.edit.is_some() && !self.commit_edit() {
                    return;
                }
                self.show_sheet(id);
                self.run(command, ui, env);
                self.sync_sheet();
            }
            None => {}
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

/// Excel's standard colours, with Dutch names.
const COLORS: [(&str, Rgb); 13] = [
    ("Donkerrood", Rgb(0xC0, 0x00, 0x00)),
    ("Rood", Rgb(0xFF, 0x00, 0x00)),
    ("Oranje", Rgb(0xFF, 0xC0, 0x00)),
    ("Geel", Rgb(0xFF, 0xFF, 0x00)),
    ("Lichtgroen", Rgb(0x92, 0xD0, 0x50)),
    ("Groen", Rgb(0x00, 0xB0, 0x50)),
    ("Lichtblauw", Rgb(0x00, 0xB0, 0xF0)),
    ("Blauw", Rgb(0x00, 0x70, 0xC0)),
    ("Donkerblauw", Rgb(0x00, 0x20, 0x60)),
    ("Paars", Rgb(0x70, 0x30, 0xA0)),
    ("Zwart", Rgb(0x00, 0x00, 0x00)),
    ("Grijs", Rgb(0x80, 0x80, 0x80)),
    ("Wit", Rgb(0xFF, 0xFF, 0xFF)),
];

/// A toolbar button that opens a colour palette. Returns the chosen colour
/// (`Some(None)` for "none") when one is picked.
fn color_menu(ui: &mut Ui, name: &str, current: Option<Rgb>, fill: bool) -> Option<Option<Rgb>> {
    let default = ui.visuals().text_color();
    let shown = current.map_or(default, |c| egui::Color32::from_rgb(c.0, c.1, c.2));
    let mut picked = None;
    // Text colour: an "A" in that colour; fill: a filled square.
    let label = if fill {
        egui::RichText::new("■").color(if current.is_some() { shown } else { ui.visuals().weak_text_color() })
    } else {
        egui::RichText::new("A").color(shown)
    };
    let response = ui
        .menu_button(label, |ui| {
            ui.set_min_width(150.0);
            ui.label(name);
            egui::Grid::new(name).spacing(egui::vec2(4.0, 4.0)).show(ui, |ui| {
                for (i, (label, c)) in COLORS.iter().enumerate() {
                    let (rect, r) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
                    ui.painter().rect_filled(rect, 3.0, egui::Color32::from_rgb(c.0, c.1, c.2));
                    let stroke = if current == Some(*c) { 2.0 } else { 0.5 };
                    ui.painter().rect_stroke(
                        rect,
                        3.0,
                        egui::Stroke::new(stroke, ui.visuals().text_color()),
                        egui::StrokeKind::Inside,
                    );
                    accessible_name(ui, &r, label);
                    if r.on_hover_text(*label).clicked() {
                        picked = Some(Some(*c));
                        ui.close();
                    }
                    if i % 5 == 4 {
                        ui.end_row();
                    }
                }
            });
            if ui.button(if fill { "Geen opvulling" } else { "Automatisch" }).clicked() {
                picked = Some(None);
                ui.close();
            }
        })
        .response;
    accessible_name(ui, &response, name);
    response.on_hover_text(name);
    picked
}

/// Gives a widget a name for VoiceOver and other assistive technology.
pub fn accessible_name(ui: &Ui, response: &egui::Response, name: &str) {
    ui.ctx().accesskit_node_builder(response.id, |node| node.set_label(name.to_string()));
}

/// A toolbar button with a hand-drawn undo (or, mirrored, redo) arrow, so
/// it does not depend on a font having the glyph.
fn icon_button(ui: &mut Ui, enabled: bool, redo: bool) -> egui::Response {
    let size = egui::vec2(26.0, 22.0);
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    accessible_name(ui, &response, if redo { "Opnieuw" } else { "Herstel" });
    let visuals = if enabled { ui.style().interact(&response) } else { &ui.visuals().widgets.noninteractive };
    if response.hovered() && enabled {
        ui.painter().rect_filled(rect, 4.0, visuals.weak_bg_fill);
    }
    let color = if enabled { visuals.fg_stroke.color } else { ui.visuals().weak_text_color() };
    let stroke = egui::Stroke::new(1.6, color);
    let c = rect.center() + egui::vec2(0.0, 1.5);
    let flip = if redo { -1.0 } else { 1.0 };
    // A half-circle arc on the right with a tail ending in an arrowhead.
    let r = 5.0;
    let mut points: Vec<egui::Pos2> = (0..=12)
        .map(|i| {
            let a = std::f32::consts::PI * (i as f32 / 12.0) - std::f32::consts::FRAC_PI_2;
            c + egui::vec2(flip * r * a.cos(), r * a.sin())
        })
        .collect();
    points.reverse();
    let tip = c + egui::vec2(-flip * 5.0, -r);
    points.push(tip);
    ui.painter().add(egui::Shape::line(points, stroke));
    ui.painter().add(egui::Shape::line(
        vec![tip + egui::vec2(flip * 3.0, -3.0), tip, tip + egui::vec2(flip * 3.0, 3.0)],
        stroke,
    ));
    response
}

fn key_event(key: Key, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

/// Keys a cell or formula-bar editor keeps for itself instead of letting
/// egui move focus with them; Enter, Tab and Escape are handled by
/// [`take_edit_keys`].
pub const EDIT_KEYS: egui::EventFilter =
    egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true };

/// Puts the text cursor of field `id` at the end of `text`.
pub fn cursor_to_end(ui: &Ui, id: egui::Id, text: &str) {
    let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
    let end = egui::text::CCursor::new(text.chars().count());
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
    state.store(ui.ctx(), id);
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
