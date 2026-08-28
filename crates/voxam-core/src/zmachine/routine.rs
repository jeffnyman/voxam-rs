//! Parsing routine headers (§5).
//!
//! A routine is a header -- one byte counting the local variables,
//! plus, in Versions 1 to 4, their initial values -- followed by
//! instructions. Nothing here executes; this module only reads the
//! header and locates the first instruction.

use crate::errors::VoxamError;
use crate::zmachine::memory::Memory;

/// A routine has between 0 and 15 local variables (§5.2).
const MAX_LOCALS: u8 = 15;

/// Initial local values live in the routine header only through
/// Version 4; from Version 5 they are all zero (§5.2.1).
const LOCALS_IN_HEADER_LAST_VERSION: u8 = 4;

const WORD_SIZE: usize = 2;

/// A parsed routine header (§5.2): its address, one initial value
/// per local (§5.2.1), and the byte address execution begins at
/// (§5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routine {
    pub address: usize,
    pub initial_locals: Vec<u16>,
    pub first_instruction: usize,
}

impl Routine {
    /// Parse the routine header beginning at an address (§5.2).
    ///
    /// Refuses a local count over 15, which usually means the
    /// address is not a routine at all.
    pub fn parse(memory: &Memory, address: usize) -> Result<Self, VoxamError> {
        let count = memory.fetch_byte(address)?;

        if count > MAX_LOCALS {
            return Err(VoxamError::ZMachineRoutine(format!(
                "the byte at ${address:04x} claims {count} locals, but a routine has \
                 at most {MAX_LOCALS} (§5.2); this is probably not a routine address"
            )));
        }

        if memory.header().version() <= LOCALS_IN_HEADER_LAST_VERSION {
            let mut initial_locals = Vec::with_capacity(usize::from(count));

            for index in 0..usize::from(count) {
                initial_locals.push(memory.fetch_word(address + 1 + WORD_SIZE * index)?);
            }

            Ok(Self {
                address,
                first_instruction: address + 1 + WORD_SIZE * usize::from(count),
                initial_locals,
            })
        } else {
            Ok(Self {
                address,
                initial_locals: vec![0; usize::from(count)],
                first_instruction: address + 1,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zmachine::testing::planted_memory;

    #[test]
    fn parses_initial_locals_through_version_4() {
        let memory = planted_memory(3, &[(0x80, &[2, 0x12, 0x34, 0x56, 0x78])]);
        let routine = Routine::parse(&memory, 0x80).unwrap();

        assert_eq!(routine.initial_locals, [0x1234, 0x5678]);
        assert_eq!(routine.first_instruction, 0x85);
    }

    #[test]
    fn locals_start_at_zero_from_version_5() {
        let memory = planted_memory(5, &[(0x80, &[3, 0xFF, 0xFF])]);
        let routine = Routine::parse(&memory, 0x80).unwrap();

        assert_eq!(routine.initial_locals, [0, 0, 0]);
        assert_eq!(routine.first_instruction, 0x81);
    }

    #[test]
    fn a_routine_may_have_no_locals() {
        let memory = planted_memory(3, &[(0x80, &[0])]);
        let routine = Routine::parse(&memory, 0x80).unwrap();

        assert!(routine.initial_locals.is_empty());
        assert_eq!(routine.first_instruction, 0x81);
    }

    #[test]
    fn fifteen_locals_is_the_maximum() {
        let memory = planted_memory(5, &[(0x80, &[16])]);
        let error = Routine::parse(&memory, 0x80).unwrap_err();

        assert!(error.to_string().contains("§5.2"));
    }
}
