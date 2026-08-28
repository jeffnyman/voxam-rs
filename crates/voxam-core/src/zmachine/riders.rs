//! Decoding the riders that may follow an instruction's operands.
//!
//! A store variable (§4.6), a branch (§4.7), and encoded text
//! (§3.2) are the three sections an instruction may carry after its
//! operands. Which opcodes carry which is knowledge that lives in
//! the opcode tables; this module only knows how to read each rider
//! once told it is present.

use crate::errors::VoxamError;
use crate::zmachine::memory::Memory;

/// Offsets 0 and 1 are not jumps: they mean return false and return
/// true from the current routine (§4.7.1).
const RETURN_FALSE_OFFSET: i32 = 0;
const RETURN_TRUE_OFFSET: i32 = 1;

/// A branch destination is the address after the branch data, plus
/// the offset, minus two (§4.7.2).
const BRANCH_TARGET_ADJUSTMENT: i64 = 2;

/// Bit 7 of the first branch byte: set means branch on true, clear
/// means branch on false (§4.7).
const BRANCH_ON_TRUE_BIT: u8 = 0b1000_0000;

/// Bit 6 of the first branch byte: set means the branch occupies
/// one byte, with an unsigned offset in the bottom 6 bits; clear
/// means a signed 14-bit offset in the bottom 6 bits plus a second
/// byte (§4.7).
const SHORT_BRANCH_BIT: u8 = 0b0100_0000;
const OFFSET_HIGH_MASK: u8 = 0b0011_1111;

/// Two's complement bounds for the signed 14-bit long offset (§4.7).
const LONG_OFFSET_SIGN: i32 = 1 << 13;
const LONG_OFFSET_RANGE: i32 = 1 << 14;

/// Encoded text is a sequence of words; only the last word of a
/// string has its top bit set (§3.2).
const STRING_TERMINATOR_BIT: u16 = 0x8000;

/// A decoded branch rider (§4.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Branch {
    /// Whether the branch is taken when the tested condition is
    /// true (bit 7 set) rather than false (bit 7 clear).
    pub on_true: bool,
    /// The branch offset. The values 0 and 1 do not mean a jump;
    /// they mean return false and return true (§4.7.1).
    pub offset: i32,
}

impl Branch {
    /// Whether this branch means "return false" (§4.7.1).
    pub fn returns_false(&self) -> bool {
        self.offset == RETURN_FALSE_OFFSET
    }

    /// Whether this branch means "return true" (§4.7.1).
    pub fn returns_true(&self) -> bool {
        self.offset == RETURN_TRUE_OFFSET
    }

    /// The destination address for a taken branch (§4.7.2), given
    /// the address immediately after the branch data. Refuses the
    /// offsets that mean a return rather than a jump (§4.7.1).
    pub fn target(&self, after: usize) -> Result<usize, VoxamError> {
        if self.returns_false() || self.returns_true() {
            return Err(VoxamError::ZMachineInstruction(format!(
                "branch offset {} means a return, not a jump, and has no target \
                 address (§4.7.1)",
                self.offset
            )));
        }

        let target = after as i64 + i64::from(self.offset) - BRANCH_TARGET_ADJUSTMENT;

        usize::try_from(target).map_err(|_| {
            VoxamError::ZMachineInstruction(format!(
                "branch offset {} from ${after:04x} lands before the story begins \
                 (§4.7.2)",
                self.offset
            ))
        })
    }
}

/// Read a store rider: the variable number for a result (§4.6),
/// returning it and the first address past the rider.
pub fn read_store_variable(memory: &Memory, address: usize) -> Result<(u8, usize), VoxamError> {
    Ok((memory.fetch_byte(address)?, address + 1))
}

/// Read a branch rider of one or two bytes (§4.7), returning the
/// decoded branch and the first address past the rider.
pub fn read_branch(memory: &Memory, address: usize) -> Result<(Branch, usize), VoxamError> {
    let first = memory.fetch_byte(address)?;
    let on_true = first & BRANCH_ON_TRUE_BIT != 0;

    if first & SHORT_BRANCH_BIT != 0 {
        return Ok((
            Branch {
                on_true,
                offset: i32::from(first & OFFSET_HIGH_MASK),
            },
            address + 1,
        ));
    }

    let mut offset =
        (i32::from(first & OFFSET_HIGH_MASK) << 8) | i32::from(memory.fetch_byte(address + 1)?);

    if offset & LONG_OFFSET_SIGN != 0 {
        offset -= LONG_OFFSET_RANGE;
    }

    Ok((Branch { on_true, offset }, address + 2))
}

/// Find the end of an encoded string without decoding it (§3.2):
/// the first address past the word whose top bit is set. Fails if
/// no terminator appears before the end of the story file.
pub fn text_end(memory: &Memory, address: usize) -> Result<usize, VoxamError> {
    let mut position = address;

    while memory.fetch_word(position)? & STRING_TERMINATOR_BIT == 0 {
        position += 2;
    }

    Ok(position + 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zmachine::testing::planted_memory;

    #[test]
    fn reads_a_store_variable() {
        let memory = planted_memory(3, &[(0x80, &[0x42])]);

        assert_eq!(read_store_variable(&memory, 0x80).unwrap(), (0x42, 0x81));
    }

    #[test]
    fn reads_short_branches() {
        let memory = planted_memory(3, &[(0x80, &[0b1100_0101])]);
        let (branch, end) = read_branch(&memory, 0x80).unwrap();

        assert!(branch.on_true);
        assert_eq!(branch.offset, 5);
        assert_eq!(end, 0x81);

        let memory = planted_memory(3, &[(0x80, &[0b0100_0011])]);
        let (branch, _) = read_branch(&memory, 0x80).unwrap();

        assert!(!branch.on_true);
        assert_eq!(branch.offset, 3);
    }

    #[test]
    fn reads_long_branches() {
        let memory = planted_memory(3, &[(0x80, &[0b1000_0001, 0x00])]);
        let (branch, end) = read_branch(&memory, 0x80).unwrap();

        assert!(branch.on_true);
        assert_eq!(branch.offset, 0x100);
        assert_eq!(end, 0x82);

        // The 14-bit offset is signed: $3FFF is -1.
        let memory = planted_memory(3, &[(0x80, &[0b0011_1111, 0xFF])]);
        let (branch, _) = read_branch(&memory, 0x80).unwrap();

        assert!(!branch.on_true);
        assert_eq!(branch.offset, -1);
    }

    #[test]
    fn offsets_0_and_1_mean_returns() {
        let memory = planted_memory(3, &[(0x80, &[0b1100_0000, 0b1100_0001])]);

        let (branch, _) = read_branch(&memory, 0x80).unwrap();
        assert!(branch.returns_false());
        assert!(!branch.returns_true());

        let (branch, _) = read_branch(&memory, 0x81).unwrap();
        assert!(branch.returns_true());
        assert!(branch.target(0x82).is_err());
    }

    #[test]
    fn computes_a_branch_target() {
        let branch = Branch {
            on_true: true,
            offset: 10,
        };

        assert_eq!(branch.target(0x100).unwrap(), 0x108);

        let backward = Branch {
            on_true: true,
            offset: -4,
        };

        assert_eq!(backward.target(0x100).unwrap(), 0xFA);
    }

    #[test]
    fn finds_the_end_of_encoded_text() {
        let memory = planted_memory(3, &[(0x80, &[0x12, 0x34, 0x94, 0xA5])]);

        assert_eq!(text_end(&memory, 0x80).unwrap(), 0x84);

        let memory = planted_memory(3, &[(0x80, &[0x94, 0xA5])]);
        assert_eq!(text_end(&memory, 0x80).unwrap(), 0x82);
    }

    #[test]
    fn unterminated_text_cannot_scan_past_readable_memory() {
        // Nothing but zero words to the end of the file.
        let memory = planted_memory(3, &[]);

        assert!(text_end(&memory, 0x1F0).is_err());
    }
}
