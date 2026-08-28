//! Reading and writing the 256 variable numbers (§4.2.2).
//!
//! Variable number $00 means the stack: writing pushes and reading
//! pulls (§6.3). Numbers $01 to $0f are the current routine's
//! locals, and $10 to $ff are the globals, stored as a word table
//! in dynamic memory (§6.2). This module unifies the three behind
//! one read and one write.
//!
//! As with the object table, the Python reference binds its stores
//! at construction; here `Variables` keeps only the globals
//! address, and each call names the memory and call state it
//! resolves into.

use crate::errors::VoxamError;
use crate::zmachine::frames::CallStack;
use crate::zmachine::memory::Memory;

/// Variable $00 is the stack; locals then run to $0f, and globals
/// from $10 to $ff (§4.2.2).
const STACK_VARIABLE: u8 = 0x00;
const FIRST_GLOBAL: u8 = 0x10;

const WORD_SIZE: usize = 2;

/// One façade over the stack, locals, and globals (§4.2.2).
pub struct Variables {
    globals: usize,
}

impl Variables {
    /// Fix the globals table's address from the header (§6.2).
    pub fn new(memory: &Memory) -> Self {
        Self {
            globals: usize::from(memory.header().global_variables_address()),
        }
    }

    /// Read a variable: pulling, a local, or a global (§4.2.2).
    /// For $00, the pulled top of stack (§6.3).
    pub fn read(
        &self,
        memory: &Memory,
        calls: &mut CallStack,
        number: u8,
    ) -> Result<u16, VoxamError> {
        if number == STACK_VARIABLE {
            return calls.pop();
        }

        if number < FIRST_GLOBAL {
            return calls.local(number);
        }

        memory.read_word(self.global_address(number))
    }

    /// Write a variable: pushing, a local, or a global (§4.2.2).
    /// For $00, the value is pushed (§6.3).
    pub fn write(
        &self,
        memory: &mut Memory,
        calls: &mut CallStack,
        number: u8,
        value: u16,
    ) -> Result<(), VoxamError> {
        if number == STACK_VARIABLE {
            return calls.push(value);
        }

        if number < FIRST_GLOBAL {
            return calls.set_local(number, value);
        }

        memory.write_word(self.global_address(number), value)
    }

    /// Read a variable by reference: the stack top stays put
    /// (§6.3.4). The seven indirect-reference opcodes use this
    /// instead of read.
    pub fn read_in_place(
        &self,
        memory: &Memory,
        calls: &mut CallStack,
        number: u8,
    ) -> Result<u16, VoxamError> {
        if number == STACK_VARIABLE {
            return calls.peek();
        }

        self.read(memory, calls, number)
    }

    /// Write a variable by reference: the stack top is replaced
    /// (§6.3.4).
    pub fn write_in_place(
        &self,
        memory: &mut Memory,
        calls: &mut CallStack,
        number: u8,
        value: u16,
    ) -> Result<(), VoxamError> {
        if number == STACK_VARIABLE {
            return calls.replace_top(value);
        }

        self.write(memory, calls, number, value)
    }

    /// Locate a global in the table at the header's address (§6.2).
    fn global_address(&self, number: u8) -> usize {
        self.globals + WORD_SIZE * usize::from(number - FIRST_GLOBAL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zmachine::routine::Routine;
    use crate::zmachine::testing::planted_memory;

    fn scene() -> (Memory, CallStack, Variables) {
        // The test image's globals table sits at $100.
        let memory = planted_memory(3, &[(0x0C, &0x0100u16.to_be_bytes())]);
        let variables = Variables::new(&memory);

        (memory, CallStack::new(), variables)
    }

    #[test]
    fn variable_0_is_the_stack() {
        let (mut memory, mut calls, variables) = scene();

        variables.write(&mut memory, &mut calls, 0, 0xBEEF).unwrap();

        assert_eq!(variables.read(&memory, &mut calls, 0).unwrap(), 0xBEEF);
        assert!(variables.read(&memory, &mut calls, 0).is_err());
    }

    #[test]
    fn low_numbers_are_locals() {
        let (mut memory, mut calls, variables) = scene();
        let routine = Routine {
            address: 0,
            initial_locals: vec![0x11, 0x22],
            first_instruction: 0,
        };
        calls.call(&routine, &[], 0, None).unwrap();

        assert_eq!(variables.read(&memory, &mut calls, 1).unwrap(), 0x11);

        variables.write(&mut memory, &mut calls, 2, 0x33).unwrap();
        assert_eq!(variables.read(&memory, &mut calls, 2).unwrap(), 0x33);
    }

    #[test]
    fn high_numbers_are_globals_in_memory() {
        let (mut memory, mut calls, variables) = scene();

        variables
            .write(&mut memory, &mut calls, 0x10, 0x1234)
            .unwrap();
        variables
            .write(&mut memory, &mut calls, 0x2F, 0x5678)
            .unwrap();

        assert_eq!(memory.read_word(0x100).unwrap(), 0x1234);
        assert_eq!(memory.read_word(0x100 + 2 * 0x1F).unwrap(), 0x5678);
        assert_eq!(variables.read(&memory, &mut calls, 0x2F).unwrap(), 0x5678);
    }

    #[test]
    fn globals_read_what_the_story_shipped() {
        let memory = planted_memory(
            3,
            &[(0x0C, &0x0100u16.to_be_bytes()), (0x102, &[0xAB, 0xCD])],
        );
        let variables = Variables::new(&memory);
        let mut calls = CallStack::new();

        assert_eq!(variables.read(&memory, &mut calls, 0x11).unwrap(), 0xABCD);
    }

    #[test]
    fn in_place_access_leaves_the_stack_standing() {
        let (mut memory, mut calls, variables) = scene();
        calls.push(0x42).unwrap();

        assert_eq!(
            variables.read_in_place(&memory, &mut calls, 0).unwrap(),
            0x42
        );
        assert_eq!(
            variables.read_in_place(&memory, &mut calls, 0).unwrap(),
            0x42
        );

        variables
            .write_in_place(&mut memory, &mut calls, 0, 0x43)
            .unwrap();

        assert_eq!(calls.pop().unwrap(), 0x43);
        assert!(calls.pop().is_err());
    }
}
