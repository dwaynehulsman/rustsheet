//! The application shell: one native window per document, the menu bar,
//! file dialogs, recent files and closing with unsaved changes.

use std::path::{Path, PathBuf};
use std::time::Instant;

use egui::{ViewportBuilder, ViewportCommand, ViewportId};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use rustsheet_core::io::{self, FileKind, OPEN_EXTENSIONS};
use rustsheet_core::{Document, Workbook};

use crate::bench::{Bench, Shot};
use crate::clip::ClipState;
use crate::menu::{AppMenu, Command};
use crate::recent::RecentFiles;
use crate::view::{Env, Request, SheetView};

/// A file being read on a background thread.
struct Loading {
    path: PathBuf,
    started: Instant,
    result: std::sync::mpsc::Receiver<Result<Workbook, io::IoError>>,
}

struct DocWindow {
    id: u64,
    view: SheetView,
    title_sent: String,
}

pub struct RustsheetApp {
    windows: Vec<DocWindow>,
    menu: Option<AppMenu>,
    recent: RecentFiles,
    clip: ClipState,
    /// Window that last had focus; menu commands go there.
    active: u64,
    next_window: u64,
    next_untitled: usize,
    quitting: bool,
    undo_state: (bool, bool),
    bench: Option<Bench>,
    shot: Option<Shot>,
    loading: Vec<Loading>,
    /// `--select`, applied once the files given at launch are open.
    pending_select: Option<String>,
    /// `--sheet`, the sheet to show once the files are open.
    pending_sheet: Option<String>,
    /// Window showing the "opening…" message.
    loading_status: Option<u64>,
}

fn viewport_of(id: u64, index: usize) -> ViewportId {
    if index == 0 { ViewportId::ROOT } else { ViewportId::from_hash_of(("document", id)) }
}

fn error_dialog(title: &str, message: &str) {
    MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title(title)
        .set_description(message)
        .set_buttons(MessageButtons::Ok)
        .show();
}

impl RustsheetApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        files: Vec<PathBuf>,
        bench: Option<Bench>,
        shot: Option<Shot>,
        select: Option<String>,
        sheet: Option<String>,
    ) -> Self {
        crate::fonts::install(&cc.egui_ctx);
        crate::open_files::set_context(&cc.egui_ctx);
        cc.egui_ctx.options_mut(|o| o.input_options.line_scroll_speed = 44.0);
        let ctx = cc.egui_ctx.clone();
        let menu = AppMenu::install(move || ctx.request_repaint());
        let recent = RecentFiles::load();
        menu.set_recent(recent.files());

        let mut app = RustsheetApp {
            windows: Vec::new(),
            menu: Some(menu),
            recent,
            clip: ClipState::new(),
            active: 0,
            next_window: 0,
            next_untitled: 1,
            quitting: false,
            undo_state: (true, true),
            bench,
            shot,
            loading: Vec::new(),
            pending_select: None,
            pending_sheet: None,
            loading_status: None,
        };
        app.new_window(None);
        for path in files {
            app.open_path(&path, &cc.egui_ctx);
        }
        app.pending_select = select;
        app.pending_sheet = sheet;
        app
    }

    fn new_window(&mut self, loaded: Option<(Document, PathBuf)>) -> u64 {
        let id = self.next_window;
        self.next_window += 1;
        let view = match loaded {
            Some((doc, path)) => SheetView::new(doc, Some(path), 0),
            None => {
                let n = self.next_untitled;
                self.next_untitled += 1;
                SheetView::new(Document::new(Workbook::new()), None, n)
            }
        };
        self.windows.push(DocWindow { id, view, title_sent: String::new() });
        self.active = id;
        id
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.windows.iter().position(|w| w.id == id)
    }

    fn active_index(&self) -> usize {
        self.index_of(self.active).unwrap_or(0)
    }

    fn focus_window(&self, ctx: &egui::Context, index: usize) {
        ctx.send_viewport_cmd_to(viewport_of(self.windows[index].id, index), ViewportCommand::Focus);
    }

    /// Starts loading `path` on a background thread; the window stays
    /// responsive and the document appears when it is ready.
    pub fn open_path(&mut self, path: &Path, ctx: &egui::Context) {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self.windows.iter().position(|w| w.view.path.as_deref() == Some(canonical.as_path()))
        {
            self.active = self.windows[i].id;
            self.focus_window(ctx, i);
            return;
        }
        if self.loading.iter().any(|l| l.path == canonical) {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        let thread_path = canonical.clone();
        std::thread::spawn(move || {
            let _ = tx.send(io::open(&thread_path));
            ctx.request_repaint();
        });
        self.loading.push(Loading { path: canonical, started: Instant::now(), result: rx });
    }

    /// Picks up documents that finished loading.
    fn finish_loads(&mut self) {
        let mut i = 0;
        while i < self.loading.len() {
            let result = match self.loading[i].result.try_recv() {
                Ok(result) => result,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    i += 1;
                    continue;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Err(io::IoError::Format("het inlezen is onverwacht gestopt".into()))
                }
            };
            let Loading { path, started, .. } = self.loading.remove(i);
            match result {
                Ok(book) => {
                    if let Some(bench) = &mut self.bench {
                        bench.loaded(&path, started.elapsed(), &book);
                    }
                    let doc = Document::new(book);
                    let active = self.active_index();
                    if self.windows.get(active).is_some_and(|w| w.view.is_pristine()) {
                        self.windows[active].view = SheetView::new(doc, Some(path.clone()), 0);
                    } else {
                        let id = self.new_window(Some((doc, path.clone())));
                        self.active = id;
                    }
                    self.recent.add(&path);
                    self.refresh_recent();
                }
                Err(e) => {
                    self.recent.remove(&path);
                    self.refresh_recent();
                    error_dialog("Kan bestand niet openen", &format!("{}\n\n{e}", path.display()));
                }
            }
        }
    }

    fn refresh_recent(&self) {
        if let Some(menu) = &self.menu {
            menu.set_recent(self.recent.files());
        }
    }

    /// Saves window `index`, asking for a name when needed. Returns whether it was saved.
    fn save(&mut self, index: usize, ask: bool) -> bool {
        let view = &self.windows[index].view;
        let writable =
            view.path.as_deref().and_then(FileKind::of).is_some_and(|k| k != FileKind::OtherWorkbook);
        let path = if ask || !writable {
            let stem = view
                .path
                .as_deref()
                .and_then(|p| p.file_stem())
                .map_or_else(|| view.name(), |s| s.to_string_lossy().into());
            let mut dialog = FileDialog::new()
                .set_title("Bewaar")
                .set_file_name(format!("{stem}.xlsx"))
                .add_filter("Excel-werkmap", &["xlsx"])
                .add_filter("CSV (komma-gescheiden)", &["csv"]);
            if let Some(dir) = view.path.as_deref().and_then(Path::parent) {
                dialog = dialog.set_directory(dir);
            }
            let Some(mut path) = dialog.save_file() else { return false };
            if FileKind::of(&path).is_none_or(|k| k == FileKind::OtherWorkbook) {
                path.set_extension("xlsx");
            }
            path
        } else {
            view.path.clone().expect("writable path")
        };
        let window = &mut self.windows[index];
        if window.view.edit.is_some() && !window.view.commit_edit() {
            return false;
        }
        let active = window.view.doc.book.index_of(window.view.sheet).unwrap_or(0);
        match io::save(&window.view.doc.book, &path, active) {
            Ok(()) => {
                window.view.doc.mark_saved();
                window.view.path = Some(path.canonicalize().unwrap_or(path.clone()));
                window.view.status = Some(format!("Bewaard als {}", path.display()));
                self.recent.add(&path);
                self.refresh_recent();
                true
            }
            Err(e) => {
                error_dialog("Kan niet bewaren", &format!("{}\n\n{e}", path.display()));
                false
            }
        }
    }

    /// Asks what to do with unsaved changes. Returns false to cancel.
    fn confirm_discard(&mut self, index: usize) -> bool {
        if !self.windows[index].view.doc.is_modified() {
            return true;
        }
        let name = self.windows[index].view.name();
        let answer = MessageDialog::new()
            .set_level(MessageLevel::Warning)
            .set_title("Wijzigingen bewaren?")
            .set_description(format!("Wil je de wijzigingen in “{name}” bewaren?"))
            .set_buttons(MessageButtons::YesNoCancelCustom(
                "Bewaar".into(),
                "Niet bewaren".into(),
                "Annuleer".into(),
            ))
            .show();
        match answer {
            MessageDialogResult::Custom(s) if s == "Bewaar" => self.save(index, false),
            MessageDialogResult::Yes => self.save(index, false),
            MessageDialogResult::Custom(s) if s == "Niet bewaren" => true,
            MessageDialogResult::No => true,
            _ => false,
        }
    }

    fn command(&mut self, command: Command, ctx: &egui::Context) {
        let index = self.active_index();
        match command {
            Command::New => {
                self.new_window(None);
            }
            Command::Open => {
                let start_dir =
                    self.windows[index].view.path.as_deref().and_then(Path::parent).map(Path::to_path_buf);
                let mut dialog = FileDialog::new()
                    .set_title("Open")
                    .add_filter("Rekenbladen", OPEN_EXTENSIONS)
                    .add_filter("Excel-werkmap", &["xlsx"])
                    .add_filter("CSV", &["csv", "tsv", "txt"]);
                if let Some(dir) = start_dir {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(paths) = dialog.pick_files() {
                    for path in paths {
                        self.open_path(&path, ctx);
                    }
                }
            }
            Command::OpenRecent(path) => self.open_path(&path, ctx),
            Command::ClearRecent => {
                self.recent.clear();
                self.refresh_recent();
            }
            Command::Save => {
                self.save(index, false);
            }
            Command::SaveAs => {
                self.save(index, true);
            }
            Command::ExportPdf | Command::Print => {
                let view = &self.windows[index].view;
                let pdf = rustsheet_core::pdf::sheet_to_pdf(view.sheet(), view.print_range());
                let stem = format!(
                    "{} - {}",
                    view.name().trim_end_matches(".xlsx").trim_end_matches(".csv"),
                    view.sheet().name()
                );
                let path = if command == Command::Print {
                    // Print through Preview, which brings the macOS print dialog.
                    let dir = std::env::temp_dir().join("rustsheet-afdrukken");
                    let _ = std::fs::create_dir_all(&dir);
                    Some(dir.join(format!("{stem}.pdf")))
                } else {
                    FileDialog::new()
                        .set_title("Exporteer als PDF")
                        .set_file_name(format!("{stem}.pdf"))
                        .add_filter("PDF", &["pdf"])
                        .save_file()
                };
                let Some(path) = path else { return };
                let printing = command == Command::Print;
                match std::fs::write(&path, pdf) {
                    Ok(()) => {
                        let view = &mut self.windows[index].view;
                        if printing {
                            let _ = std::process::Command::new("open").arg(&path).spawn();
                            view.status = Some("Geopend in Voorvertoning: druk af met ⌘P".into());
                        } else {
                            view.status = Some(format!("Geëxporteerd naar {}", path.display()));
                        }
                    }
                    Err(e) => error_dialog("Kan PDF niet maken", &format!("{}\n\n{e}", path.display())),
                }
            }
            Command::ExportCsv => {
                let view = &self.windows[index].view;
                let stem = view.sheet().name().to_string();
                let Some(path) = FileDialog::new()
                    .set_title("Exporteer als CSV")
                    .set_file_name(format!("{stem}.csv"))
                    .add_filter("CSV", &["csv"])
                    .save_file()
                else {
                    return;
                };
                match io::write_csv(view.sheet(), &path) {
                    Ok(()) => {
                        self.windows[index].view.status =
                            Some(format!("Geëxporteerd naar {}", path.display()))
                    }
                    Err(e) => error_dialog("Kan CSV niet maken", &format!("{}\n\n{e}", path.display())),
                }
            }
            Command::Close => {
                ctx.send_viewport_cmd_to(viewport_of(self.windows[index].id, index), ViewportCommand::Close);
            }
            Command::Quit => {
                for i in 0..self.windows.len() {
                    if !self.confirm_discard(i) {
                        return;
                    }
                }
                self.quitting = true;
                ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
            }
            other => self.windows[index].view.push_command(other),
        }
    }

    fn show_window(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        requests: &mut Vec<Request>,
        closing: &mut Vec<u64>,
    ) {
        let ctx = ui.ctx().clone();
        let window = &mut self.windows[index];
        let (focused, close) = ctx.input(|i| (i.viewport().focused, i.viewport().close_requested()));
        if focused == Some(true) {
            self.active = window.id;
        }
        if close {
            closing.push(window.id);
        }
        let title = window.view.title();
        if title != window.title_sent {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            window.title_sent = title;
        }
        let mut env = Env { clip: &mut self.clip, window: window.id, requests };
        window.view.ui(ui, &mut env);
    }

    fn handle_closes(&mut self, ctx: &egui::Context, closing: Vec<u64>) {
        for id in closing {
            let Some(index) = self.index_of(id) else { continue };
            let viewport = viewport_of(id, index);
            if self.quitting {
                continue;
            }
            if !self.confirm_discard(index) {
                ctx.send_viewport_cmd_to(viewport, ViewportCommand::CancelClose);
                continue;
            }
            if index == 0 {
                if self.windows.len() == 1 {
                    // Last window: let it close, which ends the app.
                    continue;
                }
                // The root window cannot close while others stay open, so it
                // takes over the next document and that window closes instead.
                ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::CancelClose);
                self.windows.remove(0);
                self.windows[0].title_sent.clear();
            } else {
                self.windows.remove(index);
            }
            if self.index_of(self.active).is_none() {
                self.active = self.windows[0].id;
            }
        }
    }
}

impl eframe::App for RustsheetApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(menu) = &self.menu {
            for command in menu.poll() {
                self.command(command, &ctx);
            }
        }
        for path in crate::open_files::take() {
            self.open_path(&path, &ctx);
        }
        self.finish_loads();
        match self.loading.first() {
            Some(loading) => {
                let name = loading.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into());
                let index = self.active_index();
                self.windows[index].view.status = Some(format!("{name} openen…"));
                self.loading_status = Some(self.windows[index].id);
                ui.ctx().set_cursor_icon(egui::CursorIcon::Progress);
            }
            None => {
                if let Some(i) = self.loading_status.take().and_then(|id| self.index_of(id)) {
                    self.windows[i].view.status = None;
                }
                if let Some(name) = self.pending_sheet.take() {
                    let index = self.active_index();
                    let view = &mut self.windows[index].view;
                    if let Some(id) = view.doc.book.find(&name) {
                        view.show_sheet(id);
                    }
                }
                if let Some(range) = self.pending_select.take() {
                    let index = self.active_index();
                    self.windows[index].view.jump_to(&range);
                }
            }
        }
        if let Some(bench) = &mut self.bench
            && let Some(window) = self.windows.first_mut()
            && bench.step(&ctx, frame, &mut window.view, self.loading.is_empty())
        {
            self.quitting = true;
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
        }

        if let Some(shot) = &mut self.shot
            && shot.step(&ctx, self.loading.is_empty())
        {
            self.quitting = true;
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
        }

        let mut requests = Vec::new();
        let mut closing = Vec::new();
        self.show_window(ui, 0, &mut requests, &mut closing);
        for index in 1..self.windows.len() {
            let id = self.windows[index].id;
            let builder = ViewportBuilder::default()
                .with_title(self.windows[index].view.title())
                .with_inner_size([1100.0, 720.0])
                .with_min_inner_size([480.0, 320.0]);
            ctx.show_viewport_immediate(viewport_of(id, index), builder, |ui, _class| {
                self.show_window(ui, index, &mut requests, &mut closing);
            });
        }

        for request in requests {
            match request {
                Request::ClearCut(cut) => {
                    if let Some(i) = self.index_of(cut.window) {
                        self.windows[i].view.clear_cut_source(cut.sheet, cut.range);
                    }
                }
            }
        }
        self.handle_closes(&ctx, closing);

        // Keep Undo/Redo in the menu in step with the active window.
        if let (Some(menu), Some(window)) = (&self.menu, self.windows.get(self.active_index())) {
            let editing = window.view.edit.is_some();
            let state = (window.view.doc.can_undo() || editing, window.view.doc.can_redo() || editing);
            if state != self.undo_state {
                menu.set_undo_state(state.0, state.1);
                self.undo_state = state;
            }
        }
    }
}
