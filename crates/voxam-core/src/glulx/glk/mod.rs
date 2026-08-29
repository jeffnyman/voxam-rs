//! Glk: the I/O layer Glulx games speak through.
//!
//! Glk is a portable API, not a wire protocol: a game calls named
//! functions like glk_put_char and glk_window_open, and Glulx
//! reaches them through the glk opcode by number. This package
//! carries that world in layers -- the object model the functions
//! operate on, the dispatch table that knows every function's
//! signature, and, in eras to come, the function surface itself
//! and the display that makes a session visible.
//!
//! Citations name sections of the vendored Glk 0.7.6
//! specification: (Glk: Text Grid Windows) works the way (Glulx:
//! The Header) and Z-Machine §1.1 citations do elsewhere in Voxam.
//!
//! One departure shapes the whole package: where the reference's
//! objects hold each other directly -- a window its parent pair, a
//! stream its window -- the port's objects live in id-keyed maps
//! and hold ids, with the maps passed to the operations that walk
//! them. The 32-bit ids Glulx sees stay the bridge's separate,
//! lazily-minted business, exactly as in the reference, so
//! transcripts diff identically.

pub mod dispatch;
pub mod objects;
