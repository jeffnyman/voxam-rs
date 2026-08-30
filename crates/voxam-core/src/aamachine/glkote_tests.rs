//! Tests for the Å-machine's wire face and its stdio serving.
//!
//! The reference battery drives the vendored fixture stories; here
//! the unit drills run on crafted stories -- the terminal tests'
//! own shapes -- and the fixture sessions are certified end to end
//! by the aaglkote sweep instead.

use super::*;
use crate::aamachine::story::{SUMMED, crc32};
use crate::glkote::json::{dumps, loads};
use crate::iff::chunk as iff_chunk;

const QUIT: &[u8] = &[0x70, 0x00];

// Print "5", wait for a line, then quit.
const ASKS_LINE: &[u8] = &[0x65, 0x40, 0x05, 0x73, 0x00, 0x70, 0x00];

// Wait for a key, print it, then quit.
const ASKS_KEY: &[u8] = &[0xF3, 0x00, 0x65, 0x80, 0x00, 0x70, 0x00];

fn parsed(text: &str) -> Object {
    match loads(text).unwrap() {
        Value::Object(held) => held,
        _ => panic!("a stanza is an object"),
    }
}

fn init() -> Object {
    parsed(
        r#"{"type":"init","gen":0,"metrics":{"width":800,"height":600},"support":["timer","hyperlinks","graphics","graphicswin"]}"#,
    )
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

fn told(value: &Value) -> String {
    dumps(value)
}

/// Every buffer character in an update, flattened.
fn texted(update: &Object) -> String {
    let mut pieces = String::new();

    if let Some(content) = update.get("content") {
        for held in items(content) {
            if let Some(text) = entry(held).get("text") {
                for paragraph in items(text) {
                    if let Some(runs) = entry(paragraph).get("content") {
                        for run in items(runs) {
                            if let Some(Value::Str(piece)) = entry(run).get("text") {
                                pieces.push_str(piece);
                            }
                        }
                    }

                    pieces.push('\n');
                }
            }
        }
    }

    pieces
}

/// Every buffer run in an update, flattened, as compact JSON.
fn runs_of(update: &Object) -> Vec<String> {
    let mut held = Vec::new();

    if let Some(content) = update.get("content") {
        for piece in items(content) {
            if let Some(text) = entry(piece).get("text") {
                for paragraph in items(text) {
                    if let Some(runs) = entry(paragraph).get("content") {
                        for run in items(runs) {
                            held.push(told(run));
                        }
                    }
                }
            }
        }
    }

    held
}

// The LOOK sheet the dress tests wear: bold, italic, a
// red-and-bold class, and a green-on-black italic body -- the
// terminal battery's own shapes, the colors spelled in hex so the
// sheet mixes them.
fn look() -> Vec<u8> {
    let classes: &[&[&str]] = &[
        &["font-weight: bold"],
        &["font-style: italic"],
        &["color: #cd3131", "font-weight: bold"],
        &[
            "color: #0dbc79",
            "background-color: #000000",
            "font-style: italic",
        ],
    ];
    let mut offsets = Vec::new();
    let mut definitions = Vec::new();
    let base = 2 + classes.len() * 2;

    for class in classes {
        offsets.push((base + definitions.len()) as u16);

        for pair in *class {
            definitions.extend_from_slice(pair.as_bytes());
            definitions.push(0);
        }

        definitions.push(0);
    }

    let mut sheet = (classes.len() as u16).to_be_bytes().to_vec();

    for offset in offsets {
        sheet.extend_from_slice(&offset.to_be_bytes());
    }

    sheet.extend(definitions);

    sheet
}

// A minimal LANG: the four offsets, an empty extended table, an
// empty endings table, and the three special sets.
fn lang() -> Vec<u8> {
    let mut held = Vec::new();

    for offset in [8u16, 8, 9, 10] {
        held.extend_from_slice(&offset.to_be_bytes());
    }

    held.extend_from_slice(&[0, 0, 0, 0, 0, 0]);

    held
}

/// A story around a code body, wearing the dress-test LOOK and
/// whatever META entries are given.
fn storied(code: &[u8], meta: Option<&[u8]>) -> Story {
    let mut whole = vec![0x01];

    whole.extend_from_slice(code);

    let mut init_chunk = vec![0u8, 0, 0, 1, 0, 1];

    init_chunk.extend_from_slice(&[0, 1]);

    let summed = |name: &[u8; 4]| -> Vec<u8> {
        match name {
            b"LANG" => lang(),
            b"DICT" => vec![0, 0],
            b"MAPS" => vec![0, 0],
            b"LOOK" => look(),
            b"WRIT" => vec![0x80],
            b"INIT" => init_chunk.clone(),
            b"CODE" => whole.clone(),
            _ => Vec::new(),
        }
    };

    let mut crc = 0;

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

    let mut pieces = iff_chunk(b"HEAD", &head);

    if let Some(meta) = meta {
        pieces.extend(iff_chunk(b"META", meta));
    }

    for name in &SUMMED {
        pieces.extend(iff_chunk(name, &summed(name)));
    }

    let mut body = b"AAVM".to_vec();

    body.extend(pieces);

    Story::new(&iff_chunk(b"FORM", &body)).unwrap()
}

/// A begun face and its running machine, at the first wait.
fn opened(code: &[u8], meta: Option<&[u8]>) -> (GlkOteFrontend, Machine<WireVoice>) {
    opened_supporting(code, meta, &init())
}

fn opened_supporting(
    code: &[u8],
    meta: Option<&[u8]>,
    opening: &Object,
) -> (GlkOteFrontend, Machine<WireVoice>) {
    let story = storied(code, meta);
    let mut face = GlkOteFrontend::new(&story);
    let mut voice = WireVoice::new(&story).unwrap();

    face.begin(&mut voice, opening).unwrap();

    let mut machine = Machine::new(story, voice, Some(7)).unwrap();

    face.waiting = Some(machine.run(None).unwrap());

    (face, machine)
}

// A META story with a title and an author.
fn carded_meta() -> Vec<u8> {
    let mut meta = vec![2u8];

    meta.extend(b"\x01Tale\x00");
    meta.extend(b"\x02A. Author\x00");

    meta
}

// The first update carries the doorway card, the opening prose,
// one buffer window, and a line input request. (The reference
// drills the vendored Cloak of Darkness; the crafted story here
// exercises the same mechanics, and the sweep certifies the
// fixtures whole.)
#[test]
fn the_first_update_opens_the_document() {
    let (mut face, mut machine) = opened(ASKS_LINE, Some(&carded_meta()));
    let update = face.render(&mut machine.voice, false).unwrap();

    assert_eq!(at(&update, "type").as_str(), Some("update"));
    assert_eq!(items(at(&update, "windows")).len(), 1);
    assert_eq!(
        entry(&items(at(&update, "windows"))[0])
            .get("type")
            .and_then(Value::as_str),
        Some("buffer")
    );
    assert_eq!(
        entry(&items(at(&update, "input"))[0])
            .get("type")
            .and_then(Value::as_str),
        Some("line")
    );

    let told = texted(&update);

    assert!(told.contains("Tale"));
    assert!(told.contains("by A. Author"));
    assert!(told.contains('5'));
}

// A delivered line runs the machine to its quit; the exit-flagged
// update carries no input request.
#[test]
fn a_quit_exits_the_update() {
    let (mut face, mut machine) = opened(ASKS_LINE, None);

    face.render(&mut machine.voice, false).unwrap();

    let verdict = face
        .accept(
            &mut machine,
            &parsed(r#"{"type":"line","gen":1,"window":1,"value":"onward"}"#),
        )
        .unwrap();

    assert_eq!(verdict, Verdict::Advance);
    assert_eq!(face.waiting, Some(Wait::Quit));

    let update = face.render(&mut machine.voice, true).unwrap();

    assert_eq!(at(&update, "exit"), &Value::Bool(true));
    assert!(
        update
            .get("input")
            .is_none_or(|held| items(held).is_empty())
    );
}

// A key wait asks for a keystroke, and a char event answers it.
#[test]
fn char_events_answer_key_waits() {
    let (mut face, mut machine) = opened(ASKS_KEY, None);
    let update = face.render(&mut machine.voice, false).unwrap();

    assert_eq!(
        entry(&items(at(&update, "input"))[0])
            .get("type")
            .and_then(Value::as_str),
        Some("char")
    );

    let verdict = face
        .accept(
            &mut machine,
            &parsed(r#"{"type":"char","gen":1,"window":1,"value":"q"}"#),
        )
        .unwrap();

    assert_eq!(verdict, Verdict::Advance);
    assert_eq!(face.waiting, Some(Wait::Quit));
}

// A named key travels by its reserved code; an unknown name earns
// the pass and the wait stands.
#[test]
fn named_keys_travel_and_unknown_names_pass() {
    let (mut face, mut machine) = opened(ASKS_KEY, None);

    face.render(&mut machine.voice, false).unwrap();

    assert_eq!(
        face.accept(
            &mut machine,
            &parsed(r#"{"type":"char","gen":1,"window":1,"value":"func12"}"#),
        )
        .unwrap(),
        Verdict::Pass
    );
    assert_eq!(
        face.accept(
            &mut machine,
            &parsed(r#"{"type":"char","gen":1,"window":1,"value":"down"}"#),
        )
        .unwrap(),
        Verdict::Advance
    );
}

// Misaimed input -- a line where a key is wanted -- earns the
// polite pass, never a fault.
#[test]
fn misaimed_events_earn_the_pass() {
    let (mut face, mut machine) = opened(ASKS_KEY, None);

    face.render(&mut machine.voice, false).unwrap();

    assert_eq!(
        face.accept(
            &mut machine,
            &parsed(r#"{"type":"line","gen":1,"window":1,"value":"q"}"#),
        )
        .unwrap(),
        Verdict::Pass
    );
}

// A refresh redraws the whole picture without disturbing the
// machine: the full scrollback returns behind a clear.
#[test]
fn a_refresh_redraws_whole() {
    let (mut face, mut machine) = opened(ASKS_LINE, Some(&carded_meta()));

    face.render(&mut machine.voice, false).unwrap();

    assert_eq!(
        face.accept(&mut machine, &parsed(r#"{"type":"refresh","gen":1}"#))
            .unwrap(),
        Verdict::Stand
    );

    let update = face.render(&mut machine.voice, false).unwrap();
    let retold = texted(&update);

    assert!(retold.contains("Tale"));
    assert!(retold.contains('5'));
}

// An arrange event moves the window's box; one with no metrics
// passes.
#[test]
fn an_arrange_resizes_the_window() {
    let (mut face, mut machine) = opened(ASKS_LINE, None);

    face.render(&mut machine.voice, false).unwrap();

    assert_eq!(
        face.accept(
            &mut machine,
            &parsed(r#"{"type":"arrange","gen":1,"metrics":{"width":400,"height":300}}"#),
        )
        .unwrap(),
        Verdict::Stand
    );

    let update = face.render(&mut machine.voice, false).unwrap();

    assert_eq!(
        entry(&items(at(&update, "windows"))[0])
            .get("width")
            .and_then(Value::as_int),
        Some(400)
    );
    assert_eq!(
        face.accept(&mut machine, &parsed(r#"{"type":"arrange","gen":1}"#))
            .unwrap(),
        Verdict::Pass
    );
}

// An init without metrics is refused at the door.
#[test]
fn an_init_without_metrics_is_refused() {
    let story = storied(QUIT, None);
    let mut face = GlkOteFrontend::new(&story);
    let mut voice = WireVoice::new(&story).unwrap();
    let refused = face
        .begin(&mut voice, &parsed(r#"{"type":"init","gen":0}"#))
        .unwrap_err();

    assert!(refused.to_string().contains("metrics carry no size"));
}

// A story without META opens without the card.
#[test]
fn a_cardless_story_opens_plain() {
    let (mut face, mut machine) = opened(ASKS_LINE, None);
    let update = face.render(&mut machine.voice, false).unwrap();
    let told = texted(&update);

    assert!(told.contains('5'));
    assert!(!told.contains("Tale"));
}

// A META blurb joins the doorway card, its line feeds honored,
// and a card-only render asks for nothing.
#[test]
fn a_blurb_joins_the_card() {
    let mut meta = vec![2u8];

    meta.extend(b"\x01Tale\x00");
    meta.extend(b"\x04Told\x10whole.\x00");

    let story = storied(QUIT, Some(&meta));
    let mut face = GlkOteFrontend::new(&story);
    let mut voice = WireVoice::new(&story).unwrap();

    face.begin(&mut voice, &init()).unwrap();

    let update = face.render(&mut voice, false).unwrap();
    let told = texted(&update);

    assert!(told.contains("Tale"));
    assert!(told.contains("Told\nwhole."));
    assert!(update.get("input").is_none());
}

// The stdio server drives a whole session: init in, updates out, a
// line delivered through the standing detours, the exit flagged.
#[test]
fn serve_drives_a_session_whole() {
    let events = [
        dumps(&Value::Object(init())),
        r#"{"type":"refresh","gen":1}"#.to_string(),
        r#"{"type":"arrange","gen":1,"metrics":{"width":500}}"#.to_string(),
        r#"{"type":"char","gen":1,"window":1,"value":"x"}"#.to_string(),
        r#"{"type":"line","gen":1,"window":1,"value":"onward"}"#.to_string(),
    ];
    let joined: String = events.iter().map(|line| format!("{line}\n")).collect();
    let mut reader = std::io::Cursor::new(joined.into_bytes());
    let mut writer: Vec<u8> = Vec::new();

    assert!(serve(
        storied(ASKS_LINE, None),
        &mut reader,
        &mut writer,
        Some(7)
    ));

    let updates: Vec<Object> = String::from_utf8(writer)
        .unwrap()
        .lines()
        .map(parsed)
        .collect();

    assert_eq!(at(&updates[0], "type").as_str(), Some("update"));
    assert_eq!(
        updates.last().unwrap().get("exit"),
        Some(&Value::Bool(true))
    );
    assert!(
        updates
            .iter()
            .any(|held| told(&Value::Object(held.clone())) == r#"{"type":"pass"}"#)
    );
}

// A conversation that opens with anything but an init is refused
// as the protocol's own error stanza.
#[test]
fn serve_refuses_a_wrong_opening() {
    let mut reader = std::io::Cursor::new(b"{\"type\":\"line\",\"value\":\"x\"}\n".to_vec());
    let mut writer: Vec<u8> = Vec::new();

    assert!(!serve(storied(QUIT, None), &mut reader, &mut writer, None));
    assert!(
        String::from_utf8(writer)
            .unwrap()
            .contains("opens with an init event")
    );
}

// A stream that is not JSON answers the same way.
#[test]
fn serve_refuses_broken_json() {
    let mut reader = std::io::Cursor::new(b"{broken\n".to_vec());
    let mut writer: Vec<u8> = Vec::new();

    assert!(!serve(storied(QUIT, None), &mut reader, &mut writer, None));
    assert!(String::from_utf8(writer).unwrap().contains("not JSON"));
}

// A stream that simply ends -- before the wait's answer -- ends
// the session cleanly.
#[test]
fn serve_survives_a_closed_stream() {
    let mut reader =
        std::io::Cursor::new(format!("{}\n", dumps(&Value::Object(init()))).into_bytes());
    let mut writer: Vec<u8> = Vec::new();

    assert!(serve(
        storied(ASKS_LINE, None),
        &mut reader,
        &mut writer,
        Some(7)
    ));
}

// -- the dress on the wire ---------------------------------------------

/// A begun face and voice over the crafted sheet, the grant chosen.
fn faced(support: &str) -> (GlkOteFrontend, WireVoice) {
    let story = storied(QUIT, None);
    let mut face = GlkOteFrontend::new(&story);
    let mut voice = WireVoice::new(&story).unwrap();

    face.begin(
        &mut voice,
        &parsed(&format!(
            r#"{{"type":"init","gen":0,"metrics":{{"width":800,"height":600}},"support":[{support}]}}"#
        )),
    )
    .unwrap();

    (face, voice)
}

// Bold rides subheader, italic emphasized, and both at once ride
// alert, the stock sheet rendering it bold as the spec permits.
#[test]
fn the_wire_wears_bold_and_italic() {
    let (mut face, mut voice) = faced(r#""timer""#);

    voice.enter_span(0);
    voice.say("clue");
    voice.leave_span();
    voice.enter_span(1);
    voice.say("aside");
    voice.enter_span(0);
    voice.say("both");
    voice.leave_span();
    voice.leave_span();

    let update = face.render(&mut voice, false).unwrap();
    let runs = runs_of(&update);

    assert!(runs.contains(&r#"{"style":"subheader","text":"clue"}"#.to_string()));
    assert!(runs.contains(&r#"{"style":"emphasized","text":"aside"}"#.to_string()));
    assert!(runs.contains(&r#"{"style":"alert","text":"both"}"#.to_string()));
}

// Under the colors grant the sheet's ink rides the runs; without
// it the same spans travel dressed but uncolored, and the voice
// answers VM_INFO's color question accordingly.
#[test]
fn color_rides_only_under_the_grant() {
    let (mut face, mut voice) = faced(r#""colors""#);

    voice.enter_span(2);
    voice.say("warning");
    voice.leave_span();

    assert!(voice.has_color());

    let update = face.render(&mut voice, false).unwrap();

    assert!(
        runs_of(&update).contains(
            &r#"{"style":"subheader","text":"warning","fg":"rgb(205,49,49)"}"#.to_string()
        )
    );

    let (mut plain, mut unfunded) = faced(r#""timer""#);

    unfunded.enter_span(2);
    unfunded.say("warning");
    unfunded.leave_span();

    assert!(!unfunded.has_color());
    assert!(unfunded.has_styles());

    let update = plain.render(&mut unfunded, false).unwrap();

    assert!(runs_of(&update).contains(&r#"{"style":"subheader","text":"warning"}"#.to_string()));
}

// The body dress layers beneath the whole document on the wire
// too: green ink on black paper, in the emphasized style.
#[test]
fn the_body_dresses_the_wire() {
    let (mut face, mut voice) = faced(r#""colors""#);

    voice.set_body(3);
    voice.say("green words");

    let update = face.render(&mut voice, false).unwrap();

    assert!(runs_of(&update).contains(
        &r#"{"style":"emphasized","text":"green words","fg":"rgb(13,188,121)","bg":"rgb(0,0,0)"}"#
            .to_string()
    ));
}

// A dressed session survives a refresh: the scrollback returns
// with its styles still on.
#[test]
fn a_refresh_keeps_the_dress() {
    let (mut face, mut voice) = faced(r#""timer""#);

    voice.enter_span(0);
    voice.say("kept");
    voice.leave_span();
    face.render(&mut voice, false).unwrap();

    face.refresh = true;

    let update = face.render(&mut voice, false).unwrap();

    assert!(runs_of(&update).contains(&r#"{"style":"subheader","text":"kept"}"#.to_string()));
}

// The sidecar rides when the display says the "voxam" token: the
// first update carries the empty block -- the feed alive, nothing
// yet to tell -- and once a line lands the block carries it (PORT:
// What the sidecar carries).
#[test]
fn the_sidecar_rides_when_granted() {
    let mut opening = init();

    opening.set(
        "support",
        Value::List(vec![Value::from("timer"), Value::from("voxam")]),
    );

    let (mut face, mut machine) = opened_supporting(ASKS_LINE, None, &opening);
    let voxam = face.sidecar(&mut machine.discontinuity);
    let update = face.render_with(&mut machine.voice, false, voxam).unwrap();

    assert_eq!(told(at(&update, "voxam")), "{}");

    face.accept(
        &mut machine,
        &parsed(r#"{"type":"line","gen":1,"window":1,"value":"west"}"#),
    )
    .unwrap();

    let voxam = face.sidecar(&mut machine.discontinuity);
    let update = face.render_with(&mut machine.voice, false, voxam).unwrap();

    assert_eq!(told(at(&update, "voxam")), r#"{"command":"west"}"#);
}

// The discontinuity bit is read once and rested; ungranted, the
// render carries no block at all.
#[test]
fn the_sidecar_rests_the_discontinuity() {
    let mut opening = init();

    opening.set("support", Value::List(vec![Value::from("voxam")]));

    let (mut face, mut machine) = opened_supporting(ASKS_LINE, None, &opening);

    machine.discontinuity = true;

    let voxam = face.sidecar(&mut machine.discontinuity);
    let update = face.render_with(&mut machine.voice, false, voxam).unwrap();

    assert_eq!(told(at(&update, "voxam")), r#"{"discontinuity":true}"#);
    assert!(!machine.discontinuity);

    let (mut plain, mut quiet) = opened(ASKS_LINE, None);
    let voxam = plain.sidecar(&mut quiet.discontinuity);

    assert!(voxam.is_none());
    assert!(
        plain
            .render_with(&mut quiet.voice, false, voxam)
            .unwrap()
            .get("voxam")
            .is_none()
    );
}
