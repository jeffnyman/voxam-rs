//! Where a machine's text and status go (§7, §8).
//!
//! The trait mirrors the Python reference's Frontend protocol,
//! trimmed to what the plain stream needs; the richer faces --
//! painted terminal, window, wire -- implement it as they arrive.
//! Default answers are the plain stream's honest claims.

/// The two windows a character screen model renders (§8.7.2).
const LOWER_WINDOW: u16 = 0;
const UPPER_WINDOW: u16 = 1;

/// erase_window -1 unsplits and clears everything (§8.7.3.3).
const UNSPLIT_AND_CLEAR: i32 = -1;

/// An upper split of one or two lines is a status bar redrawn
/// every turn -- chrome; anything taller holds content.
const STATUS_CHROME_LINES: u16 = 2;

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

    /// Render the §15 rectangle; the plain default stacks the rows
    /// as ordinary lines through the same muting as any text.
    fn write_rectangle(&mut self, rows: &[String]) {
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                self.write("\n");
            }

            self.write(row);
        }
    }

    /// Redraw the status line (§8.2); only called when
    /// `has_status_line` is claimed.
    fn show_status(&mut self, _status: &Status) {}

    /// Sound a §9 bleep; the plain stream's is silence.
    fn bleep(&mut self, _high: bool) {}

    /// Take a §8.7 style request; a frontend renders the styles it
    /// claimed in the header and ignores the rest.
    fn set_style(&mut self, _style: u16) {}

    /// Take a §8.1.2 font change the machine already vetted.
    fn set_font(&mut self, _font: u16) {}

    /// Take the word-wrap buffering toggle (§8.7).
    fn set_buffering(&mut self, _buffered: bool) {}

    /// Remember the upper window's new height (§8.7.2).
    fn split_window(&mut self, _lines: u16) {}

    /// Select a window (§8.7.2).
    fn set_window(&mut self, _window: u16) {}

    /// Move the upper window's cursor (§8.7.2.3).
    fn set_cursor(&mut self, _line: u16, _column: u16) {}

    /// Where the upper window's pen stands (§8.7.2.3.2).
    fn cursor_position(&self) -> (u16, u16) {
        (1, 1)
    }

    /// Erase a window (§8.7.3); -1 unsplits and clears everything.
    fn erase_window(&mut self, _window: i32) {}

    /// Erase rightward from the cursor (§15 erase_line).
    fn erase_line(&mut self) {}

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

/// A dumb-terminal presentation: one unadorned stream of text.
///
/// Lower-window text always flows. Upper-window text flows only
/// when the split is tall enough to hold content -- a title card,
/// a quotation -- and is muted when it is a one- or two-line
/// status bar redrawn every turn. That distinction is what keeps a
/// transcript the story and nothing else, without losing the parts
/// of the story games put up top. Dropping the rest of the chrome
/// is not a shortcut: it is the conforming behaviour of an
/// interpreter that declared the truth about itself (§11.1).
pub struct StreamFrontend<S: FnMut(&str)> {
    sink: S,
    window: u16,
    split: u16,
    upper_row: u16,
    upper_column: u16,
}

impl<S: FnMut(&str)> StreamFrontend<S> {
    /// Bind the text stream the model writes through.
    pub fn new(sink: S) -> Self {
        Self {
            sink,
            window: LOWER_WINDOW,
            split: 0,
            upper_row: 1,
            upper_column: 1,
        }
    }

    /// Whether the upper window is tall enough to be content.
    fn upper_holds_content(&self) -> bool {
        self.split > STATUS_CHROME_LINES
    }
}

impl<S: FnMut(&str)> Frontend for StreamFrontend<S> {
    /// Pass story text through to the stream, muting chrome.
    fn write(&mut self, text: &str) {
        if self.window == LOWER_WINDOW {
            (self.sink)(text);
        } else if self.upper_holds_content() {
            (self.sink)(text);
            self.upper_column += text.chars().count() as u16;
        }
    }

    /// Remember the split height: it is the chrome-or-content tell.
    fn split_window(&mut self, lines: u16) {
        self.split = lines;
    }

    /// Remember the selection, which is what routes write().
    ///
    /// Leaving a content-bearing upper window ends its last line,
    /// so upper text and the story never share one.
    fn set_window(&mut self, window: u16) {
        if window == LOWER_WINDOW && self.window == UPPER_WINDOW && self.upper_holds_content() {
            (self.sink)("\n");
        }

        self.window = window;
        self.upper_row = 1;
        self.upper_column = 1;
    }

    /// Reconstruct content-window layout in stream form.
    ///
    /// A row change becomes a new-line and a column beyond the pen
    /// becomes padding, which is how a centered title card stays
    /// centered in a transcript. Cursor moves in a status bar are
    /// dropped with the rest of the chrome.
    fn set_cursor(&mut self, line: u16, column: u16) {
        if self.window != UPPER_WINDOW || !self.upper_holds_content() {
            return;
        }

        if line != self.upper_row {
            (self.sink)("\n");
            self.upper_row = line;
            self.upper_column = 1;
        }

        if column > self.upper_column {
            let padding = " ".repeat(usize::from(column - self.upper_column));
            (self.sink)(&padding);
            self.upper_column = column;
        }
    }

    /// The stream's upper-window bookkeeping, read back
    /// (§8.7.2.3.2): where the pen would be, to reconstruct layout.
    fn cursor_position(&self) -> (u16, u16) {
        (self.upper_row, self.upper_column)
    }

    /// Drop the erasure, honouring -1's side effect (§8.7): erasing
    /// window -1 also unsplits the screen and reselects the lower
    /// window -- and THAT matters here, or a game that clears its
    /// way out of the upper window would leave the stream muted
    /// forever.
    fn erase_window(&mut self, window: i32) {
        if window == UNSPLIT_AND_CLEAR {
            self.window = LOWER_WINDOW;
            self.split = 0;
        }
    }
}

/// The plain stream to standard output.
pub fn plain() -> StreamFrontend<impl FnMut(&str)> {
    StreamFrontend::new(|text: &str| {
        use std::io::Write;

        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        let _ = handle.write_all(text.as_bytes());
        let _ = handle.flush();
    })
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
