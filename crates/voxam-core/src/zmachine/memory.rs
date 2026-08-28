//! The memory map of the Z-Machine (§1.1).

use crate::errors::VoxamError;
use crate::zmachine::header::{HEADER_SIZE, Header};
use crate::zmachine::story::Story;

const KIB: usize = 1024;

/// The maximum story length in bytes depends on the version
/// (§1.1.4); index by version, 1 through 8.
const MAX_STORY_LENGTH: [usize; 9] = [
    0,
    128 * KIB,
    128 * KIB,
    128 * KIB,
    256 * KIB,
    256 * KIB,
    512 * KIB,
    512 * KIB,
    512 * KIB,
];

/// Static memory never extends beyond this byte address, however
/// large the story file is (§1.1.2).
const STATIC_MEMORY_CAP: usize = 0xFFFF;

/// The mutable working image of a story, with §1.1 access rules.
///
/// A Story is the pristine file; Memory is the image a running
/// game reads and writes. Keeping them separate is what lets
/// restart and restore reload dynamic memory from an untouched
/// original.
///
/// Writes are permitted only below the static memory base (§1.1.1,
/// §1.1.2). Reads are permitted through dynamic and static memory,
/// whose top is the lower of the last file byte or $ffff (§1.1.2);
/// high memory beyond that is reachable only through the
/// interpreter's own fetches. The finer-grained header write rules
/// (§11.1) are not yet enforced.
#[derive(Debug)]
pub struct Memory {
    data: Vec<u8>,
    static_base: usize,
    read_limit: usize,
}

impl Memory {
    /// Build a working memory image from a validated story.
    ///
    /// Fails if the header's region boundaries do not describe a
    /// coherent memory map (§1.1), or the file exceeds its
    /// version's maximum length (§1.1.4).
    pub fn new(story: &Story) -> Result<Self, VoxamError> {
        let data = story.data().to_vec();
        let static_base = usize::from(story.header().static_memory_base());
        let high_base = usize::from(story.header().high_memory_base());

        let maximum = MAX_STORY_LENGTH[usize::from(story.version())];

        if data.len() > maximum {
            return Err(VoxamError::ZMachineMemory(format!(
                "story file is {} bytes, but version {} allows at most {} (§1.1.4)",
                data.len(),
                story.version(),
                maximum
            )));
        }

        if static_base < HEADER_SIZE {
            return Err(VoxamError::ZMachineMemory(format!(
                "static memory begins at ${static_base:04x}, which would leave dynamic \
                 memory smaller than the {HEADER_SIZE}-byte header (§1.1.1)"
            )));
        }

        if static_base > data.len() {
            return Err(VoxamError::ZMachineMemory(format!(
                "static memory begins at ${static_base:04x}, beyond the end of the \
                 {}-byte file (§1.1)",
                data.len()
            )));
        }

        if high_base < static_base {
            return Err(VoxamError::ZMachineMemory(format!(
                "high memory begins at ${high_base:04x}, inside dynamic memory, which \
                 runs up to ${static_base:04x} (§1.1.3)"
            )));
        }

        // The top of static memory is the lower of the last file
        // byte or $ffff (§1.1.2); addresses past it cannot be
        // directly accessed.
        let read_limit = data.len().min(STATIC_MEMORY_CAP + 1);

        Ok(Self {
            data,
            static_base,
            read_limit,
        })
    }

    /// A live, typed view of the header within this image (§11.1).
    ///
    /// The header sits in dynamic memory, and a running game may
    /// legally alter parts of it (§11.1.2.1), so this view reflects
    /// writes made through this Memory rather than the pristine
    /// story.
    pub fn header(&self) -> Header<'_> {
        Header::over(&self.data)
    }

    /// Capture dynamic memory whole, header included (§6.1).
    ///
    /// Every byte below the static memory base, frozen. Static and
    /// high memory never change, so this is all of memory a save
    /// needs (§6.1.1).
    pub fn dynamic_snapshot(&self) -> Vec<u8> {
        self.data[..self.static_base].to_vec()
    }

    /// Write a captured dynamic memory image back whole (§6.1.2).
    ///
    /// The finer restore duties -- preserving 'Flags 2',
    /// re-stamping the interpreter's header fields -- belong to the
    /// machine (§6.1.2, §6.1.2.2); this write is deliberately
    /// verbatim. Fails if the image's size does not match this
    /// story's dynamic memory, which means it was captured from
    /// some other game (§6.1.2.1).
    pub fn restore_dynamic(&mut self, image: &[u8]) -> Result<(), VoxamError> {
        if image.len() != self.static_base {
            return Err(VoxamError::ZMachineMemory(format!(
                "cannot restore a {}-byte dynamic memory image over the {} bytes this \
                 story defines: it was captured from a different game (§6.1.2.1)",
                image.len(),
                self.static_base
            )));
        }

        self.data[..self.static_base].copy_from_slice(image);

        Ok(())
    }

    /// Read the byte at an address in dynamic or static memory
    /// (§1.1), refusing addresses outside the game-readable
    /// regions (§1.1.2).
    pub fn read_byte(&self, address: usize) -> Result<u8, VoxamError> {
        self.require_readable(address)?;

        Ok(self.data[address])
    }

    /// Read a byte anywhere in the story, as the interpreter
    /// (§1.1.3).
    ///
    /// Game reads stop at the top of static memory, but the
    /// interpreter's own accesses -- instruction fetch, routine
    /// headers, encoded strings -- must reach all of high memory,
    /// beyond even $ffff in large stories.
    pub fn fetch_byte(&self, address: usize) -> Result<u8, VoxamError> {
        self.require_fetchable(address)?;

        Ok(self.data[address])
    }

    /// Read a big-endian word anywhere in the story (§1.1.3, §2.1).
    pub fn fetch_word(&self, address: usize) -> Result<u16, VoxamError> {
        self.require_fetchable(address)?;
        self.require_fetchable(address + 1)?;

        Ok(u16::from(self.data[address]) << 8 | u16::from(self.data[address + 1]))
    }

    /// Read the big-endian word at an address (§2.1), refusing
    /// addresses outside the game-readable regions (§1.1.2).
    pub fn read_word(&self, address: usize) -> Result<u16, VoxamError> {
        self.require_readable(address)?;
        self.require_readable(address + 1)?;

        Ok(u16::from(self.data[address]) << 8 | u16::from(self.data[address + 1]))
    }

    /// Write a byte into dynamic memory (§1.1), refusing addresses
    /// outside it (§1.1.2).
    pub fn write_byte(&mut self, address: usize, value: u8) -> Result<(), VoxamError> {
        self.require_writable(address)?;
        self.data[address] = value;

        Ok(())
    }

    /// Write a big-endian word into dynamic memory (§2.1),
    /// refusing addresses outside it (§1.1.2).
    pub fn write_word(&mut self, address: usize, value: u16) -> Result<(), VoxamError> {
        self.require_writable(address)?;
        self.require_writable(address + 1)?;

        self.data[address] = (value >> 8) as u8;
        self.data[address + 1] = (value & 0xFF) as u8;

        Ok(())
    }

    /// Reject addresses outside the story file itself (§1.1).
    fn require_fetchable(&self, address: usize) -> Result<(), VoxamError> {
        if address >= self.data.len() {
            return Err(VoxamError::ZMachineMemory(format!(
                "cannot fetch ${address:04x}: the story file ends at ${:04x} (§1.1)",
                self.data.len() - 1
            )));
        }

        Ok(())
    }

    /// Reject addresses outside dynamic and static memory (§1.1.2).
    fn require_readable(&self, address: usize) -> Result<(), VoxamError> {
        if address >= self.read_limit {
            return Err(VoxamError::ZMachineMemory(format!(
                "cannot read ${address:04x}: game-readable memory runs from $0000 up \
                 to ${:04x} (§1.1.2)",
                self.read_limit - 1
            )));
        }

        Ok(())
    }

    /// Reject addresses outside dynamic memory (§1.1.1, §1.1.2).
    fn require_writable(&self, address: usize) -> Result<(), VoxamError> {
        if address >= self.static_base {
            return Err(VoxamError::ZMachineMemory(format!(
                "cannot write ${address:04x}: only dynamic memory, below ${:04x}, is \
                 writable (§1.1.2)",
                self.static_base
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zmachine::testing::story_bytes;

    fn memory(version: u8, length: usize, static_base: u16, high_base: u16) -> Memory {
        let story = Story::new(story_bytes(version, length, static_base, high_base)).unwrap();

        Memory::new(&story).unwrap()
    }

    #[test]
    fn reads_static_memory_and_stops_at_the_file_end() {
        let memory = memory(3, 128, 64, 64);

        assert!(memory.read_byte(127).is_ok());

        let error = memory.read_byte(128).unwrap_err();
        assert!(error.to_string().contains("§1.1.2"));
    }

    #[test]
    fn rejects_word_read_straddling_the_file_end() {
        let memory = memory(3, 128, 64, 64);

        assert!(memory.read_word(126).is_ok());
        assert!(memory.read_word(127).is_err());
    }

    #[test]
    fn round_trips_a_byte_in_dynamic_memory() {
        let mut memory = memory(3, 128, 64, 64);

        memory.write_byte(63, 0xAB).unwrap();

        assert_eq!(memory.read_byte(63).unwrap(), 0xAB);
    }

    #[test]
    fn words_are_stored_big_endian() {
        let mut memory = memory(3, 128, 64, 64);

        memory.write_word(0x20, 0x1234).unwrap();

        assert_eq!(memory.read_byte(0x20).unwrap(), 0x12);
        assert_eq!(memory.read_byte(0x21).unwrap(), 0x34);
        assert_eq!(memory.read_word(0x20).unwrap(), 0x1234);
    }

    #[test]
    fn rejects_write_at_the_static_boundary() {
        let mut memory = memory(3, 128, 64, 64);

        let error = memory.write_byte(64, 1).unwrap_err();
        assert!(error.to_string().contains("§1.1.2"));
    }

    #[test]
    fn rejects_word_write_straddling_the_boundary() {
        let mut memory = memory(3, 128, 64, 64);

        assert!(memory.write_word(63, 1).is_err());
    }

    #[test]
    fn the_interpreter_fetches_past_the_game_read_cap() {
        // A file longer than $ffff caps game reads (§1.1.2) but
        // not the interpreter's own fetches (§1.1.3).
        let memory = memory(8, 0x1_0400, 64, 64);

        assert!(memory.read_byte(0xFFFF).is_ok());
        assert!(memory.read_byte(0x1_0000).is_err());
        assert!(memory.fetch_byte(0x1_0000).is_ok());
        assert!(memory.fetch_byte(0x1_0400).is_err());
    }

    #[test]
    fn rejects_static_base_inside_the_header() {
        let story = Story::new(story_bytes(3, 128, 32, 64)).unwrap();
        let error = Memory::new(&story).unwrap_err();

        assert!(error.to_string().contains("§1.1.1"));
    }

    #[test]
    fn rejects_static_base_beyond_the_file() {
        let story = Story::new(story_bytes(3, 128, 256, 256)).unwrap();
        let error = Memory::new(&story).unwrap_err();

        assert!(error.to_string().contains("§1.1"));
    }

    #[test]
    fn rejects_high_memory_overlapping_dynamic() {
        let story = Story::new(story_bytes(3, 128, 100, 64)).unwrap();
        let error = Memory::new(&story).unwrap_err();

        assert!(error.to_string().contains("§1.1.3"));
    }

    #[test]
    fn allows_high_memory_overlapping_static() {
        let story = Story::new(story_bytes(3, 128, 64, 64)).unwrap();

        assert!(Memory::new(&story).is_ok());
    }

    #[test]
    fn rejects_file_exceeding_version_maximum() {
        let story = Story::new(story_bytes(3, 128 * 1024 + 1, 64, 64)).unwrap();
        let error = Memory::new(&story).unwrap_err();

        assert!(error.to_string().contains("§1.1.4"));
    }

    #[test]
    fn the_header_view_reflects_live_memory() {
        let mut memory = memory(3, 128, 64, 64);

        memory.write_byte(0x02, 0x01).unwrap();
        memory.write_byte(0x03, 0x2C).unwrap();

        assert_eq!(memory.header().release(), 300);
    }

    #[test]
    fn dynamic_snapshot_round_trips() {
        let mut memory = memory(3, 128, 64, 64);
        let pristine = memory.dynamic_snapshot();

        assert_eq!(pristine.len(), 64);

        memory.write_byte(40, 99).unwrap();
        memory.restore_dynamic(&pristine).unwrap();

        assert_eq!(memory.read_byte(40).unwrap(), 0);
    }

    #[test]
    fn restoring_a_foreign_capture_is_refused() {
        let mut memory = memory(3, 128, 64, 64);
        let error = memory.restore_dynamic(&[0; 65]).unwrap_err();

        assert!(error.to_string().contains("§6.1.2.1"));
    }
}
