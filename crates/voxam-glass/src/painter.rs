//! The painted frontend: the screen model on a ratatui terminal.
//!
//! The model in voxam-core's screen module decides everything --
//! where each character lands, what it wears, what scrolled -- and
//! this painter's whole job is to render that grid onto the glass,
//! then park the terminal cursor where the model says the game's
//! cursor stands. The division keeps the painter too thin to hide
//! bugs: anything worth testing lives in the model, and the
//! golden-grid suite already holds it to §8.
//!
//! Where the reference repaints the rows the model reports
//! damaged, this rewrite in kind renders the whole grid into
//! ratatui's buffer on every repaint and lets the library's own
//! double-buffer diff find the changed cells -- the same minimal
//! writes, the buffer's way around. The terminal arrives through
//! ratatui's Backend seam, so the batteries drive the painter
//! against a TestBackend grid and no terminal at all.
//!
//! Input runs the other way from the reference too: the Rust
//! machine always suspends its reads, so the blocking read_line
//! and read_key here are called by the CLI's serving loop between
//! runs rather than by the machine mid-instruction -- the same
//! keystrokes, the suspension departure's way around.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::Position;
use ratatui::style::{Color, Modifier, Style};

use voxam_core::editor::{EXPIRED, LineEditor, read_line_edited};
use voxam_core::errors::VoxamError;
use voxam_core::frontend::{Frontend, GRAPHICS_FONT, Status};
use voxam_core::png::Picture;
use voxam_core::screen::{BOLD, Cell, ITALIC, REVERSE, ScreenModel};

use crate::keys::KeySource;

/// The pause prompt a screenful of unread text waits behind
/// (§8.8.3.2.6's courtesy, offered on every painted screen).
pub const MORE_PROMPT: &str = "[MORE]";

/// A screen without an answer for its size -- a pipe, a dumb TTY
/// -- paints as the classic 80 by 24 glass.
pub const FALLBACK_COLUMNS: usize = 80;
pub const FALLBACK_LINES: usize = 24;

/// How often an infinite wait surfaces for air. Each heartbeat
/// lets the machine attend to background work -- an ended sound's
/// §9.4.4 routine -- while the player thinks at a prompt; between
/// heartbeats the wait costs nothing.
pub const IDLE_HEARTBEAT: Duration = Duration::from_millis(200);

/// The §8.3.1 colour codes the painter can mix, as the terminal's
/// own eight colours -- the blessed names, spoken in ratatui.
/// Code 1 is the terminal's own default and needs no colour at
/// all, and None leaves the cell in that default.
fn colour(code: i32) -> Option<Color> {
    match code {
        2 => Some(Color::Black),
        3 => Some(Color::Red),
        4 => Some(Color::Green),
        5 => Some(Color::Yellow),
        6 => Some(Color::Blue),
        7 => Some(Color::Magenta),
        8 => Some(Color::Cyan),
        // blessed's "white" is the classic dim SGR 37, which
        // ratatui spells Gray; Color::White would be the bright
        // variant no reference session shows.
        9 => Some(Color::Gray),
        _ => None,
    }
}

/// The §16 character graphics font, one Unicode stand-in per 8x8
/// bitmap. Cells in font 3 hold the character code the game
/// printed; painting translates each to the nearest shape a
/// terminal font already has. The families, reading down the
/// spec's table: arrows, diagonals, single box-drawing lines with
/// every join, the map's solid blocks and their diagonal
/// transitions, cell-edge strokes, Beyond Zork's stat gauge as
/// eighth-blocks, and the late Anglian ("futhorc") runes the §16
/// remarks decode for a-z. Lossy cells are rounded toward
/// whatever keeps the drawn map connected, a call the reference's
/// eyeball tests settled: a solid mass meeting a diagonal road
/// keeps its mass (a quadrant, not a triangle that bites the room
/// corner), and the single-pixel road tips continue their
/// diagonal rather than leaving a gap where the road reaches the
/// room. A character beyond the table passes through as itself.
fn font_3(character: char) -> char {
    match character {
        ' ' => ' ',  // 32: blank
        '!' => '←',  // 33: left arrow
        '"' => '→',  // 34: right arrow
        '#' => '╱',  // 35: diagonal, rising
        '$' => '╲',  // 36: diagonal, falling
        '%' => ' ',  // 37: blank
        '&' => '─',  // 38: horizontal line, low
        '\'' => '─', // 39: horizontal line, high
        '(' => '│',  // 40: vertical line, right of centre
        ')' => '│',  // 41: vertical line, left of centre
        '*' => '┴',  // 42: line up, joined to a horizontal
        '+' => '┬',  // 43: line down, joined to a horizontal
        ',' => '├',  // 44: vertical joined rightward
        '-' => '┤',  // 45: vertical joined leftward
        '.' => '└',  // 46: corner, up and right
        '/' => '┌',  // 47: corner, down and right
        '0' => '┐',  // 48: corner, down and left
        '1' => '┘',  // 49: corner, up and left
        '2' => '└',  // 50: up-right corner, diagonal tail dropped
        '3' => '┌',  // 51: down-right corner, diagonal tail dropped
        '4' => '┐',  // 52: down-left corner, diagonal tail dropped
        '5' => '┘',  // 53: up-left corner, diagonal tail dropped
        '6' => '█',  // 54: solid block
        '7' => '▀',  // 55: block, upper five-eighths
        '8' => '▄',  // 56: block, lower five-eighths
        '9' => '▌',  // 57: block, left five-eighths
        ':' => '▐',  // 58: block, right five-eighths
        ';' => '▄',  // 59: lower block, line up dropped
        '<' => '▀',  // 60: upper block, line down dropped
        '=' => '▌',  // 61: left block, line right dropped
        '>' => '▐',  // 62: right block, line left dropped
        '?' => '▝',  // 63: quadrant, upper right
        '@' => '▗',  // 64: quadrant, lower right
        'A' => '▖',  // 65: quadrant, lower left
        'B' => '▘',  // 66: quadrant, upper left
        'C' => '▝',  // 67: upper-right mass meeting a diagonal
        'D' => '▗',  // 68: lower-right mass meeting a diagonal
        'E' => '▖',  // 69: lower-left mass meeting a diagonal
        'F' => '▘',  // 70: upper-left mass meeting a diagonal
        'G' => '╱',  // 71: top-right road tip
        'H' => '╲',  // 72: bottom-right road tip
        'I' => '╱',  // 73: bottom-left road tip
        'J' => '╲',  // 74: top-left road tip
        'K' => '▔',  // 75: top edge stroke
        'L' => '▁',  // 76: bottom edge stroke
        'M' => '▏',  // 77: left edge stroke
        'N' => '▕',  // 78: right edge stroke
        'O' => '═',  // 79: gauge rails, empty
        'P' => '▏',  // 80: gauge, one eighth full
        'Q' => '▎',  // 81: gauge, two eighths
        'R' => '▍',  // 82: gauge, three eighths
        'S' => '▌',  // 83: gauge, four eighths
        'T' => '▋',  // 84: gauge, five eighths
        'U' => '▊',  // 85: gauge, six eighths
        'V' => '▉',  // 86: gauge, seven eighths
        'W' => '█',  // 87: gauge, full
        'X' => '▕',  // 88: gauge, right rim
        'Y' => '▏',  // 89: gauge, left rim
        'Z' => '╳',  // 90: diagonal cross
        '[' => '┼',  // 91: four-way join
        '\\' => '↑', // 92: up arrow
        ']' => '↓',  // 93: down arrow
        '^' => '↕',  // 94: up-down arrow
        '_' => '□',  // 95: outlined box
        '`' => '?',  // 96: a drawn question mark
        'a' => 'ᚪ',  // 97: rune ac
        'b' => 'ᛒ',  // 98: rune beorc
        'c' => 'ᛇ',  // 99: rune eoh, the eo of the §16 remarks
        'd' => 'ᛞ',  // 100: rune daeg
        'e' => 'ᛖ',  // 101: rune eh
        'f' => 'ᚠ',  // 102: rune feoh
        'g' => 'ᚷ',  // 103: rune gyfu
        'h' => 'ᚻ',  // 104: rune haegl
        'i' => 'ᛁ',  // 105: rune is
        'j' => 'ᛄ',  // 106: rune ger
        'k' => 'ᛣ',  // 107: rune calc, the "other k"
        'l' => 'ᛚ',  // 108: rune lagu
        'm' => 'ᛗ',  // 109: rune man
        'n' => 'ᚾ',  // 110: rune nyd
        'o' => 'ᚩ',  // 111: rune os
        'p' => 'ᛈ',  // 112: rune peorth
        'q' => 'ᚳ',  // 113: rune cen, the Anglian k
        'r' => 'ᚱ',  // 114: rune rad
        's' => 'ᛋ',  // 115: rune sigel
        't' => 'ᛏ',  // 116: rune tir
        'u' => 'ᚢ',  // 117: rune ur
        'v' => 'ᛠ',  // 118: rune ear
        'w' => 'ᚹ',  // 119: rune wynn
        'x' => 'ᛉ',  // 120: rune eolh, standing in for z
        'y' => 'ᚣ',  // 121: rune yr
        'z' => 'ᛟ',  // 122: rune ethel, standing in for oe
        other => other,
    }
}

/// Codes 123 to 126 are the reverse-video twins of the up arrow,
/// the down arrow, the double arrow, and the question mark -- the
/// §16 bitmaps invert them pixel for pixel. Beyond Zork highlights
/// its scrolling markers with them, so the painter draws the same
/// shape and flips reverse video instead.
fn font_3_reversed(character: char) -> Option<char> {
    match character {
        '{' => Some('↑'), // 123: up arrow, reversed
        '|' => Some('↓'), // 124: down arrow, reversed
        '}' => Some('↕'), // 125: up-down arrow, reversed
        '~' => Some('?'), // 126: question mark, reversed
        _ => None,
    }
}

/// The character and style one cell paints as (§16).
///
/// Cells in the character graphics font translate to their
/// Unicode stand-ins; the four reverse-video shapes flip reverse
/// instead of carrying it in the glyph. Every other font paints
/// its characters as they are.
fn appearance(cell: &Cell) -> (char, u16) {
    if cell.font != GRAPHICS_FONT {
        let mut style = cell.style;

        // A space has no glyph to embolden, but a terminal
        // brightens a bold-reverse blank anyway -- so a status
        // bar padded with bold spaces (Border Zone's) paints as
        // a patchwork of greys. Blanks shed bold before
        // painting, as the reference interpreters render them.
        if cell.character == ' ' {
            style &= !BOLD;
        }

        return (cell.character, style);
    }

    if let Some(shape) = font_3_reversed(cell.character) {
        return (shape, cell.style ^ REVERSE);
    }

    (font_3(cell.character), cell.style)
}

/// The dress one cell paints in: its translated glyph and the
/// ratatui style its §8.7.1 bits and §8.3.1 codes spell.
fn dressed(cell: &Cell) -> (char, Style) {
    let (character, style) = appearance(cell);
    let mut dress = Style::default();

    if style & REVERSE != 0 {
        dress = dress.add_modifier(Modifier::REVERSED);
    }

    if style & BOLD != 0 {
        dress = dress.add_modifier(Modifier::BOLD);
    }

    if style & ITALIC != 0 {
        dress = dress.add_modifier(Modifier::ITALIC);
    }

    if let Some(ink) = colour(cell.foreground) {
        dress = dress.fg(ink);
    }

    if let Some(paper) = colour(cell.background) {
        dress = dress.bg(paper);
    }

    (character, dress)
}

/// The terminal half of the painted frontend: the ratatui glass,
/// the keystroke intake, and the between-keystrokes attention.
///
/// Split from the frontend so the model's [MORE] callback can
/// reach the glass while the model itself is lent out mid-write
/// -- the reference's bound method, the borrow's way around.
pub struct Glass<B: Backend> {
    terminal: Terminal<B>,
    keys: Box<dyn KeySource>,
    bell: Box<dyn FnMut()>,
    /// The machine's between-keystrokes attention, wired by the
    /// session once a machine exists: called on each heartbeat of
    /// an infinite wait. None waits the old blocking way.
    pub idle: Option<Box<dyn FnMut()>>,
    /// The glass geometry the model was cut to.
    pub columns: usize,
    pub lines: usize,
    /// The first terminal mishap, kept for the session to
    /// surface: a painted call cannot return what its trait did
    /// not promise.
    pub fault: Option<B::Error>,
}

impl<B: Backend> Glass<B> {
    /// Wrap a terminal, taking its measure; a terminal without an
    /// answer for its size paints as the classic 80 by 24 (§8.4).
    pub fn new(terminal: Terminal<B>, keys: Box<dyn KeySource>, bell: Box<dyn FnMut()>) -> Self {
        let size = terminal.size().ok();
        let columns = size.map_or(0, |size| usize::from(size.width));
        let lines = size.map_or(0, |size| usize::from(size.height));

        Self {
            terminal,
            keys,
            bell,
            idle: None,
            columns: if columns == 0 {
                FALLBACK_COLUMNS
            } else {
                columns
            },
            lines: if lines == 0 { FALLBACK_LINES } else { lines },
            fault: None,
        }
    }

    /// The terminal itself -- the batteries read their grids here.
    pub fn terminal(&self) -> &Terminal<B> {
        &self.terminal
    }

    /// The terminal, mutably -- the session's doorway for the raw
    /// mode dance around it.
    pub fn terminal_mut(&mut self) -> &mut Terminal<B> {
        &mut self.terminal
    }

    /// Keep the first terminal mishap; the session surfaces it.
    fn noted(&mut self, result: Result<(), B::Error>) {
        if let Err(error) = result
            && self.fault.is_none()
        {
            self.fault = Some(error);
        }
    }

    /// Render the model's whole grid, an optional overlay at the
    /// cursor, and park the terminal cursor where the model's
    /// stands -- ratatui's buffer diff finds the changed cells.
    fn drawn(&mut self, model: &mut ScreenModel, overlay: Option<&str>) -> Result<(), B::Error> {
        // Drain the model's damage ledger: the buffer diff has
        // taken over its job, and the sweep keeps the ledger from
        // holding rows no painter will ask about.
        model.sweep();

        let lines = model.lines();
        let columns = model.columns();

        self.terminal.draw(|frame| {
            let (row, column) = model.cursor();

            for line in 1..=lines {
                for place in 1..=columns {
                    let cell = model.cell(line, place);
                    let (character, style) = dressed(&cell);

                    if let Some(painted) = frame
                        .buffer_mut()
                        .cell_mut(Position::new((place - 1) as u16, (line - 1) as u16))
                    {
                        painted.set_char(character);
                        painted.set_style(style);
                    }
                }
            }

            if let Some(prompt) = overlay {
                // The prompt is a reverse-video overlay at the
                // cursor, clamped to fit the row.
                let start = column.min((columns.saturating_sub(prompt.chars().count()) + 1).max(1));

                for (offset, character) in prompt.chars().enumerate() {
                    if let Some(painted) = frame
                        .buffer_mut()
                        .cell_mut(Position::new((start - 1 + offset) as u16, (row - 1) as u16))
                    {
                        painted.set_char(character);
                        painted.set_style(Style::default().add_modifier(Modifier::REVERSED));
                    }
                }
            }

            frame.set_cursor_position(Position::new((column - 1) as u16, (row - 1) as u16));
        })?;

        Ok(())
    }

    /// Redraw the glass from the model, noting any mishap.
    pub fn paint(&mut self, model: &mut ScreenModel) {
        let painted = self.drawn(model, None);

        self.noted(painted);
    }

    /// Paint the model's every row over a cleared glass.
    ///
    /// At the start of a session the model is blank, so this
    /// wipes whatever the shell left on the terminal: the story
    /// begins on a clean screen instead of shingling its rows
    /// between old output. The cover flow uses it on both sides
    /// of the picture for the same reason.
    pub fn wipe(&mut self, model: &mut ScreenModel) {
        let cleared = self.terminal.clear();

        self.noted(cleared);
        self.paint(model);
    }

    /// Put the terminal cursor where the model's cursor stands.
    pub fn park(&mut self, model: &mut ScreenModel) {
        let (row, column) = model.cursor();
        let parked = self
            .terminal
            .set_cursor_position(Position::new((column - 1) as u16, (row - 1) as u16));

        self.noted(parked);

        let shown = self.terminal.show_cursor();

        self.noted(shown);
    }

    /// Ring the terminal bell: one bell serves both bleeps (§9).
    pub fn bleep(&mut self) {
        (self.bell)();
    }

    /// One keystroke read, already translated; None for nothing
    /// usable -- an expired timeout, an unmapped key.
    pub fn translated_key(&mut self, timeout: Option<Duration>) -> Option<char> {
        self.keys.key(timeout)
    }

    /// One read of an infinite wait, attentive while it lasts.
    ///
    /// Without an idle callback this is a plain blocking read.
    /// With one, the wait is chopped into heartbeats: each expiry
    /// lets the machine attend to background work -- an ended
    /// sound's routine (§9.4.4) -- before listening again. One
    /// heartbeat's answer comes back as it is; None still means
    /// "nothing usable yet", and every caller already waits that
    /// out.
    pub fn waited_key(&mut self) -> Option<char> {
        if self.idle.is_none() {
            return self.keys.key(None);
        }

        let key = self.keys.key(Some(IDLE_HEARTBEAT));

        if key.is_none()
            && let Some(idle) = self.idle.as_mut()
        {
            idle();
        }

        key
    }

    /// Hold a screenful behind [MORE] until any key arrives.
    ///
    /// The model calls this mid-write, so the damage it has piled
    /// up paints first -- the player must see what they are being
    /// asked to read. The prompt is a reverse-video overlay at the
    /// cursor; the keypress is spent on the pause, and repainting
    /// the grid from the model erases the prompt without a trace.
    /// A heartbeat expiry answers None so background work can
    /// run; only a real key ends the pause.
    pub fn pause(&mut self, model: &mut ScreenModel) {
        let held = self.drawn(model, Some(MORE_PROMPT));

        self.noted(held);

        while self.waited_key().is_none() {}

        let cleared = self.drawn(model, None);

        self.noted(cleared);
    }

    /// Show a cover picture's half-block cells over a cleared
    /// glass: each ▀ carries two pixels, its foreground the upper
    /// and its background the lower, so any terminal with exact
    /// colours can show a picture at twice its row resolution. An
    /// odd bottom row grounds on black.
    fn cover(&mut self, pixels: &[Vec<(u8, u8, u8)>], left: usize, top: usize) {
        let cleared = self.terminal.clear();

        self.noted(cleared);

        let drawn = self
            .terminal
            .draw(|frame| {
                for index in (0..pixels.len()).step_by(2) {
                    let upper = &pixels[index];
                    let lower = pixels.get(index + 1);

                    for (column, &(red, green, blue)) in upper.iter().enumerate() {
                        let below = lower.map_or((0, 0, 0), |row| row[column]);

                        if let Some(painted) = frame.buffer_mut().cell_mut(Position::new(
                            (left + column) as u16,
                            (top + index / 2) as u16,
                        )) {
                            painted.set_char('▀');
                            painted.set_style(
                                Style::default()
                                    .fg(Color::Rgb(red, green, blue))
                                    .bg(Color::Rgb(below.0, below.1, below.2)),
                            );
                        }
                    }
                }
            })
            .map(|_| ());

        self.noted(drawn);
    }
}

/// Scale a picture onto a pixel canvas, averaging boxes.
///
/// The canvas is the glass in half-block pixels: the screen's
/// columns wide, twice its lines tall. A picture larger than the
/// canvas shrinks to fit, keeping its shape; a smaller one stays
/// its own size.
fn fitted(picture: &Picture, columns: usize, rows: usize) -> Vec<Vec<(u8, u8, u8)>> {
    let picture_width = picture.width as usize;
    let picture_height = picture.height as usize;
    let scale = (columns as f64 / picture_width as f64)
        .min(rows as f64 / picture_height as f64)
        .min(1.0);
    let width = ((picture_width as f64 * scale) as usize).max(1);
    let height = ((picture_height as f64 * scale) as usize).max(1);
    let mut shrunk = Vec::with_capacity(height);

    for target_row in 0..height {
        let row_first = target_row * picture_height / height;
        let row_last = ((target_row + 1) * picture_height / height).max(row_first + 1);
        let mut row = Vec::with_capacity(width);

        for target_column in 0..width {
            let first = target_column * picture_width / width;
            let last = ((target_column + 1) * picture_width / width).max(first + 1);
            let mut red: u32 = 0;
            let mut green: u32 = 0;
            let mut blue: u32 = 0;
            let mut count: u32 = 0;

            for source_row in row_first..row_last {
                for source_column in first..last {
                    let pixel = picture.rows[source_row][source_column];

                    red += u32::from(pixel.0);
                    green += u32::from(pixel.1);
                    blue += u32::from(pixel.2);
                    count += 1;
                }
            }

            row.push((
                (red / count) as u8,
                (green / count) as u8,
                (blue / count) as u8,
            ));
        }

        shrunk.push(row);
    }

    shrunk
}

/// A frontend that keeps a screen model and paints it live.
///
/// Every Frontend operation updates the model first; the whole
/// grid is then redrawn and the buffer diff repaints what
/// changed. The capability flags tell the header the truth this
/// frontend makes true: a status line, a splittable screen, and
/// the §8.7.1 styles. The machine owns one handle as its
/// Frontend; the serving loop holds the other and drives the
/// input seams -- the wire face's shared-cell departure, worn by
/// the glass.
pub struct ScreenFrontend<B: Backend> {
    pub glass: Rc<RefCell<Glass<B>>>,
    pub model: ScreenModel,
    editor: LineEditor,
    /// Whether a timed read left a half-typed line composed: the
    /// next line read resumes it instead of starting fresh.
    composing: bool,
    prompt: String,
    /// How many prints have landed -- the serving loop compares
    /// across a timed read's interrupt to honour §15's redisplay
    /// remark, as the reference's machine counts its own.
    pub prints: usize,
    /// The first model refusal, kept for the session to surface.
    pub fault: Option<VoxamError>,
}

impl<B: Backend + 'static> ScreenFrontend<B> {
    /// Wrap a glass around a fresh screen model, cut to the
    /// glass's own measure, with the [MORE] pause wired through
    /// the model's callback seam.
    pub fn new(version: u8, glass: Rc<RefCell<Glass<B>>>) -> Self {
        let (columns, lines) = {
            let glass = glass.borrow();

            (glass.columns, glass.lines)
        };
        let mut model = ScreenModel::new(columns, lines, version);
        let handle = Rc::clone(&glass);

        model.more = Some(Box::new(move |model| {
            handle.borrow_mut().pause(model);
        }));

        Self {
            glass,
            model,
            editor: LineEditor::new(),
            composing: false,
            prompt: String::new(),
            prints: 0,
            fault: None,
        }
    }

    /// Keep the first model refusal; the session surfaces it.
    fn noted(&mut self, result: Result<(), VoxamError>) {
        if let Err(error) = result
            && self.fault.is_none()
        {
            self.fault = Some(error);
        }
    }

    /// Redraw the glass from the model.
    fn repaint(&mut self) {
        self.glass.borrow_mut().paint(&mut self.model);
    }

    /// Paint the model's every row over a cleared glass.
    pub fn clear(&mut self) {
        self.glass.borrow_mut().wipe(&mut self.model);
    }

    /// Remember the prompt: the line's text left of the cursor.
    pub fn begin_input(&mut self) {
        let (row, column) = self.model.cursor();
        let text = self.model.row_text(row);

        self.prompt = text.chars().take(column - 1).collect();
    }

    /// Show the prompt again after a printing interrupt.
    ///
    /// §15's remark: the interpreter should redisplay the input
    /// line when a timed read's interrupt has printed -- the
    /// prompt rewrites at wherever the interrupt's output left
    /// the cursor.
    pub fn resume_input(&mut self) {
        let prompt = self.prompt.clone();

        self.model.write(&prompt);
        self.repaint();
    }

    /// Read one raw keystroke at the model's cursor.
    ///
    /// Keystrokes are not echoed -- §15 read_char leaves any
    /// echoing to the game. Without a timeout, empty and
    /// unhearable reads simply wait for a real keystroke; with
    /// one, an expired wait answers None, which is the machine's
    /// cue to fire a §15 interrupt on the wall clock -- so a
    /// timed read keeps its own clock, and only the infinite wait
    /// is chopped into attentive heartbeats.
    pub fn read_key(&mut self, timeout: Option<Duration>) -> Option<char> {
        self.model.rest();
        self.glass.borrow_mut().park(&mut self.model);

        loop {
            let key = {
                let mut glass = self.glass.borrow_mut();

                match timeout {
                    None => glass.waited_key(),
                    Some(patience) => glass.translated_key(Some(patience)),
                }
            };

            if key.is_some() {
                return key;
            }

            if timeout.is_some() {
                return None;
            }
        }
    }

    /// Read one line of raw typing, edited and echoed via the
    /// model.
    ///
    /// The terminal's own echo is never invited: keystrokes
    /// arrive raw through the same seam read_key uses, and every
    /// visible change to the glass is the painter's doing -- so a
    /// prompt on the bottom row can never make the real terminal
    /// scroll the screen behind the model's back. The line editor
    /// gives the classic vocabulary: backspace rubs out, the left
    /// and right cursor keys move within the line, and up and
    /// down walk the session's command history.
    pub fn read_line(&mut self) -> String {
        self.model.rest();
        self.glass.borrow_mut().park(&mut self.model);

        let fresh = !self.composing;

        self.composing = false;

        let glass = &self.glass;
        let mut keys = || glass.borrow_mut().waited_key();
        let mut repaint = |model: &mut ScreenModel| glass.borrow_mut().paint(model);
        let line = read_line_edited(
            &mut self.editor,
            &mut self.model,
            &mut keys,
            &mut repaint,
            fresh,
        );

        // The untimed key source never expires, so the line is
        // real.
        line.expect("an untimed line read cannot expire")
    }

    /// Read a line on the clock, or None when the wait expires.
    ///
    /// The live half of a §15 timed read: the serving loop calls
    /// with the read's interval, runs the game's interrupt on
    /// None, and calls again -- and the half-typed line survives
    /// between calls, composed in the editor and standing on the
    /// glass. Border Zone's whole real-time engine rides on these
    /// ticks.
    pub fn read_line_until(&mut self, interval: Duration) -> Option<String> {
        self.model.rest();
        self.glass.borrow_mut().park(&mut self.model);

        let deadline = Instant::now() + interval;
        let glass = &self.glass;
        let mut keys = move || {
            let remaining = deadline.saturating_duration_since(Instant::now());

            if remaining.is_zero() {
                return Some(EXPIRED);
            }

            let mut glass = glass.borrow_mut();
            let wait = if glass.idle.is_some() {
                remaining.min(IDLE_HEARTBEAT)
            } else {
                remaining
            };
            let key = glass.translated_key(Some(wait));

            if key.is_none()
                && let Some(idle) = glass.idle.as_mut()
            {
                idle();
            }

            key
        };
        let mut repaint = |model: &mut ScreenModel| glass.borrow_mut().paint(model);
        let fresh = !self.composing;
        let line = read_line_edited(
            &mut self.editor,
            &mut self.model,
            &mut keys,
            &mut repaint,
            fresh,
        );

        self.composing = line.is_none();

        line
    }

    /// Erase the half-typed line a terminated timed read leaves.
    ///
    /// §15 read: a true-returning interrupt ends the read with
    /// all input erased -- so the composed line comes off the
    /// glass, rubbed out from the editor's own accounting, and
    /// the next read starts fresh.
    pub fn abandon_input(&mut self) {
        if !self.composing {
            return;
        }

        let pending = self.editor.text().chars().count();

        self.model.retreat(self.editor.cursor());
        self.model.write(&" ".repeat(pending));
        self.model.retreat(pending);
        self.editor.begin();
        self.composing = false;
        self.repaint();
    }

    /// No clicks arrive until the painter learns mouse reporting.
    pub fn click_position(&mut self) -> Option<(u16, u16)> {
        None
    }

    /// Show a cover picture until a key is pressed, then clear.
    ///
    /// The picture is scaled to fit the glass and painted centred
    /// in half-block cells. Infocom's own interpreters opened
    /// this way -- cover art, a keypress, and the story.
    /// Afterwards the blank model is repainted whole, leaving the
    /// game a clean screen no splash pixel survives on. (The
    /// reference's sixel road -- real pixels behind a --pixels
    /// ask and a terminal interrogation -- is deferred with the
    /// sixel module itself.)
    pub fn show_frontispiece(&mut self, picture: &Picture) {
        let (columns, lines) = {
            let glass = self.glass.borrow();

            (glass.columns, glass.lines)
        };
        let pixels = fitted(picture, columns, lines * 2);
        let left = (columns - pixels[0].len()) / 2;
        let top = (lines - pixels.len().div_ceil(2)) / 2;

        self.glass.borrow_mut().cover(&pixels, left, top);
        self.read_key(None);
        self.clear();
    }
}

/// The machine's handle on the painted frontend: the wire face's
/// shared-cell departure, worn by the glass. Every operation
/// updates the model first, then repaints.
pub struct PaintedHalf<B: Backend>(pub Rc<RefCell<ScreenFrontend<B>>>);

impl<B: Backend + 'static> Frontend for PaintedHalf<B> {
    /// Print story text through the model, then repaint.
    fn write(&mut self, text: &str) {
        let mut face = self.0.borrow_mut();

        face.prints += 1;
        face.model.write(text);
        face.repaint();
    }

    /// Print a §15 rectangle through the model, then repaint.
    fn write_rectangle(&mut self, rows: &[String]) {
        let mut face = self.0.borrow_mut();

        face.prints += 1;
        face.model.write_rectangle(rows);
        face.repaint();
    }

    /// Draw the Version 3 status line (§8.2).
    fn show_status(&mut self, status: &Status) {
        let mut face = self.0.borrow_mut();
        let shown = face.model.show_status(status);

        face.noted(shown);
        face.repaint();
    }

    /// Change the style for text that follows (§8.7.1).
    fn set_style(&mut self, style: u16) {
        self.0.borrow_mut().model.set_style(style);
    }

    /// Change the font for text that follows (§8.1.2).
    fn set_font(&mut self, font: u16) {
        self.0.borrow_mut().model.set_font(font);
    }

    /// Change the colours for text that follows (§8.3.1).
    fn set_colour(&mut self, foreground: i32, background: i32) {
        self.0.borrow_mut().model.set_colour(foreground, background);
    }

    /// Erase a window to background (§8.7.3.2).
    fn erase_window(&mut self, window: i32) {
        let mut face = self.0.borrow_mut();
        let erased = face.model.erase_window(window);

        face.noted(erased);
        face.repaint();
    }

    /// Erase from the cursor to the end of the line (§8.7.3.4).
    ///
    /// A pixel width never arrives here: only a Version 6 game
    /// sends one, and a Version 6 game plays on the stage.
    fn erase_line(&mut self, _pixels: Option<i32>) {
        let mut face = self.0.borrow_mut();

        face.model.erase_line();
        face.repaint();
    }

    /// Resize the upper window (§8.7.2.1).
    fn split_window(&mut self, lines: u16) {
        let mut face = self.0.borrow_mut();
        let split = face.model.split_window(i32::from(lines));

        face.noted(split);
        face.repaint();
    }

    /// Select the window taking the next printing (§8.7.2).
    fn set_window(&mut self, window: u16) {
        let mut face = self.0.borrow_mut();
        let selected = face.model.set_window(window);

        face.noted(selected);
        face.repaint();
    }

    /// Move the upper window's cursor (§8.7.2.3.1).
    fn set_cursor(&mut self, line: u16, column: u16) {
        let mut face = self.0.borrow_mut();
        let moved = face.model.set_cursor(line, column);

        face.noted(moved);
        face.repaint();
    }

    /// The model's own answer for get_cursor (§8.7.2.3.2).
    fn cursor_position(&self) -> (u16, u16) {
        let (row, column) = self.0.borrow_mut().model.get_cursor();

        (row as u16, column as u16)
    }

    /// Turn lower-window word wrapping on or off (§15
    /// buffer_mode).
    fn set_buffering(&mut self, buffered: bool) {
        self.0.borrow_mut().model.set_buffering(buffered);
    }

    /// Ring the terminal bell: one bell serves both bleeps (§9).
    fn bleep(&mut self, _high: bool) {
        self.0.borrow_mut().glass.borrow_mut().bleep();
    }

    // The capability flags tell the header the truth this
    // frontend makes true (§11.1).

    fn has_status_line(&self) -> bool {
        true
    }

    fn has_screen_splitting(&self) -> bool {
        true
    }

    fn has_bold(&self) -> bool {
        true
    }

    fn has_italic(&self) -> bool {
        true
    }

    fn has_fixed_pitch(&self) -> bool {
        true
    }

    fn has_timed_input(&self) -> bool {
        true
    }

    /// The §16 font paints as Unicode stand-ins -- box drawing,
    /// blocks, arrows, runes -- so character graphics are
    /// honestly on offer here (§8.1.5.1).
    fn has_character_graphics(&self) -> bool {
        true
    }

    /// The §8.3.1 codes 2 to 9 paint as the terminal's own eight
    /// colours, so the claim is honest (§8.3.3).
    fn has_colours(&self) -> bool {
        true
    }

    /// Terminal mouse reporting is a protocol of its own; until
    /// the painter learns it, the request bit clears honestly
    /// (§10.3.1.1).
    fn has_mouse(&self) -> bool {
        false
    }

    fn screen_lines(&self) -> u8 {
        self.0.borrow().glass.borrow().lines.min(255) as u8
    }

    fn screen_columns(&self) -> u8 {
        self.0.borrow().glass.borrow().columns.min(255) as u8
    }

    // No in-play pictures: the half-block cover is a doorway
    // courtesy, and the header's cleared bit keeps the claim
    // honest (§11.1.4). No arc band and no stage either: Version
    // 6 windows keep the two-window mimicry a cell terminal can
    // honour, and a terminal cell is the unit whatever its real
    // pixel size (§8.4.2) -- the trait's defaults already say
    // all of that, and the sound seams stay inert until a
    // speaker exists to make the claim true (§9.1.2).
}

#[cfg(test)]
#[path = "painter_tests.rs"]
mod tests;
