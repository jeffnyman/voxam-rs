//! The Z-Machine: Infocom's 1979 virtual machine, implemented to
//! the Z-Machine Standard 1.1.

pub mod dictionary;
pub mod frames;
pub mod header;
pub mod instruction;
pub mod machine;
pub mod memory;
pub mod objects;
pub mod opcodes;
pub mod packed;
pub mod quetzal;
pub mod riders;
pub mod rng;
pub mod routine;
pub mod snapshot;
pub mod story;
pub mod variables;
pub mod windows;
pub mod zscii;

/// Synthetic story files for the module tests: the smallest bytes
/// that pass validation, shaped one knob at a time.
#[cfg(test)]
pub(crate) mod testing {
    /// A story image with the given version and region boundaries,
    /// a serial of "851218", and zeroes elsewhere.
    pub(crate) fn story_bytes(
        version: u8,
        length: usize,
        static_base: u16,
        high_base: u16,
    ) -> Vec<u8> {
        let mut data = vec![0u8; length];
        data[0] = version;
        data[0x04..0x06].copy_from_slice(&high_base.to_be_bytes());
        data[0x0E..0x10].copy_from_slice(&static_base.to_be_bytes());
        data[0x12..0x18].copy_from_slice(b"851218");

        data
    }

    /// A 512-byte memory image in the reference test suite's shape
    /// -- static base $1C0 -- with chunks planted where asked.
    pub(crate) fn planted_memory(
        version: u8,
        plants: &[(usize, &[u8])],
    ) -> crate::zmachine::memory::Memory {
        let mut data = story_bytes(version, 512, 0x01C0, 0x01C0);
        data[0x12..0x18].fill(0);

        for (at, chunk) in plants {
            data[*at..at + chunk.len()].copy_from_slice(chunk);
        }

        let story = crate::zmachine::story::Story::new(data).unwrap();

        crate::zmachine::memory::Memory::new(&story).unwrap()
    }

    /// Pack Z-characters into encoded words: padded with 5s, three
    /// to a word, the terminator bit on the last (§3.2).
    pub(crate) fn pack(zchars: &[u8]) -> Vec<u8> {
        let mut padded = zchars.to_vec();
        while !padded.len().is_multiple_of(3) {
            padded.push(5);
        }

        let mut out = Vec::new();
        for (index, triple) in padded.chunks(3).enumerate() {
            let mut word =
                (u16::from(triple[0]) << 10) | (u16::from(triple[1]) << 5) | u16::from(triple[2]);
            if (index + 1) * 3 == padded.len() {
                word |= 0x8000;
            }
            out.extend_from_slice(&word.to_be_bytes());
        }

        out
    }
}
