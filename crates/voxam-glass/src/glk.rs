//! The painted Glk display: the window tree on a ratatui terminal.
//!
//! The reference splits this face in two -- a display-independent
//! spine (the tree walk, the wrappers, the pager, the typed line,
//! the timer) and a thin blessed terminal under it. The rewrite in
//! kind folds the pair into one struct over ratatui's Backend
//! seam: the spine's whole-tree repaint becomes a persistent cell
//! canvas ("painting over is all the erasing there is") copied
//! into ratatui's buffer each frame, whose diff finds the changed
//! cells.
//!
//! Input is collected synchronously at a keyboard that echoes
//! nothing, the half-typed line drawn as part of the layout; a
//! timer coming round interrupts a wait by answering the events
//! instead -- the `Asked::Instead` departure standing in for the
//! reference's posted events -- so glk_select can come back and
//! deliver it.
//!
//! Two deferrals ride along, both noted in the port map: the
//! speaker (sound claims false, honestly) and the recording
//! seams, which serve a session instrument the CLI has not
//! carried over yet.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::Position;
use ratatui::style::{Modifier, Style as Dress};

use voxam_core::glulx::glk::frontend::{Asked, Frontend};
use voxam_core::glulx::glk::objects::{
    Event, Window, WindowKind, WindowMap, event_type, file_mode, key_code, style,
};
use voxam_core::glulx::glk::wrap::{Segment, Wrapper, plain};

use crate::painter::{FALLBACK_COLUMNS, FALLBACK_LINES, MORE_PROMPT};

/// A run's appearance key: the Glk style number and the hyperlink
/// value it was written under, zero for none (Glk: Hyperlinks).
pub type Key = (u32, u32);

/// Glk styles as the three attributes every painted display can
/// dress a run in: bold, italic, and reverse. Anything absent
/// renders plain; Preformatted deliberately so, since the painted
/// displays are monospaced already.
fn attributes(number: u32) -> Modifier {
    match number {
        style::EMPHASIZED | style::NOTE | style::BLOCK_QUOTE | style::USER1 => Modifier::ITALIC,
        style::HEADER | style::SUBHEADER | style::INPUT => Modifier::BOLD,
        style::ALERT => Modifier::BOLD | Modifier::REVERSED,
        style::USER2 => Modifier::REVERSED,
        _ => Modifier::empty(),
    }
}

// The stylehint numbers glk_style_measure asks in (Glk: Suggesting
// the Appearance of Styles).
const HINT_INDENTATION: u32 = 0;
const HINT_PARA_INDENTATION: u32 = 1;
const HINT_SIZE: u32 = 3;
const HINT_WEIGHT: u32 = 4;
const HINT_OBLIQUE: u32 = 5;
const HINT_PROPORTIONAL: u32 = 6;

const PRINTABLE_FLOOR: u32 = 0x20;
const CHARACTER_CEILING: u32 = 0x0010_FFFF;

/// One raw read's outcome at the intake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fetch {
    /// A keystroke, as a Glk character code.
    Key(u32),
    /// Nothing usable: an expired timeout, an unmapped key.
    Nothing,
    /// No keystroke can ever arrive again: the terminal is gone,
    /// or a scripted intake ran dry.
    Ended,
}

/// One keystroke as a Glk character code, with a timeout for the
/// timer's sake.
pub trait CodeSource {
    fn code(&mut self, timeout: Option<Duration>) -> Fetch;
}

/// One crossterm event as a Glk character code (Glk: Character
/// Input); None for an event no read can use.
pub fn coded(event: &TermEvent) -> Option<u32> {
    let TermEvent::Key(key) = event else {
        return None;
    };

    if key.kind == KeyEventKind::Release {
        return None;
    }

    match key.code {
        KeyCode::Enter => Some(key_code::RETURN),
        KeyCode::Backspace | KeyCode::Delete => Some(key_code::DELETE),
        KeyCode::Esc => Some(key_code::ESCAPE),
        KeyCode::Left => Some(key_code::LEFT),
        KeyCode::Right => Some(key_code::RIGHT),
        KeyCode::Up => Some(key_code::UP),
        KeyCode::Down => Some(key_code::DOWN),
        KeyCode::Tab => Some(key_code::TAB),
        KeyCode::PageUp => Some(key_code::PAGE_UP),
        KeyCode::PageDown => Some(key_code::PAGE_DOWN),
        KeyCode::Home => Some(key_code::HOME),
        KeyCode::End => Some(key_code::END),
        KeyCode::Char(character) => Some(u32::from(character)),
        _ => None,
    }
}

/// The live intake: crossterm's own event queue, as Glk codes.
#[derive(Default)]
pub struct EventCodes;

impl CodeSource for EventCodes {
    fn code(&mut self, timeout: Option<Duration>) -> Fetch {
        use ratatui::crossterm::event::{poll, read};

        if let Some(patience) = timeout {
            match poll(patience) {
                Ok(false) => return Fetch::Nothing,
                Ok(true) => {}
                Err(_) => return Fetch::Ended,
            }
        }

        let Ok(event) = read() else {
            return Fetch::Ended;
        };

        // The reference's cbreak keeps SIGINT alive; raw mode ate
        // the signal, so the intake restores the shell and dies
        // the same death.
        if let TermEvent::Key(key) = &event
            && key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            let _ = ratatui::crossterm::terminal::disable_raw_mode();
            println!();
            std::process::exit(130);
        }

        coded(&event).map_or(Fetch::Nothing, Fetch::Key)
    }
}

/// A scripted intake for the batteries: codes in order, the asked
/// timeouts kept on the record, and an honest end when drained.
#[derive(Default)]
pub struct ScriptedCodes {
    /// The keystrokes still to come; None entries are expiries.
    pub codes: Vec<Option<u32>>,
    /// Every timeout the display asked with, in ask order.
    pub timeouts: Vec<Option<Duration>>,
}

impl ScriptedCodes {
    pub fn new(codes: Vec<Option<u32>>) -> Self {
        Self {
            codes,
            timeouts: Vec::new(),
        }
    }

    /// The keystrokes of a typed line, Return included.
    pub fn typed(text: &str) -> Self {
        let mut codes: Vec<Option<u32>> = text.chars().map(|held| Some(u32::from(held))).collect();

        codes.push(Some(key_code::RETURN));

        Self::new(codes)
    }
}

impl CodeSource for ScriptedCodes {
    fn code(&mut self, timeout: Option<Duration>) -> Fetch {
        self.timeouts.push(timeout);

        if self.codes.is_empty() {
            return Fetch::Ended;
        }

        match self.codes.remove(0) {
            Some(code) => Fetch::Key(code),
            None => Fetch::Nothing,
        }
    }
}

/// A shared script, so a battery can refill keys mid-test.
impl CodeSource for std::rc::Rc<std::cell::RefCell<ScriptedCodes>> {
    fn code(&mut self, timeout: Option<Duration>) -> Fetch {
        self.borrow_mut().code(timeout)
    }
}

/// A repeating deadline, for a display that waits with one.
///
/// Glk timers fire every so many milliseconds while the game is
/// blocked in glk_select (Glk: Timer Events). A display that can
/// wait on the keyboard with a timeout wants exactly this
/// bookkeeping. The moments arrive as monotonic seconds from the
/// display's own clock -- the reference's monkeypatchable
/// monotonic, spelled as an argument.
#[derive(Default)]
pub struct Timer {
    interval: f64,
    deadline: Option<f64>,
}

impl Timer {
    /// Start stopped.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start firing every millisecs; zero or less stops.
    pub fn set(&mut self, millisecs: i64, now: f64) {
        self.interval = millisecs as f64 / 1000.0;
        self.deadline = if millisecs <= 0 {
            None
        } else {
            Some(now + self.interval)
        };
    }

    /// How long a wait may block, or None for indefinitely.
    pub fn timeout(&self, now: f64) -> Option<f64> {
        self.deadline.map(|deadline| (deadline - now).max(0.0))
    }

    /// Whether the timer has come round; rearms it if so.
    pub fn due(&mut self, now: f64) -> bool {
        if let Some(deadline) = self.deadline
            && now >= deadline
        {
            self.deadline = Some(now + self.interval);

            return true;
        }

        false
    }
}

/// What one wait for a keystroke came back with.
enum Waited {
    Code(u32),
    Events(Vec<Event>),
    Ended,
}

/// Paints the Glk window tree across a whole ratatui terminal.
///
/// Redraw is unconditional and whole-tree onto a persistent
/// canvas: every window paints its own bounding box padded to its
/// full width, and the boxes partition the screen between them,
/// so painting over is all the erasing there is -- the same
/// philosophy as the Z-Machine painter, which never clears
/// either. The canvas then rides ratatui's buffer diff to the
/// glass.
pub struct GlkGlass<B: Backend> {
    terminal: Terminal<B>,
    codes: Box<dyn CodeSource>,
    clock: Box<dyn FnMut() -> f64>,
    pub timer: Timer,
    /// The persistent cell canvas the tree paints onto.
    canvas: Buffer,
    /// Each buffer window's kept text, keyed by its internal id
    /// and pruned to the live tree on every flush, so a window
    /// closed and another opened cannot inherit the first one's
    /// text.
    buffers: HashMap<u32, Wrapper<Key>>,
    // The line being typed, and where it is being typed.
    typed: String,
    typing: Option<u32>,
    size: Option<(i64, i64)>,
    root: Option<u32>,
    /// The first terminal mishap, kept for the session to surface.
    pub fault: Option<B::Error>,
}

impl<B: Backend> GlkGlass<B> {
    /// Stand over a terminal with an empty tree and a stopped
    /// timer, on the real monotonic clock.
    pub fn new(terminal: Terminal<B>, codes: Box<dyn CodeSource>) -> Self {
        let started = Instant::now();
        let measured = terminal.size().ok();
        let width = measured.map_or(0, |size| i64::from(size.width));
        let height = measured.map_or(0, |size| i64::from(size.height));
        let area = ratatui::layout::Rect::new(
            0,
            0,
            if width == 0 {
                FALLBACK_COLUMNS as u16
            } else {
                width as u16
            },
            if height == 0 {
                FALLBACK_LINES as u16
            } else {
                height as u16
            },
        );

        Self {
            terminal,
            codes,
            clock: Box::new(move || started.elapsed().as_secs_f64()),
            timer: Timer::new(),
            canvas: Buffer::empty(area),
            buffers: HashMap::new(),
            typed: String::new(),
            typing: None,
            size: None,
            root: None,
            fault: None,
        }
    }

    /// Choose a size instead of the terminal's own measure.
    pub fn sized(mut self, size: (i64, i64)) -> Self {
        self.size = Some(size);
        self.canvas = Buffer::empty(ratatui::layout::Rect::new(
            0,
            0,
            size.0.max(0) as u16,
            size.1.max(0) as u16,
        ));

        self
    }

    /// Choose a clock instead of the monotonic one -- the
    /// batteries tick theirs by hand.
    pub fn clocked(mut self, clock: Box<dyn FnMut() -> f64>) -> Self {
        self.clock = clock;

        self
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

    /// Put a styled run of cells at a display position on the
    /// canvas. The position is 0-based display units, x across
    /// and y down -- the same units the window tree's bounding
    /// boxes are measured in.
    fn place(&mut self, x: i64, y: i64, line: &[Segment<Key>]) {
        if y < 0 || y >= i64::from(self.canvas.area.height) {
            return;
        }

        let mut column = x;

        for ((number, _link), text) in line {
            let dress = Dress::default().add_modifier(attributes(*number));

            for character in text.chars() {
                if column >= 0
                    && let Some(cell) = self.canvas.cell_mut(Position::new(column as u16, y as u16))
                {
                    cell.set_char(character);
                    cell.set_style(dress);
                }

                column += 1;
            }
        }
    }

    /// End the frame: the canvas rides the buffer diff to the
    /// glass, with the cursor shown at a cell or parked out of
    /// the way at the bottom.
    fn finish(&mut self, cursor: Option<(i64, i64)>) {
        let (x, y) = cursor.unwrap_or((0, self.sizes().1 - 1));
        let Self {
            terminal, canvas, ..
        } = self;
        let drawn = terminal
            .draw(|frame| {
                let area = frame.buffer_mut().area;

                for row in 0..area.height {
                    for column in 0..area.width {
                        let position = Position::new(column, row);

                        if let (Some(held), Some(painted)) =
                            (canvas.cell(position), frame.buffer_mut().cell_mut(position))
                        {
                            *painted = held.clone();
                        }
                    }
                }

                frame.set_cursor_position(Position::new(x.max(0) as u16, y.max(0) as u16));
            })
            .map(|_| ());

        self.noted(drawn);
    }

    fn sizes(&self) -> (i64, i64) {
        if let Some(chosen) = self.size {
            return chosen;
        }

        let measured = self.terminal.size().ok();
        let width = measured.map_or(0, |size| i64::from(size.width));
        let height = measured.map_or(0, |size| i64::from(size.height));

        (
            if width == 0 {
                FALLBACK_COLUMNS as i64
            } else {
                width
            },
            if height == 0 {
                FALLBACK_LINES as i64
            } else {
                height
            },
        )
    }

    /// Paint every row blank, wiping what the shell left: the
    /// story begins on a clean screen.
    pub fn clear(&mut self) {
        let (width, height) = self.sizes();
        let blank = " ".repeat(width.max(0) as usize);

        for row in 0..height {
            self.place(0, row, &[((style::NORMAL, 0), blank.clone())]);
        }

        self.finish(None);
    }

    /// Leave the cursor under the story, for the shell's prompt.
    pub fn retire(&mut self) {
        let (_, height) = self.sizes();
        let parked = self
            .terminal
            .set_cursor_position(Position::new(0, height.saturating_sub(1).max(0) as u16));

        self.noted(parked);

        let shown = self.terminal.show_cursor();

        self.noted(shown);
    }

    /// Draw a window and its children; say where the cursor goes.
    fn paint(&mut self, windows: &mut WindowMap, key: u32) -> Option<(i64, i64)> {
        let window = windows.get(&key)?;

        match &window.kind {
            WindowKind::Pair(pair) => {
                let (child1, child2) = (pair.child1, pair.child2);
                let first = self.paint(windows, child1);
                let second = self.paint(windows, child2);

                second.or(first)
            }
            WindowKind::Grid(_) => self.paint_grid(windows, key),
            WindowKind::Buffer(_) => self.paint_buffer(windows, key),
            WindowKind::Graphics(_) => {
                // Painting over is all the erasing there is for
                // text, but a graphics window's pixels are the
                // game's own work; a terminal has none to keep,
                // so the pending clear simply rests.
                if let Some(window) = windows.get_mut(&key) {
                    window.pending_clear = false;
                }

                None
            }
            WindowKind::Blank => {
                // A blank window shows blankness (Glk: Blank
                // Windows). The box is measured directly: a
                // sizeless window answers the game zero, but its
                // box is still real and still needs covering.
                let (left, top, right, bottom) = window.bbox;
                let blank = " ".repeat((right - left).max(0) as usize);

                for index in 0..(bottom - top).max(0) {
                    self.place(left, top + index, &[((style::NORMAL, 0), blank.clone())]);
                }

                None
            }
        }
    }

    fn paint_grid(&mut self, windows: &WindowMap, key: u32) -> Option<(i64, i64)> {
        let window = windows.get(&key)?;
        let (left, top, _, _) = window.bbox;
        let WindowKind::Grid(data) = &window.kind else {
            return None;
        };

        // The grid's rows are already exactly its size: the model
        // resizes them with every rearrange.
        let rows: Vec<Vec<Segment<Key>>> = data
            .lines
            .iter()
            .enumerate()
            .map(|(index, cells)| {
                grouped(
                    cells,
                    data.styles.get(index).map_or(&[][..], Vec::as_slice),
                    data.links.get(index).map_or(&[][..], Vec::as_slice),
                )
            })
            .collect();
        let (width, height) = (window.width(), window.height());
        let (cursor_x, cursor_y) = (data.cursor_x, data.cursor_y);

        for (index, row) in rows.iter().enumerate() {
            self.place(left, top + index as i64, row);
        }

        if self.typing != Some(key) {
            return None;
        }

        // A grid window taking line input shows it at the cursor,
        // where the game left it -- there is nowhere else it
        // could sensibly go.
        let column = cursor_x.min((width - 1).max(0));
        let row = cursor_y.min((height - 1).max(0));
        let text: String = self
            .typed
            .chars()
            .take((width - column).max(0) as usize)
            .collect();
        let x = left + column;
        let y = top + row;
        let typed = text.chars().count() as i64;

        self.place(x, y, &[((style::INPUT, 0), text)]);

        Some((x + typed, y))
    }

    fn paint_buffer(&mut self, windows: &mut WindowMap, key: u32) -> Option<(i64, i64)> {
        let window = windows.get_mut(&key)?;
        let width = window.width();
        let height = window.height();
        let (left, top, _, _) = window.bbox;
        let content = window.take_content();
        let pending_clear = window.pending_clear;

        window.pending_clear = false;

        let wrapper = self.buffers.entry(key).or_insert_with(|| {
            Wrapper::new(if width > 0 {
                width as usize
            } else {
                FALLBACK_COLUMNS
            })
        });

        wrapper.resize(if width > 0 {
            width as usize
        } else {
            FALLBACK_COLUMNS
        });

        if pending_clear {
            wrapper.clear();
        }

        // The wrapper keys runs by style and link together, so a
        // linked run survives wrapping distinct from its plain
        // neighbours. Text alone: a glass that claims no buffer
        // images never has a placed picture to meet here.
        wrapper.add(content.into_iter().filter_map(|flow| match flow {
            voxam_core::glulx::glk::objects::Flow::Run {
                style: number,
                hyperlink,
                text,
            } => Some(((number, hyperlink), text)),
            _ => None,
        }));

        if height <= 0 {
            // A buffer squeezed flat by a split still keeps its
            // text; there is just nowhere to paint it.
            return None;
        }

        let view = wrapper.view(height as usize);
        let more = view.more;
        let mut visible = view.lines;
        let typing = self.typing == Some(key) && !more;

        if typing {
            // The line being typed belongs at the end of the
            // text, but is not part of it until the game accepts
            // it.
            let previewed = wrapper.preview(&[((style::INPUT, 0), self.typed.clone())]);
            let keep = previewed.len().saturating_sub(height as usize);

            visible = previewed[keep..].to_vec();
        }

        // The newest line sits at the bottom of the box, so the
        // display scrolls the way a terminal does rather than
        // filling downwards.
        let offset = height - visible.len() as i64 - i64::from(more);
        let bottom = top + height - 1;

        for index in 0..height {
            let held = index - offset;
            let line: Vec<Segment<Key>> = if held >= 0 && (held as usize) < visible.len() {
                visible[held as usize].clone()
            } else {
                Vec::new()
            };
            let pad = " ".repeat((width as usize).saturating_sub(plain(&line).chars().count()));
            let mut padded = line;

            padded.push(((style::NORMAL, 0), pad));
            self.place(left, top + index, &padded);
        }

        if more {
            let pad = " ".repeat((width as usize).saturating_sub(MORE_PROMPT.len()));

            self.place(
                left,
                bottom,
                &[
                    ((style::ALERT, 0), MORE_PROMPT.to_string()),
                    ((style::NORMAL, 0), pad),
                ],
            );

            return Some((left + MORE_PROMPT.len() as i64, bottom));
        }

        if !typing || visible.is_empty() {
            return None;
        }

        let last = plain(visible.last().expect("checked above"))
            .chars()
            .count() as i64;

        Some((left + last, bottom))
    }

    /// Redraw after a keystroke, so typing is visible.
    fn repaint(&mut self, windows: &mut WindowMap) {
        // Prune wrappers whose windows are gone, so a reused id
        // cannot inherit a closed window's text.
        self.buffers.retain(|id, _| windows.contains_key(id));

        if let Some(root) = self.root {
            let cursor = self.paint(windows, root);

            self.finish(cursor);
        }
    }

    /// Show the next page of every waiting window; did any wait?
    fn turn_page(&mut self, windows: &mut WindowMap) -> bool {
        let mut waiting = Vec::new();

        for (id, wrapper) in &mut self.buffers {
            if let Some(window) = windows.get(id) {
                let height = window.height().max(0) as usize;

                if wrapper.view(height).more {
                    waiting.push((*id, height));
                }
            }
        }

        for (id, height) in &waiting {
            if let Some(wrapper) = self.buffers.get_mut(id) {
                wrapper.advance(*height);
            }
        }

        if waiting.is_empty() {
            return false;
        }

        self.repaint(windows);

        true
    }

    /// Treat every window as read, so nothing is waiting.
    fn catch_up(&mut self) {
        for wrapper in self.buffers.values_mut() {
            wrapper.catch_up();
        }
    }

    /// Wait for a keystroke; the events instead if something else
    /// came up.
    ///
    /// A key pressed while text is waiting turns the page instead
    /// of reaching the game -- which is the whole point of the
    /// pause, and why every input path goes through here. The
    /// something else is a timer coming round: it answers its
    /// event, so glk_select can come back and deliver it.
    fn wait(&mut self, mut windows: Option<&mut WindowMap>) -> Waited {
        loop {
            let now = (self.clock)();
            let timeout = self.timer.timeout(now).map(Duration::from_secs_f64);

            match self.codes.code(timeout) {
                Fetch::Key(code) => {
                    if let Some(held) = windows.as_mut()
                        && self.turn_page(held)
                    {
                        continue;
                    }

                    return Waited::Code(code);
                }
                Fetch::Ended => return Waited::Ended,
                Fetch::Nothing => {
                    let now = (self.clock)();

                    if self.timer.due(now) {
                        return Waited::Events(vec![Event::new(event_type::TIMER, None, 0, 0)]);
                    }
                }
            }
        }
    }

    /// Apply one keystroke to the line being typed.
    fn edit(&mut self, code: u32, maxlen: usize) {
        if code == key_code::DELETE {
            self.typed.pop();
        } else if code == key_code::ESCAPE {
            self.typed.clear();
        } else if (PRINTABLE_FLOOR..=CHARACTER_CEILING).contains(&code)
            && self.typed.chars().count() < maxlen
            && let Some(character) = char::from_u32(code)
        {
            self.typed.push(character);
        }
    }

    fn accept(&mut self, maxlen: u32, terminator: u32) -> (String, u32) {
        let text: String = self.typed.chars().take(maxlen as usize).collect();

        self.typed.clear();
        self.typing = None;

        (text, terminator)
    }
}

/// Collapse a grid row's per-cell dress into runs.
///
/// The key carries the style and the link value together, so a
/// linked run stays distinct from its plain neighbours all the
/// way to the display (Glk: Hyperlinks).
pub fn grouped(row: &[char], styles: &[u32], links: &[u32]) -> Vec<Segment<Key>> {
    let mut segments: Vec<Segment<Key>> = Vec::new();

    for (index, character) in row.iter().enumerate() {
        let number = styles.get(index).copied().unwrap_or(style::NORMAL);
        let link = links.get(index).copied().unwrap_or(0);
        let key = (number, link);

        if let Some((held, text)) = segments.last_mut()
            && *held == key
        {
            text.push(*character);
        } else {
            segments.push((key, character.to_string()));
        }
    }

    segments
}

impl<B: Backend> Frontend for GlkGlass<B> {
    /// Every painted display reads a key with a timeout, so
    /// timers can fire.
    fn timer_input(&self) -> bool {
        true
    }

    /// The terminal's own measure, unless one was chosen.
    fn size(&self) -> (i64, i64) {
        self.sizes()
    }

    /// Repaint the whole display from the window tree.
    fn flush(&mut self, windows: &mut WindowMap, root: Option<u32>) {
        self.root = root;

        if root.is_some() {
            self.repaint(windows);
        }
    }

    /// Collect a line at the keyboard, drawn as it is typed.
    fn read_line(
        &mut self,
        windows: &mut WindowMap,
        window: u32,
        maxlen: u32,
    ) -> Asked<(String, u32)> {
        let terminators = windows
            .get(&window)
            .and_then(|held| held.line_request.as_ref())
            .map(|request| request.terminators.clone())
            .unwrap_or_default();

        self.typing = Some(window);
        // The flush that preceded this did not know where input
        // was going, so repaint once to put the cursor at the
        // prompt.
        self.repaint(windows);

        loop {
            match self.wait(Some(windows)) {
                Waited::Ended => return Asked::End,
                Waited::Events(events) => {
                    // A timer fired mid-line. The half-typed line
                    // stays where it is and the request stays
                    // pending; glk_select will be back for it once
                    // it has delivered the timer event.
                    return Asked::Instead(events);
                }
                Waited::Code(code) if code == key_code::RETURN => {
                    return Asked::Answer(self.accept(maxlen, 0));
                }
                Waited::Code(code) if terminators.contains(&code) => {
                    return Asked::Answer(self.accept(maxlen, code));
                }
                Waited::Code(code) => {
                    self.edit(code, maxlen as usize);
                    self.repaint(windows);
                }
            }
        }
    }

    /// One keystroke, as a Glk character code.
    fn read_char(&mut self, windows: &mut WindowMap, _window: u32) -> Asked<u32> {
        match self.wait(Some(windows)) {
            Waited::Code(code) => Asked::Answer(code),
            Waited::Events(events) => Asked::Instead(events),
            Waited::Ended => Asked::End,
        }
    }

    /// Ask for timer events every so often; zero stops them.
    fn set_timer(&mut self, millisecs: u32) {
        let now = (self.clock)();

        self.timer.set(i64::from(millisecs), now);
    }

    /// Two styles differ here when their dress differs.
    fn style_distinguish(&self, _window: &Window, first: u32, second: u32) -> bool {
        attributes(first) != attributes(second)
    }

    /// Measure a style hint. A character cell is the only unit.
    fn style_measure(&self, _window: &Window, number: u32, hint: u32) -> Option<u32> {
        let dress = attributes(number);

        match hint {
            // Relative to the normal size, which is the only
            // size.
            HINT_SIZE => Some(0),
            HINT_WEIGHT => Some(u32::from(dress.contains(Modifier::BOLD))),
            HINT_OBLIQUE => Some(u32::from(dress.contains(Modifier::ITALIC))),
            // The painted displays are monospaced throughout.
            HINT_PROPORTIONAL => Some(0),
            HINT_INDENTATION | HINT_PARA_INDENTATION => Some(0),
            _ => None,
        }
    }

    /// Ask for a filename on the bottom line of the display.
    fn prompt_file(&mut self, windows: &mut WindowMap, _usage: u32, fmode: u32) -> Option<String> {
        let verb = if fmode == file_mode::READ {
            "Load from"
        } else {
            "Save to"
        };
        let prompt = format!("{verb} which file? ");
        let (width, height) = self.sizes();
        let columns = width.max(0) as usize;
        let bottom = height - 1;

        // glkterm forces every window to the end before a prompt
        // like this one, so the player is answering a question
        // rather than fighting a pager for the keyboard.
        self.catch_up();

        let saved_typed = std::mem::take(&mut self.typed);
        let saved_typing = self.typing.take();
        let answer = loop {
            let text: String = self
                .typed
                .chars()
                .take(columns.saturating_sub(prompt.chars().count() + 1))
                .collect();
            let line = format!(
                "{prompt}{text}{}",
                " ".repeat(
                    (columns.saturating_sub(1))
                        .saturating_sub(prompt.chars().count() + text.chars().count())
                )
            );
            let parked = (prompt.chars().count() + text.chars().count()) as i64;

            self.place(0, bottom, &[((style::NORMAL, 0), line)]);
            self.finish(Some((parked, bottom)));

            match self.wait(None) {
                // A timer during a file prompt is not an event.
                Waited::Events(_) => continue,
                Waited::Ended => break None,
                Waited::Code(code) if code == key_code::RETURN => {
                    let name = self.typed.trim().to_string();

                    break if name.is_empty() { None } else { Some(name) };
                }
                Waited::Code(code) if code == key_code::ESCAPE => break None,
                Waited::Code(code) => self.edit(code, columns),
            }
        };

        // The interrupted line of play is standing where it was
        // afterwards: the typed line and its window come back, and
        // the tree repaints over the prompt.
        self.typed = saved_typed;
        self.typing = saved_typing;
        self.repaint(windows);

        answer
    }
}

#[cfg(test)]
#[path = "glk_tests.rs"]
mod tests;
