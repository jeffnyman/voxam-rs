//! The Å-machine story file: the AAVM form read whole.
//!
//! The third machine's stories are IFF: form AAVM, HEAD first, and
//! a CRC-32 running over seven starred chunks in the spec's own
//! order. An interpreter may treat the whole file as one read-only
//! address space (Aa-machine specification 1.0: Story file); this
//! reader keeps the chunks and the header's claims, everything
//! verified loud at the door -- a story that lies about its
//! checksum is a story worth refusing before it runs.
//!
//! The compatibility ledger is deliberate: the reader speaks the
//! community fork's 1.0 specification and accepts the 0.x stories
//! the Dialog compilers of the world actually emit -- the minor
//! version is backward-compatible by the spec's own numbering, and
//! a major version from the future is refused by name.

use std::collections::HashMap;

use crate::errors::VoxamError;
use crate::iff::{IffChunk, parse_form};

pub const FORM_ID: [u8; 4] = *b"AAVM";
pub const HEAD_ID: [u8; 4] = *b"HEAD";
pub const META_ID: [u8; 4] = *b"META";
pub const FILE_ID: [u8; 4] = *b"FILE";

/// The chunks the HEAD's CRC-32 runs over, in the spec's own,
/// deliberate order (Aa-machine: Story file).
pub const SUMMED: [[u8; 4]; 7] = [
    *b"LOOK", *b"LANG", *b"MAPS", *b"DICT", *b"INIT", *b"CODE", *b"WRIT",
];

/// The fixed header: version pair, word size, shift amount,
/// release, serial, checksum, and the three area sizes; the
/// optional IFID rides after (Aa-machine: HEAD).
const HEAD_SIZE: usize = 22;
const IFID_SIZE: usize = 46;

/// The only word size the specification currently speaks.
const WORD_SIZE: u8 = 2;

/// The newest major version this reader understands: the community
/// fork's own. Minor versions are backward-compatible by the
/// spec's numbering, so every 0.x story is welcome here too.
const SUPPORTED_MAJOR: u8 = 1;

/// The META chunk's identifiers, by the names the spec gives them
/// (Aa-machine: META).
const META_NAMES: [(u8, &str); 5] = [
    (1, "title"),
    (2, "author"),
    (3, "noun"),
    (4, "blurb"),
    (5, "date"),
];

fn story_error(message: String) -> VoxamError {
    VoxamError::AAMachine(message)
}

/// The reference's `zlib.crc32`, chained: the standard CRC-32 over
/// the polynomial IFF's world shares with zlib and PNG.
pub fn crc32(data: &[u8], running: u32) -> u32 {
    let mut crc = !running;

    for &byte in data {
        crc ^= u32::from(byte);

        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }

    !crc
}

/// One parsed Å-machine story, its header's claims verified.
#[derive(Debug, Clone)]
pub struct Story {
    /// The file format version as (major, minor).
    pub version: (u8, u8),
    /// The machine word size in bytes, currently 2.
    pub word_size: u8,
    /// The shift amount for short and long string pointers.
    pub shift: u8,
    /// The story's release number.
    pub release: u16,
    /// The six-character serial, as the header spells it.
    pub serial: String,
    /// The HEAD's CRC-32 claim, already verified.
    pub checksum: u32,
    /// The heap/env/choice area size, in words.
    pub heap_size: u16,
    /// The aux/trail area size, in words.
    pub aux_size: u16,
    /// The random access area size, in words.
    pub ram_size: u16,
    /// The embedded IFID's UUID, uppercased; None when the
    /// optional field is absent (Aa-machine: HEAD).
    pub ifid: Option<String>,
    /// The LANG chunk's extended characters as Unicode, in order.
    pub extended: Vec<char>,
    /// The META chunk's bibliography by field name -- title,
    /// author, noun, blurb, date -- empty without one, in the
    /// chunk's own order.
    pub meta: Vec<(String, String)>,
    /// Every chunk, in file order.
    pub chunks: Vec<IffChunk>,
    // The first chunk of each kind, by index into chunks.
    held: HashMap<[u8; 4], usize>,
}

impl Story {
    /// Parse and verify one story file's bytes.
    ///
    /// Fails for a form that is not AAVM, a HEAD missing, late, or
    /// short, a word size or major version this reader does not
    /// speak, a summed chunk missing, or a checksum that
    /// disagrees; and for a FORM that cannot be walked at all.
    pub fn new(data: &[u8]) -> Result<Self, VoxamError> {
        let (form, chunks) = parse_form(data)?;

        if form != FORM_ID {
            return Err(story_error(format!(
                "an Å-machine story is FORM AAVM, not FORM {} (Aa-machine: Story file)",
                asciied(&form)
            )));
        }

        if chunks.is_empty() || chunks[0].chunk_id != HEAD_ID {
            return Err(story_error(
                "HEAD must be the first chunk in the form (Aa-machine: Story file)".into(),
            ));
        }

        let head = &chunks[0].payload;

        if head.len() < HEAD_SIZE {
            return Err(story_error(format!(
                "the HEAD holds {} bytes, but the fixed header is {HEAD_SIZE} \
                 (Aa-machine: HEAD)",
                head.len()
            )));
        }

        let version = (head[0], head[1]);

        if version.0 > SUPPORTED_MAJOR {
            return Err(story_error(format!(
                "story format {}.{} is from a future specification; this reader \
                 speaks up to {SUPPORTED_MAJOR}.x (Aa-machine: Story file)",
                version.0, version.1
            )));
        }

        let word_size = head[2];

        if word_size != WORD_SIZE {
            return Err(story_error(format!(
                "a word of {word_size} bytes; {WORD_SIZE} is the only size the \
                 specification speaks (Aa-machine: Runtime data)"
            )));
        }

        let ifid_end = head.len().min(HEAD_SIZE + IFID_SIZE);

        let mut story = Self {
            version,
            word_size,
            shift: head[3],
            release: u16::from_be_bytes([head[4], head[5]]),
            serial: asciied(&head[6..12]),
            checksum: u32::from_be_bytes([head[12], head[13], head[14], head[15]]),
            heap_size: u16::from_be_bytes([head[16], head[17]]),
            aux_size: u16::from_be_bytes([head[18], head[19]]),
            ram_size: u16::from_be_bytes([head[20], head[21]]),
            ifid: branded(&head[HEAD_SIZE..ifid_end])?,
            extended: Vec::new(),
            meta: Vec::new(),
            held: HashMap::new(),
            chunks,
        };

        for (at, held) in story.chunks.iter().enumerate() {
            story.held.entry(held.chunk_id).or_insert(at);
        }

        story.certified()?;

        // LANG stands certified present, being a summed chunk.
        story.extended = extended(story.summed(b"LANG"))?;
        story.meta = metadata(story.chunk(&META_ID), &story.extended)?;

        Ok(story)
    }

    /// The first chunk of a kind, None when the story has none.
    pub fn chunk(&self, chunk_id: &[u8; 4]) -> Option<&IffChunk> {
        self.held.get(chunk_id).map(|&at| &self.chunks[at])
    }

    /// A summed chunk, present by the door's own certification.
    ///
    /// The checksum runs over every SUMMED chunk at parse, so a
    /// missing one never reaches here -- asking for anything else
    /// is a caller's wiring fault, loud by panic.
    pub fn summed(&self, chunk_id: &[u8; 4]) -> &IffChunk {
        self.chunk(chunk_id)
            .expect("a summed chunk stands certified present")
    }

    /// The FILE chunks alone, which may repeat, in file order.
    pub fn files(&self) -> impl Iterator<Item = &IffChunk> {
        self.chunks.iter().filter(|held| held.chunk_id == FILE_ID)
    }

    /// One META field by name, None when the bibliography lacks it.
    pub fn meta_field(&self, name: &str) -> Option<&str> {
        self.meta
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.as_str())
    }

    /// Verify the HEAD's CRC-32 over the summed chunks.
    fn certified(&self) -> Result<(), VoxamError> {
        let mut crc = 0;

        for name in &SUMMED {
            let Some(held) = self.chunk(name) else {
                return Err(story_error(format!(
                    "the {} chunk is missing, and the checksum runs over it \
                     (Aa-machine: Story file)",
                    asciied(name)
                )));
            };

            crc = crc32(&held.payload, crc);
        }

        if crc != self.checksum {
            return Err(story_error(format!(
                "the story's contents sum to {crc:08x}, but the header claims \
                 {:08x} (Aa-machine: HEAD)",
                self.checksum
            )));
        }

        Ok(())
    }
}

/// Bytes as ASCII text, the reference's `decode("ascii",
/// "replace")`: anything past $7F becomes the replacement mark.
pub(crate) fn asciied(raw: &[u8]) -> String {
    raw.iter()
        .map(|&byte| {
            if byte < 0x80 {
                byte as char
            } else {
                '\u{fffd}'
            }
        })
        .collect()
}

/// The HEAD's optional IFID, unwrapped from its UUID dressing.
///
/// Fails for a field present but dressed wrong -- the spec spells
/// it "UUID://...//" and null-terminated (Aa-machine: HEAD).
fn branded(tail: &[u8]) -> Result<Option<String>, VoxamError> {
    if tail.is_empty() {
        return Ok(None);
    }

    let ended = tail
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(tail.len());
    let told = asciied(&tail[..ended]);

    if !(told.starts_with("UUID://") && told.ends_with("//")) {
        return Err(story_error(format!(
            "the HEAD's IFID field reads '{told}', not UUID://...// (Aa-machine: HEAD)"
        )));
    }

    let chars: Vec<char> = told.chars().collect();
    let inner: String = chars[7..chars.len().max(9) - 2].iter().collect();

    Ok(Some(inner.to_uppercase()))
}

// The LANG chunk opens with four two-byte offsets; the extended
// character table's is the second (Aa-machine: LANG).
const LANG_OFFSETS: usize = 4;

/// The LANG chunk's extended characters as Unicode, in order.
///
/// Character bytes at $80 and above index this table wherever the
/// story spells text -- the META bibliography included, which is
/// how an author's Å survives the trip (Aa-machine: LANG).
///
/// Fails for a table the chunk cannot hold whole -- and, a Rust
/// boundary the reference never meets, for an entry naming a
/// codepoint no `char` can hold (a lone surrogate, or past
/// U+10FFFF), refused rather than replaced.
fn extended(lang: &IffChunk) -> Result<Vec<char>, VoxamError> {
    let payload = &lang.payload;

    if payload.len() < LANG_OFFSETS {
        return Err(story_error(
            "the LANG chunk is too short for its own offsets (Aa-machine: LANG)".into(),
        ));
    }

    let at = usize::from(u16::from_be_bytes([payload[2], payload[3]]));

    if at >= payload.len() {
        return Err(story_error(
            "the LANG extended table sits past the chunk's end (Aa-machine: LANG)".into(),
        ));
    }

    let count = usize::from(payload[at]);
    let table_end = at + 1 + count * 5;

    if table_end > payload.len() {
        return Err(story_error(
            "the LANG extended table ends mid-entry (Aa-machine: LANG)".into(),
        ));
    }

    (0..count)
        .map(|entry| {
            let seat = at + 1 + entry * 5 + 2;
            let point =
                u32::from_be_bytes([0, payload[seat], payload[seat + 1], payload[seat + 2]]);

            char::from_u32(point).ok_or_else(|| {
                story_error(format!(
                    "a LANG extended entry names {point:#x}, which is no Unicode \
                     character (Aa-machine: LANG)"
                ))
            })
        })
        .collect()
}

/// The META bibliography by field name; empty without a chunk.
///
/// Unknown identifiers are passed over -- the chunk is optional
/// and additive by design -- but a chunk that runs out of bytes
/// mid-entry is refused rather than half-read.
fn metadata(
    held: Option<&IffChunk>,
    extended: &[char],
) -> Result<Vec<(String, String)>, VoxamError> {
    let Some(held) = held else {
        return Ok(Vec::new());
    };

    let payload = &held.payload;

    if payload.is_empty() {
        return Ok(Vec::new());
    }

    let mut fields: Vec<(String, String)> = Vec::new();
    let mut at = 1;

    for _ in 0..payload[0] {
        if at >= payload.len() {
            return Err(story_error(
                "the META chunk ends mid-entry (Aa-machine: META)".into(),
            ));
        }

        let identifier = payload[at];
        let Some(ended) = payload[at + 1..].iter().position(|&byte| byte == 0) else {
            return Err(story_error(
                "a META string is missing its null ending (Aa-machine: META)".into(),
            ));
        };
        let ended = at + 1 + ended;

        if let Some(&(_, name)) = META_NAMES.iter().find(|&&(code, _)| code == identifier) {
            let value = worded(&payload[at + 1..ended], extended)?;

            match fields.iter_mut().find(|(field, _)| field == name) {
                Some(seat) => seat.1 = value,
                None => fields.push((name.to_string(), value)),
            }
        }

        at = ended + 1;
    }

    Ok(fields)
}

// Where the extended characters begin in a spelled string's bytes.
const EXTENDED_START: u8 = 0x80;

/// A spelled string's text, the story's own character space.
///
/// Bytes below $80 are ASCII; $80 and above index the LANG chunk's
/// extended character table -- the author's Å is byte $80 pointing
/// at its Unicode seat, not any general-purpose encoding
/// (Aa-machine: LANG).
fn worded(raw: &[u8], extended: &[char]) -> Result<String, VoxamError> {
    let mut pieces = String::new();

    for &byte in raw {
        if byte < EXTENDED_START {
            pieces.push(byte as char);
        } else if usize::from(byte - EXTENDED_START) < extended.len() {
            pieces.push(extended[usize::from(byte - EXTENDED_START)]);
        } else {
            return Err(story_error(format!(
                "a spelled byte {byte:#04x} points past the {}-entry extended \
                 table (Aa-machine: LANG)",
                extended.len()
            )));
        }
    }

    Ok(pieces)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iff::chunk;

    // A minimal LANG: four offsets, the extended table at byte 8
    // holding one character -- an Å, lowercase å beside it.
    fn lang() -> Vec<u8> {
        let mut held = Vec::new();

        held.extend_from_slice(&[0x00, 0x00]);
        held.extend_from_slice(&8u16.to_be_bytes());
        held.extend_from_slice(&[0x00, 0x00]);
        held.extend_from_slice(&[0x00, 0x00]);
        held.extend_from_slice(&[1, 0xE5, 0xC5]);
        held.extend_from_slice(&[0x00, 0x00, 0xC5]);

        held
    }

    // A META naming a title and an author whose Å is byte $80 --
    // the extended table's first seat -- with an unknown
    // identifier to pass over.
    fn meta() -> Vec<u8> {
        let mut held = vec![3];

        held.extend_from_slice(b"\x01Cloak\x00");
        held.extend_from_slice(b"\x02\x80kesson\x00");
        held.extend_from_slice(b"\x63x\x00");

        held
    }

    const IFID: &[u8] = b"UUID://a5aa4f02-8f50-4649-a4bd-b1b5c5408b67//\x00";

    struct Headed {
        version: (u8, u8),
        wordsz: u8,
        crc: Option<u32>,
        ifid: Vec<u8>,
    }

    impl Default for Headed {
        fn default() -> Self {
            Self {
                version: (0, 5),
                wordsz: 2,
                crc: None,
                ifid: Vec::new(),
            }
        }
    }

    // A HEAD payload; None for the crc means compute it right.
    fn headed(shape: Headed) -> Vec<u8> {
        let crc = shape.crc.unwrap_or_else(|| {
            let mut running = 0;

            for name in &SUMMED {
                let payload = if name == b"LANG" { lang() } else { Vec::new() };

                running = crc32(&payload, running);
            }

            running
        });

        let mut head = vec![shape.version.0, shape.version.1, shape.wordsz, 1];

        head.extend_from_slice(&7u16.to_be_bytes());
        head.extend_from_slice(b"260827");
        head.extend_from_slice(&crc.to_be_bytes());
        head.extend_from_slice(&16u16.to_be_bytes());
        head.extend_from_slice(&8u16.to_be_bytes());
        head.extend_from_slice(&32u16.to_be_bytes());
        head.extend_from_slice(&shape.ifid);

        head
    }

    struct Storied {
        head: Option<Vec<u8>>,
        meta: Option<Vec<u8>>,
        drop: Option<[u8; 4]>,
        lead: [u8; 4],
    }

    impl Default for Storied {
        fn default() -> Self {
            Self {
                head: None,
                meta: Some(meta()),
                drop: None,
                lead: *b"HEAD",
            }
        }
    }

    // One assembled .aastory, tweakable toward every refusal.
    fn storied(shape: Storied) -> Vec<u8> {
        let head = shape.head.unwrap_or_else(|| headed(Headed::default()));
        let mut pieces = chunk(&shape.lead, &head);

        if let Some(meta) = &shape.meta {
            pieces.extend(chunk(b"META", meta));
        }

        for name in &SUMMED {
            if Some(*name) == shape.drop {
                continue;
            }

            let payload = if name == b"LANG" { lang() } else { Vec::new() };

            pieces.extend(chunk(name, &payload));
        }

        pieces.extend(chunk(b"FILE", b"one"));
        pieces.extend(chunk(b"FILE", b"two"));

        let mut body = b"AAVM".to_vec();

        body.extend(pieces);

        chunk(b"FORM", &body)
    }

    fn plain() -> Vec<u8> {
        storied(Storied::default())
    }

    fn refused(data: &[u8]) -> String {
        Story::new(data)
            .expect_err("the door should refuse")
            .to_string()
    }

    // The header's claims land whole: the version pair, the sizes,
    // the serial, the verified checksum -- and the story's own
    // character table spells the author's Å, byte $80 through LANG.
    #[test]
    fn stories_read_their_headers_whole() {
        let story = Story::new(&plain()).unwrap();

        assert_eq!(story.version, (0, 5));
        assert_eq!(story.word_size, 2);
        assert_eq!(story.shift, 1);
        assert_eq!(story.release, 7);
        assert_eq!(story.serial, "260827");
        assert_eq!(
            (story.heap_size, story.aux_size, story.ram_size),
            (16, 8, 32)
        );
        assert_eq!(story.ifid, None);
        assert_eq!(story.extended, vec!['Å']);
        assert_eq!(
            story.meta,
            vec![
                ("title".to_string(), "Cloak".to_string()),
                ("author".to_string(), "Åkesson".to_string()),
            ]
        );
        assert_eq!(story.meta_field("author"), Some("Åkesson"));
        assert_eq!(story.files().count(), 2);
        assert!(story.chunk(b"WRIT").is_some());
        assert!(story.chunk(b"URLS").is_none());
    }

    // The optional IFID unwraps from its UUID dressing, uppercased
    // as the treaty spells identities; a bare story answers None,
    // and a field dressed wrong is refused by its own text.
    #[test]
    fn the_ifid_unwraps_or_refuses() {
        let branded = Story::new(&storied(Storied {
            head: Some(headed(Headed {
                ifid: IFID.to_vec(),
                ..Headed::default()
            })),
            ..Storied::default()
        }))
        .unwrap();

        assert_eq!(
            branded.ifid.as_deref(),
            Some("A5AA4F02-8F50-4649-A4BD-B1B5C5408B67")
        );

        let told = refused(&storied(Storied {
            head: Some(headed(Headed {
                ifid: b"GUID://nope//\x00".to_vec(),
                ..Headed::default()
            })),
            ..Storied::default()
        }));

        assert!(told.contains("not UUID"), "{told}");
    }

    // Every door refusal speaks its reason: the wrong form, a HEAD
    // missing or short, a future major version, an unspoken word
    // size, a summed chunk missing, and a checksum that disagrees.
    #[test]
    fn the_door_refusals_speak() {
        let mut alien = b"IFRS".to_vec();

        alien.extend(chunk(b"HEAD", &headed(Headed::default())));

        assert!(refused(&chunk(b"FORM", &alien)).contains("FORM AAVM"));
        assert!(
            refused(&storied(Storied {
                lead: *b"HEAP",
                ..Storied::default()
            }))
            .contains("first chunk")
        );
        assert!(
            refused(&storied(Storied {
                head: Some(headed(Headed::default())[..12].to_vec()),
                ..Storied::default()
            }))
            .contains("fixed header")
        );
        assert!(
            refused(&storied(Storied {
                head: Some(headed(Headed {
                    version: (2, 0),
                    ..Headed::default()
                })),
                ..Storied::default()
            }))
            .contains("future")
        );

        let sized = refused(&storied(Storied {
            head: Some(headed(Headed {
                wordsz: 4,
                ..Headed::default()
            })),
            ..Storied::default()
        }));

        assert!(sized.contains("only size"), "{sized}");
        assert!(
            refused(&storied(Storied {
                drop: Some(*b"WRIT"),
                ..Storied::default()
            }))
            .contains("WRIT chunk is missing")
        );
        assert!(
            refused(&storied(Storied {
                head: Some(headed(Headed {
                    crc: Some(0xDEAD_BEEF),
                    ..Headed::default()
                })),
                ..Storied::default()
            }))
            .contains("header claims")
        );
    }

    // The META chunk's own refusals: a count past the bytes, a
    // string missing its null -- and a story without META answers
    // an empty bibliography rather than a missing one.
    #[test]
    fn meta_refusals_and_absence() {
        let bare = Story::new(&storied(Storied {
            meta: None,
            ..Storied::default()
        }))
        .unwrap();

        assert!(bare.meta.is_empty());

        let hollow = Story::new(&storied(Storied {
            meta: Some(Vec::new()),
            ..Storied::default()
        }))
        .unwrap();

        assert!(hollow.meta.is_empty());

        let with_meta = |meta: Vec<u8>| {
            refused(&storied(Storied {
                meta: Some(meta),
                ..Storied::default()
            }))
        };

        assert!(with_meta([&[2u8][..], b"\x01Cloak\x00"].concat()).contains("mid-entry"));
        assert!(with_meta([&[1u8][..], b"\x01Cloak"].concat()).contains("null ending"));
        assert!(with_meta([&[1u8][..], b"\x01\x81x\x00"].concat()).contains("past the"));
    }

    // The LANG chunk's own refusals: too short for its offsets, an
    // extended table past the end, and a table that ends mid-entry.
    #[test]
    fn lang_refusals() {
        let worded = |lang: Vec<u8>| {
            let mut running = 0;

            for name in &SUMMED {
                let payload = if name == b"LANG" {
                    lang.clone()
                } else {
                    Vec::new()
                };

                running = crc32(&payload, running);
            }

            let head = headed(Headed {
                crc: Some(running),
                ..Headed::default()
            });
            let mut pieces = chunk(b"HEAD", &head);

            for name in &SUMMED {
                let payload = if name == b"LANG" {
                    lang.clone()
                } else {
                    Vec::new()
                };

                pieces.extend(chunk(name, &payload));
            }

            let mut body = b"AAVM".to_vec();

            body.extend(pieces);

            refused(&chunk(b"FORM", &body))
        };

        assert!(worded(vec![0, 0]).contains("own offsets"));

        let mut runaway = vec![0, 0];

        runaway.extend_from_slice(&99u16.to_be_bytes());
        runaway.extend_from_slice(&[0, 0, 0, 0]);

        assert!(worded(runaway).contains("past the chunk"));

        let mut cut = vec![0, 0];

        cut.extend_from_slice(&8u16.to_be_bytes());
        cut.extend_from_slice(&[0, 0, 0, 0, 2, 0xE5]);

        assert!(worded(cut).contains("mid-entry"));
    }

    // The CRC-32 is zlib's own: the empty string sums to zero, and
    // the check value for "123456789" is the classic CBF43926.
    #[test]
    fn the_crc_speaks_zlib() {
        assert_eq!(crc32(b"", 0), 0);
        assert_eq!(crc32(b"123456789", 0), 0xCBF4_3926);
        assert_eq!(crc32(b"56789", crc32(b"1234", 0)), 0xCBF4_3926);
    }
}
