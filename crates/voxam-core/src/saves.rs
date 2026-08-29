//! Where saved games live (§6.1.1.1).
//!
//! The Standard leaves the format of a saved game to the
//! interpreter (§6.1.1.1) -- Voxam writes Quetzal -- and says
//! nothing at all about where the bytes go. A SaveSlot is that
//! decision, kept apart from the machine: the machine asks the
//! slot to keep or produce bytes, and failure is an answer, not an
//! accident, because save and restore report failure to the story
//! as an ordinary result (§15).

use std::path::PathBuf;

/// A home for one saved game's bytes.
pub trait SaveSlot {
    /// Keep a saved game, reporting whether it was kept.
    fn write(&mut self, data: &[u8]) -> bool;

    /// Produce the saved game last kept, or None without one.
    fn read(&mut self) -> Option<Vec<u8>>;
}

/// A save slot bound to one file path.
pub struct FileSaveSlot {
    pub path: PathBuf,
}

impl SaveSlot for FileSaveSlot {
    /// Write the saved game to the path (§15 save); a refused disk
    /// is a failed save, not a crash.
    fn write(&mut self, data: &[u8]) -> bool {
        std::fs::write(&self.path, data).is_ok()
    }

    /// Read the saved game back (§15 restore); None means the path
    /// has no saved game to give.
    fn read(&mut self) -> Option<Vec<u8>> {
        std::fs::read(&self.path).ok()
    }
}
