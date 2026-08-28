//! Unpacking packed addresses into byte addresses (§1.2.3).
//!
//! A packed address is how a 16-bit word reaches a routine or
//! string in high memory: the word is scaled up by a
//! version-dependent factor, and Versions 6 and 7 add a
//! header-declared offset on top. Routines and strings use
//! different offsets, hence the two functions.

use crate::errors::VoxamError;
use crate::zmachine::header::{Header, OFFSET_VERSIONS};

/// The scale factor from packed to byte address (§1.2.3); index by
/// version, 1 through 8. Versions 6 and 7 scale by 4 and then add
/// an offset.
const SCALE: [usize; 9] = [0, 2, 2, 2, 4, 4, 4, 4, 8];

/// The header stores the routine and string offsets divided by 8
/// (§1.2.3), so unpacking multiplies them back up.
const OFFSET_FACTOR: usize = 8;

/// Unpack the byte address a packed routine address means
/// (§1.2.3), as found in an operand or at $06.
pub fn routine_address(header: &Header, packed: u16) -> Result<usize, VoxamError> {
    let mut address = usize::from(packed) * SCALE[usize::from(header.version())];

    if OFFSET_VERSIONS.contains(&header.version()) {
        address += OFFSET_FACTOR * usize::from(header.routines_offset()?);
    }

    Ok(address)
}

/// Unpack the byte address a packed string address means (§1.2.3),
/// as used by print_paddr.
pub fn string_address(header: &Header, packed: u16) -> Result<usize, VoxamError> {
    let mut address = usize::from(packed) * SCALE[usize::from(header.version())];

    if OFFSET_VERSIONS.contains(&header.version()) {
        address += OFFSET_FACTOR * usize::from(header.static_strings_offset()?);
    }

    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zmachine::testing::story_bytes;

    fn header_bytes(version: u8) -> Vec<u8> {
        story_bytes(version, 64, 64, 64)
    }

    #[test]
    fn scales_packed_addresses_by_version() {
        for (version, scale) in [(1, 2), (3, 2), (4, 4), (5, 4), (8, 8)] {
            let data = header_bytes(version);
            let header = Header::over(&data);

            assert_eq!(routine_address(&header, 0x100).unwrap(), 0x100 * scale);
            assert_eq!(string_address(&header, 0x100).unwrap(), 0x100 * scale);
        }
    }

    #[test]
    fn versions_6_and_7_add_distinct_offsets() {
        for version in [6, 7] {
            let mut data = header_bytes(version);
            data[0x28..0x2A].copy_from_slice(&0x0010u16.to_be_bytes());
            data[0x2A..0x2C].copy_from_slice(&0x0020u16.to_be_bytes());

            let header = Header::over(&data);

            assert_eq!(
                routine_address(&header, 0x100).unwrap(),
                0x100 * 4 + 8 * 0x10
            );
            assert_eq!(
                string_address(&header, 0x100).unwrap(),
                0x100 * 4 + 8 * 0x20
            );
        }
    }

    #[test]
    fn other_versions_have_no_offsets() {
        let mut data = header_bytes(5);
        data[0x28..0x2A].copy_from_slice(&0x0010u16.to_be_bytes());

        let header = Header::over(&data);

        assert_eq!(routine_address(&header, 0x100).unwrap(), 0x400);
    }
}
