//! The Z face of GlkOte: the screen model composed, reads
//! delivered -- and the Version 6 stage half: one scaled canvas,
//! placed text, pictures through the adaptive dance, and the
//! under-cursor samples.

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

// EXT restore alone, storing, then quit.
const RESTORED: &[u8] = &[0xBE, 0x01, 0xFF, 0x10, 0xBA];

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
    face.erase_line(None);
    face.set_window(0);
    face.write_rectangle(&["row".to_string()]);
    face.erase_line(None);
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
    assert!(fronted(6, None).expect("the stage face").stage.is_some());
    assert!(
        fronted(5, None)
            .expect("a servable version")
            .stage
            .is_none()
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

// -- the Version 6 stage half of the battery ----------------------

fn stage_init() -> Object {
    parsed(
        r#"{"type":"init","gen":0,"support":["timer","stage","sound"],"metrics":{"width":1280,"height":800}}"#,
    )
}

/// A 2-by-1 indexed-colour PNG, the reference battery's own press.
fn indexed_png(colours: &[(u8, u8, u8)], alphas: &[u8], raw: &[u8]) -> Vec<u8> {
    use crate::flate::{crc32, deflated};

    let piece = |name: &[u8; 4], payload: &[u8]| -> Vec<u8> {
        let mut out = (payload.len() as u32).to_be_bytes().to_vec();

        out.extend_from_slice(name);
        out.extend_from_slice(payload);
        out.extend_from_slice(&crc32(payload, crc32(name, 0)).to_be_bytes());

        out
    };
    let mut header = 2u32.to_be_bytes().to_vec();

    header.extend_from_slice(&1u32.to_be_bytes());
    header.extend_from_slice(&[8, 3, 0, 0, 0]);

    let palette: Vec<u8> = colours
        .iter()
        .flat_map(|&(red, green, blue)| [red, green, blue])
        .collect();
    let mut art = crate::png::SIGNATURE.to_vec();

    art.extend_from_slice(&piece(b"IHDR", &header));
    art.extend_from_slice(&piece(b"PLTE", &palette));

    if !alphas.is_empty() {
        art.extend_from_slice(&piece(b"tRNS", alphas));
    }

    art.extend_from_slice(&piece(b"IDAT", &deflated(raw)));
    art.extend_from_slice(&piece(b"IEND", &[]));

    art
}

/// A stage Blorb: a 2x1 PNG, a 24x16 Rect, Reso, release 9.
///
/// The Reso standard window is 640x400 -- roomier than the MCGA
/// default, proving the stage takes the art's own word -- and
/// picture 1 carries a standard ratio of 2, so its drawn size
/// doubles even on the standard window itself.
fn staged_resources() -> Resources {
    let art = indexed_png(&[(10, 20, 30), (40, 50, 60)], &[], &[0, 0, 1]);
    let mut rect = 24u32.to_be_bytes().to_vec();

    rect.extend_from_slice(&16u32.to_be_bytes());

    let reln = chunk(b"RelN", &9u16.to_be_bytes());
    let reso_words: Vec<u8> = [640u32, 400, 640, 400, 640, 400, 1, 2, 1, 0, 0, 0, 0]
        .iter()
        .flat_map(|word| word.to_be_bytes())
        .collect();
    let reso = chunk(b"Reso", &reso_words);
    let ridx_size = 8 + 4 + 2 * 12;
    let png_offset = 12 + ridx_size + reln.len() + reso.len();
    let rect_offset = png_offset + 8 + art.len();
    let mut index = 2u32.to_be_bytes().to_vec();

    index.extend_from_slice(b"Pict");
    index.extend_from_slice(&1u32.to_be_bytes());
    index.extend_from_slice(&(png_offset as u32).to_be_bytes());
    index.extend_from_slice(b"Pict");
    index.extend_from_slice(&2u32.to_be_bytes());
    index.extend_from_slice(&(rect_offset as u32).to_be_bytes());

    let mut body = b"IFRS".to_vec();

    body.extend_from_slice(&chunk(b"RIdx", &index));
    body.extend_from_slice(&reln);
    body.extend_from_slice(&reso);
    body.extend_from_slice(&chunk(b"PNG ", &art));
    body.extend_from_slice(&chunk(b"Rect", &rect));

    Resources::new(Some(Blorb::parse(&chunk(b"FORM", &body)).unwrap()))
}

/// A stage Blorb in the APal style: two scenes and one chrome.
///
/// Pictures 1 and 3 are scenes wearing full palettes of their own;
/// picture 2 is the adaptive chrome the APal chunk names, its stub
/// palette waiting on whatever scene plots first.
fn adaptive_resources() -> Resources {
    let scene = indexed_png(&[(200, 0, 0), (0, 200, 0)], &[], &[0, 0, 1]);
    let stub = indexed_png(&[(1, 2, 3), (4, 5, 6)], &[0], &[0, 0, 1]);
    let other = indexed_png(&[(0, 0, 200), (200, 200, 0)], &[], &[0, 0, 1]);
    let apal = chunk(b"APal", &2u32.to_be_bytes());
    let wrapped: Vec<Vec<u8>> = [&scene, &stub, &other]
        .iter()
        .map(|art| chunk(b"PNG ", art))
        .collect();
    let ridx_size = 8 + 4 + 3 * 12;
    let mut at = 12 + ridx_size + apal.len();
    let mut index = 3u32.to_be_bytes().to_vec();

    for (number, held) in (1u32..=3).zip(&wrapped) {
        index.extend_from_slice(b"Pict");
        index.extend_from_slice(&number.to_be_bytes());
        index.extend_from_slice(&(at as u32).to_be_bytes());

        at += held.len();
    }

    let mut body = b"IFRS".to_vec();

    body.extend_from_slice(&chunk(b"RIdx", &index));
    body.extend_from_slice(&apal);

    for held in &wrapped {
        body.extend_from_slice(held);
    }

    Resources::new(Some(Blorb::parse(&chunk(b"FORM", &body)).unwrap()))
}

/// A stage session fronting a Version 6 machine at its main
/// routine (§5.4): the code inside a routine at $100, the read
/// buffers and a tiny dictionary planted as the two-window helper
/// plants them.
fn staged_session(code: &[u8], resources: Option<Resources>, words: &[(usize, u16)]) -> Session {
    let mut frontend = GlkOteFrontend::staged(6, resources).unwrap();

    frontend.begin(&stage_init()).unwrap();

    let mut data = vec![0u8; 512];

    data[0] = 6;
    data[0x04..0x06].copy_from_slice(&0x01C0u16.to_be_bytes());
    data[0x06..0x08].copy_from_slice(&0x0040u16.to_be_bytes());
    data[0x0C..0x0E].copy_from_slice(&0x0080u16.to_be_bytes());
    data[0x0E..0x10].copy_from_slice(&0x01C0u16.to_be_bytes());
    data[0x100] = 0x00;
    data[0x101..0x101 + code.len()].copy_from_slice(code);

    for &(offset, value) in words {
        data[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
    }

    let mut session = Session::open(Story::new(data).unwrap(), frontend, None).unwrap();
    let memory = session.machine().memory_mut();

    memory.write_byte(TEXT_BUFFER, 21).unwrap();
    memory.write_byte(PARSE_BUFFER, 5).unwrap();
    memory.write_word(0x08, DICTIONARY_BASE as u16).unwrap();

    for (offset, value) in [2u8, b',', b'.', 0, 0, 0].into_iter().enumerate() {
        memory.write_byte(DICTIONARY_BASE + offset, value).unwrap();
    }

    session
}

/// The canvas draw ops of an update.
fn stage_ops(update: &Object) -> Vec<Object> {
    let mut ops = Vec::new();

    if let Some(Value::List(content)) = update.get("content") {
        for held in content {
            if let Value::Object(entry) = held
                && let Some(Value::List(draw)) = entry.get("draw")
            {
                for op in draw {
                    if let Value::Object(op) = op {
                        ops.push(op.clone());
                    }
                }
            }
        }
    }

    ops
}

// The stage opens at the art's own size: the Reso standard window
// when the Blorb names one, MCGA's 320 by 200 without -- and a
// display that never learned the dialect is refused at the door.
#[test]
fn the_stage_opens_at_the_arts_own_size() {
    let mut bare = GlkOteFrontend::staged(6, None).unwrap();

    assert!(bare.stage.is_some());
    assert!(bare.has_colours);
    assert!(!bare.stage.as_ref().unwrap().has_pictures);
    assert_eq!(bare.screen_columns, 40);
    assert_eq!(bare.screen_lines, 25);
    assert!(bare.stage_picture_data(1).unwrap().is_none());

    // A galleryless stage draws nothing, quietly.
    bare.stage_draw_picture(1, 1, 1).unwrap();

    let mut dressed = GlkOteFrontend::staged(6, Some(staged_resources())).unwrap();

    dressed.begin(&stage_init()).unwrap();

    assert_eq!(dressed.screen_columns, 80);
    assert_eq!(dressed.screen_lines, 50);
    assert!(dressed.stage.as_ref().unwrap().has_pictures);
    assert!(dressed.has_sounds);

    let refused = GlkOteFrontend::staged(6, None)
        .unwrap()
        .begin(&init())
        .unwrap_err();

    assert!(refused.to_string().contains("never learned the stage"));
}

// One scaled canvas carries the whole stage: the window entry
// names the art's logical space under the display's box, the
// opening curtain papers it, and the story's text lands as
// placed, coalesced text ops in the §8.8 units.
#[test]
fn the_stage_renders_one_scaled_canvas() {
    let mut session = staged_session(&[0xBA], None, &[]);
    let mut face = front(&session);

    face.write("Hi");
    face.write_rectangle(&["!!".to_string()]);
    session.machine().run().unwrap();

    let update = session.render(true).unwrap();
    let window = entry(&items(at(&update, "windows"))[0]);

    assert_eq!(str_of(at(window, "type")), "graphics");
    assert_eq!(at(window, "scaled"), &Value::Bool(true));
    assert_eq!(int_of(at(window, "graphwidth")), 320);
    assert_eq!(int_of(at(window, "graphheight")), 200);
    assert_eq!(int_of(at(window, "width")), 1280);
    assert_eq!(int_of(at(window, "height")), 800);

    let ops = stage_ops(&update);

    assert_eq!(
        told(&Value::Object(ops[0].clone())),
        r##"{"special":"setcolor","color":"#000000"}"##
    );
    assert_eq!(
        told(&Value::Object(ops[1].clone())),
        r##"{"special":"fill","x":0,"y":0,"width":320,"height":200,"color":"#000000"}"##
    );
    assert_eq!(
        told(&Value::Object(ops[2].clone())),
        r##"{"special":"text","x":0,"y":0,"text":"Hi!!","cell":[8,8],"fg":"#ffffff","bg":"#000000"}"##
    );
}

// The dress travels resolved: colours as the shared palette's CSS,
// reverse video pre-swapped, bold and italic as flags -- and the
// under-cursor sample reads the painted stage, which here is the
// opening curtain's black.
#[test]
fn stage_text_wears_its_dress() {
    let mut session = staged_session(&[0xBA], None, &[]);
    let mut face = front(&session);

    face.set_style(BOLD);
    face.write("B");
    face.set_style(0);
    face.set_style(ITALIC);
    face.set_colour(3, 4);
    face.write("i");
    face.set_style(0);
    face.set_style(REVERSE);
    face.write("r");
    face.set_style(0);
    face.set_colour(-1, -1);
    face.write("s");
    session.machine().run().unwrap();

    let ops = stage_ops(&session.render(true).unwrap());
    let texts: Vec<&Object> = ops
        .iter()
        .filter(|op| op.get("special").and_then(Value::as_str) == Some("text"))
        .collect();

    assert_eq!(str_of(at(texts[0], "text")), "B");
    assert_eq!(at(texts[0], "bold"), &Value::Bool(true));
    assert_eq!(str_of(at(texts[1], "text")), "i");
    assert_eq!(str_of(at(texts[1], "fg")), "#cc0000");
    assert_eq!(str_of(at(texts[1], "bg")), "#00cc00");
    assert_eq!(at(texts[1], "italic"), &Value::Bool(true));
    assert_eq!(str_of(at(texts[2], "text")), "r");
    assert_eq!(str_of(at(texts[2], "fg")), "#00cc00");
    assert_eq!(str_of(at(texts[2], "bg")), "#cc0000");
    assert_eq!(str_of(at(texts[3], "text")), "s");
    assert_eq!(str_of(at(texts[3], "fg")), "#000000");
    assert_eq!(str_of(at(texts[3], "bg")), "#000000");
}

// The eight-window geometry lands where the game placed it: a
// placed window's text paints at its absolute units, the scroll
// slides as a shift op, and the pixel-width erase-line fills.
#[test]
fn the_stage_forwards_the_eight_window_ops() {
    let mut session = staged_session(&[0xBA], None, &[]);
    let mut face = front(&session);

    face.place_window(2, 41, 17, 64, 128);
    face.set_window(2);
    face.set_cursor(1, 1);
    face.set_font(4);
    face.set_buffering(false);
    face.write("W");
    face.erase_line(Some(16));
    face.scroll_window(2, 8);
    face.set_margins(2, 0, 0);
    face.set_line_count(2, -999);
    face.split_window(0);
    session.machine().run().unwrap();

    assert_eq!(face.cursor_position(), (1, 9));

    // A single window's erasure homes it and keeps any chrome.
    face.erase_window(2);

    let ops = stage_ops(&session.render(true).unwrap());
    let placed = ops
        .iter()
        .find(|op| op.get("special").and_then(Value::as_str) == Some("text"))
        .unwrap();
    let shift = ops
        .iter()
        .find(|op| op.get("special").and_then(Value::as_str) == Some("shift"))
        .unwrap();
    let fill_widths: Vec<i64> = ops
        .iter()
        .filter(|op| op.get("special").and_then(Value::as_str) == Some("fill"))
        .map(|op| int_of(at(op, "width")))
        .collect();

    assert_eq!(str_of(at(placed, "text")), "W");
    assert_eq!(int_of(at(placed, "x")), 16);
    assert_eq!(int_of(at(placed, "y")), 40);
    assert_eq!(int_of(at(shift, "rise")), 8);
    assert!(fill_widths.contains(&16));

    // §8.2 has no line on a stage; the fault surfaces at render.
    face.show_status(&Status {
        location: "Here".to_string(),
        score: 0,
        turns: 0,
        time_game: false,
    });

    assert!(
        session
            .render(false)
            .unwrap_err()
            .to_string()
            .contains("no line")
    );
}

// The pictures draw Reso-scaled at their unit positions, in the
// turn's true order against the flowing text; a Rect placard has
// a size for layout but no bytes, and an unknown number draws and
// erases nothing at all.
#[test]
fn the_stage_draws_its_pictures() {
    let mut session = staged_session(&[0xBA], Some(staged_resources()), &[]);
    let mut face = front(&session);

    assert_eq!(face.picture_census(), (2, 9));
    assert_eq!(face.picture_data(1), Some((2, 4)));
    assert_eq!(face.picture_data(2), Some((16, 24)));
    assert_eq!(face.picture_data(7), None);

    face.write("A");
    face.draw_picture(1, 11, 21);
    face.write("B");
    face.draw_picture(2, 1, 1);
    face.draw_picture(7, 1, 1);
    face.erase_picture(1, 11, 21);
    face.erase_picture(7, 1, 1);
    session.machine().run().unwrap();

    let ops = stage_ops(&session.render(true).unwrap());
    let kinds: Vec<&str> = ops
        .iter()
        .filter_map(|op| op.get("special").and_then(Value::as_str))
        .filter(|kind| *kind != "setcolor")
        .collect();
    let image = ops
        .iter()
        .find(|op| op.get("special").and_then(Value::as_str) == Some("image"))
        .unwrap();
    let papered = ops.last().unwrap();

    assert_eq!(kinds, vec!["fill", "text", "image", "text", "fill"]);
    assert_eq!(int_of(at(image, "image")), 1);
    assert!(str_of(at(image, "url")).starts_with("data:image/png;base64,"));
    assert_eq!(int_of(at(image, "x")), 20);
    assert_eq!(int_of(at(image, "y")), 10);
    assert_eq!(int_of(at(image, "width")), 4);
    assert_eq!(int_of(at(image, "height")), 2);
    assert_eq!(int_of(at(papered, "width")), 4);
    assert_eq!(int_of(at(papered, "height")), 2);
}

// A line read asks at the stage's own cursor with the editor's
// cell, the table's nameable terminators offered and the click
// armed when the table names it -- and the landed line echoes
// onto the stage, though a terminator-ended one stays uncommitted.
#[test]
fn the_stage_asks_and_echoes_the_line() {
    let mut session = staged_session(AREAD, None, &[(0x2E, 0x01A0)]);

    session
        .machine()
        .memory_mut()
        .write_byte(0x01A0, 133)
        .unwrap();
    session
        .machine()
        .memory_mut()
        .write_byte(0x01A1, 254)
        .unwrap();
    front(&session).write("> ");
    session.machine().run().unwrap();

    let update = session.render(false).unwrap();
    let asked = entry(&items(at(&update, "input"))[0]);

    assert_eq!(str_of(at(asked, "type")), "line");
    assert_eq!(int_of(at(asked, "maxlen")), 21);
    assert_eq!(int_of(at(asked, "xpos")), 16);
    assert_eq!(int_of(at(asked, "ypos")), 0);
    assert_eq!(told(at(asked, "cell")), "[8,8]");
    assert_eq!(str_of(at(asked, "ink")), "#ffffff");
    assert_eq!(told(at(asked, "terminators")), r#"["func1"]"#);
    assert_eq!(at(asked, "mouse"), &Value::Bool(true));

    let generation = int_of(at(&update, "gen"));
    let verdict = session
        .accept(&parsed(&format!(
            r#"{{"type":"line","gen":{generation},"value":"go"}}"#
        )))
        .unwrap();

    assert_eq!(verdict, Verdict::Advance);

    session.machine().run().unwrap();

    let echoed = stage_ops(&session.render(true).unwrap())
        .into_iter()
        .find(|op| op.get("special").and_then(Value::as_str) == Some("text"))
        .unwrap();

    assert_eq!(str_of(at(&echoed, "text")), "go");
    assert_eq!(int_of(at(&echoed, "x")), 16);

    let mut quiet = staged_session(AREAD, None, &[(0x2E, 0x01A0)]);

    quiet
        .machine()
        .memory_mut()
        .write_byte(0x01A0, 133)
        .unwrap();
    quiet.machine().run().unwrap();

    let asked = quiet.render(false).unwrap();
    let generation = int_of(at(&asked, "gen"));

    quiet
        .accept(&parsed(&format!(
            r#"{{"type":"line","gen":{generation},"value":"held","terminator":"func1"}}"#
        )))
        .unwrap();
    quiet.machine().run().unwrap();

    let silent = stage_ops(&quiet.render(true).unwrap());

    assert!(
        !silent
            .iter()
            .any(|op| op.get("special").and_then(Value::as_str) == Some("text"))
    );
}

// A keystroke read is an invisible focus target that hears clicks
// the way it hears any key: the canvas's own click lands as the
// §10.3 single-click code, one unit step over, while a click on
// some other window -- or before any canvas stands -- passes.
#[test]
fn the_stage_hears_keys_and_clicks() {
    let mut unborn = staged_session(READ_CHAR, None, &[]);

    assert_eq!(
        unborn
            .accept(&parsed(
                r#"{"type":"mouse","gen":0,"window":9,"x":1,"y":1}"#
            ))
            .unwrap(),
        Verdict::Pass
    );

    let mut session = staged_session(READ_CHAR, None, &[]);

    session.machine().run().unwrap();

    let update = session.render(false).unwrap();
    let canvas = int_of(at(entry(&items(at(&update, "windows"))[0]), "id"));

    assert_eq!(
        told(at(&update, "input")),
        format!(r#"[{{"id":{canvas},"type":"char","mouse":true,"gen":1}}]"#)
    );

    let astray = parsed(&format!(
        r#"{{"type":"mouse","gen":1,"window":{},"x":1,"y":1}}"#,
        canvas + 9
    ));

    assert_eq!(session.accept(&astray).unwrap(), Verdict::Pass);

    let landed = parsed(&format!(
        r#"{{"type":"mouse","gen":1,"window":{canvas},"x":9,"y":15}}"#
    ));

    assert_eq!(session.accept(&landed).unwrap(), Verdict::Advance);

    session.machine().run().unwrap();

    assert_eq!(session.machine().memory_mut().read_word(0x80).unwrap(), 254);
}

// An arrange re-boxes the canvas without the machine hearing a
// word -- the units never move -- and a redraw replays the
// journal: everything since the last whole-stage fill, the scene
// papered first, the pre-scene paints gone for good. A refresh
// replays it with the windows resent.
#[test]
fn the_stage_reshapes_and_replays() {
    let mut session = staged_session(READ_CHAR, None, &[]);

    front(&session).write("old");
    session.machine().run().unwrap();
    session.render(false).unwrap();

    front(&session).erase_window(-1);
    front(&session).write("new");

    let second = session.render(false).unwrap();

    assert_eq!(
        stage_ops(&second)[0].get("special").and_then(Value::as_str),
        Some("fill")
    );

    let generation = int_of(at(&second, "gen"));
    let reboxed = parsed(&format!(
        r#"{{"type":"arrange","gen":{generation},"metrics":{{"width":640,"height":400}}}}"#
    ));

    assert_eq!(session.accept(&reboxed).unwrap(), Verdict::Stand);

    let resized = session.render(false).unwrap();
    let window = entry(&items(at(&resized, "windows"))[0]);

    assert_eq!(int_of(at(window, "width")), 640);
    assert_eq!(int_of(at(window, "graphwidth")), 320);

    let generation = int_of(at(&resized, "gen"));
    let redraw = parsed(&format!(
        r#"{{"type":"redraw","gen":{generation},"window":2}}"#
    ));

    assert_eq!(session.accept(&redraw).unwrap(), Verdict::Stand);

    let replayed = stage_ops(&session.render(false).unwrap());
    let texts: Vec<&str> = replayed
        .iter()
        .filter_map(|op| op.get("text").and_then(Value::as_str))
        .collect();

    assert_eq!(
        replayed[0].get("special").and_then(Value::as_str),
        Some("setcolor")
    );
    assert!(!texts.contains(&"old"));
    assert!(texts.contains(&"new"));

    assert_eq!(
        session.accept(&parsed(r#"{"type":"refresh"}"#)).unwrap(),
        Verdict::Stand
    );

    let told_whole = session.render(false).unwrap();

    assert!(told_whole.get("windows").is_some());
    assert!(
        stage_ops(&told_whole)
            .iter()
            .filter_map(|op| op.get("text").and_then(Value::as_str))
            .any(|text| text == "new")
    );
}

// The guards hold at the pointers on the stage too: a click with
// only a file ask standing passes at the canvas.
#[test]
fn misaimed_stage_clicks_pass() {
    let mut session = staged_session(SAVED, None, &[]);

    session.machine().run().unwrap();

    let update = session.render(false).unwrap();
    let canvas = int_of(at(entry(&items(at(&update, "windows"))[0]), "id"));
    let generation = int_of(at(&update, "gen"));
    let tapped = parsed(&format!(
        r#"{{"type":"mouse","gen":{generation},"window":{canvas},"x":1,"y":1}}"#
    ));

    assert_eq!(session.accept(&tapped).unwrap(), Verdict::Pass);
}

// The chrome wears the scene: a scene's plot absorbs its palette
// and the standing chrome re-plots in the Current Palette -- new
// bytes at the same position, the wire's spelling of Infocom's
// hardware recolouring -- while encodings are remembered per
// palette era and a whole-screen erasure takes the chrome along.
#[test]
fn the_stage_chrome_wears_the_scene() {
    let mut session = staged_session(READ_CHAR, Some(adaptive_resources()), &[]);
    let mut face = front(&session);

    face.draw_picture(1, 1, 1);
    face.draw_picture(2, 1, 9);
    face.draw_picture(2, 1, 9);
    face.draw_picture(3, 9, 1);
    session.machine().run().unwrap();

    let update = session.render(false).unwrap();
    let images: Vec<Object> = stage_ops(&update)
        .into_iter()
        .filter(|op| op.get("special").and_then(Value::as_str) == Some("image"))
        .collect();
    let numbers: Vec<i64> = images.iter().map(|op| int_of(at(op, "image"))).collect();

    assert_eq!(numbers, vec![1, 2, 2, 3, 2]);
    assert_eq!(str_of(at(&images[1], "url")), str_of(at(&images[2], "url")));
    assert_ne!(str_of(at(&images[4], "url")), str_of(at(&images[1], "url")));
    assert_eq!(int_of(at(&images[4], "x")), 8);
    assert_eq!(int_of(at(&images[4], "y")), 0);

    face.erase_window(-1);
    face.draw_picture(1, 1, 1);

    let numbers: Vec<i64> = stage_ops(&session.render(false).unwrap())
        .into_iter()
        .filter(|op| op.get("special").and_then(Value::as_str) == Some("image"))
        .map(|op| int_of(at(&op, "image")))
        .collect();

    assert_eq!(numbers, vec![1]);
}

// §8.3.1's under-cursor sample reads the painted stage itself:
// over a plotted picture the art's own pixel answers, a chrome's
// transparent hole deferring to the scene beneath, and the minted
// colour dresses the following text -- how Zork Zero's status
// text sits on its ribbons without a seam. One colour mints once.
#[test]
fn the_stage_samples_its_own_paint() {
    let mut session = staged_session(READ_CHAR, Some(adaptive_resources()), &[]);
    let mut face = front(&session);

    face.draw_picture(1, 9, 17);
    face.draw_picture(2, 9, 17);
    face.set_cursor(9, 17);
    face.set_colour(-1, -1);
    face.write("s");
    face.set_cursor(9, 17);
    face.set_colour(-1, -1);
    face.write("t");
    session.machine().run().unwrap();

    let sampled: Vec<Object> = stage_ops(&session.render(false).unwrap())
        .into_iter()
        .filter(|op| {
            matches!(
                op.get("text").and_then(Value::as_str),
                Some("s") | Some("t")
            )
        })
        .collect();

    assert_eq!(sampled.len(), 2);

    for held in &sampled {
        assert_eq!(str_of(at(held, "fg")), "#c80000");
        assert_eq!(str_of(at(held, "bg")), "#c80000");
    }
}

// The point sample walks the paint newest-first: a fill answers
// its colour inside its rectangle and defers outside it, an image
// without a gallery -- or naming art the gallery cannot decode --
// is passed over, and paint never laid answers the default paper.
#[test]
fn plotted_answers_the_top_paint() {
    let fill = parsed(r##"{"special":"fill","x":0,"y":0,"width":4,"height":4,"color":"#123456"}"##);
    let dye = parsed(r##"{"special":"setcolor","color":"#ffffff"}"##);
    let astray = parsed(r#"{"special":"image","image":9,"x":0,"y":0,"width":4,"height":4}"#);

    assert_eq!(plotted(&[], &[], 0, 0, None).unwrap(), "#000000");
    assert_eq!(
        plotted(&[], &[fill.clone(), dye], 1, 1, None).unwrap(),
        "#123456"
    );
    assert_eq!(
        plotted(&[], std::slice::from_ref(&fill), 9, 9, None).unwrap(),
        "#000000"
    );
    assert_eq!(
        plotted(&[], &[fill.clone(), astray.clone()], 1, 1, None).unwrap(),
        "#123456"
    );

    let mut gallery = Gallery::new(
        std::collections::BTreeMap::new(),
        0,
        None,
        std::collections::HashSet::new(),
        HashMap::new(),
    );

    assert_eq!(
        plotted(&[], &[fill, astray], 1, 1, Some(&mut gallery)).unwrap(),
        "#123456"
    );
}

// A save asks for its file through the protocol's special input, a
// restore asks to read -- and the cancel is delivered like any
// player answer.
#[test]
fn the_stage_asks_for_its_file() {
    let mut session = staged_session(SAVED, None, &[]);

    session.machine().run().unwrap();

    let update = session.render(false).unwrap();

    assert_eq!(
        told(at(&update, "specialinput")),
        r#"{"type":"fileref_prompt","filemode":"write","filetype":"save"}"#
    );

    let generation = int_of(at(&update, "gen"));
    let verdict = session
        .accept(&parsed(&format!(
            r#"{{"type":"specialresponse","gen":{generation},"response":"fileref_prompt"}}"#
        )))
        .unwrap();

    assert_eq!(verdict, Verdict::Advance);

    let mut reader = staged_session(RESTORED, None, &[]);

    reader.machine().run().unwrap();

    let asked = reader.render(false).unwrap();

    assert_eq!(
        str_of(at(entry(at(&asked, "specialinput")), "filemode")),
        "read"
    );
}
