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

/// The §8.1.2 fonts, as set_font speaks them: zero asks for the
/// current font, and the named ones follow.
pub const CURRENT_FONT: u16 = 0;
pub const NORMAL_FONT: u16 = 1;
pub const PICTURE_FONT: u16 = 2;
pub const GRAPHICS_FONT: u16 = 3;
pub const COURIER_FONT: u16 = 4;

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

    /// Whether input arrives from outside rather than from a read
    /// call. A blocking frontend is asked and answers on the spot;
    /// a suspending one is never asked -- a read stands down, the
    /// machine returns to its host, and the host delivers the line
    /// or keystroke. The same contract the Glk displays keep.
    fn suspends(&self) -> bool {
        false
    }

    /// Whether pictures can actually be drawn (§11.1.4) -- true
    /// only where a gallery of art hangs behind a glass with
    /// pixels.
    fn has_pictures(&self) -> bool {
        false
    }

    /// Whether the arc_image picture band can be hung above the
    /// screen (arc_image: the contract) -- true only where the
    /// band's pictures can be found and shown. Claimed as Flags
    /// 1's picture bit in Versions 5, 7, and 8.
    fn has_arc_images(&self) -> bool {
        false
    }

    /// The header's row claim under the arc band: how many text
    /// rows stand below whatever hangs, or None where no band can.
    ///
    /// The reference's wire face writes the header itself when the
    /// band changes; a Rust face cannot reach memory mid-call, so
    /// the machine asks after every arc op and re-declares -- the
    /// same bytes, the seam's way around the borrow (arc_image:
    /// the contract, part A).
    fn arc_rows_below(&self) -> Option<i64> {
        None
    }

    /// Whether a Version 6 session plays on a §8.8 stage of eight
    /// placeable windows. When true, the machine forwards window
    /// geometry and cursor moves; when false, it keeps the
    /// character frontends' flowing mimicry -- the behaviour every
    /// recording replays in.
    fn has_stage(&self) -> bool {
        false
    }

    /// The width of one character cell in the units the header
    /// speaks -- 1 on a character glass, whose unit is a
    /// character (§8.4.2).
    fn font_width(&self) -> u16 {
        1
    }

    /// The height of one character cell in units.
    fn font_height(&self) -> u16 {
        1
    }

    /// Change the printing colours for text that follows (§8.3.1).
    /// Only frontends that claimed colours receive the change; the
    /// pair travels signed, so §8.3.1's colour -1 -- the pixel
    /// under the cursor -- arrives as itself.
    fn set_colour(&mut self, _foreground: i32, _background: i32) {}

    /// A picture's height and width in pixels, in picture_data's
    /// own order (§15); None for every number on a frontend that
    /// hangs no pictures.
    fn picture_data(&self, _number: u16) -> Option<(u16, u16)> {
        None
    }

    /// How many pictures hang, and the art's release number -- the
    /// picture_data number-0 census (§15).
    fn picture_census(&self) -> (u16, u16) {
        (0, 0)
    }

    /// Draw a picture, top left at a screen units position (§15).
    /// Only frontends that claimed pictures hear the call, with
    /// the cursor defaults and window origin already resolved.
    fn draw_picture(&mut self, _number: u16, _line: u16, _column: u16) {}

    /// Paint a picture's region to the background colour (§15).
    fn erase_picture(&mut self, _number: u16, _line: u16, _column: u16) {}

    /// Hang, replace, or clear the arc_image band: EXT:0x80's two
    /// operands, passed whole -- the picture id, zero taking the
    /// band down, and the mode naming its height in text rows
    /// (arc_image: the contract). Only frontends that claimed arc
    /// images hear the call.
    fn draw_arc_image(&mut self, _image: u16, _mode: u16) {}

    /// Place a §8.8 window at a position and size, in units. Only
    /// frontends that claimed a stage hear the call.
    fn place_window(&mut self, _window: u16, _line: u16, _column: u16, _height: u16, _width: u16) {}

    /// Scroll a §8.8 window's own rectangle, in units (§15):
    /// positive scrolls up, negative down. Only frontends that
    /// claimed a stage hear the call.
    fn scroll_window(&mut self, _window: u16, _pixels: i32) {}

    /// Set a §8.8 window's margin sizes, in units (§8.8.3.2.1).
    /// Only frontends that claimed a stage hear the call.
    fn set_margins(&mut self, _window: u16, _left: u16, _right: u16) {}

    /// Set a §8.8 window's [MORE] line count (§8.8.3.2.6): games
    /// manipulate it freely, and -999 means never print [MORE].
    /// Only frontends that claimed a stage hear the call.
    fn set_line_count(&mut self, _window: u16, _count: i32) {}

    /// Start a sampled sound in the background (§9.4). The volume
    /// runs 1 to 8 (§9.3); repeats count total plays, 0 repeating
    /// until stopped (§9.4.3), and None plays as the resource
    /// file's Loop chunk says -- the Version 3 case. Answers
    /// whether a sound actually started, which decides if an
    /// end-of-sound routine is worth keeping.
    fn play_sound(&mut self, _number: u16, _volume: u16, _repeats: Option<u16>) -> bool {
        false
    }

    /// Stop a sampled sound, or all of them when None (§9.4).
    fn stop_sound(&mut self, _number: Option<u16>) {}

    /// Whether a sampled sound is still sounding (§9 remarks).
    fn sound_playing(&self) -> bool {
        false
    }

    /// Whether a sound just ended of its own accord (§9.4.4): true
    /// once per natural ending, and never for a sound stopped or
    /// replaced.
    fn sound_finished(&mut self) -> bool {
        false
    }

    /// Block until the playing sound finishes a cycle -- the §9
    /// remarks' pacing rule for The Lurking Horror.
    fn wait_for_sound(&mut self) {}
}

/// The arc_image contract's fixed facts, shared by every face that
/// hangs the band: the two modes by their text-row names, the
/// reference width the masters are painted at, and the pixel rows
/// each mode row stands for (arc_image: the contract, part A).
pub const ARC_MODES: [u16; 2] = [9, 12];
pub const ARC_REFERENCE_WIDTH: u16 = 320;
pub const ARC_PIXEL_ROWS: u16 = 8;

/// The §8.3.1 colour codes as RGB, shared by every face that shows
/// real ink: 2 to 9 are the classic eight, and the greys at 10 to
/// 12 are the Version 6 additions, their values scaled from the
/// spec's own true-colour equivalents (§8.3.7). Codes 0 and 1 --
/// "no change" and "the interpreter's default" -- deliberately
/// answer None: the default is each face's own affair.
pub fn colour_value(code: i32) -> Option<(u8, u8, u8)> {
    match code {
        2 => Some((0, 0, 0)),
        3 => Some((204, 0, 0)),
        4 => Some((0, 204, 0)),
        5 => Some((204, 204, 0)),
        6 => Some((0, 0, 204)),
        7 => Some((204, 0, 204)),
        8 => Some((0, 204, 204)),
        9 => Some((255, 255, 255)),
        10 => Some((181, 181, 181)),
        11 => Some((139, 139, 139)),
        12 => Some((90, 90, 90)),
        _ => None,
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
