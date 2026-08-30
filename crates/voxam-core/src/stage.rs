//! The Version 6 stage: §8.8's eight windows on one cell grid.
//!
//! Version 6 games place their windows in pixels -- a status strip
//! here, a story box there, chrome around a picture -- and the
//! plain and painted frontends can only mimic that with flowing
//! text. This model is for a glass that measures: it keeps all
//! eight §8.8.3 windows, each with a position and size in units,
//! its own cursor, dress, and attributes, and plots their text onto
//! one shared grid of cells. The grid interface is the screen
//! model's own -- cell, sweep, row_text -- so the graphics frontend
//! blits a stage exactly as it blits a screen.
//!
//! Units arrive from the machine's ledger world and cells leave for
//! the glass: positions and sizes are §8.8's pixels, converted here
//! with the font metrics the stage was built with. Nothing printed
//! belongs to a window once plotted (§8.8.3): moving a window moves
//! only its bookkeeping, and text lands wherever the window was at
//! the moment of printing.

use std::collections::BTreeSet;

use crate::errors::VoxamError;
use crate::frontend::{NORMAL_FONT, Status};
use crate::screen::{
    BLANK, CURRENT_COLOUR, Cell, DEFAULT_COLOUR, ERASE_KEEP_SPLIT, ERASE_UNSPLIT, ROMAN,
};

/// §8.8.3's eight windows, and the §8.8.3.1 boot attributes: window
/// 0 wraps and scrolls its running text; every other window overlays
/// in place until told otherwise.
pub const STAGE_WINDOWS: usize = 8;

/// A line count of -999 means "never print [MORE]" (§8.8.3.2.6);
/// Version 6 games set line counts freely to manipulate the paging.
pub const NEVER_MORE: i32 = -999;

/// The rectangle a stage erasure touched: first row, first column,
/// row count, and column count, in cells. The frontend uses it to
/// forget its shadow of the region.
pub type Rectangle = (usize, usize, usize, usize);

/// One dressed character placed at a unit position: the character's
/// top edge in units, 1-based -- the window's own y plus the
/// cursor's rows, unrounded, so text lands exactly where §8.8
/// placed its window -- its left edge in units, and the character
/// with its dress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPaint {
    pub line: i32,
    pub column: i32,
    pub cell: Cell,
}

/// A unit rectangle painted to a background colour (§8.8.5): the
/// top edge in units, 1-based, the left edge, the height and width
/// in units, and the §8.3.1 background colour code to paint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FillPaint {
    pub line: i32,
    pub column: i32,
    pub height: i32,
    pub width: i32,
    pub background: i32,
}

/// A unit rectangle whose pixels slide vertically (§8.8.3.6): the
/// rectangle in units, and how far the content slides -- positive
/// up, negative down. The exposed strip arrives as its own
/// FillPaint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShiftPaint {
    pub line: i32,
    pub column: i32,
    pub height: i32,
    pub width: i32,
    pub rise: i32,
}

/// One narrated painting operation, in units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paint {
    Text(TextPaint),
    Fill(FillPaint),
    Shift(ShiftPaint),
}

/// The [MORE] callback's shape: the pause's unit line and column,
/// then the window's foreground and background colour codes.
pub type MorePause = Box<dyn FnMut(i32, i32, i32, i32)>;

/// One §8.8.3 window: geometry in units, a cursor in cells.
///
/// The cursor is kept as 0-based cell offsets within the window's
/// own box -- the wrap arithmetic's natural coordinates -- and
/// converted to §8.8's 1-based units at the seam.
#[derive(Debug, Clone)]
struct Window {
    y: i32,
    x: i32,
    height: i32,
    width: i32,
    left: i32,
    right: i32,
    row: i32,
    column: i32,
    fed: i32,
    style: u16,
    foreground: i32,
    background: i32,
    font: u16,
    wrapping: bool,
    scrolling: bool,
    scroll_due: bool,
    pending: Vec<Cell>,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            y: 1,
            x: 1,
            height: 0,
            width: 0,
            left: 0,
            right: 0,
            row: 0,
            column: 0,
            fed: 0,
            style: ROMAN,
            foreground: DEFAULT_COLOUR,
            background: DEFAULT_COLOUR,
            font: NORMAL_FONT,
            wrapping: false,
            scrolling: false,
            scroll_due: false,
            pending: Vec::new(),
        }
    }
}

fn screen_error(message: String) -> VoxamError {
    VoxamError::ZMachineScreen(message)
}

/// A pure §8.8 screen: eight windows, one grid, no window system.
///
/// Every method mirrors a Frontend operation the machine forwards,
/// so the graphics frontend can hand calls straight through.
/// Inspection methods flush pending buffered text first, so tests
/// always see the stage a player would.
pub struct StageModel {
    columns: i32,
    lines: i32,
    font_width: i32,
    font_height: i32,
    grid: Vec<Vec<Cell>>,
    damage: BTreeSet<i32>,
    paints: Vec<Paint>,
    buffered: bool,
    split_seen: bool,
    selected: usize,
    /// The [MORE] seam: the frontend hangs a callback here, and
    /// the stage calls it -- with the pause's unit position and
    /// the window's colour codes -- when a scrolling window has
    /// fed a screenful of new lines since the player last rested
    /// (§8.8.3.2.6).
    pub more: Option<MorePause>,
    windows: Vec<Window>,
}

impl StageModel {
    /// Set the §8.8.3.3 boot stage: window 0 filling the screen of
    /// `columns` by `lines` cells, each cell `font_width` by
    /// `font_height` units.
    pub fn new(columns: usize, lines: usize, font_width: usize, font_height: usize) -> Self {
        let columns = columns as i32;
        let lines = lines as i32;
        let font_width = font_width as i32;
        let font_height = font_height as i32;
        let mut windows: Vec<Window> = (0..STAGE_WINDOWS).map(|_| Window::default()).collect();

        windows[0].height = lines * font_height;
        windows[0].width = columns * font_width;
        windows[0].wrapping = true;
        windows[0].scrolling = true;
        // Window 1 boots screen-wide and flat: §8.8.4.1's split
        // tiles it against window 0 without touching widths, so a
        // width must already be there for the split to mean
        // anything.
        windows[1].width = columns * font_width;

        Self {
            columns,
            lines,
            font_width,
            font_height,
            grid: vec![vec![BLANK; columns as usize]; lines as usize],
            damage: BTreeSet::new(),
            paints: Vec::new(),
            buffered: true,
            split_seen: false,
            selected: 0,
            more: None,
            windows,
        }
    }

    /// The screen width in cells.
    pub fn columns(&self) -> usize {
        self.columns as usize
    }

    /// The screen height in cells.
    pub fn lines(&self) -> usize {
        self.lines as usize
    }

    /// Which of the eight windows takes the next printing.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The selected window's §8.3.1 background colour code.
    pub fn background(&self) -> i32 {
        self.windows[self.selected].background
    }

    /// The selected window's §8.3.1 foreground colour code.
    pub fn foreground(&self) -> i32 {
        self.windows[self.selected].foreground
    }

    // --- geometry, from units to the cell grid ---

    /// The window's first screen cell row, 1-based.
    fn first_row(&self, window: usize) -> i32 {
        (self.windows[window].y - 1).div_euclid(self.font_height) + 1
    }

    /// The window's first screen cell column, 1-based.
    fn first_column(&self, window: usize) -> i32 {
        (self.windows[window].x - 1).div_euclid(self.font_width) + 1
    }

    /// How many whole cell rows fit the window's height.
    fn row_count(&self, window: usize) -> i32 {
        self.windows[window].height.div_euclid(self.font_height)
    }

    /// How many whole cell columns fit the window's width.
    fn column_count(&self, window: usize) -> i32 {
        self.windows[window].width.div_euclid(self.font_width)
    }

    /// The first writable cell column offset: the left margin.
    fn left_edge(&self, window: usize) -> i32 {
        self.windows[window].left.div_euclid(self.font_width)
    }

    /// One past the last writable column offset: the right margin.
    ///
    /// Margins are §8.8.3.2.1's: sizes in units, 0 by default, and
    /// text is clipped to stay inside them.
    fn right_edge(&self, window: usize) -> i32 {
        (self.windows[window].width - self.windows[window].right).div_euclid(self.font_width)
    }

    /// The window's cell rectangle, clipped to the screen: first
    /// row, first column, row count, column count, all i32 for the
    /// wrap arithmetic.
    fn boxed(&self, window: usize) -> (i32, i32, i32, i32) {
        let first_row = self.first_row(window).max(1);
        let first_column = self.first_column(window).max(1);
        let last_row = (self.first_row(window) + self.row_count(window) - 1).min(self.lines);
        let last_column =
            (self.first_column(window) + self.column_count(window) - 1).min(self.columns);

        (
            first_row,
            first_column,
            (last_row - first_row + 1).max(0),
            (last_column - first_column + 1).max(0),
        )
    }

    // --- the stage seam the machine drives ---

    /// Place a window at (line, column) with a size, in units.
    ///
    /// Nothing on screen moves (§8.8.3): the geometry only decides
    /// where future text lands. The window's own cursor is
    /// relative to its origin and rides along unchanged (§8.8.3.5).
    pub fn place_window(
        &mut self,
        window: i32,
        line: i32,
        column: i32,
        height: i32,
        width: i32,
    ) -> Result<(), VoxamError> {
        let target = self.known(window)?;

        self.flush(self.selected);

        let held = &mut self.windows[target];

        held.y = line;
        held.x = column;
        held.height = height;
        held.width = width;

        Ok(())
    }

    /// Select the window that takes the next printing (§8.8.3).
    ///
    /// Each window remembers its own cursor (§8.8.3.5), so
    /// selection homes nothing.
    pub fn set_window(&mut self, window: i32) -> Result<(), VoxamError> {
        self.flush(self.selected);

        self.selected = self.known(window)?;

        Ok(())
    }

    /// Move the selected window's cursor, in relative units.
    pub fn set_cursor(&mut self, line: i32, column: i32) {
        self.flush(self.selected);

        let held = &mut self.windows[self.selected];

        held.row = (line - 1).div_euclid(self.font_height).max(0);
        held.column = (column - 1).div_euclid(self.font_width).max(0);
        held.scroll_due = false;
    }

    /// The selected window's cursor, in relative units.
    pub fn get_cursor(&mut self) -> (i32, i32) {
        self.flush(self.selected);

        let held = &self.windows[self.selected];

        (
            held.row * self.font_height + 1,
            held.column * self.font_width + 1,
        )
    }

    /// The selected window's cursor as absolute screen units.
    ///
    /// Where get_cursor answers in the window's own coordinates
    /// (§8.7.2.3.2), this folds the window's origin in -- the
    /// position on the glass itself that §8.3.1's "colour of the
    /// pixel under the cursor" reads from.
    pub fn screen_cursor(&mut self) -> (i32, i32) {
        self.flush(self.selected);

        let held = &self.windows[self.selected];

        (
            held.y + held.row * self.font_height,
            held.x + held.column * self.font_width,
        )
    }

    /// Tile windows 1 and 0 vertically, the height in units.
    ///
    /// Window 1 takes the top of the screen at the given height
    /// and window 0 the rest (§8.8.4.1); x coordinates and widths
    /// stay put. Each cursor keeps its absolute screen position
    /// unless that now falls outside its window, in which case it
    /// homes (§15 split_window).
    pub fn split_window(&mut self, height: i32) {
        self.flush(self.selected);

        self.split_seen = self.split_seen || height > 0;

        let screen_height = self.lines * self.font_height;
        let absolutes = [
            self.first_row(1) + self.windows[1].row,
            self.first_row(0) + self.windows[0].row,
        ];

        self.windows[1].y = 1;
        self.windows[1].height = height;
        self.windows[0].y = height + 1;
        self.windows[0].height = (screen_height - height).max(0);

        for (window, absolute) in [1usize, 0].into_iter().zip(absolutes) {
            let row = absolute - self.first_row(window);

            self.windows[window].row = row;

            if !(0..self.row_count(window).max(1)).contains(&row) {
                self.windows[window].row = 0;
                self.windows[window].column = 0;
            }
        }
    }

    /// Print to the selected window, by its §8.8.3.1 attributes.
    ///
    /// A wrapping window breaks lines at its own right edge --
    /// whole words while buffering is on -- and a scrolling one
    /// scrolls its own rectangle; a window with neither overlays
    /// until its right margin, where the cursor stays and further
    /// text is ignored (§8.8.3.1.1).
    pub fn write(&mut self, text: &str) {
        for character in text.chars() {
            if character == '\n' {
                self.flush(self.selected);
                self.feed(self.selected);
            } else if !self.buffered {
                let cell = self.dressed(character);

                self.emit(self.selected, cell);
            } else if character == ' ' {
                self.flush(self.selected);
                self.emit_space(self.selected);
            } else {
                let cell = self.dressed(character);

                self.windows[self.selected].pending.push(cell);
            }
        }
    }

    /// Erase a window's rectangle to background (§8.8.5.3).
    ///
    /// Window -1 erases the whole screen to window 0's background,
    /// re-tiles windows 0 and 1 if a split had happened, and
    /// selects window 0 (§8.8.5.3.1, §8.8.4.2); window -2 erases
    /// the whole screen to the current background and changes
    /// nothing else (§8.8.5.3.2). A plain window erases its own
    /// rectangle and homes its cursor. Answers the erased cell
    /// rectangle, for the glass to forget; a window §8.8.3 does
    /// not name is refused.
    pub fn erase_window(&mut self, window: i32) -> Result<Rectangle, VoxamError> {
        self.flush(self.selected);

        if window == ERASE_UNSPLIT {
            let background = self.windows[0].background;

            self.blank_rows(1, self.lines, background);
            self.paints.push(Paint::Fill(self.screen_fill(background)));

            if self.split_seen {
                self.split_window(0);
            }

            self.selected = 0;
            self.windows[0].row = 0;
            self.windows[0].column = 0;
            self.windows[0].scroll_due = false;
            // Erased text cannot be unread: the whole screen is
            // gone, so every [MORE] budget refills (§8.8.3.2.6).
            self.rest();

            return Ok((1, 1, self.lines as usize, self.columns as usize));
        }

        if window == ERASE_KEEP_SPLIT {
            let background = self.background();

            self.blank_rows(1, self.lines, background);
            self.paints.push(Paint::Fill(self.screen_fill(background)));
            self.rest();

            return Ok((1, 1, self.lines as usize, self.columns as usize));
        }

        let target = self.known(window)?;
        let (first_row, first_column, row_count, column_count) = self.boxed(target);
        let background = self.windows[target].background;

        for row in first_row..first_row + row_count {
            for column in first_column..first_column + column_count {
                self.paint(row, column, blank(background));
            }
        }

        // The glass erases the window's true unit rectangle -- not
        // the cell approximation -- as §8.8.5.3 measures it.
        let held = &mut self.windows[target];

        self.paints.push(Paint::Fill(FillPaint {
            line: held.y,
            column: held.x,
            height: held.height,
            width: held.width,
            background: held.background,
        }));

        held.row = 0;
        held.column = 0;
        held.scroll_due = false;

        // Erased text cannot be unread: this window's [MORE]
        // budget refills -- Shogun erases window 0 before printing
        // its title menu into a freshly shrunken box, and a stale
        // count would pause the menu mid-print (§8.8.3.2.6). An
        // explicit never-pause stays in force.
        if held.fed != NEVER_MORE {
            held.fed = 0;
        }

        Ok((
            first_row as usize,
            first_column as usize,
            row_count as usize,
            column_count as usize,
        ))
    }

    /// The whole screen as one fill, in units.
    fn screen_fill(&self, background: i32) -> FillPaint {
        FillPaint {
            line: 1,
            column: 1,
            height: self.lines * self.font_height,
            width: self.columns * self.font_width,
            background,
        }
    }

    /// Scroll a window's rectangle by a pixel amount (§8.8.3.6).
    ///
    /// Positive scrolls the text up, negative down, in whole cell
    /// rows -- the §15 opcode, unrelated to the scrolling
    /// attribute -- and the exposed rows blank to the window's
    /// background. Arthur scrolls its story window this way at
    /// every prompt.
    pub fn scroll_window(&mut self, window: i32, pixels: i32) -> Result<(), VoxamError> {
        self.flush(self.selected);

        let target = self.known(window)?;

        for _ in 0..pixels.abs().div_euclid(self.font_height) {
            if pixels > 0 {
                self.scroll(target);
            } else {
                self.scroll_down(target);
            }
        }

        Ok(())
    }

    /// The player is at an input: every [MORE] budget refills.
    ///
    /// Keyboard attention is the §8.8.3.2.6 clock -- a read means
    /// the player has caught up with the screen.
    pub fn rest(&mut self) {
        for window in &mut self.windows {
            if window.fed != NEVER_MORE {
                window.fed = 0;
            }
        }
    }

    /// Set a window's §8.8.3.2.6 line count directly.
    ///
    /// Version 6 games often set line counts to manipulate when
    /// [MORE] is printed; -999 means never print it at all.
    pub fn set_line_count(&mut self, window: i32, count: i32) -> Result<(), VoxamError> {
        let target = self.known(window)?;

        self.windows[target].fed = count;

        Ok(())
    }

    /// Set a window's margins in units (§8.8.3.2.1).
    ///
    /// Wrapping text is clipped to stay inside them, and a cursor
    /// the new margins would strand moves to the left margin
    /// (§8.8.3.2.2.2).
    pub fn set_margins(&mut self, window: i32, left: i32, right: i32) -> Result<(), VoxamError> {
        self.flush(self.selected);

        let target = self.known(window)?;

        self.windows[target].left = left;
        self.windows[target].right = right;

        let column = self.windows[target].column;

        if !(self.left_edge(target)..self.right_edge(target)).contains(&column) {
            self.windows[target].column = self.left_edge(target);
        }

        Ok(())
    }

    /// Erase rightward from the cursor (§8.8.5.2).
    ///
    /// To the right margin by default; a Version 6 game may
    /// instead give a width in pixels, clipped to stay inside the
    /// margin. The grid blanks only the cells the span fully
    /// covers -- the fill is the pixel truth.
    pub fn erase_line(&mut self, pixels: Option<i32>) {
        self.flush(self.selected);

        let current = self.selected;
        let (first_row, first_column, row_count, _column_count) = self.boxed(current);

        if self.windows[current].row >= row_count {
            return;
        }

        let mut width = (self.right_edge(current) - self.windows[current].column) * self.font_width;

        if let Some(pixels) = pixels {
            width = pixels.min(width);
        }

        let row = first_row + self.windows[current].row;
        let start = first_column + self.windows[current].column;
        let background = self.windows[current].background;

        for column in start..start + width.div_euclid(self.font_width) {
            self.paint(row, column, blank(background));
        }

        if width > 0 {
            let held = &self.windows[current];

            self.paints.push(Paint::Fill(FillPaint {
                line: held.y + held.row * self.font_height,
                column: held.x + held.column * self.font_width,
                height: self.font_height,
                width,
                background: held.background,
            }));
        }
    }

    /// Retreat the cursor one cell and blank it (§15 read).
    pub fn rub_out(&mut self) {
        self.flush(self.selected);

        let current = self.selected;

        if self.windows[current].column > 0 {
            self.windows[current].column -= 1;

            let (first_row, first_column, _rows, _columns) = self.boxed(current);
            let held = &self.windows[current];
            let (row, column) = (first_row + held.row, first_column + held.column);
            let background = held.background;
            let fill = FillPaint {
                line: held.y + held.row * self.font_height,
                column: held.x + held.column * self.font_width,
                height: self.font_height,
                width: self.font_width,
                background,
            };

            self.paint(row, column, blank(background));
            self.paints.push(Paint::Fill(fill));
        }
    }

    /// Move the cursor left without erasing (§15 line editing).
    ///
    /// The stage's half of the line editor's cursor motion:
    /// rub_out's retreat without the blanking, stopped at the
    /// window's left edge, with the cells actually moved answered
    /// back.
    pub fn retreat(&mut self, cells: i32) -> i32 {
        self.flush(self.selected);

        let held = &mut self.windows[self.selected];
        let moved = cells.min(held.column);

        held.column -= moved;

        moved
    }

    /// Print a §15 rectangle, right and down from the cursor.
    ///
    /// Each row after the first begins one line down at the column
    /// where the rectangle began, overlaying without wrap -- the
    /// §15 print_table shape.
    pub fn write_rectangle<S: AsRef<str>>(&mut self, rows: &[S]) {
        self.flush(self.selected);

        let current = self.selected;
        let (start_row, start_column) = (self.windows[current].row, self.windows[current].column);
        let wrapping = self.windows[current].wrapping;

        self.windows[current].wrapping = false;

        for (index, row_text) in rows.iter().enumerate() {
            if index > 0 {
                let bottom = (self.row_count(current) - 1).max(0);

                self.windows[current].row = (start_row + index as i32).min(bottom);
                self.windows[current].column = start_column;
            }

            for character in row_text.as_ref().chars() {
                let cell = self.dressed(character);

                self.emit(current, cell);
            }
        }

        self.windows[current].wrapping = wrapping;
    }

    /// Change the selected window's style (§8.8.3.2.3).
    pub fn set_style(&mut self, style: u16) {
        let held = &mut self.windows[self.selected];

        if style == ROMAN {
            held.style = ROMAN;
        } else {
            held.style |= style;
        }
    }

    /// Change the selected window's colours (§8.8.3.2.4).
    pub fn set_colour(&mut self, foreground: i32, background: i32) {
        let held = &mut self.windows[self.selected];

        if foreground != CURRENT_COLOUR {
            held.foreground = foreground;
        }

        if background != CURRENT_COLOUR {
            held.background = background;
        }
    }

    /// Change the selected window's font (§8.8.3.2.5).
    pub fn set_font(&mut self, font: u16) {
        self.windows[self.selected].font = font;
    }

    /// Turn buffered printing off or on (§8.8.3.1.2).
    pub fn set_buffering(&mut self, buffered: bool) {
        self.flush(self.selected);

        self.buffered = buffered;
    }

    /// Refuse: a Version 6 game draws its own status (§8.2). The
    /// machine never sends one, and a stray call is a wiring fault
    /// worth hearing about.
    pub fn show_status(&self, _status: &Status) -> Result<(), VoxamError> {
        Err(screen_error(
            "version 6 draws its own status area; the stage has no line (§8.2)".to_string(),
        ))
    }

    // --- the grid the glass blits ---

    /// The unit-positioned paints since the last drain, in order.
    ///
    /// The glass performs exactly these -- text at true §8.8
    /// positions, fills, and scrolls -- and its own persistent
    /// pixels are the retained screen, §8.8.3's rule made literal.
    /// Draining clears the slate; the cell grid remains the
    /// inspectable approximation the tests read.
    pub fn paints(&mut self) -> Vec<Paint> {
        self.flush(self.selected);

        std::mem::take(&mut self.paints)
    }

    /// The rows changed since the last sweep, in screen order.
    pub fn sweep(&mut self) -> Vec<usize> {
        self.flush(self.selected);

        let damaged = self.damage.iter().map(|&row| row as usize).collect();

        self.damage.clear();

        damaged
    }

    /// One grid position, pending text flushed first.
    pub fn cell(&mut self, row: usize, column: usize) -> Cell {
        self.flush(self.selected);

        self.grid[row - 1][column - 1]
    }

    /// One row's characters as a string, right side trimmed.
    pub fn row_text(&mut self, row: usize) -> String {
        self.flush(self.selected);

        let text: String = self.grid[row - 1]
            .iter()
            .map(|cell| cell.character)
            .collect();

        text.trim_end().to_string()
    }

    /// The whole stage as a text block, one line per row.
    pub fn rendered(&mut self) -> String {
        (1..=self.lines as usize)
            .map(|row| self.row_text(row))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // --- the wrap machinery, one window at a time ---

    /// Police a window number against §8.8.3's eight.
    fn known(&self, window: i32) -> Result<usize, VoxamError> {
        if !(0..STAGE_WINDOWS as i32).contains(&window) {
            return Err(screen_error(format!(
                "window {window} is not one of the eight (§8.8.3)"
            )));
        }

        Ok(window as usize)
    }

    /// One character wearing the selected window's dress.
    fn dressed(&self, character: char) -> Cell {
        let held = &self.windows[self.selected];

        Cell {
            character,
            style: held.style,
            foreground: held.foreground,
            background: held.background,
            font: held.font,
        }
    }

    /// Emit a pending word, wrapping it whole when that fits.
    fn flush(&mut self, window: usize) {
        if self.windows[window].pending.is_empty() {
            return;
        }

        let word = std::mem::take(&mut self.windows[window].pending);
        let edge = self.right_edge(window);
        let length = word.len() as i32;

        if self.windows[window].wrapping
            && length > edge - self.windows[window].column
            && length <= edge - self.left_edge(window)
        {
            self.feed(window);
        }

        for cell in word {
            self.emit(window, cell);
        }
    }

    /// Emit one space, or let the line break swallow it.
    fn emit_space(&mut self, window: usize) {
        if self.windows[window].wrapping && self.windows[window].column >= self.right_edge(window) {
            self.feed(window);

            return;
        }

        let cell = self.dressed(' ');

        self.emit(window, cell);
    }

    /// Place one cell at the window's cursor, edge rules and all.
    ///
    /// The edges are the §8.8.3.2.1 margins' -- the whole window
    /// when they are 0, their default.
    fn emit(&mut self, window: usize, cell: Cell) {
        let edge = self.right_edge(window);

        if edge <= self.left_edge(window) || self.row_count(window) == 0 {
            return;
        }

        if self.windows[window].column >= edge {
            if !self.windows[window].wrapping {
                // §8.8.3.1.1: the cursor moves to the right margin
                // and stays there; further text is ignored.
                self.windows[window].column = edge;

                return;
            }

            self.feed(window);
        }

        if self.windows[window].scroll_due {
            self.scroll(window);

            self.windows[window].scroll_due = false;
        }

        let (first_row, first_column, _rows, _columns) = self.boxed(window);
        let held = &self.windows[window];
        let (row, column) = (first_row + held.row, first_column + held.column);
        let text = TextPaint {
            line: held.y + held.row * self.font_height,
            column: held.x + held.column * self.font_width,
            cell,
        };

        self.paint(row, column, cell);
        self.paints.push(Paint::Text(text));

        self.windows[window].column += 1;
    }

    /// Move to the next line, scrolling or pinning at the bottom.
    ///
    /// The cursor returns to the left margin (§8.8.3.2.1) -- the
    /// window's own left edge when no margin is set. A scrolling
    /// window counts its new lines, and a screenful of them since
    /// the player's last rest earns the [MORE] pause (§8.8.3.2.6);
    /// a line count of -999 never pauses.
    fn feed(&mut self, window: usize) {
        if self.windows[window].scroll_due {
            self.scroll(window);

            self.windows[window].scroll_due = false;
        }

        let bottom = (self.row_count(window) - 1).max(0);

        if self.windows[window].scrolling && self.windows[window].fed != NEVER_MORE {
            self.windows[window].fed += 1;

            if self.windows[window].fed >= bottom.max(1) && self.more.is_some() {
                let held = &self.windows[window];
                let (line, column) = (held.y + bottom * self.font_height, held.x + held.left);
                let (ink, paper) = (held.foreground, held.background);

                if let Some(more) = self.more.as_mut() {
                    more(line, column, ink, paper);
                }

                self.windows[window].fed = 0;
            }
        }

        if self.windows[window].row >= bottom {
            self.windows[window].row = bottom;

            if self.windows[window].scrolling {
                // The scroll is owed, not paid: it happens when the
                // next text arrives, keeping the last line at the
                // window's foot instead of above a blank one.
                self.windows[window].scroll_due = true;
            }
        } else {
            self.windows[window].row += 1;
        }

        self.windows[window].column = self.left_edge(window);
    }

    /// Scroll the window's own rectangle up one cell row.
    fn scroll(&mut self, window: usize) {
        let (first_row, first_column, row_count, column_count) = self.boxed(window);

        for row in first_row..first_row + row_count - 1 {
            for column in first_column..first_column + column_count {
                // The row below, 1-based: index `row` is the next
                // row down from 1-based `row`.
                let below = self.grid[row as usize][(column - 1) as usize];

                self.paint(row, column, below);
            }
        }

        let background = self.windows[window].background;

        for column in first_column..first_column + column_count {
            self.paint(first_row + row_count - 1, column, blank(background));
        }

        // Only the flowed region between the margins scrolls: the
        // margins keep their art -- Shogun anchors its ship in a
        // right margin while the text beside it scrolls fifty
        // times, which is only possible if the reference
        // interpreters left the margins unswept (§8.8.3.2.1).
        let flowed = self.flowed_width(window, column_count * self.font_width);
        let held = &self.windows[window];

        self.paints.push(Paint::Shift(ShiftPaint {
            line: held.y,
            column: held.x + held.left,
            height: row_count * self.font_height,
            width: flowed,
            rise: self.font_height,
        }));
        self.paints.push(Paint::Fill(FillPaint {
            line: held.y + (row_count - 1) * self.font_height,
            column: held.x + held.left,
            height: self.font_height,
            width: flowed,
            background: held.background,
        }));
    }

    /// Scroll the window's own rectangle down one cell row.
    fn scroll_down(&mut self, window: usize) {
        let (first_row, first_column, row_count, column_count) = self.boxed(window);

        for row in (first_row + 1..first_row + row_count).rev() {
            for column in first_column..first_column + column_count {
                // The row above, 1-based: index `row - 2`.
                let above = self.grid[(row - 2) as usize][(column - 1) as usize];

                self.paint(row, column, above);
            }
        }

        let background = self.windows[window].background;

        for column in first_column..first_column + column_count {
            self.paint(first_row, column, blank(background));
        }

        // The downward twin keeps its margins too (§8.8.3.2.1).
        let flowed = self.flowed_width(window, column_count * self.font_width);
        let held = &self.windows[window];

        self.paints.push(Paint::Shift(ShiftPaint {
            line: held.y,
            column: held.x + held.left,
            height: row_count * self.font_height,
            width: flowed,
            rise: -self.font_height,
        }));
        self.paints.push(Paint::Fill(FillPaint {
            line: held.y,
            column: held.x + held.left,
            height: self.font_height,
            width: flowed,
            background: held.background,
        }));
    }

    /// The scrolled region's width: between the margins, clipped.
    ///
    /// A window without margins scrolls its whole painted width,
    /// exactly as before; one with margins scrolls only where text
    /// flows, leaving the margins' art anchored (§8.8.3.2.1).
    fn flowed_width(&self, window: usize, painted: i32) -> i32 {
        let held = &self.windows[window];
        let between = held.width - held.left - held.right;

        between.min(painted).max(0)
    }

    /// Blank whole screen rows to a background colour.
    fn blank_rows(&mut self, first: i32, last: i32, background: i32) {
        for row in first..=last {
            self.grid[(row - 1) as usize] = vec![blank(background); self.columns as usize];
        }

        self.damage.extend(first..=last);
    }

    /// Set one grid position, clipped to the screen, and damage it.
    fn paint(&mut self, row: i32, column: i32, cell: Cell) {
        if (1..=self.lines).contains(&row) && (1..=self.columns).contains(&column) {
            self.grid[(row - 1) as usize][(column - 1) as usize] = cell;
            self.damage.insert(row);
        }
    }
}

/// A blank cell in a window's background, never reversed.
fn blank(background: i32) -> Cell {
    Cell {
        character: ' ',
        style: ROMAN,
        foreground: DEFAULT_COLOUR,
        background,
        font: NORMAL_FONT,
    }
}
