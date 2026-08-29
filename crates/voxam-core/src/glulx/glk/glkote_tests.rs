//! The GlkOte face of Glk: composed updates, delivered events.

use super::*;
use crate::base64::b64;
use crate::blorb::Blorb;
use crate::glkote::json::dumps;
use crate::glkote::json::loads;
use crate::glulx::bridge::plain;
use crate::glulx::glk::frontend::NullFrontend;
use crate::glulx::glk::objects::{GridData, MemArray, PairData, window_method, window_type};
use crate::glulx::testing;
use crate::iff::chunk;

const ABOVE_FIXED: u32 = window_method::ABOVE | window_method::FIXED;

const TEXT_BUFFER_AT: u32 = 0x110;
const KEYCODES_AT: u32 = 0x130;

fn parsed(text: &str) -> Object {
    match loads(text).unwrap() {
        Value::Object(held) => held,
        _ => panic!("a stanza is an object"),
    }
}

fn at<'held>(entry: &'held Object, key: &str) -> &'held Value {
    entry
        .get(key)
        .unwrap_or_else(|| panic!("no {key} in {}", dumps(&Value::Object(entry.clone()))))
}

fn items(value: &Value) -> &Vec<Value> {
    match value {
        Value::List(held) => held,
        _ => panic!("not a list"),
    }
}

fn entry(value: &Value) -> &Object {
    match value {
        Value::Object(held) => held,
        _ => panic!("not an object"),
    }
}

fn int_of(value: &Value) -> i64 {
    value.as_int().expect("an int")
}

fn str_of(value: &Value) -> &str {
    value.as_str().expect("a string")
}

fn told(value: &Value) -> String {
    dumps(value)
}

fn scratch_memory() -> Memory {
    Memory::new(&crate::glulx::story::Story::new(testing::image(&[])).unwrap())
}

/// A silent display that claims graphics, so canvases open.
struct Canvased;

impl Frontend for Canvased {
    fn graphics(&self) -> bool {
        true
    }

    fn size(&self) -> (i64, i64) {
        (80, 24)
    }

    fn flush(&mut self, _windows: &mut WindowMap, _root: Option<u32>) {}

    fn read_line(
        &mut self,
        _windows: &mut WindowMap,
        _window: u32,
        _maxlen: u32,
    ) -> Asked<(String, u32)> {
        Asked::End
    }

    fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
        Asked::End
    }
}

/// A library with a buffer window at the root.
fn rooted(frontend: Box<dyn Frontend>) -> (Glk, u32) {
    let mut library = Glk::new(frontend);
    let window = plain(library.glk_window_open(None, 0, 0, window_type::TEXT_BUFFER, 0))
        .unwrap()
        .expect("the root window opened");

    (library, window)
}

/// One whole cycle: compose the library and take the update.
fn turned(composer: &mut Composer, library: &mut Glk, memory: &Memory, page: &mut Page) -> Object {
    composer.compose(library, memory, page).unwrap();

    page.update(false, false).unwrap()
}

fn saying(library: &mut Glk, window: u32, text: &str, style_number: u32) {
    let held = library.windows.get_mut(&window).expect("the window lives");

    held.style = style_number;

    for character in text.chars() {
        held.put_char(u32::from(character), 0);
    }
}

// A split tree composes to a flat pair of drawn windows -- the
// pair itself stays home -- with boxes and grid sizes translated
// from the model's own arrangement.
#[test]
fn a_split_tree_composes_flat() {
    let (mut library, root) = rooted(Box::new(NullFrontend));

    plain(library.glk_window_open(Some(root), ABOVE_FIXED, 2, window_type::TEXT_GRID, 7)).unwrap();

    let memory = scratch_memory();
    let update = turned(
        &mut Composer::new(),
        &mut library,
        &memory,
        &mut Page::new(),
    );

    assert_eq!(
        told(at(&update, "windows")),
        r#"[{"id":1,"type":"buffer","rock":0,"left":0,"top":2,"width":80,"height":22},{"id":2,"type":"grid","rock":7,"left":0,"top":0,"width":80,"height":2,"gridwidth":80,"gridheight":2}]"#
    );
}

// Ids are minted once and never come back: a closed grid's id is
// retired, and its replacement is a new window with a new number.
#[test]
fn ids_are_never_reused() {
    let (mut library, root) = rooted(Box::new(NullFrontend));
    let mut composer = Composer::new();
    let mut page = Page::new();
    let memory = scratch_memory();
    let grid =
        plain(library.glk_window_open(Some(root), ABOVE_FIXED, 2, window_type::TEXT_GRID, 0))
            .unwrap();

    turned(&mut composer, &mut library, &memory, &mut page);
    plain(library.glk_window_close(grid, None)).unwrap();
    turned(&mut composer, &mut library, &memory, &mut page);
    plain(library.glk_window_open(Some(root), ABOVE_FIXED, 2, window_type::TEXT_GRID, 0)).unwrap();

    let update = turned(&mut composer, &mut library, &memory, &mut page);

    assert_eq!(
        items(at(&update, "windows"))
            .iter()
            .map(|held| int_of(at(entry(held), "id")))
            .collect::<Vec<_>>(),
        [1, 3]
    );
    assert_eq!(composer.idents.len(), 2);
}

// Buffer text drains destructively into named-style runs -- the
// seventh style spells blockquote, one word -- and a pending clear
// rides along, consumed.
#[test]
fn buffer_text_drains_into_named_runs() {
    let (mut library, root) = rooted(Box::new(NullFrontend));
    let mut composer = Composer::new();
    let mut page = Page::new();
    let memory = scratch_memory();

    saying(&mut library, root, "quoth", style::BLOCK_QUOTE);

    let update = turned(&mut composer, &mut library, &memory, &mut page);

    assert_eq!(
        told(at(&update, "content")),
        r#"[{"id":1,"text":[{"content":[{"style":"blockquote","text":"quoth"}]}]}]"#
    );
    assert_eq!(
        told(&Value::Object(turned(
            &mut composer,
            &mut library,
            &memory,
            &mut page
        ))),
        r#"{"type":"pass"}"#
    );

    library.glk_window_clear(Some(root));

    let cleared = turned(&mut composer, &mut library, &memory, &mut page);

    assert_eq!(told(at(&cleared, "content")), r#"[{"id":1,"clear":true}]"#);
    assert!(!library.windows[&root].pending_clear);
}

// A style number beyond the eleven composes as normal, the same
// plainness the painted displays give it.
#[test]
fn a_wild_style_composes_normal() {
    let (mut library, root) = rooted(Box::new(NullFrontend));

    saying(&mut library, root, "odd", 23);

    let memory = scratch_memory();
    let update = turned(
        &mut Composer::new(),
        &mut library,
        &memory,
        &mut Page::new(),
    );

    assert_eq!(
        told(at(entry(&items(at(&update, "content"))[0]), "text")),
        r#"[{"content":[{"style":"normal","text":"odd"}]}]"#
    );
}

// Grid rows arrive through the same grouping the painted displays
// use: per-cell dress collapsed into runs, only what shows sent.
#[test]
fn grid_rows_compose_through_grouping() {
    let (mut library, root) = rooted(Box::new(NullFrontend));
    let grid =
        plain(library.glk_window_open(Some(root), ABOVE_FIXED, 2, window_type::TEXT_GRID, 0))
            .unwrap()
            .expect("the split opened a grid");

    saying(&mut library, grid, "Score", style::SUBHEADER);

    let memory = scratch_memory();
    let update = turned(
        &mut Composer::new(),
        &mut library,
        &memory,
        &mut Page::new(),
    );

    assert_eq!(
        told(at(&update, "content")),
        r#"[{"id":2,"lines":[{"line":0,"content":[{"style":"subheader","text":"Score"}]}]}]"#
    );
}

// A line request composes whole: capacity as maxlen, the buffer's
// pre-filled text as initial, and terminators by name -- the keys
// the protocol cannot name dropped.
#[test]
fn a_line_request_composes_whole() {
    let (mut library, root) = rooted(Box::new(NullFrontend));
    let mut memory = scratch_memory();
    let held = MemArray {
        address: TEXT_BUFFER_AT,
        count: 8,
        width: 1,
    };

    held.set(&mut memory, 0, u32::from(b'g')).unwrap();
    held.set(&mut memory, 1, u32::from(b'o')).unwrap();
    plain(library.glk_request_line_event(Some(root), Some(held), 2)).unwrap();

    let keys = MemArray {
        address: KEYCODES_AT,
        count: 3,
        width: 4,
    };

    keys.set(&mut memory, 0, key_code::ESCAPE).unwrap();
    keys.set(&mut memory, 1, key_code::TAB).unwrap();
    keys.set(&mut memory, 2, key_code::FUNC5).unwrap();
    plain(library.glk_set_terminators_line_event(&memory, Some(root), Some(keys))).unwrap();

    let update = turned(
        &mut Composer::new(),
        &mut library,
        &memory,
        &mut Page::new(),
    );

    assert_eq!(
        told(at(&update, "input")),
        r#"[{"id":1,"type":"line","maxlen":8,"initial":"go","terminators":["escape","func5"],"gen":1}]"#
    );

    // A request with no buffer at all holds nothing and asks for
    // nothing beyond its zero capacity.
    let (mut bare, spare) = rooted(Box::new(NullFrontend));

    plain(bare.glk_request_line_event(Some(spare), None, 0)).unwrap();

    let asked = turned(&mut Composer::new(), &mut bare, &memory, &mut Page::new());

    assert_eq!(
        told(at(&asked, "input")),
        r#"[{"id":1,"type":"line","maxlen":0,"gen":1}]"#
    );
}

// Grid input carries the cursor, clamped inside the grid the way
// the painted displays clamp it.
#[test]
fn grid_input_carries_the_clamped_cursor() {
    let (mut library, root) = rooted(Box::new(NullFrontend));
    let grid =
        plain(library.glk_window_open(Some(root), ABOVE_FIXED, 2, window_type::TEXT_GRID, 0))
            .unwrap();

    plain(library.glk_window_move_cursor(grid, 500, 500)).unwrap();
    plain(library.glk_request_char_event(grid)).unwrap();

    let memory = scratch_memory();
    let update = turned(
        &mut Composer::new(),
        &mut library,
        &memory,
        &mut Page::new(),
    );

    assert_eq!(
        told(at(&update, "input")),
        r#"[{"id":2,"type":"char","xpos":79,"ypos":1,"gen":1}]"#
    );
}

// Click and link listening translate to the passive form -- except
// on a buffer, where the protocol takes no clicks and the request
// is quietly set aside.
#[test]
fn clicks_and_links_translate() {
    let (mut library, root) = rooted(Box::new(NullFrontend));
    let grid =
        plain(library.glk_window_open(Some(root), ABOVE_FIXED, 2, window_type::TEXT_GRID, 0))
            .unwrap();

    library.glk_request_mouse_event(grid);
    library.glk_request_hyperlink_event(Some(root));
    library.glk_request_mouse_event(Some(root));

    let memory = scratch_memory();
    let update = turned(
        &mut Composer::new(),
        &mut library,
        &memory,
        &mut Page::new(),
    );

    assert_eq!(
        told(at(&update, "input")),
        r#"[{"id":1,"hyperlink":true},{"id":2,"mouse":true}]"#
    );
}

// The timer cadence passes through without a restart claim: from
// polled state, a re-request at the same value is invisible.
#[test]
fn the_timer_passes_through() {
    let (mut library, _) = rooted(Box::new(NullFrontend));
    let mut composer = Composer::new();
    let mut page = Page::new();
    let memory = scratch_memory();

    library.glk_request_timer_events(250);

    assert_eq!(
        int_of(at(
            &turned(&mut composer, &mut library, &memory, &mut page),
            "timer"
        )),
        250
    );
    assert_eq!(
        told(&Value::Object(turned(
            &mut composer,
            &mut library,
            &memory,
            &mut page
        ))),
        r#"{"type":"pass"}"#
    );
}

// A canvas declares its drawable size and keeps its pending clear:
// clearing is a background fill, and the background lives with the
// display that draws, not with the model.
#[test]
fn a_canvas_keeps_its_pending_clear() {
    let (mut library, root) = rooted(Box::new(Canvased));
    let canvas =
        plain(library.glk_window_open(Some(root), ABOVE_FIXED, 8, window_type::GRAPHICS, 0))
            .unwrap()
            .expect("the canvas opened");

    let memory = scratch_memory();
    let update = turned(
        &mut Composer::new(),
        &mut library,
        &memory,
        &mut Page::new(),
    );
    let drawn = items(at(&update, "windows"))
        .iter()
        .map(entry)
        .find(|held| held.get("type").and_then(Value::as_str) == Some("graphics"))
        .expect("the canvas entry");

    assert_eq!(int_of(at(drawn, "graphwidth")), 80);
    assert_eq!(int_of(at(drawn, "graphheight")), 8);
    assert!(library.windows[&canvas].pending_clear);
}

// A blank window stays home: the protocol's window list knows only
// the three drawn kinds.
#[test]
fn a_blank_window_stays_home() {
    let (mut library, root) = rooted(Box::new(NullFrontend));

    plain(library.glk_window_open(Some(root), ABOVE_FIXED, 2, window_type::BLANK, 0)).unwrap();

    let memory = scratch_memory();
    let update = turned(
        &mut Composer::new(),
        &mut library,
        &memory,
        &mut Page::new(),
    );

    assert_eq!(items(at(&update, "windows")).len(), 1);
    assert_eq!(
        str_of(at(entry(&items(at(&update, "windows"))[0]), "type")),
        "buffer"
    );
}

// -- the frontend and its conversation ---------------------------------------

/// A frontend that has heard its init.
fn begun(support: &str, metrics: &str) -> Rc<RefCell<GlkOteFrontend>> {
    let face = Rc::new(RefCell::new(GlkOteFrontend::new()));

    face.borrow_mut()
        .begin(&parsed(&format!(
            r#"{{"type":"init","gen":0,"support":[{support}],"metrics":{metrics}}}"#
        )))
        .unwrap();

    face
}

fn plain_metrics() -> &'static str {
    r#"{"width":80,"height":24}"#
}

fn handle(face: &Rc<RefCell<GlkOteFrontend>>) -> SharedFace {
    shared(face)
}

/// A library over a spoken-for display, a buffer at the root.
fn sessioned() -> (Glk, Rc<RefCell<GlkOteFrontend>>, u32, Memory) {
    let face = begun(r#""timer","graphicswin","hyperlinks""#, plain_metrics());
    let (library, window) = rooted(Box::new(handle(&face)));

    (library, face, window, scratch_memory())
}

/// A session with a canvas split above the buffer root.
fn canvased() -> (Glk, Rc<RefCell<GlkOteFrontend>>, u32, Memory) {
    let (mut library, face, root, memory) = sessioned();
    let canvas =
        plain(library.glk_window_open(Some(root), ABOVE_FIXED, 8, window_type::GRAPHICS, 0))
            .unwrap()
            .expect("the canvas opened");

    (library, face, canvas, memory)
}

fn rendered(face: &Rc<RefCell<GlkOteFrontend>>, library: &mut Glk, memory: &Memory) -> Object {
    face.borrow_mut().render(library, memory, false).unwrap()
}

fn accepted(
    face: &Rc<RefCell<GlkOteFrontend>>,
    library: &mut Glk,
    memory: &mut Memory,
    stanza: &str,
) -> Accepted {
    face.borrow_mut()
        .accept(library, memory, &parsed(stanza))
        .unwrap()
}

fn event_of(verdict: Accepted) -> Event {
    match verdict {
        Accepted::Event(held) => held,
        other => panic!("no event landed: {other:?}"),
    }
}

// The init event grants the capabilities: graphicswin for
// canvases, bare graphics for pictures in a buffer's text flow,
// timer for timers, hyperlinks for links; clicks need no grant.
#[test]
fn the_init_grants_the_capabilities() {
    let face = begun(r#""timer","graphicswin","hyperlinks""#, plain_metrics());
    let front = handle(&face);

    assert!(front.suspends());
    assert!(front.mouse_input());
    assert!(front.timer_input());
    assert!(front.graphics());
    assert!(!front.buffer_images());
    assert!(front.hyperlink_input());
    assert!(!front.sound());

    let bare = handle(&begun(r#""graphics""#, plain_metrics()));

    assert!(!bare.graphics());
    assert!(bare.buffer_images());
    assert!(!bare.timer_input());
    assert!(!bare.hyperlink_input());
    assert!(bare.mouse_input());

    let heard = handle(&begun(r#""sound""#, plain_metrics()));

    assert!(heard.sound());
}

// The metrics measure the display and its cells, falling back from
// the qualified key to the generic to the default, the rules
// RemGlk reads by.
#[test]
fn metrics_measure_the_cells() {
    let face = begun(
        r#""timer""#,
        r#"{"width":640,"height":480,"gridcharwidth":8,"gridcharheight":16,"gridmarginx":4,"charheight":12,"marginy":3,"margin":2,"graphicsmarginx":6}"#,
    );
    let front = handle(&face);

    assert_eq!(face.borrow().size().unwrap(), (640, 480));
    assert_eq!(
        front.metrics_for(&Window::new(WindowKind::Grid(GridData::default()), 0)),
        Metrics::new(8.0, 16.0, 4.0, 3.0)
    );
    assert_eq!(
        front.metrics_for(&Window::new(WindowKind::Buffer(Default::default()), 0)),
        Metrics::new(1.0, 12.0, 2.0, 3.0)
    );
    assert_eq!(
        front.metrics_for(&Window::new(WindowKind::Graphics(Default::default()), 0)),
        Metrics::new(1.0, 1.0, 6.0, 3.0)
    );
    assert_eq!(
        front.metrics_for(&Window::new(
            WindowKind::Pair(PairData::new(0, 0, 0, 0, 0)),
            0
        )),
        CHARACTER_CELL
    );
}

// A display that has not spoken its init has no size to answer
// with, and metrics that carry no size are refused outright.
#[test]
fn a_sizeless_display_is_refused() {
    assert!(
        GlkOteFrontend::new()
            .size()
            .unwrap_err()
            .to_string()
            .contains("not spoken its init")
    );

    let refused = GlkOteFrontend::new()
        .begin(&parsed(
            r#"{"type":"init","gen":0,"metrics":{"width":640}}"#,
        ))
        .unwrap_err();

    assert!(refused.to_string().contains("carry no size"));
}

// A suspending display is never asked for input; its flush paints
// nothing. (The reference's "not attached" render refusal has no
// Rust spelling: render takes the library as an argument.)
#[test]
#[should_panic(expected = "never asked for a line")]
fn a_suspending_display_is_never_asked_for_a_line() {
    let face = begun(r#""timer""#, plain_metrics());
    let mut windows = WindowMap::new();

    handle(&face).flush(&mut windows, None);
    handle(&face).read_line(&mut windows, 1, 80);
}

#[test]
#[should_panic(expected = "never asked for a keystroke")]
fn a_suspending_display_is_never_asked_for_a_key() {
    let face = begun(r#""timer""#, plain_metrics());
    let mut windows = WindowMap::new();

    handle(&face).read_char(&mut windows, 1);
}

// Render speaks a full update first, the pass while nothing moves,
// and an exit rides the finale.
#[test]
fn render_speaks_updates_and_passes() {
    let (mut library, face, _, memory) = sessioned();
    let first = rendered(&face, &mut library, &memory);

    assert_eq!(str_of(at(&first, "type")), "update");
    assert_eq!(int_of(at(&first, "gen")), 1);
    assert_eq!(items(at(&first, "windows")).len(), 1);
    assert_eq!(
        told(&Value::Object(rendered(&face, &mut library, &memory))),
        r#"{"type":"pass"}"#
    );
    assert_eq!(
        told(&Value::Object(
            face.borrow_mut()
                .render(&mut library, &memory, true)
                .unwrap()
        )),
        r#"{"type":"update","gen":2,"exit":true}"#
    );
}

// A canvas's operations travel in call order, its pending clear
// settled ahead of them as the colorless whole-window fill -- and
// ahead of a background change, since a clear wears the old color.
#[test]
fn a_canvas_speaks_its_draws_in_order() {
    let (mut library, face, canvas, memory) = sessioned();
    let canvas = {
        let root = *library.window_order.last().unwrap();
        let _ = canvas;

        plain(library.glk_window_open(Some(root), ABOVE_FIXED, 8, window_type::GRAPHICS, 0))
            .unwrap()
            .expect("the canvas opened")
    };
    let mut front = handle(&face);

    front.set_background_color(&mut library.windows, canvas, 0x123456);
    front.fill_rect(&mut library.windows, canvas, 0xAB12_CD34, 1, 2, 3, 4);
    front.erase_rect(&mut library.windows, canvas, 5, 6, 7, 8);

    let update = rendered(&face, &mut library, &memory);
    let drawn = items(at(&update, "content"))
        .iter()
        .map(entry)
        .find(|held| held.get("draw").is_some())
        .expect("the canvas entry");

    assert_eq!(
        told(at(drawn, "draw")),
        r##"[{"special":"fill"},{"special":"setcolor","color":"#123456"},{"special":"fill","color":"#12CD34","x":1,"y":2,"width":3,"height":4},{"special":"fill","x":5,"y":6,"width":7,"height":8}]"##
    );
    assert!(!library.windows[&canvas].pending_clear);
}

// A canvas cleared and then left alone still owes the display its
// fill, at the open and at every clear after.
#[test]
fn a_cleared_canvas_still_fills() {
    let (mut library, face, canvas, memory) = canvased();
    let first = rendered(&face, &mut library, &memory);
    let drawn = items(at(&first, "content"))
        .iter()
        .map(entry)
        .find(|held| held.get("draw").is_some())
        .expect("the canvas entry");

    assert_eq!(told(at(drawn, "draw")), r#"[{"special":"fill"}]"#);

    library.glk_window_clear(Some(canvas));

    let again = rendered(&face, &mut library, &memory);

    assert_eq!(
        told(at(&again, "content")),
        r#"[{"id":2,"draw":[{"special":"fill"}]}]"#
    );
}

// A picture draws on a canvas by its Pict number with the picture
// itself inlined beside it as a data: url -- PNG and JPEG each
// under its own kind, the bytes riding whole. A buffer refuses
// without the display's bare-graphics grant, and a grid always.
#[test]
fn images_draw_on_canvases_alone() {
    let (mut library, face, canvas, memory) = canvased();
    let root = *library.window_order.last().unwrap();
    let grid =
        plain(library.glk_window_open(Some(root), ABOVE_FIXED, 1, window_type::TEXT_GRID, 0))
            .unwrap()
            .expect("the grid opened");
    let picture = ImageInfo {
        number: 5,
        kind: *b"PNG ",
        data: b"\x89PNG\r\n".to_vec(),
        width: 2,
        height: 2,
    };
    let mut front = handle(&face);

    assert!(front.draw_image(&mut library.windows, canvas, &picture, 3, 4, 10, 20, 0));
    assert!(!front.draw_image(&mut library.windows, root, &picture, 0, 0, 1, 1, 0));
    assert!(!front.draw_image(&mut library.windows, grid, &picture, 0, 0, 1, 1, 0));

    let update = rendered(&face, &mut library, &memory);
    let drawn = items(at(
        items(at(&update, "content"))
            .iter()
            .map(entry)
            .find(|held| held.get("draw").is_some())
            .expect("the canvas entry"),
        "draw",
    ))
    .to_vec();
    let image = entry(drawn.last().expect("the image op"));

    assert_eq!(int_of(at(image, "image")), 5);
    assert_eq!((int_of(at(image, "x")), int_of(at(image, "y"))), (3, 4));
    assert_eq!(
        (int_of(at(image, "width")), int_of(at(image, "height"))),
        (10, 20)
    );
    assert_eq!(
        str_of(at(image, "url")),
        format!("data:image/png;base64,{}", b64(b"\x89PNG\r\n"))
    );

    let photograph = ImageInfo {
        number: 6,
        kind: *b"JPEG",
        data: b"\xff\xd8\xff".to_vec(),
        width: 2,
        height: 2,
    };

    assert!(front.draw_image(&mut library.windows, canvas, &photograph, 0, 0, 2, 2, 0));

    let jpeg = rendered(&face, &mut library, &memory);
    let last = items(at(
        items(at(&jpeg, "content"))
            .iter()
            .map(entry)
            .find(|held| held.get("draw").is_some())
            .expect("the canvas entry"),
        "draw",
    ))
    .to_vec();

    assert!(
        str_of(at(entry(last.last().expect("the image op")), "url"))
            .starts_with("data:image/jpeg;base64,")
    );
}

/// A session whose display grants bare graphics: buffer images.
fn flowing() -> (Glk, Rc<RefCell<GlkOteFrontend>>, u32, Memory) {
    let face = begun(
        r#""timer","graphics","graphicswin","hyperlinks""#,
        plain_metrics(),
    );
    let (library, window) = rooted(Box::new(handle(&face)));

    (library, face, window, scratch_memory())
}

// Under the display's bare-graphics grant, a picture draws into
// the buffer's flow: the placed image rides the line data between
// the text spans -- its alignment named, the picture whole as a
// data: url -- text after it starts a fresh span, and a flow break
// moves the next paragraph below the margins.
#[test]
fn pictures_set_into_the_buffers_flow() {
    let (mut library, face, window, memory) = flowing();
    let picture = ImageInfo {
        number: 3,
        kind: *b"PNG ",
        data: b"\x89PNG".to_vec(),
        width: 4,
        height: 5,
    };
    let mut front = handle(&face);

    saying(&mut library, window, "Once", style::NORMAL);

    assert!(front.draw_image(&mut library.windows, window, &picture, 1, 0, 4, 5, 0));

    saying(&mut library, window, " upon a time.\n", style::NORMAL);
    front.flow_break(&mut library.windows, window);
    saying(&mut library, window, "Below.\n", style::NORMAL);

    let update = rendered(&face, &mut library, &memory);
    let text = items(at(
        items(at(&update, "content"))
            .iter()
            .map(entry)
            .find(|held| held.get("text").is_some())
            .expect("the buffer entry"),
        "text",
    ))
    .to_vec();

    assert_eq!(
        told(at(entry(&text[0]), "content")),
        format!(
            r#"[{{"style":"normal","text":"Once"}},{{"special":"image","image":3,"url":"data:image/png;base64,{}","width":4,"height":5,"alignment":"inlineup"}},{{"style":"normal","text":" upon a time."}}]"#,
            b64(b"\x89PNG")
        )
    );
    assert_eq!(at(entry(&text[1]), "flowbreak"), &Value::Bool(true));
    assert_eq!(
        told(at(entry(&text[1]), "content")),
        r#"[{"style":"normal","text":"Below."}]"#
    );
}

// The margin alignments wear their names, an alignment the library
// does not recognize draws inlineup as the spec instructs, and a
// picture drawn under a link value carries it -- clickable art
// stays clickable. A flow break in any other window is shrugged
// off, as the spec allows. (The link rides the draw call itself
// here: the api reads it off the window's stream and passes it
// through, the borrow's way to the same value.)
#[test]
fn the_alignments_and_links_ride_along() {
    let (mut library, face, window, memory) = flowing();
    let grid =
        plain(library.glk_window_open(Some(window), ABOVE_FIXED, 1, window_type::TEXT_GRID, 0))
            .unwrap()
            .expect("the grid opened");
    let picture = ImageInfo {
        number: 3,
        kind: *b"PNG ",
        data: b"\x89PNG".to_vec(),
        width: 4,
        height: 5,
    };
    let mut front = handle(&face);

    assert!(front.draw_image(&mut library.windows, window, &picture, 4, 0, 4, 5, 7));
    assert!(front.draw_image(&mut library.windows, window, &picture, 99, 0, 4, 5, 0));

    front.flow_break(&mut library.windows, grid);

    let update = rendered(&face, &mut library, &memory);
    let spans = items(at(
        entry(
            &items(at(
                items(at(&update, "content"))
                    .iter()
                    .map(entry)
                    .find(|held| held.get("text").is_some())
                    .expect("the buffer entry"),
                "text",
            ))[0],
        ),
        "content",
    ))
    .to_vec();

    assert_eq!(str_of(at(entry(&spans[0]), "alignment")), "marginleft");
    assert_eq!(int_of(at(entry(&spans[0]), "hyperlink")), 7);
    assert_eq!(str_of(at(entry(&spans[1]), "alignment")), "inlineup");
    assert!(entry(&spans[1]).get("hyperlink").is_none());
}

/// Resources with one 2x3 PNG as picture 8, named the cover.
fn fronted_resources(record: Option<&[u8]>) -> Resources {
    let mut art = b"\x89PNG\r\n\x1a\n".to_vec();

    art.extend(13u32.to_be_bytes());
    art.extend(b"IHDR");
    art.extend(2u32.to_be_bytes());
    art.extend(3u32.to_be_bytes());

    let fspc = chunk(b"Fspc", &8u32.to_be_bytes());
    let ifmd = record.map_or_else(Vec::new, |held| chunk(b"IFmd", held));
    let offset = 12 + (8 + 4 + 12) + fspc.len() + ifmd.len();
    let mut index = 1u32.to_be_bytes().to_vec();

    index.extend(b"Pict");
    index.extend(8u32.to_be_bytes());
    index.extend((offset as u32).to_be_bytes());

    let mut body = b"IFRS".to_vec();

    body.extend(chunk(b"RIdx", &index));
    body.extend(&fspc);
    body.extend(&ifmd);
    body.extend(chunk(b"PNG ", &art));

    Resources::new(Some(Blorb::parse(&chunk(b"FORM", &body)).unwrap()))
}

// The record's card needs no grant: it shows even where the
// display never granted graphics -- the cover stays home, the
// bibliography stands -- with the missing fields simply absent and
// the game's own text following after.
#[test]
fn the_card_stands_at_the_door() {
    let record: &[u8] = b"<ifindex><story><bibliographic><title>Tiny Case</title>\
        <author>A. Tester</author></bibliographic></story></ifindex>";
    let face = begun(r#""timer","graphicswin","hyperlinks""#, plain_metrics());
    let mut library = Glk::new(Box::new(handle(&face)));

    library.resources = fronted_resources(Some(record));

    let window = plain(library.glk_window_open(None, 0, 0, window_type::TEXT_BUFFER, 0))
        .unwrap()
        .expect("the root window opened");
    let memory = scratch_memory();

    saying(&mut library, window, "Banner", style::NORMAL);

    let update = rendered(&face, &mut library, &memory);
    let text = items(at(
        items(at(&update, "content"))
            .iter()
            .map(entry)
            .find(|held| held.get("text").is_some())
            .expect("the buffer entry"),
        "text",
    ))
    .to_vec();

    assert_eq!(
        told(at(entry(&text[0]), "content")),
        r#"[{"style":"header","text":"Tiny Case"}]"#
    );
    assert_eq!(
        told(at(entry(&text[1]), "content")),
        r#"[{"style":"emphasized","text":"A. Tester"}]"#
    );
    assert_eq!(told(&text[2]), r"{}");
    assert_eq!(
        told(at(entry(&text[3]), "content")),
        r#"[{"style":"normal","text":"Banner"}]"#
    );
}

// The gblorb's Fspc cover stands at the head of the first buffer
// window -- once, above whatever the game already wrote, waiting
// through renders until the tree grows a buffer -- and only under
// the display's bare-graphics grant.
#[test]
fn the_cover_stands_at_the_door() {
    let face = begun(
        r#""timer","graphics","graphicswin","hyperlinks""#,
        plain_metrics(),
    );
    let mut library = Glk::new(Box::new(handle(&face)));

    library.resources = fronted_resources(None);

    let grid = plain(library.glk_window_open(None, 0, 0, window_type::TEXT_GRID, 0))
        .unwrap()
        .expect("the grid opened");
    let memory = scratch_memory();
    let early = rendered(&face, &mut library, &memory);

    assert!(early.get("content").is_none());

    let window =
        plain(library.glk_window_open(Some(grid), ABOVE_FIXED, 2, window_type::TEXT_BUFFER, 0))
            .unwrap()
            .expect("the buffer opened");

    saying(&mut library, window, "Banner", style::NORMAL);

    let update = rendered(&face, &mut library, &memory);
    let text = items(at(
        items(at(&update, "content"))
            .iter()
            .map(entry)
            .find(|held| held.get("text").is_some())
            .expect("the buffer entry"),
        "text",
    ))
    .to_vec();
    let cover = entry(&items(at(entry(&text[0]), "content"))[0]);

    assert_eq!(str_of(at(cover, "special")), "image");
    assert_eq!(int_of(at(cover, "image")), 8);
    assert_eq!(
        (int_of(at(cover, "width")), int_of(at(cover, "height"))),
        (2, 3)
    );
    assert_eq!(str_of(at(cover, "alignment")), "inlineup");
    assert_eq!(
        told(at(entry(&text[1]), "content")),
        r#"[{"style":"normal","text":"Banner"}]"#
    );

    saying(&mut library, window, "More", style::NORMAL);

    let again = rendered(&face, &mut library, &memory);
    let retold = items(at(
        items(at(&again, "content"))
            .iter()
            .map(entry)
            .find(|held| held.get("text").is_some())
            .expect("the buffer entry"),
        "text",
    ))
    .to_vec();

    assert_eq!(
        told(at(entry(&retold[0]), "content")),
        r#"[{"style":"normal","text":"More"}]"#
    );
}

/// Resources with a tiny AIFF as sound 3 and a MOD as sound 5.
fn sounding_resources() -> Resources {
    let mut comm = 1i16.to_be_bytes().to_vec();

    comm.extend(2u32.to_be_bytes());
    comm.extend(8i16.to_be_bytes());
    comm.extend(16397u16.to_be_bytes());
    comm.extend((1u64 << 63).to_be_bytes());

    let mut ssnd = vec![0u8; 8];

    ssnd.extend(b"\x01\xfe");

    let mut aiff = b"AIFF".to_vec();

    aiff.extend(chunk(b"COMM", &comm));
    aiff.extend(chunk(b"SSND", &ssnd));

    let aiff_form = chunk(b"FORM", &aiff);
    let mod_music = chunk(b"MOD ", &[0]);
    let first = 12 + (8 + 4 + 24);
    let mut index = 2u32.to_be_bytes().to_vec();

    index.extend(b"Snd ");
    index.extend(3u32.to_be_bytes());
    index.extend((first as u32).to_be_bytes());
    index.extend(b"Snd ");
    index.extend(5u32.to_be_bytes());
    index.extend(((first + aiff_form.len()) as u32).to_be_bytes());

    let mut body = b"IFRS".to_vec();

    body.extend(chunk(b"RIdx", &index));
    body.extend(&aiff_form);
    body.extend(&mod_music);

    Resources::new(Some(Blorb::parse(&chunk(b"FORM", &body)).unwrap()))
}

/// A session whose display says the sound word, sounds aboard.
fn sounding() -> (Glk, Rc<RefCell<GlkOteFrontend>>, Memory) {
    let face = begun(r#""timer","sound""#, plain_metrics());
    let mut library = Glk::new(Box::new(handle(&face)));

    library.resources = sounding_resources();
    plain(library.glk_window_open(None, 0, 0, window_type::TEXT_BUFFER, 0)).unwrap();

    (library, face, scratch_memory())
}

// Under the display's own sound word the channels speak the wire
// dialect: a play op carries the sound whole as a WAVE data: url
// with repeats, notify, and the channel's unit gain; volume fades
// and stops follow on the same minted channel ident; forever
// spells -1; and a MOD no wire container carries refuses, exactly
// as the music gestalt said it would.
#[test]
fn sound_channels_speak_the_dialect() {
    let (mut library, face, memory) = sounding();
    let channel = library.glk_schannel_create(0).expect("the channel opened");

    assert_eq!(library.glk_schannel_play_ext(Some(channel), 3, 2, 7), 1);

    library.glk_schannel_set_volume_ext(Some(channel), 0x8000, 500, 0);
    library.glk_schannel_stop(Some(channel));

    assert_eq!(library.glk_schannel_play_ext(Some(channel), 5, 1, 0), 0);

    let update = rendered(&face, &mut library, &memory);
    let ops: Vec<&Object> = items(at(&update, "sounds")).iter().map(entry).collect();

    assert_eq!(ops.len(), 3);
    assert_eq!(str_of(at(ops[0], "op")), "play");
    assert_eq!(int_of(at(ops[0], "channel")), 1);
    assert_eq!(int_of(at(ops[0], "sound")), 3);
    assert_eq!(int_of(at(ops[0], "repeats")), 2);
    assert_eq!(int_of(at(ops[0], "notify")), 7);
    assert_eq!(told(at(ops[0], "volume")), "1.0");
    assert!(str_of(at(ops[0], "url")).starts_with("data:audio/wav;base64,"));
    assert_eq!(
        told(&Value::Object(ops[1].clone())),
        r#"{"channel":1,"op":"volume","volume":0.5,"duration":500}"#
    );
    assert_eq!(
        told(&Value::Object(ops[2].clone())),
        r#"{"channel":1,"op":"stop"}"#
    );
    assert!(
        rendered(&face, &mut library, &memory)
            .get("sounds")
            .is_none()
    );
}

// A pause is silence and an unpause starts the sound over -- the
// painted spine's own semantics -- while a channel with nothing
// sounding shrugs both off; a finished play comes home as the
// SoundNotify completion, falling silent in the model, and a play
// that never asked for notification only falls silent.
#[test]
fn sound_completions_come_home() {
    let (mut library, face, mut memory) = sounding();
    let channel = library.glk_schannel_create(0).expect("the channel opened");
    let idle = library.glk_schannel_create(0).expect("the idle channel");

    assert_eq!(
        library.glk_schannel_play_ext(Some(channel), 3, 0xFFFF_FFFF, 9),
        1
    );

    library.glk_schannel_pause(Some(channel));
    library.glk_schannel_unpause(Some(channel));
    library.glk_schannel_pause(Some(idle));
    library.glk_schannel_unpause(Some(idle));

    let update = rendered(&face, &mut library, &memory);
    let ops: Vec<&Object> = items(at(&update, "sounds")).iter().map(entry).collect();

    assert_eq!(
        ops.iter()
            .map(|held| held.get("op").and_then(Value::as_str).unwrap_or(""))
            .collect::<Vec<_>>(),
        ["play", "stop", "play"]
    );
    assert_eq!(int_of(at(ops[2], "repeats")), -1);

    let generation = face.borrow().page.generation();
    let landed = event_of(accepted(
        &face,
        &mut library,
        &mut memory,
        &format!(r#"{{"type":"sound","gen":{generation},"channel":1,"sound":3,"notify":9}}"#),
    ));

    assert_eq!(
        (landed.kind, landed.window, landed.val1, landed.val2),
        (event_type::SOUND_NOTIFY, None, 3, 9)
    );
    assert_eq!(library.channels[&channel].sound, 0);

    assert_eq!(library.glk_schannel_play_ext(Some(channel), 3, 1, 0), 1);

    rendered(&face, &mut library, &memory);

    let generation = face.borrow().page.generation();

    assert_eq!(
        accepted(
            &face,
            &mut library,
            &mut memory,
            &format!(r#"{{"type":"sound","gen":{generation},"channel":1,"sound":3,"notify":0}}"#),
        ),
        Accepted::Nothing
    );
    assert_eq!(library.channels[&channel].sound, 0);
    assert_eq!(
        accepted(
            &face,
            &mut library,
            &mut memory,
            &format!(r#"{{"type":"sound","gen":{generation},"channel":99,"sound":3,"notify":0}}"#),
        ),
        Accepted::Nothing
    );
}

// Draws for a window that closed before the update vanish rather
// than crash: there is nothing to show and nowhere to show it.
#[test]
fn draws_for_a_closed_window_vanish() {
    let (mut library, face, canvas, memory) = canvased();

    handle(&face).fill_rect(&mut library.windows, canvas, 0xFF_FFFF, 0, 0, 1, 1);
    plain(library.glk_window_close(Some(canvas), None)).unwrap();

    let update = rendered(&face, &mut library, &memory);

    assert!(update.get("content").is_none());
    assert_eq!(items(at(&update, "windows")).len(), 1);
    assert_eq!(
        str_of(at(entry(&items(at(&update, "windows"))[0]), "type")),
        "buffer"
    );
}

// A timer re-request restarts the display's clock even at the same
// cadence: the restart rides through render, where polled state
// alone would stay silent.
#[test]
fn the_timer_restart_rides_through_render() {
    let (mut library, face, _, memory) = sessioned();

    library.glk_request_timer_events(100);

    assert_eq!(
        int_of(at(&rendered(&face, &mut library, &memory), "timer")),
        100
    );

    library.glk_request_timer_events(100);

    assert_eq!(
        int_of(at(&rendered(&face, &mut library, &memory), "timer")),
        100
    );
    assert_eq!(
        told(&Value::Object(rendered(&face, &mut library, &memory))),
        r#"{"type":"pass"}"#
    );
}

// A line event lands in its request, the echo goes back out in the
// input style, and the re-asked field wears the new generation; a
// named terminator translates, an unnamed one is a plain Return.
#[test]
fn a_line_event_lands_and_echoes() {
    let (mut library, face, window, mut memory) = sessioned();
    let buf = MemArray {
        address: TEXT_BUFFER_AT,
        count: 8,
        width: 1,
    };

    plain(library.glk_request_line_event(Some(window), Some(buf), 0)).unwrap();
    rendered(&face, &mut library, &memory);

    let event = event_of(accepted(
        &face,
        &mut library,
        &mut memory,
        r#"{"type":"line","gen":1,"window":1,"value":"go"}"#,
    ));

    assert_eq!(
        (event.kind, event.window, event.val1, event.val2),
        (event_type::LINE_INPUT, Some(window), 2, 0)
    );

    plain(library.glk_request_line_event(Some(window), Some(buf), 0)).unwrap();

    let update = rendered(&face, &mut library, &memory);

    assert_eq!(
        told(
            &items(at(
                entry(&items(at(entry(&items(at(&update, "content"))[0]), "text"))[0]),
                "content"
            ))[0]
        ),
        r#"{"style":"input","text":"go"}"#
    );
    assert_eq!(int_of(at(entry(&items(at(&update, "input"))[0]), "gen")), 2);

    let keys = MemArray {
        address: KEYCODES_AT,
        count: 1,
        width: 4,
    };

    keys.set(&mut memory, 0, key_code::ESCAPE).unwrap();
    plain(library.glk_set_terminators_line_event(&memory, Some(window), Some(keys))).unwrap();
    rendered(&face, &mut library, &memory);

    let ended = event_of(accepted(
        &face,
        &mut library,
        &mut memory,
        r#"{"type":"line","gen":3,"window":1,"value":"x","terminator":"escape"}"#,
    ));

    assert_eq!(ended.val2, key_code::ESCAPE);
}

/// One char event through a fresh session; the code it lands as.
fn keyed_code(value: &str, unicode: bool) -> u32 {
    let (mut library, face, window, mut memory) = sessioned();

    if unicode {
        plain(library.glk_request_char_event_uni(Some(window))).unwrap();
    } else {
        plain(library.glk_request_char_event(Some(window))).unwrap();
    }

    rendered(&face, &mut library, &memory);

    let event = event_of(accepted(
        &face,
        &mut library,
        &mut memory,
        &format!(r#"{{"type":"char","gen":1,"window":1,"value":{value}}}"#),
    ));

    event.val1
}

// A char event's value is a literal character or a key's name; a
// character beyond Latin-1 lands as the unknown key when the
// request cannot carry it, and passes whole when it can.
#[test]
fn a_char_event_translates_its_key() {
    assert_eq!(keyed_code(r#""A""#, false), 65);
    assert_eq!(keyed_code(r#""left""#, false), key_code::LEFT);
    assert_eq!(keyed_code(r#""borogove""#, false), key_code::UNKNOWN);
    assert_eq!(keyed_code("5", false), key_code::UNKNOWN);
    assert_eq!(keyed_code(r#""λ""#, false), key_code::UNKNOWN);
    assert_eq!(keyed_code(r#""λ""#, true), 0x3BB);
}

// Clicks and link selections route to their windows' requests.
#[test]
fn clicks_and_links_arrive() {
    let (mut library, face, root, mut memory) = sessioned();
    let grid =
        plain(library.glk_window_open(Some(root), ABOVE_FIXED, 2, window_type::TEXT_GRID, 0))
            .unwrap()
            .expect("the grid opened");

    library.glk_request_mouse_event(Some(grid));
    library.glk_request_hyperlink_event(Some(root));
    rendered(&face, &mut library, &memory);

    let clicked = event_of(accepted(
        &face,
        &mut library,
        &mut memory,
        r#"{"type":"mouse","gen":1,"window":2,"x":3,"y":0}"#,
    ));

    assert_eq!(
        (clicked.kind, clicked.window, clicked.val1, clicked.val2),
        (event_type::MOUSE_INPUT, Some(grid), 3, 0)
    );

    let linked = event_of(accepted(
        &face,
        &mut library,
        &mut memory,
        r#"{"type":"hyperlink","gen":1,"window":1,"value":7}"#,
    ));

    assert_eq!(
        (linked.kind, linked.window, linked.val1, linked.val2),
        (event_type::HYPERLINK, Some(root), 7, 0)
    );
}

// A timer event carries no window at all; a redraw names its
// canvas, or names none to mean every canvas, Glk's null window.
#[test]
fn timers_and_redraws_arrive() {
    let (mut library, face, canvas, mut memory) = canvased();

    rendered(&face, &mut library, &memory);

    let ticked = event_of(accepted(
        &face,
        &mut library,
        &mut memory,
        r#"{"type":"timer","gen":1}"#,
    ));

    assert_eq!(ticked.kind, event_type::TIMER);

    let named = event_of(accepted(
        &face,
        &mut library,
        &mut memory,
        r#"{"type":"redraw","gen":1,"window":2}"#,
    ));

    assert_eq!(
        (named.kind, named.window, named.val1, named.val2),
        (event_type::REDRAW, Some(canvas), 0, 0)
    );

    let blanket = event_of(accepted(
        &face,
        &mut library,
        &mut memory,
        r#"{"type":"redraw","gen":1}"#,
    ));

    assert_eq!(blanket.window, None);
}

// An arrange remeasures the display, re-lays the tree, and answers
// with the arrange event -- taken from the end of the queue, so a
// moved canvas's redraw stays queued ahead for the next selects.
#[test]
fn an_arrange_relays_and_remeasures() {
    let (mut library, face, canvas, mut memory) = canvased();

    rendered(&face, &mut library, &memory);

    let event = event_of(accepted(
        &face,
        &mut library,
        &mut memory,
        r#"{"type":"arrange","gen":1,"metrics":{"width":100,"height":30}}"#,
    ));

    assert_eq!(event.kind, event_type::ARRANGE);
    assert_eq!(face.borrow().size().unwrap(), (100, 30));
    assert_eq!(library.windows[&canvas].bbox, (0, 0, 100, 8));
    assert_eq!(
        library
            .pending_events
            .iter()
            .map(|held| held.kind)
            .collect::<Vec<_>>(),
        [event_type::REDRAW]
    );
}

// A display that lost its picture asks with refresh -- accepted
// ahead of the generation gate, since a lost display is out of
// sync by definition -- and hears the spec's own redraw for its
// canvases, while the next update replays everything kept: the
// windows whole, the buffer's scrollback behind a clear, the input
// field stamped anew, and whatever the redraw repainted.
#[test]
fn a_refresh_earns_the_whole_picture() {
    let (mut library, face, canvas, mut memory) = canvased();
    let window = *library
        .window_order
        .iter()
        .find(|key| matches!(library.windows[key].kind, WindowKind::Buffer(_)))
        .expect("the buffer");

    saying(&mut library, window, "Once upon a time.\n", style::NORMAL);
    plain(library.glk_request_line_event(Some(window), None, 0)).unwrap();
    rendered(&face, &mut library, &memory);

    let event = event_of(accepted(
        &face,
        &mut library,
        &mut memory,
        r#"{"type":"refresh","gen":77}"#,
    ));

    assert_eq!((event.kind, event.window), (event_type::REDRAW, None));

    handle(&face).fill_rect(&mut library.windows, canvas, 0xFF_FFFF, 0, 0, 1, 1);

    let whole = rendered(&face, &mut library, &memory);

    assert_eq!(
        items(at(&whole, "windows"))
            .iter()
            .map(|held| str_of(at(entry(held), "type")).to_string())
            .collect::<Vec<_>>(),
        ["buffer", "graphics"]
    );

    let texted = items(at(&whole, "content"))
        .iter()
        .map(entry)
        .find(|held| held.get("clear").is_some())
        .expect("the buffer replay");

    assert_eq!(
        str_of(at(
            entry(&items(at(entry(&items(at(texted, "text"))[0]), "content"))[0]),
            "text"
        )),
        "Once upon a time."
    );

    let drawn = items(at(
        items(at(&whole, "content"))
            .iter()
            .map(entry)
            .find(|held| held.get("draw").is_some())
            .expect("the repaint"),
        "draw",
    ))
    .to_vec();

    assert_eq!(
        str_of(at(entry(drawn.last().expect("the fill")), "special")),
        "fill"
    );
    assert_eq!(
        at(entry(&items(at(&whole, "input"))[0]), "gen"),
        at(&whole, "gen")
    );

    // A canvas nothing repainted contributes nothing: its pixels
    // were the game's to redraw, and the game declined.
    accepted(
        &face,
        &mut library,
        &mut memory,
        r#"{"type":"refresh","gen":88}"#,
    );

    let again = rendered(&face, &mut library, &memory);
    let repainted = again.get("content").map(items).map_or(0, |held| {
        held.iter()
            .map(entry)
            .filter(|held| held.get("draw").is_some())
            .count()
    });

    assert_eq!(repainted, 0);
}

// A stale generation and the kinds this face does not carry mean
// nothing here, quietly.
#[test]
fn stale_and_foreign_stanzas_mean_nothing() {
    let (mut library, face, _, mut memory) = sessioned();

    rendered(&face, &mut library, &memory);

    assert_eq!(
        accepted(
            &face,
            &mut library,
            &mut memory,
            r#"{"type":"char","gen":0,"window":1}"#
        ),
        Accepted::Nothing
    );
    assert_eq!(
        accepted(
            &face,
            &mut library,
            &mut memory,
            r#"{"type":"external","gen":1,"value":9}"#
        ),
        Accepted::Nothing
    );
    assert_eq!(
        accepted(
            &face,
            &mut library,
            &mut memory,
            r#"{"type":"debuginput","gen":1}"#
        ),
        Accepted::Nothing
    );
}

// An event for a window this session never showed is loud, and so
// is one that names no window at all.
#[test]
fn an_unknown_window_is_loud() {
    let (mut library, face, window, mut memory) = sessioned();

    plain(library.glk_request_char_event(Some(window))).unwrap();
    rendered(&face, &mut library, &memory);

    let loud = face
        .borrow_mut()
        .accept(
            &mut library,
            &mut memory,
            &parsed(r#"{"type":"char","gen":1,"window":99,"value":"A"}"#),
        )
        .unwrap_err();

    assert!(loud.to_string().contains("no window is numbered 99"));

    let unnamed = face
        .borrow_mut()
        .accept(
            &mut library,
            &mut memory,
            &parsed(r#"{"type":"mouse","gen":1,"x":1,"y":1}"#),
        )
        .unwrap_err();

    assert!(unnamed.to_string().contains("no window is numbered None"));
}

// -- the serve loop ----------------------------------------------------------

// The suspension story from the machine tests: open a buffer, ask
// for a keystroke, select, quit on the far side of the resume.
const AWAITS_KEY: &[u8] = &[
    0xC0, 0x00, 0x00, // the start function
    0x40, 0x81, 0x00, // copy 0 -> sp padding
    0x40, 0x81, 0x03, //
    0x40, 0x81, 0x00, //
    0x40, 0x81, 0x00, //
    0x40, 0x81, 0x00, //
    0x81, 0x30, 0x11, 0x06, 0x23, 0x05, 0x01, 0x40, // glk window_open
    0x40, 0x86, 0x01, 0x40, // copy the window
    0x81, 0x30, 0x12, 0x00, 0x00, 0xD2, 0x01, // glk request_char_event
    0x40, 0x82, 0x01, 0xC0, // copy the event seat
    0x81, 0x30, 0x12, 0x00, 0x00, 0xC0, 0x01, // glk select
    0x81, 0x20, // quit
];

// A story that asks the player for a save file and quits.
const PROMPTS: &[u8] = &[
    0xC0, 0x00, 0x00, //
    0x40, 0x81, 0x00, //
    0x40, 0x81, 0x01, //
    0x40, 0x81, 0x01, //
    0x81, 0x30, 0x11, 0x06, 0x62, 0x03, 0x01, 0x40, // glk fileref_create_by_prompt
    0x81, 0x20, // quit
];

fn init_line() -> String {
    r#"{"type":"init","gen":0,"support":["timer","graphicswin","hyperlinks"],"metrics":{"width":80,"height":24}}"#
        .to_string()
}

/// One whole conversation over byte pipes.
fn served_lines(lines: &[String], code: &[u8]) -> (bool, Vec<Object>, Machine) {
    let story = crate::glulx::story::Story::new(testing::image(code)).unwrap();
    let (mut machine, face) = opened(story, None, None).unwrap();
    let joined: String = lines.iter().map(|line| format!("{line}\n")).collect();
    let mut reader = std::io::Cursor::new(joined.into_bytes());
    let mut writer: Vec<u8> = Vec::new();
    let clean = serve(&mut machine, &face, &mut reader, &mut writer);
    let spoken = String::from_utf8(writer)
        .unwrap()
        .lines()
        .map(parsed)
        .collect();

    (clean, spoken, machine)
}

// The whole conversation: init, the first update with its window
// and its field, one keystroke, and the exit update -- blank lines
// on the wire skipped along the way.
#[test]
fn a_session_serves_end_to_end() {
    let (clean, stanzas, machine) = served_lines(
        &[
            init_line(),
            String::new(),
            r#"{"type":"char","gen":1,"window":1,"value":"A"}"#.to_string(),
        ],
        AWAITS_KEY,
    );

    assert!(clean);
    assert!(!machine.running());
    assert_eq!(
        told(&Value::Object(stanzas[0].clone())),
        r#"{"type":"update","gen":1,"windows":[{"id":1,"type":"buffer","rock":0,"left":0,"top":0,"width":80,"height":24}],"input":[{"id":1,"type":"char","gen":1}]}"#
    );
    assert_eq!(
        told(&Value::Object(stanzas.last().unwrap().clone())),
        r#"{"type":"update","gen":2,"input":[],"exit":true}"#
    );
}

// The conversation opens with an init event, or not at all.
#[test]
fn the_conversation_opens_with_init() {
    let (clean, stanzas, _) = served_lines(
        &[r#"{"type":"char","gen":1,"window":1,"value":"A"}"#.to_string()],
        AWAITS_KEY,
    );

    assert!(!clean);
    assert_eq!(str_of(at(&stanzas[0], "type")), "error");
    assert!(str_of(at(&stanzas[0], "message")).contains("opens with an init"));
}

// A display that hangs up ends the session cleanly, mid-wait.
#[test]
fn a_hangup_ends_cleanly() {
    let (clean, stanzas, machine) = served_lines(&[init_line()], AWAITS_KEY);

    assert!(clean);
    assert!(machine.running());
    assert_eq!(stanzas.len(), 1);
}

// What is not JSON, and JSON that is not a stanza, are answered in
// kind: the protocol's own error stanza.
#[test]
fn garbage_is_answered_in_kind() {
    let (clean, stanzas, _) = served_lines(&[init_line(), "{nope".to_string()], AWAITS_KEY);

    assert!(!clean);
    assert_eq!(str_of(at(stanzas.last().unwrap(), "type")), "error");
    assert!(str_of(at(stanzas.last().unwrap(), "message")).contains("not JSON"));

    let (listed, shaped, _) = served_lines(&[init_line(), "[1, 2]".to_string()], AWAITS_KEY);

    assert!(!listed);
    assert!(str_of(at(shaped.last().unwrap(), "message")).contains("a stanza is a JSON object"));
}

// A stanza that asks for nothing is answered with the pass -- a
// lockstep display is owed a response for every event it sends.
#[test]
fn a_stale_event_draws_a_pass() {
    let (clean, stanzas, machine) = served_lines(
        &[
            init_line(),
            r#"{"type":"char","gen":0,"window":1,"value":"A"}"#.to_string(),
            r#"{"type":"char","gen":1,"window":1,"value":"A"}"#.to_string(),
        ],
        AWAITS_KEY,
    );

    assert!(clean);
    assert!(!machine.running());
    assert_eq!(
        stanzas
            .iter()
            .map(|held| str_of(at(held, "type")).to_string())
            .collect::<Vec<_>>(),
        ["update", "pass", "update"]
    );
}

// While a file prompt stands, render dresses it in the protocol's
// names -- the usage's text-mode bit stripped -- and a mode beyond
// the four is refused the way the file streams refuse it.
#[test]
fn a_prompt_renders_as_special_input() {
    let (mut library, face, _, memory) = sessioned();

    rendered(&face, &mut library, &memory);
    library.glk_fileref_create_by_prompt(file_usage::SAVED_GAME, file_mode::WRITE, 0);

    let update = rendered(&face, &mut library, &memory);

    assert_eq!(
        told(at(&update, "specialinput")),
        r#"{"type":"fileref_prompt","filemode":"write","filetype":"save"}"#
    );

    let (mut scripted, spoken, _, told_memory) = sessioned_named();

    rendered(&spoken, &mut scripted, &told_memory);
    scripted.glk_fileref_create_by_prompt(
        file_usage::TRANSCRIPT | file_usage::TEXT_MODE,
        file_mode::READ_WRITE,
        0,
    );

    assert_eq!(
        told(at(
            &rendered(&spoken, &mut scripted, &told_memory),
            "specialinput"
        )),
        r#"{"type":"fileref_prompt","filemode":"readwrite","filetype":"transcript"}"#
    );

    let (mut rogue, faced, _, rogue_memory) = sessioned_named();

    rendered(&faced, &mut rogue, &rogue_memory);

    rogue.waiting = Some(Waiting::Prompt {
        usage: file_usage::DATA,
        fmode: 7,
        rock: 0,
    });

    let refused = faced
        .borrow_mut()
        .render(&mut rogue, &rogue_memory, false)
        .unwrap_err();

    assert!(refused.to_string().contains("cannot be prompted"));
}

fn sessioned_named() -> (Glk, Rc<RefCell<GlkOteFrontend>>, u32, Memory) {
    sessioned()
}

// The player's file answer travels up as a verdict for the parked
// call's keeper -- the machine bridge; a stale one, an answer to
// some other ask, and a dialog's object all read as the protocol
// says. (The reference completes the call from inside; the parked
// half here is the bridge's, proven by the end-to-end drill.)
#[test]
fn a_file_answer_travels_up() {
    let (mut library, face, _, mut memory) = sessioned();

    rendered(&face, &mut library, &memory);

    library.waiting = Some(Waiting::Prompt {
        usage: file_usage::SAVED_GAME,
        fmode: file_mode::WRITE,
        rock: 0,
    });

    assert_eq!(
        accepted(
            &face,
            &mut library,
            &mut memory,
            r#"{"type":"specialresponse","gen":0,"response":"fileref_prompt"}"#
        ),
        Accepted::Nothing
    );
    assert!(library.waiting.is_some());
    assert_eq!(
        accepted(
            &face,
            &mut library,
            &mut memory,
            r#"{"type":"specialresponse","gen":1,"response":"other","value":"x"}"#
        ),
        Accepted::Nothing
    );
    assert!(library.waiting.is_some());
    assert_eq!(
        accepted(
            &face,
            &mut library,
            &mut memory,
            r#"{"type":"specialresponse","gen":1,"response":"fileref_prompt","value":"saga"}"#
        ),
        Accepted::File(Some("saga".to_string()))
    );

    // A dialog's fileref object was never invited: it cancels.
    assert_eq!(
        accepted(
            &face,
            &mut library,
            &mut memory,
            r#"{"type":"specialresponse","gen":1,"response":"fileref_prompt","value":{"dialog":true}}"#
        ),
        Accepted::File(None)
    );
}

// The player's half-typed line rides every event -- stale ones
// included, since the typing is current even when the event is not
// -- and a field made anew wears it; a special response leaves the
// picture alone, and what is not shaped like typing is quietly no
// typing at all.
#[test]
fn partial_input_survives_an_interruption() {
    let (mut library, face, window, mut memory) = sessioned();
    let buf = MemArray {
        address: TEXT_BUFFER_AT,
        count: 8,
        width: 1,
    };

    plain(library.glk_request_line_event(Some(window), Some(buf), 0)).unwrap();
    rendered(&face, &mut library, &memory);

    let ticked = accepted(
        &face,
        &mut library,
        &mut memory,
        r#"{"type":"timer","gen":1,"partial":{"1":"go nor"}}"#,
    );

    assert!(matches!(ticked, Accepted::Event(_)));

    saying(&mut library, window, "The clock strikes.\n", style::NORMAL);

    let update = rendered(&face, &mut library, &memory);

    assert_eq!(
        str_of(at(entry(&items(at(&update, "input"))[0]), "initial")),
        "go nor"
    );
    assert_eq!(int_of(at(entry(&items(at(&update, "input"))[0]), "gen")), 2);

    assert_eq!(
        accepted(
            &face,
            &mut library,
            &mut memory,
            r#"{"type":"timer","gen":0,"partial":{"1":"go north"}}"#
        ),
        Accepted::Nothing
    );

    accepted(
        &face,
        &mut library,
        &mut memory,
        r#"{"type":"specialresponse","gen":0,"response":"other"}"#,
    );
    saying(&mut library, window, "Again.\n", style::NORMAL);

    assert_eq!(
        str_of(at(
            entry(&items(at(&rendered(&face, &mut library, &memory), "input"))[0]),
            "initial"
        )),
        "go north"
    );

    accepted(
        &face,
        &mut library,
        &mut memory,
        r#"{"type":"timer","gen":3,"partial":{"one":"x","1":9}}"#,
    );
    saying(&mut library, window, "Still.\n", style::NORMAL);

    assert!(
        entry(&items(at(&rendered(&face, &mut library, &memory), "input"))[0])
            .get("initial")
            .is_none()
    );
}

// The file ask end to end: the update wears the special input, a
// stale answer draws the pass, and the real one resumes the story
// without any event delivered -- the call itself was the
// destination.
#[test]
fn a_file_ask_serves_end_to_end() {
    let (clean, stanzas, machine) = served_lines(
        &[
            init_line(),
            r#"{"type":"specialresponse","gen":0,"response":"fileref_prompt"}"#.to_string(),
            r#"{"type":"specialresponse","gen":1,"response":"fileref_prompt","value":"saga"}"#
                .to_string(),
        ],
        PROMPTS,
    );

    assert!(clean);
    assert!(!machine.running());
    assert_eq!(
        told(at(&stanzas[0], "specialinput")),
        r#"{"type":"fileref_prompt","filemode":"write","filetype":"save"}"#
    );
    assert_eq!(
        stanzas
            .iter()
            .map(|held| str_of(at(held, "type")).to_string())
            .collect::<Vec<_>>(),
        ["update", "pass", "update"]
    );

    // The prompted name minted a real fileref path under the save
    // directory; nothing was written, so nothing to clean up.
}

// Input delivered where no request stands is a driver's bug, and
// the conversation says so before it ends.
#[test]
fn a_wrongful_event_is_loud() {
    let (clean, stanzas, _) = served_lines(
        &[
            init_line(),
            r#"{"type":"line","gen":1,"window":1,"value":"go"}"#.to_string(),
        ],
        AWAITS_KEY,
    );

    assert!(!clean);
    assert_eq!(str_of(at(stanzas.last().unwrap(), "type")), "error");
}
