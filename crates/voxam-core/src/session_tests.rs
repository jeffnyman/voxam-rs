//! The session facade: stories recognized by the reference
//! loaders' rules and served by the machine that owns them.
//!
//! The recognition battery drills the routing table -- suffixes,
//! magic, containers, sidecars, and the honest refusals -- on
//! crafted stories; the corpus certification rides the wire
//! sweeps, which drive the CLI through this same facade.

use std::io::{BufRead, Cursor, Write};

use crate::aamachine::story::{SUMMED, crc32};
use crate::iff::{IffChunk, chunk as iff_chunk, write_form};
use crate::session::Opening;
use crate::zmachine::machine::Identity;

/// A 512-byte Version 3 story with code planted at $40, the wire
/// tests' own shape.
fn z_image(code: &[u8]) -> Vec<u8> {
    let mut data = vec![0u8; 512];

    data[0] = 3;
    data[0x04..0x06].copy_from_slice(&0x01C0u16.to_be_bytes());
    data[0x06..0x08].copy_from_slice(&0x0040u16.to_be_bytes());
    data[0x0C..0x0E].copy_from_slice(&0x0100u16.to_be_bytes());
    data[0x0E..0x10].copy_from_slice(&0x01C0u16.to_be_bytes());
    data[0x40..0x40 + code.len()].copy_from_slice(code);

    data
}

// quit, the 0OP short form.
const Z_QUIT: &[u8] = &[0xBA];

// A start function that quits: the stack-argument header with an
// empty locals format, then the two-byte quit opcode.
const GLULX_QUIT: &[u8] = &[0xC0, 0x00, 0x00, 0x81, 0x20];

// The Å-machine's quit.
const AA_QUIT: &[u8] = &[0x70, 0x00];

/// A minimal LANG: the four offsets, an empty extended table, an
/// empty endings table, and the three special sets.
fn lang() -> Vec<u8> {
    let mut told = Vec::new();

    for offset in [8u16, 8, 9, 10] {
        told.extend_from_slice(&offset.to_be_bytes());
    }

    told.extend_from_slice(&[0, 0, 0, 0, 0, 0]);

    told
}

/// A one-class LOOK sheet, the smallest that parses.
fn look() -> Vec<u8> {
    let definition = b"font-weight: bold\0\0";
    let mut told = 1u16.to_be_bytes().to_vec();

    told.extend_from_slice(&4u16.to_be_bytes());
    told.extend_from_slice(definition);

    told
}

/// An Å-machine story's bytes around a code body -- the terminal
/// battery's fixture, kept as bytes for the recognizer.
fn aa_story(code: &[u8]) -> Vec<u8> {
    let mut whole = vec![0x01];

    whole.extend_from_slice(code);

    let mut init = vec![0u8, 0, 0, 1, 0, 1];

    init.extend_from_slice(&[0, 1]);

    let summed = |name: &[u8; 4]| -> Vec<u8> {
        match name {
            b"LANG" => lang(),
            b"DICT" => vec![0, 0],
            b"MAPS" => vec![0, 0],
            b"LOOK" => look(),
            b"WRIT" => vec![0x80],
            b"INIT" => init.clone(),
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

    for name in &SUMMED {
        pieces.extend(iff_chunk(name, &summed(name)));
    }

    let mut body = b"AAVM".to_vec();

    body.extend(pieces);

    iff_chunk(b"FORM", &body)
}

/// One (usage, number, id, payload) row of a test Blorb.
type Row = ([u8; 4], u32, [u8; 4], Vec<u8>);

/// A Blorb's bytes assembled from its rows, offsets computed the
/// way a writer lays them out.
fn blorb_bytes(entries: &[Row]) -> Vec<u8> {
    let mut index = (entries.len() as u32).to_be_bytes().to_vec();
    let mut body = Vec::new();

    // The FORM prelude (12) plus the RIdx chunk's own frame.
    let mut offset = 12 + 8 + 4 + 12 * entries.len();

    for (usage, number, chunk_id, payload) in entries {
        index.extend(usage);
        index.extend(number.to_be_bytes());
        index.extend((offset as u32).to_be_bytes());

        offset += 8 + payload.len() + payload.len() % 2;

        body.push(IffChunk {
            chunk_id: *chunk_id,
            payload: payload.clone(),
            offset: 0,
        });
    }

    let mut framed = vec![IffChunk {
        chunk_id: *b"RIdx",
        payload: index,
        offset: 0,
    }];

    framed.extend(body);

    write_form(b"IFRS", &framed)
}

#[test]
fn recognizes_a_bare_z_story() {
    let opening = Opening::of("story.z3", z_image(Z_QUIT), None).expect("a loadable story");

    assert!(matches!(opening, Opening::Z { blorb: None, .. }));
}

#[test]
fn attaches_a_sidecar_to_a_bare_z_story() {
    let sidecar = blorb_bytes(&[]);
    let opening =
        Opening::of("story.z3", z_image(Z_QUIT), Some(sidecar)).expect("a loadable story");

    assert!(matches!(opening, Opening::Z { blorb: Some(_), .. }));
}

#[test]
fn recognizes_bare_glulx_by_its_magic() {
    let opening = Opening::of("story.ulx", crate::glulx::testing::image(GLULX_QUIT), None)
        .expect("a loadable story");

    assert!(matches!(opening, Opening::Glulx { blorb: None, .. }));
}

#[test]
fn attaches_a_sidecar_to_bare_glulx() {
    let sidecar = blorb_bytes(&[]);
    let opening = Opening::of(
        "story.ulx",
        crate::glulx::testing::image(GLULX_QUIT),
        Some(sidecar),
    )
    .expect("a loadable story");

    assert!(matches!(opening, Opening::Glulx { blorb: Some(_), .. }));
}

#[test]
fn unwraps_a_glulx_blorb() {
    let packaged = blorb_bytes(&[(
        *b"Exec",
        0,
        *b"GLUL",
        crate::glulx::testing::image(GLULX_QUIT),
    )]);
    let opening = Opening::of("story.gblorb", packaged, None).expect("a loadable story");

    assert!(matches!(opening, Opening::Glulx { blorb: Some(_), .. }));
}

#[test]
fn unwraps_a_z_blorb() {
    let packaged = blorb_bytes(&[(*b"Exec", 0, *b"ZCOD", z_image(Z_QUIT))]);
    let opening = Opening::of("story.zblorb", packaged, None).expect("a loadable story");

    assert!(matches!(opening, Opening::Z { blorb: Some(_), .. }));
}

#[test]
fn the_blorb_suffix_speaks_in_any_case() {
    let packaged = blorb_bytes(&[(
        *b"Exec",
        0,
        *b"GLUL",
        crate::glulx::testing::image(GLULX_QUIT),
    )]);
    let opening = Opening::of("STORY.GBLORB", packaged, None).expect("a loadable story");

    assert!(matches!(opening, Opening::Glulx { .. }));
}

#[test]
fn refuses_a_storyless_blorb_by_name() {
    let Err(refused) = Opening::of("story.blorb", blorb_bytes(&[]), None) else {
        panic!("a storyless blorb must refuse");
    };

    assert_eq!(
        refused.to_string(),
        "story.blorb packages no Z-code story to run"
    );
}

#[test]
fn recognizes_the_aa_form() {
    let opening = Opening::of("story.aastory", aa_story(AA_QUIT), None).expect("a loadable story");

    assert!(matches!(opening, Opening::Aa { .. }));
}

#[test]
fn unrecognized_bytes_take_the_z_loaders_refusal() {
    assert!(Opening::of("story.dat", b"not a story at all".to_vec(), None).is_err());
}

/// One whole conversation: the init in, EOF after, the update out.
fn served(opening: Opening, init: &str) -> (bool, String) {
    let mut reader = Cursor::new(format!("{init}\n").into_bytes());
    let mut writer = Vec::new();
    let clean = opening
        .serve(&mut reader, &mut writer, Some(1), Identity::default())
        .expect("a served session");

    (clean, String::from_utf8(writer).expect("wire text"))
}

#[test]
fn serves_a_z_story_to_its_quit() {
    let opening = Opening::of("story.z3", z_image(Z_QUIT), None).expect("a loadable story");
    let (clean, wire) = served(
        opening,
        r#"{"type":"init","gen":0,"support":["timer"],"metrics":{"width":800,"height":480,"gridcharwidth":10,"gridcharheight":20}}"#,
    );

    assert!(clean);
    assert!(wire.contains(r#""type":"update""#));
}

#[test]
fn serves_a_glulx_story_to_its_quit() {
    let opening = Opening::of("story.ulx", crate::glulx::testing::image(GLULX_QUIT), None)
        .expect("a loadable story");
    let (clean, wire) = served(
        opening,
        r#"{"type":"init","gen":0,"support":["timer","graphicswin","hyperlinks"],"metrics":{"width":80,"height":24}}"#,
    );

    assert!(clean);
    assert!(wire.contains(r#""type":"update""#));
}

// The linked host's arrangement: the story crosses to a thread as
// bytes, the session is built and served entirely over there, and
// the conversation travels the pipes. Nothing but bytes and pipe
// ends cross the boundary, which is what lets a machine full of
// Rc handles be served from a thread at all.
#[test]
fn serves_over_linked_pipes_from_another_thread() {
    let bytes = z_image(Z_QUIT);
    let (mut to_session, from_host) = crate::pipe::pipe();
    let (to_host, mut from_session) = crate::pipe::pipe();

    let serving = std::thread::spawn(move || {
        let mut reader = from_host;
        let mut writer = to_host;
        let opening = Opening::of("story.z3", bytes, None).expect("a loadable story");

        opening
            .serve(&mut reader, &mut writer, Some(1), Identity::default())
            .expect("a served session")
    });

    writeln!(
        to_session,
        r#"{{"type":"init","gen":0,"support":["timer"],"metrics":{{"width":800,"height":480,"gridcharwidth":10,"gridcharheight":20}}}}"#
    )
    .expect("a written init");

    let mut update = String::new();

    from_session.read_line(&mut update).expect("an update");

    assert!(update.contains(r#""type":"update""#));

    // The host hangs up; the session ends as it would on a closed
    // stdin, and its verdict is a clean session.
    drop(to_session);

    assert!(serving.join().expect("the serving thread"));
}

#[test]
fn serves_an_aa_story_to_its_quit() {
    let opening = Opening::of("story.aastory", aa_story(AA_QUIT), None).expect("a loadable story");
    let (clean, wire) = served(
        opening,
        r#"{"type":"init","gen":0,"metrics":{"width":800,"height":600},"support":["timer","hyperlinks","graphics","graphicswin"]}"#,
    );

    assert!(clean);
    assert!(wire.contains(r#""type":"update""#));
}
