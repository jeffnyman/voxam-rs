//! The dictionary table and lexical analysis (§13).
//!
//! The dictionary lives in static memory at the address the header
//! word at $08 gives (§13.1): a list of word-separator codes, an
//! entry length, an entry count, and sorted entries whose first
//! bytes are the encoded text of each word (§13.2). Tokenization
//! splits typed text at spaces and separators; lookup encodes each
//! word and hunts for its entry.

use std::collections::HashSet;

use crate::errors::VoxamError;
use crate::zmachine::memory::Memory;
use crate::zmachine::zscii::{
    AlphabetRows, alphabets, encode_word, extras, units_to_string, zscii_to_units,
};

/// Entry text is 4 bytes through Version 3 and 6 bytes after
/// (§13.3, §13.4).
const V3_LAST_VERSION: u8 = 3;
const V3_TEXT_BYTES: usize = 4;
const V4_TEXT_BYTES: usize = 6;

/// An unrecognised word's dictionary address is 0 (§13.6.3).
pub const NOT_IN_DICTIONARY: usize = 0;

/// A user dictionary may give its entry count as -n, meaning n
/// entries unsorted (§13.5, §15 tokenise) -- convenient for tables
/// altered in play. A linear hunt does not care about order, so the
/// sign only affects how the count is read.
const COUNT_SIGN_BIT: u16 = 0x8000;
const COUNT_RANGE: u32 = 0x10000;

/// A view of one dictionary table (§13.2).
///
/// Usually the standard dictionary the header names, but tokenise
/// may supply any table in the same format (§13.6).
pub struct Dictionary<'a> {
    memory: &'a Memory,
    version: u8,
    rows: AlphabetRows,
    extras: Vec<u16>,
    separators: HashSet<String>,
    entry_length: usize,
    count: usize,
    entries: usize,
    text_bytes: usize,
}

impl<'a> Dictionary<'a> {
    /// Read the table's header once (§13.2). `base` names the
    /// table's byte address; `None` means the standard dictionary
    /// named in the story header (§13.1).
    pub fn new(memory: &'a Memory, base: Option<usize>) -> Result<Self, VoxamError> {
        let version = memory.header().version();
        let rows = alphabets(memory)?;
        let repertoire = extras(memory)?;
        let base = base.unwrap_or_else(|| usize::from(memory.header().dictionary_address()));

        let separator_count = usize::from(memory.read_byte(base)?);
        let mut separators = HashSet::new();

        for index in 0..separator_count {
            let code = memory.read_byte(base + 1 + index)?;
            let units = zscii_to_units(u16::from(code), &repertoire, version)?;
            separators.insert(units_to_string(&units));
        }

        let entry_length = usize::from(memory.read_byte(base + 1 + separator_count)?);

        let raw_count = memory.read_word(base + 2 + separator_count)?;
        let count = if raw_count & COUNT_SIGN_BIT != 0 {
            (COUNT_RANGE - u32::from(raw_count)) as usize
        } else {
            usize::from(raw_count)
        };

        Ok(Self {
            memory,
            version,
            rows,
            extras: repertoire,
            separators,
            entry_length,
            count,
            entries: base + 4 + separator_count,
            text_bytes: if version <= V3_LAST_VERSION {
                V3_TEXT_BYTES
            } else {
                V4_TEXT_BYTES
            },
        })
    }

    /// The word-separator characters (§13.2).
    pub fn separators(&self) -> &HashSet<String> {
        &self.separators
    }

    /// How many words the dictionary holds (§13.2).
    pub fn entry_count(&self) -> usize {
        self.count
    }

    /// Find a typed word's entry address, or 0 (§13.6.2).
    ///
    /// The word is encoded to dictionary form first, so lookup
    /// inherits the resolution guillotine: only the leading six or
    /// nine Z-characters distinguish words.
    pub fn lookup(&self, word: &str) -> Result<usize, VoxamError> {
        let target = encode_word(self.version, word, Some(&self.rows), &self.extras)?;

        for index in 0..self.count {
            let address = self.entries + index * self.entry_length;

            if self.text_at(address)? == target {
                return Ok(address);
            }
        }

        Ok(NOT_IN_DICTIONARY)
    }

    /// Read an entry's encoded text (§13.3).
    fn text_at(&self, address: usize) -> Result<Vec<u8>, VoxamError> {
        (0..self.text_bytes)
            .map(|offset| self.memory.read_byte(address + offset))
            .collect()
    }
}

/// Split typed text into words (§13.6.1), returning each word with
/// the character offset it starts at within the text.
///
/// Spaces divide words and are otherwise ignored; separators divide
/// words while being words themselves.
pub fn tokenize(text: &str, separators: &HashSet<String>) -> Vec<(String, usize)> {
    let mut words: Vec<(String, usize)> = Vec::new();
    let mut held: Option<(String, usize)> = None;

    for (position, character) in text.chars().enumerate() {
        let alone = character.to_string();

        if character == ' ' || separators.contains(&alone) {
            if let Some(word) = held.take() {
                words.push(word);
            }

            if character != ' ' {
                words.push((alone, position));
            }
        } else if let Some((word, _)) = &mut held {
            word.push(character);
        } else {
            held = Some((alone, position));
        }
    }

    if let Some(word) = held {
        words.push(word);
    }

    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zmachine::memory::Memory;
    use crate::zmachine::testing::planted_memory;

    const DICTIONARY_BASE: usize = 0x150;

    /// Two hand-encoded Version 3 entries in sorted order: "go"
    /// packs to 3285 94A5 and "hi" to 35C5 94A5 (§13.3, §13.5),
    /// exactly the reference suite's fixture.
    const GO: [u8; 4] = [0x32, 0x85, 0x94, 0xA5];
    const HI: [u8; 4] = [0x35, 0xC5, 0x94, 0xA5];

    fn dictionary_memory(
        version: u8,
        entries: &[&[u8]],
        separators: &[u8],
        entry_length: u8,
        base: usize,
    ) -> Memory {
        let mut table = vec![separators.len() as u8];
        table.extend_from_slice(separators);
        table.push(entry_length);
        table.extend_from_slice(&(entries.len() as u16).to_be_bytes());

        for entry in entries {
            let mut padded = entry.to_vec();
            padded.resize(usize::from(entry_length), 0);
            table.extend_from_slice(&padded);
        }

        planted_memory(
            version,
            &[(0x08, &(base as u16).to_be_bytes()), (base, &table)],
        )
    }

    fn standard_memory() -> Memory {
        dictionary_memory(3, &[&GO, &HI], b",.", 7, DICTIONARY_BASE)
    }

    #[test]
    fn reads_the_dictionary_header() {
        let memory = standard_memory();
        let dictionary = Dictionary::new(&memory, None).unwrap();

        assert_eq!(dictionary.entry_count(), 2);

        let mut separators: Vec<&str> =
            dictionary.separators().iter().map(String::as_str).collect();
        separators.sort_unstable();
        assert_eq!(separators, [",", "."]);
    }

    #[test]
    fn lookup_finds_a_word() {
        let memory = standard_memory();
        let dictionary = Dictionary::new(&memory, None).unwrap();

        // Entries begin past the 4 + 2 header bytes; "hi" is the
        // second 7-byte entry.
        assert_eq!(dictionary.lookup("go").unwrap(), DICTIONARY_BASE + 6);
        assert_eq!(dictionary.lookup("hi").unwrap(), DICTIONARY_BASE + 6 + 7);
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let memory = standard_memory();
        let dictionary = Dictionary::new(&memory, None).unwrap();

        assert_eq!(dictionary.lookup("GO").unwrap(), DICTIONARY_BASE + 6);
    }

    #[test]
    fn lookup_misses_give_address_0() {
        let memory = standard_memory();
        let dictionary = Dictionary::new(&memory, None).unwrap();

        assert_eq!(dictionary.lookup("xyzzy").unwrap(), NOT_IN_DICTIONARY);
    }

    #[test]
    fn lookup_inherits_the_guillotine() {
        // "gooseberry" and "gooseberries" agree through six
        // Z-characters, so a v3 dictionary cannot tell them apart.
        let entry = crate::zmachine::zscii::encode_word(3, "gooseberry", None, &[]).unwrap();
        let memory = dictionary_memory(3, &[&entry], b"", 7, DICTIONARY_BASE);
        let dictionary = Dictionary::new(&memory, None).unwrap();

        assert_eq!(
            dictionary.lookup("gooseberries").unwrap(),
            dictionary.lookup("gooseberry").unwrap()
        );
    }

    #[test]
    fn a_dictionary_can_live_at_any_base() {
        let memory = dictionary_memory(3, &[&GO], b"", 7, 0x180);
        let dictionary = Dictionary::new(&memory, Some(0x180)).unwrap();

        assert_eq!(dictionary.lookup("go").unwrap(), 0x180 + 4);
    }

    #[test]
    fn a_negative_count_means_unsorted_entries() {
        // A count word of -2 still describes two entries (§13.5).
        let mut table = vec![0u8, 7];
        table.extend_from_slice(&0xFFFEu16.to_be_bytes());
        table.extend_from_slice(&[&HI[..], &[0, 0, 0]].concat());
        table.extend_from_slice(&[&GO[..], &[0, 0, 0]].concat());

        let memory = planted_memory(
            3,
            &[
                (0x08, &(DICTIONARY_BASE as u16).to_be_bytes()),
                (DICTIONARY_BASE, &table),
            ],
        );
        let dictionary = Dictionary::new(&memory, None).unwrap();

        assert_eq!(dictionary.entry_count(), 2);
        assert_eq!(dictionary.lookup("go").unwrap(), DICTIONARY_BASE + 4 + 7);
    }

    #[test]
    fn version_4_entries_have_longer_text() {
        // "carousels" and "carousel" agree through six Z-characters
        // but part at the ninth: nine separate what six could not.
        let entry = crate::zmachine::zscii::encode_word(4, "carousels", None, &[]).unwrap();
        let memory = dictionary_memory(4, &[&entry], b"", 9, DICTIONARY_BASE);
        let dictionary = Dictionary::new(&memory, None).unwrap();

        assert_eq!(dictionary.lookup("carousels").unwrap(), DICTIONARY_BASE + 4);
        assert_eq!(dictionary.lookup("carousel").unwrap(), NOT_IN_DICTIONARY);

        let v3_entry = crate::zmachine::zscii::encode_word(3, "carousels", None, &[]).unwrap();
        let v3_memory = dictionary_memory(3, &[&v3_entry], b"", 7, DICTIONARY_BASE);
        let v3_dictionary = Dictionary::new(&v3_memory, None).unwrap();

        assert_eq!(
            v3_dictionary.lookup("carousel").unwrap(),
            DICTIONARY_BASE + 4
        );
    }

    fn separator_set(separators: &[&str]) -> HashSet<String> {
        separators.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn tokenize_matches_the_specs_example() {
        let words = tokenize("fred,go fishing", &separator_set(&[","]));

        assert_eq!(
            words,
            [
                ("fred".to_string(), 0),
                (",".to_string(), 4),
                ("go".to_string(), 5),
                ("fishing".to_string(), 8),
            ]
        );
    }

    #[test]
    fn tokenize_ignores_stray_spaces() {
        let words = tokenize("  open  mailbox ", &separator_set(&[]));

        assert_eq!(words, [("open".to_string(), 2), ("mailbox".to_string(), 8)]);
    }

    #[test]
    fn tokenize_of_nothing_is_no_words() {
        assert!(tokenize("", &separator_set(&[","])).is_empty());
        assert!(tokenize("   ", &separator_set(&[","])).is_empty());
    }
}
