//! The painted frontend, held to golden TestBackend grids.
//!
//! The reference battery drives a stub terminal and reads its
//! escape stream; the rewrite in kind drives ratatui's TestBackend
//! and reads the painted grid itself -- the same scenarios,
//! asserted on what a player would actually see.

use std::cell::Cell as SharedCell;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use ratatui::backend::TestBackend;
use ratatui::buffer::Cell as Painted;

use voxam_core::screen::UPPER;

use super::*;
use crate::keys::ScriptedKeys;

/// The stub glass: a 30 by 8 TestBackend, a scripted key intake
/// the battery keeps a handle on, and a counted bell.
#[expect(clippy::type_complexity, reason = "a test fixture's bundle")]
fn glassed(
    keys: ScriptedKeys,
) -> (
    Rc<RefCell<Glass<TestBackend>>>,
    Rc<RefCell<ScriptedKeys>>,
    Rc<SharedCell<usize>>,
) {
    let script = Rc::new(RefCell::new(keys));
    let bells = Rc::new(SharedCell::new(0));
    let terminal = Terminal::new(TestBackend::new(30, 8)).expect("a test terminal");
    let counter = Rc::clone(&bells);
    let glass = Glass::new(
        terminal,
        Box::new(Rc::clone(&script)),
        Box::new(move || counter.set(counter.get() + 1)),
    );

    (Rc::new(RefCell::new(glass)), script, bells)
}

/// A painted frontend over the stub glass, with the machine's
/// handle beside it.
#[expect(clippy::type_complexity, reason = "a test fixture's bundle")]
fn painted(
    version: u8,
    keys: ScriptedKeys,
) -> (
    Rc<RefCell<ScreenFrontend<TestBackend>>>,
    PaintedHalf<TestBackend>,
    Rc<RefCell<ScriptedKeys>>,
    Rc<SharedCell<usize>>,
) {
    let (glass, script, bells) = glassed(keys);
    let face = Rc::new(RefCell::new(ScreenFrontend::new(version, glass)));
    let half = PaintedHalf(Rc::clone(&face));

    (face, half, script, bells)
}

/// The keystrokes of a typed line, enter included.
fn typing(text: &str) -> ScriptedKeys {
    ScriptedKeys::typed(&format!("{text}\n"))
}

/// One painted cell of the face's glass.
fn cell_at(face: &Rc<RefCell<ScreenFrontend<TestBackend>>>, x: u16, y: u16) -> Painted {
    let face = face.borrow();
    let glass = face.glass.borrow();

    glass
        .terminal()
        .backend()
        .buffer()
        .cell(Position::new(x, y))
        .expect("a cell within the glass")
        .clone()
}

/// One painted row of the face's glass, trailing blanks trimmed.
fn row_at(face: &Rc<RefCell<ScreenFrontend<TestBackend>>>, y: u16) -> String {
    let mut row = String::new();

    for x in 0..30 {
        row.push_str(cell_at(face, x, y).symbol());
    }

    row.trim_end().to_string()
}

/// Where the glass parked its cursor.
fn parked(face: &Rc<RefCell<ScreenFrontend<TestBackend>>>) -> (u16, u16) {
    let face = face.borrow();
    let mut glass = face.glass.borrow_mut();
    let position = glass
        .terminal_mut()
        .get_cursor_position()
        .expect("a parked cursor");

    (position.x, position.y)
}

// The picture seam is inert at the terminal: no sizes, a census of
// zero, and draws that paint nothing -- the half-block cover is a
// doorway courtesy, and the header's cleared pictures bit already
// said so (§11.1.4, §15 picture_data).
#[test]
fn the_picture_seam_is_inert() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.draw_picture(1, 1, 1);
    half.erase_picture(1, 1, 1);
    half.place_window(1, 1, 1, 1, 1);

    assert!(!half.has_stage());
    assert!(!half.has_pictures());
    assert!(half.picture_data(1).is_none());
    assert_eq!(half.picture_census(), (0, 0));
    assert_eq!(row_at(&face, 0), "");
}

// A write lands in the model and the glass repaints it in place.
#[test]
fn writes_repaint_the_damaged_row() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.write("hello");

    assert_eq!(row_at(&face, 0), "hello");
    assert_eq!(face.borrow_mut().model.row_text(1), "hello");
}

// The Version 3 status line paints the top row in reverse video
// (§8.2).
#[test]
fn the_status_line_paints_in_reverse() {
    let (face, mut half, _script, _bells) = painted(3, ScriptedKeys::default());

    half.show_status(&Status {
        location: "Kitchen".to_string(),
        score: 10,
        turns: 2,
        time_game: false,
    });

    assert!(row_at(&face, 0).contains("Kitchen"));
    assert!(
        cell_at(&face, 0, 0)
            .style()
            .add_modifier
            .contains(Modifier::REVERSED)
    );
}

// Styles and colours dress the painted cells (§8.7.1, §8.3.1) --
// the run-length escape economies are ratatui's own business now.
#[test]
fn styles_and_colours_reach_the_glass() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.set_style(BOLD);
    half.set_style(ITALIC);
    half.set_colour(3, 4);
    half.write("dressed");

    let style = cell_at(&face, 0, 0).style();

    assert!(style.add_modifier.contains(Modifier::BOLD));
    assert!(style.add_modifier.contains(Modifier::ITALIC));
    assert_eq!(style.fg, Some(Color::Red));
    assert_eq!(style.bg, Some(Color::Green));
}

// Reverse video passes through as its own modifier (§8.7.1).
#[test]
fn reverse_video_reaches_the_glass() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.set_style(REVERSE);
    half.write("dark");

    assert!(
        cell_at(&face, 0, 0)
            .style()
            .add_modifier
            .contains(Modifier::REVERSED)
    );
}

// A §15 rectangle flows through the model and repaints, each row
// returning to the column where the rectangle began.
#[test]
fn rectangles_paint_right_and_down() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.split_window(3);
    half.set_window(UPPER);
    half.set_cursor(1, 4);
    half.write_rectangle(&["ab".to_string(), "cd".to_string()]);

    assert_eq!(face.borrow_mut().model.row_text(1), "   ab");
    assert_eq!(face.borrow_mut().model.row_text(2), "   cd");
    assert_eq!(row_at(&face, 0), "   ab");
    assert_eq!(row_at(&face, 1), "   cd");
}

// Cells in the character graphics font paint as their §16 Unicode
// stand-ins: box-drawing for the map lines, runes for the letters.
#[test]
fn font_3_paints_its_unicode_stand_ins() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.set_font(GRAPHICS_FONT);
    half.write("(f");

    assert_eq!(cell_at(&face, 0, 0).symbol(), "│");
    assert_eq!(cell_at(&face, 1, 0).symbol(), "ᚠ");
}

// Codes 123 to 126 are the reverse-video twins of the arrows and
// the drawn question mark -- the §16 bitmaps invert them pixel for
// pixel -- so the painter draws the same shape and flips reverse
// video instead of carrying it in the glyph.
#[test]
fn font_3_reversed_shapes_flip_reverse_video() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.set_font(GRAPHICS_FONT);
    half.write("{");

    let cell = cell_at(&face, 0, 0);

    assert_eq!(cell.symbol(), "↑");
    assert!(cell.style().add_modifier.contains(Modifier::REVERSED));
}

// The map-connectivity calls the reference's eyeball tests
// settled: a solid mass meeting a diagonal road stays a quadrant
// block, so room corners keep their shape, and the single-pixel
// road tips continue their diagonal, so a road reaches its room
// without a gap (§16).
#[test]
fn font_3_keeps_the_map_connected() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.set_font(GRAPHICS_FONT);
    half.write("CG");

    assert_eq!(cell_at(&face, 0, 0).symbol(), "▝");
    assert_eq!(cell_at(&face, 1, 0).symbol(), "╱");
}

// A font 3 character beyond the §16 table -- an accented letter,
// say -- passes through as itself rather than vanishing.
#[test]
fn font_3_passes_unknown_characters_through() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.set_font(GRAPHICS_FONT);
    half.write("é");

    assert_eq!(cell_at(&face, 0, 0).symbol(), "é");
}

// Window operations flow through the model and park the terminal
// cursor where the model's cursor stands.
#[test]
fn window_operations_park_the_cursor() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.split_window(2);
    half.set_window(UPPER);
    half.set_cursor(2, 5);
    half.write("X");

    assert_eq!(parked(&face), (5, 1));
}

// Erasure repaints the blanked rows.
#[test]
fn erasure_repaints() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.write("about to go");
    half.erase_window(-1);

    assert_eq!(row_at(&face, 0), "");
    assert_eq!(face.borrow_mut().model.row_text(1), "");
}

// A printing interrupt strands the prompt above its output; the
// §15 remark asks the interpreter to redisplay the input line, and
// the painter rewrites the remembered prompt at the new cursor --
// Jigsaw's chapter epigraphs are the earner.
#[test]
fn the_prompt_returns_after_an_interrupts_output() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.write("\n>");
    face.borrow_mut().begin_input();
    half.write("\n\n   All the generals were on holiday.\n\n");
    face.borrow_mut().resume_input();

    let mut face = face.borrow_mut();
    let (row, _column) = face.model.cursor();

    assert_eq!(face.model.row_text(row), ">");
}

// erase_line clears from the cursor onward and repaints the row
// (§8.7.3.4).
#[test]
fn erase_line_repaints_the_row() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.write("wiped nearly");
    half.erase_line(None);

    assert_eq!(face.borrow_mut().model.row_text(1), "wiped nearly");
}

// Buffering flows through to the model without painting anything.
#[test]
fn buffering_paints_nothing() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.set_buffering(false);

    assert_eq!(row_at(&face, 0), "");
}

// Both bleeps ring the terminal's one bell (§9).
#[test]
fn bleeps_ring_the_bell() {
    let (_face, mut half, _script, bells) = painted(5, ScriptedKeys::default());

    half.bleep(false);
    half.bleep(true);

    assert_eq!(bells.get(), 2);
}

// The sound seams stay inert until a speaker exists to make the
// claim true (§9.1.2).
#[test]
fn the_sound_seam_is_inert_without_a_speaker() {
    let (_face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    assert!(!half.has_sounds());
    assert!(!half.play_sound(3, 8, Some(1)));

    half.stop_sound(None);
    half.wait_for_sound();

    assert!(!half.sound_playing());
    assert!(!half.sound_finished());
}

// read_line reads raw keystrokes and echoes them through the model
// itself -- the terminal's own echo is never invited, so nothing
// but the painter ever writes to the glass.
#[test]
fn read_line_echoes_through_the_model() {
    let (face, _half, _script, _bells) = painted(5, typing("open mailbox"));

    let line = face.borrow_mut().read_line();

    assert_eq!(line, "open mailbox");
    assert_eq!(face.borrow_mut().model.row_text(1), "open mailbox");
    assert_eq!(row_at(&face, 0), "open mailbox");
}

// Backspace rubs out the last typed character, on the glass and in
// the returned line alike (§15 read's line editor).
#[test]
fn read_line_backspace_rubs_out() {
    let (face, _half, _script, _bells) = painted(5, ScriptedKeys::typed("loox\u{7f}k\n"));

    let line = face.borrow_mut().read_line();

    assert_eq!(line, "look");
    assert_eq!(face.borrow_mut().model.row_text(1), "look");
    assert_eq!(row_at(&face, 0), "look");
}

// With nothing typed there is nothing to rub: backspace at the
// start of a line is quietly nothing.
#[test]
fn read_line_backspace_stops_at_the_start() {
    let (face, _half, _script, _bells) = painted(5, ScriptedKeys::typed("\u{7f}n\n"));

    assert_eq!(face.borrow_mut().read_line(), "n");
}

// Escape, the §3.8.4 codes beyond the cursor keys, and unmapped
// keys mean nothing to a line: read_line waits them out -- and
// cursor-up with no history yet is just as quiet.
#[test]
fn read_line_waits_out_keys_a_line_cannot_use() {
    let keys = ScriptedKeys::new(vec![
        Some('\u{1b}'),
        Some('\u{81}'),
        None,
        Some('y'),
        Some('\n'),
    ]);
    let (face, _half, _script, _bells) = painted(5, keys);

    let line = face.borrow_mut().read_line();

    assert_eq!(line, "y");
    assert_eq!(face.borrow_mut().model.row_text(1), "y");
}

// The cursor keys edit within the line: left walks back, an
// insertion lands at the cursor, and the model repaints the whole
// line -- glass and returned text agreeing (§15 read).
#[test]
fn read_line_edits_mid_line() {
    let (face, _half, _script, _bells) = painted(5, ScriptedKeys::typed("gt\u{83}e\u{84}\n"));

    let line = face.borrow_mut().read_line();

    assert_eq!(line, "get");
    assert_eq!(face.borrow_mut().model.row_text(1), "get");
    assert_eq!(row_at(&face, 0), "get");
}

// Cursor-up recalls the previous command from the session's
// history, painted onto the glass like typing; the recalled line
// replaces a longer draft cleanly.
#[test]
fn read_line_recalls_history() {
    let (face, _half, _script, _bells) = painted(5, ScriptedKeys::typed("inventory\nlo\u{81}\n"));

    let first = face.borrow_mut().read_line();
    let second = face.borrow_mut().read_line();

    assert_eq!(first, "inventory");
    assert_eq!(second, "inventory");
    assert_eq!(face.borrow_mut().model.row_text(2), "inventory");
    assert_eq!(row_at(&face, 1), "inventory");
}

// A timed read answers None on the deadline with the half-typed
// line still composed on the glass; the next call resumes it to
// completion, and abandoning erases it -- §15's live line read,
// the seam Border Zone's clock ticks through. The scripted intake
// answers the typed keys instantly and expiries after, so the
// tiny real deadline passes deterministically.
#[test]
fn timed_reads_pause_resume_and_abandon() {
    let (face, _half, script, _bells) = painted(5, ScriptedKeys::typed("go"));
    let brief = Duration::from_millis(5);

    let line = face.borrow_mut().read_line_until(brief);

    assert!(line.is_none());
    assert_eq!(face.borrow_mut().model.row_text(1), "go");

    script.borrow_mut().keys = vec![Some('\n')];

    assert_eq!(
        face.borrow_mut().read_line_until(brief).as_deref(),
        Some("go")
    );

    // Nothing composed: abandoning is quietly nothing.
    face.borrow_mut().abandon_input();

    script.borrow_mut().keys = vec![Some('n'), Some('o')];

    assert!(face.borrow_mut().read_line_until(brief).is_none());

    face.borrow_mut().abandon_input();

    assert_eq!(face.borrow_mut().model.row_text(2), "");
    assert_eq!(row_at(&face, 1), "");

    script.borrow_mut().keys = typing("go").keys;

    assert_eq!(face.borrow_mut().read_line(), "go");

    // With the idle heartbeat armed, an empty read lets background
    // work run and the wait chunks at the heartbeat.
    let beats = Rc::new(SharedCell::new(0));
    let counter = Rc::clone(&beats);

    face.borrow().glass.borrow_mut().idle = Some(Box::new(move || counter.set(counter.get() + 1)));
    script.borrow_mut().keys = vec![None, Some('h'), Some('i'), Some('\n')];

    assert_eq!(
        face.borrow_mut()
            .read_line_until(Duration::from_secs(9))
            .as_deref(),
        Some("hi")
    );
    assert_eq!(beats.get(), 1);
}

// A bold space paints without its bold: there is no glyph to
// embolden, and a terminal would brighten the blank's reverse
// background into a patchwork -- Border Zone pads its status bar
// with exactly such spaces. Bold text keeps its dress.
#[test]
fn bold_spaces_shed_their_bold() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.set_style(BOLD);
    half.write("a b");

    assert!(
        cell_at(&face, 0, 0)
            .style()
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert!(
        !cell_at(&face, 1, 0)
            .style()
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert!(
        cell_at(&face, 2, 0)
            .style()
            .add_modifier
            .contains(Modifier::BOLD)
    );
}

// A screenful of prints pauses behind [MORE] at the cursor,
// spends one key on the pause, and repaints the grid clean -- the
// top of Bureaucracy's post-form text wall survives. The idle
// heartbeat is armed and expires once before the real key: a
// heartbeat must never answer the pause, or [MORE] clicks itself
// after a fifth of a second.
#[test]
fn a_screenful_pauses_behind_more() {
    let (face, mut half, script, _bells) = painted(5, ScriptedKeys::new(vec![None, Some('x')]));
    let beats = Rc::new(SharedCell::new(0));
    let counter = Rc::clone(&beats);

    face.borrow().glass.borrow_mut().idle = Some(Box::new(move || counter.set(counter.get() + 1)));

    half.write(&"line\n".repeat(8));

    assert_eq!(beats.get(), 1);
    assert!(script.borrow().keys.is_empty());
    assert!(!face.borrow_mut().model.rendered().contains(MORE_PROMPT));

    for y in 0..8 {
        assert!(!row_at(&face, y).contains(MORE_PROMPT));
    }
}

// The pause overlay itself: reverse-video [MORE] at the cursor,
// clamped to fit the row, painted over the grid without entering
// the model.
#[test]
fn the_more_overlay_paints_reversed_at_the_cursor() {
    let (glass, _script, _bells) = glassed(ScriptedKeys::default());
    let mut model = ScreenModel::new(30, 8, 5);

    model.write("read me\n");

    let drawn = glass.borrow_mut().drawn(&mut model, Some(MORE_PROMPT));

    assert!(drawn.is_ok());

    let held = glass.borrow();
    let buffer = held.terminal().backend().buffer();
    let overlay: String = (0..6)
        .map(|x| {
            buffer
                .cell(Position::new(x, 1))
                .expect("an overlay cell")
                .symbol()
                .to_string()
        })
        .collect();

    assert_eq!(overlay, MORE_PROMPT);
    assert!(
        buffer
            .cell(Position::new(0, 1))
            .expect("an overlay cell")
            .style()
            .add_modifier
            .contains(Modifier::REVERSED)
    );
}

// A plain keystroke passes through read_key as itself, unechoed --
// §15 read_char leaves echoing to the game. The terminal has no
// mouse reporting yet, so clicks never arrive.
#[test]
fn read_key_passes_plain_keys_through() {
    let (face, half, _script, _bells) = painted(5, ScriptedKeys::typed("n"));

    assert!(!half.has_mouse());
    assert!(face.borrow_mut().click_position().is_none());

    let key = face.borrow_mut().read_key(None);

    assert_eq!(key, Some('n'));
    assert_eq!(row_at(&face, 0), "");
}

// An empty heartbeat is not a keystroke: read_key waits for one
// it can use.
#[test]
fn read_key_waits_out_empty_reads() {
    let (face, _half, _script, _bells) = painted(5, ScriptedKeys::new(vec![None, Some('q')]));

    assert_eq!(face.borrow_mut().read_key(None), Some('q'));
}

// With a timeout, an expired wait answers None -- the machine's
// cue to fire a wall-clock interrupt -- and the timeout is handed
// through to the intake.
#[test]
fn read_key_reports_expired_timeouts() {
    let (face, _half, script, _bells) = painted(5, ScriptedKeys::default());
    let patience = Duration::from_millis(500);

    assert!(face.borrow_mut().read_key(Some(patience)).is_none());
    assert_eq!(script.borrow().timeouts, vec![Some(patience)]);
}

// An unusable key inside a timed wait is no keystroke either: the
// wait reports as expired rather than pretending.
#[test]
fn read_key_timeout_swallows_unusable_keys() {
    let (face, _half, _script, _bells) = painted(5, ScriptedKeys::new(vec![None]));

    assert!(
        face.borrow_mut()
            .read_key(Some(Duration::from_millis(500)))
            .is_none()
    );
}

// A key that beats the clock comes back as itself.
#[test]
fn read_key_returns_keys_that_beat_the_clock() {
    let (face, _half, _script, _bells) = painted(5, ScriptedKeys::typed("z"));

    assert_eq!(
        face.borrow_mut().read_key(Some(Duration::from_millis(500))),
        Some('z')
    );
}

// With an idle callback wired, an infinite wait is chopped into
// heartbeats: each expiry lets the machine attend to background
// work, and the typed line is unaffected.
#[test]
fn read_line_heartbeats_through_its_idle_callback() {
    let (face, _half, script, _bells) = painted(
        5,
        ScriptedKeys::new(vec![None, Some('g'), Some('o'), Some('\n')]),
    );
    let beats = Rc::new(SharedCell::new(0));
    let counter = Rc::clone(&beats);

    face.borrow().glass.borrow_mut().idle = Some(Box::new(move || counter.set(counter.get() + 1)));

    assert_eq!(face.borrow_mut().read_line(), "go");
    assert_eq!(beats.get(), 1);

    let timeouts = script.borrow().timeouts.clone();

    assert!(timeouts.iter().all(|ask| *ask == Some(IDLE_HEARTBEAT)));
}

// An infinite single-key wait heartbeats the same way.
#[test]
fn read_key_heartbeats_while_waiting_forever() {
    let (face, _half, script, _bells) = painted(5, ScriptedKeys::new(vec![None, Some('n')]));
    let beats = Rc::new(SharedCell::new(0));
    let counter = Rc::clone(&beats);

    face.borrow().glass.borrow_mut().idle = Some(Box::new(move || counter.set(counter.get() + 1)));

    assert_eq!(face.borrow_mut().read_key(None), Some('n'));
    assert_eq!(beats.get(), 1);
    assert_eq!(
        script.borrow().timeouts,
        vec![Some(IDLE_HEARTBEAT), Some(IDLE_HEARTBEAT)]
    );
}

// A timed read keeps its own clock: the game's timeout passes
// through untouched and the idle callback never fires there.
#[test]
fn timed_read_keys_keep_their_own_clock() {
    let (face, _half, script, _bells) = painted(5, ScriptedKeys::typed("y"));

    face.borrow().glass.borrow_mut().idle = Some(Box::new(|| panic!("a timed read must not idle")));

    let patience = Duration::from_millis(500);

    assert_eq!(face.borrow_mut().read_key(Some(patience)), Some('y'));
    assert_eq!(script.borrow().timeouts, vec![Some(patience)]);
}

// clear() paints the blank model over the whole glass, so a story
// starts on a clean screen with no shell output showing through
// the rows it has not yet painted.
#[test]
fn clear_wipes_every_row() {
    let (face, _half, _script, _bells) = painted(5, ScriptedKeys::default());

    face.borrow_mut().clear();

    for y in 0..8 {
        assert_eq!(row_at(&face, y), "");
    }
}

// A cover paints centred in half-block cells -- each ▀ carries two
// pixels, the upper as ink and the lower as ground, an odd bottom
// row grounding on black.
#[test]
fn the_cover_paints_in_half_blocks() {
    let (glass, _script, _bells) = glassed(ScriptedKeys::default());
    let picture = Picture {
        width: 2,
        height: 3,
        rows: vec![
            vec![(255, 0, 0), (0, 255, 0)],
            vec![(0, 0, 255), (255, 255, 255)],
            vec![(10, 20, 30), (40, 50, 60)],
        ],
        clear: None,
        alpha: None,
    };
    let pixels = fitted(&picture, 30, 16);
    let left = (30 - pixels[0].len()) / 2;
    let top = (8 - pixels.len().div_ceil(2)) / 2;

    assert_eq!((left, top), (14, 3));

    glass.borrow_mut().cover(&pixels, left, top);

    let held = glass.borrow();
    let buffer = held.terminal().backend().buffer();
    let first = buffer.cell(Position::new(14, 3)).expect("a cover cell");
    let odd = buffer.cell(Position::new(14, 4)).expect("a cover cell");

    assert_eq!(first.symbol(), "▀");
    assert_eq!(first.style().fg, Some(Color::Rgb(255, 0, 0)));
    assert_eq!(first.style().bg, Some(Color::Rgb(0, 0, 255)));
    assert_eq!(odd.style().fg, Some(Color::Rgb(10, 20, 30)));
    assert_eq!(odd.style().bg, Some(Color::Rgb(0, 0, 0)));
}

// The frontispiece shows the cover until a key is pressed, then
// leaves the game a clean screen no splash pixel survives on.
#[test]
fn the_frontispiece_spends_a_key_and_clears() {
    let (face, _half, script, _bells) = painted(5, ScriptedKeys::typed("x"));
    let picture = Picture {
        width: 2,
        height: 2,
        rows: vec![vec![(255, 0, 0); 2]; 2],
        clear: None,
        alpha: None,
    };

    face.borrow_mut().show_frontispiece(&picture);

    assert!(script.borrow().keys.is_empty());

    for y in 0..8 {
        assert_eq!(row_at(&face, y), "");
    }
}

// A cover larger than the glass shrinks to fit, keeping its shape;
// the box average of a uniform picture is itself.
#[test]
fn large_covers_shrink_to_the_glass() {
    let picture = Picture {
        width: 60,
        height: 32,
        rows: vec![vec![(100, 150, 200); 60]; 32],
        clear: None,
        alpha: None,
    };
    let pixels = fitted(&picture, 30, 16);

    assert_eq!(pixels.len(), 16);
    assert_eq!(pixels[0].len(), 30);
    assert!(
        pixels
            .iter()
            .all(|row| row.iter().all(|pixel| *pixel == (100, 150, 200)))
    );
}

// The fallback dimensions cover a terminal that reports no size
// (§8.4).
#[test]
fn a_sizeless_terminal_falls_back() {
    let terminal = Terminal::new(TestBackend::new(0, 0)).expect("a test terminal");
    let glass = Glass::new(terminal, Box::new(ScriptedKeys::default()), Box::new(|| ()));

    assert_eq!(glass.columns, FALLBACK_COLUMNS);
    assert_eq!(glass.lines, FALLBACK_LINES);
}

// Only story-window prints count toward the §15 redisplay:
// Border Zone's clock tick repaints the status window every
// interval, and redisplaying the untouched prompt for those
// would grow a picket fence of > characters.
#[test]
fn only_story_prints_count_toward_redisplay() {
    let (face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.write(">");

    assert_eq!(face.borrow().prints, 1);

    half.split_window(1);
    half.set_window(UPPER);
    half.set_cursor(1, 1);
    half.write("17:26");

    assert_eq!(face.borrow().prints, 1);

    half.set_window(0);
    half.write("The train lurches.");

    assert_eq!(face.borrow().prints, 2);
}

// The painted answer for get_cursor is the model's own
// (§8.7.2.3.2).
#[test]
fn cursor_position_reads_the_model() {
    let (_face, mut half, _script, _bells) = painted(5, ScriptedKeys::default());

    half.split_window(3);
    half.set_window(UPPER);
    half.set_cursor(2, 5);

    assert_eq!(half.cursor_position(), (2, 5));
}
