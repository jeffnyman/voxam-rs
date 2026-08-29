//! The Voxam core: three virtual machines and the formats they
//! ride in, ported from the Python implementation at
//! <https://github.com/jeffnyman/voxam>.
//!
//! This crate holds everything a face needs and nothing a face
//! owns: the machines, the binary formats, and the wire types.
//! Frontends -- terminal, window, web, shell -- live in other
//! crates and consume this one.

pub mod errors;
pub mod format;
pub mod frontend;
pub mod iff;
pub mod saves;
pub mod zmachine;
