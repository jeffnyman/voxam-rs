//! Glk's view of the Blorb: pictures measured, sounds reframed.

use super::*;
use crate::iff::write_form;

/// One (usage, number, id, payload) row of a test Blorb.
type Row = ([u8; 4], u32, [u8; 4], Vec<u8>);

/// Assemble a parsed Blorb from its rows, offsets computed the way
/// a writer lays them out.
fn blorb(entries: &[Row]) -> Blorb {
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

    Blorb::parse(&write_form(b"IFRS", &framed)).unwrap()
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

    held.extend(b"\xff\x01"); // a standalone marker, walked over
    held.extend(b"\xff\xe0\x00\x04\x00\x00"); // an APP0 segment, walked over
    held.extend(b"\xff\xc0\x00\x08\x08"); // SOF0, length, precision
    held.extend((height as u16).to_be_bytes());
    held.extend((width as u16).to_be_bytes());

    held
}

// PNG puts its dimensions at fixed offsets behind the required
// IHDR; JPEG hides them in a start-of-frame segment behind
// whatever markers come first.
#[test]
fn dimensions_are_read_from_the_bytes() {
    assert_eq!(image_size(&png(320, 200)), Some((320, 200)));
    assert_eq!(image_size(&jpeg(640, 400)), Some((640, 400)));
}

// Damaged or foreign bytes answer None rather than a wrong size:
// a PNG cut before its header, a JPEG that loses marker alignment,
// one that ends mid-frame-header, one with no frame at all, and
// something that is no image whatsoever.
#[test]
fn unmeasurable_bytes_answer_none() {
    assert_eq!(image_size(b"\x89PNG\r\n\x1a\nIH"), None);
    assert_eq!(image_size(b"\xff\xd8\x00\x00\x00\x00"), None);
    assert_eq!(image_size(b"\xff\xd8\xff\xc0\x00\x08\x08"), None);
    assert_eq!(image_size(b"\xff\xd8\xff\xe0\x00\x04\x00\x00"), None);
    assert_eq!(image_size(b"GIF89a"), None);
}

// With no Blorb behind it, everything answers "nothing here" --
// the right answer for a bare .ulx story.
#[test]
fn no_blorb_answers_nothing() {
    let mut bare = Resources::new(None);

    assert!(bare.image(1).is_none());
    assert!(bare.sound(1).is_none());
    assert!(bare.data(1).is_none());
    assert!(bare.frontispiece().is_none());
    assert!(bare.audible(1).is_none());
    assert!(bare.pictured(1).is_none());
}

// The Fspc chunk names the cover outright; a Blorb full of
// pictures but no Fspc offers nothing rather than a guess (Blorb:
// Frontispiece Chunk).
#[test]
fn the_frontispiece_answers_only_when_named() {
    let mut unnamed = Resources::new(Some(blorb(&[(*b"Pict", 1, *b"PNG ", png(32, 16))])));

    assert!(unnamed.frontispiece().is_none());
}

/// A mono 8-bit AIFF FORM payload: two sample points at 16384Hz.
fn tiny_aiff() -> Vec<u8> {
    let mut comm = 1i16.to_be_bytes().to_vec();

    comm.extend(2u32.to_be_bytes());
    comm.extend(8i16.to_be_bytes());
    comm.extend(16397u16.to_be_bytes());
    comm.extend((1u64 << 63).to_be_bytes());

    let mut ssnd = 0u32.to_be_bytes().to_vec();

    ssnd.extend(0u32.to_be_bytes());
    ssnd.extend(b"\x01\xfe");

    let mut held = b"AIFF".to_vec();

    held.extend(chunk(b"COMM", &comm));
    held.extend(chunk(b"SSND", &ssnd));

    held
}

/// The base64 payload of a data: url.
fn decoded_payload(url: &str) -> Vec<u8> {
    let payload = url.split_once(',').expect("a data: url").1;
    let mut held = Vec::new();
    let mut bits = 0u32;
    let mut count = 0u32;

    for byte in payload.bytes().filter(|held| *held != b'=') {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            _ => 63,
        };

        bits = (bits << 6) | u32::from(value);
        count += 6;

        if count >= 8 {
            count -= 8;
            held.push((bits >> count) as u8);
        }
    }

    held
}

// Sounds travel the wire in containers browsers decode: the AIFF's
// points come back inside a WAVE data: url, byte for byte where
// the unsigned convention moves them; an Ogg travels as itself;
// MOD music, a broken AIFF, and an absent number answer None --
// and every answer is remembered.
#[test]
fn sounds_travel_in_wire_containers() {
    let mut held = Resources::new(Some(blorb(&[
        (*b"Snd ", 3, *b"FORM", tiny_aiff()),
        (*b"Snd ", 4, *b"OGGV", b"OggS-ish".to_vec()),
        (*b"Snd ", 5, *b"MOD ", vec![0]),
        (*b"Snd ", 6, *b"FORM", b"AIFFjunk".to_vec()),
    ])));

    let url = held.audible(3).expect("the AIFF travelled");

    assert!(url.starts_with("data:audio/wav;base64,"));
    assert_eq!(&decoded_payload(&url)[44..], &[0x81, 0x7E]);
    assert_eq!(
        held.audible(4).as_deref(),
        Some(format!("data:audio/ogg;base64,{}", b64(b"OggS-ish")).as_str())
    );
    assert!(held.audible(5).is_none());
    assert!(held.audible(6).is_none());
    assert!(held.audible(9).is_none());
    assert_eq!(held.audible(3), held.audible(3));
}

// A picture is measured on first use and remembered after -- the
// unmeasurable and the absent are remembered as None the same way.
#[test]
fn pictures_are_measured_once() {
    let mut held = Resources::new(Some(blorb(&[
        (*b"Pict", 1, *b"PNG ", png(32, 16)),
        (*b"Pict", 2, *b"Rect", vec![0; 8]),
    ])));

    assert_eq!(
        held.image(1),
        Some(&ImageInfo {
            number: 1,
            kind: *b"PNG ",
            data: png(32, 16),
            width: 32,
            height: 16,
        })
    );
    assert!(held.image(2).is_none());
    assert!(held.image(9).is_none());
}

// A picture rides whole as a data: url, its media type told by the
// chunk kind (Blorb: Picture Resource Chunks).
#[test]
fn pictures_ride_as_data_urls() {
    let mut held = Resources::new(Some(blorb(&[
        (*b"Pict", 1, *b"PNG ", png(32, 16)),
        (*b"Pict", 2, *b"JPEG", jpeg(4, 2)),
    ])));

    assert_eq!(
        held.pictured(1).as_deref(),
        Some(format!("data:image/png;base64,{}", b64(&png(32, 16))).as_str())
    );
    assert_eq!(
        held.pictured(2).as_deref(),
        Some(format!("data:image/jpeg;base64,{}", b64(&jpeg(4, 2))).as_str())
    );
    assert!(held.pictured(9).is_none());
}
