//! The error family, mirroring the Python implementation's
//! `voxam.errors`: every failure names the rule it enforces, §
//! citation included, and the CLI prints the message bare behind
//! a `voxam:` prefix.

use std::fmt;

/// A rule of one of the machines, enforced with its citation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoxamError {
    ZMachineStory(String),
    ZMachineHeader(String),
    ZMachineMemory(String),
    ZMachineText(String),
}

impl fmt::Display for VoxamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (Self::ZMachineStory(message)
        | Self::ZMachineHeader(message)
        | Self::ZMachineMemory(message)
        | Self::ZMachineText(message)) = self;

        f.write_str(message)
    }
}

impl std::error::Error for VoxamError {}
