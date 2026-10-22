//! The native macOS menu bar, built with muda. Menu items carry the Cmd
//! shortcuts, so macOS shows them in the menu and handles the key presses.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use muda::accelerator::Accelerator;
use muda::{AboutMetadata, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use rustsheet_core::{Align, NumFmt};

/// Everything the menu bar, toolbar and shortcuts can ask for.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    New,
    Open,
    OpenRecent(PathBuf),
    ClearRecent,
    Save,
    SaveAs,
    Close,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Delete,
    Bold,
    Align(Align),
    NumFmt(NumFmt),
    AddSheet,
    DuplicateSheet,
    RenameSheet,
    DeleteSheet,
    NextSheet,
    PrevSheet,
    InsertRows,
    InsertCols,
    DeleteRows,
    DeleteCols,
}

impl Command {
    fn id(&self) -> String {
        match self {
            Command::New => "new".into(),
            Command::Open => "open".into(),
            Command::OpenRecent(p) => format!("recent:{}", p.display()),
            Command::ClearRecent => "clear-recent".into(),
            Command::Save => "save".into(),
            Command::SaveAs => "save-as".into(),
            Command::Close => "close".into(),
            Command::Quit => "quit".into(),
            Command::Undo => "undo".into(),
            Command::Redo => "redo".into(),
            Command::Cut => "cut".into(),
            Command::Copy => "copy".into(),
            Command::Paste => "paste".into(),
            Command::SelectAll => "select-all".into(),
            Command::Delete => "delete".into(),
            Command::Bold => "bold".into(),
            Command::Align(a) => format!("align:{a:?}"),
            Command::NumFmt(f) => format!("numfmt:{f:?}"),
            Command::AddSheet => "add-sheet".into(),
            Command::DuplicateSheet => "duplicate-sheet".into(),
            Command::RenameSheet => "rename-sheet".into(),
            Command::DeleteSheet => "delete-sheet".into(),
            Command::NextSheet => "next-sheet".into(),
            Command::PrevSheet => "prev-sheet".into(),
            Command::InsertRows => "insert-rows".into(),
            Command::InsertCols => "insert-cols".into(),
            Command::DeleteRows => "delete-rows".into(),
            Command::DeleteCols => "delete-cols".into(),
        }
    }

    fn from_id(id: &str) -> Option<Command> {
        if let Some(path) = id.strip_prefix("recent:") {
            return Some(Command::OpenRecent(PathBuf::from(path)));
        }
        let all = [
            Command::New,
            Command::Open,
            Command::ClearRecent,
            Command::Save,
            Command::SaveAs,
            Command::Close,
            Command::Quit,
            Command::Undo,
            Command::Redo,
            Command::Cut,
            Command::Copy,
            Command::Paste,
            Command::SelectAll,
            Command::Delete,
            Command::Bold,
            Command::Align(Align::General),
            Command::Align(Align::Left),
            Command::Align(Align::Center),
            Command::Align(Align::Right),
            Command::AddSheet,
            Command::DuplicateSheet,
            Command::RenameSheet,
            Command::DeleteSheet,
            Command::NextSheet,
            Command::PrevSheet,
            Command::InsertRows,
            Command::InsertCols,
            Command::DeleteRows,
            Command::DeleteCols,
        ];
        all.into_iter().chain(NumFmt::ALL.into_iter().map(Command::NumFmt)).find(|c| c.id() == id)
    }
}

fn item(text: &str, command: Command, shortcut: Option<&str>) -> MenuItem {
    let accel = shortcut.map(|s| s.parse::<Accelerator>().expect("valid accelerator"));
    MenuItem::with_id(command.id(), text, true, accel)
}

pub struct AppMenu {
    /// Must stay alive for the menu bar to keep working.
    _menu: Menu,
    recent: Submenu,
    undo: MenuItem,
    redo: MenuItem,
    events: Receiver<Command>,
}

impl AppMenu {
    /// Builds the menu bar and installs it. `wake` is called after each menu
    /// event so the UI repaints and handles it.
    pub fn install(wake: impl Fn() + Send + Sync + 'static) -> AppMenu {
        let menu = Menu::new();

        let about = AboutMetadata {
            name: Some("Rustsheet".into()),
            version: Some(env!("CARGO_PKG_VERSION").into()),
            comments: Some("Een spreadsheet in Rust, gemaakt voor macOS.".into()),
            ..Default::default()
        };
        let app = Submenu::with_items(
            "Rustsheet",
            true,
            &[
                &PredefinedMenuItem::about(Some("Over Rustsheet"), Some(about)),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::services(Some("Voorzieningen")),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::hide(Some("Verberg Rustsheet")),
                &PredefinedMenuItem::hide_others(Some("Verberg andere")),
                &PredefinedMenuItem::show_all(Some("Toon alles")),
                &PredefinedMenuItem::separator(),
                &item("Stop Rustsheet", Command::Quit, Some("CmdOrCtrl+Q")),
            ],
        )
        .expect("app menu");

        let recent = Submenu::new("Open recent", true);
        let file = Submenu::with_items(
            "Archief",
            true,
            &[
                &item("Nieuw", Command::New, Some("CmdOrCtrl+N")),
                &item("Open…", Command::Open, Some("CmdOrCtrl+O")),
                &recent,
                &PredefinedMenuItem::separator(),
                &item("Sluit venster", Command::Close, Some("CmdOrCtrl+W")),
                &item("Bewaar", Command::Save, Some("CmdOrCtrl+S")),
                &item("Bewaar als…", Command::SaveAs, Some("CmdOrCtrl+Shift+S")),
            ],
        )
        .expect("file menu");

        let undo = item("Herstel", Command::Undo, Some("CmdOrCtrl+Z"));
        let redo = item("Opnieuw", Command::Redo, Some("CmdOrCtrl+Shift+Z"));
        let edit = Submenu::with_items(
            "Wijzig",
            true,
            &[
                &undo,
                &redo,
                &PredefinedMenuItem::separator(),
                &item("Knip", Command::Cut, Some("CmdOrCtrl+X")),
                &item("Kopieer", Command::Copy, Some("CmdOrCtrl+C")),
                &item("Plak", Command::Paste, Some("CmdOrCtrl+V")),
                &item("Wis", Command::Delete, None),
                &item("Selecteer alles", Command::SelectAll, Some("CmdOrCtrl+A")),
                &PredefinedMenuItem::separator(),
                &item("Voeg rijen in", Command::InsertRows, Some("CmdOrCtrl+Shift+Equal")),
                &item("Voeg kolommen in", Command::InsertCols, None),
                &item("Verwijder rijen", Command::DeleteRows, Some("CmdOrCtrl+Minus")),
                &item("Verwijder kolommen", Command::DeleteCols, None),
            ],
        )
        .expect("edit menu");

        let formats = Submenu::with_items(
            "Getalnotatie",
            true,
            &NumFmt::ALL
                .iter()
                .map(|f| {
                    let shortcut = match f {
                        NumFmt::General => Some("Ctrl+Shift+Digit0"),
                        NumFmt::Number => Some("Ctrl+Shift+Digit1"),
                        NumFmt::Date => Some("Ctrl+Shift+Digit3"),
                        NumFmt::Currency => Some("Ctrl+Shift+Digit4"),
                        NumFmt::Percent => Some("Ctrl+Shift+Digit5"),
                    };
                    item(f.label(), Command::NumFmt(*f), shortcut)
                })
                .collect::<Vec<_>>()
                .iter()
                .map(|i| i as &dyn muda::IsMenuItem)
                .collect::<Vec<_>>(),
        )
        .expect("format submenu");
        let format = Submenu::with_items(
            "Opmaak",
            true,
            &[
                &item("Vet", Command::Bold, Some("CmdOrCtrl+B")),
                &PredefinedMenuItem::separator(),
                &item("Lijn links uit", Command::Align(Align::Left), Some("CmdOrCtrl+Shift+BracketLeft")),
                &item("Centreer", Command::Align(Align::Center), Some("CmdOrCtrl+Shift+Backslash")),
                &item("Lijn rechts uit", Command::Align(Align::Right), Some("CmdOrCtrl+Shift+BracketRight")),
                &item("Standaarduitlijning", Command::Align(Align::General), None),
                &PredefinedMenuItem::separator(),
                &formats,
            ],
        )
        .expect("format menu");

        let sheet = Submenu::with_items(
            "Blad",
            true,
            &[
                &item("Nieuw blad", Command::AddSheet, Some("Shift+F11")),
                &item("Dupliceer blad", Command::DuplicateSheet, None),
                &item("Wijzig naam…", Command::RenameSheet, None),
                &item("Verwijder blad", Command::DeleteSheet, None),
                &PredefinedMenuItem::separator(),
                &item("Volgend blad", Command::NextSheet, Some("Alt+CmdOrCtrl+ArrowRight")),
                &item("Vorig blad", Command::PrevSheet, Some("Alt+CmdOrCtrl+ArrowLeft")),
            ],
        )
        .expect("sheet menu");

        let window = Submenu::with_items(
            "Venster",
            true,
            &[
                &PredefinedMenuItem::minimize(Some("Minimaliseer")),
                &PredefinedMenuItem::maximize(Some("Zoom")),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::bring_all_to_front(Some("Alles op voorgrond")),
            ],
        )
        .expect("window menu");

        menu.append_items(&[&app, &file, &edit, &format, &sheet, &window]).expect("menu bar");
        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();

        let (tx, rx) = channel();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(command) = Command::from_id(event.id().as_ref()) {
                let _ = tx.send(command);
                wake();
            }
        }));

        AppMenu { _menu: menu, recent, undo, redo, events: rx }
    }

    pub fn poll(&self) -> Vec<Command> {
        self.events.try_iter().collect()
    }

    pub fn set_undo_state(&self, can_undo: bool, can_redo: bool) {
        self.undo.set_enabled(can_undo);
        self.redo.set_enabled(can_redo);
    }

    /// Rebuilds the Open Recent submenu.
    pub fn set_recent(&self, files: &[PathBuf]) {
        while self.recent.remove_at(0).is_some() {}
        for path in files {
            let name =
                path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into());
            let _ = self.recent.append(&item(&name, Command::OpenRecent(path.clone()), None));
        }
        if !files.is_empty() {
            let _ = self.recent.append(&PredefinedMenuItem::separator());
        }
        let _ = self.recent.append(&MenuItem::with_id(
            Command::ClearRecent.id(),
            "Wis menu",
            !files.is_empty(),
            None,
        ));
    }
}
