//! The Glk library spoken over the GlkOte protocol, both ways.
//!
//! The Page in `crate::glkote` builds the updates; the composer
//! here walks the library the way a painted display walks it --
//! the same tree, the same boxes -- and feeds the Page plain
//! facts. What belongs to Glk stays here: style numbers become
//! names, terminator keycodes become key names, and the line a
//! request pre-filled is read back out of its buffer.
//!
//! The composer also holds the one identity the protocol demands:
//! GlkOte window ids are minted here, sequentially and never
//! reused (GlkOte: The Windows Update Array), because the windows
//! themselves carry no dispatch-layer identity at all.
//!
//! The display itself is the GlkOteFrontend: a display that
//! suspends rather than blocks. It is never asked for input; serve
//! runs the machine until a select stands waiting, sends the
//! update, and delivers whatever event the far side answers with
//! -- JSON, one stanza to a line, each way (GlkOte: The
//! Application's Life Story).
//!
//! The file prompt is carried: a game's ask for a file suspends
//! the call itself, travels as the protocol's special input, and
//! the player's answer -- a name, or the ever-legitimate cancel --
//! completes the parked call (GlkOte: Special Input Requests). So
//! is the player's half-typed line: every event carries it, and a
//! field that must be made anew takes it as its initial, so an
//! interruption never eats a command in progress (GlkOte: Partial
//! Input).
//!
//! Deliberately not carried yet: the metrics' outspacing and
//! inspacing (the window arrangement leaves no gaps for them).
//! Buffer windows claim their images here: the display lays text
//! around pictures, so the placed pictures and flow breaks travel
//! in the line data (Glk: Graphics in Text Buffer Windows) -- and
//! a display that lost its picture may ask for it whole with the
//! refresh event, answered by an update complete in content.
//!
//! One reshaping from the reference, in the Z wire's standing
//! manner: the reference's face holds its library and the serve
//! halves live on the face -- a cycle Rust refuses, since the
//! library owns the face as its [`Frontend`]. Here the face's
//! state lives behind a cell: the library holds one handle, the
//! serving loop the other, and render and accept take the library
//! (and memory, for the buffers) as arguments. A file answer is
//! handed back as a verdict rather than delivered from inside,
//! because completing it is the machine bridge's parked business.

use std::cell::RefCell;
use std::io::{BufRead, Write};
use std::rc::Rc;

use crate::babel::ifiction;
use crate::errors::VoxamError;
use crate::glkote::json::{Object, Value};
use crate::glkote::{
    LineSpec, Page, Run as PageRun, STYLES, TextRun, WindowSpec, carded, measured, partials,
    read_stanza, write_stanza,
};
use crate::glulx::glk::api::{Glk, Waiting};
use crate::glulx::glk::frontend::{Asked, Frontend};
use crate::glulx::glk::objects::{
    CHARACTER_CELL, Event, Flow, Metrics, Placed, SoundChannel, Window, WindowKind, WindowMap,
    event_type, file_mode, file_usage, key_code, style, to_char,
};
use crate::glulx::glk::resources::{ImageInfo, Resources, pictured};
use crate::glulx::machine::Machine;
use crate::glulx::story::Story;

/// A terminator keycode as the name the protocol can say; Glk's
/// other specials are dropped from the request, which a library
/// may do (Glk: Line Input Events; GlkOte: The Input Update
/// Array).
fn terminator_name(keycode: u32) -> Option<String> {
    if keycode == key_code::ESCAPE {
        return Some("escape".to_string());
    }

    if (key_code::FUNC12..=key_code::FUNC1).contains(&keycode) {
        return Some(format!("func{}", key_code::FUNC1 - keycode + 1));
    }

    None
}

/// The same keys read back: the names a line event's terminator
/// wears; an unnamed ending is an ordinary Return (GlkOte: Input:
/// Accepting User Events).
fn terminator_code(name: &str) -> u32 {
    if name == "escape" {
        return key_code::ESCAPE;
    }

    name.strip_prefix("func")
        .and_then(|number| number.parse::<u32>().ok())
        .filter(|number| (1..=12).contains(number))
        .map_or(0, |number| key_code::FUNC1 - (number - 1))
}

/// A §imagealign value as the protocol's alignment name; a value
/// the library does not recognize draws inlineup, as the spec
/// instructs (Glk: Graphics in Text Buffer Windows; GlkOte: The
/// Line Data Array).
fn alignment_name(alignment: u32) -> &'static str {
    match alignment {
        2 => "inlinedown",
        3 => "inlinecenter",
        4 => "marginleft",
        5 => "marginright",
        _ => "inlineup",
    }
}

/// Glk's full volume, which the wire's own unit gain divides by --
/// a channel may legally ask for more than 1.0, and the display's
/// gain node obliges (Glk: Other Sound Channel Functions).
const FULL_GAIN: u32 = 0x10000;

/// An iFiction card's protocol dress as Glk's own style number,
/// for the runs the doorway courtesy writes into the model.
fn card_style(name: &str) -> u32 {
    match name {
        "header" => style::HEADER,
        "emphasized" => style::EMPHASIZED,
        _ => style::NORMAL,
    }
}

/// A named key of a char event as its Glk keycode; a name from
/// some newer display reads as unknown (GlkOte: Input: Accepting
/// User Events; Glk: Character Input).
fn key_for(name: &str) -> u32 {
    match name {
        "left" => key_code::LEFT,
        "right" => key_code::RIGHT,
        "up" => key_code::UP,
        "down" => key_code::DOWN,
        "return" => key_code::RETURN,
        "delete" => key_code::DELETE,
        "escape" => key_code::ESCAPE,
        "tab" => key_code::TAB,
        "pageup" => key_code::PAGE_UP,
        "pagedown" => key_code::PAGE_DOWN,
        "home" => key_code::HOME,
        "end" => key_code::END,
        _ => name
            .strip_prefix("func")
            .and_then(|number| number.parse::<u32>().ok())
            .filter(|number| (1..=12).contains(number))
            .map_or(key_code::UNKNOWN, |number| key_code::FUNC1 - (number - 1)),
    }
}

/// The highest character a Latin-1 char request can carry (Glk:
/// Character Input).
const LATIN_1_TOP: u32 = 0xFF;

/// The events that never carry the player's partial input: the
/// init by definition, and the kinds the display suppresses it on
/// -- their absence of a partial means nothing (GlkOte: Partial
/// Input).
const NO_PARTIAL: [&str; 4] = ["init", "specialresponse", "refresh", "debuginput"];

/// A file prompt's dress in the protocol's names: Glk's file modes
/// and usages, spelled the way specialinput spells them (GlkOte:
/// Special Input Requests). A mode outside the four is refused the
/// way the file streams refuse it.
fn file_mode_name(fmode: u32) -> Option<&'static str> {
    match fmode {
        file_mode::READ => Some("read"),
        file_mode::WRITE => Some("write"),
        file_mode::READ_WRITE => Some("readwrite"),
        file_mode::WRITE_APPEND => Some("writeappend"),
        _ => None,
    }
}

fn file_kind_name(usage: u32) -> &'static str {
    match usage & file_usage::TYPE_MASK {
        file_usage::SAVED_GAME => "save",
        file_usage::TRANSCRIPT => "transcript",
        file_usage::INPUT_RECORD => "command",
        _ => "data",
    }
}

fn glkote_error(message: String) -> VoxamError {
    VoxamError::GlkOte(message)
}

fn glk_error(message: String) -> VoxamError {
    VoxamError::GlulxGlk(message)
}

use crate::glulx::memory::Memory;

/// What the display measured and granted, in a cell of its own.
///
/// The library asks for these mid-call -- display_resized re-lays
/// the tree with metrics_for while accept still holds the face --
/// so they live apart from the face's working state, and the
/// trait's getters borrow only this.
#[derive(Clone, Copy)]
struct Claims {
    size: Option<(i64, i64)>,
    grid_cell: Metrics,
    buffer_cell: Metrics,
    graphics_cell: Metrics,
    timer_input: bool,
    graphics: bool,
    buffer_images: bool,
    hyperlink_input: bool,
    sound: bool,
}

impl Default for Claims {
    fn default() -> Self {
        Self {
            size: None,
            grid_cell: CHARACTER_CELL,
            buffer_cell: CHARACTER_CELL,
            graphics_cell: CHARACTER_CELL,
            timer_input: false,
            graphics: false,
            buffer_images: false,
            hyperlink_input: false,
            sound: false,
        }
    }
}

/// Reads a Glk library into a Page, one cycle per flush.
///
/// The buffer drain makes the composer the display's sole reader:
/// take_content empties what it reports, so no painted display can
/// share the same library.
#[derive(Default)]
pub struct Composer {
    idents: Vec<(u32, i64)>,
    next: i64,
}

impl Composer {
    /// Open with no windows known and the first id unminted.
    pub fn new() -> Self {
        Self {
            idents: Vec::new(),
            next: 1,
        }
    }

    /// Feed the library's whole face to the Page, one cycle.
    ///
    /// Pairs and blanks stay home -- the protocol's window list is
    /// flat and knows only the three drawn kinds (GlkOte: The
    /// Windows Update Array) -- and a window gone from the tree
    /// goes undeclared, which is how the Page learns it closed.
    pub fn compose(
        &mut self,
        glk: &mut Glk,
        memory: &Memory,
        page: &mut Page,
    ) -> Result<(), VoxamError> {
        for key in visible(&glk.windows, glk.root) {
            let ident = self.ident(key);
            let window = glk.windows.get_mut(&key).expect("a visible window");

            match &window.kind {
                WindowKind::Grid(_) => grid(page, ident, window)?,
                WindowKind::Buffer(_) => buffer(page, ident, window)?,
                _ => graphics(page, ident, window)?,
            }

            asked(page, ident, window, memory)?;
        }

        page.timer(i64::from(glk.timer_interval), false);

        // A closed window's memo goes with it; the counter never
        // rewinds, so its id stays retired.
        self.idents.retain(|(key, _)| glk.windows.contains_key(key));

        Ok(())
    }

    /// The window's GlkOte id, minted on first sight.
    ///
    /// Public because the display's other half needs it too: a
    /// drawing operation names its window before the cycle that
    /// declares it.
    pub fn ident(&mut self, window: u32) -> i64 {
        if let Some((_, ident)) = self.idents.iter().find(|(key, _)| *key == window) {
            return *ident;
        }

        let minted = self.next;

        self.next += 1;
        self.idents.push((window, minted));

        minted
    }

    /// The window an id names, while it lives; None after.
    pub fn window_for(&self, ident: i64) -> Option<u32> {
        self.idents
            .iter()
            .find(|(_, held)| *held == ident)
            .map(|(key, _)| *key)
    }
}

/// Declare a grid and feed its whole face; the Page diffs.
fn grid(page: &mut Page, ident: i64, window: &Window) -> Result<(), VoxamError> {
    page.window(
        ident,
        "grid",
        i64::from(window.rock),
        window.bbox,
        WindowSpec {
            gridsize: Some((window.width(), window.height())),
            ..WindowSpec::default()
        },
    )?;

    let WindowKind::Grid(data) = &window.kind else {
        unreachable!("matched above");
    };

    let rows: Vec<Vec<TextRun>> = (0..data.lines.len())
        .map(|index| grouped(&data.lines[index], &data.styles[index], &data.links[index]))
        .collect();

    page.grid(ident, &rows)
}

/// Declare a buffer and drain its new flow into the Page.
fn buffer(page: &mut Page, ident: i64, window: &mut Window) -> Result<(), VoxamError> {
    page.window(
        ident,
        "buffer",
        i64::from(window.rock),
        window.bbox,
        WindowSpec::default(),
    )?;

    let clear = window.pending_clear;

    window.pending_clear = false;

    let flow = window.take_content();

    if !flow.is_empty() || clear {
        let runs: Vec<PageRun> = flow.iter().map(flowed).collect();

        page.buffer(ident, &runs, clear)?;
    }

    Ok(())
}

/// Declare a graphics window by its drawable size.
///
/// The canvas's own pending clear stays untouched: a clear is a
/// fill with the background color, and that color lives with the
/// display that draws, not with the model.
fn graphics(page: &mut Page, ident: i64, window: &Window) -> Result<(), VoxamError> {
    let cell = window.metrics;

    page.window(
        ident,
        "graphics",
        i64::from(window.rock),
        window.bbox,
        WindowSpec {
            graphsize: Some((
                ((window.width() as f64 - cell.margin_x) as i64).max(0),
                ((window.height() as f64 - cell.margin_y) as i64).max(0),
            )),
            ..WindowSpec::default()
        },
    )
}

/// Translate a window's outstanding requests, if any.
///
/// Clicks are suppressed for buffers -- "buffer windows do not
/// support mouse-click input" (GlkOte: The Input Update Array) --
/// and grid input carries the cursor, clamped into the grid the
/// way the painted displays clamp it.
fn asked(page: &mut Page, ident: i64, window: &Window, memory: &Memory) -> Result<(), VoxamError> {
    let linked = window.hyperlink_request;
    let clicked = window.mouse_request && !matches!(window.kind, WindowKind::Buffer(_));
    let cursor = caret(window);

    if let Some(request) = &window.line_request {
        page.line_input(
            ident,
            i64::from(request.capacity()),
            LineSpec {
                initial: initial_text(request, memory)?,
                terminators: request
                    .terminators
                    .iter()
                    .filter_map(|keycode| terminator_name(*keycode))
                    .collect(),
                cursor,
                hyperlink: linked,
                mouse: clicked,
                ..LineSpec::default()
            },
        )
    } else if window.char_request {
        page.char_input(ident, cursor, linked, clicked)
    } else {
        page.passive_input(ident, linked, clicked)
    }
}

/// The verdict accept hands the serving loop beside its event.
#[derive(Debug, PartialEq, Eq)]
pub enum Accepted {
    /// An event to deliver to the suspended select.
    Event(Event),
    /// A file answer to hand the parked call -- the machine
    /// bridge's business, so it travels up rather than completing
    /// here.
    File(Option<String>),
    /// The stanza asked for nothing.
    Nothing,
}

/// The display at the far end of the protocol.
///
/// A display that suspends rather than blocks: it is never asked
/// for input, and its flush is a deliberate no-op -- a select can
/// flush more than once between updates, so nothing composes until
/// render gathers the whole cycle at once. Its capabilities are
/// not its own to claim: the init event's support list says what
/// the far side can show, and the claims follow it (GlkOte: Input:
/// Accepting User Events).
pub struct GlkOteFrontend {
    /// The display's picture of the session, update by update.
    pub page: Page,
    /// The window-identity ledger the protocol demands.
    pub composer: Composer,
    claims: Rc<RefCell<Claims>>,
    // The buffered drawing ops, by internal window key, in call
    // order.
    ops: Vec<(u32, Vec<Object>)>,
    restarted: bool,
    covered: bool,
    refresh_owed: bool,
    sound_ops: Vec<Object>,
    channel_idents: Vec<(u32, i64)>,
    next_channel: i64,
}

impl Default for GlkOteFrontend {
    fn default() -> Self {
        Self::new()
    }
}

impl GlkOteFrontend {
    /// Open unattached and unmeasured, before any init.
    pub fn new() -> Self {
        Self {
            page: Page::new(),
            composer: Composer::new(),
            claims: Rc::new(RefCell::new(Claims::default())),
            ops: Vec::new(),
            restarted: false,
            covered: false,
            refresh_owed: false,
            sound_ops: Vec::new(),
            channel_idents: Vec::new(),
            next_channel: 1,
        }
    }

    /// Open the session on the init event's word.
    ///
    /// The support list grants the capabilities: graphicswin for
    /// canvases, bare graphics for pictures set into a buffer's
    /// text flow -- a display that grants it really does lay text
    /// around them -- timer for timers, hyperlinks for links
    /// (GlkOte: Input: Accepting User Events). Fails when the
    /// metrics carry no size.
    pub fn begin(&mut self, stanza: &Object) -> Result<(), VoxamError> {
        let support: Vec<String> = match stanza.get("support") {
            Some(Value::List(held)) => held
                .iter()
                .filter_map(|word| word.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        let supports = |word: &str| support.iter().any(|held| held == word);

        {
            let mut claims = self.claims.borrow_mut();

            claims.timer_input = supports("timer");
            claims.graphics = supports("graphicswin");
            claims.buffer_images = supports("graphics");
            claims.hyperlink_input = supports("hyperlinks");
            // Sound is VΘXΔM's own dialect word: only a display
            // that says it -- our pages say it -- gets channels
            // opened over it, and every other display keeps the
            // conforming quiet.
            claims.sound = supports("sound");
        }

        self.measure(stanza)
    }

    /// Take the display's size and cells from its metrics.
    ///
    /// Every arrange carries a complete metrics object, so this
    /// replaces rather than amends (GlkOte: The Metrics Object).
    fn measure(&mut self, stanza: &Object) -> Result<(), VoxamError> {
        let metrics = match stanza.get("metrics") {
            Some(Value::Object(held)) => held.clone(),
            _ => Object::new(),
        };
        let width = metrics.get("width").and_then(Value::as_float);
        let height = metrics.get("height").and_then(Value::as_float);

        let (Some(width), Some(height)) = (width, height) else {
            return Err(glkote_error(
                "the display's metrics carry no size (GlkOte: The Metrics Object)".into(),
            ));
        };

        let mut claims = self.claims.borrow_mut();

        claims.size = Some((width as i64, height as i64));
        claims.grid_cell = cell(&metrics, "grid");
        claims.buffer_cell = cell(&metrics, "buffer");

        // A canvas's unit is the pixel itself; only the margins
        // come from the metrics.
        let edged = cell(&metrics, "graphics");

        claims.graphics_cell = Metrics::new(1.0, 1.0, edged.margin_x, edged.margin_y);

        Ok(())
    }

    /// The display in pixels, as its init declared. Fails before
    /// any init has been accepted.
    pub fn size(&self) -> Result<(i64, i64), VoxamError> {
        self.claims
            .borrow()
            .size
            .ok_or_else(|| glkote_error("the display has not spoken its init yet".into()))
    }

    /// Note that the cadence was set anew.
    ///
    /// Even the same interval restarts the clock when re-asked
    /// (Glk: Timer Events), which polled state cannot show; render
    /// carries the restart through.
    fn note_restart(&mut self) {
        self.restarted = true;
    }

    // -- the drawing ops, buffered until render ----------------------------

    fn ops_for(&mut self, window: u32) -> &mut Vec<Object> {
        if let Some(index) = self.ops.iter().position(|(key, _)| *key == window) {
            return &mut self.ops[index].1;
        }

        self.ops.push((window, Vec::new()));

        &mut self.ops.last_mut().expect("just pushed").1
    }

    /// Emit a canvas's pending clear ahead of anything else.
    ///
    /// A clear is a whole-window fill in the background color the
    /// window had at the time -- which is the colorless fill,
    /// since the display's default fill color is exactly that
    /// background (GlkOte: Graphics Window Updates). It must land
    /// before later draws, and before any change of background.
    fn settled(&mut self, windows: &mut WindowMap, window: u32) {
        let Some(held) = windows.get_mut(&window) else {
            return;
        };

        if held.pending_clear {
            held.pending_clear = false;

            let mut fill = Object::new();

            fill.set("special", "fill");
            self.ops_for(window).push(fill);
        }
    }

    /// The channel's wire ident, minted once and never reused.
    fn channeled(&mut self, channel: u32) -> i64 {
        if let Some((_, ident)) = self.channel_idents.iter().find(|(key, _)| *key == channel) {
            return *ident;
        }

        let minted = self.next_channel;

        self.next_channel += 1;
        self.channel_idents.push((channel, minted));

        minted
    }

    /// A play finished naturally on the display.
    ///
    /// The channel falls silent in the model, as the painted
    /// spine's listener clears it, and a play that asked for
    /// notification comes home as the completion glk_select
    /// promises -- a zero notify was never an event, only the
    /// model's own bookkeeping (Glk: Playing Sounds).
    fn sound_ended(&mut self, glk: &mut Glk, stanza: &Object) -> Option<Event> {
        let ended = stanza.get("sound").and_then(Value::as_int).unwrap_or(0);
        let notify = stanza.get("notify").and_then(Value::as_int).unwrap_or(0);
        let named = stanza.get("channel").and_then(Value::as_int);

        for (key, minted) in &self.channel_idents {
            if Some(*minted) == named
                && let Some(held) = glk.channels.get_mut(key)
                && i64::from(held.sound) == ended
            {
                held.sound = 0;
            }
        }

        if notify == 0 {
            return None;
        }

        Some(Event::new(
            event_type::SOUND_NOTIFY,
            None,
            ended as u32,
            notify as u32,
        ))
    }

    // -- the two halves of the conversation --------------------------------

    /// Stand the cover and the record's card at the first buffer.
    ///
    /// The doorway courtesy, over the wire: shown once, before
    /// whatever the game has already written -- the Fspc cover
    /// when the display grants bare graphics, then the iFiction
    /// card, which is only text and needs no grant (Blorb:
    /// Frontispiece Chunk; Babel: The iFiction format). A tree
    /// with no buffer window yet waits for one; art and
    /// bibliography are courtesies, never gates, so a session with
    /// neither simply plays on.
    fn front(&mut self, glk: &mut Glk) {
        if self.covered {
            return;
        }

        let mut opening: Vec<Flow> = Vec::new();
        let cover = if self.claims.borrow().buffer_images {
            glk.resources.frontispiece().cloned()
        } else {
            None
        };

        if let Some(cover) = cover {
            opening.push(Flow::Placed(Placed {
                image: cover.number,
                url: pictured(&cover),
                width: cover.width,
                height: cover.height,
                alignment: 1,
                hyperlink: 0,
            }));
            opening.push(Flow::Run {
                style: style::NORMAL,
                hyperlink: 0,
                text: "\n".to_string(),
            });
        }

        let record = glk
            .resources
            .blorb
            .as_ref()
            .and_then(|blorb| blorb.ifiction.as_deref())
            .and_then(ifiction);

        if let Some(record) = record {
            for (name, text) in carded(&record) {
                opening.push(Flow::Run {
                    style: card_style(&name),
                    hyperlink: 0,
                    text,
                });
            }
        }

        if opening.is_empty() {
            self.covered = true;

            return;
        }

        let window = glk
            .window_order
            .iter()
            .rev()
            .find(|key| matches!(glk.windows[key].kind, WindowKind::Buffer(_)))
            .copied();

        let Some(window) = window else {
            return;
        };

        if let WindowKind::Buffer(data) = &mut glk.windows.get_mut(&window).expect("found").kind {
            data.content.splice(0..0, opening);
        }

        self.covered = true;
    }

    /// Compose everything since the last update into a stanza.
    ///
    /// The buffered drawing goes first -- dropped outright for a
    /// window that closed before its draws could show, which is
    /// also why the ops never touch the Page mid-run -- then the
    /// composer reads the tree, then a timer restart is re-fed,
    /// since the composer's own polled feeding cannot carry one.
    pub fn render(
        &mut self,
        glk: &mut Glk,
        memory: &Memory,
        exit: bool,
    ) -> Result<Object, VoxamError> {
        self.front(glk);
        self.ops.retain(|(key, _)| glk.windows.contains_key(key));

        // A canvas cleared and then left alone still owes the
        // display its fill.
        let canvases: Vec<u32> = glk
            .windows
            .iter()
            .filter(|(_, held)| matches!(held.kind, WindowKind::Graphics(_)))
            .map(|(key, _)| *key)
            .collect();

        for key in canvases {
            self.settled(&mut glk.windows, key);
        }

        // The composer walks first, so ids mint in tree order; the
        // Page takes the ops in any order before the update.
        self.composer.compose(glk, memory, &mut self.page)?;

        let ops = std::mem::take(&mut self.ops);

        for (window, held) in ops {
            let ident = self.composer.ident(window);

            self.page.draw(ident, held)?;
        }

        if !self.sound_ops.is_empty() {
            let held = std::mem::take(&mut self.sound_ops);

            self.page.sounds(held);
        }

        if self.restarted {
            self.page.timer(i64::from(glk.timer_interval), true);

            self.restarted = false;
        }

        if let Some(Waiting::Prompt { usage, fmode, .. }) = glk.waiting {
            let Some(mode) = file_mode_name(fmode) else {
                return Err(glk_error(format!(
                    "a file cannot be prompted for in mode {fmode}"
                )));
            };

            self.page.prompt(mode, file_kind_name(usage))?;
        }

        let refresh = std::mem::replace(&mut self.refresh_owed, false);

        self.page.update(exit, refresh)
    }

    /// Translate one inbound stanza into the verdict it means.
    ///
    /// Nothing means the stanza asks for nothing here: a stale
    /// generation the protocol says to ignore (GlkOte: The
    /// Generation Number), or a kind this face does not carry --
    /// external, debuginput. Fails for a window this session never
    /// showed, and for input no request stands to receive --
    /// unreachable from a conforming display, whose generations
    /// shield every withdrawal.
    pub fn accept(
        &mut self,
        glk: &mut Glk,
        memory: &mut Memory,
        stanza: &Object,
    ) -> Result<Accepted, VoxamError> {
        let kind = stanza
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        if !NO_PARTIAL.contains(&kind.as_str()) {
            // The player's half-typed lines ride every event that
            // can carry them, a stale one included -- the typing
            // is current even when the event is not.
            self.page.typed(partials(stanza.get("partial")));
        }

        if kind == "refresh" {
            // The display lost its picture and asks for it whole
            // -- ahead of the generation gate, since a refreshing
            // display is out of sync by definition (GlkOte: the
            // refresh input event). The game hears the spec's own
            // redraw for every canvas; the next render carries the
            // rest.
            self.refresh_owed = true;

            return Ok(Accepted::Event(Event::new(event_type::REDRAW, None, 0, 0)));
        }

        if stanza.get("gen").and_then(Value::as_int) != Some(self.page.generation()) {
            return Ok(Accepted::Nothing);
        }

        match kind.as_str() {
            "line" => {
                let terminator = stanza
                    .get("terminator")
                    .and_then(Value::as_str)
                    .map_or(0, terminator_code);
                let window = self.window(stanza)?;
                let value = stringy(stanza.get("value"));

                crate::glulx::bridge::plain(glk.deliver_line(memory, window, &value, terminator))
                    .map(Accepted::Event)
            }
            "char" => {
                let (window, code) = self.keyed(glk, stanza)?;

                crate::glulx::bridge::plain(glk.deliver_char(window, code)).map(Accepted::Event)
            }
            "mouse" => {
                let window = self.window(stanza)?;
                let x = stanza.get("x").and_then(Value::as_int).unwrap_or(0) as u32;
                let y = stanza.get("y").and_then(Value::as_int).unwrap_or(0) as u32;

                crate::glulx::bridge::plain(glk.deliver_mouse(window, x, y)).map(Accepted::Event)
            }
            "hyperlink" => {
                let window = self.window(stanza)?;
                let value = stanza.get("value").and_then(Value::as_int).unwrap_or(0) as u32;

                crate::glulx::bridge::plain(glk.deliver_hyperlink(window, value))
                    .map(Accepted::Event)
            }
            "timer" => Ok(Accepted::Event(Event::new(event_type::TIMER, None, 0, 0))),
            "sound" => Ok(match self.sound_ended(glk, stanza) {
                Some(event) => Accepted::Event(event),
                None => Accepted::Nothing,
            }),
            "redraw" => {
                // An unnamed window means every canvas, which Glk
                // spells as the null window (Glk: Window Events).
                let named = if stanza.get("window").is_some() {
                    Some(self.window(stanza)?)
                } else {
                    None
                };

                Ok(Accepted::Event(Event::new(event_type::REDRAW, named, 0, 0)))
            }
            "arrange" => self.rearranged(glk, stanza).map(Accepted::Event),
            "specialresponse" => Ok(self.answered(glk, stanza)),
            _ => Ok(Accepted::Nothing),
        }
    }

    /// Take a special response: the player's file name, or not.
    ///
    /// Completing the parked call is the machine bridge's
    /// business, so the answer travels up as a verdict; a response
    /// to some other ask leaves the wait standing (GlkOte: Special
    /// Input Requests). A non-string value would be a browser
    /// dialog's fileref object, and no dialog was invited: it
    /// reads as a cancel, which is always legitimate.
    fn answered(&mut self, glk: &Glk, stanza: &Object) -> Accepted {
        if stanza.get("response").and_then(Value::as_str) != Some("fileref_prompt") {
            return Accepted::Nothing;
        }

        if !matches!(glk.waiting, Some(Waiting::Prompt { .. })) {
            return Accepted::Nothing;
        }

        Accepted::File(
            stanza
                .get("value")
                .and_then(Value::as_str)
                .map(str::to_string),
        )
    }

    /// Take an arrange event: new metrics, then the re-lay.
    ///
    /// The re-lay may queue redraws for moved canvases before the
    /// arrange event lands last in the queue -- so the arrange is
    /// taken from the end, and the redraws drain through the next
    /// selects in their natural order.
    fn rearranged(&mut self, glk: &mut Glk, stanza: &Object) -> Result<Event, VoxamError> {
        self.measure(stanza)?;
        glk.display_resized();

        glk.pending_events
            .pop()
            .ok_or_else(|| glk_error("the re-lay queued no arrange event".into()))
    }

    /// The window an event names. Fails for an id this session
    /// never showed.
    fn window(&self, stanza: &Object) -> Result<u32, VoxamError> {
        let ident = stanza.get("window").and_then(Value::as_int);
        let window = ident.and_then(|held| self.composer.window_for(held));

        window.ok_or_else(|| {
            glkote_error(format!(
                "no window is numbered {}",
                ident.map_or("None".to_string(), |held| held.to_string())
            ))
        })
    }

    /// A char event's window and Glk character code.
    ///
    /// A literal character beyond Latin-1 lands as the unknown key
    /// when the request was not a Unicode one -- the request
    /// cannot carry it (Glk: Character Input).
    fn keyed(&self, glk: &Glk, stanza: &Object) -> Result<(u32, u32), VoxamError> {
        let window = self.window(stanza)?;

        if let Some(Value::Str(value)) = stanza.get("value")
            && value.chars().count() == 1
        {
            let mut code = u32::from(value.chars().next().expect("one character"));

            if code > LATIN_1_TOP
                && !glk
                    .windows
                    .get(&window)
                    .is_some_and(|held| held.char_unicode)
            {
                code = key_code::UNKNOWN;
            }

            return Ok((window, code));
        }

        Ok((window, key_for(&stringy(stanza.get("value")))))
    }
}

/// The face's shareable half: the library's [`Frontend`] handle.
///
/// Made with [`shared`], which clones both cells: the working face
/// and the claims the library may read mid-call.
pub struct SharedFace {
    face: Rc<RefCell<GlkOteFrontend>>,
    claims: Rc<RefCell<Claims>>,
}

/// The library's handle on a face.
pub fn shared(face: &Rc<RefCell<GlkOteFrontend>>) -> SharedFace {
    let claims = face.borrow().claims.clone();

    SharedFace {
        face: face.clone(),
        claims,
    }
}

impl Frontend for SharedFace {
    fn timer_input(&self) -> bool {
        self.claims.borrow().timer_input
    }

    /// Clicks have no support token: they are core GlkOte.
    fn mouse_input(&self) -> bool {
        true
    }

    fn hyperlink_input(&self) -> bool {
        self.claims.borrow().hyperlink_input
    }

    fn graphics(&self) -> bool {
        self.claims.borrow().graphics
    }

    fn buffer_images(&self) -> bool {
        self.claims.borrow().buffer_images
    }

    fn sound(&self) -> bool {
        self.claims.borrow().sound
    }

    fn suspends(&self) -> bool {
        true
    }

    /// The cell for a window's kind, from the init's metrics.
    ///
    /// Pairs and blanks are asked too, when the tree re-lays;
    /// their spans are pixels pure and simple.
    fn metrics_for(&self, window: &Window) -> Metrics {
        let claims = self.claims.borrow();

        match window.kind {
            WindowKind::Grid(_) => claims.grid_cell,
            WindowKind::Buffer(_) => claims.buffer_cell,
            WindowKind::Graphics(_) => claims.graphics_cell,
            _ => CHARACTER_CELL,
        }
    }

    /// The display in pixels, as its init declared; the classic
    /// 80x24 before any init, though serve never lets a library
    /// lay out before begin.
    fn size(&self) -> (i64, i64) {
        self.claims.borrow().size.unwrap_or((80, 24))
    }

    /// Deliberately nothing: render gathers the whole cycle.
    fn flush(&mut self, _windows: &mut WindowMap, _root: Option<u32>) {}

    /// Never asked: a suspending display records the wait instead.
    fn read_line(
        &mut self,
        _windows: &mut WindowMap,
        _window: u32,
        _maxlen: u32,
    ) -> Asked<(String, u32)> {
        unreachable!("a suspending display is never asked for a line")
    }

    /// Never asked, as read_line is never asked.
    fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
        unreachable!("a suspending display is never asked for a keystroke")
    }

    fn set_timer(&mut self, _millisecs: u32) {
        self.face.borrow_mut().note_restart();
    }

    /// Set the color future clears and plain fills wear.
    fn set_background_color(&mut self, windows: &mut WindowMap, window: u32, color: u32) {
        let mut face = self.face.borrow_mut();

        face.settled(windows, window);

        let mut op = Object::new();

        op.set("special", "setcolor");
        op.set("color", css(color));
        face.ops_for(window).push(op);
    }

    /// Fill a rectangle with a color.
    #[allow(clippy::too_many_arguments)] // the fill call's own shape
    fn fill_rect(
        &mut self,
        windows: &mut WindowMap,
        window: u32,
        color: u32,
        left: i64,
        top: i64,
        width: u32,
        height: u32,
    ) {
        let mut face = self.face.borrow_mut();

        face.settled(windows, window);

        let mut op = Object::new();

        op.set("special", "fill");
        op.set("color", css(color));
        op.set("x", left);
        op.set("y", top);
        op.set("width", i64::from(width));
        op.set("height", i64::from(height));
        face.ops_for(window).push(op);
    }

    /// Erase a rectangle to the background.
    ///
    /// A fill with no color named fills with the display's default
    /// fill color -- the background, exactly (GlkOte: Graphics
    /// Window Updates).
    fn erase_rect(
        &mut self,
        windows: &mut WindowMap,
        window: u32,
        left: i64,
        top: i64,
        width: u32,
        height: u32,
    ) {
        let mut face = self.face.borrow_mut();

        face.settled(windows, window);

        let mut op = Object::new();

        op.set("special", "fill");
        op.set("x", left);
        op.set("y", top);
        op.set("width", i64::from(width));
        op.set("height", i64::from(height));
        face.ops_for(window).push(op);
    }

    /// Draw a picture on a canvas or into a buffer's flow.
    ///
    /// The operation names the Pict by number and carries the
    /// picture whole as a data: url beside it (GlkOte: Graphics
    /// Window Updates): a host with a Blorb of its own may keep
    /// resolving numbers the way GiLoad does, and a host with none
    /// -- the desktop shell's webview -- draws from the update
    /// alone. A buffer takes the picture into its flow instead:
    /// val1 is the §imagealign value and val2 means nothing there,
    /// and the link value it is drawn under rides along, so a
    /// clickable picture stays clickable (Glk: Graphics in Text
    /// Buffer Windows).
    #[allow(clippy::too_many_arguments)] // the drawing call's own shape
    fn draw_image(
        &mut self,
        windows: &mut WindowMap,
        window: u32,
        image: &ImageInfo,
        val1: i64,
        val2: i64,
        width: u32,
        height: u32,
        hyperlink: u32,
    ) -> bool {
        let Some(held) = windows.get_mut(&window) else {
            return false;
        };

        if matches!(held.kind, WindowKind::Buffer(_)) {
            // Only under the display's own grant: the refusal here
            // matches the gestalt's answer exactly.
            if !self.claims.borrow().buffer_images {
                return false;
            }

            held.put_placed(Placed {
                image: image.number,
                url: pictured(image),
                width,
                height,
                alignment: val1.clamp(0, i64::from(u32::MAX)) as u32,
                hyperlink,
            });

            return true;
        }

        if !matches!(held.kind, WindowKind::Graphics(_)) {
            return false;
        }

        let mut face = self.face.borrow_mut();

        face.settled(windows, window);

        let mut op = Object::new();

        op.set("special", "image");
        op.set("image", i64::from(image.number));
        op.set("url", pictured(image));
        op.set("x", val1);
        op.set("y", val2);
        op.set("width", i64::from(width));
        op.set("height", i64::from(height));
        face.ops_for(window).push(op);

        true
    }

    /// Set a flow break into a buffer's flow.
    ///
    /// Text past the break starts below any margin images standing
    /// at the point of the break; any other window shrugs it off,
    /// as the spec allows (Glk: Graphics in Text Buffer Windows).
    fn flow_break(&mut self, windows: &mut WindowMap, window: u32) {
        if let Some(held) = windows.get_mut(&window)
            && matches!(held.kind, WindowKind::Buffer(_))
        {
            held.put_break();
        }
    }

    /// Start a sound on its own wire channel.
    ///
    /// The play op carries the sound whole as a data: url in a
    /// container the display's audio engine decodes, the repeat
    /// count with -1 for until-stopped, the notify value whose
    /// completion comes back as a sound event, and the channel's
    /// own volume as a unit gain (Glk: Playing Sounds). A sound no
    /// wire container can carry -- MOD music -- refuses here, and
    /// the music gestalt already said so.
    fn play_sound(
        &mut self,
        resources: &mut Resources,
        channel: u32,
        snapshot: &SoundChannel,
        sound: u32,
        repeats: u32,
        notify: u32,
    ) -> bool {
        let Some(url) = resources.audible(sound) else {
            return false;
        };

        let mut face = self.face.borrow_mut();
        let ident = face.channeled(channel);
        let mut op = Object::new();

        op.set("channel", ident);
        op.set("op", "play");
        op.set("sound", i64::from(sound));
        op.set("url", url);
        op.set(
            "repeats",
            if repeats == u32::MAX {
                -1i64
            } else {
                i64::from(repeats)
            },
        );
        op.set("notify", i64::from(notify));
        op.set("volume", f64::from(snapshot.volume) / f64::from(FULL_GAIN));
        face.sound_ops.push(op);

        true
    }

    /// Silence a channel (Glk: Playing Sounds).
    fn stop_sound(&mut self, channel: u32, _snapshot: &SoundChannel) {
        let mut face = self.face.borrow_mut();
        let ident = face.channeled(channel);
        let mut op = Object::new();

        op.set("channel", ident);
        op.set("op", "stop");
        face.sound_ops.push(op);
    }

    /// Pause as silence; resume as starting over.
    ///
    /// The painted spine's own semantics, kept for parity: no
    /// display here tracks a playback position, so an unpaused
    /// channel plays its sound again from the start, and neither
    /// edge is a natural ending (Glk: Playing Sounds). A channel
    /// with nothing sounding shrugs both edges off, and no op
    /// rides the wire for it.
    fn pause_sound(
        &mut self,
        resources: &mut Resources,
        channel: u32,
        snapshot: &SoundChannel,
        paused: bool,
    ) {
        if snapshot.sound == 0 {
            return;
        }

        if paused {
            self.stop_sound(channel, snapshot);
        } else {
            self.play_sound(
                resources,
                channel,
                snapshot,
                snapshot.sound,
                snapshot.repeats,
                snapshot.notify,
            );
        }
    }

    /// Change a sounding channel's gain, fading over a duration.
    ///
    /// Better than the speaker's next-play-only honesty: the
    /// display's gain node ramps live, so the extended form's fade
    /// means what it says (Glk: Other Sound Channel Functions).
    fn set_volume(&mut self, channel: u32, _snapshot: &SoundChannel, volume: u32, duration: u32) {
        let mut face = self.face.borrow_mut();
        let ident = face.channeled(channel);
        let mut op = Object::new();

        op.set("channel", ident);
        op.set("op", "volume");
        op.set("volume", f64::from(volume) / f64::from(FULL_GAIN));
        op.set("duration", i64::from(duration));
        face.sound_ops.push(op);
    }
}

/// Drive one session over the protocol, stanza by stanza.
///
/// JSON lines both ways: the conversation opens with the display's
/// init, and thereafter the machine runs to a suspension, the
/// update goes out, and the display's answer comes back and is
/// delivered -- the intermittent burst model (GlkOte: The
/// Application's Life Story). Every inbound stanza is owed a
/// response, so one that asks for nothing is answered with the
/// pass stanza rather than silence, which would starve a lockstep
/// display (GlkOte: Output: Updating the Display).
///
/// True is a session that ended cleanly -- the story quit, or the
/// display hung up. A broken conversation answers the protocol's
/// own error stanza and is false.
pub fn serve(
    machine: &mut Machine,
    face: &Rc<RefCell<GlkOteFrontend>>,
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
) -> bool {
    match served(machine, face, reader, writer) {
        Ok(clean) => clean,
        Err(VoxamError::GlkOteJson(message)) => {
            error_stanza(writer, &format!("voxam: not JSON: {message}"));

            false
        }
        Err(error) => {
            error_stanza(writer, &format!("voxam: {error}"));

            false
        }
    }
}

/// The serving loop itself, its failures still errors.
fn served(
    machine: &mut Machine,
    face: &Rc<RefCell<GlkOteFrontend>>,
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
) -> Result<bool, VoxamError> {
    let opening = read_stanza(reader)?;
    let opening = opening.filter(|held| held.get("type").and_then(Value::as_str) == Some("init"));

    let Some(opening) = opening else {
        return Err(glkote_error(
            "the conversation opens with an init event (GlkOte: The Application's Life Story)"
                .into(),
        ));
    };

    face.borrow_mut().begin(&opening)?;

    loop {
        machine.run(None)?;

        let running = machine.running();
        let update = {
            let (glk, memory) = attached(machine)?;

            face.borrow_mut().render(glk, memory, !running)?
        };

        write_stanza(writer, &update);

        if !running {
            return Ok(true);
        }

        loop {
            let Some(stanza) = read_stanza(reader)? else {
                // The display hung up: the session ends the way a
                // closed window ends it.
                return Ok(true);
            };

            let verdict = {
                let (glk, memory) = attached(machine)?;

                face.borrow_mut().accept(glk, memory, &stanza)?
            };

            match verdict {
                Accepted::Event(event) => {
                    machine.deliver_event(event)?;

                    break;
                }
                Accepted::File(name) => {
                    // The stanza itself completes the wait: the
                    // answer stores through the parked call, and
                    // the machine can simply step on.
                    machine.deliver_file(name.as_deref())?;

                    break;
                }
                Accepted::Nothing => {
                    let cleared = machine.glk_mut().is_none_or(|glk| glk.waiting.is_none());

                    if cleared {
                        break;
                    }

                    let mut pass = Object::new();

                    pass.set("type", "pass");
                    write_stanza(writer, &pass);
                }
            }
        }
    }
}

/// The machine's library and memory, both in hand.
fn attached(
    machine: &mut Machine,
) -> Result<(&mut Glk, &mut crate::glulx::memory::Memory), VoxamError> {
    machine
        .glk_and_memory_mut()
        .ok_or_else(|| glkote_error("the display is not attached to a library".into()))
}

/// One error stanza out: the protocol's own answer to a broken
/// conversation.
fn error_stanza(writer: &mut dyn Write, message: &str) {
    let mut stanza = Object::new();

    stanza.set("type", "error");
    stanza.set("message", message);
    write_stanza(writer, &stanza);
}

/// A wire session's fixed parts, for hosts and drills: the machine
/// with its library installed over a shared face.
pub fn opened(
    story: Story,
    blorb: Option<crate::blorb::Blorb>,
    seed: Option<u32>,
) -> Result<(Machine, Rc<RefCell<GlkOteFrontend>>), VoxamError> {
    let face = Rc::new(RefCell::new(GlkOteFrontend::new()));
    let mut library = Glk::new(Box::new(shared(&face)));

    library.resources = Resources::new(blorb);

    let mut machine = Machine::new(story, seed)?;

    machine.install_glk(library);

    Ok((machine, face))
}

/// A Glk color word as the CSS string the protocol draws with.
///
/// Masked to its low three bytes: a color is 0x00RRGGBB (Glk:
/// Suggesting Colors of Styles).
fn css(color: u32) -> String {
    format!("#{:06X}", color & 0xFF_FFFF)
}

/// One window kind's cell, the shared measure worn as Metrics.
fn cell(metrics: &Object, prefix: &str) -> Metrics {
    let (width, height, margin_x, margin_y) = measured(metrics, prefix);

    Metrics::new(width, height, margin_x, margin_y)
}

/// The drawn windows of a tree, in tree order.
fn visible(windows: &WindowMap, root: Option<u32>) -> Vec<u32> {
    let Some(key) = root else {
        return Vec::new();
    };

    let Some(window) = windows.get(&key) else {
        return Vec::new();
    };

    match &window.kind {
        WindowKind::Blank => Vec::new(),
        WindowKind::Pair(pair) => {
            let mut held = visible(windows, Some(pair.child1));

            held.extend(visible(windows, Some(pair.child2)));

            held
        }
        _ => vec![key],
    }
}

/// One grid row's cells coalesced into named runs: per-cell dress
/// collapsed wherever adjacent cells share it, the grouping the
/// painted displays read by.
fn grouped(line: &[char], styles: &[u32], links: &[u32]) -> Vec<TextRun> {
    let mut runs: Vec<TextRun> = Vec::new();

    for (index, character) in line.iter().enumerate() {
        let name = styled(styles.get(index).copied().unwrap_or(style::NORMAL));
        let link = i64::from(links.get(index).copied().unwrap_or(0));

        match runs.last_mut() {
            Some(last) if last.style == name && last.link == link => {
                last.text.push(*character);
            }
            _ => {
                let mut text = String::new();

                text.push(*character);
                runs.push(TextRun {
                    style: name.to_string(),
                    link,
                    text,
                    ink: None,
                });
            }
        }
    }

    runs
}

/// A Glk style number as its protocol name.
///
/// A number beyond the eleven renders normal, exactly as the
/// painted displays render it plain (Glk: Styles).
fn styled(style: u32) -> &'static str {
    STYLES.get(style as usize).copied().unwrap_or("normal")
}

/// One drained flow element in the Page's own vocabulary.
///
/// Text runs keep their shape; a placed picture becomes the
/// ready-made special span the line data carries, its link value
/// riding only when real; a flow break is the Page's own sentinel
/// (GlkOte: The Line Data Array).
fn flowed(piece: &Flow) -> PageRun {
    match piece {
        Flow::Run {
            style,
            hyperlink,
            text,
        } => PageRun::Text(TextRun {
            style: styled(*style).to_string(),
            link: i64::from(*hyperlink),
            text: text.clone(),
            ink: None,
        }),
        Flow::Placed(placed) => {
            let mut span = Object::new();

            span.set("special", "image");
            span.set("image", i64::from(placed.image));
            span.set("url", placed.url.clone());
            span.set("width", i64::from(placed.width));
            span.set("height", i64::from(placed.height));
            span.set("alignment", alignment_name(placed.alignment));

            if placed.hyperlink != 0 {
                span.set("hyperlink", i64::from(placed.hyperlink));
            }

            PageRun::Special(span)
        }
        Flow::Break => PageRun::Flowbreak,
    }
}

/// A grid's input position, clamped inside it; None elsewhere.
fn caret(window: &Window) -> Option<(i64, i64)> {
    let WindowKind::Grid(data) = &window.kind else {
        return None;
    };

    Some((
        data.cursor_x.min((window.width() - 1).max(0)),
        data.cursor_y.min((window.height() - 1).max(0)),
    ))
}

/// The text a line request arrived pre-filled with.
fn initial_text(
    request: &crate::glulx::glk::objects::LineRequest,
    memory: &Memory,
) -> Result<String, VoxamError> {
    let Some(buf) = request.buf else {
        return Ok(String::new());
    };

    let count = request.initlen.min(buf.count);
    let mut held = String::with_capacity(count as usize);

    for index in 0..count {
        held.push(to_char(buf.get(memory, index)?));
    }

    Ok(held)
}

/// An event value as the text the reference's str() would read.
fn stringy(value: Option<&Value>) -> String {
    match value {
        Some(Value::Str(held)) => held.clone(),
        Some(Value::Int(held)) => held.to_string(),
        Some(Value::Bool(held)) => if *held { "True" } else { "False" }.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
#[path = "glkote_tests.rs"]
mod tests;
