//! The line editor: pure transitions, and the shared read loop.

use super::*;

/// An editor with the given lines already typed and submitted.
fn composed(lines: &[&str]) -> LineEditor {
    let mut editor = LineEditor::new();

    for line in lines {
        for character in line.chars() {
            editor.insert(character);
        }

        editor.submit();
    }

    editor
}

/// One echo operation the loop performed on the canvas.
#[derive(Debug, PartialEq, Eq)]
enum Op {
    Write(String),
    Retreat(usize),
}

/// Records the echo operations the loop performs.
#[derive(Default)]
struct FakeCanvas {
    operations: Vec<Op>,
}

impl LineCanvas for FakeCanvas {
    fn write(&mut self, text: &str) {
        self.operations.push(Op::Write(text.to_string()));
    }

    fn retreat(&mut self, cells: usize) -> usize {
        self.operations.push(Op::Retreat(cells));

        cells
    }
}

/// Run one fresh read over scripted keys, None being a heartbeat.
fn run(editor: &mut LineEditor, keys: &[Option<char>]) -> (Option<String>, FakeCanvas, usize) {
    let mut canvas = FakeCanvas::default();
    let mut remaining = keys.to_vec();

    remaining.reverse();

    let mut repaints = 0;
    let line = read_line_edited(
        editor,
        &mut canvas,
        &mut || remaining.pop().expect("a scripted key"),
        &mut |_canvas| repaints += 1,
        true,
    );

    (line, canvas, repaints)
}

/// The scripted keys, every one a real keystroke.
fn keyed(keys: &str) -> Vec<Option<char>> {
    keys.chars().map(Some).collect()
}

/// Whether the operations contain the given pair, in order.
fn contains_pair(operations: &[Op], first: &Op, second: &Op) -> bool {
    operations
        .iter()
        .position(|op| op == first)
        .is_some_and(|start| operations[start + 1..].contains(second))
}

// Typing builds the line at the insertion point, and submit hands
// it over and resets for the next.
#[test]
fn typing_and_submitting() {
    let mut editor = LineEditor::new();

    for character in "go".chars() {
        editor.insert(character);
    }

    assert_eq!(editor.text(), "go");
    assert_eq!(editor.cursor(), 2);
    assert_eq!(editor.submit(), "go");
    assert!(editor.text().is_empty());
}

// Rub-out deletes the character before the insertion point; at the
// line's start there is nothing left of the line to rub.
#[test]
fn rub_out_deletes_before_the_cursor() {
    let mut editor = LineEditor::new();

    for character in "cat".chars() {
        editor.insert(character);
    }

    editor.left();

    assert!(editor.rub_out());
    assert_eq!(editor.text(), "ct");
    assert!(editor.left());
    assert!(!editor.rub_out());
}

// The cursor moves within the line and stops honestly at both ends.
#[test]
fn cursor_motion_stops_at_the_ends() {
    let mut editor = LineEditor::new();

    editor.insert('x');

    assert!(!editor.right());
    assert!(editor.left());
    assert!(!editor.left());
    assert!(editor.right());
}

// An insertion mid-line lands at the cursor, not the end.
#[test]
fn insertion_lands_at_the_cursor() {
    let mut editor = LineEditor::new();

    for character in "gt".chars() {
        editor.insert(character);
    }

    editor.left();
    editor.insert('e');

    assert_eq!(editor.text(), "get");
    assert_eq!(editor.cursor(), 2);
}

// Cursor-up walks back through the session's history, oldest last,
// and stops there; with no history at all it is quietly nothing.
#[test]
fn earlier_walks_back_through_history() {
    assert!(!LineEditor::new().earlier());

    let mut editor = composed(&["north", "south"]);

    assert!(editor.earlier());
    assert_eq!(editor.text(), "south");
    assert!(editor.earlier());
    assert_eq!(editor.text(), "north");
    assert!(!editor.earlier());
}

// Cursor-down walks forward again and, past the newest history
// line, restores the draft that recall interrupted.
#[test]
fn later_returns_to_the_draft() {
    let mut editor = composed(&["north"]);

    for character in "dr".chars() {
        editor.insert(character);
    }

    editor.earlier();

    assert_eq!(editor.text(), "north");
    assert!(editor.later());
    assert_eq!(editor.text(), "dr");
    assert!(!editor.later());
}

// Walking down through the middle of history recalls each line on
// the way back to the draft.
#[test]
fn later_recalls_intermediate_lines() {
    let mut editor = composed(&["north", "south"]);

    editor.earlier();
    editor.earlier();
    editor.later();

    assert_eq!(editor.text(), "south");
}

// An empty line never joins the history, and repeating a command
// records it once -- recall should not walk through repetitions.
#[test]
fn history_skips_empties_and_repeats() {
    let mut editor = composed(&["look", "look"]);

    editor.submit();
    editor.earlier();

    assert_eq!(editor.text(), "look");
    assert!(!editor.earlier());
}

// The history is bounded: the oldest line falls off past the limit.
#[test]
fn history_is_bounded() {
    let lines: Vec<String> = (0..=HISTORY_LIMIT)
        .map(|index| format!("go {index}"))
        .collect();
    let names: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut editor = composed(&names);

    while editor.earlier() {}

    assert_eq!(editor.text(), "go 1");
}

// The loop types a line through the canvas and submits on enter,
// with the fast path writing each appended character as itself.
#[test]
fn loop_types_and_submits() {
    let (line, canvas, _repaints) = run(&mut LineEditor::new(), &keyed("hi\n"));

    assert_eq!(line.as_deref(), Some("hi"));
    assert_eq!(
        canvas.operations,
        vec![
            Op::Write("h".to_string()),
            Op::Write("i".to_string()),
            Op::Write("\n".to_string())
        ]
    );
}

// Idle heartbeats (None) and escape are waited out, as are the
// §3.8.4 input-only codes beyond the editing keys.
#[test]
fn loop_waits_out_unusable_keys() {
    let (line, _canvas, repaints) = run(
        &mut LineEditor::new(),
        &[None, Some('\u{1b}'), Some('\u{85}'), Some('y'), Some('\n')],
    );

    assert_eq!(line.as_deref(), Some("y"));
    assert_eq!(repaints, 2);
}

// An editing key that changes nothing repaints nothing: rub-out at
// the line's start is quiet.
#[test]
fn loop_skips_redraws_that_change_nothing() {
    let (_line, canvas, repaints) = run(&mut LineEditor::new(), &keyed("\u{7f}\n"));

    assert_eq!(canvas.operations, vec![Op::Write("\n".to_string())]);
    assert_eq!(repaints, 1);
}

// A mid-line edit redraws the whole line from its start: retreat to
// the beginning, the new text, and the cursor walked back to its
// place.
#[test]
fn loop_redraws_mid_line_edits() {
    let (line, canvas, _repaints) = run(&mut LineEditor::new(), &keyed("gt\u{83}e\n"));

    assert_eq!(line.as_deref(), Some("get"));
    assert!(contains_pair(
        &canvas.operations,
        &Op::Retreat(2),
        &Op::Write("gt".to_string())
    ));
    assert!(contains_pair(
        &canvas.operations,
        &Op::Write("get".to_string()),
        &Op::Retreat(1)
    ));
}

// Recalling a shorter line blanks the longer draft's remnant with
// spaces, then retreats over them.
#[test]
fn loop_blanks_recall_remnants() {
    let mut editor = composed(&["in"]);
    let (line, canvas, _repaints) = run(&mut editor, &keyed("look\u{81}\n"));

    assert_eq!(line.as_deref(), Some("in"));
    assert!(contains_pair(
        &canvas.operations,
        &Op::Write("in".to_string()),
        &Op::Write("  ".to_string())
    ));
}

// Cursor-down walks recall forward again inside the loop.
#[test]
fn loop_walks_history_both_ways() {
    let mut editor = composed(&["north", "south"]);
    let (line, _canvas, _repaints) = run(&mut editor, &keyed("\u{81}\u{81}\u{82}\n"));

    assert_eq!(line.as_deref(), Some("south"));
}

// The right cursor key moves back over a line the left key walked
// into, restoring the append fast path at the end.
#[test]
fn loop_moves_right_after_left() {
    let (line, _canvas, _repaints) = run(&mut LineEditor::new(), &keyed("a\u{83}\u{84}b\n"));

    assert_eq!(line.as_deref(), Some("ab"));
}

// Raw control characters -- the tab key's "\t" chief among them --
// have no ZSCII code to submit (§3.8) and are waited out, where
// inserting them would crash the session at submit. A bare
// carriage return is the return key itself (§3.8.2.5), for a
// terminal that hands it over unnamed.
#[test]
fn control_characters_are_waited_out() {
    let (line, canvas, _repaints) = run(&mut LineEditor::new(), &keyed("\tg\u{1}o\r"));

    assert_eq!(line.as_deref(), Some("go"));
    assert_eq!(
        canvas.operations,
        vec![
            Op::Write("g".to_string()),
            Op::Write("o".to_string()),
            Op::Write("\n".to_string())
        ]
    );
}

// An EXPIRED answer pauses the read: the loop hands back None with
// the composed line intact, and a fresh=false call resumes it to
// completion -- how a timed read survives its interrupts.
#[test]
fn expiry_pauses_and_resume_completes() {
    let mut editor = LineEditor::new();
    let (line, _canvas, _repaints) = run(&mut editor, &keyed("go\0"));

    assert!(line.is_none());
    assert_eq!(editor.text(), "go");

    let mut canvas = FakeCanvas::default();
    let mut keys = keyed(" n\n");

    keys.reverse();

    let resumed = read_line_edited(
        &mut editor,
        &mut canvas,
        &mut || keys.pop().expect("a scripted key"),
        &mut |_canvas| {},
        false,
    );

    assert_eq!(resumed.as_deref(), Some("go n"));
}
