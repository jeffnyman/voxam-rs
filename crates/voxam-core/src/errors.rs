//! The error family, mirroring the Python implementation's
//! `voxam.errors`: every failure names the rule it enforces, §
//! citation included, and the CLI prints the message bare behind
//! a `voxam:` prefix.

use std::fmt;

/// A rule of one of the machines, enforced with its citation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoxamError {
    Blorb(String),
    Iff(String),
    ZMachineArithmetic(String),
    ZMachineQuetzal(String),
    ZMachineStory(String),
    ZMachineHeader(String),
    ZMachineInstruction(String),
    ZMachineMemory(String),
    ZMachineObject(String),
    ZMachineRoutine(String),
    ZMachineStack(String),
    ZMachineText(String),
    /// The frontier reporter: pointing Voxam at a story and reading
    /// this error's message is how the implementation backlog
    /// announces itself.
    ZMachineUnimplemented(String),
}

impl fmt::Display for VoxamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (Self::Blorb(message)
        | Self::Iff(message)
        | Self::ZMachineArithmetic(message)
        | Self::ZMachineQuetzal(message)
        | Self::ZMachineStory(message)
        | Self::ZMachineHeader(message)
        | Self::ZMachineInstruction(message)
        | Self::ZMachineMemory(message)
        | Self::ZMachineObject(message)
        | Self::ZMachineRoutine(message)
        | Self::ZMachineStack(message)
        | Self::ZMachineText(message)
        | Self::ZMachineUnimplemented(message)) = self;

        f.write_str(message)
    }
}

impl std::error::Error for VoxamError {}
