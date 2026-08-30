//! The glass serving loop, drilled end to end on a TestBackend.
//!
//! The painter's own battery lives with the painter; this drill
//! proves the loop around it -- a real corpus story served whole
//! through the suspend-and-deliver arrangement, every keystroke
//! travelling the editor, the model, and the machine, with the
//! final grid read back from the glass.

use std::cell::RefCell;
use std::rc::Rc;

use voxam_core::zmachine::machine::{Identity, Machine};
use voxam_core::zmachine::story::Story;
use voxam_glass::keys::ScriptedKeys;
use voxam_glass::painter::{Glass, PaintedHalf, ScreenFrontend};
use voxam_glass::ratatui::Terminal;
use voxam_glass::ratatui::backend::TestBackend;
use voxam_glass::ratatui::layout::Position;

use super::*;

/// One painted row of the face's glass, trailing blanks trimmed.
fn row_at(face: &Face<TestBackend>, y: u16) -> String {
    let face = face.borrow();
    let glass = face.glass.borrow();
    let buffer = glass.terminal().backend().buffer();
    let mut row = String::new();

    for x in 0..buffer.area.width {
        row.push_str(
            buffer
                .cell(Position::new(x, y))
                .expect("a cell within the glass")
                .symbol(),
        );
    }

    row.trim_end().to_string()
}

// Zork I plays on the test glass from boot to quit: the banner
// scrolls the lower window, the status line paints the top row,
// the typed commands echo through the model, and the machine
// halts on its own answer.
#[test]
fn zork_plays_on_the_glass_to_its_end() {
    let bytes = std::fs::read("../../entharion/zcode-infocom/zork1-r88-s840726.z3")
        .expect("the corpus story beside the workspace");
    let story = Story::new(bytes).expect("a loadable story");
    let terminal = Terminal::new(TestBackend::new(80, 24)).expect("a test terminal");
    let glass = Rc::new(RefCell::new(Glass::new(
        terminal,
        Box::new(ScriptedKeys::typed("look\nquit\ny\n")),
        Box::new(|| ()),
    )));
    let face: Face<TestBackend> = Rc::new(RefCell::new(ScreenFrontend::new(3, glass)));
    let mut machine = Machine::new(
        story,
        Box::new(PaintedHalf(Rc::clone(&face))),
        Some(1),
        Identity::default(),
        None,
    )
    .expect("a bootable machine");

    served(&mut machine, &face).expect("a served session");

    // The Version 3 status line names the player's whereabouts on
    // the top row (§8.2).
    assert!(row_at(&face, 0).contains("West of House"));

    // The typed commands echoed through the model onto the glass.
    let rows: Vec<String> = (0..24).map(|y| row_at(&face, y)).collect();
    let whole = rows.join("\n");

    assert!(whole.contains(">look"));
    assert!(whole.contains(">quit"));
}
