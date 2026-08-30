//! The painted terminal: voxam's screen models on a ratatui glass.
//!
//! The reference paints with blessed -- damaged rows diffed by the
//! model, escape sequences by hand. This crate is the rewrite in
//! kind the port map planned: the models still decide everything,
//! and each repaint renders the whole grid into ratatui's buffer,
//! whose own double-buffer diff replaces the damaged-row sweep.
//! Anything worth testing lives in the models; the painting here
//! is held to golden TestBackend grids, the mirrored batteries'
//! own scenarios.

pub mod glk;
pub mod keys;
pub mod painter;

// The one terminal library, spoken with one voice: the CLI takes
// ratatui (and its crossterm re-export) from here, so the backend
// and the event queue can never drift apart by version.
pub use ratatui;
