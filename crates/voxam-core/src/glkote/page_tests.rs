//! The GlkOte update builder: what changed travels, the rest
//! stays home.

use super::json::{Object, Value, dumps};
use super::*;

const BOX: (i64, i64, i64, i64) = (0, 0, 640, 400);
const TOP: (i64, i64, i64, i64) = (0, 0, 640, 30);

fn shown(value: &Value) -> String {
    dumps(value)
}

fn told(stanza: &Object) -> String {
    dumps(&Value::Object(stanza.clone()))
}

fn run(style: &str, link: i64, text: &str) -> Run {
    Run::text(style, link, text)
}

fn declare(page: &mut Page, ident: i64, box_: (i64, i64, i64, i64)) {
    page.window(ident, "buffer", 0, box_, WindowSpec::default())
        .unwrap();
}

fn declare_grid(page: &mut Page, ident: i64, box_: (i64, i64, i64, i64), gridsize: (i64, i64)) {
    page.window(
        ident,
        "grid",
        0,
        box_,
        WindowSpec {
            gridsize: Some(gridsize),
            ..WindowSpec::default()
        },
    )
    .unwrap();
}

fn declare_stage(page: &mut Page, ident: i64, graphsize: (i64, i64)) {
    page.window(
        ident,
        "graphics",
        0,
        BOX,
        WindowSpec {
            graphsize: Some(graphsize),
            scaled: true,
            ..WindowSpec::default()
        },
    )
    .unwrap();
}

// A page with one buffer window declared.
fn buffered() -> Page {
    let mut page = Page::new();

    declare(&mut page, 1, BOX);

    page
}

// A page with one grid window declared.
fn gridded() -> Page {
    let mut page = Page::new();

    declare_grid(&mut page, 1, TOP, (80, 3));

    page
}

// Send one update and redeclare the lone buffer for the next.
fn turned(page: &mut Page) {
    page.update(false, false, None).unwrap();
    declare(page, 1, BOX);
}

// The text entries of the first content element.
fn spans(update: &Object) -> String {
    let content = update.get("content").and_then(Value::as_list).unwrap();
    let text = content[0].as_object().unwrap().get("text").unwrap();

    shown(text)
}

fn refused(result: Result<(), crate::errors::VoxamError>, wants: &str) {
    let error = result.expect_err("the page should refuse").to_string();

    assert!(error.contains(wants), "{error}");
}

// The first update always carries the whole windows array at
// generation one -- the display starts knowing nothing (GlkOte:
// The Generation Number).
#[test]
fn the_first_update_carries_the_whole_tree() {
    let mut page = buffered();
    let update = page.update(false, false, None).unwrap();

    assert_eq!(
        told(&update),
        "{\"type\":\"update\",\"gen\":1,\"windows\":[{\"id\":1,\
         \"type\":\"buffer\",\"rock\":0,\"left\":0,\"top\":0,\
         \"width\":640,\"height\":400}]}"
    );
    assert_eq!(page.generation(), 1);
}

// Even an empty tree's first update says so out loud: the empty
// windows array closes everything, which is not the same as
// omitting it (GlkOte: Output: Updating the Display).
#[test]
fn an_empty_tree_still_speaks_first() {
    let mut page = Page::new();

    assert_eq!(
        told(&page.update(false, false, None).unwrap()),
        "{\"type\":\"update\",\"gen\":1,\"windows\":[]}"
    );
}

// A cycle where nothing changed answers the pass stanza and holds
// the generation where it stood.
#[test]
fn an_unchanged_cycle_passes() {
    let mut page = buffered();

    page.update(false, false, None).unwrap();
    declare(&mut page, 1, BOX);

    assert_eq!(
        told(&page.update(false, false, None).unwrap()),
        "{\"type\":\"pass\"}"
    );
    assert_eq!(page.generation(), 1);
}

// A moved window resends the whole windows array; a stable tree
// with fresh content omits it.
#[test]
fn windows_travel_only_when_the_tree_moves() {
    let mut page = buffered();

    page.update(false, false, None).unwrap();
    declare(&mut page, 1, (0, 30, 640, 400));

    let moved = page.update(false, false, None).unwrap();

    assert_eq!(moved.get("gen"), Some(&Value::Int(2)));
    assert_eq!(
        moved.get("windows").and_then(Value::as_list).unwrap()[0]
            .as_object()
            .unwrap()
            .get("top"),
        Some(&Value::Int(30))
    );

    declare(&mut page, 1, (0, 30, 640, 400));
    page.buffer(1, &[run("normal", 0, "text")], false).unwrap();

    let steady = page.update(false, false, None).unwrap();

    assert!(!steady.contains("windows"));
    assert!(steady.contains("content"));
}

// Closing one window shrinks the array; closing the last sends
// the empty array that closes them all.
#[test]
fn closing_windows_shrinks_the_array() {
    let mut page = Page::new();

    declare(&mut page, 1, BOX);
    declare_grid(&mut page, 2, TOP, (80, 1));
    page.update(false, false, None).unwrap();

    declare(&mut page, 1, BOX);

    let fewer = page.update(false, false, None).unwrap();
    let ids: Vec<i64> = fewer
        .get("windows")
        .and_then(Value::as_list)
        .unwrap()
        .iter()
        .map(|held| {
            held.as_object()
                .unwrap()
                .get("id")
                .unwrap()
                .as_int()
                .unwrap()
        })
        .collect();

    assert_eq!(ids, vec![1]);

    let empty = page.update(false, false, None).unwrap();

    assert_eq!(shown(empty.get("windows").unwrap()), "[]");
}

// A closed window's id is retired for good: the protocol forbids
// reuse (GlkOte: The Windows Update Array).
#[test]
fn a_retired_id_may_never_return() {
    let mut page = buffered();

    page.update(false, false, None).unwrap();
    page.update(false, false, None).unwrap();

    refused(
        page.window(1, "buffer", 0, BOX, WindowSpec::default()),
        "may never return",
    );
}

// Newlines split runs into paragraph entries; text after the last
// newline leaves its paragraph open, and the next cycle's first
// entry continues it with the append flag -- until a newline
// closes it (GlkOte: Buffer Window Updates).
#[test]
fn paragraphs_split_and_append() {
    let mut page = buffered();

    page.buffer(1, &[run("normal", 0, "a\nb")], false).unwrap();

    assert_eq!(
        spans(&page.update(false, false, None).unwrap()),
        "[{\"content\":[{\"style\":\"normal\",\"text\":\"a\"}]},\
         {\"content\":[{\"style\":\"normal\",\"text\":\"b\"}]}]"
    );

    declare(&mut page, 1, BOX);
    page.buffer(1, &[run("normal", 0, "c\n")], false).unwrap();

    assert_eq!(
        spans(&page.update(false, false, None).unwrap()),
        "[{\"append\":true,\"content\":[{\"style\":\"normal\",\"text\":\"c\"}]}]"
    );

    declare(&mut page, 1, BOX);
    page.buffer(1, &[run("normal", 0, "d")], false).unwrap();

    assert_eq!(
        spans(&page.update(false, false, None).unwrap()),
        "[{\"content\":[{\"style\":\"normal\",\"text\":\"d\"}]}]"
    );
}

// Consecutive newlines become blank lines, the empty object; a
// leading newline on an open paragraph only closes it, and on a
// fresh window it is a blank line like any other.
#[test]
fn blank_lines_are_empty_objects() {
    let mut page = buffered();

    page.buffer(1, &[run("normal", 0, "x\n\n\ny")], false)
        .unwrap();

    assert_eq!(
        spans(&page.update(false, false, None).unwrap()),
        "[{\"content\":[{\"style\":\"normal\",\"text\":\"x\"}]},{},{},\
         {\"content\":[{\"style\":\"normal\",\"text\":\"y\"}]}]"
    );

    declare(&mut page, 1, BOX);
    page.buffer(1, &[run("normal", 0, "\nz")], false).unwrap();

    assert_eq!(
        spans(&page.update(false, false, None).unwrap()),
        "[{\"content\":[{\"style\":\"normal\",\"text\":\"z\"}]}]"
    );

    let mut fresh = buffered();

    fresh.buffer(1, &[run("normal", 0, "\nz")], false).unwrap();

    assert_eq!(
        spans(&fresh.update(false, false, None).unwrap()),
        "[{},{\"content\":[{\"style\":\"normal\",\"text\":\"z\"}]}]"
    );
}

// A clear rides the content entry, resets the open paragraph, and
// needs no text to be worth sending.
#[test]
fn a_clear_rides_the_entry() {
    let mut page = buffered();

    page.buffer(1, &[run("normal", 0, "before")], false)
        .unwrap();
    turned(&mut page);

    page.buffer(1, &[run("normal", 0, "after")], true).unwrap();

    let update = page.update(false, false, None).unwrap();
    let entry = update.get("content").and_then(Value::as_list).unwrap()[0]
        .as_object()
        .unwrap();

    assert!(entry.get("clear").is_some_and(Value::is_true));
    assert_eq!(
        shown(entry.get("text").unwrap()),
        "[{\"content\":[{\"style\":\"normal\",\"text\":\"after\"}]}]"
    );

    declare(&mut page, 1, BOX);
    page.buffer(1, &[], true).unwrap();

    let update = page.update(false, false, None).unwrap();

    assert_eq!(
        shown(update.get("content").unwrap()),
        "[{\"id\":1,\"clear\":true}]"
    );

    // An empty helping with nothing to clear is no helping at all.
    declare(&mut page, 1, BOX);
    page.buffer(1, &[run("normal", 0, "")], false).unwrap();

    assert_eq!(
        told(&page.update(false, false, None).unwrap()),
        "{\"type\":\"pass\"}"
    );
}

// Runs wear their style names and hyperlink values; alike
// neighbours coalesce, and a style the protocol does not name is
// refused (GlkOte: The Line Data Array).
#[test]
fn runs_wear_style_and_link() {
    let mut page = buffered();

    page.buffer(
        1,
        &[
            run("header", 0, "H"),
            run("normal", 3, "link"),
            run("normal", 0, "a"),
            run("normal", 0, "b"),
        ],
        false,
    )
    .unwrap();

    assert_eq!(
        spans(&page.update(false, false, None).unwrap()),
        "[{\"content\":[{\"style\":\"header\",\"text\":\"H\"},\
         {\"style\":\"normal\",\"text\":\"link\",\"hyperlink\":3},\
         {\"style\":\"normal\",\"text\":\"ab\"}]}]"
    );

    refused(
        buffered().buffer(1, &[run("fancy", 0, "x")], false),
        "no style is named",
    );
}

// A flow break closes the paragraph and flags the next entry --
// even when the next entry arrives a whole cycle later.
#[test]
fn a_flow_break_flags_what_follows() {
    let mut page = buffered();

    page.buffer(
        1,
        &[
            run("normal", 0, "para"),
            Run::Flowbreak,
            run("normal", 0, "below"),
        ],
        false,
    )
    .unwrap();

    assert_eq!(
        spans(&page.update(false, false, None).unwrap()),
        "[{\"content\":[{\"style\":\"normal\",\"text\":\"para\"}]},\
         {\"flowbreak\":true,\"content\":[{\"style\":\"normal\",\"text\":\"below\"}]}]"
    );

    declare(&mut page, 1, BOX);
    page.buffer(1, &[run("normal", 0, "held"), Run::Flowbreak], false)
        .unwrap();
    turned(&mut page);

    page.buffer(1, &[run("normal", 0, "later")], false).unwrap();

    assert_eq!(
        spans(&page.update(false, false, None).unwrap()),
        "[{\"flowbreak\":true,\"content\":[{\"style\":\"normal\",\"text\":\"later\"}]}]"
    );
}

// Only a grid's changed rows travel, trailing plain whitespace
// stripped first -- so a fresh grid sends only what shows, and a
// row gone blank sends a bare line number (GlkOte: Grid Window
// Updates).
#[test]
fn a_grid_sends_only_changed_rows() {
    let mut page = gridded();
    let face = vec![
        vec![TextRun::new("normal", 0, "Score 10   ")],
        vec![],
        vec![],
    ];

    page.grid(1, &face).unwrap();

    let update = page.update(false, false, None).unwrap();

    assert_eq!(
        shown(update.get("content").unwrap()),
        "[{\"id\":1,\"lines\":[{\"line\":0,\"content\":\
         [{\"style\":\"normal\",\"text\":\"Score 10\"}]}]}]"
    );

    declare_grid(&mut page, 1, TOP, (80, 3));
    page.grid(1, &face).unwrap();

    assert_eq!(
        told(&page.update(false, false, None).unwrap()),
        "{\"type\":\"pass\"}"
    );

    declare_grid(&mut page, 1, TOP, (80, 3));
    page.grid(1, &[vec![], vec![TextRun::new("alert", 0, "!  ")], vec![]])
        .unwrap();

    let update = page.update(false, false, None).unwrap();
    let lines = update.get("content").and_then(Value::as_list).unwrap()[0]
        .as_object()
        .unwrap()
        .get("lines")
        .unwrap();

    assert_eq!(
        shown(lines),
        "[{\"line\":0},{\"line\":1,\"content\":\
         [{\"style\":\"alert\",\"text\":\"!  \"}]}]"
    );
}

// A resized grid forgets its cache: what the display keeps across
// a resize is unspecified, so every row is resent.
#[test]
fn a_resized_grid_resends_its_rows() {
    let mut page = gridded();
    let face = vec![vec![TextRun::new("normal", 0, "steady")], vec![], vec![]];

    page.grid(1, &face).unwrap();
    page.update(false, false, None).unwrap();

    declare_grid(&mut page, 1, (0, 0, 640, 20), (80, 2));
    page.grid(1, &face[..2]).unwrap();

    let update = page.update(false, false, None).unwrap();

    assert_eq!(
        update.get("windows").and_then(Value::as_list).unwrap()[0]
            .as_object()
            .unwrap()
            .get("gridheight"),
        Some(&Value::Int(2))
    );

    let lines = update.get("content").and_then(Value::as_list).unwrap()[0]
        .as_object()
        .unwrap()
        .get("lines")
        .unwrap();

    assert_eq!(
        shown(lines),
        "[{\"line\":0,\"content\":[{\"style\":\"normal\",\"text\":\"steady\"}]}]"
    );
}

// A posted line input carries the current generation and its
// dress; carried unchanged, it keeps that generation and the
// input array stays home (GlkOte: The Input Update Array).
#[test]
fn a_line_input_posts_and_carries() {
    let mut page = Page::new();

    declare(&mut page, 1, BOX);
    declare(&mut page, 2, TOP);
    page.line_input(
        1,
        80,
        LineSpec {
            initial: "go".to_string(),
            terminators: vec!["escape".to_string(), "func5".to_string()],
            ..LineSpec::default()
        },
    )
    .unwrap();

    let update = page.update(false, false, None).unwrap();

    assert_eq!(
        shown(update.get("input").unwrap()),
        "[{\"id\":1,\"type\":\"line\",\"maxlen\":80,\"initial\":\"go\",\
         \"terminators\":[\"escape\",\"func5\"],\"gen\":1}]"
    );

    declare(&mut page, 1, BOX);
    declare(&mut page, 2, TOP);
    page.line_input(
        1,
        80,
        LineSpec {
            initial: "go".to_string(),
            terminators: vec!["escape".to_string(), "func5".to_string()],
            ..LineSpec::default()
        },
    )
    .unwrap();
    page.buffer(2, &[run("normal", 0, "elsewhere")], false)
        .unwrap();

    let carried = page.update(false, false, None).unwrap();

    assert!(!carried.contains("input"));
    assert_eq!(carried.get("gen"), Some(&Value::Int(2)));

    refused(
        buffered().line_input(
            1,
            80,
            LineSpec {
                terminators: vec!["tab".to_string()],
                ..LineSpec::default()
            },
        ),
        "no terminator key",
    );
}

// Content reaching a window recreates its carried field at the
// new generation -- a carried field forbids content, a recreated
// one permits it (GlkOte: The Input Update Array).
#[test]
fn content_recreates_a_carried_field() {
    let mut page = buffered();

    page.line_input(1, 80, LineSpec::default()).unwrap();
    turned(&mut page);

    page.line_input(1, 80, LineSpec::default()).unwrap();
    page.buffer(1, &[run("input", 0, "go north\n")], false)
        .unwrap();

    let update = page.update(false, false, None).unwrap();

    assert_eq!(
        shown(update.get("input").unwrap()),
        "[{\"id\":1,\"type\":\"line\",\"maxlen\":80,\"gen\":2}]"
    );
}

// Changed parameters recreate the field even with no content: a
// carried field's initial and terminators would be ignored.
#[test]
fn changed_parameters_recreate_the_field() {
    let mut page = buffered();

    page.line_input(1, 80, LineSpec::default()).unwrap();
    turned(&mut page);

    page.line_input(
        1,
        80,
        LineSpec {
            initial: "north".to_string(),
            ..LineSpec::default()
        },
    )
    .unwrap();

    let update = page.update(false, false, None).unwrap();

    assert_eq!(
        shown(update.get("input").unwrap()),
        "[{\"id\":1,\"type\":\"line\",\"maxlen\":80,\"initial\":\"north\",\"gen\":2}]"
    );
}

// Cancelling one field resends the shrunken roster; cancelling
// the last sends the empty array that cancels them all.
#[test]
fn cancelling_fields_resends_the_roster() {
    let mut page = Page::new();

    declare(&mut page, 1, BOX);
    declare(&mut page, 2, TOP);
    page.char_input(1, None, false, false).unwrap();
    page.char_input(2, None, false, false).unwrap();
    page.update(false, false, None).unwrap();

    declare(&mut page, 1, BOX);
    declare(&mut page, 2, TOP);
    page.char_input(1, None, false, false).unwrap();

    let fewer = page.update(false, false, None).unwrap();

    assert_eq!(
        shown(fewer.get("input").unwrap()),
        "[{\"id\":1,\"type\":\"char\",\"gen\":1}]"
    );

    declare(&mut page, 1, BOX);
    declare(&mut page, 2, TOP);

    let empty = page.update(false, false, None).unwrap();

    assert_eq!(shown(empty.get("input").unwrap()), "[]");
}

// Character input in a grid carries its cursor; a mouse-and-link
// listener with no typing is the passive form, and one listening
// for nothing is left out entirely.
#[test]
fn char_and_passive_entries() {
    let mut page = Page::new();

    declare_grid(&mut page, 1, TOP, (80, 1));
    page.window(
        2,
        "graphics",
        0,
        BOX,
        WindowSpec {
            graphsize: Some((640, 400)),
            ..WindowSpec::default()
        },
    )
    .unwrap();
    page.char_input(1, Some((3, 0)), false, false).unwrap();
    page.passive_input(2, true, true).unwrap();

    let update = page.update(false, false, None).unwrap();

    assert_eq!(
        shown(update.get("input").unwrap()),
        "[{\"id\":1,\"type\":\"char\",\"xpos\":3,\"ypos\":0,\"gen\":1},\
         {\"id\":2,\"hyperlink\":true,\"mouse\":true}]"
    );

    let mut quiet = buffered();

    quiet.passive_input(1, false, false).unwrap();

    assert!(!quiet.update(false, false, None).unwrap().contains("input"));
}

// The timer travels when it changes, as null when it stops, not
// at all while it holds steady -- and again on a deliberate
// restart, since resending restarts the display's clock (GlkOte:
// The Timer Update).
#[test]
fn the_timer_travels_only_on_change() {
    let mut page = buffered();

    page.timer(100, false);

    assert_eq!(
        page.update(false, false, None).unwrap().get("timer"),
        Some(&Value::Int(100))
    );

    declare(&mut page, 1, BOX);
    page.timer(100, false);

    assert_eq!(
        told(&page.update(false, false, None).unwrap()),
        "{\"type\":\"pass\"}"
    );

    declare(&mut page, 1, BOX);
    page.timer(100, true);

    assert_eq!(
        page.update(false, false, None).unwrap().get("timer"),
        Some(&Value::Int(100))
    );

    declare(&mut page, 1, BOX);
    page.timer(0, false);

    assert_eq!(
        page.update(false, false, None).unwrap().get("timer"),
        Some(&Value::Null)
    );

    declare(&mut page, 1, BOX);
    page.timer(0, false);

    assert_eq!(
        told(&page.update(false, false, None).unwrap()),
        "{\"type\":\"pass\"}"
    );
}

// Drawing operations accumulate and travel in order; a fill names
// its whole rectangle or none of it, and an operation the
// protocol does not draw is refused (GlkOte: Graphics Window
// Updates).
#[test]
fn draw_ops_travel_in_order() {
    let mut page = Page::new();

    page.window(
        1,
        "graphics",
        0,
        BOX,
        WindowSpec {
            graphsize: Some((640, 400)),
            ..WindowSpec::default()
        },
    )
    .unwrap();
    page.draw(
        1,
        vec![Object::from([
            ("special", Value::from("setcolor")),
            ("color", Value::from("#C0207F")),
        ])],
    )
    .unwrap();
    page.draw(
        1,
        vec![Object::from([
            ("special", Value::from("fill")),
            ("x", Value::from(0i64)),
            ("y", Value::from(0i64)),
            ("width", Value::from(8i64)),
            ("height", Value::from(8i64)),
        ])],
    )
    .unwrap();

    let update = page.update(false, false, None).unwrap();

    assert_eq!(
        shown(update.get("content").unwrap()),
        "[{\"id\":1,\"draw\":[{\"special\":\"setcolor\",\"color\":\"#C0207F\"},\
         {\"special\":\"fill\",\"x\":0,\"y\":0,\"width\":8,\"height\":8}]}]"
    );

    refused(
        page.draw(
            1,
            vec![Object::from([
                ("special", Value::from("fill")),
                ("x", Value::from(1i64)),
            ])],
        ),
        "whole rectangle",
    );
    refused(
        page.draw(1, vec![Object::from([("special", Value::from("sparkle"))])]),
        "no drawing operation",
    );
}

// The stage dialect's own drawing words travel like any others: a
// text op places its string of dressed cells, a shift op slides a
// rectangle by a rise, and either missing a field it must name is
// refused at once. A scaled window wears the flag on its entry,
// and only a graphics window may.
#[test]
fn stage_ops_travel_and_validate() {
    let mut page = Page::new();

    declare_stage(&mut page, 1, (320, 200));
    page.draw(
        1,
        vec![
            Object::from([
                ("special", Value::from("text")),
                ("x", Value::from(8i64)),
                ("y", Value::from(16i64)),
                ("text", Value::from("West of House")),
                ("cell", Value::List(vec![Value::Int(8), Value::Int(8)])),
                ("fg", Value::from("#000000")),
                ("bg", Value::from("#FFFFFF")),
            ]),
            Object::from([
                ("special", Value::from("shift")),
                ("x", Value::from(0i64)),
                ("y", Value::from(0i64)),
                ("width", Value::from(320i64)),
                ("height", Value::from(200i64)),
                ("rise", Value::from(8i64)),
            ]),
        ],
    )
    .unwrap();

    let update = page.update(false, false, None).unwrap();

    assert!(
        update.get("windows").and_then(Value::as_list).unwrap()[0]
            .as_object()
            .unwrap()
            .get("scaled")
            .is_some_and(Value::is_true)
    );
    assert_eq!(
        update.get("content").and_then(Value::as_list).unwrap()[0]
            .as_object()
            .unwrap()
            .get("draw")
            .and_then(Value::as_list)
            .unwrap()
            .len(),
        2
    );

    refused(
        page.draw(
            1,
            vec![Object::from([
                ("special", Value::from("text")),
                ("x", Value::from(1i64)),
                ("y", Value::from(1i64)),
                ("text", Value::from("?")),
            ])],
        ),
        "places its string",
    );
    refused(
        page.draw(
            1,
            vec![Object::from([
                ("special", Value::from("shift")),
                ("x", Value::from(0i64)),
                ("y", Value::from(0i64)),
                ("width", Value::from(8i64)),
                ("height", Value::from(8i64)),
            ])],
        ),
        "by a rise",
    );
    refused(
        Page::new().window(
            1,
            "grid",
            0,
            TOP,
            WindowSpec {
                gridsize: Some((80, 1)),
                scaled: true,
                ..WindowSpec::default()
            },
        ),
        "scaled logical space",
    );
}

// The stage's editor is placed: a canvas line request names both
// its cursor and its cell and travels with them, one missing
// either is refused, and a cell anywhere but a canvas is refused
// too -- a grid's display already knows its own cell.
#[test]
fn the_stage_editor_is_placed() {
    let mut page = Page::new();

    declare_stage(&mut page, 1, (320, 200));
    page.line_input(
        1,
        40,
        LineSpec {
            cursor: Some((8, 184)),
            cell: Some((8, 8)),
            ink: Some("#c0ffee".to_string()),
            ..LineSpec::default()
        },
    )
    .unwrap();

    let update = page.update(false, false, None).unwrap();
    let list = update.get("input").and_then(Value::as_list).unwrap();
    let entry = list[0].as_object().unwrap();

    assert_eq!(entry.get("xpos"), Some(&Value::Int(8)));
    assert_eq!(entry.get("ypos"), Some(&Value::Int(184)));
    assert_eq!(shown(entry.get("cell").unwrap()), "[8,8]");
    assert_eq!(entry.get("ink").and_then(Value::as_str), Some("#c0ffee"));

    let mut blind = Page::new();

    declare_stage(&mut blind, 1, (320, 200));
    blind
        .line_input(
            1,
            40,
            LineSpec {
                cell: Some((8, 8)),
                ..LineSpec::default()
            },
        )
        .unwrap();

    let error = blind
        .update(false, false, None)
        .expect_err("no cursor")
        .to_string();

    assert!(error.contains("placed cell"), "{error}");

    let mut celless = Page::new();

    declare_stage(&mut celless, 1, (320, 200));
    celless
        .line_input(
            1,
            40,
            LineSpec {
                cursor: Some((8, 184)),
                ..LineSpec::default()
            },
        )
        .unwrap();

    let error = celless
        .update(false, false, None)
        .expect_err("no cell")
        .to_string();

    assert!(error.contains("placed cell"), "{error}");

    let mut grounded = Page::new();

    declare_grid(&mut grounded, 1, TOP, (80, 1));
    grounded
        .line_input(
            1,
            40,
            LineSpec {
                cursor: Some((0, 0)),
                cell: Some((8, 8)),
                ..LineSpec::default()
            },
        )
        .unwrap();

    let error = grounded
        .update(false, false, None)
        .expect_err("no stage")
        .to_string();

    assert!(error.contains("no stage"), "{error}");
}

// The cycle's pieces must agree: content belongs to a declared
// window of the right kind, buffers take no clicks, and grid
// input names its cursor.
#[test]
fn contradictory_cycles_are_refused() {
    let mut page = Page::new();

    page.buffer(9, &[run("normal", 0, "lost")], false).unwrap();

    let error = page
        .update(false, false, None)
        .expect_err("undeclared")
        .to_string();

    assert!(error.contains("never declared"), "{error}");

    let mut unasked = Page::new();

    unasked.char_input(9, None, false, false).unwrap();

    let error = unasked
        .update(false, false, None)
        .expect_err("unasked")
        .to_string();

    assert!(error.contains("input was asked"), "{error}");

    let mut crowded = buffered();

    refused(
        crowded.window(1, "buffer", 0, BOX, WindowSpec::default()),
        "declared twice",
    );

    let mut rowed = buffered();

    rowed.grid(1, &[vec![]]).unwrap();

    let error = rowed
        .update(false, false, None)
        .expect_err("not a grid")
        .to_string();

    assert!(error.contains("not a grid"), "{error}");

    let mut clicked = buffered();

    clicked.char_input(1, None, false, true).unwrap();

    let error = clicked
        .update(false, false, None)
        .expect_err("clicked")
        .to_string();

    assert!(error.contains("takes no clicks"), "{error}");

    let mut blind = Page::new();

    declare_grid(&mut blind, 1, TOP, (80, 1));
    blind.char_input(1, None, false, false).unwrap();

    let error = blind
        .update(false, false, None)
        .expect_err("cursorless")
        .to_string();

    assert!(error.contains("at a cursor"), "{error}");

    refused(
        Page::new().window(1, "porthole", 0, BOX, WindowSpec::default()),
        "cannot be a",
    );
    refused(
        Page::new().window(1, "grid", 0, TOP, WindowSpec::default()),
        "columns and rows",
    );
    refused(
        Page::new().window(
            1,
            "buffer",
            0,
            BOX,
            WindowSpec {
                graphsize: Some((1, 1)),
                ..WindowSpec::default()
            },
        ),
        "drawable size",
    );
}

// One window, one helping: text, rows, and input arrive once per
// cycle each.
#[test]
fn second_helpings_are_refused() {
    let mut page = buffered();

    page.buffer(1, &[run("normal", 0, "once")], false).unwrap();

    refused(
        page.buffer(1, &[run("normal", 0, "twice")], false),
        "fed text twice",
    );

    let mut rows = gridded();

    rows.grid(1, &[vec![]]).unwrap();

    refused(rows.grid(1, &[vec![]]), "fed rows twice");

    let mut asked = buffered();

    asked.line_input(1, 80, LineSpec::default()).unwrap();

    refused(asked.char_input(1, None, false, false), "input twice");
}

// What the player has typed rides a regenerated field as its
// initial -- and only a regenerated one: a carried field keeps
// its editing state at the display, a quiet cycle stays the pass,
// and the roster's memory keeps the game's own dress so a steady
// request never churns.
#[test]
fn typing_survives_a_fields_regeneration() {
    let mut page = buffered();

    page.line_input(1, 80, LineSpec::default()).unwrap();
    page.update(false, false, None).unwrap();

    page.typed(std::collections::HashMap::from([(1, "go nor".to_string())]));

    declare(&mut page, 1, BOX);
    page.line_input(1, 80, LineSpec::default()).unwrap();

    assert_eq!(
        told(&page.update(false, false, None).unwrap()),
        "{\"type\":\"pass\"}"
    );

    declare(&mut page, 1, BOX);
    page.line_input(1, 80, LineSpec::default()).unwrap();
    page.buffer(1, &[run("normal", 0, "The clock strikes.\n")], false)
        .unwrap();

    let update = page.update(false, false, None).unwrap();

    assert_eq!(
        shown(update.get("input").unwrap()),
        "[{\"id\":1,\"type\":\"line\",\"maxlen\":80,\"gen\":2,\
         \"initial\":\"go nor\"}]"
    );

    declare(&mut page, 1, BOX);
    page.line_input(1, 80, LineSpec::default()).unwrap();

    assert_eq!(
        told(&page.update(false, false, None).unwrap()),
        "{\"type\":\"pass\"}"
    );

    page.typed(std::collections::HashMap::new());
    declare(&mut page, 1, BOX);
    page.line_input(1, 80, LineSpec::default()).unwrap();
    page.buffer(1, &[run("normal", 0, "Later.\n")], false)
        .unwrap();

    let update = page.update(false, false, None).unwrap();

    assert_eq!(
        shown(update.get("input").unwrap()),
        "[{\"id\":1,\"type\":\"line\",\"maxlen\":80,\"gen\":3}]"
    );
}

// A file ask rides the update as special input and forces one on
// its own; a second ask in a cycle, and names the protocol lacks,
// are loud.
#[test]
fn a_file_ask_rides_the_update() {
    let mut page = buffered();

    page.update(false, false, None).unwrap();
    declare(&mut page, 1, BOX);
    page.prompt("write", "save").unwrap();

    assert_eq!(
        told(&page.update(false, false, None).unwrap()),
        "{\"type\":\"update\",\"gen\":2,\"specialinput\":\
         {\"type\":\"fileref_prompt\",\"filemode\":\"write\",\
         \"filetype\":\"save\"}}"
    );

    let mut asked = buffered();

    asked.prompt("read", "data").unwrap();

    refused(asked.prompt("read", "data"), "one file");
    refused(buffered().prompt("scribble", "save"), "no file prompt asks");
}

// An exit rides an update of its own making: the game is over,
// and that is worth a generation even when nothing else moved.
#[test]
fn exit_forces_a_real_update() {
    let mut page = buffered();

    page.update(false, false, None).unwrap();
    declare(&mut page, 1, BOX);

    assert_eq!(
        told(&page.update(true, false, None).unwrap()),
        "{\"type\":\"update\",\"gen\":2,\"exit\":true}"
    );
}

// A record with none of the card's four fields makes no card at
// all -- no stray blank lines for a bibliography-less blorb.
#[test]
fn an_empty_record_makes_no_card() {
    assert!(carded(&IFiction::default()).is_empty());
}

// The card's four fields land in their dresses, description
// paragraphs separated by blank lines, a closing blank line
// after.
#[test]
fn a_full_record_makes_the_card() {
    let record = IFiction {
        title: Some("Trinity".to_string()),
        headline: Some("An interactive fantasy".to_string()),
        author: Some("Brian Moriarty".to_string()),
        description: Some("First line.\nSecond line.".to_string()),
        ..IFiction::default()
    };

    assert_eq!(
        carded(&record),
        vec![
            ("header".to_string(), "Trinity\n".to_string()),
            (
                "emphasized".to_string(),
                "An interactive fantasy\n".to_string()
            ),
            ("emphasized".to_string(), "Brian Moriarty\n".to_string()),
            (
                "normal".to_string(),
                "\nFirst line.\n\nSecond line.\n".to_string()
            ),
            ("normal".to_string(), "\n".to_string()),
        ]
    );
}

// The metrics fallback chain: qualified name, generic, default --
// and the margin's extra rung through the bare margin key.
#[test]
fn measured_walks_the_fallback_chain() {
    let metrics = Object::from([
        ("buffercharwidth", Value::Float(9.5)),
        ("charheight", Value::Int(18)),
        ("margin", Value::Int(4)),
        ("gridmarginy", Value::Int(2)),
    ]);

    assert_eq!(measured(&metrics, "buffer"), (9.5, 18.0, 4.0, 4.0));
    assert_eq!(measured(&metrics, "grid"), (1.0, 18.0, 4.0, 2.0));
}

// Partial input arrives keyed by strings and lands keyed by ids;
// anything not shaped like typing is quietly no typing at all.
#[test]
fn partials_land_keyed_by_id() {
    let held = json::loads("{\"1\":\"go nor\",\"x\":\"no\",\"2\":7}").unwrap();
    let stashed = partials(Some(&held));

    assert_eq!(stashed.len(), 1);
    assert_eq!(stashed.get(&1).map(String::as_str), Some("go nor"));
    assert!(partials(Some(&Value::Int(3))).is_empty());
    assert!(partials(None).is_empty());
}

// Stanzas read line by line, blank lines passed over, a hangup
// answering None -- and what is not an object is refused.
#[test]
fn stanzas_read_and_write() {
    let mut feed = std::io::Cursor::new(b"\n{\"type\":\"init\",\"gen\":0}\n".to_vec());
    let stanza = read_stanza(&mut feed).unwrap().unwrap();

    assert_eq!(stanza.get("type").and_then(Value::as_str), Some("init"));

    let mut dry = std::io::Cursor::new(Vec::new());

    assert!(read_stanza(&mut dry).unwrap().is_none());

    let mut listed = std::io::Cursor::new(b"[1,2]\n".to_vec());
    let error = read_stanza(&mut listed)
        .expect_err("not an object")
        .to_string();

    assert!(error.contains("JSON object"), "{error}");

    let mut out: Vec<u8> = Vec::new();
    let mut told_stanza = Object::new();

    told_stanza.set("type", "pass");
    write_stanza(&mut out, &told_stanza);

    assert_eq!(out, b"{\"type\":\"pass\"}\n");
}

// The voxam sidecar rides a real update between the sounds and the
// exit flag, granted by the caller alone; None leaves the stanza
// untouched, and a cycle where nothing changed stays the pass --
// the sidecar never forces an update (PORT: What the sidecar
// carries).
#[test]
fn the_voxam_block_rides_the_update() {
    let mut page = Page::new();

    page.window(1, "buffer", 0, (0, 0, 640, 400), WindowSpec::default())
        .unwrap();

    let mut block = Object::new();

    block.set("command", "north");

    let update = page.update(false, false, Some(block.clone())).unwrap();

    assert_eq!(
        dumps(update.get("voxam").unwrap()),
        r#"{"command":"north"}"#
    );
    assert_eq!(update.iter().last().map(|(key, _)| key), Some("voxam"));

    page.window(1, "buffer", 0, (0, 0, 640, 400), WindowSpec::default())
        .unwrap();

    assert_eq!(
        told(&page.update(false, false, Some(block)).unwrap()),
        r#"{"type":"pass"}"#
    );

    let mut ended = Page::new();

    ended
        .window(1, "buffer", 0, (0, 0, 640, 400), WindowSpec::default())
        .unwrap();

    let told_whole = ended.update(true, false, Some(Object::new())).unwrap();
    let keys: Vec<&str> = told_whole.iter().map(|(key, _)| key).collect();

    assert_eq!(&keys[keys.len() - 2..], ["voxam", "exit"]);

    let mut plain = Page::new();

    plain
        .window(1, "buffer", 0, (0, 0, 640, 400), WindowSpec::default())
        .unwrap();

    assert!(
        plain
            .update(false, false, None)
            .unwrap()
            .get("voxam")
            .is_none()
    );
}
