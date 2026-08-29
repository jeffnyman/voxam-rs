//! Loading and validation of Glulx story files (Glulx: The
//! Header).
//!
//! The header is the first 36 bytes: nine big-endian 32-bit words,
//! opening with the magic 'Glul'. It lives in ROM, so everything
//! here is fixed for the story's whole life -- which is why
//! loading is the right moment to hold the file to all of the
//! header's promises: the version window, the 256-byte alignment
//! of every memory boundary, and the checksum over the entire
//! initial image.

use crate::errors::VoxamError;

/// The magic number: ASCII 'Glul' (Glulx: The Header).
const MAGIC: &[u8; 4] = b"Glul";

/// Nine 32-bit words (Glulx: The Header).
const HEADER_SIZE: usize = 36;
const VERSION_AT: usize = 4;
const RAMSTART_AT: usize = 8;
const EXTSTART_AT: usize = 12;
const ENDMEM_AT: usize = 16;
const STACK_SIZE_AT: usize = 20;
const START_FUNCTION_AT: usize = 24;
const DECODING_TABLE_AT: usize = 28;
const CHECKSUM_AT: usize = 32;

/// An interpreter written to specification 3.1.3 accepts game
/// files from 2.0.0 through 3.1.*: minor versions are backwards
/// compatible, subminor versions do not matter, and 2.0 differs
/// from 3.0 only in lacking Unicode (Glulx: The Header).
const VERSION_FLOOR: u32 = 0x0002_0000;
const VERSION_CEILING: u32 = 0x0003_01FF;

/// RAMSTART, EXTSTART, and ENDMEM must sit on 256-byte boundaries,
/// and ROM must be at least 256 bytes so the header fits in it;
/// the stack size is a multiple of 256 as well (Glulx: The Header,
/// Glulx: The Stack).
const BOUNDARY: u32 = 256;

const WORD_SIZE: usize = 4;

fn story_error(message: String) -> VoxamError {
    VoxamError::GlulxStory(message)
}

/// A Glulx story file held in memory, its header promises kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Story {
    data: Vec<u8>,
}

impl Story {
    /// Accept a game file, holding it to every promise its header
    /// makes: the magic, the version window, boundary alignment
    /// and order, and the declared length.
    pub fn new(data: Vec<u8>) -> Result<Self, VoxamError> {
        if data.len() < HEADER_SIZE {
            return Err(story_error(format!(
                "a Glulx story opens with a {HEADER_SIZE}-byte header, but only {} \
                 bytes are present (Glulx: The Header)",
                data.len()
            )));
        }

        if &data[..4] != MAGIC {
            return Err(story_error(
                "the file does not open with the magic number 'Glul' (Glulx: The \
                 Header)"
                    .into(),
            ));
        }

        let story = Self { data };

        story.require_version()?;
        story.require_map()?;

        Ok(story)
    }

    fn require_version(&self) -> Result<(), VoxamError> {
        let version = self.word(VERSION_AT);

        if !(VERSION_FLOOR..=VERSION_CEILING).contains(&version) {
            return Err(story_error(format!(
                "the story declares Glulx version {}, but an interpreter written to \
                 3.1.3 accepts 2.0.0 through 3.1.* (Glulx: The Header)",
                dotted(version)
            )));
        }

        Ok(())
    }

    fn require_map(&self) -> Result<(), VoxamError> {
        for (name, value) in [
            ("RAMSTART", self.ramstart()),
            ("EXTSTART", self.extstart()),
            ("ENDMEM", self.endmem()),
            ("the stack size", self.stack_size()),
        ] {
            if !value.is_multiple_of(BOUNDARY) {
                return Err(story_error(format!(
                    "{name} is {value}, which is not a multiple of {BOUNDARY} \
                     (Glulx: The Header)"
                )));
            }
        }

        if !(BOUNDARY <= self.ramstart()
            && self.ramstart() <= self.extstart()
            && self.extstart() <= self.endmem())
        {
            return Err(story_error(format!(
                "the memory map is out of order: ROM holds the header so RAMSTART is \
                 at least {BOUNDARY}, and RAMSTART ({}) precedes EXTSTART ({}) \
                 precedes ENDMEM ({}) (Glulx: The Header)",
                self.ramstart(),
                self.extstart(),
                self.endmem()
            )));
        }

        if self.data.len() != self.extstart() as usize {
            return Err(story_error(format!(
                "the file is {} bytes, but its header declares EXTSTART {} -- the \
                 length of the stored initial memory (Glulx: The Header)",
                self.data.len(),
                self.extstart()
            )));
        }

        Ok(())
    }

    /// The raw bytes of the game file -- the initial memory image
    /// from 0 to EXTSTART (Glulx: The Header).
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The declared Glulx version, dotted: 3.1.2 and kin.
    pub fn version(&self) -> String {
        dotted(self.word(VERSION_AT))
    }

    /// The first address the program can write to.
    pub fn ramstart(&self) -> u32 {
        self.word(RAMSTART_AT)
    }

    /// The end of stored initial memory: the game file's length.
    pub fn extstart(&self) -> u32 {
        self.word(EXTSTART_AT)
    }

    /// The end of the memory map; above EXTSTART starts zeroed.
    pub fn endmem(&self) -> u32 {
        self.word(ENDMEM_AT)
    }

    /// The stack the program needs, in bytes.
    pub fn stack_size(&self) -> u32 {
        self.word(STACK_SIZE_AT)
    }

    /// The function execution will commence by calling.
    pub fn start_function(&self) -> u32 {
        self.word(START_FUNCTION_AT)
    }

    /// The string-decoding table's address; 0 means none.
    pub fn decoding_table(&self) -> u32 {
        self.word(DECODING_TABLE_AT)
    }

    /// The checksum word the compiler stored.
    pub fn stored_checksum(&self) -> u32 {
        self.word(CHECKSUM_AT)
    }

    /// The checksum as an interpreter computes it: a simple sum of
    /// the entire initial contents of memory as big-endian 32-bit
    /// words, with the checksum field itself counted as zero
    /// (Glulx: The Header).
    pub fn computed_checksum(&self) -> u32 {
        let mut total: u32 = 0;

        for at in (0..self.data.len()).step_by(WORD_SIZE) {
            if at != CHECKSUM_AT {
                total = total.wrapping_add(self.word(at));
            }
        }

        total
    }

    /// Whether the stored and computed checksums agree.
    pub fn verify(&self) -> bool {
        self.stored_checksum() == self.computed_checksum()
    }

    /// The big-endian 32-bit word at a byte address, short reads
    /// padding with nothing as the reference's slicing does.
    fn word(&self, at: usize) -> u32 {
        let end = (at + WORD_SIZE).min(self.data.len());

        self.data[at..end]
            .iter()
            .fold(0u32, |value, byte| value << 8 | u32::from(*byte))
    }
}

/// A packed version word as its major.minor.subminor reading.
fn dotted(version: u32) -> String {
    format!(
        "{}.{}.{}",
        version >> 16,
        (version >> 8) & 0xFF,
        version & 0xFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest honest story: a 256-byte ROM-only image with a
    /// correct checksum.
    pub(crate) fn story_bytes() -> Vec<u8> {
        let mut data = vec![0u8; 256];
        data[..4].copy_from_slice(b"Glul");
        data[4..8].copy_from_slice(&0x0003_0102u32.to_be_bytes());
        data[8..12].copy_from_slice(&256u32.to_be_bytes()); // RAMSTART
        data[12..16].copy_from_slice(&256u32.to_be_bytes()); // EXTSTART
        data[16..20].copy_from_slice(&512u32.to_be_bytes()); // ENDMEM
        data[20..24].copy_from_slice(&256u32.to_be_bytes()); // stack
        data[24..28].copy_from_slice(&0x60u32.to_be_bytes()); // start fn

        let provisional = Story { data: data.clone() };
        let checksum = provisional.computed_checksum();
        data[32..36].copy_from_slice(&checksum.to_be_bytes());

        data
    }

    #[test]
    fn loads_and_verifies_an_honest_story() {
        let story = Story::new(story_bytes()).unwrap();

        assert_eq!(story.version(), "3.1.2");
        assert_eq!(story.ramstart(), 256);
        assert_eq!(story.endmem(), 512);
        assert_eq!(story.start_function(), 0x60);
        assert!(story.verify());
    }

    #[test]
    fn refuses_the_wrong_magic() {
        let mut data = story_bytes();
        data[0] = b'X';

        let error = Story::new(data).unwrap_err();
        assert!(error.to_string().contains("Glul"));
    }

    #[test]
    fn refuses_versions_outside_the_window() {
        let mut data = story_bytes();
        data[4..8].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        assert!(Story::new(data).is_err());

        let mut data = story_bytes();
        data[4..8].copy_from_slice(&0x0003_0200u32.to_be_bytes());
        assert!(Story::new(data).is_err());
    }

    #[test]
    fn refuses_misaligned_boundaries() {
        let mut data = story_bytes();
        data[8..12].copy_from_slice(&300u32.to_be_bytes());

        let error = Story::new(data).unwrap_err();
        assert!(error.to_string().contains("multiple of 256"));
    }

    #[test]
    fn refuses_a_length_that_is_not_extstart() {
        let mut data = story_bytes();
        data.push(0);

        let error = Story::new(data).unwrap_err();
        assert!(error.to_string().contains("EXTSTART"));
    }

    #[test]
    fn a_corrupted_story_fails_verification() {
        let mut data = story_bytes();
        data[100] = 0xFF;

        let story = Story::new(data).unwrap();
        assert!(!story.verify());
    }
}
