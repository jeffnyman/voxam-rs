//! Decoding encoded text into readable strings (§3).
//!
//! Encoded text is a sequence of words, each holding three 5-bit
//! Z-characters (§3.2). A Z-character is a storage unit, not a
//! character: what it means depends on the version, the current
//! alphabet, and its neighbours. The same bytes decode to different
//! text under different version bytes.
//!
//! One departure from the Python reference, forced by the
//! languages' string models: Python strings carry lone UTF-16
//! surrogate halves and voxam fuses them at its print seams, but a
//! Rust `String` cannot hold them at all. Text therefore decodes
//! internally to a stream of 16-bit units, and the fusing --
//! adjacent halves becoming their astral character, orphans
//! becoming the replacement character -- happens whenever units
//! become a `String`. The composition is the reference's exact
//! fuse-after-decode behaviour.

use crate::errors::VoxamError;
use crate::zmachine::memory::Memory;

/// Only the last word of a string has its top bit set (§3.2).
const STRING_TERMINATOR_BIT: u16 = 0x8000;

/// Three Z-characters per word: bits 14-10, 9-5, and 4-0 (§3.2).
const Z_CHAR_SHIFTS: [u16; 3] = [10, 5, 0];
const Z_CHAR_MASK: u16 = 0x1F;

/// Z-character 0 is a space (§3.5.1); in Version 1, Z-character 1
/// is a new-line (§3.5.2).
const SPACE: u8 = 0;
const V1_NEWLINE: u8 = 1;

/// In Versions 1 and 2, Z-characters 2 and 3 shift the alphabet
/// for one character and 4 and 5 lock it (§3.2.2). From Version 3,
/// only 4 and 5 shift -- absolutely, for one character -- and 1 to
/// 3 introduce abbreviations (§3.2.3, §3.3).
const LAST_SHIFT_LOCK_VERSION: u8 = 2;
const FIRST_ABBREVIATION_VERSION: u8 = 3;
const V2_ABBREVIATION_CHAR: u8 = 1;

/// Abbreviation z then x names entry 32(z - 1) + x of the table the
/// header points at; each entry is a word address, doubled to reach
/// bytes (§3.3, §1.2.2).
const ABBREVIATION_BANK_SIZE: usize = 32;
const WORD_ADDRESS_SCALE: usize = 2;
const WORD_SIZE: usize = 2;

/// In alphabet A2, character 6 escapes to a ten-bit ZSCII code and
/// character 7 is a new-line, except in Version 1 (§3.4, §3.5.3).
const A2: usize = 2;
const ESCAPE: u8 = 6;
const A2_NEWLINE: u8 = 7;

/// The alphabet rows for Z-characters 6 to 31 (§3.5.3). Leading A2
/// entries marked ? are placeholders: the escape (all versions) and
/// the new-line (Version 2 up) are handled before any table lookup.
/// Version 1 keeps the escape but has no A2 new-line -- its
/// new-line is Z-character 1 -- freeing a slot for the < character
/// (§3.5.4).
const ALPHABET_A0: &str = "abcdefghijklmnopqrstuvwxyz";
const ALPHABET_A1: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const ALPHABET_A2: &str = "??0123456789.,!?_#'\"/\\-:()";
const ALPHABET_A2_V1: &str = "?0123456789.,!?_#'\"/\\<-:()";

/// From Version 5 the header word at $34 may name a custom alphabet
/// table: 78 bytes as 3 blocks of 26 ZSCII values for Z-characters
/// 6 to 31 of A0, A1, and A2 -- except that A2's characters 6 and 7
/// stay the escape and the new-line whatever the table says
/// (§3.5.5, §3.5.5.1).
const CUSTOM_ALPHABET_VERSION: u8 = 5;
const ALPHABET_LENGTH: usize = 26;
const A2_FIXED_ENTRIES: usize = 2;

/// A run of 16-bit text units: what decoded text is before it
/// becomes a `String`, and the shape a UTF-16-era surrogate half
/// can survive in.
pub type Units = Vec<u16>;

/// One alphabet row is 26 slots, each holding whatever its entry
/// prints -- usually one character, but a null slot converts to
/// nothing (§3.8.2.1) and a Version 6 typography code expands to a
/// run of spaces (§3.8.2.3-4). Keeping the slots separate is what
/// stops an expansion from shifting its neighbours' Z-characters.
pub type AlphabetRow = Vec<Units>;
pub type AlphabetRows = [AlphabetRow; 3];

/// ZSCII output codes: 0 is the null, "defined for output but has
/// no effect in any output stream" (§3.8.2.1); 13 is new-line; and
/// 32 to 126 agree with ASCII (§3.8.2.5, §3.8.3). Codes 8 (delete)
/// and 27 (escape) are defined for input only (§3.8.2.2).
const ZSCII_NULL: u16 = 0;
const ZSCII_DELETE: u16 = 8;
const ZSCII_NEWLINE: u16 = 13;
const ZSCII_ESCAPE: u16 = 27;
const ZSCII_PRINTABLE_START: u16 = 32;
const ZSCII_PRINTABLE_END: u16 = 126;

/// Beyond Zork prints its menu key hints as IBM display codes: in
/// the CP437 character set 24 to 27 are the arrows -- up and down
/// for its selection menus, right and left for the character
/// builder's point allocation.
const IBM_ARROWS_START: u16 = 24;
const IBM_ARROWS: [u16; 4] = [0x2191, 0x2193, 0x2192, 0x2190];

/// Version 6 -- and only Version 6 -- defines two typography codes
/// for output: 9 prints a paragraph indentation and 11 the wider
/// "sentence space" typographers put after a full stop (§3.8.2.3,
/// §3.8.2.4). A character glass renders them the way Frotz does,
/// three spaces and two; everywhere else they stay loud.
const TYPOGRAPHY_VERSION: u8 = 6;
const V6_INDENT: u16 = 9;
const V6_SENTENCE_SPACE: u16 = 11;
const UNIT_SPACE: u16 = 0x20;

/// Codes 129 to 154 are defined for input only (§3.8.4): the
/// cursor keys 129-132, the function keys 133-144, and the keypad
/// digits 145-154.
const ZSCII_INPUT_KEYS_START: u16 = 129;
const ZSCII_INPUT_KEYS_END: u16 = 154;

/// Codes 155 up are the "extra characters" (§3.8.5), defined for
/// both input and output by the default Unicode translation table
/// of §3.8.5.3 -- the accented Latin repertoire below, codepoint
/// for codepoint from Table 1. A Version 5+ story may substitute
/// its own table through the header extension (§3.8.5.2).
const ZSCII_EXTRA_START: u16 = 155;
pub const DEFAULT_EXTRAS: [u16; 69] = [
    0x0E4, 0x0F6, 0x0FC, 0x0C4, 0x0D6, 0x0DC, 0x0DF, 0x0BB, 0x0AB, 0x0EB, 0x0EF, 0x0FF, 0x0CB,
    0x0CF, 0x0E1, 0x0E9, 0x0ED, 0x0F3, 0x0FA, 0x0FD, 0x0C1, 0x0C9, 0x0CD, 0x0D3, 0x0DA, 0x0DD,
    0x0E0, 0x0E8, 0x0EC, 0x0F2, 0x0F9, 0x0C0, 0x0C8, 0x0CC, 0x0D2, 0x0D9, 0x0E2, 0x0EA, 0x0EE,
    0x0F4, 0x0FB, 0x0C2, 0x0CA, 0x0CE, 0x0D4, 0x0DB, 0x0E5, 0x0C5, 0x0F8, 0x0D8, 0x0E3, 0x0F1,
    0x0F5, 0x0C3, 0x0D1, 0x0D5, 0x0E6, 0x0C6, 0x0E7, 0x0C7, 0x0FE, 0x0F0, 0x0DE, 0x0D0, 0x0A3,
    0x153, 0x152, 0x0A1, 0x0BF,
];

const FIRST_ALPHABET_CHARACTER: u8 = 6;

/// Dictionary-form encoding is fixed-length: 6 Z-characters through
/// Version 3, 9 after, padded with 5s and guillotined past that
/// (§3.7, §13.3, §13.4).
const PAD: u8 = 5;

fn dictionary_zchars(version: u8) -> usize {
    if version <= 3 { 6 } else { 9 }
}

fn text_error(message: String) -> VoxamError {
    VoxamError::ZMachineText(message)
}

/// Convert decoded units into a `String`, fusing UTF-16 surrogate
/// halves into their characters.
///
/// The Z-Machine's unicode is 16-bit, so no conforming story can
/// name a character past $ffff (§3.8.5) -- but UTF-16-native
/// interpreters historically passed surrogate halves through to
/// displays that fused them, an accidental extension that test
/// stories now probe deliberately. A well-formed adjacent pair
/// becomes its astral character; a half with no partner becomes the
/// replacement character, because an honest blot beats a crash.
pub fn units_to_string(units: &[u16]) -> String {
    char::decode_utf16(units.iter().copied())
        .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Decode the encoded string beginning at an address (§3.2),
/// returning the text and the first address past the string.
///
/// Fails on an abbreviation breaking §3.3.1's rules, or a string
/// running outside the story file (§1.1).
pub fn decode_string(memory: &Memory, address: usize) -> Result<(String, usize), VoxamError> {
    let (units, end) = decode_units(memory, address)?;

    Ok((units_to_string(&units), end))
}

/// Decode the encoded string beginning at an address into raw text
/// units (§3.2), for callers that buffer output before display.
pub fn decode_units(memory: &Memory, address: usize) -> Result<(Units, usize), VoxamError> {
    let (zchars, end) = zchars_at(memory, address)?;

    Ok((text_of(memory, &zchars, false)?, end))
}

/// The three alphabet rows in force for this story (§3.5).
///
/// The standard rows of §3.5.3, unless a Version 5+ header names a
/// custom table at $34 (§3.5.5) -- read afresh each time, since
/// nothing stops a story keeping its table in dynamic memory. A2's
/// first two slots are placeholders: the escape and the new-line
/// are handled before any table lookup, and stay themselves even
/// under a custom table (§3.5.5.1).
pub fn alphabets(memory: &Memory) -> Result<AlphabetRows, VoxamError> {
    let header = memory.header();
    let version = header.version();
    let base = usize::from(header.alphabet_table_address());

    if version < CUSTOM_ALPHABET_VERSION || base == 0 {
        return Ok(standard_alphabets(version));
    }

    let repertoire = extras(memory)?;
    let mut rows: AlphabetRows = [Vec::new(), Vec::new(), Vec::new()];

    for (row, skip) in [(0, 0), (1, 0), (2, A2_FIXED_ENTRIES)] {
        let mut slots: AlphabetRow = if skip > 0 {
            vec![vec![u16::from(b'?')]; skip]
        } else {
            Vec::new()
        };

        for index in skip..ALPHABET_LENGTH {
            let code = memory.fetch_byte(base + row * ALPHABET_LENGTH + index)?;
            slots.push(zscii_to_units(u16::from(code), &repertoire, version)?);
        }

        rows[row] = slots;
    }

    Ok(rows)
}

/// The extra-character repertoire in force (§3.8.5).
///
/// The default table of §3.8.5.3, unless a Version 5+ story names
/// its own Unicode translation table through the header extension
/// (§3.8.5.2): a count byte N, then N words of Unicode codepoints
/// for ZSCII 155 to 155+N-1. N may legally be zero, leaving every
/// extra character undefined.
pub fn extras(memory: &Memory) -> Result<Vec<u16>, VoxamError> {
    let header = memory.header();

    if header.version() < CUSTOM_ALPHABET_VERSION {
        return Ok(DEFAULT_EXTRAS.to_vec());
    }

    let base = usize::from(header.unicode_translation_address());

    if base == 0 {
        return Ok(DEFAULT_EXTRAS.to_vec());
    }

    let count = usize::from(memory.fetch_byte(base)?);
    let mut table = Vec::with_capacity(count);

    for index in 0..count {
        table.push(memory.fetch_word(base + 1 + WORD_SIZE * index)?);
    }

    Ok(table)
}

/// Convert a ZSCII output code to its text units (§3.8).
///
/// Usually one unit, but the null converts to nothing (§3.8.2.1)
/// and the Version 6 typography codes render as runs of spaces.
/// Callers without a version pass 0, keeping those codes loud.
/// Fails for codes the repertoire leaves undefined (§3.8.5).
pub fn zscii_to_units(code: u16, extras_table: &[u16], version: u8) -> Result<Units, VoxamError> {
    if code == ZSCII_NULL {
        // Defined for output, with no effect in any stream (§3.8.2.1).
        return Ok(Vec::new());
    }

    if version == TYPOGRAPHY_VERSION && (code == V6_INDENT || code == V6_SENTENCE_SPACE) {
        let width = if code == V6_INDENT { 3 } else { 2 };

        return Ok(vec![UNIT_SPACE; width]);
    }

    if (IBM_ARROWS_START..IBM_ARROWS_START + 4).contains(&code) {
        // Undefined for output on paper (§3.8.2) -- but Beyond
        // Zork's menus print their key hints as IBM display codes,
        // trusting the interpreter's screen font: in CP437, 24 and
        // 25 are the up and down arrows. Converted here the way the
        // §16 remarks say the Zip interpreters converted Beyond
        // Zork's other IBM codes back.
        return Ok(vec![IBM_ARROWS[usize::from(code - IBM_ARROWS_START)]]);
    }

    if code == ZSCII_NEWLINE {
        return Ok(vec![u16::from(b'\n')]);
    }

    if (ZSCII_PRINTABLE_START..=ZSCII_PRINTABLE_END).contains(&code) {
        return Ok(vec![code]);
    }

    if code >= ZSCII_EXTRA_START && usize::from(code - ZSCII_EXTRA_START) < extras_table.len() {
        return Ok(vec![extras_table[usize::from(code - ZSCII_EXTRA_START)]]);
    }

    Err(text_error(format!(
        "ZSCII code {code} is not yet printable (§3.8)"
    )))
}

/// Convert a typed character to its ZSCII code (§3.8).
///
/// The input mirror of [`zscii_to_units`]: the extra characters are
/// "defined for both input and output" (§3.8.5.2.2), so a typed
/// accented letter lands in the buffer as its ZSCII code, not its
/// Unicode codepoint. Fails for characters ZSCII has no code for.
pub fn char_to_zscii(character: char, extras_table: &[u16]) -> Result<u16, VoxamError> {
    if character == '\n' {
        return Ok(ZSCII_NEWLINE);
    }

    // The input-only codes (§3.8.2.2): both classic delete bytes
    // mean ZSCII 8, and the terminal escape means ZSCII 27.
    if character == '\u{8}' || character == '\u{7f}' {
        return Ok(ZSCII_DELETE);
    }

    if character == '\u{1b}' {
        return Ok(ZSCII_ESCAPE);
    }

    let code = character as u32;

    if u32::from(ZSCII_PRINTABLE_START) <= code && code <= u32::from(ZSCII_PRINTABLE_END) {
        return Ok(code as u16);
    }

    // The cursor, function, and keypad keys travel as their §3.8.4
    // codepoints, defined for input only.
    if u32::from(ZSCII_INPUT_KEYS_START) <= code && code <= u32::from(ZSCII_INPUT_KEYS_END) {
        return Ok(code as u16);
    }

    if let Some(position) = extras_table
        .iter()
        .position(|&unit| u32::from(unit) == code)
    {
        return Ok(ZSCII_EXTRA_START + position as u16);
    }

    Err(text_error(format!(
        "the character {character:?} has no ZSCII code (§3.8)"
    )))
}

/// Convert one text unit to its ZSCII code (§3.8): the unit-level
/// mirror of [`char_to_zscii`], for text that lives as raw 16-bit
/// units -- a stream 3 table's contents, where a surrogate half
/// may legally stand (§3.8.5.4).
pub fn unit_to_zscii(unit: u16, extras_table: &[u16]) -> Result<u16, VoxamError> {
    if unit == u16::from(b'\n') {
        return Ok(ZSCII_NEWLINE);
    }

    if unit == 8 || unit == 127 {
        return Ok(ZSCII_DELETE);
    }

    if unit == 27 {
        return Ok(ZSCII_ESCAPE);
    }

    if (ZSCII_PRINTABLE_START..=ZSCII_PRINTABLE_END).contains(&unit)
        || (ZSCII_INPUT_KEYS_START..=ZSCII_INPUT_KEYS_END).contains(&unit)
    {
        return Ok(unit);
    }

    if let Some(position) = extras_table.iter().position(|&entry| entry == unit) {
        return Ok(ZSCII_EXTRA_START + position as u16);
    }

    Err(text_error(format!(
        "the character {:?} has no ZSCII code (§3.8)",
        char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER)
    )))
}

/// Encode typed text in dictionary form (§3.7).
///
/// The text is lowercased, encoded without abbreviations, padded
/// with 5s, and cut to the dictionary resolution; the final word
/// carries the terminator bit. `rows` names the alphabet rows in
/// force -- a custom table's when the story has one (§3.5.5) --
/// with `None` meaning the version's standard rows. The result is
/// four bytes through Version 3, six after.
pub fn encode_word(
    version: u8,
    word: &str,
    rows: Option<&AlphabetRows>,
    extras_table: &[u16],
) -> Result<Vec<u8>, VoxamError> {
    let resolution = dictionary_zchars(version);
    let mut zchars = encode_zchars(version, &word.to_lowercase(), rows, extras_table)?;
    zchars.truncate(resolution);
    zchars.resize(resolution, PAD);

    let mut encoded = Vec::with_capacity(resolution / 3 * 2);

    for index in (0..resolution).step_by(3) {
        let mut packed = (u16::from(zchars[index]) << Z_CHAR_SHIFTS[0])
            | (u16::from(zchars[index + 1]) << Z_CHAR_SHIFTS[1])
            | u16::from(zchars[index + 2]);

        if index + 3 == resolution {
            packed |= STRING_TERMINATOR_BIT;
        }

        encoded.extend_from_slice(&packed.to_be_bytes());
    }

    Ok(encoded)
}

/// Turn lowercased text into shift-laden Z-characters (§3.7).
///
/// From Version 3, each A1 or A2 character takes a single shift. In
/// Versions 1 and 2 the shifts are relative, and a lock is used
/// instead when the next two characters share an alphabet (§3.7.1).
/// Under the standard alphabets a lowercased character is never in
/// A1, but a custom table may put it nowhere else (§3.5.5).
fn encode_zchars(
    version: u8,
    text: &str,
    rows: Option<&AlphabetRows>,
    extras_table: &[u16],
) -> Result<Vec<u8>, VoxamError> {
    let standard;
    let rows = match rows {
        Some(rows) => rows,
        None => {
            standard = standard_alphabets(version);
            &standard
        }
    };

    let a2_search_start = if version == 1 { 1 } else { 2 };
    let mut targets: Vec<(usize, Vec<u8>)> = Vec::new();

    for character in text.chars() {
        let unit: Option<u16> = u16::try_from(character as u32).ok();
        let slot_of = |row: &AlphabetRow, start: usize| {
            unit.and_then(|unit| {
                row[start..]
                    .iter()
                    .position(|slot| slot.as_slice() == [unit])
                    .map(|position| start + position)
            })
        };

        if let Some(position) = slot_of(&rows[0], 0) {
            targets.push((0, vec![position as u8 + FIRST_ALPHABET_CHARACTER]));
        } else if let Some(position) = slot_of(&rows[1], 0) {
            targets.push((1, vec![position as u8 + FIRST_ALPHABET_CHARACTER]));
        } else if let Some(position) = slot_of(&rows[2], a2_search_start) {
            targets.push((A2, vec![position as u8 + FIRST_ALPHABET_CHARACTER]));
        } else {
            let code = char_to_zscii(character, extras_table)?;
            let escape = vec![ESCAPE, (code >> 5) as u8 & 0x1F, code as u8 & 0x1F];
            targets.push((A2, escape));
        }
    }

    if version > LAST_SHIFT_LOCK_VERSION {
        let mut out = Vec::new();

        for (alphabet, chars) in targets {
            if alphabet != 0 {
                // Z-characters 4 and 5 select A1 and A2 for one
                // character (§3.2.3).
                out.push(3 + alphabet as u8);
            }

            out.extend(chars);
        }

        return Ok(out);
    }

    Ok(shift_locked(&targets))
}

/// Emit Version 1 and 2 shifts, locking for runs (§3.2.2, §3.7.1).
fn shift_locked(targets: &[(usize, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut locked = 0usize;

    for (index, (alphabet, chars)) in targets.iter().enumerate() {
        if *alphabet != locked {
            let run = index + 1 < targets.len() && targets[index + 1].0 == *alphabet;
            let upward = (3 + alphabet - locked) % 3 == 1;

            if run {
                out.push(if upward { 4 } else { 5 });
                locked = *alphabet;
            } else {
                out.push(if upward { 2 } else { 3 });
            }
        }

        out.extend(chars);
    }

    out
}

/// Gather a string's Z-characters up to its terminator (§3.2),
/// returning them and the first address past the string.
fn zchars_at(memory: &Memory, address: usize) -> Result<(Vec<u8>, usize), VoxamError> {
    let mut zchars = Vec::new();
    let mut position = address;

    loop {
        let word = memory.fetch_word(position)?;
        position += WORD_SIZE;

        for shift in Z_CHAR_SHIFTS {
            zchars.push(((word >> shift) & Z_CHAR_MASK) as u8);
        }

        if word & STRING_TERMINATOR_BIT != 0 {
            return Ok((zchars, position));
        }
    }
}

/// Interpret Z-characters under a version's rules (§3.2, §3.5).
///
/// A string may legally end mid-construction, the remnant ignored
/// (§3.6.1) -- except inside an abbreviation, where that and any
/// further abbreviation are illegal (§3.3.1).
fn text_of(memory: &Memory, zchars: &[u8], in_abbreviation: bool) -> Result<Units, VoxamError> {
    let version = memory.header().version();
    let rows = alphabets(memory)?;
    let mut out: Units = Vec::new();
    let mut locked = 0usize;
    let mut current = 0usize;
    let mut position = 0usize;

    while position < zchars.len() {
        let char = zchars[position];
        position += 1;

        if char == SPACE {
            out.push(UNIT_SPACE);
            current = locked;
        } else if version == 1 && char == V1_NEWLINE {
            out.push(u16::from(b'\n'));
            current = locked;
        } else if is_abbreviation(version, char) {
            if in_abbreviation {
                return Err(text_error(
                    "an abbreviation may not use abbreviations (§3.3.1)".into(),
                ));
            }

            if position >= zchars.len() {
                require_complete(in_abbreviation)?;
                break;
            }

            out.extend(abbreviation(memory, char, zchars[position])?);
            position += 1;
            current = locked;
        } else if char < FIRST_ALPHABET_CHARACTER {
            (current, locked) = shift(version, current, locked, char);
        } else if current == A2 && char == ESCAPE {
            if position + 2 > zchars.len() {
                require_complete(in_abbreviation)?;
                break;
            }

            let code = (u16::from(zchars[position]) << 5) | u16::from(zchars[position + 1]);
            out.extend(zscii_to_units(code, &extras(memory)?, version)?);
            position += 2;
            current = locked;
        } else if current == A2 && version > 1 && char == A2_NEWLINE {
            out.push(u16::from(b'\n'));
            current = locked;
        } else {
            out.extend(&rows[current][usize::from(char - FIRST_ALPHABET_CHARACTER)]);
            current = locked;
        }
    }

    Ok(out)
}

/// Police §3.3.1: only abbreviations may not end mid-construction.
fn require_complete(in_abbreviation: bool) -> Result<(), VoxamError> {
    if in_abbreviation {
        return Err(text_error(
            "an abbreviation may not end with an incomplete multi-Z-character \
             construction (§3.3.1)"
                .into(),
        ));
    }

    Ok(())
}

/// Expand abbreviation entry 32(z - 1) + x (§3.3).
///
/// The table entry is a word address, doubled to reach the string's
/// bytes (§1.2.2) -- the one place word addresses are used at all.
fn abbreviation(memory: &Memory, bank_char: u8, index: u8) -> Result<Units, VoxamError> {
    let table = usize::from(memory.header().abbreviations_table_address());
    let entry_number = ABBREVIATION_BANK_SIZE * usize::from(bank_char - 1) + usize::from(index);
    let entry = memory.fetch_word(table + WORD_SIZE * entry_number)?;
    let (zchars, _) = zchars_at(memory, WORD_ADDRESS_SCALE * usize::from(entry))?;

    text_of(memory, &zchars, true)
}

/// Pick the version's standard alphabet rows (§3.5.3, §3.5.4).
fn standard_alphabets(version: u8) -> AlphabetRows {
    let a2 = if version == 1 {
        ALPHABET_A2_V1
    } else {
        ALPHABET_A2
    };

    [row_of(ALPHABET_A0), row_of(ALPHABET_A1), row_of(a2)]
}

fn row_of(alphabet: &str) -> AlphabetRow {
    alphabet.chars().map(|slot| vec![slot as u16]).collect()
}

/// Whether a Z-character introduces an abbreviation (§3.3).
fn is_abbreviation(version: u8, char: u8) -> bool {
    if version >= FIRST_ABBREVIATION_VERSION {
        return (1..=3).contains(&char);
    }

    version == LAST_SHIFT_LOCK_VERSION && char == V2_ABBREVIATION_CHAR
}

/// Apply a shift character, returning (current, locked) (§3.2.2,
/// §3.2.3).
///
/// In Versions 1 and 2, characters 2 and 3 rotate the alphabet for
/// one character and 4 and 5 rotate the lock; from Version 3, 4 and
/// 5 select A1 or A2 absolutely, for one character.
fn shift(version: u8, current: usize, locked: usize, char: u8) -> (usize, usize) {
    if version > LAST_SHIFT_LOCK_VERSION {
        return (usize::from(char) - 3, locked);
    }

    let rotated = (current + if char.is_multiple_of(2) { 1 } else { 2 }) % 3;

    if char == 4 || char == 5 {
        return (rotated, rotated);
    }

    (rotated, locked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zmachine::testing::{pack, planted_memory};

    // Every expected value below is a golden vector generated from
    // the reference Python implementation (voxam.zmachine.zscii);
    // the two implementations must decode and encode identically.

    fn decoded(version: u8, zchars: &[u8]) -> (String, usize) {
        let memory = planted_memory(version, &[(0x80, &pack(zchars))]);
        let (text, end) = decode_string(&memory, 0x80).unwrap();

        (text, end - 0x80)
    }

    #[test]
    fn decodes_the_reference_vectors() {
        let vectors: &[(u8, &[u8], &str, usize)] = &[
            (
                3,
                &[13, 10, 17, 17, 20, 0, 28, 20, 23, 17, 9],
                "hello world",
                8,
            ),
            (3, &[4, 13, 14], "Hi", 2),
            (3, &[4, 0, 22], " q", 2),
            (3, &[5, 8, 5, 9, 5, 18], "01.", 4),
            (3, &[6, 5, 7, 7], "a\nb", 4),
            (3, &[5, 6, 2, 0, 0, 0], "@  ", 4),
            (3, &[5, 6, 4, 27, 0, 0], "\u{e4}  ", 4),
            (3, &[6, 5, 6], "a", 2),
            (3, &[6, 6, 1], "aa", 2),
            (3, &[5, 0, 8], " c", 2),
            (3, &[6, 5, 6, 0, 0, 7, 0], "ab ", 6),
            (1, &[6, 1, 7], "a\nb", 2),
            (1, &[3, 21, 0], "_ ", 2),
            (1, &[2, 13, 14], "Hi", 2),
            (1, &[3, 8, 31], "1z", 2),
            (1, &[5, 8, 9, 10], "123", 4),
            (2, &[4, 6, 7, 5, 5, 8], "AB0", 4),
            (2, &[2, 13, 14], "Hi", 2),
            (2, &[3, 8, 31], "0z", 2),
            (6, &[6, 5, 6, 0, 9, 7, 0], "a   b ", 6),
            (6, &[6, 5, 6, 0, 11, 7, 0], "a  b ", 6),
            (5, &[5, 6, 0, 24, 0, 0], "\u{2191}  ", 4),
        ];

        for (version, zchars, expected, expected_end) in vectors {
            let (text, end) = decoded(*version, zchars);

            assert_eq!(&text, expected, "v{version} {zchars:?}");
            assert_eq!(end, *expected_end, "v{version} {zchars:?} end");
        }
    }

    /// The custom table of the reference vectors: A0 the reversed
    /// lowercase letters, A1 digits and periods with one comma, A2
    /// uppercase with its first two slots overridden in vain.
    fn custom_alphabet_plants() -> (Vec<u8>, Vec<u8>) {
        let mut table = Vec::new();
        table.extend("zyxwvutsrqponmlkjihgfedcba".bytes());
        table.extend("0123456789.........,......".bytes());
        table.extend([0, 0]);
        table.extend("ABCDEFGHIJKLMNOPQRSTUVWX".bytes());

        (0x0150u16.to_be_bytes().to_vec(), table)
    }

    fn custom_alphabet_memory(zchars: &[u8]) -> crate::zmachine::memory::Memory {
        let (address, table) = custom_alphabet_plants();

        planted_memory(
            5,
            &[(0x34, &address), (0x150, &table), (0x80, &pack(zchars))],
        )
    }

    #[test]
    fn a_custom_table_redefines_the_alphabets() {
        let memory = custom_alphabet_memory(&[6, 4, 6, 5, 8, 0]);

        assert_eq!(decode_string(&memory, 0x80).unwrap().0, "z0A ");
    }

    #[test]
    fn a2_escape_and_newline_defy_the_table() {
        let memory = custom_alphabet_memory(&[5, 7, 6]);

        assert_eq!(decode_string(&memory, 0x80).unwrap().0, "\nz");
    }

    #[test]
    fn a_null_alphabet_slot_prints_nothing() {
        let memory = custom_alphabet_memory(&[5, 8, 5, 9, 7]);

        assert_eq!(decode_string(&memory, 0x80).unwrap().0, "ABy");
    }

    /// A header extension at $170 whose word 3 names a Unicode
    /// translation table at $180 with the given entries.
    fn extras_memory(entries: &[u16], zchars: &[u8]) -> crate::zmachine::memory::Memory {
        let mut unicode_table = vec![entries.len() as u8];
        for entry in entries {
            unicode_table.extend_from_slice(&entry.to_be_bytes());
        }

        planted_memory(
            5,
            &[
                (0x36, &0x0170u16.to_be_bytes()),
                (0x170, &3u16.to_be_bytes()),
                (0x176, &0x0180u16.to_be_bytes()),
                (0x180, &unicode_table),
                (0x80, &pack(zchars)),
            ],
        )
    }

    #[test]
    fn a_custom_translation_table_redefines_the_extras() {
        let memory = extras_memory(&[0x0107, 0x0142], &[5, 6, 4, 27, 5, 6]);

        assert_eq!(decode_string(&memory, 0x80).unwrap().0, "\u{107}");
    }

    #[test]
    fn codes_past_a_custom_table_still_halt() {
        let memory = extras_memory(&[0x0107, 0x0142], &[5, 6, 4, 29, 0, 0]);
        let error = decode_string(&memory, 0x80).unwrap_err();

        assert_eq!(
            error.to_string(),
            "ZSCII code 157 is not yet printable (§3.8)"
        );
    }

    #[test]
    fn an_empty_translation_table_undefines_all_extras() {
        let memory = extras_memory(&[], &[5, 6, 4, 27, 0, 0]);
        let error = decode_string(&memory, 0x80).unwrap_err();

        assert_eq!(
            error.to_string(),
            "ZSCII code 155 is not yet printable (§3.8)"
        );
    }

    #[test]
    fn adjacent_surrogates_fuse_into_astral_characters() {
        let memory = extras_memory(&[0xD83D, 0xDE00], &[5, 6, 4, 27, 5, 6, 4, 28]);

        assert_eq!(decode_string(&memory, 0x80).unwrap().0, "\u{1F600}");
    }

    #[test]
    fn orphaned_surrogates_blot_honestly() {
        let memory = extras_memory(&[0xD83D, 0xDE00], &[5, 6, 4, 27, 0, 0]);

        assert_eq!(decode_string(&memory, 0x80).unwrap().0, "\u{FFFD}  ");
    }

    /// The abbreviation layout of the reference vectors: table at
    /// $60, entries 0, 32, and 95 naming strings from $130.
    fn abbreviation_memory(
        version: u8,
        zchars: &[u8],
        entry0: &[u8],
    ) -> crate::zmachine::memory::Memory {
        let table_address = 0x0060u16.to_be_bytes();
        let entries: [(usize, [u8; 2]); 3] = [
            (0x60, (0x130u16 / 2).to_be_bytes()),
            (0x60 + 64, (0x134u16 / 2).to_be_bytes()),
            (0x60 + 190, (0x138u16 / 2).to_be_bytes()),
        ];
        let go = pack(entry0);
        let hi = pack(&[13, 14]);
        let ok = pack(&[20, 16]);
        let using = pack(zchars);

        planted_memory(
            version,
            &[
                (0x18, &table_address),
                (entries[0].0, &entries[0].1),
                (entries[1].0, &entries[1].1),
                (entries[2].0, &entries[2].1),
                (0x130, &go),
                (0x134, &hi),
                (0x138, &ok),
                (0xC0, &using),
            ],
        )
    }

    #[test]
    fn abbreviations_expand_across_their_banks() {
        let go: &[u8] = &[12, 20];

        let memory = abbreviation_memory(3, &[1, 0, 29], go);
        assert_eq!(decode_string(&memory, 0xC0).unwrap().0, "gox");

        let memory = abbreviation_memory(3, &[2, 0, 0], go);
        assert_eq!(decode_string(&memory, 0xC0).unwrap().0, "hi ");

        let memory = abbreviation_memory(3, &[3, 31, 0], go);
        assert_eq!(decode_string(&memory, 0xC0).unwrap().0, "ok ");

        let memory = abbreviation_memory(2, &[1, 0, 0], go);
        assert_eq!(decode_string(&memory, 0xC0).unwrap().0, "go ");
    }

    #[test]
    fn abbreviations_may_not_nest() {
        let memory = abbreviation_memory(3, &[1, 0, 0], &[1, 1, 0]);
        let error = decode_string(&memory, 0xC0).unwrap_err();

        assert_eq!(
            error.to_string(),
            "an abbreviation may not use abbreviations (§3.3.1)"
        );
    }

    #[test]
    fn abbreviations_may_not_end_incomplete() {
        let memory = abbreviation_memory(3, &[1, 0, 0], &[5, 6, 1]);
        let error = decode_string(&memory, 0xC0).unwrap_err();

        assert_eq!(
            error.to_string(),
            "an abbreviation may not end with an incomplete multi-Z-character \
             construction (§3.3.1)"
        );
    }

    #[test]
    fn encodes_the_reference_vectors() {
        let vectors: &[(u8, &str, &str)] = &[
            (1, "hello", "3551c685"),
            (1, "xyzzy", "77dfffc5"),
            (1, "Frobozz", "2ef49e9f"),
            (1, "x", "74a594a5"),
            (1, "it's", "3b23df05"),
            (1, "a1b2", "18689c69"),
            (1, "toRVALD", "6697ecd1"),
            (1, "ab<cd", "18e3ed09"),
            (2, "hello", "3551c685"),
            (2, "xyzzy", "77dfffc5"),
            (2, "Frobozz", "2ef49e9f"),
            (2, "x", "74a594a5"),
            (2, "it's", "3b23e305"),
            (2, "a1b2", "18699c6a"),
            (2, "toRVALD", "6697ecd1"),
            (2, "ab<cd", "18e3983c"),
            (3, "hello", "3551c685"),
            (3, "xyzzy", "77dfffc5"),
            (3, "Frobozz", "2ef49e9f"),
            (3, "x", "74a594a5"),
            (3, "it's", "3b25e305"),
            (3, "a1b2", "18a99caa"),
            (3, "toRVALD", "6697ecd1"),
            (3, "ab<cd", "18e5983c"),
            (4, "hello", "3551468594a5"),
            (4, "xyzzy", "77df7fc594a5"),
            (4, "Frobozz", "2ef41e9ffca5"),
            (4, "x", "74a514a594a5"),
            (4, "it's", "3b25630594a5"),
            (4, "a1b2", "18a91caa94a5"),
            (4, "toRVALD", "66976cd1a4a5"),
            (4, "ab<cd", "18e5183ca125"),
            (5, "hello", "3551468594a5"),
            (5, "xyzzy", "77df7fc594a5"),
            (5, "Frobozz", "2ef41e9ffca5"),
            (5, "x", "74a514a594a5"),
            (5, "it's", "3b25630594a5"),
            (5, "a1b2", "18a91caa94a5"),
            (5, "toRVALD", "66976cd1a4a5"),
            (5, "ab<cd", "18e5183ca125"),
        ];

        for (version, word, expected) in vectors {
            let encoded = encode_word(*version, word, None, &DEFAULT_EXTRAS).unwrap();
            let hex: String = encoded.iter().map(|byte| format!("{byte:02x}")).collect();

            assert_eq!(&hex, expected, "v{version} {word:?}");
        }
    }

    #[test]
    fn encoding_follows_the_custom_rows() {
        let memory = custom_alphabet_memory(&[5]);
        let rows = alphabets(&memory).unwrap();

        let vectors: &[(&str, &str)] = &[("zy", "18e514a594a5"), ("z0A", "18867ca594a5")];

        for (word, expected) in vectors {
            let encoded = encode_word(5, word, Some(&rows), &DEFAULT_EXTRAS).unwrap();
            let hex: String = encoded.iter().map(|byte| format!("{byte:02x}")).collect();

            assert_eq!(&hex, expected, "{word:?}");
        }
    }

    #[test]
    fn typed_characters_land_as_their_zscii_codes() {
        assert_eq!(char_to_zscii('\n', &DEFAULT_EXTRAS).unwrap(), 13);
        assert_eq!(char_to_zscii('\u{8}', &DEFAULT_EXTRAS).unwrap(), 8);
        assert_eq!(char_to_zscii('\u{7f}', &DEFAULT_EXTRAS).unwrap(), 8);
        assert_eq!(char_to_zscii('\u{1b}', &DEFAULT_EXTRAS).unwrap(), 27);
        assert_eq!(char_to_zscii('a', &DEFAULT_EXTRAS).unwrap(), 97);
        assert_eq!(char_to_zscii('\u{82}', &DEFAULT_EXTRAS).unwrap(), 130);
        assert_eq!(char_to_zscii('\u{e4}', &DEFAULT_EXTRAS).unwrap(), 155);
    }

    #[test]
    fn characters_outside_zscii_are_refused() {
        let error = char_to_zscii('\u{2603}', &DEFAULT_EXTRAS).unwrap_err();

        assert_eq!(
            error.to_string(),
            "the character '\u{2603}' has no ZSCII code (§3.8)"
        );
    }

    #[test]
    fn the_default_extras_match_the_reference_table() {
        assert_eq!(DEFAULT_EXTRAS.len(), 69);
        assert_eq!(DEFAULT_EXTRAS[0], 0x0E4);
        assert_eq!(DEFAULT_EXTRAS[64], 0x0A3);
        assert_eq!(DEFAULT_EXTRAS[68], 0x0BF);
    }
}
