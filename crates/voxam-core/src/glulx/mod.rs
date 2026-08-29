//! Glulx: the Z-Machine's successor, built to shed its size
//! limits, and the target of today's Inform (Glulx 3.1.3).

pub mod accel;
pub mod floats;
pub mod funcs;
pub mod gestalt;
pub mod heap;
pub mod machine;
pub mod memory;
pub mod opcodes;
pub mod operand;
pub mod rng;
pub mod search;
pub mod serial;
pub mod stack;
pub mod story;
pub mod strings;

#[cfg(test)]
pub(crate) mod testing;
