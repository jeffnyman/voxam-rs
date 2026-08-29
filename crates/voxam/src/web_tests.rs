//! The browser face: one session served over HTTP, turn by turn.

use super::*;
use voxam_core::iff::chunk;

// The suspension story from the machine tests: open a buffer, ask
// for a keystroke, select, quit on the far side of the resume.
const AWAITS_KEY: &[u8] = &[
    0xC0, 0x00, 0x00, //
    0x40, 0x81, 0x00, //
    0x40, 0x81, 0x03, //
    0x40, 0x81, 0x00, //
    0x40, 0x81, 0x00, //
    0x40, 0x81, 0x00, //
    0x81, 0x30, 0x11, 0x06, 0x23, 0x05, 0x01, 0x40, //
    0x40, 0x86, 0x01, 0x40, //
    0x81, 0x30, 0x12, 0x00, 0x00, 0xD2, 0x01, //
    0x40, 0x82, 0x01, 0xC0, //
    0x81, 0x30, 0x12, 0x00, 0x00, 0xC0, 0x01, //
    0x81, 0x20, //
];

// A story that asks the player for a save file and quits.
const PROMPTS: &[u8] = &[
    0xC0, 0x00, 0x00, //
    0x40, 0x81, 0x00, //
    0x40, 0x81, 0x01, //
    0x40, 0x81, 0x01, //
    0x81, 0x30, 0x11, 0x06, 0x62, 0x03, 0x01, 0x40, //
    0x81, 0x20, //
];

const INIT: &str = r#"{"type":"init","gen":0,"support":["timer","graphicswin","hyperlinks"],"metrics":{"width":80,"height":24}}"#;

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

fn told(stanza: &Object) -> String {
    dumps(&Value::Object(stanza.clone()))
}

/// A tiny valid Glulx image, checksummed.
fn glulx_image(code: &[u8]) -> Vec<u8> {
    let mut data = vec![0u8; 0x200];

    data[0..4].copy_from_slice(b"Glul");
    data[4..8].copy_from_slice(&0x0003_0102u32.to_be_bytes());
    data[8..12].copy_from_slice(&0x100u32.to_be_bytes());
    data[12..16].copy_from_slice(&0x200u32.to_be_bytes());
    data[16..20].copy_from_slice(&0x300u32.to_be_bytes());
    data[20..24].copy_from_slice(&0x100u32.to_be_bytes());
    data[24..28].copy_from_slice(&0x48u32.to_be_bytes());
    data[0x48..0x48 + code.len()].copy_from_slice(code);

    let checksum = (0..data.len()).step_by(4).fold(0u32, |total, held| {
        total.wrapping_add(u32::from_be_bytes([
            data[held],
            data[held + 1],
            data[held + 2],
            data[held + 3],
        ]))
    });

    data[32..36].copy_from_slice(&checksum.to_be_bytes());

    data
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut held = b"\x89PNG\r\n\x1a\n".to_vec();

    held.extend(13u32.to_be_bytes());
    held.extend(b"IHDR");
    held.extend(width.to_be_bytes());
    held.extend(height.to_be_bytes());

    held
}

fn jpeg(width: u32, height: u32) -> Vec<u8> {
    let mut held = b"\xff\xd8".to_vec();

    held.extend(b"\xff\xc0\x00\x08\x08");
    held.extend((height as u16).to_be_bytes());
    held.extend((width as u16).to_be_bytes());

    held
}

/// A Blorb holding one PNG, one JPEG, and one placeholder.
fn pictured() -> Blorb {
    type Row = ([u8; 4], u32, [u8; 4], Vec<u8>);

    let entries: [Row; 3] = [
        (*b"Pict", 1, *b"PNG ", png(2, 2)),
        (*b"Pict", 2, *b"JPEG", jpeg(4, 4)),
        (*b"Pict", 3, *b"Rect", vec![0; 8]),
    ];
    let mut index = (entries.len() as u32).to_be_bytes().to_vec();
    let mut body = Vec::new();
    let mut offset = 12 + 8 + 4 + 12 * entries.len();

    for (usage, number, chunk_id, payload) in &entries {
        index.extend(usage);
        index.extend(number.to_be_bytes());
        index.extend((offset as u32).to_be_bytes());

        let framed = chunk(chunk_id, payload);

        offset += framed.len();
        body.extend(framed);
    }

    let mut form = b"IFRS".to_vec();

    form.extend(chunk(b"RIdx", &index));
    form.extend(body);

    Blorb::parse(&chunk(b"FORM", &form)).unwrap()
}

/// A Face over a fresh session of the keystroke story.
fn faced(caption: Option<&str>) -> Face {
    Face::new(
        Session::glulx(
            GlulxStory::new(glulx_image(AWAITS_KEY)).unwrap(),
            Some(pictured()),
            Some(7),
        ),
        caption,
    )
}

fn named_face() -> Face {
    faced(Some("Sensory Jam — Voxam"))
}

/// One POST /event through the Face, the answer parsed back.
fn posted(face: &mut Face, body: &str) -> Object {
    let (status, kind, payload) = face.respond("POST", "/event", body.as_bytes());

    assert_eq!(status, 200);
    assert_eq!(kind, "application/json");

    parsed(&String::from_utf8(payload).unwrap())
}

// The page arrives wearing the story's own name -- and the plain
// Voxam name when no record or catalog could offer one.
#[test]
fn the_page_wears_the_story_name() {
    let (status, kind, payload) = named_face().respond("GET", "/", b"");
    let told = String::from_utf8(payload).unwrap();

    assert_eq!(status, 200);
    assert_eq!(kind, "text/html; charset=utf-8");
    assert!(told.contains("<title>Sensory Jam — Voxam</title>"));
    assert!(!told.contains("VOXAM_TITLE"));

    let (_, _, unnamed) = faced(None).respond("GET", "/", b"");

    assert!(
        String::from_utf8(unnamed)
            .unwrap()
            .contains("<title>Voxam</title>")
    );
}

// The display's own files serve under their names and types; the
// license rides in the source tree but is nobody's fetch, and
// unknown roads answer 404.
#[test]
fn the_assets_serve_with_their_types() {
    let mut face = named_face();

    for (name, kind) in [
        ("glkote.js", "text/javascript"),
        ("glkote.css", "text/css"),
        ("jquery-1.12.4.min.js", "text/javascript"),
        ("waiting.gif", "image/gif"),
    ] {
        let (status, served, payload) = face.respond("GET", &format!("/{name}"), b"");

        assert_eq!(status, 200);
        assert_eq!(served, kind);
        assert!(!payload.is_empty());
    }

    assert_eq!(face.respond("GET", "/LICENSE-glkote.txt", b"").0, 404);
    assert_eq!(face.respond("GET", "/nothing", b"").0, 404);
    assert_eq!(face.respond("PUT", "/", b"").0, 404);
}

// Pictures serve by Blorb number with their own content types; a
// placeholder rectangle, a missing number, and a road that names
// no number at all are 404s.
#[test]
fn pictures_serve_by_number() {
    let mut face = named_face();
    let (status, kind, payload) = face.respond("GET", "/pict/1", b"");

    assert_eq!((status, kind.as_str()), (200, "image/png"));
    assert_eq!(&payload[..4], b"\x89PNG");

    let (status, kind, _) = face.respond("GET", "/pict/2", b"");

    assert_eq!((status, kind.as_str()), (200, "image/jpeg"));
    assert_eq!(face.respond("GET", "/pict/3", b"").0, 404);
    assert_eq!(face.respond("GET", "/pict/9", b"").0, 404);
    assert_eq!(face.respond("GET", "/pict/abc", b"").0, 404);
}

// The tab wears the machine's own mark -- the same window icons
// the reference title bars wear: a Glulx face answers the glulx
// icon, a Z face its version's own, and the page asks by link.
#[test]
fn the_tab_wears_the_machine_icon() {
    let (status, kind, payload) = named_face().respond("GET", "/favicon.ico", b"");

    assert_eq!((status, kind.as_str()), (200, "image/x-icon"));
    assert_eq!(&payload[..4], b"\x00\x00\x01\x00");

    let mut zed = Face::new(Session::z(z_story(), Some(pictured()), Some(7)), None);

    assert_eq!(zed.session.icon, "z4.ico");
    assert_eq!(
        &zed.respond("GET", "/favicon.ico", b"").2[..4],
        b"\x00\x00\x01\x00"
    );

    let (_, _, page) = named_face().respond("GET", "/", b"");

    assert!(String::from_utf8(page).unwrap().contains("rel=\"icon\""));
}

// A whole turn travels by POST: the init births the session and
// answers the first update, the keystroke answers the exit.
#[test]
fn a_turn_travels_by_post() {
    let mut face = named_face();
    let first = posted(&mut face, INIT);

    assert_eq!(at(&first, "type").as_str(), Some("update"));
    assert_eq!(at(&first, "gen").as_int(), Some(1));
    assert_eq!(items(at(&first, "windows")).len(), 1);
    assert_eq!(
        dumps(at(&first, "input")),
        r#"[{"id":1,"type":"char","gen":1}]"#
    );

    let last = posted(
        &mut face,
        r#"{"type":"char","gen":1,"window":1,"value":"A"}"#,
    );

    assert_eq!(
        told(&last),
        r#"{"type":"update","gen":2,"input":[],"exit":true}"#
    );
}

// A stale event draws the pass, exactly as the stdio face answers.
#[test]
fn a_stale_event_passes() {
    let mut face = named_face();

    posted(&mut face, INIT);

    assert_eq!(
        told(&posted(
            &mut face,
            r#"{"type":"char","gen":0,"window":1,"value":"A"}"#
        )),
        r#"{"type":"pass"}"#
    );
}

// A reload is a fresh init, and a fresh init starts the story
// over: new machine, new windows, generation one again -- even
// after the last one ended, and even after a fault.
#[test]
fn a_reload_starts_the_story_over() {
    let mut face = named_face();

    posted(&mut face, INIT);
    posted(
        &mut face,
        r#"{"type":"char","gen":1,"window":1,"value":"A"}"#,
    );

    let reborn = posted(&mut face, INIT);

    assert_eq!(at(&reborn, "gen").as_int(), Some(1));
    assert_eq!(items(at(&reborn, "windows")).len(), 1);
}

// A fault answers the protocol's error stanza and keeps answering
// it -- the session is dead until a reload -- and an event before
// any init is told where conversations begin.
#[test]
fn a_fault_holds_until_the_reload() {
    let mut face = named_face();

    posted(&mut face, INIT);

    let fault = posted(
        &mut face,
        r#"{"type":"line","gen":1,"window":1,"value":"go"}"#,
    );

    assert_eq!(at(&fault, "type").as_str(), Some("error"));
    assert!(
        at(&fault, "message")
            .as_str()
            .unwrap()
            .contains("not expecting")
    );

    let again = posted(
        &mut face,
        r#"{"type":"char","gen":1,"window":1,"value":"A"}"#,
    );

    assert_eq!(told(&again), told(&fault));
    assert_eq!(
        posted(&mut face, INIT).get("gen").and_then(Value::as_int),
        Some(1)
    );

    let mut fresh = named_face();
    let unopened = posted(&mut fresh, r#"{"type":"char","gen":0}"#);

    assert!(
        at(&unopened, "message")
            .as_str()
            .unwrap()
            .contains("opens with an init")
    );
}

// A game's ask for a file crosses the wire as special input, and
// the posted answer resumes the turn -- no event delivered, the
// call itself was the destination.
#[test]
fn a_file_ask_crosses_the_wire() {
    let mut face = Face::new(
        Session::glulx(
            GlulxStory::new(glulx_image(PROMPTS)).unwrap(),
            Some(pictured()),
            Some(7),
        ),
        Some("Saves — Voxam"),
    );
    let first = posted(&mut face, INIT);

    assert_eq!(
        dumps(at(&first, "specialinput")),
        r#"{"type":"fileref_prompt","filemode":"write","filetype":"save"}"#
    );

    let done = posted(
        &mut face,
        r#"{"type":"specialresponse","gen":1,"response":"fileref_prompt","value":"saga"}"#,
    );

    assert_eq!(told(&done), r#"{"type":"update","gen":2,"exit":true}"#);
}

// What is not JSON, and JSON that is not a stanza, answer 200 with
// the protocol's own error stanza: the display renders that far
// better than a bare status would.
#[test]
fn garbage_posts_answer_in_kind() {
    let mut face = named_face();

    assert!(
        at(&posted(&mut face, "{nope"), "message")
            .as_str()
            .unwrap()
            .contains("not JSON")
    );
    assert!(
        at(&posted(&mut face, "[1, 2]"), "message")
            .as_str()
            .unwrap()
            .contains("a stanza is a JSON object")
    );

    let (status, _, payload) = face.respond("POST", "/event", b"\xff\xfe");

    assert_eq!(status, 200);
    assert!(String::from_utf8(payload).unwrap().contains("not JSON"));
}

/// A Version 4 story that reads one line and quits, whole.
fn z_story() -> ZStory {
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

    ZStory::new(data).unwrap()
}

// A Z-Machine story serves through the same face: init births a
// machine over the screen model, a posted line echoes and answers,
// and a reload starts the story over.
#[test]
fn a_z_story_serves_through_the_face() {
    let mut face = Face::new(
        Session::z(z_story(), Some(pictured()), Some(7)),
        Some("Sensory Jam — Voxam"),
    );
    let first = posted(&mut face, INIT);

    assert_eq!(at(&first, "gen").as_int(), Some(1));
    assert_eq!(
        dumps(at(&first, "input")),
        r#"[{"id":1,"type":"line","maxlen":6,"gen":1}]"#
    );

    let done = posted(
        &mut face,
        r#"{"type":"line","gen":1,"window":1,"value":"look"}"#,
    );

    assert_eq!(done.get("exit"), Some(&Value::Bool(true)));

    let echoed = items(at(entry_of(&items(at(&done, "content"))[0]), "text"))
        .first()
        .map(|held| dumps(at(entry_of(held), "content")))
        .unwrap();

    assert!(echoed.contains(r#""style":"input""#));
    assert_eq!(
        posted(&mut face, INIT).get("gen").and_then(Value::as_int),
        Some(1)
    );
    assert_eq!(
        told(&posted(
            &mut face,
            r#"{"type":"line","gen":0,"window":1,"value":"stale"}"#
        )),
        r#"{"type":"pass"}"#
    );

    // A standing verdict renders the picture as it stands: the
    // arrange moved the boxes, and the update says so.
    let arranged = posted(
        &mut face,
        r#"{"type":"arrange","gen":1,"metrics":{"width":400,"height":200}}"#,
    );

    assert_eq!(at(&arranged, "type").as_str(), Some("update"));
    assert_eq!(
        entry_of(&items(at(&arranged, "windows"))[0])
            .get("width")
            .and_then(Value::as_int),
        Some(400)
    );

    // An event before any init is told where conversations begin.
    let mut unopened = Face::new(Session::z(z_story(), Some(pictured()), Some(7)), None);

    assert!(
        at(
            &posted(&mut unopened, r#"{"type":"line","gen":0,"value":"x"}"#),
            "message"
        )
        .as_str()
        .unwrap()
        .contains("opens with an init")
    );
}

fn entry_of(value: &Value) -> &Object {
    match value {
        Value::Object(held) => held,
        _ => panic!("not an object"),
    }
}

/// An Å-machine session serves through the same face too. (The
/// reference exercises this from its aamachine battery; here it
/// rides beside its siblings.)
#[test]
fn an_aa_story_serves_through_the_face() {
    let story = voxam_core::aamachine::story::Story::new(&aa_story_bytes()).unwrap();
    let mut face = Face::new(Session::aamachine(story, None, Some(7)), None);
    let first = posted(&mut face, INIT);

    assert_eq!(at(&first, "type").as_str(), Some("update"));
    assert_eq!(
        entry_of(&items(at(&first, "input"))[0])
            .get("type")
            .and_then(Value::as_str),
        Some("line")
    );

    let done = posted(
        &mut face,
        r#"{"type":"line","gen":1,"window":1,"value":"onward"}"#,
    );

    assert_eq!(done.get("exit"), Some(&Value::Bool(true)));
}

/// A crafted Å story that prints, waits for a line, then quits --
/// the wire battery's own shape.
fn aa_story_bytes() -> Vec<u8> {
    use voxam_core::aamachine::story::{SUMMED, crc32};

    let code: &[u8] = &[0x65, 0x40, 0x05, 0x73, 0x00, 0x70, 0x00];
    let mut whole = vec![0x01];

    whole.extend_from_slice(code);

    let mut init_chunk = vec![0u8, 0, 0, 1, 0, 1];

    init_chunk.extend_from_slice(&[0, 1]);

    let mut lang = Vec::new();

    for offset in [8u16, 8, 9, 10] {
        lang.extend_from_slice(&offset.to_be_bytes());
    }

    lang.extend_from_slice(&[0, 0, 0, 0, 0, 0]);

    let summed = |name: &[u8; 4]| -> Vec<u8> {
        match name {
            b"LANG" => lang.clone(),
            b"DICT" | b"MAPS" | b"LOOK" => vec![0, 0],
            b"WRIT" => vec![0x80],
            b"INIT" => init_chunk.clone(),
            b"CODE" => whole.clone(),
            _ => Vec::new(),
        }
    };

    let mut crc = 0u32;

    for name in &SUMMED {
        crc = crc32(&summed(name), crc);
    }

    let mut head = vec![0, 5, 2, 0];

    head.extend_from_slice(&1u16.to_be_bytes());
    head.extend_from_slice(b"260827");
    head.extend_from_slice(&crc.to_be_bytes());
    head.extend_from_slice(&32u16.to_be_bytes());
    head.extend_from_slice(&16u16.to_be_bytes());
    head.extend_from_slice(&16u16.to_be_bytes());

    let mut pieces = chunk(b"HEAD", &head);

    for name in &SUMMED {
        pieces.extend(chunk(name, &summed(name)));
    }

    let mut body = b"AAVM".to_vec();

    body.extend(pieces);

    chunk(b"FORM", &body)
}

// The whole server, once, over a real socket: the page, a turn,
// and a wrong road, through the hand-rolled handler shell.
#[test]
fn the_server_answers_over_a_real_socket() {
    let listener = webbed(0).unwrap();
    let port = listener.local_addr().unwrap().port();
    let runner = std::thread::spawn(move || {
        let mut face = named_face();

        for _ in 0..3 {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };

            let _ = handled(&mut face, stream);
        }
    });

    let (status, body) = fetched(port, "GET", "/", b"");

    assert_eq!(status, 200);
    assert!(body.contains("Sensory Jam"));

    let (status, body) = fetched(port, "POST", "/event", INIT.as_bytes());

    assert_eq!(status, 200);
    assert!(body.contains(r#""gen":1"#));

    let (status, _) = fetched(port, "GET", "/nothing", b"");

    assert_eq!(status, 404);

    runner.join().unwrap();
}

/// One request over a real socket; (status, body text) back.
fn fetched(port: u16, method: &str, path: &str, body: &[u8]) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();

    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();

    let mut answer = String::new();

    BufReader::new(stream).read_to_string(&mut answer).unwrap();

    let status: u16 = answer
        .split_whitespace()
        .nth(1)
        .and_then(|held| held.parse().ok())
        .unwrap_or(0);
    let body = answer
        .split_once("\r\n\r\n")
        .map(|(_, held)| held.to_string())
        .unwrap_or_default();

    (status, body)
}
