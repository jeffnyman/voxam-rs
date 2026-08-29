//! The screen model: §8's two windows as a pure grid of cells.
//!
//! This is the presentation half of the screen split into its
//! testable core: a two-dimensional grid of attributed characters
//! obeying every window, cursor, scrolling, and erasure rule of §8
//! -- and touching no terminal whatsoever. A painter diffs this
//! grid onto a real screen; tests read the grid directly and
//! assert on what a player would see. The model is deterministic
//! by construction, so a seeded replay draws the same grid,
//! forever.
//!
//! Rows and columns are 1-based, (1,1) at the top left, matching
//! the coordinates set_cursor speaks (§8.7.2.3.1). In Version 3
//! the top row belongs to the interpreter's own status line and
//! the upper window hangs below it (§8.6.1.1); from Version 4 the
//! upper window starts at the top of the screen (§8.7.2.1).

use crate::errors::VoxamError;
use crate::frontend::{NORMAL_FONT, Status};

pub const LOWER: u16 = 0;
pub const UPPER: u16 = 1;

// The §8.7.1 text styles, as set_text_style speaks them (§15): a
// bitmask where 0 is Roman and clears the rest.
pub const ROMAN: u16 = 0;
pub const REVERSE: u16 = 1;
pub const BOLD: u16 = 2;
pub const ITALIC: u16 = 4;
pub const FIXED_PITCH: u16 = 8;

// The §8.3.1 colour codes: 0 means "no change" and 1 is the
// interpreter's default, which the model keeps symbolic -- mapping
// it to actual ink belongs to the painter.
pub const CURRENT_COLOUR: i32 = 0;
pub const DEFAULT_COLOUR: i32 = 1;

// The status line belongs to versions 1 to 3 (§8.2); the windowed
// model beneath it belongs to Version 3 alone (§8.6), and versions
// 1 and 2 are teletypes with no windows at all (§8.5.1). From
// Version 4 the lower window's cursor holds the bottom line and
// erasure homes it there (§8.7.2.2, §8.7.3.2.1); from Version 5
// the cursor may sit on any line clear of the upper window and
// erasure homes to the top left (§8.7.3.3).
pub const STATUS_LAST_VERSION: u8 = 3;
pub const WINDOWS_FIRST_VERSION: u8 = 3;
pub const BOTTOM_HOME_LAST_VERSION: u8 = 4;

// Erase window's two whole-screen requests (§15 erase_window).
pub const ERASE_UNSPLIT: i32 = -1;
pub const ERASE_KEEP_SPLIT: i32 = -2;

fn screen_error(message: String) -> VoxamError {
    VoxamError::ZMachineScreen(message)
}

/// One character position on the screen with its dress (§8.7.1):
/// the character shown (a space when blank), the §8.7.1 style
/// bitmask, the §8.3.1 colour codes, and the §8.1.2 font it was
/// printed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub character: char,
    pub style: u16,
    pub foreground: i32,
    pub background: i32,
    pub font: u16,
}

pub const BLANK: Cell = Cell {
    character: ' ',
    style: ROMAN,
    foreground: DEFAULT_COLOUR,
    background: DEFAULT_COLOUR,
    font: NORMAL_FONT,
};

/// A pure §8 screen: two windows, one grid, no terminal.
///
/// Every method mirrors a Frontend operation, so a grid-backed
/// frontend can forward calls one-to-one. Inspection methods --
/// row_text, rendered, cell -- flush pending buffered text first,
/// so tests always see the screen a player would.
pub struct ScreenModel {
    columns: usize,
    lines: usize,
    version: u8,
    grid: Vec<Vec<Cell>>,
    split: usize,
    selected: u16,
    style: u16,
    font: u16,
    foreground: i32,
    background: i32,
    buffered: bool,
    pending: Vec<Cell>,
    scroll_due: bool,
    damage: std::collections::HashSet<usize>,
    /// [MORE] paging is interpreter courtesy: a frontend that
    /// wants a pause before a screenful of unread text scrolls
    /// away hangs a callback here, and the model counts the lower
    /// window's fed lines toward it. Left as None -- the plain
    /// stream, the tests -- nothing counts and nothing pauses, so
    /// recordings replay byte-identically.
    pub more: Option<Box<dyn FnMut()>>,
    fed: usize,
    upper_cursor: (usize, usize),
    lower_cursor: (usize, usize),
}

impl ScreenModel {
    /// Set the §8.7.3.3 start-of-game screen.
    ///
    /// Cleared, lower window selected, and the cursor at the
    /// bottom left through Version 4 (§8.6.3), the top left from
    /// Version 5.
    pub fn new(columns: usize, lines: usize, version: u8) -> Self {
        let mut model = Self {
            columns,
            lines,
            version,
            grid: vec![vec![BLANK; columns]; lines],
            split: 0,
            selected: LOWER,
            style: ROMAN,
            font: NORMAL_FONT,
            foreground: DEFAULT_COLOUR,
            background: DEFAULT_COLOUR,
            buffered: true,
            pending: Vec::new(),
            scroll_due: false,
            damage: std::collections::HashSet::new(),
            more: None,
            fed: 0,
            upper_cursor: (1, 1),
            lower_cursor: (1, 1),
        };

        model.lower_cursor = if version <= BOTTOM_HOME_LAST_VERSION {
            (lines, 1)
        } else {
            (model.lower_top(), 1)
        };

        model
    }

    /// The screen width in characters (§8.4).
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// The screen height in lines (§8.4).
    pub fn lines(&self) -> usize {
        self.lines
    }

    /// The upper window's current height in lines (§8.7.2.1).
    pub fn split(&self) -> usize {
        self.split
    }

    /// Which window takes the next printing (§8.7.2).
    pub fn selected(&self) -> u16 {
        self.selected
    }

    /// The screen row the upper window starts on.
    ///
    /// Below the Version 3 status line, at the very top from
    /// Version 4 (§8.6.1.1, §8.7.2.1).
    fn upper_top(&self) -> usize {
        if self.version <= STATUS_LAST_VERSION {
            2
        } else {
            1
        }
    }

    /// The first screen row belonging to the lower window.
    fn lower_top(&self) -> usize {
        self.upper_top() + self.split
    }

    /// Refuse window operations on a teletype (§8.5.1).
    fn require_windows(&self) -> Result<(), VoxamError> {
        if self.version < WINDOWS_FIRST_VERSION {
            return Err(screen_error(format!(
                "version {} has no windows: its screen can only be printed to \
                 (§8.5.1)",
                self.version
            )));
        }

        Ok(())
    }

    /// Print text to the selected window (§8.7.2).
    ///
    /// The upper window overlays whatever is there and never
    /// scrolls or buffers (§8.6.1.1.1, §8.7.2.5); the lower
    /// window word-wraps while buffering is on and scrolls at the
    /// bottom (§8.7.3.1).
    pub fn write(&mut self, text: &str) {
        for character in text.chars() {
            if self.selected == UPPER {
                self.write_upper(character);
            } else {
                self.write_lower(character);
            }
        }
    }

    /// Overlay one character at the upper cursor (§8.6.1.1.1).
    ///
    /// A newline moves to the start of the next window line,
    /// stopping at the window's bottom. Printing in the last
    /// column is legal and the cursor stays put, as §8.7.3.1's
    /// author suggests.
    fn write_upper(&mut self, character: char) {
        let (row, column) = self.upper_cursor;

        if character == '\n' {
            self.upper_cursor = ((row + 1).min(self.split.max(1)), 1);

            return;
        }

        let cell = self.dressed(character);

        self.paint(self.upper_top() + row - 1, column, cell);

        if column < self.columns {
            self.upper_cursor = (row, column + 1);
        }
    }

    /// Queue or emit one character for the lower window.
    ///
    /// While buffering is on, word characters gather in a pending
    /// buffer so a word that would overrun the margin wraps whole
    /// (§8.7.2.5 buffer_mode); spaces and newlines flush it.
    fn write_lower(&mut self, character: char) {
        if character == '\n' {
            self.flush();
            self.line_feed();
        } else if !self.buffered {
            let cell = self.dressed(character);

            self.emit(cell);
        } else if character == ' ' {
            self.flush();
            self.emit_space();
        } else {
            let cell = self.dressed(character);

            self.pending.push(cell);
        }
    }

    /// One character wearing the current style, colours, and font.
    fn dressed(&self, character: char) -> Cell {
        Cell {
            character,
            style: self.style,
            foreground: self.foreground,
            background: self.background,
            font: self.font,
        }
    }

    /// Emit the pending word, wrapping it whole if it fits a line.
    ///
    /// A word too long for any line simply character-wraps: there
    /// is no whole line it could have waited for.
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }

        let word = std::mem::take(&mut self.pending);
        let (_row, column) = self.lower_cursor;

        if word.len() > self.columns + 1 - column.min(self.columns + 1)
            && word.len() <= self.columns
        {
            self.line_feed();
        }

        for cell in word {
            self.emit(cell);
        }
    }

    /// Emit one space, or break the line instead at the margin.
    ///
    /// A space that would wrap becomes the line break itself: the
    /// next line never opens with the gap.
    fn emit_space(&mut self) {
        let (_row, column) = self.lower_cursor;

        if column > self.columns {
            self.line_feed();

            return;
        }

        let cell = self.dressed(' ');

        self.emit(cell);
    }

    /// Place one cell at the lower cursor, wrapping at the margin.
    ///
    /// A scroll owed by an earlier line feed happens now, just as
    /// the text "reaches" the bottom (§8.7.3.1): deferring it
    /// keeps the last printed line visible at the foot of the
    /// screen instead of above a permanently blank one.
    fn emit(&mut self, cell: Cell) {
        let (row, column) = self.lower_cursor;

        if column > self.columns {
            self.line_feed();
        }

        let (row, column) = if column > self.columns {
            self.lower_cursor
        } else {
            (row, column)
        };

        if self.scroll_due {
            self.scroll();
            self.scroll_due = false;
        }

        self.paint(row, column, cell);
        self.lower_cursor = (row, column + 1);
    }

    /// Move the lower cursor down, scrolling at the bottom
    /// (§8.7.3.1).
    ///
    /// At the bottom line the scroll is owed rather than paid:
    /// the next emission performs it. An already-owed scroll is
    /// paid first, so consecutive blank lines each earn their own.
    fn line_feed(&mut self) {
        if self.scroll_due {
            self.scroll();
            self.scroll_due = false;
        }

        let (row, _column) = self.lower_cursor;

        if row >= self.lines {
            self.scroll_due = true;
            self.lower_cursor = (self.lines, 1);
        } else {
            self.lower_cursor = (row + 1, 1);
        }

        self.feed_page();
    }

    /// Count one fed line toward a [MORE] pause (§8.8.3.2.6
    /// spirit).
    ///
    /// A screenful is a lower window's height less one -- the
    /// line the pause prompt itself stands on. The pause fires
    /// mid-write by design: the callback repaints and waits for a
    /// key, and reentrant reads of the grid are safe because a
    /// flush empties its pending word before emitting.
    fn feed_page(&mut self) {
        if self.more.is_none() {
            return;
        }

        self.fed += 1;

        if self.fed >= (self.lines.saturating_sub(self.split + 1)).max(1) {
            self.fed = 0;

            if let Some(more) = &mut self.more {
                more();
            }
        }
    }

    /// Reset the [MORE] budget: input means everything was read.
    pub fn rest(&mut self) {
        self.fed = 0;
    }

    /// Scroll the lower window up one line (§8.7.3.1).
    ///
    /// The upper window and the Version 3 status line never move
    /// (§8.6.2), and the fresh bottom line is blank in the
    /// current background without any reverse video.
    fn scroll(&mut self) {
        let top = self.lower_top();

        for row in top..self.lines {
            self.grid[row - 1] = self.grid[row].clone();
        }

        self.grid[self.lines - 1] = vec![self.blank_cell(); self.columns];
        self.damage.extend(top..=self.lines);
    }

    /// A blank in the current background (§8.7.3.1, §8.7.3.2).
    fn blank_cell(&self) -> Cell {
        Cell {
            character: ' ',
            style: ROMAN,
            foreground: DEFAULT_COLOUR,
            background: self.background,
            font: NORMAL_FONT,
        }
    }

    /// Set one grid position, remembering the row as damaged.
    fn paint(&mut self, row: usize, column: usize, cell: Cell) {
        self.grid[row - 1][column - 1] = cell;
        self.damage.insert(row);
    }

    /// Resize the upper window to the given height (§8.7.2.1).
    ///
    /// Version 3 clears the freshly split upper window
    /// (§8.6.1.1.2); later versions leave the screen's appearance
    /// alone (§8.6.1). A split that would swallow the lower
    /// cursor pushes it to the line just below the new upper
    /// window (§8.7.2.2), and a split made while the upper window
    /// is selected keeps its cursor when still inside, moving it
    /// to the top left otherwise (§8.7.2.1.1).
    pub fn split_window(&mut self, height: i32) -> Result<(), VoxamError> {
        self.require_windows()?;
        self.flush();

        // The upper window may take the whole screen -- Z-Tornado
        // plays its entire game in a full-height split -- but not
        // more than exists (§8.7.2.1).
        if height < 0 || height as usize > self.lines + 1 - self.upper_top() {
            return Err(screen_error(format!(
                "an upper window {height} lines tall does not fit a {}-line \
                 screen (§8.7.2.1)",
                self.lines
            )));
        }

        self.split = height as usize;

        if self.version == STATUS_LAST_VERSION && height != 0 {
            let top = self.upper_top();

            self.clear_rows(top, top + self.split - 1);
        }

        let (row, _column) = self.lower_cursor;

        if row < self.lower_top() {
            // The line just below the new upper window (§8.7.2.2)
            // -- or the last line there is, when the split took
            // all.
            self.lower_cursor = (self.lower_top().min(self.lines), 1);
        }

        let (upper_row, _upper_column) = self.upper_cursor;

        if self.selected == UPPER && upper_row > self.split.max(1) {
            self.upper_cursor = (1, 1);
        }

        Ok(())
    }

    /// Select a window for printing (§8.7.2).
    ///
    /// Selecting the upper window homes its cursor to the top
    /// left every time (§8.6.1, §8.7.2); the lower window keeps
    /// its place.
    pub fn set_window(&mut self, window: u16) -> Result<(), VoxamError> {
        self.require_windows()?;
        self.flush();

        if window != LOWER && window != UPPER {
            return Err(screen_error(format!(
                "there is no window {window} before version 6 (§8.7.2)"
            )));
        }

        self.selected = window;

        if window == UPPER {
            self.upper_cursor = (1, 1);
        }

        Ok(())
    }

    /// Move the upper window's cursor (§8.7.2.3.1).
    ///
    /// The opcode has no effect when the lower window is
    /// selected, by the spec's own sentence -- a conforming
    /// quiet, not a shortcut.
    pub fn set_cursor(&mut self, line: u16, column: u16) -> Result<(), VoxamError> {
        self.flush();

        if self.selected != UPPER {
            return Ok(());
        }

        // §8.7.2.3.1 calls a cursor outside the upper window
        // illegal, but prescribes nothing for the interpreter --
        // and Frotz checks nothing at all, silently placing the
        // cursor wherever it was asked. Careless authors lean on
        // that: Solitaire Poker splits a 20-row window and deals
        // its cards from row 21. The settlement is Frotz's, out
        // to the screen's edge; past the physical screen there is
        // nothing to paint on, and the halt stays loud.
        let reach = self.lines + 1 - self.upper_top();

        if !(1..=reach).contains(&usize::from(line))
            || !(1..=self.columns).contains(&usize::from(column))
        {
            return Err(screen_error(format!(
                "the cursor cannot move to ({line}, {column}): even §8.7.2.3.1's \
                 tolerated overreach past the upper window's {} lines ends at \
                 the screen, {reach} lines by {}",
                self.split, self.columns
            )));
        }

        self.upper_cursor = (usize::from(line), usize::from(column));

        Ok(())
    }

    /// The upper window's cursor, from either window (§8.7.2.3.2).
    pub fn get_cursor(&mut self) -> (usize, usize) {
        self.flush();

        self.upper_cursor
    }

    /// Erase a window to the background colour (§8.7.3.2).
    ///
    /// Window -1 unsplits the screen, clears the lot, selects the
    /// lower window, and homes its cursor -- bottom left in
    /// Version 4, top left from Version 5 (§8.7.3.3). Window -2
    /// clears the screen but keeps the split and the cursors (§15
    /// erase_window). Erasing a plain window homes its cursor to
    /// the top left from Version 5; in Version 4 the lower
    /// window's cursor homes to the bottom left (§8.7.3.2.1).
    pub fn erase_window(&mut self, window: i32) -> Result<(), VoxamError> {
        self.flush();

        // Erased text cannot be unread, so an erase refills the
        // [MORE] budget -- the rested-erase rule, brought over
        // from the v6 stage (§8.8.3.2.6).
        if window != i32::from(UPPER) {
            self.fed = 0;
        }

        if window == ERASE_UNSPLIT {
            self.clear_all();
            self.split = 0;
            self.selected = LOWER;
            self.home_lower();
        } else if window == ERASE_KEEP_SPLIT {
            self.clear_all();
        } else if window == i32::from(LOWER) {
            self.clear_rows(self.lower_top(), self.lines);
            self.home_lower();
        } else if window == i32::from(UPPER) {
            let top = self.upper_top();

            if self.split > 0 {
                self.clear_rows(top, top + self.split - 1);
            }

            self.upper_cursor = (1, 1);
        } else {
            return Err(screen_error(format!(
                "there is no window {window} to erase (§15 erase_window)"
            )));
        }

        Ok(())
    }

    /// Blank every row, status line included.
    fn clear_all(&mut self) {
        self.clear_rows(1, self.lines);
    }

    /// Blank the rows to the background, never reversed (§8.7.3.2).
    fn clear_rows(&mut self, first: usize, last: usize) {
        for row in first..=last {
            self.grid[row - 1] = vec![self.blank_cell(); self.columns];
        }

        self.damage.extend(first..=last);
    }

    /// Home the lower cursor per version (§8.7.3.2.1, §8.7.3.3).
    fn home_lower(&mut self) {
        self.scroll_due = false;

        if self.version <= BOTTOM_HOME_LAST_VERSION {
            self.lower_cursor = (self.lines, 1);
        } else {
            self.lower_cursor = (self.lower_top().min(self.lines), 1);
        }
    }

    /// Erase from the cursor to the end of the line (§8.7.3.4).
    pub fn erase_line(&mut self) {
        self.flush();

        let (screen_row, column) = if self.selected == UPPER {
            let (row, column) = self.upper_cursor;

            (self.upper_top() + row - 1, column)
        } else {
            self.lower_cursor
        };

        for position in column..=self.columns {
            self.grid[screen_row - 1][position - 1] = self.blank_cell();
        }

        self.damage.insert(screen_row);
    }

    /// Change the text style for what follows (§8.7.1).
    ///
    /// Roman clears every style; the rest combine, which §15
    /// set_text_style permits and Standard 1.1 blesses outright.
    /// Changing style mid-word is legal (§8.7.1.2), so the
    /// pending buffer stays: its cells are already dressed.
    pub fn set_style(&mut self, style: u16) {
        if style == ROMAN {
            self.style = ROMAN;
        } else {
            self.style |= style;
        }
    }

    /// Erase the last typed character during line input (§15
    /// read).
    ///
    /// Line editing belongs to the interpreter, and its rubout
    /// retreats the selected window's cursor one cell and blanks
    /// it. At the left edge there is nothing left of the line to
    /// rub, and the cursor stays put: the editor never chews into
    /// an earlier row.
    pub fn rub_out(&mut self) {
        self.flush();

        if self.selected == UPPER {
            let (row, column) = self.upper_cursor;

            if column > 1 {
                let cell = self.blank_cell();

                self.paint(self.upper_top() + row - 1, column - 1, cell);
                self.upper_cursor = (row, column - 1);
            }
        } else {
            let (row, column) = self.lower_cursor;

            if column > 1 {
                let cell = self.blank_cell();

                self.paint(row, column - 1, cell);
                self.lower_cursor = (row, column - 1);
            }
        }
    }

    /// Move the selected window's cursor left without erasing.
    ///
    /// The line editor's cursor motion (§15 read): unlike rub_out
    /// nothing is blanked, and like rub_out the motion stops at
    /// the left edge -- the editor never chews into an earlier
    /// row, so on a line that wrapped only the final row is
    /// editable. The cells actually moved come back so the
    /// editor's own ledger stays honest.
    pub fn retreat(&mut self, cells: usize) -> usize {
        self.flush();

        if self.selected == UPPER {
            let (row, column) = self.upper_cursor;
            let moved = cells.min(column - 1);

            self.upper_cursor = (row, column - moved);

            moved
        } else {
            let (row, column) = self.lower_cursor;
            let moved = cells.min(column - 1);

            self.lower_cursor = (row, column - moved);

            moved
        }
    }

    /// Print a §15 rectangle, right and down from the cursor.
    ///
    /// In the upper window each row after the first begins one
    /// line down from the last, at the column where the rectangle
    /// began -- how Beyond Zork stamps its map beside the story
    /// box without touching it. A rectangle taller than the
    /// window presses its last rows onto the bottom line, as an
    /// upper-window newline would. §15 leaves heights past 1
    /// undefined in the lower window; ordinary stacked lines,
    /// scrolling and all, are this model's settled answer.
    pub fn write_rectangle(&mut self, rows: &[String]) {
        self.flush();

        if self.selected != UPPER {
            for (index, row_text) in rows.iter().enumerate() {
                if index > 0 {
                    self.write("\n");
                }

                self.write(row_text);
            }

            return;
        }

        let (start_row, start_column) = self.upper_cursor;

        for (index, row_text) in rows.iter().enumerate() {
            if index > 0 {
                self.upper_cursor = ((start_row + index).min(self.split.max(1)), start_column);
            }

            for character in row_text.chars() {
                self.write_upper(character);
            }
        }
    }

    /// Change the font for what follows (§8.1.2).
    ///
    /// The model records which font dressed each cell and leaves
    /// the drawing of §16's shapes to the painter. Changing font
    /// mid-word is legal (§8.1.3.1), so the pending buffer stays:
    /// its cells are already dressed.
    pub fn set_font(&mut self, font: u16) {
        self.font = font;
    }

    /// Turn lower-window word-wrapping on or off (§15
    /// buffer_mode).
    ///
    /// The upper window never buffers either way (§8.7.2.5).
    pub fn set_buffering(&mut self, buffered: bool) {
        self.flush();
        self.buffered = buffered;
    }

    /// The current §8.3.1 background colour code.
    ///
    /// What erase_picture paints with: the nearest thing this
    /// model has to a window's own background (§15).
    pub fn background(&self) -> i32 {
        self.background
    }

    /// Change the printing colours (§8.3.1).
    ///
    /// A zero keeps that colour current; the codes stay symbolic,
    /// and mapping them to actual ink belongs to the painter.
    pub fn set_colour(&mut self, foreground: i32, background: i32) {
        if foreground != CURRENT_COLOUR {
            self.foreground = foreground;
        }

        if background != CURRENT_COLOUR {
            self.background = background;
        }
    }

    /// Draw the Version 1 to 3 status line on the top row (§8.2).
    ///
    /// The location sits on the left (§8.2.2), broken with an
    /// ellipsis when too long (§8.2.2.2); the right side shows
    /// score and turns, or an hours:minutes clock in a time game
    /// (§8.2.3). The whole row wears reverse video, the customary
    /// dress interpreters give it.
    pub fn show_status(&mut self, status: &Status) -> Result<(), VoxamError> {
        if self.version > STATUS_LAST_VERSION {
            return Err(screen_error(format!(
                "version {} draws its own status area; the interpreter's line \
                 ends at version 3 (§8.2)",
                self.version
            )));
        }

        self.flush();

        let right = if status.time_game {
            format!("Time: {}:{:02}", status.score, status.turns)
        } else {
            format!("Score: {}  Moves: {}", status.score, status.turns)
        };

        let mut room = status.location.clone();
        let available = self.columns as i64 - right.chars().count() as i64 - 3;

        if room.chars().count() as i64 > available {
            let kept = (available - 3).max(0) as usize;

            room = format!(
                "{}...",
                room.chars().take(kept).collect::<String>().trim_end()
            );
        }

        let width = self.columns.saturating_sub(right.chars().count() + 1);
        let mut line = format!(" {room}");

        while line.chars().count() < width {
            line.push(' ');
        }

        line.push_str(&right);
        line.push(' ');

        self.grid[0] = line
            .chars()
            .take(self.columns)
            .map(|character| Cell {
                character,
                style: REVERSE,
                foreground: DEFAULT_COLOUR,
                background: DEFAULT_COLOUR,
                font: NORMAL_FONT,
            })
            .collect();

        while self.grid[0].len() < self.columns {
            self.grid[0].push(Cell {
                character: ' ',
                style: REVERSE,
                foreground: DEFAULT_COLOUR,
                background: DEFAULT_COLOUR,
                font: NORMAL_FONT,
            });
        }

        self.damage.insert(1);

        Ok(())
    }

    /// The rows changed since the last sweep, in screen order.
    ///
    /// The painter's contract: repaint exactly these rows and the
    /// grid and glass agree again. Sweeping clears the slate.
    pub fn sweep(&mut self) -> Vec<usize> {
        self.flush();

        let mut damaged: Vec<usize> = self.damage.drain().collect();

        damaged.sort_unstable();

        damaged
    }

    /// One grid position, pending text flushed first.
    pub fn cell(&mut self, row: usize, column: usize) -> Cell {
        self.flush();

        self.grid[row - 1][column - 1]
    }

    /// One row's characters as a string, right side trimmed.
    pub fn row_text(&mut self, row: usize) -> String {
        self.flush();

        self.grid[row - 1]
            .iter()
            .map(|cell| cell.character)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// The whole screen as a text block, one line per row.
    pub fn rendered(&mut self) -> String {
        self.flush();

        (1..=self.lines)
            .map(|row| self.row_text(row))
            .collect::<Vec<String>>()
            .join("\n")
    }

    /// The selected window's cursor in screen coordinates.
    pub fn cursor(&mut self) -> (usize, usize) {
        self.flush();

        if self.selected == UPPER {
            let (row, column) = self.upper_cursor;

            return (self.upper_top() + row - 1, column);
        }

        self.lower_cursor
    }
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;
