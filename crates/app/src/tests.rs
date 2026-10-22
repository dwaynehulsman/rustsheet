//! Interaction tests: real key and pointer events through the whole view.

use egui::{Event, Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use rustsheet_core::{CellRef, Document, Sheet, Value};

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
        view: SheetView::new(Document::new(Sheet::new("Blad1")), None, 1),
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
    h.state().view.doc.sheet.display(c(at))
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
    assert_eq!(view.doc.sheet.display(c("A1")), "");
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
    assert_eq!(h.state().view.doc.sheet.state(c("B3")).content.source(), "=A3*10");

    // Cut keeps references and empties the source.
    click(&mut h, "B1", Modifiers::NONE);
    command(&mut h, Command::Cut);
    click(&mut h, "D5", Modifiers::NONE);
    command(&mut h, Command::Paste);
    assert_eq!(h.state().view.doc.sheet.state(c("D5")).content.source(), "=A1*10");
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
    assert_eq!(h.state().view.doc.sheet.value(c("C3")), &Value::Number(2.5));
    assert_eq!(h.state().view.sel.range().to_string(), "B2:C3");
}

#[test]
fn shift_click_extends_selection_and_formats_apply() {
    let mut h = harness();
    click(&mut h, "B2", Modifiers::NONE);
    click(&mut h, "C4", Modifiers::SHIFT);
    assert_eq!(h.state().view.sel.range().to_string(), "B2:C4");
    command(&mut h, Command::Bold);
    assert!(h.state().view.doc.sheet.format(c("C3")).bold);
    command(&mut h, Command::NumFmt(rustsheet_core::NumFmt::Percent));
    assert_eq!(h.state().view.doc.sheet.format(c("B4")).num, rustsheet_core::NumFmt::Percent);
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
