//! Interaction tests: real key and pointer events through the whole view.

use egui::{Event, Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use rustsheet_core::{CellRef, Document, Value, Workbook};

use crate::clip::ClipState;
use crate::menu::Command;
use crate::view::{Env, Request, SheetView};

struct State {
    fonts_ready: bool,
    view: SheetView,
    clip: ClipState,
    requests: Vec<Request>,
}

fn harness<'a>() -> Harness<'a, State> {
    let state = State {
        fonts_ready: false,
        view: SheetView::new(Document::new(Workbook::new()), None, 1),
        clip: ClipState::detached(),
        requests: Vec::new(),
    };
    let mut h = Harness::builder().with_size([900.0, 600.0]).build_ui_state(
        |ui, s: &mut State| {
            // Install the app fonts first; they apply from the next frame.
            if !s.fonts_ready {
                crate::fonts::install(ui.ctx());
                s.fonts_ready = true;
                ui.ctx().request_repaint();
                return;
            }
            let mut env = Env { clip: &mut s.clip, window: 7, requests: &mut s.requests };
            s.view.ui(ui, &mut env);
        },
        state,
    );
    h.run();
    h
}

fn c(s: &str) -> CellRef {
    CellRef::parse(s).unwrap()
}

fn text(h: &mut Harness<'_, State>, t: &str) {
    h.event(Event::Text(t.to_string()));
    h.run();
}

fn key(h: &mut Harness<'_, State>, k: Key) {
    h.key_press(k);
    h.run();
}

fn key_mod(h: &mut Harness<'_, State>, m: Modifiers, k: Key) {
    h.key_press_modifiers(m, k);
    h.run();
}

fn command(h: &mut Harness<'_, State>, command: Command) {
    h.state_mut().view.push_command(command);
    h.run();
}

fn shown(h: &Harness<'_, State>, at: &str) -> String {
    h.state().view.sheet().display(c(at))
}

fn click(h: &mut Harness<'_, State>, at: &str, modifiers: Modifiers) {
    let pos = h.state().view.cell_center(c(at));
    h.hover_at(pos);
    h.event_modifiers(
        Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers },
        modifiers,
    );
    h.event_modifiers(
        Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers },
        modifiers,
    );
    h.run();
}

#[test]
fn typing_enter_and_tab_commit_and_move() {
    let mut h = harness();
    text(&mut h, "4");
    text(&mut h, "2");
    assert!(h.state().view.edit.is_some());
    key(&mut h, Key::Enter);
    assert_eq!(shown(&h, "A1"), "42");
    assert_eq!(h.state().view.sel.active, c("A2"));

    text(&mut h, "=A1*2");
    key(&mut h, Key::Tab);
    assert_eq!(shown(&h, "A2"), "84");
    assert_eq!(h.state().view.sel.active, c("B2"));
}

#[test]
fn escape_cancels_and_f2_edits_existing_text() {
    let mut h = harness();
    text(&mut h, "hallo");
    key(&mut h, Key::Escape);
    assert_eq!(shown(&h, "A1"), "");
    assert!(h.state().view.edit.is_none());

    text(&mut h, "abc");
    key(&mut h, Key::Enter);
    key(&mut h, Key::ArrowUp);
    key(&mut h, Key::F2);
    text(&mut h, "d");
    key(&mut h, Key::Enter);
    assert_eq!(shown(&h, "A1"), "abcd");

    // Enter on a selected cell starts editing too.
    key(&mut h, Key::ArrowUp);
    key(&mut h, Key::Enter);
    assert!(h.state().view.edit.as_ref().is_some_and(|e| e.text == "abcd" && !e.typed));
}

#[test]
fn arrows_in_typed_mode_commit_and_move() {
    let mut h = harness();
    text(&mut h, "1");
    key(&mut h, Key::ArrowRight);
    assert_eq!(shown(&h, "A1"), "1");
    assert_eq!(h.state().view.sel.active, c("B1"));
}

#[test]
fn shift_arrows_extend_and_delete_clears() {
    let mut h = harness();
    for v in ["1", "2", "3"] {
        text(&mut h, v);
        key(&mut h, Key::Enter);
    }
    // Like Excel: from an empty cell Cmd+Up stops at the nearest value, then
    // at the top of that block.
    key_mod(&mut h, Modifiers::COMMAND, Key::ArrowUp);
    assert_eq!(h.state().view.sel.active, c("A3"));
    key_mod(&mut h, Modifiers::COMMAND, Key::ArrowUp);
    assert_eq!(h.state().view.sel.active, c("A1"));
    key_mod(&mut h, Modifiers::SHIFT, Key::ArrowDown);
    key_mod(&mut h, Modifiers::SHIFT, Key::ArrowDown);
    assert_eq!(h.state().view.sel.range().to_string(), "A1:A3");
    key(&mut h, Key::Delete);
    assert_eq!(shown(&h, "A2"), "");
    command(&mut h, Command::Undo);
    assert_eq!(shown(&h, "A2"), "2");
    command(&mut h, Command::Redo);
    assert_eq!(shown(&h, "A2"), "");
}

#[test]
fn bad_formula_keeps_editing_with_message() {
    let mut h = harness();
    text(&mut h, "=1+");
    key(&mut h, Key::Enter);
    let view = &h.state().view;
    assert!(view.edit.is_some());
    assert!(view.status.as_deref().is_some_and(|s| s.starts_with("Formule klopt niet")));
    assert_eq!(view.sheet().display(c("A1")), "");
}

#[test]
fn copy_paste_shifts_references_and_cut_moves() {
    let mut h = harness();
    text(&mut h, "5");
    key(&mut h, Key::Tab);
    text(&mut h, "=A1*10");
    key(&mut h, Key::Enter);
    click(&mut h, "B1", Modifiers::NONE);
    command(&mut h, Command::Copy);
    click(&mut h, "B3", Modifiers::NONE);
    command(&mut h, Command::Paste);
    assert_eq!(h.state().view.sheet().state(c("B3")).content.source(), "=A3*10");

    // Cut keeps references and empties the source.
    click(&mut h, "B1", Modifiers::NONE);
    command(&mut h, Command::Cut);
    click(&mut h, "D5", Modifiers::NONE);
    command(&mut h, Command::Paste);
    assert_eq!(h.state().view.sheet().state(c("D5")).content.source(), "=A1*10");
    assert_eq!(shown(&h, "D5"), "50");
    assert_eq!(shown(&h, "B1"), "");
    // One undo restores both the source and the target.
    command(&mut h, Command::Undo);
    assert_eq!(shown(&h, "B1"), "50");
    assert_eq!(shown(&h, "D5"), "");
}

#[test]
fn paste_from_another_app_reads_tab_separated_values() {
    let mut h = harness();
    h.state_mut().clip.set_foreign_text("Naam\tPrijs\nThee\t€ 2.50\n");
    click(&mut h, "B2", Modifiers::NONE);
    command(&mut h, Command::Paste);
    assert_eq!(shown(&h, "B2"), "Naam");
    assert_eq!(h.state().view.sheet().value(c("C3")), &Value::Number(2.5));
    assert_eq!(h.state().view.sel.range().to_string(), "B2:C3");
}

#[test]
fn shift_click_extends_selection_and_formats_apply() {
    let mut h = harness();
    click(&mut h, "B2", Modifiers::NONE);
    click(&mut h, "C4", Modifiers::SHIFT);
    assert_eq!(h.state().view.sel.range().to_string(), "B2:C4");
    command(&mut h, Command::Bold);
    assert!(h.state().view.sheet().format(c("C3")).bold);
    command(&mut h, Command::NumFmt(rustsheet_core::NumFmt::Percent));
    assert_eq!(h.state().view.sheet().format(c("B4")).num, rustsheet_core::NumFmt::Percent);
}

#[test]
fn clicking_cells_while_typing_a_formula_inserts_references() {
    let mut h = harness();
    for v in ["2", "3"] {
        text(&mut h, v);
        key(&mut h, Key::Enter);
    }
    click(&mut h, "C1", Modifiers::NONE);
    text(&mut h, "=");
    click(&mut h, "A1", Modifiers::NONE);
    text(&mut h, "+");
    click(&mut h, "A2", Modifiers::NONE);
    assert_eq!(h.state().view.edit.as_ref().map(|e| e.text.as_str()), Some("=A1+A2"));
    key(&mut h, Key::Enter);
    assert_eq!(shown(&h, "C1"), "5");
}

#[test]
fn double_click_edits_the_cell() {
    let mut h = harness();
    text(&mut h, "x");
    key(&mut h, Key::Enter);
    key(&mut h, Key::ArrowUp);
    let pos = h.state().view.cell_center(c("A1"));
    // The harness clock moves 0.25 s per frame and delivers press and
    // release in separate frames, so allow a slower double click.
    h.ctx.options_mut(|o| o.input_options.max_double_click_delay = 2.0);
    for _ in 0..2 {
        for pressed in [true, false] {
            h.event(Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        h.step();
    }
    h.run();
    assert!(h.state().view.edit.as_ref().is_some_and(|e| e.text == "x"));
}

#[test]
fn dragging_a_column_border_resizes_and_undoes() {
    let mut h = harness();
    let view = &h.state().view;
    let a = view.cell_center(c("A1"));
    let b = view.cell_center(c("B1"));
    // The border between A and B, in the column header row above row 1.
    let start = egui::pos2((a.x + b.x) / 2.0, a.y - crate::grid::HEADER_H);
    h.hover_at(start);
    h.event(Event::PointerButton {
        pos: start,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.step();
    let end = start + egui::vec2(60.0, 0.0);
    h.event(Event::PointerMoved(end));
    h.step();
    h.event(Event::PointerButton {
        pos: end,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run();
    let width = h.state().view.sheet().col_width(0);
    assert!((width - (rustsheet_core::sheet::DEFAULT_COL_WIDTH + 60.0)).abs() < 1.0, "width {width}");
    command(&mut h, Command::Undo);
    assert_eq!(h.state().view.sheet().custom_col_width(0), None);
}

fn sheet_names(h: &Harness<'_, State>) -> Vec<String> {
    h.state().view.doc.book.sheets().iter().map(|s| s.name().to_string()).collect()
}

fn current_sheet(h: &Harness<'_, State>) -> String {
    h.state().view.sheet().name().to_string()
}

#[test]
fn tabs_add_switch_and_keep_their_own_cells() {
    let mut h = harness();
    text(&mut h, "op blad 1");
    key(&mut h, Key::Enter);
    h.get_by_label("Nieuw blad").click();
    h.run();
    assert_eq!(sheet_names(&h), ["Blad1", "Blad2"]);
    assert_eq!(current_sheet(&h), "Blad2");
    assert_eq!(shown(&h, "A1"), "");
    text(&mut h, "op blad 2");
    key(&mut h, Key::Enter);

    // Ctrl+Page Up/Down and Cmd+Option+arrows switch sheets.
    key_mod(&mut h, Modifiers::CTRL, Key::PageUp);
    assert_eq!(current_sheet(&h), "Blad1");
    assert_eq!(shown(&h, "A1"), "op blad 1");
    // Each sheet remembers its own selection.
    assert_eq!(h.state().view.sel.active, c("A2"));
    key_mod(&mut h, Modifiers::COMMAND | Modifiers::ALT, Key::ArrowRight);
    assert_eq!(current_sheet(&h), "Blad2");
    assert_eq!(shown(&h, "A1"), "op blad 2");

    // Clicking a tab switches too.
    h.get_by_label("Blad1").click();
    h.run();
    assert_eq!(current_sheet(&h), "Blad1");
}

#[test]
fn formulas_point_at_cells_on_other_tabs() {
    let mut h = harness();
    command(&mut h, Command::AddSheet);
    text(&mut h, "21");
    key(&mut h, Key::Enter);
    h.get_by_label("Blad1").click();
    h.run();

    // Type "=", click the Blad2 tab and a cell there, then "*2" and Enter.
    text(&mut h, "=");
    h.get_by_label("Blad2").click();
    h.run();
    assert_eq!(current_sheet(&h), "Blad2");
    click(&mut h, "A1", Modifiers::NONE);
    text(&mut h, "*2");
    assert_eq!(h.state().view.edit.as_ref().map(|e| e.text.as_str()), Some("=Blad2!A1*2"));
    key(&mut h, Key::Enter);
    assert_eq!(current_sheet(&h), "Blad1");
    assert_eq!(shown(&h, "A1"), "42");
    assert_eq!(h.state().view.sheet().state(c("A1")).content.source(), "=Blad2!A1*2");
}

#[test]
fn renaming_a_tab_updates_formulas() {
    let mut h = harness();
    command(&mut h, Command::AddSheet);
    text(&mut h, "5");
    key(&mut h, Key::Enter);
    key_mod(&mut h, Modifiers::CTRL, Key::PageUp);
    text(&mut h, "=Blad2!A1+1");
    key(&mut h, Key::Enter);

    // Renaming (double-click on the tab, or the menu) selects the old name; typing replaces it.
    key_mod(&mut h, Modifiers::CTRL, Key::PageDown);
    command(&mut h, Command::RenameSheet);
    assert!(h.state().view.rename.is_some());
    text(&mut h, "Mijn kosten");
    key(&mut h, Key::Enter);
    assert_eq!(sheet_names(&h), ["Blad1", "Mijn kosten"]);
    let first = h.state().view.doc.book.sheet(0);
    assert_eq!(first.state(c("A1")).content.source(), "='Mijn kosten'!A1+1");
    assert_eq!(first.display(c("A1")), "6");

    // A name that is taken is refused and the field stays open.
    command(&mut h, Command::RenameSheet);
    text(&mut h, "blad1");
    key(&mut h, Key::Enter);
    assert!(h.state().view.rename.is_some());
    assert!(h.state().view.status.as_deref().is_some_and(|s| s.contains("al een blad")));
    key(&mut h, Key::Escape);
    assert!(h.state().view.rename.is_none());
    assert_eq!(sheet_names(&h), ["Blad1", "Mijn kosten"]);

    // Undo puts the old name and formulas back.
    command(&mut h, Command::Undo);
    assert_eq!(sheet_names(&h), ["Blad1", "Blad2"]);
    assert_eq!(h.state().view.doc.book.sheet(0).state(c("A1")).content.source(), "=Blad2!A1+1");
}

#[test]
fn tab_context_menu_duplicates_and_deletes() {
    let mut h = harness();
    text(&mut h, "x");
    key(&mut h, Key::Enter);
    h.get_by_label("Blad1").click_secondary();
    h.run();
    h.get_by_label("Dupliceer").click();
    h.run();
    assert_eq!(sheet_names(&h), ["Blad1", "Blad1 (2)"]);
    assert_eq!(current_sheet(&h), "Blad1 (2)");
    assert_eq!(shown(&h, "A1"), "x");

    h.get_by_label("Blad1 (2)").click_secondary();
    h.run();
    h.get_by_label("Verwijder").click();
    h.run();
    assert_eq!(sheet_names(&h), ["Blad1"]);
    assert_eq!(current_sheet(&h), "Blad1");

    // The last sheet cannot be deleted.
    command(&mut h, Command::DeleteSheet);
    assert_eq!(sheet_names(&h), ["Blad1"]);

    // Undoing the delete brings the sheet back and shows it.
    command(&mut h, Command::Undo);
    assert_eq!(sheet_names(&h), ["Blad1", "Blad1 (2)"]);
    assert_eq!(current_sheet(&h), "Blad1 (2)");
}

fn right_click(h: &mut Harness<'_, State>, at: &str) {
    let pos = h.state().view.cell_center(c(at));
    h.hover_at(pos);
    h.event(Event::PointerButton {
        pos,
        button: PointerButton::Secondary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.event(Event::PointerButton {
        pos,
        button: PointerButton::Secondary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run();
}

#[test]
fn context_menu_inserts_and_deletes_rows_and_columns() {
    let mut h = harness();
    for v in ["1", "2", "3"] {
        text(&mut h, v);
        key(&mut h, Key::Enter);
    }
    text(&mut h, "=SUM(A1:A3)");
    key(&mut h, Key::Enter);

    right_click(&mut h, "A2");
    assert_eq!(h.state().view.sel.active, c("A2"));
    h.get_by_label("Voeg rij in boven rij 2").click();
    h.run();
    assert_eq!(h.state().view.sheet().state(c("A5")).content.source(), "=SUM(A1:A4)");
    assert_eq!(shown(&h, "A3"), "2");

    // Two selected rows delete together.
    click(&mut h, "A2", Modifiers::NONE);
    click(&mut h, "A3", Modifiers::SHIFT);
    right_click(&mut h, "A3");
    h.get_by_label("Verwijder 2 rijen").click();
    h.run();
    assert_eq!(h.state().view.sheet().state(c("A3")).content.source(), "=SUM(A1:A2)");
    assert_eq!(shown(&h, "A3"), "4");

    // Menu commands for columns, and undo.
    command(&mut h, Command::InsertCols);
    assert_eq!(h.state().view.sheet().state(c("B3")).content.source(), "=SUM(B1:B2)");
    command(&mut h, Command::Undo);
    command(&mut h, Command::Undo);
    command(&mut h, Command::Undo);
    assert_eq!(h.state().view.sheet().state(c("A4")).content.source(), "=SUM(A1:A3)");
    assert_eq!(shown(&h, "A4"), "6");
}

fn drag(h: &mut Harness<'_, State>, from: egui::Pos2, to: egui::Pos2) {
    h.hover_at(from);
    h.event(Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.step();
    h.event(Event::PointerMoved(to));
    h.step();
    h.event(Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run();
}

#[test]
fn dragging_the_fill_handle_continues_a_series() {
    let mut h = harness();
    for v in ["1", "2"] {
        text(&mut h, v);
        key(&mut h, Key::Enter);
    }
    click(&mut h, "A1", Modifiers::NONE);
    click(&mut h, "A2", Modifiers::SHIFT);
    let handle = h.state().view.fill_handle_center().expect("fill handle visible");
    let to = h.state().view.cell_center(c("A6"));
    drag(&mut h, handle, to);
    for (at, v) in [("A3", "3"), ("A4", "4"), ("A5", "5"), ("A6", "6")] {
        assert_eq!(shown(&h, at), v, "{at}");
    }
    assert_eq!(h.state().view.sel.range().to_string(), "A1:A6");

    // Fill Right copies the first column across the selection.
    click(&mut h, "A1", Modifiers::NONE);
    click(&mut h, "C2", Modifiers::SHIFT);
    command(&mut h, Command::FillRight);
    assert_eq!(shown(&h, "C2"), "2");
    command(&mut h, Command::Undo);
    assert_eq!(shown(&h, "C2"), "");
}

#[test]
fn find_and_replace_bar() {
    let mut h = harness();
    for v in ["Appel", "Peer", "appeltaart", "=LEN(A1)"] {
        text(&mut h, v);
        key(&mut h, Key::Enter);
    }
    command(&mut h, Command::Find);
    assert!(h.state().view.find.is_some());
    text(&mut h, "appel");
    key(&mut h, Key::Enter);
    assert_eq!(h.state().view.sel.active, c("A1"));
    key(&mut h, Key::Enter);
    assert_eq!(h.state().view.sel.active, c("A3"));
    assert_eq!(h.state().view.find.as_ref().unwrap().info, "2 van 2");
    key_mod(&mut h, Modifiers::SHIFT, Key::Enter);
    assert_eq!(h.state().view.sel.active, c("A1"));

    // Replace all, then undo as one step.
    command(&mut h, Command::Replace);
    h.get_by_label("Vervangtekst").click();
    h.run();
    text(&mut h, "Peer");
    h.get_by_label("Vervang alles").click();
    h.run();
    assert_eq!(shown(&h, "A1"), "Peer");
    assert_eq!(shown(&h, "A3"), "Peertaart");
    assert_eq!(h.state().view.find.as_ref().unwrap().info, "2 vervangen");
    command(&mut h, Command::Undo);
    assert_eq!(shown(&h, "A3"), "appeltaart");

    // Esc in the field closes the bar.
    h.get_by_label("Zoekterm").click();
    h.run();
    key(&mut h, Key::Escape);
    assert!(h.state().view.find.is_none());
}

#[test]
fn italic_and_colours_from_the_toolbar() {
    let mut h = harness();
    text(&mut h, "kleur");
    key(&mut h, Key::Enter);
    key(&mut h, Key::ArrowUp);
    command(&mut h, Command::Italic);
    assert!(h.state().view.sheet().format(c("A1")).italic);

    h.get_by_label("Tekstkleur").click();
    h.run();
    h.get_by_label("Rood").click();
    h.run();
    h.get_by_label("Opvulkleur").click();
    h.run();
    h.get_by_label("Geel").click();
    h.run();
    let f = h.state().view.sheet().format(c("A1"));
    assert_eq!(f.color, Some(rustsheet_core::format::Rgb(0xFF, 0, 0)));
    assert_eq!(f.fill, Some(rustsheet_core::format::Rgb(0xFF, 0xFF, 0)));

    h.get_by_label("Opvulkleur").click();
    h.run();
    h.get_by_label("Geen opvulling").click();
    h.run();
    assert_eq!(h.state().view.sheet().format(c("A1")).fill, None);
}

#[test]
fn sorting_the_block_around_the_active_cell() {
    let mut h = harness();
    for row in [["Naam", "Score"], ["Piet", "7"], ["Anna", "9"], ["Kees", "8"]] {
        text(&mut h, row[0]);
        key(&mut h, Key::Tab);
        text(&mut h, row[1]);
        key(&mut h, Key::Enter);
    }
    click(&mut h, "B3", Modifiers::NONE);
    command(&mut h, Command::SortDesc);
    let names: Vec<String> = ["A1", "A2", "A3", "A4"].iter().map(|a| shown(&h, a)).collect();
    assert_eq!(names, ["Naam", "Anna", "Kees", "Piet"]);
    assert_eq!(h.state().view.sel.range().to_string(), "A1:B4");

    click(&mut h, "A2", Modifiers::NONE);
    command(&mut h, Command::SortAsc);
    assert_eq!(shown(&h, "A2"), "Anna");
    assert_eq!(shown(&h, "A4"), "Piet");
    command(&mut h, Command::Undo);
    command(&mut h, Command::Undo);
    assert_eq!(shown(&h, "A2"), "Piet");
}

#[test]
fn enter_after_tabs_returns_to_the_starting_column() {
    let mut h = harness();
    click(&mut h, "B2", Modifiers::NONE);
    for v in ["a", "b"] {
        text(&mut h, v);
        key(&mut h, Key::Tab);
    }
    text(&mut h, "c");
    key(&mut h, Key::Enter);
    assert_eq!(h.state().view.sel.active, c("B3"));
    assert_eq!(shown(&h, "D2"), "c");
}
