//! The Å-machine's speech: bitstreams, the dictionary, the charset.
//!
//! Strings live in WRIT as packed bitstreams, MSB first, each
//! stream opening on a byte boundary; the LANG chunk's decoding
//! table walks them a bit at a time -- a Huffman-inspired tree
//! whose bytes spell characters, jumps, an end mark, and an escape
//! that carries the far characters and whole dictionary words
//! (Aa-machine: LANG; WRIT). The dictionary's own words are plain
//! arrays in the story's character set: ASCII below $80, the LANG
//! extended table above it (Aa-machine: Text; DICT).
//!
//! The escape changed shape at story format 0.4 -- seven fixed
//! bits before, a table-sized read after -- and this decoder
//! speaks both, choosing by the story's own version claim.

use crate::aamachine::story::Story;
use crate::errors::VoxamError;

// The decoding table's byte meanings (Aa-machine: LANG): a direct
// character rides as $20 + x, the escape and end marks stand
// alone, and anything above $80 jumps to another table entry.
const END: u8 = 0x80;
const ESCAPE: u8 = 0x5F;
const DIRECT_TOP: u8 = 0x7F;
const CHARACTER_BASE: u32 = 0x20;

// Where the game-specific characters begin in the character set
// (Aa-machine: Text).
const EXTENDED_START: u32 = 0x80;

// The old escape's fixed read, and its floor: format 0.3 and
// earlier read seven bits and refuse a result below $20
// (Aa-machine: LANG).
const OLD_ESCAPE_BITS: u32 = 7;
const OLD_ESCAPE_FLOOR: u32 = 0x20;

// The new escape's character band starts at $A0: the first 32
// extended characters travel directly in the tree (Aa-machine:
// LANG).
const NEW_ESCAPE_BASE: u32 = 0xA0;
const DIRECT_EXTENDED: usize = 32;

// The format where the escape changed shape.
const NEW_ESCAPE_VERSION: (u8, u8) = (0, 4);

// A dictionary entry: a length byte and a two-byte offset, after
// the two-byte word count (Aa-machine: DICT).
const COUNT_SIZE: usize = 2;
const ENTRY_SIZE: usize = 3;

// The packing grain: bits fill bytes MSB first (Aa-machine: WRIT).
const BYTE_BITS: u32 = 8;

fn text_error(message: String) -> VoxamError {
    VoxamError::AAMachine(message)
}

/// One story's whole text apparatus, ready to spell.
///
/// The chunk payloads are copied out of the story at the door --
/// the state-view departure inverted: speech is small, the borrow
/// is long, and the story stays free to travel.
#[derive(Debug)]
pub struct Speech {
    version: (u8, u8),
    extended: Vec<char>,
    shift: u8,
    lang: Vec<u8>,
    table_at: usize,
    writ: Vec<u8>,
    /// The dictionary, decoded in order -- each word in the
    /// story's own character set.
    pub words: Vec<String>,
}

impl Speech {
    /// Gather the LANG table, the dictionary, and WRIT.
    ///
    /// The three chunks stand certified present: they are summed,
    /// and the story verified its checksum at the door. Fails for
    /// a dictionary the chunk cannot hold whole.
    pub fn new(story: &Story) -> Result<Self, VoxamError> {
        let lang = story.summed(b"LANG").payload.clone();
        let table_at = usize::from(u16::from_be_bytes([lang[0], lang[1]]));
        let words = worded_dictionary(&story.summed(b"DICT").payload, &story.extended)?;

        Ok(Self {
            version: story.version,
            extended: story.extended.clone(),
            shift: story.shift,
            lang,
            table_at,
            writ: story.summed(b"WRIT").payload.clone(),
            words,
        })
    }

    /// Decode one string from its byte address in WRIT.
    ///
    /// The walk starts at the table's root and returns there after
    /// every produced piece; a jump byte moves the walk, the end
    /// byte closes it (Aa-machine: LANG). Fails for an address
    /// outside WRIT, a walk past the table or the stream, or an
    /// escape the story has no characters or words to answer.
    pub fn spelled(&self, address: usize) -> Result<String, VoxamError> {
        if address >= self.writ.len() {
            return Err(text_error(format!(
                "string address {address} lies outside WRIT's {} bytes \
                 (Aa-machine: WRIT)",
                self.writ.len()
            )));
        }

        let mut bits = Bits::new(&self.writ, address);
        let mut pieces = String::new();
        let mut entry = 0;

        loop {
            let told = self.entry(entry)?[bits.take(1)? as usize];

            if told == END {
                break;
            }

            if told == ESCAPE {
                pieces.push_str(&self.escaped(&mut bits)?);
                entry = 0;
            } else if told <= DIRECT_TOP {
                pieces.push(self.character(CHARACTER_BASE + u32::from(told))?);
                entry = 0;
            } else {
                entry = usize::from(told - END);
            }
        }

        Ok(pieces)
    }

    /// Decode the string a shifted pointer names.
    ///
    /// A string pointer is a shifted byte address in WRIT: tiny
    /// pointers are shifted right by one bit, short and long
    /// pointers by the header's own shift amount -- so the way
    /// back is a left shift by the same (Aa-machine: Runtime
    /// data).
    pub fn pointed(&self, pointer: u32, tiny: bool) -> Result<String, VoxamError> {
        let shift = if tiny { 1 } else { u32::from(self.shift) };
        let address = (pointer as usize).checked_shl(shift).unwrap_or(usize::MAX);

        self.spelled(address)
    }

    /// One decoding-table pair, bounds held loud.
    fn entry(&self, entry: usize) -> Result<[u8; 2], VoxamError> {
        let at = self.table_at + entry * 2;

        if at + 2 > self.lang.len() {
            return Err(text_error(format!(
                "the decoding walk reached entry {entry}, past the LANG \
                 chunk's end (Aa-machine: LANG)"
            )));
        }

        Ok([self.lang[at], self.lang[at + 1]])
    }

    /// One escape's yield: a far character, or a whole word.
    ///
    /// Format 0.3 and earlier read seven fixed bits; 0.4 and later
    /// size the read by the extended characters beyond the tree's
    /// reach plus the dictionary, a word arriving with its own
    /// leading space (Aa-machine: LANG).
    fn escaped(&self, bits: &mut Bits<'_>) -> Result<String, VoxamError> {
        if self.version < NEW_ESCAPE_VERSION {
            let told = bits.take(OLD_ESCAPE_BITS)?;

            if told < OLD_ESCAPE_FLOOR {
                return Err(text_error(format!(
                    "an escape read {told:#04x}, below the ${OLD_ESCAPE_FLOOR:02x} \
                     floor the old escape requires (Aa-machine: LANG)"
                )));
            }

            return Ok(self.character(EXTENDED_START + told)?.to_string());
        }

        let beyond = self.extended.len().saturating_sub(DIRECT_EXTENDED);
        let total = beyond + self.words.len();

        if total == 0 {
            return Err(text_error(
                "an escape appears, but the story has no far characters and no \
                 dictionary words to answer it (Aa-machine: LANG)"
                    .into(),
            ));
        }

        let told = bits.take(bit_width(total))?;

        if (told as usize) < beyond {
            return Ok(self.character(NEW_ESCAPE_BASE + told)?.to_string());
        }

        if told as usize - beyond >= self.words.len() {
            return Err(text_error(format!(
                "an escape read {told}, past the {total} answers the story \
                 holds (Aa-machine: LANG)"
            )));
        }

        Ok(format!(" {}", self.words[told as usize - beyond]))
    }

    /// One character-set code as text (Aa-machine: Text).
    fn character(&self, code: u32) -> Result<char, VoxamError> {
        charactered(code, &self.extended)
    }
}

/// One character-set code as text, the extended table ruling.
fn charactered(code: u32, extended: &[char]) -> Result<char, VoxamError> {
    if code < EXTENDED_START {
        return Ok(char::from_u32(code).expect("ASCII is always a character"));
    }

    let seat = (code - EXTENDED_START) as usize;

    if seat < extended.len() {
        return Ok(extended[seat]);
    }

    Err(text_error(format!(
        "character {code:#04x} points past the {}-entry extended table \
         (Aa-machine: LANG)",
        extended.len()
    )))
}

/// How many bits the new escape reads: ceil(log2(total)).
fn bit_width(total: usize) -> u32 {
    if total > 1 {
        usize::BITS - (total - 1).leading_zeros()
    } else {
        0
    }
}

/// The DICT chunk's words, decoded in order (Aa-machine: DICT).
fn worded_dictionary(payload: &[u8], extended: &[char]) -> Result<Vec<String>, VoxamError> {
    if payload.len() < COUNT_SIZE {
        return Err(text_error(
            "the DICT chunk is too short for its own count (Aa-machine: DICT)".into(),
        ));
    }

    let count = usize::from(u16::from_be_bytes([payload[0], payload[1]]));
    let table_end = COUNT_SIZE + count * ENTRY_SIZE;

    if table_end > payload.len() {
        return Err(text_error(format!(
            "the DICT table claims {count} words, past the chunk's {} bytes \
             (Aa-machine: DICT)",
            payload.len()
        )));
    }

    let mut words = Vec::with_capacity(count);

    for held in 0..count {
        let at = COUNT_SIZE + held * ENTRY_SIZE;
        let length = usize::from(payload[at]);
        let start = usize::from(u16::from_be_bytes([payload[at + 1], payload[at + 2]]));

        if start + length > payload.len() {
            return Err(text_error(format!(
                "dictionary word {held} runs past the chunk's end (Aa-machine: DICT)"
            )));
        }

        words.push(
            payload[start..start + length]
                .iter()
                .map(|&code| charactered(u32::from(code), extended))
                .collect::<Result<String, _>>()?,
        );
    }

    Ok(words)
}

/// A bitstream over WRIT, MSB first (Aa-machine: WRIT).
struct Bits<'a> {
    data: &'a [u8],
    byte: usize,
    bit: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8], at: usize) -> Self {
        Self {
            data,
            byte: at,
            bit: 0,
        }
    }

    /// The next count bits as an integer, MSB first.
    fn take(&mut self, count: u32) -> Result<u32, VoxamError> {
        let mut told = 0;

        for _ in 0..count {
            if self.byte >= self.data.len() {
                return Err(text_error(
                    "the bitstream ran out mid-string (Aa-machine: WRIT)".into(),
                ));
            }

            let bit = (self.data[self.byte] >> (BYTE_BITS - 1 - self.bit)) & 1;

            told = (told << 1) | u32::from(bit);
            self.bit += 1;

            if self.bit == BYTE_BITS {
                self.bit = 0;
                self.byte += 1;
            }
        }

        Ok(told)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aamachine::story::{SUMMED, crc32};
    use crate::iff::chunk as iff_chunk;

    // The workhorse table: entry 0 offers the letter a or a jump
    // to entry 1; entry 1 offers the escape or the end mark. Every
    // walk below is a sentence in this two-entry language.
    const TABLE: &[u8] = &[0x41, 0x81, 0x5F, 0x80];

    // A one-entry table whose either bit jumps far past the chunk.
    const RUNAWAY_TABLE: &[u8] = &[0xFF, 0xFF];

    // A one-entry table whose first bit is the character-set byte
    // $80: the first extended character, ridden directly.
    const EXTENDED_TABLE: &[u8] = &[0x60, 0x80];

    // A LANG payload: offset header, decoding table, extended table.
    fn langed(table: &[u8], extended: &[u32]) -> Vec<u8> {
        let mut charactered = vec![extended.len() as u8];

        for &point in extended {
            charactered.push((point & 0xFF) as u8);
            charactered.push((point & 0xFF) as u8);
            charactered.extend_from_slice(&point.to_be_bytes()[1..]);
        }

        let mut held = Vec::new();

        held.extend_from_slice(&8u16.to_be_bytes());
        held.extend_from_slice(&((8 + table.len()) as u16).to_be_bytes());
        held.extend_from_slice(&[0, 0, 0, 0]);
        held.extend_from_slice(table);
        held.extend(charactered);

        held
    }

    // A DICT payload holding the given words, arrays after the table.
    fn worded(words: &[&[u8]]) -> Vec<u8> {
        let table_end = 2 + 3 * words.len();
        let mut entries = Vec::new();
        let mut arrays = Vec::new();
        let mut at = table_end;

        for word in words {
            entries.push(word.len() as u8);
            entries.extend_from_slice(&(at as u16).to_be_bytes());
            arrays.extend_from_slice(word);
            at += word.len();
        }

        let mut held = (words.len() as u16).to_be_bytes().to_vec();

        held.extend(entries);
        held.extend(arrays);

        held
    }

    struct Storied {
        lang: Vec<u8>,
        writ: Vec<u8>,
        dictionary: Vec<u8>,
        version: (u8, u8),
        shift: u8,
    }

    impl Storied {
        fn of(lang: Vec<u8>) -> Self {
            Self {
                lang,
                writ: Vec::new(),
                dictionary: vec![0, 0],
                version: (0, 5),
                shift: 0,
            }
        }
    }

    // A minimal story around the given LANG, WRIT, and DICT.
    fn storied(shape: Storied) -> Story {
        let summed = |name: &[u8; 4]| -> Vec<u8> {
            match name {
                b"LANG" => shape.lang.clone(),
                b"WRIT" => shape.writ.clone(),
                b"DICT" => shape.dictionary.clone(),
                _ => Vec::new(),
            }
        };

        let mut crc = 0;

        for name in &SUMMED {
            crc = crc32(&summed(name), crc);
        }

        let mut head = vec![shape.version.0, shape.version.1, 2, shape.shift];

        head.extend_from_slice(&1u16.to_be_bytes());
        head.extend_from_slice(b"260827");
        head.extend_from_slice(&crc.to_be_bytes());
        head.extend_from_slice(&[0; 6]);

        let mut pieces = iff_chunk(b"HEAD", &head);

        for name in &SUMMED {
            pieces.extend(iff_chunk(name, &summed(name)));
        }

        let mut body = b"AAVM".to_vec();

        body.extend(pieces);

        Story::new(&iff_chunk(b"FORM", &body)).unwrap()
    }

    // Bits as WRIT bytes, MSB first, zero-padded to the boundary.
    //
    // Spaces group the bits by meaning -- one walk step, one
    // escape read -- and pack to nothing.
    fn packed(bits: &str) -> Vec<u8> {
        let told: String = bits.chars().filter(|held| !held.is_whitespace()).collect();
        let padding = told.len().next_multiple_of(8) - told.len();
        let padded = format!("{told}{}", "0".repeat(padding));

        (0..padded.len())
            .step_by(8)
            .map(|at| u8::from_str_radix(&padded[at..at + 8], 2).unwrap())
            .collect()
    }

    // Thirty-three extended characters, the last beyond the tree.
    //
    // Thirty-two ride the decoding tree directly; the thirty-third
    // -- an e-acute at seat 32 -- is reachable only by escape, in
    // both the old seven-bit shape and the new sized read.
    fn far_extended() -> Vec<u32> {
        let mut held: Vec<u32> = (0..32).map(|seat| 0x100 + seat).collect();

        held.push(0xE9);

        held
    }

    fn refuses(result: Result<String, VoxamError>, wants: &str) {
        let told = result.expect_err("the walk should refuse").to_string();

        assert!(told.contains(wants), "{told}");
    }

    // The simplest whole walk: bit 0 spells the letter a from the
    // root, and the 1-1 path reaches entry 1's end mark. The walk
    // returns to the root after each character, so two letters are
    // just the letter bit twice.
    #[test]
    fn direct_characters_spell_and_the_end_mark_closes() {
        let mut shape = Storied::of(langed(TABLE, &[]));

        shape.writ = packed("0 0 11");

        let speech = Speech::new(&storied(shape)).unwrap();

        assert_eq!(speech.spelled(0).unwrap(), "aa");
    }

    // Streams begin on byte boundaries: a string at address 1
    // decodes untroubled by the noise byte before it (Aa-machine:
    // WRIT).
    #[test]
    fn a_stream_opens_on_its_own_byte_boundary() {
        let mut shape = Storied::of(langed(TABLE, &[]));

        shape.writ = [&[0xFFu8][..], &packed("0 11")].concat();

        let speech = Speech::new(&storied(shape)).unwrap();

        assert_eq!(speech.spelled(1).unwrap(), "a");
    }

    // Table bytes $60 to $7f spell characters $80 and up directly:
    // the first 32 extended characters ride the tree without any
    // escape (Aa-machine: LANG).
    #[test]
    fn the_tree_carries_near_extended_characters_directly() {
        let mut shape = Storied::of(langed(EXTENDED_TABLE, &[0xC5]));

        shape.writ = packed("0 1");

        let speech = Speech::new(&storied(shape)).unwrap();

        assert_eq!(speech.spelled(0).unwrap(), "Å");
    }

    // A direct extended character with no table behind it is
    // refused by the character set, not silently blanked.
    #[test]
    fn a_character_past_the_extended_table_is_refused() {
        let mut shape = Storied::of(langed(EXTENDED_TABLE, &[]));

        shape.writ = packed("0 1");

        let speech = Speech::new(&storied(shape)).unwrap();

        refuses(speech.spelled(0), "past the 0-entry extended table");
    }

    // Before format 0.4 the escape reads seven fixed bits and
    // spells character $80 + X: here X is $20, the escape band's
    // own floor, landing on extended seat 32 (Aa-machine: LANG).
    #[test]
    fn the_old_escape_reads_seven_bits() {
        let mut shape = Storied::of(langed(TABLE, &far_extended()));

        shape.writ = packed("10 0100000 11");
        shape.version = (0, 3);

        let speech = Speech::new(&storied(shape)).unwrap();

        assert_eq!(speech.spelled(0).unwrap(), "é");
    }

    // The old escape refuses a read below $20: those seats belong
    // to the control characters no string may spell (Aa-machine:
    // LANG).
    #[test]
    fn the_old_escape_refuses_a_read_below_its_floor() {
        let mut shape = Storied::of(langed(TABLE, &[]));

        shape.writ = packed("10 0011111 11");
        shape.version = (0, 3);

        let speech = Speech::new(&storied(shape)).unwrap();

        refuses(speech.spelled(0), "below the $20 floor");
    }

    // From format 0.4 the escape's read is sized by the far
    // extended characters plus the dictionary: 33 extended
    // characters put one beyond the tree's reach, one word joins
    // it, and the two-answer read takes a single bit. X = 0 is the
    // far character.
    #[test]
    fn the_new_escape_reaches_the_far_characters() {
        let mut shape = Storied::of(langed(TABLE, &far_extended()));

        shape.writ = packed("10 0 11");
        shape.dictionary = worded(&[b"xyzzy"]);

        let speech = Speech::new(&storied(shape)).unwrap();

        assert_eq!(speech.spelled(0).unwrap(), "é");
    }

    // The same escape's other answer: X past the far characters is
    // a dictionary word, arriving with its own leading space.
    #[test]
    fn the_new_escape_spells_a_dictionary_word() {
        let mut shape = Storied::of(langed(TABLE, &far_extended()));

        shape.writ = packed("0 10 1 11");
        shape.dictionary = worded(&[b"xyzzy"]);

        let speech = Speech::new(&storied(shape)).unwrap();

        assert_eq!(speech.spelled(0).unwrap(), "a xyzzy");
    }

    // One answer in all the world means a zero-bit read: the
    // escape produces the lone dictionary word without consuming
    // anything.
    #[test]
    fn a_lone_answer_takes_a_zero_bit_read() {
        let mut shape = Storied::of(langed(TABLE, &[]));

        shape.writ = packed("10 11");
        shape.dictionary = worded(&[b"plugh"]);

        let speech = Speech::new(&storied(shape)).unwrap();

        assert_eq!(speech.spelled(0).unwrap(), " plugh");
    }

    // An escape in a story with no far characters and no words has
    // nothing it could mean; the walk refuses it loud.
    #[test]
    fn an_escape_with_nothing_to_answer_is_refused() {
        let mut shape = Storied::of(langed(TABLE, &[]));

        shape.writ = packed("10 11");

        let speech = Speech::new(&storied(shape)).unwrap();

        refuses(speech.spelled(0), "no far characters");
    }

    // A read sized for three answers can still spell a fourth: the
    // out-of-range X is refused by name, not wrapped or clamped.
    #[test]
    fn the_new_escape_refuses_a_read_past_its_answers() {
        let mut shape = Storied::of(langed(TABLE, &[]));

        shape.writ = packed("10 11 11");
        shape.dictionary = worded(&[b"plugh", b"plover", b"zork"]);

        let speech = Speech::new(&storied(shape)).unwrap();

        refuses(speech.spelled(0), "past the 3 answers");
    }

    // A jump byte aims at a table entry; one aimed past the LANG
    // chunk stops the walk with the entry named.
    #[test]
    fn a_jump_past_the_table_is_refused() {
        let mut shape = Storied::of(langed(RUNAWAY_TABLE, &[]));

        shape.writ = packed("0 1");

        let speech = Speech::new(&storied(shape)).unwrap();

        refuses(speech.spelled(0), "entry 127, past the LANG");
    }

    // A stream that never reaches the end mark runs out of WRIT;
    // the walk refuses to invent bits past the chunk.
    #[test]
    fn a_stream_that_runs_out_is_refused() {
        let mut shape = Storied::of(langed(TABLE, &[]));

        shape.writ = vec![0x00];

        let speech = Speech::new(&storied(shape)).unwrap();

        refuses(speech.spelled(0), "ran out mid-string");
    }

    // An address outside WRIT never opens a stream at all.
    #[test]
    fn an_address_outside_writ_is_refused() {
        let mut shape = Storied::of(langed(TABLE, &[]));

        shape.writ = packed("11");

        let speech = Speech::new(&storied(shape)).unwrap();

        refuses(speech.spelled(9), "outside WRIT's 1 bytes");
    }

    // A tiny string pointer is a byte address shifted right by one
    // bit, whatever the header's shift says: pointer 1 names the
    // stream at byte 2 (Aa-machine: Runtime data).
    #[test]
    fn a_tiny_pointer_shifts_by_one_bit() {
        let mut shape = Storied::of(langed(TABLE, &[]));

        shape.writ = [&[0xFFu8, 0xFF][..], &packed("0 11")].concat();
        shape.shift = 3;

        let speech = Speech::new(&storied(shape)).unwrap();

        assert_eq!(speech.pointed(1, true).unwrap(), "a");
    }

    // Short and long pointers shift by the header's own amount:
    // with a shift of 2, pointer 1 names the stream at byte 4
    // (Aa-machine: Runtime data).
    #[test]
    fn a_pointer_shifts_by_the_header_amount() {
        let mut shape = Storied::of(langed(TABLE, &[]));

        shape.writ = [&[0xFFu8; 4][..], &packed("0 11")].concat();
        shape.shift = 2;

        let speech = Speech::new(&storied(shape)).unwrap();

        assert_eq!(speech.pointed(1, false).unwrap(), "a");
    }

    // The dictionary decodes in order, through the story's own
    // character space: byte $80 is the extended table's first seat
    // (Aa-machine: DICT).
    #[test]
    fn the_dictionary_speaks_the_story_character_set() {
        let mut shape = Storied::of(langed(TABLE, &[0xC5]));

        shape.dictionary = worded(&[b"\x80mulet", b"lamp"]);

        let speech = Speech::new(&storied(shape)).unwrap();

        assert_eq!(speech.words, vec!["Åmulet".to_string(), "lamp".to_string()]);
    }

    // A DICT too short for even its own count is refused at the
    // door.
    #[test]
    fn a_dict_too_short_for_its_count_is_refused() {
        let mut shape = Storied::of(langed(TABLE, &[]));

        shape.dictionary = vec![0x00];

        let told = Speech::new(&storied(shape))
            .expect_err("too short")
            .to_string();

        assert!(told.contains("too short for its own count"), "{told}");
    }

    // A count that claims more entries than the chunk holds is a
    // lie the table refuses whole.
    #[test]
    fn a_dict_table_past_the_chunk_is_refused() {
        let mut shape = Storied::of(langed(TABLE, &[]));

        shape.dictionary = 9u16.to_be_bytes().to_vec();

        let told = Speech::new(&storied(shape))
            .expect_err("a lying count")
            .to_string();

        assert!(told.contains("claims 9 words"), "{told}");
    }

    // An entry whose character array runs past the chunk's end is
    // refused by its seat number.
    #[test]
    fn a_dict_word_past_the_chunk_is_refused() {
        let mut tabled = 1u16.to_be_bytes().to_vec();

        tabled.push(200);
        tabled.extend_from_slice(&5u16.to_be_bytes());

        let mut shape = Storied::of(langed(TABLE, &[]));

        shape.dictionary = tabled;

        let told = Speech::new(&storied(shape))
            .expect_err("a runaway word")
            .to_string();

        assert!(told.contains("word 0 runs past"), "{told}");
    }
}
