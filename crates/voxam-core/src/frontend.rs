//! Where a machine's text and status go (§7, §8).
//!
//! The trait mirrors the Python reference's Frontend protocol,
//! trimmed to what the plain stream needs; the richer faces --
//! painted terminal, window, wire -- implement it as they arrive.
//! Default answers are the plain stream's honest claims.

/// One status line's worth of game state (§8.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The short name of the object held in the first global
    /// variable -- the player's whereabouts (§8.2.2).
    pub location: String,
    /// The second global: the score, or the hour of a 24-hour
    /// clock in a time game (§8.2.3).
    pub score: i32,
    /// The third global: the turn count, or the minutes (§8.2.3).
    pub turns: u16,
    /// Whether the numbers are a clock reading rather than score
    /// and turns (§8.2.3.2).
    pub time_game: bool,
}

/// A presentation seam: where story text lands, and what the
/// interpreter may honestly claim about it (§11.1).
pub trait Frontend {
    /// Send story text onward; surrogates are already fused.
    fn write(&mut self, text: &str);

    /// Redraw the status line (§8.2); only called when
    /// `has_status_line` is claimed.
    fn show_status(&mut self, _status: &Status) {}

    /// Sound a §9 bleep; the plain stream's is silence.
    fn bleep(&mut self, _high: bool) {}

    fn has_status_line(&self) -> bool {
        false
    }

    fn has_screen_splitting(&self) -> bool {
        false
    }

    fn has_bold(&self) -> bool {
        false
    }

    fn has_italic(&self) -> bool {
        false
    }

    fn has_fixed_pitch(&self) -> bool {
        true
    }

    /// Timed input is real, if virtual: the machine fires read
    /// interrupts on the patient typist's deterministic clock
    /// rather than a wall clock (§15 read).
    fn has_timed_input(&self) -> bool {
        true
    }

    fn has_sounds(&self) -> bool {
        false
    }

    fn has_character_graphics(&self) -> bool {
        false
    }

    fn has_colours(&self) -> bool {
        false
    }

    fn has_mouse(&self) -> bool {
        false
    }

    /// The screen height in lines; 255 means "infinite", the right
    /// claim for an unpaged stream (§8.4).
    fn screen_lines(&self) -> u8 {
        255
    }

    /// The screen width in characters (§8.4).
    fn screen_columns(&self) -> u8 {
        80
    }
}

/// A dumb-terminal presentation: one unadorned stream of text to
/// standard output. Dropping a status is not a shortcut: it is the
/// conforming behaviour of an interpreter that declared the truth
/// about itself (§11.1).
pub struct PlainFrontend;

impl Frontend for PlainFrontend {
    fn write(&mut self, text: &str) {
        use std::io::Write;

        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        let _ = handle.write_all(text.as_bytes());
        let _ = handle.flush();
    }
}

/// A frontend that keeps everything it hears: the test suite's ear.
#[derive(Default)]
pub struct CaptureFrontend {
    pub output: String,
}

impl Frontend for CaptureFrontend {
    fn write(&mut self, text: &str) {
        self.output.push_str(text);
    }
}
