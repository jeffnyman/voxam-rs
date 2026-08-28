//! Typed access to the fields of the Z-Machine header (§11.1).
//!
//! The read side of the Python implementation's header module; the
//! `declare_*` write side -- the interpreter stamping in its own
//! capabilities -- arrives with the machine that needs it.

use crate::errors::VoxamError;

/// Dynamic memory must contain at least 64 bytes (§1.1.1), and the
/// first 64 bytes are the header (§1.1.1.1), so no story file can
/// be shorter.
pub const HEADER_SIZE: usize = 64;

/// Flags 1 lives at $01; Flags 2 in the word at $10 (§11.1).
pub const FLAGS_1: usize = 0x01;
pub const FLAGS_2: usize = 0x10;

/// A Version 3 game sets bit 1 of Flags 1 to ask for an
/// hours:minutes status line instead of score/turns (§8.2.3.2);
/// bit 3 is "the legendary 'Tandy' bit" (§11.1 remarks). Only
/// Version 3's Flags 1 defines these bits.
pub const TIME_STATUS_BIT: u8 = 0x02;
pub const TANDY_BIT: u8 = 0x08;
pub const STATUS_FLAGS_VERSION: u8 = 3;

/// The game-authored request bits of Flags 2 (§11.1): the low byte
/// holds pictures-or-font (3), undo (4), mouse (5), colours (6),
/// and sound (7); menus are bit 8, in the word's high byte.
pub const GRAPHICS_BIT: u16 = 0x08;
pub const UNDO_BIT: u16 = 0x10;
pub const MOUSE_BIT: u16 = 0x20;
pub const COLOURS_REQUEST_BIT: u16 = 0x40;
pub const SOUND_BIT: u16 = 0x80;
pub const MENUS_BIT: u16 = 0x01 << 8;

/// Version 6 gives Flags 2 bit 3 to pictures rather than the §16
/// character graphics font (§11.1).
pub const PICTURE_FLAGS_VERSION: u8 = 6;

/// Field locations from the table in §11.1.
const RELEASE: usize = 0x02;
const HIGH_MEMORY_BASE: usize = 0x04;
const INITIAL_PC: usize = 0x06;
const DICTIONARY: usize = 0x08;
const OBJECT_TABLE: usize = 0x0A;
const GLOBAL_VARIABLES: usize = 0x0C;
const STATIC_MEMORY_BASE: usize = 0x0E;
const SERIAL_START: usize = 0x12;
const SERIAL_END: usize = 0x18;
const ABBREVIATIONS_TABLE: usize = 0x18;
const FILE_LENGTH: usize = 0x1A;
const CHECKSUM: usize = 0x1C;

/// Versions 6 and 7 locate routines and static strings via
/// offsets, stored divided by 8, at $28 and $2a (§1.2.3, §11.1).
const ROUTINES_OFFSET: usize = 0x28;
const STATIC_STRINGS_OFFSET: usize = 0x2A;
pub const OFFSET_VERSIONS: [u8; 2] = [6, 7];

/// From Version 5, the word at $34 may name a custom alphabet
/// table; zero means the standard alphabets (§3.5.5, §11.1). The
/// word at $36 names the header extension table, whose third word
/// may in turn name a custom Unicode translation table (§3.8.5.2).
const ALPHABET_TABLE: usize = 0x34;
const TERMINATING_TABLE: usize = 0x2E;
const HEADER_EXTENSION: usize = 0x36;
const UNICODE_TABLE_WORD: u16 = 3;

/// The file length is stored divided by a version-dependent
/// constant (§11.1.6); index by version, 1 through 8.
const FILE_LENGTH_SCALE: [usize; 9] = [0, 2, 2, 2, 4, 4, 8, 8, 8];

/// Verification sums the bytes from $0040 up to the stored file
/// length, modulo $10000; padding beyond that length must be
/// excluded (§15, verify).
const CHECKSUM_START: usize = 0x40;

/// In Version 6 the word at $06 is the packed address of a "main"
/// routine rather than the byte address of a first instruction
/// (§11.1).
pub const PACKED_PC_VERSION: u8 = 6;

/// A typed view of the header fields within story file memory.
///
/// Over a Story's bytes this is a fixed view of the pristine file;
/// over a Memory's image it is a live view, since a running game
/// may legally alter parts of the header (§11.1.2.1). The caller
/// guarantees at least 64 bytes, as Story and Memory both validate.
#[derive(Debug, Clone, Copy)]
pub struct Header<'a> {
    data: &'a [u8],
}

impl<'a> Header<'a> {
    /// View the header within a validated story or memory image.
    pub fn over(data: &'a [u8]) -> Self {
        debug_assert!(
            data.len() >= HEADER_SIZE,
            "a header requires 64 bytes (§1.1.1.1)"
        );

        Self { data }
    }

    /// Read the big-endian word at a byte offset (§2.1), with the
    /// reference implementation's slicing manners: bytes beyond
    /// the data simply do not contribute.
    fn word(&self, offset: usize) -> u16 {
        let end = (offset + 2).min(self.data.len());
        let bytes = self.data.get(offset..end).unwrap_or(&[]);

        bytes
            .iter()
            .fold(0u16, |value, byte| value << 8 | u16::from(*byte))
    }

    /// The Z-Machine version this story targets (§11.1).
    pub fn version(&self) -> u8 {
        self.data[0]
    }

    /// The release number of this story (§11.1).
    pub fn release(&self) -> u16 {
        self.word(RELEASE)
    }

    /// Six ASCII characters, conventionally the compile date (§11.1).
    pub fn serial_number(&self) -> String {
        self.data[SERIAL_START..SERIAL_END]
            .iter()
            .map(|byte| char::from(*byte))
            .collect()
    }

    /// The story length in bytes, unscaled from the header word
    /// (§11.1.6). The file on disk may be longer: interpreters must
    /// allow for padding beyond the declared length (§15 remarks).
    pub fn declared_file_length(&self) -> usize {
        usize::from(self.word(FILE_LENGTH)) * FILE_LENGTH_SCALE[usize::from(self.version())]
    }

    /// The checksum the compiler recorded at $1c (§11.1).
    pub fn stored_checksum(&self) -> u16 {
        self.word(CHECKSUM)
    }

    /// The checksum of the story bytes actually present (§15, verify).
    pub fn computed_checksum(&self) -> u16 {
        let start = CHECKSUM_START.min(self.data.len());
        let end = self.declared_file_length().clamp(start, self.data.len());

        let sum: u32 = self.data[start..end]
            .iter()
            .fold(0u32, |sum, byte| (sum + u32::from(*byte)) & 0xFFFF);

        sum as u16
    }

    /// Whether the computed and stored checksums agree (§15).
    ///
    /// Some early Version 3 files store no length or checksum at
    /// all (§11.1), so a mismatch against a stored zero may mean
    /// "absent" rather than "corrupt".
    pub fn verify(&self) -> bool {
        self.computed_checksum() == self.stored_checksum()
    }

    /// The byte address at which high memory begins (§11.1).
    pub fn high_memory_base(&self) -> u16 {
        self.word(HIGH_MEMORY_BASE)
    }

    /// The byte address of the dictionary (§11.1).
    pub fn dictionary_address(&self) -> u16 {
        self.word(DICTIONARY)
    }

    /// The byte address of the object table (§11.1).
    pub fn object_table_address(&self) -> u16 {
        self.word(OBJECT_TABLE)
    }

    /// The byte address of the global variables table (§11.1).
    pub fn global_variables_address(&self) -> u16 {
        self.word(GLOBAL_VARIABLES)
    }

    /// The byte address at which static memory begins (§11.1).
    pub fn static_memory_base(&self) -> u16 {
        self.word(STATIC_MEMORY_BASE)
    }

    /// The byte address of the abbreviations table (§11.1).
    pub fn abbreviations_table_address(&self) -> u16 {
        self.word(ABBREVIATIONS_TABLE)
    }

    /// The custom alphabet table's byte address, or 0 for the
    /// standard alphabets (§3.5.5).
    pub fn alphabet_table_address(&self) -> u16 {
        self.word(ALPHABET_TABLE)
    }

    /// The terminating characters table's byte address, or 0 when
    /// new-line alone ends a read (§10.5.2.1).
    pub fn terminating_table_address(&self) -> u16 {
        self.word(TERMINATING_TABLE)
    }

    /// The custom Unicode translation table's address, or 0 for
    /// the default table of §3.8.5.3 (§3.8.5.2).
    pub fn unicode_translation_address(&self) -> u16 {
        let extension = self.word(HEADER_EXTENSION);

        if extension == 0 || self.word(usize::from(extension)) < UNICODE_TABLE_WORD {
            return 0;
        }

        self.word(usize::from(extension) + 2 * usize::from(UNICODE_TABLE_WORD))
    }

    /// The routines offset, as stored: divided by 8 (§1.2.3, §11.1).
    pub fn routines_offset(&self) -> Result<u16, VoxamError> {
        self.require_offset_version("routines offset")?;

        Ok(self.word(ROUTINES_OFFSET))
    }

    /// The static strings offset, as stored: divided by 8 (§1.2.3).
    pub fn static_strings_offset(&self) -> Result<u16, VoxamError> {
        self.require_offset_version("static strings offset")?;

        Ok(self.word(STATIC_STRINGS_OFFSET))
    }

    fn require_offset_version(&self, field: &str) -> Result<(), VoxamError> {
        if !OFFSET_VERSIONS.contains(&self.version()) {
            return Err(VoxamError::ZMachineHeader(format!(
                "version {} has no {field}; only versions 6 and 7 unpack addresses with one (§1.2.3)",
                self.version()
            )));
        }

        Ok(())
    }

    /// The byte address of the first instruction to execute (§11.1).
    pub fn initial_program_counter(&self) -> Result<u16, VoxamError> {
        if self.version() == PACKED_PC_VERSION {
            return Err(VoxamError::ZMachineHeader(
                "version 6 stores a packed routine address at $06, not an initial \
                 program counter; use main_routine_packed_address (§11.1)"
                    .into(),
            ));
        }

        Ok(self.word(INITIAL_PC))
    }

    /// The packed address of the initial routine in Version 6 (§11.1).
    pub fn main_routine_packed_address(&self) -> Result<u16, VoxamError> {
        if self.version() != PACKED_PC_VERSION {
            return Err(VoxamError::ZMachineHeader(format!(
                "version {} stores an initial program counter at $06, not a packed \
                 routine address; use initial_program_counter (§11.1)",
                self.version()
            )));
        }

        Ok(self.word(INITIAL_PC))
    }

    /// Whether the status line shows the time of day (§8.2.3.2).
    ///
    /// A Version 3 game claims a clock with bit 1 of Flags 1;
    /// Versions 1 and 2 predate the bit and always show score and
    /// turns.
    pub fn time_game(&self) -> Result<bool, VoxamError> {
        if self.version() > STATUS_FLAGS_VERSION {
            return Err(VoxamError::ZMachineHeader(format!(
                "version {} has no status line for a type bit to describe (§8.2)",
                self.version()
            )));
        }

        if self.version() < STATUS_FLAGS_VERSION {
            return Ok(false);
        }

        Ok(self.data[FLAGS_1] & TIME_STATUS_BIT != 0)
    }

    /// The Flags 1 byte at $01, as shipped (§11.1).
    pub fn flags_1(&self) -> u8 {
        self.data[FLAGS_1]
    }

    /// The Flags 2 word at $10, as shipped (§11.1).
    pub fn flags_2(&self) -> u16 {
        self.word(FLAGS_2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zmachine::testing::story_bytes;

    #[test]
    fn reads_the_identity_fields() {
        let mut data = story_bytes(3, 128, 64, 64);
        data[RELEASE..RELEASE + 2].copy_from_slice(&97u16.to_be_bytes());

        let header = Header::over(&data);

        assert_eq!(header.version(), 3);
        assert_eq!(header.release(), 97);
        assert_eq!(header.serial_number(), "851218");
    }

    #[test]
    fn scales_declared_length_by_version() {
        for (version, scale) in [(1, 2), (3, 2), (4, 4), (5, 4), (6, 8), (8, 8)] {
            let mut data = story_bytes(version, 64, 64, 64);
            data[FILE_LENGTH..FILE_LENGTH + 2].copy_from_slice(&100u16.to_be_bytes());

            assert_eq!(Header::over(&data).declared_file_length(), 100 * scale);
        }
    }

    #[test]
    fn computes_checksum_by_the_spec_rule() {
        // Bytes from $40 up to the declared length, mod $10000
        // (§15, verify); padding beyond the length is excluded.
        let mut data = story_bytes(3, 128, 64, 64);
        data[FILE_LENGTH..FILE_LENGTH + 2].copy_from_slice(&40u16.to_be_bytes());
        data[0x40..0x50].fill(7);
        data[0x50..].fill(255);

        let header = Header::over(&data);

        assert_eq!(header.computed_checksum(), 7 * 16);
    }

    #[test]
    fn verification_judges_the_stored_word() {
        let mut data = story_bytes(3, 128, 64, 64);
        data[FILE_LENGTH..FILE_LENGTH + 2].copy_from_slice(&40u16.to_be_bytes());
        data[0x40..0x50].fill(7);
        data[CHECKSUM..CHECKSUM + 2].copy_from_slice(&(7u16 * 16).to_be_bytes());

        assert!(Header::over(&data).verify());

        data[0x40] = 8;

        assert!(!Header::over(&data).verify());
    }

    #[test]
    fn a_declared_length_beyond_the_file_is_clamped() {
        let mut data = story_bytes(3, 128, 64, 64);
        data[FILE_LENGTH..FILE_LENGTH + 2].copy_from_slice(&0xFFFFu16.to_be_bytes());
        data[0x40..].fill(1);

        assert_eq!(Header::over(&data).computed_checksum(), 128 - 64);
    }

    #[test]
    fn version_6_stores_a_packed_main_routine() {
        let data = story_bytes(6, 64, 64, 64);
        let header = Header::over(&data);

        assert!(header.main_routine_packed_address().is_ok());

        let error = header.initial_program_counter().unwrap_err();
        assert!(error.to_string().contains("§11.1"));
    }

    #[test]
    fn other_versions_store_an_initial_program_counter() {
        let data = story_bytes(3, 64, 64, 64);
        let header = Header::over(&data);

        assert!(header.initial_program_counter().is_ok());
        assert!(header.main_routine_packed_address().is_err());
    }

    #[test]
    fn offsets_belong_to_versions_6_and_7_alone() {
        for version in [6, 7] {
            let data = story_bytes(version, 64, 64, 64);

            assert!(Header::over(&data).routines_offset().is_ok());
            assert!(Header::over(&data).static_strings_offset().is_ok());
        }

        let data = story_bytes(5, 64, 64, 64);
        let error = Header::over(&data).routines_offset().unwrap_err();

        assert!(error.to_string().contains("§1.2.3"));
    }

    #[test]
    fn the_time_bit_speaks_only_in_version_3() {
        let mut data = story_bytes(3, 64, 64, 64);

        assert!(!Header::over(&data).time_game().unwrap());

        data[FLAGS_1] |= TIME_STATUS_BIT;

        assert!(Header::over(&data).time_game().unwrap());

        let earlier = story_bytes(1, 64, 64, 64);
        assert!(!Header::over(&earlier).time_game().unwrap());

        let later = story_bytes(4, 64, 64, 64);
        assert!(Header::over(&later).time_game().is_err());
    }
}
