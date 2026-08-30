//! The painted Glk display, held to golden TestBackend grids.
//!
//! The reference battery drives a stub blessed terminal and reads
//! its escape stream; the rewrite in kind drives ratatui's
//! TestBackend and reads the painted grid itself. The sound and
//! recording scenarios wait with their deferred seams.

use std::cell::Cell as SharedCell;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};

use voxam_core::glulx::glk::frontend::{Asked, Frontend};
use voxam_core::glulx::glk::objects::{
    BufferData, GridData, LineRequest, PairData, Window, WindowKind, WindowMap, event_type,
    file_mode, key_code, rearrange, style, window_method,
};

use super::*;

/// The stub glass: a 30 by 8 TestBackend over a scripted intake
/// the battery keeps a handle on.
fn glassed(codes: ScriptedCodes) -> (GlkGlass<TestBackend>, Rc<RefCell<ScriptedCodes>>) {
    let script = Rc::new(RefCell::new(codes));
    let terminal = Terminal::new(TestBackend::new(30, 8)).expect("a test terminal");

    (
        GlkGlass::new(terminal, Box::new(Rc::clone(&script))),
        script,
    )
}

/// A ticking intake: the shared clock moves one second per read,
/// the reference's TickingTerminal.
struct TickingCodes {
    codes: Vec<Option<u32>>,
    clock: Rc<SharedCell<f64>>,
}

impl CodeSource for TickingCodes {
    fn code(&mut self, _timeout: Option<Duration>) -> Fetch {
        self.clock.set(self.clock.get() + 1.0);

        if self.codes.is_empty() {
            return Fetch::Ended;
        }

        match self.codes.remove(0) {
            Some(code) => Fetch::Key(code),
            None => Fetch::Nothing,
        }
    }
}

/// A glass on a hand-ticked clock.
fn ticking(codes: Vec<Option<u32>>) -> (GlkGlass<TestBackend>, Rc<SharedCell<f64>>) {
    let clock = Rc::new(SharedCell::new(0.0));
    let terminal = Terminal::new(TestBackend::new(30, 8)).expect("a test terminal");
    let handle = Rc::clone(&clock);
    let glass = GlkGlass::new(
        terminal,
        Box::new(TickingCodes {
            codes,
            clock: Rc::clone(&clock),
        }),
    )
    .clocked(Box::new(move || handle.get()));

    (glass, clock)
}

fn typing(text: &str) -> ScriptedCodes {
    ScriptedCodes::typed(text)
}

/// One window standing alone in a map, at id 1.
fn windowed(kind: WindowKind, bbox: (i64, i64, i64, i64)) -> WindowMap {
    let mut map = WindowMap::new();

    map.insert(1, Window::new(kind, 0));
    rearrange(&mut map, 1, bbox);

    map
}

fn buffer() -> WindowKind {
    WindowKind::Buffer(BufferData::default())
}

fn grid() -> WindowKind {
    WindowKind::Grid(GridData::default())
}

/// Print into a window's stream, in a style.
fn saying(map: &mut WindowMap, id: u32, text: &str, number: u32) {
    let window = map.get_mut(&id).expect("a window to speak into");

    window.style = number;

    for character in text.chars() {
        window.put_char(u32::from(character), 0);
    }
}

/// Write into a grid at a cursor position.
fn writing(map: &mut WindowMap, id: u32, x: i64, y: i64, text: &str) {
    let window = map.get_mut(&id).expect("a grid to write into");

    window.move_cursor(x, y);

    for character in text.chars() {
        window.put_char(u32::from(character), 0);
    }
}

/// One painted row of the glass, trailing blanks trimmed.
fn row_at(glass: &GlkGlass<TestBackend>, y: u16) -> String {
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

/// One painted cell's modifiers.
fn dress_at(glass: &GlkGlass<TestBackend>, x: u16, y: u16) -> Modifier {
    glass
        .terminal()
        .backend()
        .buffer()
        .cell(Position::new(x, y))
        .expect("a cell within the glass")
        .style()
        .add_modifier
}

/// Where the glass parked its cursor.
fn parked(glass: &mut GlkGlass<TestBackend>) -> (u16, u16) {
    let position = glass
        .terminal_mut()
        .get_cursor_position()
        .expect("a parked cursor");

    (position.x, position.y)
}

// The size is the terminal's own measure, the classic 80x24 when
// the terminal cannot say, or whatever the caller chose.
#[test]
fn the_size_is_the_terminals_measure() {
    let (glass, _script) = glassed(ScriptedCodes::default());

    assert_eq!(glass.size(), (30, 8));

    let sizeless = GlkGlass::new(
        Terminal::new(TestBackend::new(0, 0)).expect("a test terminal"),
        Box::new(ScriptedCodes::default()),
    );

    assert_eq!(sizeless.size(), (80, 24));

    let chosen = GlkGlass::new(
        Terminal::new(TestBackend::new(30, 8)).expect("a test terminal"),
        Box::new(ScriptedCodes::default()),
    )
    .sized((10, 5));

    assert_eq!(chosen.size(), (10, 5));
}

// Clearing paints every row blank and parks the cursor out of the
// way: the story begins on a clean screen, whatever the shell
// left behind.
#[test]
fn clearing_wipes_the_glass() {
    let (mut glass, _script) = glassed(ScriptedCodes::default());

    glass.clear();

    for y in 0..8 {
        assert_eq!(row_at(&glass, y), "");
    }

    assert_eq!(parked(&mut glass), (0, 7));
}

// With no window tree there is nothing to paint, and nothing is.
#[test]
fn no_tree_paints_nothing() {
    let (mut glass, _script) = glassed(ScriptedCodes::default());
    let mut map = WindowMap::new();

    glass.flush(&mut map, None);

    assert_eq!(row_at(&glass, 0), "");
}

// A buffer window scrolls the way a terminal does: the newest
// line sits at the bottom of the box, rows above it blank-padded,
// and the cursor parks out of the way when no input is wanted.
#[test]
fn a_buffer_paints_bottom_aligned() {
    let (mut glass, _script) = glassed(ScriptedCodes::default());
    let mut map = windowed(buffer(), (0, 0, 10, 4));

    saying(&mut map, 1, "Hello\n", style::NORMAL);
    glass.flush(&mut map, Some(1));

    assert_eq!(row_at(&glass, 0), "");
    assert_eq!(row_at(&glass, 1), "");
    assert_eq!(row_at(&glass, 2), "Hello");
    assert_eq!(parked(&mut glass), (0, 7));
}

// Styled runs wear their modifiers, and a plain run wears nothing
// at all.
#[test]
fn styled_runs_dress_for_the_terminal() {
    let (mut glass, _script) = glassed(ScriptedCodes::default());
    let mut map = windowed(buffer(), (0, 0, 20, 2));

    saying(&mut map, 1, "plain ", style::NORMAL);
    saying(&mut map, 1, "slanted", style::EMPHASIZED);
    glass.flush(&mut map, Some(1));

    assert_eq!(row_at(&glass, 1), "plain slanted");
    assert_eq!(dress_at(&glass, 0, 1), Modifier::empty());
    assert_eq!(dress_at(&glass, 6, 1), Modifier::ITALIC);
}

// A grid paints in place at its own box, its per-cell styles
// collapsed into runs.
#[test]
fn a_grid_paints_in_place() {
    let (mut glass, _script) = glassed(ScriptedCodes::default());
    let mut map = windowed(grid(), (2, 1, 12, 3));

    writing(&mut map, 1, 0, 0, "Score: 10");

    map.get_mut(&1).expect("the grid").style = style::SUBHEADER;

    writing(&mut map, 1, 0, 1, "Moves");
    glass.flush(&mut map, Some(1));

    assert!(row_at(&glass, 1).contains("Score: 10"));
    assert!(row_at(&glass, 2).contains("Moves"));
    assert_eq!(dress_at(&glass, 2, 2), Modifier::BOLD);
}

// A split screen paints both halves, and the cursor follows the
// window that is taking input -- whichever side of the pair it
// is.
#[test]
fn a_split_screen_paints_both_windows() {
    let (mut glass, _script) = glassed(ScriptedCodes::default());
    let mut map = WindowMap::new();

    map.insert(1, Window::new(buffer(), 0));
    map.insert(2, Window::new(grid(), 0));
    map.insert(
        3,
        Window::new(
            WindowKind::Pair(PairData::new(
                1,
                2,
                2,
                window_method::ABOVE | window_method::FIXED,
                1,
            )),
            0,
        ),
    );
    rearrange(&mut map, 3, (0, 0, 20, 8));
    writing(&mut map, 2, 0, 0, "Status");
    saying(&mut map, 1, "You are in a maze.\n", style::NORMAL);
    glass.flush(&mut map, Some(3));

    assert!(row_at(&glass, 0).contains("Status"));
    assert!((0..8).any(|y| row_at(&glass, y).contains("You are in a maze.")));
    assert_eq!(parked(&mut glass), (0, 7));

    glass.typed = "n".to_string();
    glass.typing = Some(1);
    glass.flush(&mut map, Some(3));

    assert_eq!(parked(&mut glass), (1, 7));

    glass.typing = Some(2);
    glass.flush(&mut map, Some(3));

    assert_eq!(parked(&mut glass), (7, 0));
}

// A blank window shows blankness, measured by its box: the game
// is told it has no size, but the glass still has rows to cover.
#[test]
fn a_blank_window_shows_blankness() {
    let (mut glass, _script) = glassed(ScriptedCodes::default());

    glass.place(0, 0, &[((style::NORMAL, 0), "XXXXX".to_string())]);
    glass.finish(None);

    assert_eq!(row_at(&glass, 0), "XXXXX");

    let mut map = windowed(WindowKind::Blank, (0, 0, 5, 2));

    glass.flush(&mut map, Some(1));

    assert_eq!(row_at(&glass, 0), "");
    assert_eq!(row_at(&glass, 1), "");
}

// A window the game cleared starts over: the kept text is
// forgotten along with the flag.
#[test]
fn a_cleared_window_starts_over() {
    let (mut glass, _script) = glassed(ScriptedCodes::default());
    let mut map = windowed(buffer(), (0, 0, 20, 3));

    saying(&mut map, 1, "Before the clear.\n", style::NORMAL);
    glass.flush(&mut map, Some(1));

    assert!((0..8).any(|y| row_at(&glass, y).contains("Before the clear.")));

    map.get_mut(&1).expect("the buffer").clear();
    saying(&mut map, 1, "After.\n", style::NORMAL);
    glass.flush(&mut map, Some(1));

    assert!(!(0..8).any(|y| row_at(&glass, y).contains("Before the clear.")));
    assert!((0..8).any(|y| row_at(&glass, y).contains("After.")));
    assert!(!map.get(&1).expect("the buffer").pending_clear);
}

// A rearranged window rewraps its kept text to the new width --
// and a window squeezed to nothing paints nothing, keeping its
// text for better days.
#[test]
fn a_resized_window_rewraps() {
    let (mut glass, _script) = glassed(ScriptedCodes::default());
    let mut map = windowed(buffer(), (0, 0, 20, 3));

    saying(&mut map, 1, "hello wide world\n", style::NORMAL);
    glass.flush(&mut map, Some(1));

    assert!((0..8).any(|y| row_at(&glass, y).contains("hello wide world")));

    rearrange(&mut map, 1, (0, 0, 10, 3));
    glass.flush(&mut map, Some(1));

    // Only the window's own 10-column box repaints: what stood
    // outside it stays on the glass, as it does at the reference
    // -- in play a sibling window covers the rest.
    assert_eq!(row_at(&glass, 0), "hello wide");
    assert!(row_at(&glass, 1).starts_with("world"));

    rearrange(&mut map, 1, (0, 0, 10, 0));
    glass.flush(&mut map, Some(1));

    // Nothing painted: the glass keeps what stood, the cursor
    // parks out of the way, and the text waits for better days.
    assert_eq!(parked(&mut glass), (0, 7));
}

// A width of zero would be no width to wrap to; the fallback
// keeps the arithmetic sensible until the layout says better.
#[test]
fn a_widthless_window_wraps_to_the_fallback() {
    let (mut glass, _script) = glassed(ScriptedCodes::default());
    let mut map = windowed(buffer(), (0, 0, 0, 3));

    saying(&mut map, 1, "narrow\n", style::NORMAL);
    glass.flush(&mut map, Some(1));

    assert_eq!(glass.buffers.get(&1).expect("a kept wrapper").width, 80);
}

// More text than a windowful holds waits behind the pause prompt,
// and a keystroke turns the page instead of reaching the game.
#[test]
fn a_windowful_waits_behind_the_pause() {
    let mut codes = vec![Some(u32::from(' ')); 5];

    codes.push(Some(u32::from('x')));

    let (mut glass, _script) = glassed(ScriptedCodes::new(codes));
    let mut map = windowed(buffer(), (0, 0, 10, 3));
    let text: String = (0..8).map(|index| format!("line {index}\n")).collect();

    saying(&mut map, 1, &text, style::NORMAL);
    glass.flush(&mut map, Some(1));

    assert!(row_at(&glass, 2).starts_with(MORE_PROMPT));
    assert_eq!(dress_at(&glass, 0, 2), Modifier::BOLD | Modifier::REVERSED);
    assert_eq!(parked(&mut glass), (MORE_PROMPT.len() as u16, 2));

    let code = glass.read_char(&mut map, 1);

    assert_eq!(code, Asked::Answer(u32::from('x')));
    assert!(!(0..8).any(|y| row_at(&glass, y).contains(MORE_PROMPT)));
    assert!((0..8).any(|y| row_at(&glass, y).contains("line 7")));
}

// The line being typed is drawn in the input style as part of the
// layout, and accepted whole when Return arrives.
#[test]
fn typing_shows_in_the_input_style() {
    let (mut glass, _script) = glassed(typing("go"));
    let mut map = windowed(buffer(), (0, 0, 20, 3));

    saying(&mut map, 1, "> ", style::NORMAL);
    glass.flush(&mut map, Some(1));

    assert_eq!(
        glass.read_line(&mut map, 1, 80),
        Asked::Answer(("go".to_string(), 0))
    );
    assert!(row_at(&glass, 2).contains("> go"));
    assert_eq!(dress_at(&glass, 2, 2), Modifier::BOLD);
}

// A terminator key ends the line and is reported as having done
// so (Glk: Line Input Events).
#[test]
fn a_terminator_ends_the_line() {
    let (mut glass, _script) = glassed(ScriptedCodes::new(vec![
        Some(u32::from('x')),
        Some(key_code::ESCAPE),
    ]));
    let mut map = windowed(buffer(), (0, 0, 20, 3));
    let mut request = LineRequest::new(None, 0, false);

    request.terminators = vec![key_code::ESCAPE];
    map.get_mut(&1).expect("the buffer").line_request = Some(request);

    assert_eq!(
        glass.read_line(&mut map, 1, 80),
        Asked::Answer(("x".to_string(), key_code::ESCAPE))
    );
}

// Backspace rubs out; Escape clears the whole line and typing
// starts over.
#[test]
fn the_line_edits_classically() {
    let (mut glass, _script) = glassed(ScriptedCodes::new(vec![
        Some(u32::from('a')),
        Some(u32::from('b')),
        Some(key_code::DELETE),
        Some(u32::from('c')),
        Some(key_code::RETURN),
        Some(u32::from('o')),
        Some(key_code::ESCAPE),
        Some(u32::from('n')),
        Some(key_code::RETURN),
    ]));
    let mut map = windowed(buffer(), (0, 0, 20, 3));

    assert_eq!(
        glass.read_line(&mut map, 1, 80),
        Asked::Answer(("ac".to_string(), 0))
    );
    assert_eq!(
        glass.read_line(&mut map, 1, 80),
        Asked::Answer(("n".to_string(), 0))
    );
}

// The line never grows past what the game's buffer can hold.
#[test]
fn the_line_respects_the_buffer() {
    let (mut glass, _script) = glassed(typing("abc"));
    let mut map = windowed(buffer(), (0, 0, 20, 3));

    assert_eq!(
        glass.read_line(&mut map, 1, 2),
        Asked::Answer(("ab".to_string(), 0))
    );
}

// An unusable keystroke is nothing and is waited past; a control
// character reaches the editor and does nothing.
#[test]
fn garbage_keys_are_nothing() {
    let (mut glass, _script) = glassed(ScriptedCodes::new(vec![
        None,
        Some(1),
        Some(u32::from('z')),
        Some(key_code::RETURN),
    ]));
    let mut map = windowed(buffer(), (0, 0, 20, 3));

    assert_eq!(
        glass.read_line(&mut map, 1, 80),
        Asked::Answer(("z".to_string(), 0))
    );
}

// A drained intake ends the session rather than hanging forever.
#[test]
fn a_dead_intake_ends_the_session() {
    let (mut glass, _script) = glassed(ScriptedCodes::default());
    let mut map = windowed(buffer(), (0, 0, 20, 3));

    assert_eq!(glass.read_line(&mut map, 1, 80), Asked::End);
    assert_eq!(glass.read_char(&mut map, 1), Asked::End);
}

// Keystrokes pass through read_char as the Glk codes the intake
// already speaks.
#[test]
fn read_char_speaks_glk() {
    let (mut glass, _script) = glassed(ScriptedCodes::new(vec![
        Some(u32::from('a')),
        Some(key_code::UP),
        Some(key_code::TAB),
        Some(key_code::RETURN),
    ]));
    let mut map = windowed(buffer(), (0, 0, 20, 3));

    assert_eq!(glass.read_char(&mut map, 1), Asked::Answer(u32::from('a')));
    assert_eq!(glass.read_char(&mut map, 1), Asked::Answer(key_code::UP));
    assert_eq!(glass.read_char(&mut map, 1), Asked::Answer(key_code::TAB));
    assert_eq!(
        glass.read_char(&mut map, 1),
        Asked::Answer(key_code::RETURN)
    );
}

// A timer firing mid-line answers its event instead and hands
// control back with the request still pending; the half-typed
// line survives to the next call (Glk: Timer Events).
#[test]
fn a_timer_fires_between_keystrokes() {
    let (mut glass, _clock) = ticking(vec![
        Some(u32::from('g')),
        None,
        Some(u32::from('o')),
        Some(key_code::RETURN),
    ]);
    let mut map = windowed(buffer(), (0, 0, 20, 3));

    glass.set_timer(1500);

    let interrupted = glass.read_line(&mut map, 1, 80);

    assert_eq!(
        interrupted,
        Asked::Instead(vec![Event::new(event_type::TIMER, None, 0, 0)])
    );
    assert_eq!(
        glass.read_line(&mut map, 1, 80),
        Asked::Answer(("go".to_string(), 0))
    );

    glass.set_timer(0);

    assert!(glass.timer.timeout(99.0).is_none());
}

// A grid window taking line input shows it at the cursor, clamped
// to the grid when the game left the cursor past an edge.
#[test]
fn a_grid_takes_input_at_its_cursor() {
    let (mut glass, script) = glassed(typing("hi"));
    let mut map = windowed(grid(), (0, 0, 10, 2));

    glass.flush(&mut map, Some(1));
    map.get_mut(&1).expect("the grid").move_cursor(3, 0);

    assert_eq!(
        glass.read_line(&mut map, 1, 80),
        Asked::Answer(("hi".to_string(), 0))
    );
    assert!(row_at(&glass, 0).contains("hi"));
    assert_eq!(dress_at(&glass, 3, 0), Modifier::BOLD);

    script.borrow_mut().codes = vec![Some(key_code::RETURN)];
    map.get_mut(&1).expect("the grid").move_cursor(20, 5);

    assert_eq!(
        glass.read_line(&mut map, 1, 80),
        Asked::Answer((String::new(), 0))
    );
    assert_eq!(parked(&mut glass), (9, 1));
}

// The file prompt asks on the bottom line, Return answers, Escape
// and an empty answer cancel -- and the interrupted line of play
// is standing where it was afterwards.
#[test]
fn the_file_prompt_asks_on_the_bottom_line() {
    let (mut glass, _clock) = ticking(vec![
        Some(u32::from('g')),
        None,
        Some(u32::from('s')),
        None,
        Some(key_code::RETURN),
        Some(key_code::ESCAPE),
        Some(key_code::RETURN),
        Some(u32::from('o')),
        Some(key_code::RETURN),
    ]);
    let mut map = windowed(buffer(), (0, 0, 20, 3));

    glass.set_timer(1500);

    assert!(matches!(
        glass.read_line(&mut map, 1, 80),
        Asked::Instead(_)
    ));

    // Mid-prompt the bottom line asks; a timer here is not an
    // event, and Return answers with the name.
    assert_eq!(
        glass.prompt_file(&mut map, 0, file_mode::WRITE),
        Some("s".to_string())
    );
    assert_eq!(glass.prompt_file(&mut map, 0, file_mode::READ), None);
    assert_eq!(glass.prompt_file(&mut map, 0, file_mode::WRITE), None);
    assert!(row_at(&glass, 7).contains("which file?"));
    assert_eq!(
        glass.read_line(&mut map, 1, 80),
        Asked::Answer(("go".to_string(), 0))
    );
}

// A file prompt outranks the pager: every window is forced to the
// end first, so the player answers a question instead of fighting
// a pause prompt for the keyboard.
#[test]
fn the_file_prompt_catches_the_windows_up() {
    let (mut glass, _script) = glassed(ScriptedCodes::new(vec![Some(key_code::RETURN)]));
    let mut map = windowed(buffer(), (0, 0, 10, 3));
    let text: String = (0..8).map(|index| format!("line {index}\n")).collect();

    saying(&mut map, 1, &text, style::NORMAL);
    glass.flush(&mut map, Some(1));

    assert!(row_at(&glass, 2).contains(MORE_PROMPT));
    assert_eq!(glass.prompt_file(&mut map, 0, file_mode::WRITE), None);
    assert!(!(0..8).any(|y| row_at(&glass, y).contains(MORE_PROMPT)));
}

// Retiring leaves the cursor under the story, so whatever the
// session prints next lands below the game's last words.
#[test]
fn retiring_parks_below_the_story() {
    let (mut glass, _script) = glassed(ScriptedCodes::default());

    glass.retire();

    assert_eq!(parked(&mut glass), (0, 7));
}

// Two styles are distinguishable exactly when they dress
// differently here, and timer events are honestly on offer.
#[test]
fn styles_distinguish_by_their_dress() {
    let (glass, _script) = glassed(ScriptedCodes::default());
    let window = Window::new(buffer(), 0);

    assert!(glass.timer_input());
    assert!(glass.style_distinguish(&window, style::EMPHASIZED, style::NORMAL));
    assert!(!glass.style_distinguish(&window, style::HEADER, style::SUBHEADER));
}

// Style measurements answer in the terminal's only unit, the
// character cell, and decline what a cell cannot measure.
#[test]
fn style_measures_answer_in_cells() {
    let (glass, _script) = glassed(ScriptedCodes::default());
    let window = Window::new(buffer(), 0);
    let asked = [
        (style::NORMAL, 3),
        (style::HEADER, 4),
        (style::NORMAL, 4),
        (style::EMPHASIZED, 5),
        (style::NORMAL, 5),
        (style::NORMAL, 6),
        (style::NORMAL, 0),
        (style::NORMAL, 1),
        (style::NORMAL, 2),
    ];
    let measures: Vec<Option<u32>> = asked
        .iter()
        .map(|(number, hint)| glass.style_measure(&window, *number, *hint))
        .collect();

    assert_eq!(
        measures,
        vec![
            Some(0),
            Some(1),
            Some(0),
            Some(1),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            None
        ]
    );
}

// A grid row's per-cell dress collapses into runs keyed by style
// and link together, a missing style or link reading as plain --
// so a linked run stays distinct from its neighbours.
#[test]
fn grouping_collapses_a_row_into_runs() {
    assert_eq!(
        grouped(&['a', 'b'], &[style::ALERT], &[0, 0]),
        vec![
            ((style::ALERT, 0), "a".to_string()),
            ((style::NORMAL, 0), "b".to_string())
        ]
    );
    assert_eq!(
        grouped(&['c', 'd'], &[style::NORMAL, style::NORMAL], &[0, 0]),
        vec![((style::NORMAL, 0), "cd".to_string())]
    );
    assert_eq!(
        grouped(&['e', 'f'], &[style::NORMAL, style::NORMAL], &[7]),
        vec![
            ((style::NORMAL, 7), "e".to_string()),
            ((style::NORMAL, 0), "f".to_string())
        ]
    );
}

// The timer keeps its own deadline arithmetic: setting arms it,
// zero disarms it, and a due timer rearms itself for the next
// round (Glk: Timer Events).
#[test]
fn the_timer_comes_round_and_round() {
    let mut timer = Timer::new();

    assert!(timer.timeout(0.0).is_none());
    assert!(!timer.due(0.0));

    timer.set(2000, 0.0);

    assert_eq!(timer.timeout(0.0), Some(2.0));
    assert!(!timer.due(0.0));
    assert_eq!(timer.timeout(3.0), Some(0.0));
    assert!(timer.due(3.0));
    assert_eq!(timer.timeout(3.0), Some(2.0));
}

// Crossterm events translate to Glk character codes: named keys
// to their keycodes, characters to themselves, and a key the
// table cannot spell is nothing usable.
#[test]
fn events_translate_to_glk_codes() {
    let event = |code| TermEvent::Key(KeyEvent::new(code, KeyModifiers::empty()));

    assert_eq!(coded(&event(KeyCode::Enter)), Some(key_code::RETURN));
    assert_eq!(coded(&event(KeyCode::Backspace)), Some(key_code::DELETE));
    assert_eq!(coded(&event(KeyCode::Esc)), Some(key_code::ESCAPE));
    assert_eq!(coded(&event(KeyCode::Up)), Some(key_code::UP));
    assert_eq!(coded(&event(KeyCode::PageDown)), Some(key_code::PAGE_DOWN));
    assert_eq!(coded(&event(KeyCode::Char('n'))), Some(u32::from('n')));
    assert_eq!(coded(&event(KeyCode::F(5))), None);
    assert_eq!(coded(&TermEvent::Resize(80, 24)), None);
}
