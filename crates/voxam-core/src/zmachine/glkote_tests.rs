//! The Z face of GlkOte: the screen model composed, reads
//! delivered. The stage half of the reference battery arrives
//! with the stage rung.

use super::*;
use crate::blorb::Blorb;
use crate::glkote::KEPT_PARAGRAPHS;
use crate::glkote::json::{dumps, loads};
use crate::iff::chunk;
use crate::screen::ROMAN;

const TEXT_BUFFER: usize = 0x120;
const PARSE_BUFFER: usize = 0x140;
const DICTIONARY_BASE: usize = 0x150;
const ROUTINE_BASE: usize = 0x70;
const SCREEN_LINES: usize = 0x20;

// aread text-buffer parse-buffer -> store; then quit.
const AREAD: &[u8] = &[0xE4, 0x0F, 0x01, 0x20, 0x01, 0x40, 0x10, 0xBA];

// sread with a §15 time and routine pair, Version 4.
const TIMED: &[u8] = &[0xE4, 0x05, 0x01, 0x20, 0x01, 0x40, 0x0A, 0x1C, 0xBA];

// read_char -> store; then quit.
const READ_CHAR: &[u8] = &[0xF6, 0x7F, 0x01, 0x10, 0xBA];

// Interrupt routines: mark a global then return false or true.
const MARK_THEN_FALSE: &[u8] = &[0x00, 0x0D, 0x11, 0x63, 0xB1];
const MARK_THEN_TRUE: &[u8] = &[0x00, 0x0D, 0x11, 0x63, 0xB0];

// sound_effect 3 start volume-word routine, then the aread: the
// routine operand is ROUTINE_BASE packed for Version 5.
const SOUNDED: &[u8] = &[
    0xF5, 0x51, 0x03, 0x02, 0x00, 0x08, 0x1C, 0xE4, 0x0F, 0x01, 0x20, 0x01, 0x40, 0x10, 0xBA,
];

// EXT save then EXT restore, each storing, then quit.
const SAVING: &[u8] = &[0xBE, 0x00, 0xFF, 0x10, 0xBE, 0x01, 0xFF, 0x11, 0xBA];

// EXT save alone, storing, then quit.
const SAVED: &[u8] = &[0xBE, 0x00, 0xFF, 0x10, 0xBA];

fn parsed(text: &str) -> Object {
    match loads(text).unwrap() {
        Value::Object(held) => held,
        _ => panic!("a stanza is an object"),
    }
}

fn init() -> Object {
    init_supporting(r#""timer""#)
}

fn init_supporting(words: &str) -> Object {
    parsed(&format!(
        r#"{{"type":"init","gen":0,"support":[{words}],"metrics":{{"width":800,"height":480,"gridcharwidth":10,"gridcharheight":20}}}}"#
    ))
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

/// A 512-byte story with code planted at $40, the reference test
/// suite's shape: globals at $100, static memory at $1C0.
fn image(code: &[u8], version: u8) -> Story {
    let mut data = vec![0u8; 512];

    data[0] = version;
    data[0x04..0x06].copy_from_slice(&0x01C0u16.to_be_bytes());
    data[0x06..0x08].copy_from_slice(&0x0040u16.to_be_bytes());
    data[0x0C..0x0E].copy_from_slice(&0x0100u16.to_be_bytes());
    data[0x0E..0x10].copy_from_slice(&0x01C0u16.to_be_bytes());
    data[0x40..0x40 + code.len()].copy_from_slice(code);

    Story::new(data).unwrap()
}

/// A begun session at its buffers: the dictionary planted, any
/// interrupt routine at $70.
fn session_with(
    program: &[u8],
    version: u8,
    routine: Option<&[u8]>,
    resources: Option<Resources>,
    opening: &Object,
) -> Session {
    let mut frontend = GlkOteFrontend::new(version, resources);

    frontend.begin(opening).unwrap();

    let mut session = Session::open(image(program, version), frontend, None).unwrap();
    let memory = session.machine().memory_mut();

    memory.write_byte(TEXT_BUFFER, 21).unwrap();
    memory.write_byte(PARSE_BUFFER, 5).unwrap();
    memory.write_word(0x08, DICTIONARY_BASE as u16).unwrap();

    for (offset, value) in [2u8, b',', b'.', 0, 0, 0].into_iter().enumerate() {
        memory.write_byte(DICTIONARY_BASE + offset, value).unwrap();
    }

    if let Some(routine) = routine {
        for (offset, value) in routine.iter().enumerate() {
            memory.write_byte(ROUTINE_BASE + offset, *value).unwrap();
        }
    }

    session
}

fn opened(program: &[u8], version: u8) -> Session {
    session_with(program, version, None, None, &init())
}

/// The machine's own handle on the face, for driving screen ops
/// the way the reference tests call the frontend directly.
fn front(session: &Session) -> SharedFace {
    SharedFace {
        face: session.face.clone(),
    }
}

/// Resources holding one 320x96 PNG as picture 8, maybe more.
fn banded_resources(fronted: bool, record: Option<&[u8]>) -> Resources {
    let mut art = b"\x89PNG\r\n\x1a\n".to_vec();

    art.extend(13u32.to_be_bytes());
    art.extend(b"IHDR");
    art.extend(320u32.to_be_bytes());
    art.extend(96u32.to_be_bytes());

    let fspc = if fronted {
        chunk(b"Fspc", &8u32.to_be_bytes())
    } else {
        Vec::new()
    };
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

/// Resources with a tiny AIFF as sound 3, maybe looping forever.
fn sounding_resources(looped: bool) -> Resources {
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
    let looping = if looped {
        let mut entry = 3u32.to_be_bytes().to_vec();

        entry.extend(0u32.to_be_bytes());

        chunk(b"Loop", &entry)
    } else {
        Vec::new()
    };
    let first = 12 + (8 + 4 + 12) + looping.len();
    let mut index = 1u32.to_be_bytes().to_vec();

    index.extend(b"Snd ");
    index.extend(3u32.to_be_bytes());
    index.extend((first as u32).to_be_bytes());

    let mut body = b"IFRS".to_vec();

    body.extend(chunk(b"RIdx", &index));
    body.extend(&looping);
    body.extend(&aiff_form);

    Resources::new(Some(Blorb::parse(&chunk(b"FORM", &body)).unwrap()))
}

/// A face granted the sound word, a machine at its buffers.
fn hearing(resources: Resources, program: &[u8], support: &str) -> Session {
    session_with(
        program,
        5,
        Some(MARK_THEN_FALSE),
        Some(resources),
        &init_supporting(support),
    )
}

// The init measures the screen in cells before any machine boots
// over the display; metrics with no size are refused. (The
// reference's "fronts no machine" refusal has no Rust spelling: a
// Session cannot exist without its machine.)
#[test]
fn the_init_measures_the_screen() {
    let mut frontend = GlkOteFrontend::new(5, None);

    frontend.begin(&init()).unwrap();

    assert_eq!(frontend.screen_columns, 80);
    assert_eq!(frontend.screen_lines, 24);
    assert!(frontend.has_timed_input);

    let mut quiet = GlkOteFrontend::new(5, None);

    quiet
        .begin(&parsed(
            r#"{"type":"init","gen":0,"metrics":{"width":80,"height":24}}"#,
        ))
        .unwrap();

    assert!(!quiet.has_timed_input);

    let refused = GlkOteFrontend::new(5, None)
        .begin(&parsed(r#"{"type":"init","gen":0,"metrics":{}}"#))
        .unwrap_err();

    assert!(refused.to_string().contains("carry no size"));
}

// Every §8.7 dress has its protocol name, reverse video ranking
// first -- the page's own CSS wears user1 as inverse.
#[test]
fn styles_wear_their_names() {
    assert_eq!(named(ROMAN), "normal");
    assert_eq!(named(ITALIC), "emphasized");
    assert_eq!(named(BOLD), "subheader");
    assert_eq!(named(BOLD | ITALIC), "alert");
    assert_eq!(named(FIXED_PITCH), "preformatted");
    assert_eq!(named(REVERSE | BOLD), "user1");
}

// The lower window streams styled runs while the upper window
// grids through the model: a split appears with its rows, and the
// suspended read rides the update as the buffer's own field.
#[test]
fn the_lower_streams_and_the_upper_grids() {
    let mut session = opened(AREAD, 5);
    let mut face = front(&session);

    face.write("Hello ");
    face.set_style(ITALIC);
    face.write("slanted");
    face.set_style(0);
    face.split_window(1);
    face.set_window(1);
    face.set_cursor(1, 1);
    face.write("Status");
    face.set_window(0);

    let update = session.render(false).unwrap();
    let windows = items(at(&update, "windows"));

    assert_eq!(
        windows
            .iter()
            .map(|held| str_of(at(entry(held), "type")).to_string())
            .collect::<Vec<_>>(),
        ["buffer", "grid"]
    );
    assert_eq!(int_of(at(entry(&windows[1]), "gridheight")), 1);

    let content = items(at(&update, "content"));

    assert_eq!(
        told(
            &items(at(
                entry(&items(at(entry(&content[0]), "text"))[0]),
                "content"
            ))[0]
        ),
        r#"{"style":"normal","text":"Hello "}"#
    );
    assert_eq!(
        told(
            &items(at(
                entry(&items(at(entry(&content[0]), "text"))[0]),
                "content"
            ))[1]
        ),
        r#"{"style":"emphasized","text":"slanted"}"#
    );
    assert_eq!(
        told(
            &items(at(
                entry(&items(at(entry(&content[1]), "lines"))[0]),
                "content"
            ))[0]
        ),
        r#"{"style":"normal","text":"Status"}"#
    );

    session.machine().run().unwrap();

    let asked = session.render(false).unwrap();

    assert_eq!(
        told(at(&asked, "input")),
        r#"[{"id":1,"type":"line","maxlen":21,"gen":2}]"#
    );
}

// The quieter protocol ops each find their place: rectangles
// stamp the grid or stack in the stream, erasures of the grid
// leave the buffer standing, a lower erase-line unsays nothing,
// and fonts and cursors ride the model's own ledger.
#[test]
fn the_quieter_ops_find_their_places() {
    let mut session = opened(AREAD, 5);
    let mut face = front(&session);

    face.split_window(1);
    face.set_window(1);
    face.set_cursor(1, 1);
    face.write_rectangle(&["AB".to_string()]);
    face.erase_line();
    face.set_window(0);
    face.write_rectangle(&["row".to_string()]);
    face.erase_line();
    face.set_font(4);
    face.erase_window(1);

    assert_eq!(face.cursor_position().0, 1);

    let update = session.render(false).unwrap();
    let content = items(at(&update, "content"));
    let first = entry(&content[0]);

    assert_eq!(
        str_of(at(
            entry(&items(at(entry(&items(at(first, "text"))[0]), "content"))[0]),
            "text"
        )),
        "row"
    );
    assert!(first.get("clear").is_none());
}

// The Version 1 to 3 status line rides the grid's first row in
// reverse video, the model's own §8.2 formatting.
#[test]
fn the_status_line_rides_the_grid() {
    let mut session = opened(AREAD, 3);

    front(&session).show_status(&Status {
        location: "West of House".to_string(),
        score: 0,
        turns: 1,
        time_game: false,
    });

    let update = session.render(false).unwrap();
    let content = items(at(&update, "content"));
    let gridded = entry(content.last().expect("the grid entry"));
    let line = entry(&items(at(entry(&items(at(gridded, "lines"))[0]), "content"))[0]);

    assert_eq!(str_of(at(line, "style")), "user1");
    assert!(str_of(at(line, "text")).contains("West of House"));
    assert!(str_of(at(line, "text")).contains("Score: 0"));
}

// An erasure of the lower half clears the buffer whole; the typed
// line echoes in the input dress with its newline, since the
// machine never echoes.
#[test]
fn erasures_and_echoes() {
    let mut session = opened(AREAD, 5);
    let mut face = front(&session);

    face.write("before");
    face.erase_window(-1);

    let update = session.render(false).unwrap();

    assert_eq!(told(at(&update, "content")), r#"[{"id":1,"clear":true}]"#);

    session.machine().run().unwrap();
    session.render(false).unwrap();

    let verdict = session
        .accept(&parsed(
            r#"{"type":"line","gen":2,"window":1,"value":"go"}"#,
        ))
        .unwrap();

    assert_eq!(verdict, Verdict::Advance);

    session.machine().run().unwrap();

    let landed = session.render(true).unwrap();
    let content = items(at(&landed, "content"));

    assert_eq!(
        told(at(
            entry(&items(at(entry(&content[0]), "text"))[0]),
            "content"
        )),
        r#"[{"style":"input","text":"go"}]"#
    );
}

// A read under a §10.5.2.1 terminating table offers the function
// keys the wire can name -- a cursor-key entry stays legal but
// unnameable in the protocol's vocabulary -- and the key that ends
// the line stores its own code with nothing echoed, since only a
// return-ended read prints its return (§15 read).
#[test]
fn a_terminator_rides_the_wire() {
    let mut session = opened(AREAD, 5);
    let memory = session.machine().memory_mut();

    memory.write_word(0x2E, 0x1A0).unwrap();

    for (offset, code) in [135u8, 133, 129, 0].into_iter().enumerate() {
        memory.write_byte(0x1A0 + offset, code).unwrap();
    }

    session.machine().run().unwrap();

    let asked = session.render(false).unwrap();

    assert_eq!(
        told(at(&asked, "input")),
        r#"[{"id":1,"type":"line","maxlen":21,"terminators":["func1","func3"],"gen":1}]"#
    );

    let verdict = session
        .accept(&parsed(
            r#"{"type":"line","gen":1,"window":1,"value":"go","terminator":"func3"}"#,
        ))
        .unwrap();

    assert_eq!(verdict, Verdict::Advance);
    assert_eq!(session.machine().memory().read_word(0x100).unwrap(), 135);

    session.machine().run().unwrap();

    let landed = session.render(true).unwrap();

    assert!(landed.get("content").is_none());
}

// A keystroke read arms the grid for clicks -- the whole clickable
// surface, since buffers take none -- and a click lands as §10.3's
// code 254: a wrong window passes with the read standing, while a
// story without a header extension still hears the click, it just
// cannot ask where.
#[test]
fn a_click_lands_on_a_keystroke_read() {
    let mut session = opened(READ_CHAR, 5);

    front(&session).split_window(1);
    session.machine().run().unwrap();

    let asked = session.render(false).unwrap();

    assert_eq!(
        told(at(&asked, "input")),
        r#"[{"id":1,"type":"char","gen":1},{"id":2,"mouse":true}]"#
    );

    let stray = session
        .accept(&parsed(
            r#"{"type":"mouse","gen":1,"window":9,"x":0,"y":0}"#,
        ))
        .unwrap();

    assert_eq!(stray, Verdict::Pass);

    let verdict = session
        .accept(&parsed(
            r#"{"type":"mouse","gen":1,"window":2,"x":4,"y":0}"#,
        ))
        .unwrap();

    assert_eq!(verdict, Verdict::Advance);
    assert_eq!(session.machine().memory().read_word(0x100).unwrap(), 254);
}

// A line read under a table naming the click code arms the grid
// too, and the click ends the line: the typed text rides the event
// as the buffer's partial input -- the field carries no §15
// preload, since the story prints its own -- the machine appends
// it after the held text, and the click's cell coordinates land
// one step over in the header extension, which counts the screen
// from (1,1).
#[test]
fn a_click_ends_a_line_read() {
    let mut session = opened(AREAD, 5);

    front(&session).split_window(1);

    let memory = session.machine().memory_mut();

    memory.write_word(0x2E, 0x1A0).unwrap();
    memory.write_byte(0x1A0, 254).unwrap();
    memory.write_byte(0x1A1, 0).unwrap();
    memory.write_word(0x36, 0x1B0).unwrap();
    memory.write_word(0x1B0, 2).unwrap();
    memory.write_byte(TEXT_BUFFER + 1, 2).unwrap();
    memory.write_byte(TEXT_BUFFER + 2, b'g').unwrap();
    memory.write_byte(TEXT_BUFFER + 3, b'o').unwrap();

    session.machine().run().unwrap();

    let asked = session.render(false).unwrap();

    assert_eq!(
        told(at(&asked, "input")),
        r#"[{"id":1,"type":"line","maxlen":21,"gen":1},{"id":2,"mouse":true}]"#
    );

    let verdict = session
        .accept(&parsed(
            r#"{"type":"mouse","gen":1,"window":2,"x":3,"y":0,"partial":{"1":" hi"}}"#,
        ))
        .unwrap();

    assert_eq!(verdict, Verdict::Advance);

    let memory = session.machine().memory();

    assert_eq!(memory.read_byte(TEXT_BUFFER + 1).unwrap(), 5);
    assert_eq!(memory.read_word(0x100).unwrap(), 254);
    assert_eq!(memory.read_word(0x1B2).unwrap(), 4);
    assert_eq!(memory.read_word(0x1B4).unwrap(), 1);
}

// A click nothing can hear passes with the wait standing: with no
// grid there is nowhere to land, and a line read whose table never
// named the click code leaves it unheard.
#[test]
fn a_click_nothing_hears_passes() {
    let mut session = opened(AREAD, 5);

    session.machine().run().unwrap();
    session.render(false).unwrap();

    let gridless = session
        .accept(&parsed(
            r#"{"type":"mouse","gen":1,"window":2,"x":0,"y":0}"#,
        ))
        .unwrap();

    assert_eq!(gridless, Verdict::Pass);

    let mut armed = opened(AREAD, 5);

    front(&armed).split_window(1);
    armed.machine().run().unwrap();
    armed.render(false).unwrap();

    let verdict = armed
        .accept(&parsed(
            r#"{"type":"mouse","gen":1,"window":2,"x":0,"y":0}"#,
        ))
        .unwrap();

    assert_eq!(verdict, Verdict::Pass);
    assert!(armed.machine().waiting().is_some());
}

// Named keys land as their §3.8 codes; a name the table lacks, and
// a key ZSCII cannot spell, pass with the read standing.
#[test]
fn named_keys_land() {
    let mut session = opened(READ_CHAR, 4);

    session.machine().run().unwrap();
    session.render(false).unwrap();

    assert_eq!(
        session
            .accept(&parsed(
                r#"{"type":"char","gen":1,"window":1,"value":"borogove"}"#
            ))
            .unwrap(),
        Verdict::Pass
    );
    assert_eq!(
        session
            .accept(&parsed(r#"{"type":"char","gen":1,"window":1,"value":"λ"}"#))
            .unwrap(),
        Verdict::Pass
    );
    assert_eq!(
        session
            .accept(&parsed(
                r#"{"type":"char","gen":1,"window":1,"value":"escape"}"#
            ))
            .unwrap(),
        Verdict::Advance
    );
    assert_eq!(session.machine().memory().read_word(0x100).unwrap(), 27);
}

// A timed read feeds the display's clock and restarts it for a
// fresh read; a tick fires the interrupt and stands, a true return
// advances, and a tick with no timed read passes.
#[test]
fn ticks_stand_and_advance() {
    let mut session = session_with(TIMED, 4, Some(MARK_THEN_FALSE), None, &init());

    session.machine().run().unwrap();

    let update = session.render(false).unwrap();

    assert_eq!(int_of(at(&update, "timer")), 1000);
    assert_eq!(
        session
            .accept(&parsed(r#"{"type":"timer","gen":1}"#))
            .unwrap(),
        Verdict::Stand
    );
    assert_eq!(session.machine().memory().read_word(0x102).unwrap(), 0x63);
    assert_eq!(
        session
            .accept(&parsed(
                r#"{"type":"line","gen":1,"window":1,"value":"on"}"#
            ))
            .unwrap(),
        Verdict::Advance
    );

    let mut ended = session_with(TIMED, 4, Some(MARK_THEN_TRUE), None, &init());

    ended.machine().run().unwrap();
    ended.render(false).unwrap();

    assert_eq!(
        ended
            .accept(&parsed(r#"{"type":"timer","gen":1}"#))
            .unwrap(),
        Verdict::Advance
    );

    let mut idle = opened(AREAD, 5);

    idle.machine().run().unwrap();
    idle.render(false).unwrap();

    assert_eq!(
        idle.accept(&parsed(r#"{"type":"timer","gen":1}"#)).unwrap(),
        Verdict::Pass
    );
}

// A grid that closes and reopens is a new window with a new id --
// the protocol forbids reuse -- and an arrange remeasures for the
// next boot while the picture stands. The teardown is §8.7.3.3's
// whole-screen erasure: a bare unsplit holds until the next input,
// the quote-box courtesy.
#[test]
fn the_grid_comes_and_goes_with_new_names() {
    let mut session = opened(AREAD, 5);

    front(&session).split_window(1);

    let first = session.render(false).unwrap();

    assert_eq!(int_of(at(entry(&items(at(&first, "windows"))[1]), "id")), 2);

    front(&session).erase_window(-1);

    let gone = session.render(false).unwrap();

    assert_eq!(items(at(&gone, "windows")).len(), 1);
    assert_eq!(
        str_of(at(entry(&items(at(&gone, "windows"))[0]), "type")),
        "buffer"
    );

    front(&session).split_window(1);

    let again = session.render(false).unwrap();

    assert_eq!(int_of(at(entry(&items(at(&again, "windows"))[1]), "id")), 3);

    let verdict = session
        .accept(&parsed(
            r#"{"type":"arrange","gen":3,"metrics":{"width":400,"height":200}}"#,
        ))
        .unwrap();

    assert_eq!(verdict, Verdict::Stand);
    assert_eq!(
        session
            .accept(&parsed(r#"{"type":"external","gen":3}"#))
            .unwrap(),
        Verdict::Pass
    );
}

// An arrange that grows the display leaves the grid at its boot
// width: the model keeps its size until a reload boots a machine
// at the new one (§8.4), and the wider face never reads past the
// model's edge -- the desktop shell's Measure menu found the
// reference falling over exactly there.
#[test]
fn a_grown_arrange_keeps_the_boot_grid() {
    let mut session = opened(AREAD, 5);

    front(&session).split_window(1);
    session.machine().run().unwrap();
    session.render(false).unwrap();

    let verdict = session
        .accept(&parsed(
            r#"{"type":"arrange","gen":1,"metrics":{"width":1600,"height":900,"gridcharwidth":10,"gridcharheight":20}}"#,
        ))
        .unwrap();

    assert_eq!(verdict, Verdict::Stand);

    let update = session.render(false).unwrap();
    let grid = entry(&items(at(&update, "windows"))[1]);

    // The box grows with the display; the cells stay the boot
    // grid's 80.
    assert_eq!(int_of(at(grid, "width")), 1600);
    assert_eq!(int_of(at(grid, "gridwidth")), 160);
}

// The grid's box carries the display's interior margins on top of
// its rows (GlkOte: The Metrics Object) -- a box of bare rows
// clips its bottom and floats the buffer up into the status line
// -- and with no grid at all the buffer starts back at the very
// top.
#[test]
fn the_grid_box_wears_the_margins() {
    let mut session = opened(AREAD, 5);

    session
        .face()
        .begin(&parsed(
            r#"{"type":"init","gen":0,"metrics":{"width":800,"height":480,"gridcharwidth":10,"gridcharheight":20,"gridmarginx":20,"gridmarginy":12}}"#,
        ))
        .unwrap();

    front(&session).split_window(2);

    let split = session.render(false).unwrap();
    let windows = items(at(&split, "windows"));

    assert_eq!(int_of(at(entry(&windows[1]), "top")), 0);
    assert_eq!(int_of(at(entry(&windows[1]), "height")), 52);
    assert_eq!(int_of(at(entry(&windows[0]), "top")), 52);
    assert_eq!(int_of(at(entry(&windows[0]), "height")), 428);

    front(&session).erase_window(-1);

    let alone = session.render(false).unwrap();
    let windows = items(at(&alone, "windows"));

    assert_eq!(int_of(at(entry(&windows[0]), "top")), 0);
    assert_eq!(int_of(at(entry(&windows[0]), "height")), 480);
}

// The §9 sounds speak the wire's dialect: a play op carries the
// AIFF re-wrapped as a WAVE data: url on the one channel with the
// volume in eighths, zero repeats spell forever, Version 3's
// silence is answered by the Loop chunk, a stop lands only on the
// number sounding, and the bleeps ride as the display's own
// oscillator notes. Without the display's word nothing rides at
// all, and without a Blorb nothing is claimed even with it.
#[test]
fn z_sounds_speak_the_dialect() {
    let session = hearing(sounding_resources(true), AREAD, r#""timer","sound""#);
    let mut face = front(&session);

    assert!(face.has_sounds());
    assert!(face.play_sound(3, 4, None));
    assert!(face.play_sound(3, 8, Some(0)));
    assert!(face.play_sound(3, 8, Some(2)));
    assert!(!face.play_sound(9, 8, Some(1)));

    face.stop_sound(Some(7));
    face.stop_sound(Some(3));
    face.stop_sound(None);
    face.bleep(true);
    face.bleep(false);

    let mut session = session;
    let update = session.render(false).unwrap();
    let ops: Vec<&Object> = items(at(&update, "sounds")).iter().map(entry).collect();

    assert_eq!(
        ops.iter()
            .map(|held| held.get("op").and_then(Value::as_str).unwrap_or(""))
            .collect::<Vec<_>>(),
        ["play", "play", "play", "stop", "bleep", "bleep"]
    );
    assert!(str_of(at(ops[0], "url")).starts_with("data:audio/wav;base64,"));
    assert_eq!(int_of(at(ops[0], "repeats")), -1);
    assert_eq!(told(at(ops[0], "volume")), "0.5");
    assert_eq!(int_of(at(ops[1], "repeats")), -1);
    assert_eq!(told(at(ops[1], "volume")), "1.0");
    assert_eq!(int_of(at(ops[2], "repeats")), 2);
    assert_eq!(int_of(at(ops[4], "bleep")), 1);
    assert_eq!(int_of(at(ops[5], "bleep")), 2);

    let mut quiet = hearing(sounding_resources(false), AREAD, r#""timer""#);

    assert!(!front(&quiet).has_sounds());

    front(&quiet).bleep(true);
    quiet.machine().run().unwrap();

    assert!(quiet.render(false).unwrap().get("sounds").is_none());

    let mut bare = GlkOteFrontend::new(5, None);

    bare.begin(&init_supporting(r#""sound""#)).unwrap();

    let mut bare = SharedFace {
        face: Rc::new(RefCell::new(bare)),
    };

    assert!(!bare.has_sounds());
    assert!(!bare.play_sound(3, 8, Some(1)));
}

// The whole §9.4 round over the wire: sound_effect starts the
// sample and keeps its routine, the wire's finish report fires the
// end-of-sound routine through the machine's own loop with the
// read still standing, and a report for a sound since stopped or
// replaced means nothing, §9.4.4's own rule.
#[test]
fn the_end_of_sound_routine_fires() {
    let mut session = hearing(sounding_resources(false), SOUNDED, r#""timer","sound""#);

    session.machine().run().unwrap();

    let update = session.render(false).unwrap();
    let played = entry(&items(at(&update, "sounds"))[0]);

    assert_eq!(int_of(at(played, "sound")), 3);
    assert_eq!(int_of(at(played, "repeats")), 1);
    assert_eq!(told(at(played, "volume")), "1.0");

    let stray = session
        .accept(&parsed(r#"{"type":"sound","gen":1,"channel":1,"sound":9}"#))
        .unwrap();

    assert_eq!(stray, Verdict::Pass);

    let verdict = session
        .accept(&parsed(r#"{"type":"sound","gen":1,"channel":1,"sound":3}"#))
        .unwrap();

    assert_eq!(verdict, Verdict::Stand);
    assert_eq!(session.machine().memory().read_word(0x102).unwrap(), 0x63);
    assert!(session.machine().waiting().is_some());

    let silent = session
        .accept(&parsed(r#"{"type":"sound","gen":1,"channel":1,"sound":3}"#))
        .unwrap();

    assert_eq!(silent, Verdict::Pass);
}

// A display that lost its picture asks for it whole: the refresh
// event is accepted ahead of the generation gate -- a lost display
// is out of sync by definition -- and earns an update complete in
// content: every window, the buffer's kept scrollback behind a
// clear, the grid's every row with the blank ones as bare line
// numbers, the input field stamped anew at the new generation, and
// a running timer renamed. The keeping is bounded: a long session
// replays its recent paragraphs, not its whole life.
#[test]
fn a_refresh_earns_the_whole_picture() {
    let mut session = opened(AREAD, 5);

    {
        let mut face = front(&session);

        face.write("Once upon a time.\n");
        face.split_window(2);
        face.set_window(1);
        face.set_cursor(1, 1);
        face.write("Status");
        face.set_window(0);
        face.write("And then more.\n");
    }

    session.machine().run().unwrap();
    session.render(false).unwrap();

    assert_eq!(
        session
            .accept(&parsed(r#"{"type":"refresh","gen":99}"#))
            .unwrap(),
        Verdict::Stand
    );

    let whole = session.render(false).unwrap();
    let windows = items(at(&whole, "windows"));

    assert_eq!(
        windows
            .iter()
            .map(|held| str_of(at(entry(held), "type")).to_string())
            .collect::<Vec<_>>(),
        ["buffer", "grid"]
    );

    let content = items(at(&whole, "content"));
    let texted = content
        .iter()
        .map(entry)
        .find(|held| held.get("id").and_then(Value::as_int) == Some(1))
        .expect("the buffer entry");

    assert_eq!(at(texted, "clear"), &Value::Bool(true));

    let paragraphs: Vec<String> = items(at(texted, "text"))
        .iter()
        .map(entry)
        .filter(|held| held.get("content").is_some())
        .map(|held| str_of(at(entry(&items(at(held, "content"))[0]), "text")).to_string())
        .collect();

    assert_eq!(paragraphs, ["Once upon a time.", "And then more."]);

    let gridded = content
        .iter()
        .map(entry)
        .find(|held| held.get("lines").is_some())
        .expect("the grid entry");
    let lines = items(at(gridded, "lines"));

    assert!(
        str_of(at(
            entry(&items(at(entry(&lines[0]), "content"))[0]),
            "text"
        ))
        .contains("Status")
    );
    assert_eq!(told(&lines[1]), r#"{"line":1}"#);
    assert_eq!(
        at(entry(&items(at(&whole, "input"))[0]), "gen"),
        at(&whole, "gen")
    );

    let mut ticking = session_with(TIMED, 4, Some(MARK_THEN_FALSE), None, &init());

    ticking.machine().run().unwrap();
    ticking.render(false).unwrap();
    ticking
        .accept(&parsed(r#"{"type":"refresh","gen":1}"#))
        .unwrap();

    assert_eq!(int_of(at(&ticking.render(false).unwrap(), "timer")), 1000);

    let mut longwinded = opened(AREAD, 5);

    {
        let mut face = front(&longwinded);

        for number in 0..KEPT_PARAGRAPHS + 10 {
            face.write(&format!("para {number}\n"));
        }
    }

    longwinded.machine().run().unwrap();
    longwinded.render(false).unwrap();
    longwinded
        .accept(&parsed(r#"{"type":"refresh","gen":1}"#))
        .unwrap();

    let retold = longwinded.render(false).unwrap();
    let kept = items(at(&retold, "content"))
        .iter()
        .map(entry)
        .find(|held| held.get("id").and_then(Value::as_int) == Some(1))
        .expect("the buffer entry");

    assert_eq!(items(at(kept, "text")).len(), KEPT_PARAGRAPHS);
    assert_eq!(
        str_of(at(
            entry(&items(at(entry(&items(at(kept, "text"))[0]), "content"))[0]),
            "text"
        )),
        "para 10"
    );

    let mut banded = session_with(
        AREAD,
        5,
        None,
        Some(banded_resources(false, None)),
        &init_supporting(r#""timer","graphicswin""#),
    );

    front(&banded).draw_arc_image(8, 12);
    banded.machine().rebase_rows().unwrap();
    banded.render(false).unwrap();
    banded
        .accept(&parsed(r#"{"type":"refresh","gen":5}"#))
        .unwrap();

    let refed = banded.render(false).unwrap();
    let hung = items(at(&refed, "content"))
        .iter()
        .map(entry)
        .find(|held| held.get("draw").is_some())
        .expect("the band's drawing");
    let draw = items(at(hung, "draw"));

    assert_eq!(
        str_of(at(entry(draw.last().expect("the image op")), "special")),
        "image"
    );
}

// An Inform quote box splits the upper window tall, writes, and
// shrinks the split back at once, trusting §8.6.1.2's no-clearing
// rule to leave the box standing on the screen -- so the grid
// stays at the turn's high water until the next input arrives, and
// a whole-screen erasure tears the box down with the split.
#[test]
fn a_quote_box_survives_the_shrink() {
    let mut session = opened(AREAD, 5);

    {
        let mut face = front(&session);

        face.split_window(3);
        face.set_window(1);
        face.set_cursor(2, 5);
        face.write("Will you read me a story?");
        face.set_window(0);
        face.split_window(1);
    }

    let update = session.render(false).unwrap();

    assert_eq!(
        int_of(at(entry(&items(at(&update, "windows"))[1]), "gridheight")),
        3
    );

    let boxed = items(at(&update, "content"))
        .iter()
        .map(entry)
        .filter(|held| held.get("lines").is_some())
        .flat_map(|held| items(at(held, "lines")).iter().map(entry))
        .find(|held| held.get("line").and_then(Value::as_int) == Some(1))
        .expect("the boxed row");

    assert!(
        str_of(at(entry(&items(at(boxed, "content"))[0]), "text"))
            .contains("Will you read me a story?")
    );

    session.machine().run().unwrap();
    session.render(false).unwrap();
    session
        .accept(&parsed(
            r#"{"type":"line","gen":2,"window":1,"value":"go"}"#,
        ))
        .unwrap();
    session.machine().run().unwrap();

    let shrunk = session.render(true).unwrap();

    assert_eq!(
        int_of(at(entry(&items(at(&shrunk, "windows"))[1]), "gridheight")),
        1
    );

    let mut torn = opened(AREAD, 5);

    {
        let mut face = front(&torn);

        face.split_window(3);
        face.erase_window(-1);
    }

    let cleared = torn.render(false).unwrap();

    assert_eq!(items(at(&cleared, "windows")).len(), 1);
    assert_eq!(
        str_of(at(entry(&items(at(&cleared, "windows"))[0]), "type")),
        "buffer"
    );

    // Version 3 keeps no high water: splitting clears the upper
    // window there (§8.6.1.1), so no box could survive anyway.
    let mut classic = opened(AREAD, 3);

    {
        let mut face = front(&classic);

        face.split_window(2);
        face.split_window(0);
    }

    let plain = classic.render(false).unwrap();

    assert_eq!(
        int_of(at(entry(&items(at(&plain, "windows"))[1]), "gridheight")),
        1
    );
}

// The §8.3 colours ride the wire under the display's own word:
// runs carry the shared palette's CSS ink, adjacent same-ink text
// coalesces, reverse video swaps ink and paper as every painted
// face swaps them, the grid's cells dress their spans through the
// model, and the model's background travels as both windows' paper
// -- Photopia's scenes bleed to the window's edge, not just under
// its letters. Without the word there is no claim at all.
#[test]
fn colours_ride_the_wire() {
    let mut session = session_with(
        AREAD,
        5,
        None,
        None,
        &init_supporting(r#""timer","colors""#),
    );

    assert!(front(&session).has_colours());

    {
        let mut face = front(&session);

        face.set_colour(3, 1);
        face.write("red ");
        face.write("more");
        face.set_style(REVERSE);
        face.write("swap");
        face.set_style(0);
        face.set_colour(0, 6);
        face.write("sea");
        face.split_window(1);
        face.set_window(1);
        face.set_cursor(1, 1);
        face.write("Top");
        face.set_window(0);
        face.set_colour(1, 0);
        face.write("plain");
    }

    let update = session.render(false).unwrap();
    let content = items(at(&update, "content"));

    assert_eq!(
        told(at(
            entry(&items(at(entry(&content[0]), "text"))[0]),
            "content"
        )),
        r##"[{"style":"normal","text":"red more","fg":"#cc0000"},{"style":"user1","text":"swap","bg":"#cc0000"},{"style":"normal","text":"sea","fg":"#cc0000","bg":"#0000cc"},{"style":"normal","text":"plain","bg":"#0000cc"}]"##
    );
    assert_eq!(
        told(at(
            entry(&items(at(entry(&content[1]), "lines"))[0]),
            "content"
        )),
        r##"[{"style":"normal","text":"Top","fg":"#cc0000","bg":"#0000cc"}]"##
    );

    let windows = items(at(&update, "windows"));

    assert_eq!(str_of(at(entry(&windows[0]), "bg")), "#0000cc");
    assert_eq!(str_of(at(entry(&windows[1]), "bg")), "#0000cc");

    let mut plain = GlkOteFrontend::new(5, None);

    plain.begin(&init()).unwrap();

    assert!(!plain.has_colours);
}

// The record's card joins the cover at the door: the title in the
// header dress, the headline and author emphasized, and the
// description's paragraphs blank-line separated -- needing no
// display grant, since a card is only text (Babel: The iFiction
// format).
#[test]
fn the_card_stands_at_the_door() {
    let record: &[u8] = b"<ifindex><story><bibliographic><title>Tiny Case</title>\
        <headline>An interactive test</headline><author>A. Tester</author>\
        <description>One.<br/>Two.</description>\
        </bibliographic></story></ifindex>";
    let mut session = session_with(
        AREAD,
        5,
        None,
        Some(banded_resources(true, Some(record))),
        &init_supporting(r#""timer","graphics""#),
    );

    let update = session.render(false).unwrap();
    let text = items(at(entry(&items(at(&update, "content"))[0]), "text")).to_vec();
    let para = |index: usize| -> &Value { &items(at(entry(&text[index]), "content"))[0] };

    assert_eq!(str_of(at(entry(para(0)), "special")), "image");
    assert_eq!(told(para(1)), r#"{"style":"header","text":"Tiny Case"}"#);
    assert_eq!(
        told(para(2)),
        r#"{"style":"emphasized","text":"An interactive test"}"#
    );
    assert_eq!(
        told(para(3)),
        r#"{"style":"emphasized","text":"A. Tester"}"#
    );
    assert_eq!(told(para(5)), r#"{"style":"normal","text":"One."}"#);
    assert_eq!(told(para(7)), r#"{"style":"normal","text":"Two."}"#);
}

// The doorway courtesy over the wire: a Blorb's Fspc cover stands
// at the top of the story's text -- once, before anything the
// machine prints, its own paragraph -- when the display grants
// bare graphics; without the grant, or without a cover, the story
// simply opens plain.
#[test]
fn the_cover_stands_at_the_door() {
    let mut session = session_with(
        AREAD,
        5,
        None,
        Some(banded_resources(true, None)),
        &init_supporting(r#""timer","graphics""#),
    );

    front(&session).write("Hello");

    let update = session.render(false).unwrap();
    let text = items(at(entry(&items(at(&update, "content"))[0]), "text")).to_vec();
    let cover = entry(&items(at(entry(&text[0]), "content"))[0]);

    assert_eq!(str_of(at(cover, "special")), "image");
    assert_eq!(int_of(at(cover, "image")), 8);
    assert_eq!(str_of(at(cover, "alignment")), "inlineup");
    assert_eq!(int_of(at(cover, "width")), 320);
    assert_eq!(int_of(at(cover, "height")), 96);
    assert!(str_of(at(cover, "url")).starts_with("data:image/png;base64,"));
    assert_eq!(
        told(at(entry(&text[1]), "content")),
        r#"[{"style":"normal","text":"Hello"}]"#
    );

    let mut ungranted = session_with(AREAD, 5, None, Some(banded_resources(true, None)), &init());

    front(&ungranted).write("Hello");

    let opening = ungranted.render(false).unwrap();

    assert_eq!(
        told(at(
            entry(&items(at(entry(&items(at(&opening, "content"))[0]), "text"))[0]),
            "content"
        )),
        r#"[{"style":"normal","text":"Hello"}]"#
    );

    let mut coverless = session_with(
        AREAD,
        5,
        None,
        Some(banded_resources(false, None)),
        &init_supporting(r#""timer","graphics""#),
    );

    front(&coverless).write("Hello");

    let bare = coverless.render(false).unwrap();

    assert_eq!(
        told(at(
            entry(&items(at(entry(&items(at(&bare, "content"))[0]), "text"))[0]),
            "content"
        )),
        r#"[{"style":"normal","text":"Hello"}]"#
    );
}

// The arc_image band hangs above the whole screen: a graphics
// window at the top, the picture inlined as a data: url shaped to
// the display's width, the buffer re-based below, and the header's
// rows shrunk to what remains. Ignorable calls are ignored -- an
// unanswered id, a mode outside the two named -- a clear gives the
// rows back and retires the canvas, a reopened band wears a fresh
// id, a redraw refeeds the drawing, and an arrange re-shapes it.
// The claim itself is honest twice over: no art or no graphicswin,
// no claim.
#[test]
fn the_band_hangs_above_the_screen() {
    let mut session = session_with(
        AREAD,
        5,
        None,
        Some(banded_resources(false, None)),
        &init_supporting(r#""timer","graphicswin""#),
    );

    assert!(front(&session).has_arc_images());

    front(&session).draw_arc_image(9, 12); // no such picture: ignored
    front(&session).draw_arc_image(8, 7); // no such mode: ignored

    assert!(session.face().band.is_none());

    front(&session).draw_arc_image(8, 12);
    session.machine().rebase_rows().unwrap();

    // 800 wide at 96/320 aspect is a 240-pixel band; twelve rows
    // of twenty pixels remain below, and the header says so.
    assert_eq!(
        session.machine().memory().read_byte(SCREEN_LINES).unwrap(),
        12
    );

    let update = session.render(false).unwrap();
    let windows = items(at(&update, "windows"));
    let band = entry(&windows[0]);

    assert_eq!(str_of(at(band, "type")), "graphics");
    assert_eq!(int_of(at(band, "top")), 0);
    assert_eq!(int_of(at(band, "height")), 240);
    assert_eq!(int_of(at(entry(&windows[1]), "top")), 240);

    let drawn = items(at(
        items(at(&update, "content"))
            .iter()
            .map(entry)
            .find(|held| held.get("draw").is_some())
            .expect("the band's drawing"),
        "draw",
    ))
    .to_vec();

    assert_eq!(told(&drawn[0]), r#"{"special":"fill"}"#);
    assert!(str_of(at(entry(&drawn[1]), "url")).starts_with("data:image/png;base64,"));
    assert_eq!(int_of(at(entry(&drawn[1]), "width")), 800);
    assert_eq!(int_of(at(entry(&drawn[1]), "height")), 240);

    // A redraw refeeds the drawing whole.
    let generation = session.face().page.generation();

    assert_eq!(
        session
            .accept(&parsed(&format!(
                r#"{{"type":"redraw","gen":{generation}}}"#
            )))
            .unwrap(),
        Verdict::Stand
    );

    let refed = session.render(false).unwrap();

    assert_eq!(
        items(at(
            items(at(&refed, "content"))
                .iter()
                .map(entry)
                .find(|held| held.get("draw").is_some())
                .expect("the refed drawing"),
            "draw",
        ))
        .len(),
        2
    );

    // An arrange re-shapes the band to the new width.
    let generation = session.face().page.generation();

    session
        .accept(&parsed(&format!(
            r#"{{"type":"arrange","gen":{generation},"metrics":{{"width":400,"height":480,"gridcharwidth":10,"gridcharheight":20}}}}"#
        )))
        .unwrap();

    let arranged = session.render(false).unwrap();
    let first_ident = int_of(at(entry(&items(at(&arranged, "windows"))[0]), "id"));

    assert_eq!(
        int_of(at(entry(&items(at(&arranged, "windows"))[0]), "height")),
        120
    );

    // A clear takes the canvas down and gives the rows back; the
    // band reopened wears a fresh id.
    front(&session).draw_arc_image(0, 12);
    session.machine().rebase_rows().unwrap();

    assert_eq!(
        session.machine().memory().read_byte(SCREEN_LINES).unwrap(),
        24
    );

    let bare = session.render(false).unwrap();

    assert_eq!(items(at(&bare, "windows")).len(), 1);
    assert_eq!(
        str_of(at(entry(&items(at(&bare, "windows"))[0]), "type")),
        "buffer"
    );

    front(&session).draw_arc_image(8, 12);

    let reopened = session.render(false).unwrap();

    assert!(int_of(at(entry(&items(at(&reopened, "windows"))[0]), "id")) > first_ident);

    // Re-drawing the hanging picture owes nothing new: the update
    // that follows is the pass stanza, the canvas untouched.
    session.render(false).unwrap();
    front(&session).draw_arc_image(8, 12);

    assert_eq!(
        told(&Value::Object(session.render(false).unwrap())),
        r#"{"type":"pass"}"#
    );

    // A redraw with no band has nothing here to repaint.
    front(&session).draw_arc_image(0, 12);
    session.render(false).unwrap();

    let generation = session.face().page.generation();

    assert_eq!(
        session
            .accept(&parsed(&format!(
                r#"{{"type":"redraw","gen":{generation}}}"#
            )))
            .unwrap(),
        Verdict::Pass
    );

    // A band drawn before any machine boots simply hangs: the
    // header's rows are told when there is a header to tell.
    let mut early = GlkOteFrontend::new(5, Some(banded_resources(false, None)));

    early.begin(&init_supporting(r#""graphicswin""#)).unwrap();
    early.hang_arc_image(8, 9);

    assert_eq!(early.band, Some((8, 9)));

    // The claim is honest: art without graphicswin, or a display
    // without art, never claims.
    let mut artless = GlkOteFrontend::new(5, None);

    artless
        .begin(&parsed(
            r#"{"type":"init","gen":0,"support":["graphicswin"],"metrics":{"width":80,"height":24}}"#,
        ))
        .unwrap();

    assert!(!artless.has_arc_images);

    let canvasless = session_with(AREAD, 5, None, Some(banded_resources(false, None)), &init());

    assert!(!front(&canvasless).has_arc_images());
}

// A save asks through the protocol's special input: the update
// carries the fileref prompt in the write mode, the answered path
// advances the machine and keeps a real file, a restore asks in
// the read mode, a response to some other ask asks nothing here,
// and a dialog's non-string ref reads as the cancel it is.
#[test]
fn saves_ask_through_special_input() {
    let mut session = opened(SAVING, 5);

    session.machine().run().unwrap();

    let update = session.render(false).unwrap();

    assert_eq!(
        told(at(&update, "specialinput")),
        r#"{"type":"fileref_prompt","filemode":"write","filetype":"save"}"#
    );

    let generation = session.face().page.generation();

    assert_eq!(
        session
            .accept(&parsed(&format!(
                r#"{{"type":"specialresponse","gen":{generation},"response":"unknown_prompt","value":"x"}}"#
            )))
            .unwrap(),
        Verdict::Pass
    );

    let kept = std::env::temp_dir().join(format!("voxam-zglkote-{}.sav", std::process::id()));
    let kept_text = kept.to_string_lossy().replace('\\', "\\\\");

    assert_eq!(
        session
            .accept(&parsed(&format!(
                r#"{{"type":"specialresponse","gen":{generation},"response":"fileref_prompt","value":"{kept_text}"}}"#
            )))
            .unwrap(),
        Verdict::Advance
    );
    assert!(kept.is_file());

    session.machine().run().unwrap();

    let asked = session.render(false).unwrap();

    assert_eq!(
        str_of(at(entry(at(&asked, "specialinput")), "filemode")),
        "read"
    );

    // A fileref object from some browser dialog is a cancel.
    let generation = session.face().page.generation();

    assert_eq!(
        session
            .accept(&parsed(&format!(
                r#"{{"type":"specialresponse","gen":{generation},"response":"fileref_prompt","value":{{"filename":"x"}}}}"#
            )))
            .unwrap(),
        Verdict::Advance
    );

    session.machine().run().unwrap();

    assert!(!session.machine().running());

    let _ = std::fs::remove_file(kept);
}

/// A Version 4 story that reads one line and quits, whole.
fn reading_image() -> Story {
    let mut data = vec![0u8; 96];

    data[0] = 4;
    data[0x04..0x06].copy_from_slice(&0x0060u16.to_be_bytes());
    data[0x06..0x08].copy_from_slice(&0x0040u16.to_be_bytes());
    data[0x08..0x0A].copy_from_slice(&0x005Au16.to_be_bytes());
    data[0x0E..0x10].copy_from_slice(&0x0060u16.to_be_bytes());
    data[0x40..0x47].copy_from_slice(&[0xE4, 0x0F, 0x00, 0x50, 0x00, 0x58, 0xBA]);
    data[0x50] = 6;
    data[0x58] = 1;
    data[0x5A] = 0;
    data[0x5B] = 7;

    Story::new(data).unwrap()
}

/// One whole Z conversation over byte pipes.
fn served_lines(lines: &[String]) -> (bool, Vec<Object>) {
    let joined: String = lines.iter().map(|line| format!("{line}\n")).collect();
    let mut reader = std::io::Cursor::new(joined.into_bytes());
    let mut writer: Vec<u8> = Vec::new();
    let clean = serve(
        reading_image(),
        GlkOteFrontend::new(4, None),
        &mut reader,
        &mut writer,
        Some(7),
    );
    let spoken = String::from_utf8(writer)
        .unwrap()
        .lines()
        .map(parsed)
        .collect();

    (clean, spoken)
}

fn init_line() -> String {
    dumps(&Value::Object(init()))
}

// The whole conversation: init boots the machine at the measured
// size, the update carries the ask, the line echoes and answers,
// and the stray and the garbled are answered in kind.
#[test]
fn a_session_serves_end_to_end() {
    let (clean, stanzas) = served_lines(&[
        init_line(),
        r#"{"type":"line","gen":0,"window":1,"value":"stale"}"#.to_string(),
        r#"{"type":"arrange","gen":1,"metrics":{"width":400,"height":200}}"#.to_string(),
        r#"{"type":"line","gen":2,"window":1,"value":"go"}"#.to_string(),
    ]);

    assert!(clean);
    assert_eq!(
        stanzas
            .iter()
            .map(|held| str_of(at(held, "type")).to_string())
            .collect::<Vec<_>>(),
        ["update", "pass", "update", "update"]
    );
    assert_eq!(
        str_of(at(entry(&items(at(&stanzas[0], "input"))[0]), "type")),
        "line"
    );
    assert_eq!(
        at(stanzas.last().expect("the exit"), "exit"),
        &Value::Bool(true)
    );

    let (refused, spoken) = served_lines(&[r#"{"type":"line","gen":0,"value":"x"}"#.to_string()]);

    assert!(!refused);
    assert!(str_of(at(&spoken[0], "message")).contains("opens with an init"));

    let (hung, quiet) = served_lines(&[init_line()]);

    assert!(hung);
    assert_eq!(quiet.len(), 1);

    let (garbled, noise) = served_lines(&[init_line(), "{nope".to_string()]);

    assert!(!garbled);
    assert!(str_of(at(noise.last().expect("the error"), "message")).contains("not JSON"));

    // A misaimed keystroke -- a char event while a line read
    // stands -- is the blocking loop's shrug now, not a fatal
    // wiring fault: the session answers the pass stanza and lives.
    let (misaimed, spoken) = served_lines(&[
        init_line(),
        r#"{"type":"char","gen":1,"value":"A"}"#.to_string(),
    ]);

    assert!(misaimed);
    assert_eq!(
        told(&Value::Object(spoken.last().expect("the pass").clone())),
        r#"{"type":"pass"}"#
    );
}

// The face a story asks for: the two-window picture for every
// version but 6, which waits on the stage rung and is refused
// rather than mis-served.
#[test]
fn fronted_picks_the_face() {
    assert!(fronted(5, None).is_ok());
    assert!(
        fronted(6, None)
            .map(|_| ())
            .unwrap_err()
            .to_string()
            .contains("stage")
    );
}

// The misaimed-event shrugs hold at every doorway: a char while a
// line read stands, a specialresponse with no ask, a line while a
// key read stands -- each passes with the wait standing -- and the
// named return lands as ZSCII 13.
#[test]
fn misaimed_events_pass_and_return_lands() {
    let mut session = opened(AREAD, 5);

    session.machine().run().unwrap();

    let lined = int_of(at(&session.render(false).unwrap(), "gen"));

    assert_eq!(
        session
            .accept(&parsed(&format!(
                r#"{{"type":"char","gen":{lined},"value":"x"}}"#
            )))
            .unwrap(),
        Verdict::Pass
    );
    assert_eq!(
        session
            .accept(&parsed(&format!(
                r#"{{"type":"specialresponse","gen":{lined},"response":"fileref_prompt"}}"#
            )))
            .unwrap(),
        Verdict::Pass
    );

    let mut keyed = opened(READ_CHAR, 5);

    keyed.machine().run().unwrap();

    let asked = int_of(at(&keyed.render(false).unwrap(), "gen"));

    assert_eq!(
        keyed
            .accept(&parsed(&format!(
                r#"{{"type":"line","gen":{asked},"value":"go"}}"#
            )))
            .unwrap(),
        Verdict::Pass
    );
    assert_eq!(
        keyed
            .accept(&parsed(&format!(
                r#"{{"type":"char","gen":{asked},"value":"return"}}"#
            )))
            .unwrap(),
        Verdict::Advance
    );

    keyed.machine().run().unwrap();

    assert_eq!(keyed.machine().memory().read_word(0x100).unwrap(), 13);
}

// The guards hold at the pointers too: a click with only a file
// ask standing passes at the grid. (The stage half of this drill
// arrives with the stage rung.)
#[test]
fn misaimed_clicks_pass() {
    let mut session = opened(SAVED, 5);

    front(&session).split_window(1);
    session.machine().run().unwrap();

    let update = session.render(false).unwrap();
    let grid = int_of(at(entry(&items(at(&update, "windows"))[1]), "id"));
    let generation = int_of(at(&update, "gen"));

    assert_eq!(
        session
            .accept(&parsed(&format!(
                r#"{{"type":"mouse","gen":{generation},"window":{grid},"x":1,"y":1}}"#
            )))
            .unwrap(),
        Verdict::Pass
    );
}
