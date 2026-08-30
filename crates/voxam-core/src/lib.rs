//! The Voxam core: three virtual machines and the formats they
//! ride in, ported from the Python implementation at
//! <https://github.com/jeffnyman/voxam>.
//!
//! This crate holds everything a face needs and nothing a face
//! owns: the machines, the binary formats, and the wire types.
//! Frontends -- terminal, window, web, shell -- live in other
//! crates and consume this one.

pub mod aamachine;
pub mod aiff;
pub mod babel;
pub mod base64;
pub mod blorb;
pub mod errors;
pub mod flate;
pub mod format;
pub mod frontend;
pub mod gallery;
#[cfg(test)]
mod gallery_tests;
pub mod glkote;
pub mod glulx;
pub mod iff;
pub mod infocom;
pub mod png;
#[cfg(test)]
mod png_tests;
pub mod saves;
pub mod screen;
pub mod wav;
pub mod zmachine;
