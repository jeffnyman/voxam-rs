//! The Glk function surface.
//!
//! Every method named glk_* is what the game reaches when the
//! bridge era routes the glk opcode here by selector. The library
//! holds the window tree, the live object lists, and the current
//! output stream; nothing in it knows about Glulx -- ids,
//! addresses, and the stack are the bridge's translation, though
//! following the port's standing departure the memory map is
//! passed to every function that touches a buffer.
//!
//! Blocking by default, suspending on request: glkote's glkapi.js
//! cannot block, so its glk_select returns a sentinel and the
//! interpreter resumes from a callback. A display that can block
//! is simply asked for input and glk_select returns when it has
//! some -- the cheapglk and glkterm arrangement. A display that
//! cannot block raises its suspends flag, and glk_select records
//! the wait instead: the machine returns to its host, and the host
//! answers through deliver_event.
//!
//! The reference's session-end exception becomes the `Stop::End`
//! fault, per the suspension-as-return-value departure; the
//! reference's holder objects (its refs module) fold in here as
//! `RefSlot` and `StructSlot`, filled by the library and written
//! back by the bridge; and the disposal callback becomes a
//! drainable report list, read by the bridge after each call.

use std::collections::HashMap;
use std::path::PathBuf;

use unicode_normalization::UnicodeNormalization;

use crate::errors::VoxamError;
use crate::glulx::glk::dispatch::{CLASS_SCHANNEL, CLASS_STREAM, CLASS_WINDOW};
use crate::glulx::glk::frontend::{Asked, Frontend, NullFrontend};
use crate::glulx::glk::objects::{
    BufferData, Event, FileRef, GraphicsData, GridData, LineRequest, MemArray, NEWLINE, PairData,
    SoundChannel, Stream, StreamKind, Window, WindowKind, WindowMap, event_type, file_mode,
    file_usage, key_code, rearrange, style, subtree, window_method, window_type,
};
use crate::glulx::glk::resources::Resources;
use crate::glulx::memory::Memory;

/// Glk 0.7.6, the version the dispatch table is drawn from (Glk:
/// The Version Number).
pub const GLK_VERSION: u32 = 0x0000_0706;

pub const FULL_VOLUME: u32 = 0x10000;

/// The Glk gestalt selectors (Glk: The Gestalt System). These are
/// Glk's own capability questions, asked through glk_gestalt --
/// not the Glulx machine's, which answer for the VM.
pub mod glk_gestalt {
    pub const VERSION: u32 = 0;
    pub const CHAR_INPUT: u32 = 1;
    pub const LINE_INPUT: u32 = 2;
    pub const CHAR_OUTPUT: u32 = 3;
    pub const MOUSE_INPUT: u32 = 4;
    pub const TIMER: u32 = 5;
    pub const GRAPHICS: u32 = 6;
    pub const DRAW_IMAGE: u32 = 7;
    pub const SOUND: u32 = 8;
    pub const SOUND_VOLUME: u32 = 9;
    pub const SOUND_NOTIFY: u32 = 10;
    pub const HYPERLINKS: u32 = 11;
    pub const HYPERLINK_INPUT: u32 = 12;
    pub const SOUND_MUSIC: u32 = 13;
    pub const GRAPHICS_TRANSPARENCY: u32 = 14;
    pub const UNICODE: u32 = 15;
    pub const UNICODE_NORM: u32 = 16;
    pub const LINE_INPUT_ECHO: u32 = 17;
    pub const LINE_TERMINATORS: u32 = 18;
    pub const LINE_TERMINATOR_KEY: u32 = 19;
    pub const DATE_TIME: u32 = 20;
    pub const SOUND2: u32 = 21;
    pub const RESOURCE_STREAM: u32 = 22;
    pub const GRAPHICS_CHAR_INPUT: u32 = 23;
    pub const DRAW_IMAGE_SCALE: u32 = 24;
}

/// The CharOutput selector's answers (Glk: Output).
pub const CHAR_OUTPUT_CANNOT_PRINT: u32 = 0;
pub const CHAR_OUTPUT_APPROX_PRINT: u32 = 1;
pub const CHAR_OUTPUT_EXACT_PRINT: u32 = 2;

/// The lowest special keycode; glk.h defines the range this way.
const SPECIAL_KEYS: u32 = (0x1_0000_0000u64 - key_code::MAXVAL as u64) as u32;

/// Characters deleted from a game-supplied filename (Glk: File
/// References).
const ILLEGAL_IN_NAME: &[char] = &['"', '\\', '/', '>', '<', ':', '|', '?', '*'];

const DEFAULT_SUFFIX: &str = ".glkdata";

const MICROS: i64 = 1_000_000;

/// The suffix a file wears, by what it is for (Glk: File
/// References).
fn suffix_for(usage: u32) -> &'static str {
    match usage & file_usage::TYPE_MASK {
        file_usage::SAVED_GAME => ".glksave",
        file_usage::TRANSCRIPT | file_usage::INPUT_RECORD => ".txt",
        _ => DEFAULT_SUFFIX,
    }
}

/// Why a Glk call did not return normally: a fault, or the end of
/// the session -- the reference's GlulxSessionEnd, spoken as a
/// value per the port's suspension departure.
#[derive(Debug)]
pub enum Stop {
    Fault(VoxamError),
    End,
}

impl From<VoxamError> for Stop {
    fn from(error: VoxamError) -> Self {
        Self::Fault(error)
    }
}

pub type Outcome<T> = Result<T, Stop>;

fn fault<T>(message: &str) -> Outcome<T> {
    Err(Stop::Fault(VoxamError::GlulxGlk(message.into())))
}

/// What a reference can hold: a word, or an opaque object of a
/// class -- because turning objects into the 32-bit ids Glulx sees
/// is the bridge's translation, not the library's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    Word(u32),
    Obj(u32, Option<u32>),
}

impl Default for Held {
    fn default() -> Self {
        Self::Word(0)
    }
}

impl Held {
    /// The held word, an object reading as zero -- for tests and
    /// the bridge's plain encodings.
    pub fn word(&self) -> u32 {
        match self {
            Self::Word(value) => *value,
            Self::Obj(..) => 0,
        }
    }

    fn signed(&self) -> i64 {
        i64::from(self.word() as i32)
    }
}

/// A single call-by-reference output value.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RefSlot(pub Held);

/// A struct passed by reference: a fixed row of fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructSlot(pub Vec<Held>);

impl StructSlot {
    /// Open with a count of zeroed fields.
    pub fn new(count: usize) -> Self {
        Self(vec![Held::default(); count])
    }

    fn set_all(&mut self, values: &[Held]) {
        self.0 = values.to_vec();
    }
}

fn fill_event(slot: &mut StructSlot, event: Event) {
    slot.set_all(&[
        Held::Word(event.kind),
        Held::Obj(CLASS_WINDOW, event.window),
        Held::Word(event.val1),
        Held::Word(event.val2),
    ]);
}

/// A suspended wait: the select an event has yet to answer, or the
/// file prompt a name has yet to answer, for a display that
/// suspends. The bridge parks the call's deferred tail beside
/// this; the library holds only what it knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waiting {
    /// A suspended select: the seat the awaited event will land
    /// in.
    Select,
    /// A suspended file prompt: a Glk call standing mid-flight,
    /// its result the player's answer (Glk: File References).
    Prompt { usage: u32, fmode: u32, rock: u32 },
}

/// A Glk library instance.
pub struct Glk {
    /// The display rendered into and read from.
    pub frontend: Box<dyn Frontend>,
    /// Where game-named files live; every sanitized filename
    /// resolves inside it.
    pub save_dir: PathBuf,
    /// The pictures, sounds, and data on offer.
    pub resources: Resources,
    /// The root of the window tree, or None before the first
    /// window opens.
    pub root: Option<u32>,
    /// Where the printing functions send output, or None (Glk:
    /// Streams).
    pub current_stream: Option<u32>,
    /// Every live window, by internal key.
    pub windows: WindowMap,
    /// The window walk order, newest first -- the order glkapi.js
    /// iterates, by prepending.
    pub window_order: Vec<u32>,
    /// Every live stream, by internal key. A pair window's stream
    /// lives here but not on the walk order, as in the reference.
    pub streams: HashMap<u32, Stream>,
    /// The stream walk order, newest first.
    pub stream_order: Vec<u32>,
    /// Every live file reference, by internal key.
    pub filerefs: HashMap<u32, FileRef>,
    /// The fileref walk order, newest first.
    pub fileref_order: Vec<u32>,
    /// Every live sound channel, by internal key.
    pub channels: HashMap<u32, SoundChannel>,
    /// The channel walk order, newest first.
    pub channel_order: Vec<u32>,
    /// The hints set by stylehint_set, for a display that honors
    /// them.
    pub stylehints: HashMap<(u32, u32, u32), u32>,
    /// The requested timer cadence in milliseconds, zero for none.
    pub timer_interval: u32,
    /// Events a display has posted -- timers, sound notifications
    /// -- waiting for the next select.
    pub pending_events: Vec<Event>,
    /// The suspended wait, or None while the machine runs.
    pub waiting: Option<Waiting>,
    /// The local timezone's offset from UTC in seconds, for the
    /// _local clock functions. The reference asks the host's
    /// timezone database per timestamp; the port takes one offset
    /// from its host -- a recorded simplification until a gate
    /// demands more.
    pub local_offset_seconds: i64,
    /// A pinned clock for tests; None means the real one.
    pub now_override: Option<f64>,
    /// Disposal reports the bridge drains, so a closed object's id
    /// stops resolving: (class, internal key).
    disposals: Vec<(u32, u32)>,
    next_key: u32,
}

impl Glk {
    /// Open with no windows, over a display.
    pub fn new(frontend: Box<dyn Frontend>) -> Self {
        Self {
            frontend,
            save_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            resources: Resources::default(),
            root: None,
            current_stream: None,
            windows: WindowMap::new(),
            window_order: Vec::new(),
            streams: HashMap::new(),
            stream_order: Vec::new(),
            filerefs: HashMap::new(),
            fileref_order: Vec::new(),
            channels: HashMap::new(),
            channel_order: Vec::new(),
            stylehints: HashMap::new(),
            timer_interval: 0,
            pending_events: Vec::new(),
            waiting: None,
            local_offset_seconds: 0,
            now_override: None,
            disposals: Vec::new(),
            next_key: 0,
        }
    }

    /// Open over a display that shows nothing.
    pub fn over_nothing() -> Self {
        Self::new(Box::new(NullFrontend))
    }

    // -- internals -------------------------------------------------------

    fn mint(&mut self) -> u32 {
        self.next_key += 1;

        self.next_key
    }

    /// The disposal reports since last drained: (class, key).
    pub fn take_disposals(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.disposals)
    }

    fn dispose_stream(&mut self, key: u32) {
        if let Some(stream) = self.streams.get_mut(&key) {
            stream.close();
        }

        self.streams.remove(&key);
        self.disposals.push((CLASS_STREAM, key));
    }

    /// Lay the window tree out over the display again.
    ///
    /// Metrics are refreshed here rather than at window creation,
    /// so a display that changes its font mid-game only has to
    /// re-arrange.
    fn re_lay(&mut self) {
        let Some(root) = self.root else {
            return;
        };

        for key in self.window_order.clone() {
            if let Some(window) = self.windows.get(&key) {
                let metrics = self.frontend.metrics_for(window);

                self.windows.get_mut(&key).unwrap().metrics = metrics;
            }
        }

        let (width, height) = self.frontend.size();

        rearrange(&mut self.windows, root, (0, 0, width, height));

        for key in self.window_order.clone() {
            let moved = matches!(
                self.windows.get(&key).map(|window| &window.kind),
                Some(WindowKind::Graphics(GraphicsData { moved: true }))
            );

            if moved {
                // The move cleared the canvas to background; the
                // game owes it a redraw and is told so (Glk:
                // Window Events).
                if let Some(window) = self.windows.get_mut(&key)
                    && let WindowKind::Graphics(data) = &mut window.kind
                {
                    data.moved = false;
                }

                self.post_event(Event::new(event_type::REDRAW, Some(key), 0, 0));
            }
        }
    }

    // -- main (Glk: Your Program's Main Function) --------------------------

    /// End the session, showing whatever is pending first.
    pub fn glk_exit(&mut self) -> Outcome<()> {
        self.frontend.flush(&mut self.windows, self.root);

        Err(Stop::End)
    }

    /// Yield time to the display; here, nothing (Glk: The Tick
    /// Thing).
    pub fn glk_tick(&mut self) {}

    /// Ask a capability question (Glk: The Gestalt System).
    pub fn glk_gestalt(&mut self, selector: u32, value: u32) -> u32 {
        self.gestalt_answer(selector, value)
    }

    /// Ask a capability question with room for extra answers.
    pub fn glk_gestalt_ext(
        &mut self,
        memory: &mut Memory,
        selector: u32,
        value: u32,
        array: Option<MemArray>,
    ) -> Outcome<u32> {
        if selector == glk_gestalt::CHAR_OUTPUT
            && let Some(array) = array
            && array.count > 0
        {
            array.set(memory, 0, u32::from(is_printable(value)))?;
        }

        Ok(self.gestalt_answer(selector, value))
    }

    fn gestalt_answer(&mut self, selector: u32, value: u32) -> u32 {
        match selector {
            glk_gestalt::VERSION => GLK_VERSION,

            // Any Latin-1 printable, plus the special keycodes.
            // Unknown is not a key a game can ask to receive -- it
            // is what a display reports when it cannot name one
            // (Glk: Character Input).
            glk_gestalt::CHAR_INPUT => {
                u32::from(is_printable(value) || (SPECIAL_KEYS..key_code::UNKNOWN).contains(&value))
            }

            // A line is made of printable characters; the special
            // keys can only end one, which is the LineTerminators
            // selector's business (Glk: Line Input).
            glk_gestalt::LINE_INPUT => u32::from(is_printable(value) && value != NEWLINE),

            glk_gestalt::CHAR_OUTPUT => {
                if is_printable(value) {
                    CHAR_OUTPUT_EXACT_PRINT
                } else {
                    CHAR_OUTPUT_CANNOT_PRINT
                }
            }

            glk_gestalt::GRAPHICS | glk_gestalt::GRAPHICS_TRANSPARENCY => {
                u32::from(self.frontend.graphics())
            }

            // The argument is a window type: "libraries may
            // implement both, neither, or only one" (Glk: Testing
            // for Graphics Capabilities).
            glk_gestalt::DRAW_IMAGE | glk_gestalt::DRAW_IMAGE_SCALE => u32::from(
                (self.frontend.graphics() && value == window_type::GRAPHICS)
                    || (self.frontend.buffer_images() && value == window_type::TEXT_BUFFER),
            ),

            // Character input is window-blind at every display
            // here -- the keyboard answers whichever window asked
            // -- so a canvas takes keystrokes wherever a canvas
            // can exist at all.
            glk_gestalt::GRAPHICS_CHAR_INPUT => u32::from(self.frontend.graphics()),

            glk_gestalt::SOUND
            | glk_gestalt::SOUND2
            | glk_gestalt::SOUND_VOLUME
            | glk_gestalt::SOUND_NOTIFY => u32::from(self.frontend.sound()),

            // Music means MOD and song files (Glk: Testing for
            // Sound Capabilities); the only decoder aboard is
            // AIFF, so the claim stays honestly zero.
            glk_gestalt::SOUND_MUSIC => 0,

            // The argument is a window type, and only grids and
            // graphics windows can carry a mouse position (Glk:
            // Mouse Input Events).
            glk_gestalt::MOUSE_INPUT => u32::from(
                self.frontend.mouse_input()
                    && matches!(value, window_type::TEXT_GRID | window_type::GRAPHICS),
            ),

            glk_gestalt::TIMER => u32::from(self.frontend.timer_input()),

            // Link markup is accepted on any stream; whether a
            // link can be *selected* is the separate question
            // below.
            glk_gestalt::HYPERLINKS => 1,

            glk_gestalt::HYPERLINK_INPUT => u32::from(self.frontend.hyperlink_input()),

            glk_gestalt::UNICODE
            | glk_gestalt::UNICODE_NORM
            | glk_gestalt::LINE_INPUT_ECHO
            | glk_gestalt::LINE_TERMINATORS
            | glk_gestalt::LINE_TERMINATOR_KEY
            | glk_gestalt::DATE_TIME
            | glk_gestalt::RESOURCE_STREAM => 1,

            // Every selector from a Glk yet to be written: zero is
            // the honest answer for the unsupported and the
            // unknown alike.
            _ => 0,
        }
    }

    // -- windows (Glk: Window Opening, Closing, and Constraints) -----------

    /// Walk the live windows (Glk: Iterating Through Opaque
    /// Objects).
    pub fn glk_window_iterate(
        &mut self,
        window: Option<u32>,
        rockref: Option<&mut RefSlot>,
    ) -> Option<u32> {
        let rocks = &self.windows;

        iterate(
            &self.window_order,
            |key| rocks.get(&key).map_or(0, |held| held.rock),
            window,
            rockref,
        )
    }

    /// The rock the window was opened with (Glk: Rocks).
    pub fn glk_window_get_rock(&self, window: Option<u32>) -> u32 {
        window
            .and_then(|key| self.windows.get(&key))
            .map_or(0, |window| window.rock)
    }

    /// The root of the window tree, or None with none open.
    pub fn glk_window_get_root(&self) -> Option<u32> {
        self.root
    }

    /// Open a window, splitting an existing one after the first.
    ///
    /// An unsupported window type answers None rather than
    /// faulting, so a game can probe for graphics support by
    /// trying (Glk: Window Opening, Closing, and Constraints).
    pub fn glk_window_open(
        &mut self,
        split: Option<u32>,
        method: u32,
        size: u32,
        wtype: u32,
        rock: u32,
    ) -> Outcome<Option<u32>> {
        if self.root.is_none() {
            if split.is_some() {
                return fault("window_open: splitwin must be null for the first window");
            }
        } else if split.filter(|key| self.windows.contains_key(key)).is_none() {
            // A key that no longer resolves earns the same refusal
            // the null window does, as it would through the
            // bridge.
            return fault("window_open: splitwin must not be null");
        } else {
            let division = method & window_method::DIVISION_MASK;

            if !matches!(division, window_method::FIXED | window_method::PROPORTIONAL) {
                return fault("window_open: the method is neither fixed nor proportional");
            }

            if (method & window_method::DIR_MASK) > window_method::BELOW {
                return fault("window_open: the method names no direction");
            }
        }

        let Some(kind) = make_window_kind(wtype, self.frontend.graphics())? else {
            return Ok(None);
        };

        let wkey = self.mint();
        let skey = self.mint();
        let mut window = Window::new(kind, rock);

        window.stream = skey;

        self.windows.insert(wkey, window);
        self.streams.insert(skey, Stream::window(wkey));
        self.window_order.insert(0, wkey);
        self.stream_order.insert(0, skey);

        match split {
            None => self.root = Some(wkey),
            Some(split) => {
                let parent = self.windows[&split].parent;
                let pkey = self.mint();
                let pair_stream = self.mint();
                let mut pair = Window::new(
                    WindowKind::Pair(PairData::new(split, wkey, wkey, method, size)),
                    0,
                );

                pair.stream = pair_stream;
                pair.parent = parent;

                self.windows.insert(pkey, pair);
                self.streams.insert(pair_stream, Stream::window(pkey));
                self.window_order.insert(0, pkey);

                self.windows.get_mut(&split).unwrap().parent = Some(pkey);
                self.windows.get_mut(&wkey).unwrap().parent = Some(pkey);

                match parent {
                    None => self.root = Some(pkey),
                    Some(grand) => {
                        if let WindowKind::Pair(data) =
                            &mut self.windows.get_mut(&grand).unwrap().kind
                        {
                            if data.child1 == split {
                                data.child1 = pkey;
                            } else {
                                data.child2 = pkey;
                            }
                        }
                    }
                }
            }
        }

        self.re_lay();

        Ok(Some(wkey))
    }

    /// Close a window and its whole subtree.
    ///
    /// The sibling is promoted into the parent pair's place (Glk:
    /// Window Opening, Closing, and Constraints).
    pub fn glk_window_close(
        &mut self,
        window: Option<u32>,
        result: Option<&mut StructSlot>,
    ) -> Outcome<()> {
        let Some(wkey) = window.filter(|key| self.windows.contains_key(key)) else {
            return fault("window_close: invalid window");
        };

        let stream_key = self.windows[&wkey].stream;
        let parent = self.windows[&wkey].parent;
        let counts = self
            .streams
            .get_mut(&stream_key)
            .map_or((0, 0), Stream::close);

        if let Some(slot) = result {
            slot.set_all(&[Held::Word(counts.0), Held::Word(counts.1)]);
        }

        for descendant in subtree(&self.windows, wkey) {
            self.forget_window(descendant);
        }

        match parent {
            None => self.root = None,
            Some(pkey) => {
                let (child1, child2, grandparent) = {
                    let pair = &self.windows[&pkey];

                    match &pair.kind {
                        WindowKind::Pair(data) => (data.child1, data.child2, pair.parent),
                        _ => unreachable!("a window's parent is a pair"),
                    }
                };
                let sibling = if child1 == wkey { child2 } else { child1 };

                self.forget_window(pkey);

                if let Some(held) = self.windows.get_mut(&sibling) {
                    held.parent = grandparent;
                }

                match grandparent {
                    None => self.root = Some(sibling),
                    Some(grand) => {
                        if let Some(window) = self.windows.get_mut(&grand)
                            && let WindowKind::Pair(data) = &mut window.kind
                        {
                            if data.child1 == pkey {
                                data.child1 = sibling;
                            } else {
                                data.child2 = sibling;
                            }
                        }
                    }
                }
            }
        }

        self.re_lay();

        Ok(())
    }

    /// Drop one window and its stream from the live lists.
    fn forget_window(&mut self, key: u32) {
        self.window_order.retain(|held| *held != key);

        let Some(window) = self.windows.get(&key) else {
            return;
        };
        let stream_key = window.stream;

        self.stream_order.retain(|held| *held != stream_key);

        if self.current_stream == Some(stream_key) {
            self.current_stream = None;
        }

        self.dispose_stream(stream_key);
        self.windows.remove(&key);
        self.disposals.push((CLASS_WINDOW, key));
    }

    /// The window's size in its own units (Glk: Changing Window
    /// Constraints).
    pub fn glk_window_get_size(
        &self,
        window: Option<u32>,
        widthref: Option<&mut RefSlot>,
        heightref: Option<&mut RefSlot>,
    ) {
        let (width, height) = window
            .and_then(|key| self.windows.get(&key))
            .map_or((0, 0), |held| (held.width(), held.height()));

        if let Some(slot) = widthref {
            slot.0 = Held::Word(width as u32);
        }

        if let Some(slot) = heightref {
            slot.0 = Held::Word(height as u32);
        }
    }

    /// Change a pair's split (Glk: Changing Window Constraints).
    ///
    /// The windows never flip or rotate: changing the direction
    /// within its axis moves the constraint to the other child
    /// while the glass stays where it is, which the model carries
    /// by swapping the children -- glkapi.js does the same.
    pub fn glk_window_set_arrangement(
        &mut self,
        window: Option<u32>,
        method: u32,
        size: u32,
        key: Option<u32>,
    ) -> Outcome<()> {
        let Some(pkey) = window.filter(|held| {
            matches!(
                self.windows.get(held).map(|window| &window.kind),
                Some(WindowKind::Pair(_))
            )
        }) else {
            return fault("window_set_arrangement: not a pair window");
        };

        let direction = method & window_method::DIR_MASK;
        let vertical = matches!(direction, window_method::LEFT | window_method::RIGHT);
        let backward = matches!(direction, window_method::LEFT | window_method::ABOVE);

        let (was_vertical, was_backward) = match &self.windows[&pkey].kind {
            WindowKind::Pair(data) => (data.vertical, data.backward),
            _ => unreachable!(),
        };

        if vertical != was_vertical {
            // "You can't flip or rotate them" (Glk: Changing
            // Window Constraints).
            return fault("window_set_arrangement: a split cannot change its axis");
        }

        if let Some(kkey) = key {
            if matches!(
                self.windows.get(&kkey).map(|window| &window.kind),
                Some(WindowKind::Pair(_))
            ) {
                return fault("window_set_arrangement: the key cannot be a pair window");
            }

            if !subtree(&self.windows, pkey).contains(&kkey) {
                return fault("window_set_arrangement: the key must live under the pair");
            }
        }

        if let Some(pair) = self.windows.get_mut(&pkey)
            && let WindowKind::Pair(data) = &mut pair.kind
        {
            if backward != was_backward {
                std::mem::swap(&mut data.child1, &mut data.child2);
            }

            data.set_method(method);

            data.size = size;

            if let Some(kkey) = key {
                data.key = kkey;
            }
        }

        self.re_lay();

        Ok(())
    }

    /// Report a pair's split (Glk: Changing Window Constraints).
    pub fn glk_window_get_arrangement(
        &self,
        window: Option<u32>,
        methodref: Option<&mut RefSlot>,
        sizeref: Option<&mut RefSlot>,
        keyref: Option<&mut RefSlot>,
    ) -> Outcome<()> {
        let Some(WindowKind::Pair(data)) = window
            .and_then(|key| self.windows.get(&key))
            .map(|held| &held.kind)
        else {
            return fault("window_get_arrangement: not a pair window");
        };

        if let Some(slot) = methodref {
            slot.0 = Held::Word(data.method());
        }

        if let Some(slot) = sizeref {
            slot.0 = Held::Word(data.size);
        }

        if let Some(slot) = keyref {
            slot.0 = Held::Obj(CLASS_WINDOW, Some(data.key));
        }

        Ok(())
    }

    /// The window's type number (Glk: The Types of Windows).
    pub fn glk_window_get_type(&self, window: Option<u32>) -> u32 {
        window
            .and_then(|key| self.windows.get(&key))
            .map_or(0, Window::wintype)
    }

    /// The pair above the window, or None at the root.
    pub fn glk_window_get_parent(&self, window: Option<u32>) -> Option<u32> {
        window
            .and_then(|key| self.windows.get(&key))
            .and_then(|held| held.parent)
    }

    /// The window on the other side of the parent pair.
    pub fn glk_window_get_sibling(&self, window: Option<u32>) -> Option<u32> {
        let key = window?;
        let parent = self.windows.get(&key)?.parent?;

        match &self.windows.get(&parent)?.kind {
            WindowKind::Pair(data) => Some(if data.child1 == key {
                data.child2
            } else {
                data.child1
            }),
            _ => None,
        }
    }

    /// Erase the window (Glk: Other Window Functions).
    pub fn glk_window_clear(&mut self, window: Option<u32>) {
        if let Some(held) = window.and_then(|key| self.windows.get_mut(&key)) {
            held.clear();
        }
    }

    /// Place a grid's cursor (Glk: Text Grid Windows).
    pub fn glk_window_move_cursor(
        &mut self,
        window: Option<u32>,
        xpos: i64,
        ypos: i64,
    ) -> Outcome<()> {
        let grid = window
            .and_then(|key| self.windows.get_mut(&key))
            .filter(|held| matches!(held.kind, WindowKind::Grid(_)));

        let Some(held) = grid else {
            return fault("window_move_cursor: not a text grid window");
        };

        held.move_cursor(xpos, ypos);

        Ok(())
    }

    /// The window's own output stream (Glk: Window Streams).
    pub fn glk_window_get_stream(&self, window: Option<u32>) -> Option<u32> {
        window
            .and_then(|key| self.windows.get(&key))
            .map(|held| held.stream)
    }

    /// Copy the window's output to a stream too (Glk: Echo
    /// Streams).
    pub fn glk_window_set_echo_stream(&mut self, window: Option<u32>, stream: Option<u32>) {
        if let Some(held) = window.and_then(|key| self.windows.get_mut(&key)) {
            held.echo_stream = stream;
        }
    }

    /// The window's echo stream, or None without one.
    pub fn glk_window_get_echo_stream(&self, window: Option<u32>) -> Option<u32> {
        window
            .and_then(|key| self.windows.get(&key))
            .and_then(|held| held.echo_stream)
    }

    /// Send the printing functions to this window (Glk: How To
    /// Print).
    pub fn glk_set_window(&mut self, window: Option<u32>) {
        self.current_stream = window
            .and_then(|key| self.windows.get(&key))
            .map(|held| held.stream);
    }

    // -- streams (Glk: Streams) --------------------------------------------

    /// Walk the live streams.
    pub fn glk_stream_iterate(
        &mut self,
        stream: Option<u32>,
        rockref: Option<&mut RefSlot>,
    ) -> Option<u32> {
        let rocks = &self.streams;

        iterate(
            &self.stream_order,
            |key| rocks.get(&key).map_or(0, |held| held.rock),
            stream,
            rockref,
        )
    }

    /// The rock the stream was opened with (Glk: Rocks).
    pub fn glk_stream_get_rock(&self, stream: Option<u32>) -> u32 {
        stream
            .and_then(|key| self.streams.get(&key))
            .map_or(0, |held| held.rock)
    }

    /// Open a stream over game memory (Glk: Memory Streams).
    pub fn glk_stream_open_memory(
        &mut self,
        buf: Option<MemArray>,
        fmode: u32,
        rock: u32,
    ) -> Outcome<u32> {
        self.open_memory(buf, fmode, rock, false)
    }

    /// Open a word-array stream over game memory.
    pub fn glk_stream_open_memory_uni(
        &mut self,
        buf: Option<MemArray>,
        fmode: u32,
        rock: u32,
    ) -> Outcome<u32> {
        self.open_memory(buf, fmode, rock, true)
    }

    fn open_memory(
        &mut self,
        buf: Option<MemArray>,
        fmode: u32,
        rock: u32,
        unicode: bool,
    ) -> Outcome<u32> {
        if !matches!(
            fmode,
            file_mode::READ | file_mode::WRITE | file_mode::READ_WRITE
        ) {
            // WriteAppend is forbidden on a memory stream (Glk:
            // Memory Streams).
            return fault("stream_open_memory: illegal filemode");
        }

        let key = self.mint();

        self.streams
            .insert(key, Stream::memory(buf, fmode, rock, unicode));
        self.stream_order.insert(0, key);

        Ok(key)
    }

    /// Close a stream, reporting its counts (Glk: Closing
    /// Streams).
    pub fn glk_stream_close(
        &mut self,
        stream: Option<u32>,
        result: Option<&mut StructSlot>,
    ) -> Outcome<()> {
        let Some(key) = stream.filter(|held| self.streams.contains_key(held)) else {
            return fault("stream_close: invalid stream");
        };

        let counts = self.streams.get_mut(&key).map_or((0, 0), Stream::close);

        if let Some(slot) = result {
            slot.set_all(&[Held::Word(counts.0), Held::Word(counts.1)]);
        }

        self.stream_order.retain(|held| *held != key);

        if self.current_stream == Some(key) {
            self.current_stream = None;
        }

        self.dispose_stream(key);

        Ok(())
    }

    /// Choose where the printing functions send output.
    pub fn glk_stream_set_current(&mut self, stream: Option<u32>) {
        self.current_stream = stream;
    }

    /// The stream the printing functions write to, or None.
    pub fn glk_stream_get_current(&self) -> Option<u32> {
        self.current_stream
    }

    /// Move a stream's mark (Glk: Stream Positions).
    pub fn glk_stream_set_position(
        &mut self,
        stream: Option<u32>,
        position: i64,
        mode: u32,
    ) -> Outcome<()> {
        if let Some(held) = stream.and_then(|key| self.streams.get_mut(&key)) {
            held.set_position(position, mode)?;
        }

        Ok(())
    }

    /// A stream's mark (Glk: Stream Positions).
    pub fn glk_stream_get_position(&mut self, stream: Option<u32>) -> Outcome<u32> {
        match stream.and_then(|key| self.streams.get_mut(&key)) {
            Some(held) => Ok(held.get_position()?),
            None => Ok(0),
        }
    }

    // -- file references (Glk: File References) ----------------------------

    /// A reference to a fresh temporary file.
    pub fn glk_fileref_create_temp(&mut self, usage: u32, rock: u32) -> Outcome<u32> {
        let path = fresh_temp_path()?;

        Ok(self.new_fileref(path, usage, rock, true))
    }

    /// A reference to a file the game names itself.
    pub fn glk_fileref_create_by_name(&mut self, usage: u32, name: &str, rock: u32) -> u32 {
        let path = self.path_for(name, usage);

        self.new_fileref(path, usage, rock, false)
    }

    /// A reference to a file the player names.
    ///
    /// A blocking display is asked on the spot; a suspending one
    /// is never asked -- the call itself stands down mid-flight,
    /// its tail parked on the wait, until the host answers through
    /// deliver_file. A cancelled prompt yields the null reference
    /// either way (Glk: File References).
    pub fn glk_fileref_create_by_prompt(
        &mut self,
        usage: u32,
        fmode: u32,
        rock: u32,
    ) -> Option<u32> {
        if self.frontend.suspends() {
            self.waiting = Some(Waiting::Prompt { usage, fmode, rock });

            // A placeholder the bridge encodes but the machine
            // never stores: the real result arrives with the name.
            return None;
        }

        let name = self.frontend.prompt_file(usage, fmode)?;

        if name.is_empty() {
            return None;
        }

        let path = self.path_for(&name, usage);

        Some(self.new_fileref(path, usage, rock, false))
    }

    /// Complete a suspended file prompt with the player's name.
    ///
    /// The prompt's name is the player's own, not the game's: the
    /// sanitizing jail guards names arriving from bytecode, but a
    /// name a person chose is honored as given (Glk: File
    /// References; cheapglk draws the same line). A relative name
    /// lands in the save dir, a bare one gains its usage's suffix
    /// as a courtesy, and a cancel is nothing at all, which is
    /// always legitimate. The call's parked tail -- the bridge's
    /// encoding, the machine's store -- is the bridge era's to
    /// run.
    pub fn deliver_file(&mut self, name: Option<&str>) -> Outcome<Option<u32>> {
        let Some(Waiting::Prompt { usage, rock, .. }) = self.waiting else {
            return fault("a file name arrived with no prompt suspended to receive it");
        };

        let fileref = match name {
            None | Some("") => None,
            Some(name) => {
                let mut chosen = PathBuf::from(name);

                if !chosen.is_absolute() {
                    chosen = self.save_dir.join(chosen);
                }

                if chosen.extension().is_none() {
                    let mut named = chosen.into_os_string();

                    named.push(suffix_for(usage));

                    chosen = PathBuf::from(named);
                }

                Some(self.new_fileref(chosen, usage, rock, false))
            }
        };

        self.waiting = None;

        Ok(fileref)
    }

    /// A reference to the same file, for a different usage.
    pub fn glk_fileref_create_from_fileref(
        &mut self,
        usage: u32,
        fileref: Option<u32>,
        rock: u32,
    ) -> Outcome<u32> {
        let Some(source) = fileref.and_then(|key| self.filerefs.get(&key)) else {
            return fault("fileref_create_from_fileref: invalid fileref");
        };

        let path = PathBuf::from(source.filename.clone());

        Ok(self.new_fileref(path, usage, rock, false))
    }

    /// Drop a reference; a temporary file dies with it (Glk: File
    /// References).
    pub fn glk_fileref_destroy(&mut self, fileref: Option<u32>) {
        let Some(key) = fileref else {
            return;
        };
        let Some(held) = self.filerefs.get(&key) else {
            return;
        };

        if held.temporary {
            let _ = std::fs::remove_file(&held.filename);
        }

        self.fileref_order.retain(|kept| *kept != key);
        self.filerefs.remove(&key);
        self.disposals
            .push((crate::glulx::glk::dispatch::CLASS_FILEREF, key));
    }

    /// Delete the file the reference names.
    pub fn glk_fileref_delete_file(&mut self, fileref: Option<u32>) {
        if let Some(held) = fileref.and_then(|key| self.filerefs.get(&key)) {
            let _ = std::fs::remove_file(&held.filename);
        }
    }

    /// Whether the named file exists right now.
    pub fn glk_fileref_does_file_exist(&self, fileref: Option<u32>) -> u32 {
        u32::from(
            fileref
                .and_then(|key| self.filerefs.get(&key))
                .is_some_and(|held| std::path::Path::new(&held.filename).is_file()),
        )
    }

    /// Walk the live file references.
    pub fn glk_fileref_iterate(
        &mut self,
        fileref: Option<u32>,
        rockref: Option<&mut RefSlot>,
    ) -> Option<u32> {
        let rocks = &self.filerefs;

        iterate(
            &self.fileref_order,
            |key| rocks.get(&key).map_or(0, |held| held.rock),
            fileref,
            rockref,
        )
    }

    /// The rock the reference was created with (Glk: Rocks).
    pub fn glk_fileref_get_rock(&self, fileref: Option<u32>) -> u32 {
        fileref
            .and_then(|key| self.filerefs.get(&key))
            .map_or(0, |held| held.rock)
    }

    /// Record a reference on the live list.
    fn new_fileref(&mut self, path: PathBuf, usage: u32, rock: u32, temporary: bool) -> u32 {
        let key = self.mint();

        self.filerefs.insert(
            key,
            FileRef::new(path.to_string_lossy().into_owned(), usage, rock, temporary),
        );
        self.fileref_order.insert(0, key);

        key
    }

    /// A game-supplied name, made a path inside the save dir.
    ///
    /// The recommended simplification, as cheapglk implements it:
    /// delete every character in the illegal set, truncate at the
    /// first period, use "null" if nothing is left, then append a
    /// suffix chosen by usage (Glk: File References). Not a spec
    /// requirement, but it is what lets Glk implementations
    /// exchange files -- and it means a name arriving from game
    /// bytecode cannot reach outside the save directory by any
    /// route.
    fn path_for(&self, name: &str, usage: u32) -> PathBuf {
        let head = name.split('.').next().unwrap_or("");
        let mut stem: String = head
            .chars()
            .filter(|held| !ILLEGAL_IN_NAME.contains(held))
            .collect();

        if stem.is_empty() {
            stem = "null".into();
        }

        self.save_dir.join(stem + suffix_for(usage))
    }

    // -- file streams (Glk: File Streams) ----------------------------------

    /// Open a byte stream over the referenced file.
    pub fn glk_stream_open_file(
        &mut self,
        fileref: Option<u32>,
        fmode: u32,
        rock: u32,
    ) -> Outcome<Option<u32>> {
        self.open_file(fileref, fmode, rock, false)
    }

    /// Open a word stream over the referenced file.
    pub fn glk_stream_open_file_uni(
        &mut self,
        fileref: Option<u32>,
        fmode: u32,
        rock: u32,
    ) -> Outcome<Option<u32>> {
        self.open_file(fileref, fmode, rock, true)
    }

    fn open_file(
        &mut self,
        fileref: Option<u32>,
        fmode: u32,
        rock: u32,
        unicode: bool,
    ) -> Outcome<Option<u32>> {
        let Some(source) = fileref.and_then(|key| self.filerefs.get(&key)) else {
            return fault("stream_open_file: invalid fileref");
        };

        let path = PathBuf::from(source.filename.clone());
        let text_mode = source.text_mode;

        let mut options = std::fs::OpenOptions::new();

        match fmode {
            file_mode::READ => {
                options.read(true);
            }
            file_mode::WRITE => {
                options.write(true).create(true).truncate(true);
            }
            // Not append mode: POSIX append forces every write to
            // the end of the file, but Glk only asks that the
            // *mark* start there -- a later seek must be honored
            // (Glk: Stream Positions).
            file_mode::READ_WRITE | file_mode::WRITE_APPEND => {
                if !path.exists() {
                    let _ = std::fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(false)
                        .open(&path);
                }

                options.read(true).write(true);
            }
            _ => return fault("stream_open_file: illegal filemode"),
        }

        let Ok(mut handle) = options.open(&path) else {
            // Opening may simply fail, and yields the null stream
            // (Glk: File Streams).
            return Ok(None);
        };

        if fmode == file_mode::WRITE_APPEND {
            use std::io::Seek;

            let _ = handle.seek(std::io::SeekFrom::End(0));
        }

        let key = self.mint();

        self.streams.insert(
            key,
            Stream::file(
                crate::glulx::glk::objects::FileHandle::Real(handle),
                fmode,
                rock,
                unicode,
                text_mode,
            ),
        );
        self.stream_order.insert(0, key);

        Ok(Some(key))
    }

    // -- resource streams (Glk: Resource Streams) --------------------------

    /// Open a byte stream over a Blorb data resource.
    pub fn glk_stream_open_resource(&mut self, filenum: u32, rock: u32) -> Option<u32> {
        self.open_resource(filenum, rock, false)
    }

    /// Open a word stream over a Blorb data resource.
    pub fn glk_stream_open_resource_uni(&mut self, filenum: u32, rock: u32) -> Option<u32> {
        self.open_resource(filenum, rock, true)
    }

    fn open_resource(&mut self, filenum: u32, rock: u32, unicode: bool) -> Option<u32> {
        let (data, is_text) = self.resources.data(filenum)?;

        // The same encoding matrix as a file, over bytes instead
        // of a file handle: text plus Unicode means UTF-8, binary
        // means four-byte words (Glk: Resource Streams).
        let key = self.mint();

        self.streams.insert(
            key,
            Stream::file(
                crate::glulx::glk::objects::FileHandle::Bytes(std::io::Cursor::new(data)),
                file_mode::READ,
                rock,
                unicode,
                is_text,
            ),
        );
        self.stream_order.insert(0, key);

        Some(key)
    }

    // -- sound channels (Glk: Sound) ---------------------------------------

    /// Walk the live sound channels.
    pub fn glk_schannel_iterate(
        &mut self,
        channel: Option<u32>,
        rockref: Option<&mut RefSlot>,
    ) -> Option<u32> {
        let rocks = &self.channels;

        iterate(
            &self.channel_order,
            |key| rocks.get(&key).map_or(0, |held| held.rock),
            channel,
            rockref,
        )
    }

    /// The rock the channel was created with (Glk: Rocks).
    pub fn glk_schannel_get_rock(&self, channel: Option<u32>) -> u32 {
        channel
            .and_then(|key| self.channels.get(&key))
            .map_or(0, |held| held.rock)
    }

    /// Create a channel at full volume. The null channel comes
    /// back where sound cannot play (Glk: Creating and Destroying
    /// Sound Channels).
    pub fn glk_schannel_create(&mut self, rock: u32) -> Option<u32> {
        self.glk_schannel_create_ext(rock, FULL_VOLUME)
    }

    /// Create a channel, or None where sound cannot play.
    pub fn glk_schannel_create_ext(&mut self, rock: u32, volume: u32) -> Option<u32> {
        if !self.frontend.sound() {
            return None;
        }

        let key = self.mint();

        self.channels.insert(key, SoundChannel::new(volume, rock));
        self.channel_order.insert(0, key);

        Some(key)
    }

    /// Stop and drop a channel.
    pub fn glk_schannel_destroy(&mut self, channel: Option<u32>) {
        let Some(key) = channel.filter(|held| self.channels.contains_key(held)) else {
            return;
        };

        self.glk_schannel_stop(Some(key));

        self.channel_order.retain(|held| *held != key);
        self.channels.remove(&key);
        self.disposals.push((CLASS_SCHANNEL, key));
    }

    /// Play a sound once (Glk: Playing Sounds).
    pub fn glk_schannel_play(&mut self, channel: Option<u32>, sound: u32) -> u32 {
        self.glk_schannel_play_ext(channel, sound, 1, 0)
    }

    /// Play a sound repeatedly; return whether it took (Glk:
    /// Playing Sounds).
    pub fn glk_schannel_play_ext(
        &mut self,
        channel: Option<u32>,
        sound: u32,
        repeats: u32,
        notify: u32,
    ) -> u32 {
        let Some(key) = channel.filter(|held| self.channels.contains_key(held)) else {
            return 0;
        };

        self.glk_schannel_stop(Some(key));

        if repeats == 0 || self.resources.sound(sound).is_none() {
            // Zero repeats is a legal way to say "stop and play
            // nothing" (Glk: Playing Sounds).
            return 0;
        }

        let snapshot = self.channels[&key].clone();

        if !self.frontend.play_sound(&snapshot, sound, repeats, notify) {
            return 0;
        }

        let held = self.channels.get_mut(&key).unwrap();

        held.sound = sound;
        held.repeats = repeats;
        held.notify = notify;
        held.paused = false;

        1
    }

    /// Start channels together; return how many took (Glk: Playing
    /// Sounds).
    pub fn glk_schannel_play_multi(
        &mut self,
        memory: &Memory,
        channels: &[Option<u32>],
        sounds: Option<MemArray>,
        notify: u32,
    ) -> Outcome<u32> {
        let mut started = 0;

        if let Some(sounds) = sounds {
            for (index, channel) in channels.iter().enumerate() {
                if index as u32 >= sounds.count {
                    break;
                }

                let sound = sounds.get(memory, index as u32)?;

                started += self.glk_schannel_play_ext(*channel, sound, 1, notify);
            }
        }

        Ok(started)
    }

    /// Silence a channel (Glk: Playing Sounds).
    pub fn glk_schannel_stop(&mut self, channel: Option<u32>) {
        let Some(key) = channel else {
            return;
        };

        let playing = self.channels.get(&key).is_some_and(|held| held.sound != 0);

        if playing {
            let snapshot = self.channels[&key].clone();

            self.frontend.stop_sound(&snapshot);

            let held = self.channels.get_mut(&key).unwrap();

            held.sound = 0;
            held.paused = false;
        }
    }

    /// Hold a channel where it is (Glk: Playing Sounds).
    pub fn glk_schannel_pause(&mut self, channel: Option<u32>) {
        self.set_paused(channel, true);
    }

    /// Let a held channel continue.
    pub fn glk_schannel_unpause(&mut self, channel: Option<u32>) {
        self.set_paused(channel, false);
    }

    fn set_paused(&mut self, channel: Option<u32>, paused: bool) {
        let Some(key) = channel else {
            return;
        };

        let change = self
            .channels
            .get(&key)
            .is_some_and(|held| held.paused != paused);

        if change {
            self.channels.get_mut(&key).unwrap().paused = paused;

            let snapshot = self.channels[&key].clone();

            self.frontend.pause_sound(&snapshot, paused);
        }
    }

    /// Set a channel's volume at once (Glk: Other Sound Channel
    /// Functions).
    pub fn glk_schannel_set_volume(&mut self, channel: Option<u32>, volume: u32) {
        self.glk_schannel_set_volume_ext(channel, volume, 0, 0);
    }

    /// Set a channel's volume, with optional fade and notify (Glk:
    /// Other Sound Channel Functions).
    pub fn glk_schannel_set_volume_ext(
        &mut self,
        channel: Option<u32>,
        volume: u32,
        duration: u32,
        notify: u32,
    ) {
        let Some(key) = channel.filter(|held| self.channels.contains_key(held)) else {
            return;
        };

        self.channels.get_mut(&key).unwrap().volume = volume;

        let snapshot = self.channels[&key].clone();

        self.frontend.set_volume(&snapshot, volume, duration);

        if notify != 0 {
            self.post_event(Event::new(event_type::VOLUME_NOTIFY, None, 0, notify));
        }
    }

    /// Advisory only: a sound is (or is not) about to be used.
    pub fn glk_sound_load_hint(&mut self, _sound: u32, _flag: u32) {}

    // -- output (Glk: How To Print) ----------------------------------------

    /// Print one character through a stream; the window handoff
    /// and any echo-stream copy happen here, since they cross
    /// objects. Echo first, then content -- the reference's own
    /// order.
    fn put_to_stream(
        &mut self,
        memory: &mut Memory,
        stream: Option<u32>,
        character: u32,
    ) -> Outcome<()> {
        let Some(key) = stream else {
            return Ok(());
        };
        let Some(held) = self.streams.get_mut(&key) else {
            return Ok(());
        };

        if let Some(character) = held.put_char(memory, character)? {
            let StreamKind::Window(wkey) = self.streams[&key].kind else {
                unreachable!("only a window stream hands characters back");
            };

            let echo = self
                .windows
                .get(&wkey)
                .and_then(|window| window.echo_stream);

            if let Some(echo) = echo {
                self.put_to_stream(memory, Some(echo), character)?;
            }

            let hyperlink = self.streams.get(&key).map_or(0, |held| held.hyperlink);

            if let Some(window) = self.windows.get_mut(&wkey) {
                window.put_char(character, hyperlink);
            }
        }

        Ok(())
    }

    /// Print one Latin-1 character to the current stream.
    pub fn glk_put_char(&mut self, memory: &mut Memory, ch: u32) -> Outcome<()> {
        self.put_to_stream(memory, self.current_stream, ch & 0xFF)
    }

    /// Print one Unicode character to the current stream.
    pub fn glk_put_char_uni(&mut self, memory: &mut Memory, ch: u32) -> Outcome<()> {
        self.put_to_stream(memory, self.current_stream, ch)
    }

    /// Print one Latin-1 character to a named stream.
    pub fn glk_put_char_stream(
        &mut self,
        memory: &mut Memory,
        stream: Option<u32>,
        ch: u32,
    ) -> Outcome<()> {
        self.put_to_stream(memory, stream, ch & 0xFF)
    }

    /// Print one Unicode character to a named stream.
    pub fn glk_put_char_stream_uni(
        &mut self,
        memory: &mut Memory,
        stream: Option<u32>,
        ch: u32,
    ) -> Outcome<()> {
        self.put_to_stream(memory, stream, ch)
    }

    /// Print a string to the current stream.
    pub fn glk_put_string(&mut self, memory: &mut Memory, text: &str) -> Outcome<()> {
        self.glk_put_string_stream(memory, self.current_stream, text)
    }

    /// Print a Unicode string to the current stream.
    pub fn glk_put_string_uni(&mut self, memory: &mut Memory, text: &str) -> Outcome<()> {
        self.glk_put_string_stream(memory, self.current_stream, text)
    }

    /// Print a string to a named stream.
    pub fn glk_put_string_stream(
        &mut self,
        memory: &mut Memory,
        stream: Option<u32>,
        text: &str,
    ) -> Outcome<()> {
        for character in text.chars() {
            self.put_to_stream(memory, stream, u32::from(character))?;
        }

        Ok(())
    }

    /// Print a Unicode string to a named stream.
    pub fn glk_put_string_stream_uni(
        &mut self,
        memory: &mut Memory,
        stream: Option<u32>,
        text: &str,
    ) -> Outcome<()> {
        self.glk_put_string_stream(memory, stream, text)
    }

    /// Print an array of characters to the current stream.
    pub fn glk_put_buffer(&mut self, memory: &mut Memory, buf: Option<MemArray>) -> Outcome<()> {
        self.glk_put_buffer_stream(memory, self.current_stream, buf)
    }

    /// Print an array of Unicode characters.
    pub fn glk_put_buffer_uni(
        &mut self,
        memory: &mut Memory,
        buf: Option<MemArray>,
    ) -> Outcome<()> {
        self.glk_put_buffer_stream(memory, self.current_stream, buf)
    }

    /// Print an array of characters to a named stream.
    pub fn glk_put_buffer_stream(
        &mut self,
        memory: &mut Memory,
        stream: Option<u32>,
        buf: Option<MemArray>,
    ) -> Outcome<()> {
        let Some(buf) = buf else {
            return Ok(());
        };

        for index in 0..buf.count {
            let value = buf.get(memory, index)?;

            self.put_to_stream(memory, stream, value)?;
        }

        Ok(())
    }

    /// Print an array of Unicode characters to a named stream.
    pub fn glk_put_buffer_stream_uni(
        &mut self,
        memory: &mut Memory,
        stream: Option<u32>,
        buf: Option<MemArray>,
    ) -> Outcome<()> {
        self.glk_put_buffer_stream(memory, stream, buf)
    }

    /// Choose the style of coming output (Glk: Styles).
    pub fn glk_set_style(&mut self, value: u32) {
        self.glk_set_style_stream(self.current_stream, value);
    }

    /// Choose a stream's style; only window streams show one.
    pub fn glk_set_style_stream(&mut self, stream: Option<u32>, value: u32) {
        let window = match stream.and_then(|key| self.streams.get(&key)) {
            Some(Stream {
                kind: StreamKind::Window(wkey),
                ..
            }) => Some(*wkey),
            _ => None,
        };

        if let Some(held) = window.and_then(|key| self.windows.get_mut(&key)) {
            held.style = value;
        }
    }

    // -- input from streams (Glk: How To Read) -----------------------------

    /// Read one character, or -1 at the end.
    pub fn glk_get_char_stream(&mut self, memory: &Memory, stream: Option<u32>) -> Outcome<i64> {
        match stream.and_then(|key| self.streams.get_mut(&key)) {
            Some(held) => Ok(held.get_char(memory)?),
            None => Ok(-1),
        }
    }

    /// Read one Unicode character, or -1 at the end.
    pub fn glk_get_char_stream_uni(
        &mut self,
        memory: &Memory,
        stream: Option<u32>,
    ) -> Outcome<i64> {
        self.glk_get_char_stream(memory, stream)
    }

    /// Fill a buffer from a stream; return the count read.
    pub fn glk_get_buffer_stream(
        &mut self,
        memory: &mut Memory,
        stream: Option<u32>,
        buf: Option<MemArray>,
    ) -> Outcome<u32> {
        match (stream.and_then(|key| self.streams.get_mut(&key)), buf) {
            (Some(held), Some(buf)) => Ok(held.get_buffer(memory, buf)?),
            _ => Ok(0),
        }
    }

    /// Fill a word buffer from a stream.
    pub fn glk_get_buffer_stream_uni(
        &mut self,
        memory: &mut Memory,
        stream: Option<u32>,
        buf: Option<MemArray>,
    ) -> Outcome<u32> {
        self.glk_get_buffer_stream(memory, stream, buf)
    }

    /// Read a line from a stream; return the count read.
    pub fn glk_get_line_stream(
        &mut self,
        memory: &mut Memory,
        stream: Option<u32>,
        buf: Option<MemArray>,
    ) -> Outcome<u32> {
        match (stream.and_then(|key| self.streams.get_mut(&key)), buf) {
            (Some(held), Some(buf)) => Ok(held.get_line(memory, buf)?),
            _ => Ok(0),
        }
    }

    /// Read a line of Unicode characters from a stream.
    pub fn glk_get_line_stream_uni(
        &mut self,
        memory: &mut Memory,
        stream: Option<u32>,
        buf: Option<MemArray>,
    ) -> Outcome<u32> {
        self.glk_get_line_stream(memory, stream, buf)
    }

    // -- style hints (Glk: Suggesting the Appearance of Styles) ------------

    /// Record a styling suggestion for a display to honor.
    pub fn glk_stylehint_set(&mut self, wtype: u32, styl: u32, hint: u32, value: u32) {
        self.stylehints.insert((wtype, styl, hint), value);
    }

    /// Withdraw a styling suggestion.
    pub fn glk_stylehint_clear(&mut self, wtype: u32, styl: u32, hint: u32) {
        self.stylehints.remove(&(wtype, styl, hint));
    }

    /// Whether two styles look different (Glk: Testing the
    /// Appearance of Styles). Only the display knows; one that
    /// cannot say answers no, which is what the spec asks of the
    /// unsure.
    pub fn glk_style_distinguish(&self, window: Option<u32>, style1: u32, style2: u32) -> u32 {
        let Some(held) = window.and_then(|key| self.windows.get(&key)) else {
            return 0;
        };

        if style1 == style2 {
            return 0;
        }

        u32::from(self.frontend.style_distinguish(held, style1, style2))
    }

    /// Measure one attribute of a style, if the display can.
    pub fn glk_style_measure(
        &self,
        window: Option<u32>,
        styl: u32,
        hint: u32,
        resultref: Option<&mut RefSlot>,
    ) -> u32 {
        let Some(held) = window.and_then(|key| self.windows.get(&key)) else {
            return 0;
        };

        let Some(measured) = self.frontend.style_measure(held, styl, hint) else {
            return 0;
        };

        if let Some(slot) = resultref {
            slot.0 = Held::Word(measured);
        }

        1
    }

    // -- graphics (Glk: Graphics) ------------------------------------------

    /// Report a picture's size. Answered from the resource bytes,
    /// so it works even where nothing can be drawn (Glk: Testing
    /// for Graphics Capabilities).
    pub fn glk_image_get_info(
        &mut self,
        image: u32,
        widthref: Option<&mut RefSlot>,
        heightref: Option<&mut RefSlot>,
    ) -> u32 {
        let info = self.resources.image(image);
        let (width, height) = info.map_or((0, 0), |held| (held.width, held.height));
        let found = info.is_some();

        if let Some(slot) = widthref {
            slot.0 = Held::Word(width);
        }

        if let Some(slot) = heightref {
            slot.0 = Held::Word(height);
        }

        u32::from(found)
    }

    /// Draw a picture at its own size (Glk: Graphics in Graphics
    /// Windows).
    pub fn glk_image_draw(&mut self, window: Option<u32>, image: u32, val1: i64, val2: i64) -> u32 {
        self.draw(window, image, val1, val2, None, None)
    }

    /// Draw a picture scaled to a size.
    pub fn glk_image_draw_scaled(
        &mut self,
        window: Option<u32>,
        image: u32,
        val1: i64,
        val2: i64,
        width: u32,
        height: u32,
    ) -> u32 {
        self.draw(window, image, val1, val2, Some(width), Some(height))
    }

    /// Draw a picture under the extended scaling rules. The rules
    /// beyond plain scaling are aspect-ratio hints for the
    /// display; the display era decides how far to honor them, so
    /// they pass through untouched here.
    #[allow(clippy::too_many_arguments)] // the call's own eight values
    pub fn glk_image_draw_scaled_ext(
        &mut self,
        window: Option<u32>,
        image: u32,
        val1: i64,
        val2: i64,
        width: u32,
        height: u32,
        _imagerule: u32,
        _maxwidth: u32,
    ) -> u32 {
        self.draw(window, image, val1, val2, Some(width), Some(height))
    }

    /// Hand a measured picture to the display, if there is one.
    fn draw(
        &mut self,
        window: Option<u32>,
        image: u32,
        val1: i64,
        val2: i64,
        width: Option<u32>,
        height: Option<u32>,
    ) -> u32 {
        let Some(info) = self.resources.image(image).cloned() else {
            return 0;
        };
        let Some(wkey) = window.filter(|key| self.windows.contains_key(key)) else {
            return 0;
        };

        u32::from(self.frontend.draw_image(
            &mut self.windows,
            wkey,
            &info,
            val1,
            val2,
            width.unwrap_or(info.width),
            height.unwrap_or(info.height),
        ))
    }

    /// Break text past the margin images (Glk: Graphics in Text
    /// Buffer Windows).
    pub fn glk_window_flow_break(&mut self, window: Option<u32>) {
        if let Some(key) = window.filter(|key| self.windows.contains_key(key)) {
            self.frontend.flow_break(key);
        }
    }

    /// Erase a rectangle to the background (Glk: Graphics in
    /// Graphics Windows).
    pub fn glk_window_erase_rect(
        &mut self,
        window: Option<u32>,
        left: i64,
        top: i64,
        width: u32,
        height: u32,
    ) {
        if let Some(key) = window.filter(|key| self.windows.contains_key(key)) {
            self.frontend.erase_rect(key, left, top, width, height);
        }
    }

    /// Fill a rectangle with a color.
    #[allow(clippy::too_many_arguments)] // the rectangle, colored
    pub fn glk_window_fill_rect(
        &mut self,
        window: Option<u32>,
        color: u32,
        left: i64,
        top: i64,
        width: u32,
        height: u32,
    ) {
        if let Some(key) = window.filter(|key| self.windows.contains_key(key)) {
            self.frontend
                .fill_rect(key, color, left, top, width, height);
        }
    }

    /// Choose the color future clears fill with.
    pub fn glk_window_set_background_color(&mut self, window: Option<u32>, color: u32) {
        if let Some(key) = window.filter(|key| self.windows.contains_key(key)) {
            self.frontend.set_background_color(key, color);
        }
    }

    // -- hyperlinks (Glk: Hyperlinks) --------------------------------------

    /// Mark coming output as a link (Glk: Creating Hyperlinks).
    pub fn glk_set_hyperlink(&mut self, linkval: u32) {
        self.glk_set_hyperlink_stream(self.current_stream, linkval);
    }

    /// Everything written from here on belongs to this link.
    pub fn glk_set_hyperlink_stream(&mut self, stream: Option<u32>, linkval: u32) {
        if let Some(held) = stream.and_then(|key| self.streams.get_mut(&key)) {
            held.hyperlink = linkval;
        }
    }

    /// Ask for a link selection (Glk: Accepting Hyperlink Events).
    pub fn glk_request_hyperlink_event(&mut self, window: Option<u32>) {
        if let Some(held) = window.and_then(|key| self.windows.get_mut(&key)) {
            held.hyperlink_request = true;
        }
    }

    /// Withdraw the link request.
    pub fn glk_cancel_hyperlink_event(&mut self, window: Option<u32>) {
        if let Some(held) = window.and_then(|key| self.windows.get_mut(&key)) {
            held.hyperlink_request = false;
        }
    }

    // -- mouse input (Glk: Mouse Input Events) -----------------------------

    /// Ask for a click in a grid or graphics window.
    pub fn glk_request_mouse_event(&mut self, window: Option<u32>) {
        if let Some(held) = window.and_then(|key| self.windows.get_mut(&key)) {
            held.mouse_request = true;
        }
    }

    /// Withdraw the click request.
    pub fn glk_cancel_mouse_event(&mut self, window: Option<u32>) {
        if let Some(held) = window.and_then(|key| self.windows.get_mut(&key)) {
            held.mouse_request = false;
        }
    }

    // -- events (Glk: Events) ----------------------------------------------

    /// Wait until something happens, then report it.
    ///
    /// A blocking display is asked for input on the spot and the
    /// struct fills before this returns. A suspending display is
    /// never asked: whatever is already queued is delivered, and
    /// otherwise the wait is recorded for the host, who answers
    /// through deliver_event once the event arrives (Glk: Events).
    pub fn glk_select(&mut self, memory: &mut Memory, event: &mut StructSlot) -> Outcome<()> {
        if !self.frontend.suspends() {
            let result = self.wait_for_event(memory)?;

            fill_event(event, result);

            return Ok(());
        }

        self.frontend.flush(&mut self.windows, self.root);

        if !self.pending_events.is_empty() {
            fill_event(event, self.pending_events.remove(0));

            return Ok(());
        }

        if !self.awaited() {
            return fault("glk_select with no input requested: the game would wait forever");
        }

        self.waiting = Some(Waiting::Select);

        Ok(())
    }

    /// Whether any outstanding request can ever be answered.
    ///
    /// A request counts only where the display claims the matching
    /// capability, the same rule the blocking loop enforces one
    /// refusal at a time. A running timer counts too: a suspending
    /// display's host raises timer events itself, which is more
    /// than a blocking display can promise when no input is
    /// requested alongside.
    fn awaited(&self) -> bool {
        let windows = || {
            self.window_order
                .iter()
                .filter_map(|key| self.windows.get(key))
        };

        if windows().any(|held| held.line_request.is_some() || held.char_request) {
            return true;
        }

        if self.frontend.mouse_input() && windows().any(|held| held.mouse_request) {
            return true;
        }

        if self.frontend.hyperlink_input() && windows().any(|held| held.hyperlink_request) {
            return true;
        }

        self.frontend.timer_input() && self.timer_interval != 0
    }

    /// Complete a suspended select with the event a host
    /// collected; the event comes back for the bridge to land in
    /// VM memory, exactly where the game will look when it steps
    /// on.
    pub fn deliver_event(&mut self, event: Event) -> Outcome<Event> {
        if self.waiting != Some(Waiting::Select) {
            return fault("an event arrived with no select suspended to receive it");
        }

        self.waiting = None;

        Ok(event)
    }

    /// Report a queued non-input event without waiting.
    ///
    /// A poll must never return input, but it may return the
    /// events a display raises by itself -- a timer, a resize, a
    /// sound ending (Glk: Other Events). Those are exactly the
    /// ones sitting in the pending queue.
    pub fn glk_select_poll(&mut self, event: &mut StructSlot) {
        let found = self
            .pending_events
            .iter()
            .position(|queued| pollable(queued.kind));

        match found {
            Some(index) => fill_event(event, self.pending_events.remove(index)),
            None => fill_event(event, Event::none()),
        }
    }

    /// Re-lay the windows after the display changed size, and tell
    /// the game, so it can redraw anything it keeps track of
    /// itself (Glk: Window Arrangement Events).
    pub fn display_resized(&mut self) {
        self.re_lay();

        self.post_event(Event::new(event_type::ARRANGE, self.root, 0, 0));
    }

    /// Queue an event for the next select. Glk delivers these
    /// asynchronously; a blocking display has no other way to
    /// raise one.
    pub fn post_event(&mut self, event: Event) {
        self.pending_events.push(event);
    }

    /// Block until something happens, then report it.
    ///
    /// The loop exists for interruptions: a display may answer
    /// `Instead` from an input call because a timer fired, in
    /// which case the input request stays pending and we come
    /// round again to pick the queued event up.
    fn wait_for_event(&mut self, memory: &mut Memory) -> Outcome<Event> {
        loop {
            self.frontend.flush(&mut self.windows, self.root);

            if !self.pending_events.is_empty() {
                return Ok(self.pending_events.remove(0));
            }

            let find = |test: fn(&Window) -> bool| {
                self.window_order
                    .iter()
                    .copied()
                    .find(|key| self.windows.get(key).is_some_and(test))
            };

            if let Some(wkey) = find(|held| held.line_request.is_some()) {
                let maxlen = self.windows[&wkey]
                    .line_request
                    .as_ref()
                    .map_or(0, LineRequest::capacity);

                match self.frontend.read_line(&mut self.windows, wkey, maxlen) {
                    Asked::Answer((text, terminator)) => {
                        return self.deliver_line(memory, wkey, &text, terminator);
                    }
                    Asked::Instead(events) => {
                        self.pending_events.extend(events);

                        continue;
                    }
                    Asked::End => return Err(Stop::End),
                }
            }

            if let Some(wkey) = find(|held| held.char_request) {
                match self.frontend.read_char(&mut self.windows, wkey) {
                    Asked::Answer(value) => return self.deliver_char(wkey, value),
                    Asked::Instead(events) => {
                        self.pending_events.extend(events);

                        continue;
                    }
                    Asked::End => return Err(Stop::End),
                }
            }

            if let Some(wkey) = find(|held| held.mouse_request) {
                if let Some((x, y)) = self.frontend.read_mouse(wkey) {
                    return self.deliver_mouse(wkey, x, y);
                }

                if self.frontend.mouse_input() {
                    // It can click, so this was an interruption,
                    // not a refusal: come round again. A display
                    // that cannot click falls through to the error
                    // below instead.
                    continue;
                }
            }

            if let Some(wkey) = find(|held| held.hyperlink_request) {
                let value = self.frontend.read_hyperlink(wkey);

                if let Some(value) = value.filter(|value| *value != 0) {
                    return self.deliver_hyperlink(wkey, value);
                }

                if self.frontend.hyperlink_input() {
                    continue;
                }
            }

            return fault("glk_select with no input requested: the game would wait forever");
        }
    }

    /// Complete a window's line request with text from anywhere.
    ///
    /// Split out from the display ask because a display need not
    /// be asked for the window it answers about: a protocol
    /// display gets told which window the player typed into, which
    /// may not be the one glk_select happened to ask after.
    pub fn deliver_line(
        &mut self,
        memory: &mut Memory,
        window: u32,
        text: &str,
        terminator: u32,
    ) -> Outcome<Event> {
        let request = self
            .windows
            .get_mut(&window)
            .and_then(|held| held.line_request.take());

        let Some(request) = request else {
            return fault("line input delivered to a window not expecting it");
        };

        let length = fill_array(memory, request.buf, text.chars().map(u32::from))?;

        let echoes = request.echo
            && !self.frontend.echoes_input()
            && matches!(
                self.windows.get(&window).map(|held| &held.kind),
                Some(WindowKind::Buffer(_))
            );

        if echoes {
            // The line the player typed becomes part of the
            // window's text, in the Input style (Glk: Line Input
            // Events).
            let stream = self.windows[&window].stream;
            let previous = self.windows[&window].style;

            self.windows.get_mut(&window).unwrap().style = style::INPUT;

            for character in text.chars().take(length as usize) {
                self.put_to_stream(memory, Some(stream), u32::from(character))?;
            }

            self.put_to_stream(memory, Some(stream), NEWLINE)?;

            self.windows.get_mut(&window).unwrap().style = previous;
        }

        Ok(Event::new(
            event_type::LINE_INPUT,
            Some(window),
            length,
            terminator,
        ))
    }

    /// Complete a window's character request.
    pub fn deliver_char(&mut self, window: u32, value: u32) -> Outcome<Event> {
        let asked = self
            .windows
            .get(&window)
            .is_some_and(|held| held.char_request);

        if !asked {
            return fault("character input delivered to a window not expecting it");
        }

        self.windows.get_mut(&window).unwrap().char_request = false;

        Ok(Event::new(event_type::CHAR_INPUT, Some(window), value, 0))
    }

    /// Complete a window's mouse request with a clicked position.
    pub fn deliver_mouse(&mut self, window: u32, x: u32, y: u32) -> Outcome<Event> {
        let asked = self
            .windows
            .get(&window)
            .is_some_and(|held| held.mouse_request);

        if !asked {
            return fault("mouse input delivered to a window not expecting it");
        }

        self.windows.get_mut(&window).unwrap().mouse_request = false;

        Ok(Event::new(event_type::MOUSE_INPUT, Some(window), x, y))
    }

    /// Complete a window's hyperlink request with a link value.
    pub fn deliver_hyperlink(&mut self, window: u32, value: u32) -> Outcome<Event> {
        let asked = self
            .windows
            .get(&window)
            .is_some_and(|held| held.hyperlink_request);

        if !asked {
            return fault("hyperlink input delivered to a window not expecting it");
        }

        self.windows.get_mut(&window).unwrap().hyperlink_request = false;

        Ok(Event::new(event_type::HYPERLINK, Some(window), value, 0))
    }

    /// Ask for a line of Latin-1 input (Glk: Line Input Events).
    pub fn glk_request_line_event(
        &mut self,
        window: Option<u32>,
        buf: Option<MemArray>,
        initlen: u32,
    ) -> Outcome<()> {
        self.request_line(window, buf, initlen, false)
    }

    /// Ask for a line of Unicode input.
    pub fn glk_request_line_event_uni(
        &mut self,
        window: Option<u32>,
        buf: Option<MemArray>,
        initlen: u32,
    ) -> Outcome<()> {
        self.request_line(window, buf, initlen, true)
    }

    fn request_line(
        &mut self,
        window: Option<u32>,
        buf: Option<MemArray>,
        initlen: u32,
        unicode: bool,
    ) -> Outcome<()> {
        let Some(held) = window.and_then(|key| self.windows.get_mut(&key)) else {
            return fault("request_line_event: invalid window");
        };

        if held.line_request.is_some() {
            return fault("request_line_event: input already requested");
        }

        held.line_request = Some(LineRequest::new(buf, initlen, unicode));

        Ok(())
    }

    /// Withdraw a line request (Glk: Line Input Events).
    ///
    /// The full spec behavior returns any partial input; with a
    /// blocking display there is never any, so the answer is the
    /// no-event.
    pub fn glk_cancel_line_event(&mut self, window: Option<u32>, event: Option<&mut StructSlot>) {
        if let Some(held) = window.and_then(|key| self.windows.get_mut(&key)) {
            held.line_request = None;
        }

        if let Some(slot) = event {
            fill_event(slot, Event::none());
        }
    }

    /// Ask for one Latin-1 keystroke (Glk: Character Input
    /// Events).
    pub fn glk_request_char_event(&mut self, window: Option<u32>) -> Outcome<()> {
        self.request_char(window, false)
    }

    /// Ask for one Unicode keystroke.
    pub fn glk_request_char_event_uni(&mut self, window: Option<u32>) -> Outcome<()> {
        self.request_char(window, true)
    }

    fn request_char(&mut self, window: Option<u32>, unicode: bool) -> Outcome<()> {
        let Some(held) = window.and_then(|key| self.windows.get_mut(&key)) else {
            return fault("request_char_event: invalid window");
        };

        held.char_request = true;
        held.char_unicode = unicode;

        Ok(())
    }

    /// Withdraw a character request.
    pub fn glk_cancel_char_event(&mut self, window: Option<u32>) {
        if let Some(held) = window.and_then(|key| self.windows.get_mut(&key)) {
            held.char_request = false;
        }
    }

    /// Ask for a timer event every so often; zero stops them (Glk:
    /// Timer Events).
    pub fn glk_request_timer_events(&mut self, millisecs: u32) {
        self.timer_interval = millisecs;

        self.frontend.set_timer(millisecs);
    }

    /// Choose whether the pending line echoes (Glk: Line Input
    /// Events).
    pub fn glk_set_echo_line_event(&mut self, window: Option<u32>, value: u32) {
        if let Some(request) = window
            .and_then(|key| self.windows.get_mut(&key))
            .and_then(|held| held.line_request.as_mut())
        {
            request.echo = value != 0;
        }
    }

    /// Choose the special keys that may end the pending line.
    pub fn glk_set_terminators_line_event(
        &mut self,
        memory: &Memory,
        window: Option<u32>,
        keycodes: Option<MemArray>,
    ) -> Outcome<()> {
        let mut terminators = Vec::new();

        if let Some(keycodes) = keycodes {
            for index in 0..keycodes.count {
                terminators.push(keycodes.get(memory, index)?);
            }
        }

        if let Some(request) = window
            .and_then(|key| self.windows.get_mut(&key))
            .and_then(|held| held.line_request.as_mut())
        {
            request.terminators = terminators;
        }

        Ok(())
    }

    // -- the system clock (Glk: The System Clock) --------------------------

    fn now(&self) -> f64 {
        self.now_override.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |lived| lived.as_secs_f64())
        })
    }

    /// Store the current Unix time as a glktimeval_t.
    pub fn glk_current_time(&self, timeref: Option<&mut StructSlot>) {
        let now = self.now();
        let seconds = now as i64;
        let (high, low) = split_seconds(seconds);
        let microsec = ((now - seconds as f64) * 1_000_000.0) as i64;

        if let Some(slot) = timeref {
            slot.set_all(&[
                Held::Word(high),
                Held::Word(low),
                Held::Word(microsec as u32),
            ]);
        }
    }

    /// The Unix time divided down, rounding toward the past.
    pub fn glk_current_simple_time(&self, factor: u32) -> i64 {
        if factor == 0 {
            return -1;
        }

        (self.now() as i64).div_euclid(i64::from(factor))
    }

    /// Explode a timestamp into a UTC glkdate_t (Glk: Time and
    /// Date Conversions).
    pub fn glk_time_to_date_utc(
        &self,
        timeref: Option<&StructSlot>,
        dateref: Option<&mut StructSlot>,
    ) {
        self.time_to_date(timeref, dateref, true);
    }

    /// Explode a timestamp into a local-time glkdate_t.
    pub fn glk_time_to_date_local(
        &self,
        timeref: Option<&StructSlot>,
        dateref: Option<&mut StructSlot>,
    ) {
        self.time_to_date(timeref, dateref, false);
    }

    fn time_to_date(
        &self,
        timeref: Option<&StructSlot>,
        dateref: Option<&mut StructSlot>,
        utc: bool,
    ) {
        let Some(slot) = dateref else {
            return;
        };

        let Some(timeref) = timeref else {
            slot.set_all(&[Held::Word(0); 8]);

            return;
        };

        let high = timeref.0[0].signed();
        let low = i64::from(timeref.0[1].word());
        let microsec = timeref.0[2].signed();

        fill_date(slot, self.break_out((high << 32) | low, microsec, utc));
    }

    /// Explode a divided-down time into a UTC date.
    pub fn glk_simple_time_to_date_utc(
        &self,
        time: i64,
        factor: u32,
        dateref: Option<&mut StructSlot>,
    ) {
        self.simple_to_date(time, factor, dateref, true);
    }

    /// Explode a divided-down time into a local date.
    pub fn glk_simple_time_to_date_local(
        &self,
        time: i64,
        factor: u32,
        dateref: Option<&mut StructSlot>,
    ) {
        self.simple_to_date(time, factor, dateref, false);
    }

    fn simple_to_date(&self, time: i64, factor: u32, dateref: Option<&mut StructSlot>, utc: bool) {
        if let Some(slot) = dateref {
            // Resolution is whole seconds, so microseconds come
            // back zero (Glk: Time and Date Conversions).
            fill_date(
                slot,
                self.break_out(time.saturating_mul(i64::from(factor)), 0, utc),
            );
        }
    }

    /// Collapse a UTC date into a glktimeval_t.
    pub fn glk_date_to_time_utc(
        &self,
        dateref: Option<&StructSlot>,
        timeref: Option<&mut StructSlot>,
    ) {
        self.date_to_time(dateref, timeref, true);
    }

    /// Collapse a local date into a glktimeval_t.
    pub fn glk_date_to_time_local(
        &self,
        dateref: Option<&StructSlot>,
        timeref: Option<&mut StructSlot>,
    ) {
        self.date_to_time(dateref, timeref, false);
    }

    fn date_to_time(
        &self,
        dateref: Option<&StructSlot>,
        timeref: Option<&mut StructSlot>,
        utc: bool,
    ) {
        let Some(slot) = timeref else {
            return;
        };

        let seconds = dateref.and_then(|held| self.to_seconds(held, utc));

        match seconds {
            None => {
                // An unrepresentable time is -1 in both words
                // (Glk: Time and Date Conversions).
                slot.set_all(&[
                    Held::Word(0xFFFF_FFFF),
                    Held::Word(0xFFFF_FFFF),
                    Held::Word(0),
                ]);
            }
            Some(seconds) => {
                let (high, low) = split_seconds(seconds);
                let microsec = dateref.map_or(0, |held| held.0[7].signed().rem_euclid(MICROS));

                slot.set_all(&[
                    Held::Word(high),
                    Held::Word(low),
                    Held::Word(microsec as u32),
                ]);
            }
        }
    }

    /// Collapse a UTC date into a divided-down time.
    pub fn glk_date_to_simple_time_utc(&self, dateref: Option<&StructSlot>, factor: u32) -> i64 {
        self.date_to_simple(dateref, factor, true)
    }

    /// Collapse a local date into a divided-down time.
    pub fn glk_date_to_simple_time_local(&self, dateref: Option<&StructSlot>, factor: u32) -> i64 {
        self.date_to_simple(dateref, factor, false)
    }

    fn date_to_simple(&self, dateref: Option<&StructSlot>, factor: u32, utc: bool) -> i64 {
        if factor == 0 {
            return -1;
        }

        match dateref.and_then(|held| self.to_seconds(held, utc)) {
            None => -1,
            Some(seconds) => seconds.div_euclid(i64::from(factor)),
        }
    }

    /// Explode a timestamp into the eight fields of a glkdate_t,
    /// or zeros for a second count past every calendar.
    fn break_out(&self, seconds: i64, microsec: i64, utc: bool) -> [i64; 8] {
        let local = if utc {
            seconds
        } else {
            seconds.saturating_add(self.local_offset_seconds)
        };

        let days = local.div_euclid(86_400);
        let in_day = local.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);

        if !(1..=9999).contains(&year) {
            return [0; 8];
        }

        [
            year,
            month,
            day,
            // Glk counts weekdays from Sunday (Glk: The System
            // Clock).
            (days + 4).rem_euclid(7),
            in_day / 3600,
            (in_day / 60) % 60,
            in_day % 60,
            microsec,
        ]
    }

    /// Turn glkdate_t fields into a timestamp, or None if
    /// impossible.
    ///
    /// The fields "need not be in their normal ranges; they will
    /// be normalized" (Glk: Time and Date Conversions). Months are
    /// normalized by hand because they have no fixed length;
    /// everything else is a plain offset from the first of the
    /// month, which lets a day of 40 or an hour of -3 work.
    fn to_seconds(&self, date: &StructSlot, utc: bool) -> Option<i64> {
        if date.0.len() < 8 {
            return None;
        }

        let field = |index: usize| date.0[index].signed();
        let (year, month) = (field(0), field(1));
        let year = year + (month - 1).div_euclid(12);
        let month = (month - 1).rem_euclid(12) + 1;

        if !(1..=9999).contains(&year) {
            return None;
        }

        let base = days_from_civil(year, month, 1).checked_mul(86_400)?;
        let offset = (field(2) - 1).checked_mul(86_400)?;
        let clock = field(4)
            .checked_mul(3600)?
            .checked_add(field(5).checked_mul(60)?)?
            .checked_add(field(6))?;
        let micro = field(7);

        let mut seconds = base
            .checked_add(offset)?
            .checked_add(clock)?
            .checked_add(micro.div_euclid(MICROS))?;
        let frac = micro.rem_euclid(MICROS);

        if !utc {
            seconds = seconds.checked_sub(self.local_offset_seconds)?;
        }

        // The reference truncates the float timestamp toward zero;
        // only a negative time with a fractional tail differs from
        // the floor.
        if seconds < 0 && frac > 0 {
            seconds += 1;
        }

        Some(seconds)
    }

    // -- case mapping (Glk: Upper and Lower Case) --------------------------

    /// Lowercase one character, where one character can hold it.
    pub fn glk_char_to_lower(&self, ch: u32) -> u32 {
        map_case(ch, true)
    }

    /// Uppercase one character, where one character can hold it.
    pub fn glk_char_to_upper(&self, ch: u32) -> u32 {
        map_case(ch, false)
    }

    /// Lowercase a Unicode buffer in place.
    pub fn glk_buffer_to_lower_case_uni(
        &self,
        memory: &mut Memory,
        buf: Option<MemArray>,
        numchars: u32,
    ) -> Outcome<u32> {
        map_buffer(memory, buf, numchars, |held| held.to_lowercase().collect())
    }

    /// Uppercase a Unicode buffer in place.
    pub fn glk_buffer_to_upper_case_uni(
        &self,
        memory: &mut Memory,
        buf: Option<MemArray>,
        numchars: u32,
    ) -> Outcome<u32> {
        map_buffer(memory, buf, numchars, |held| held.to_uppercase().collect())
    }

    /// Title-case the first character (Glk: Upper and Lower Case).
    ///
    /// Titlecase is a third Unicode case, not a synonym for
    /// uppercase: the ligature U+FB04 uppercases to "FFL" but
    /// title-cases to "Ffl", and U+01C4 has the distinct titlecase
    /// form U+01C5.
    pub fn glk_buffer_to_title_case_uni(
        &self,
        memory: &mut Memory,
        buf: Option<MemArray>,
        numchars: u32,
        lowerrest: u32,
    ) -> Outcome<u32> {
        let chars = chars_of(memory, buf, numchars)?;

        if chars.is_empty() {
            return Ok(0);
        }

        let mut text = title_char(chars[0]);

        for held in &chars[1..] {
            if lowerrest != 0 {
                text.extend(held.to_lowercase());
            } else {
                text.push(*held);
            }
        }

        store_chars(memory, buf, &text)
    }

    /// Unicode NFD decomposition (Glk: Unicode String
    /// Normalization).
    pub fn glk_buffer_canon_decompose_uni(
        &self,
        memory: &mut Memory,
        buf: Option<MemArray>,
        numchars: u32,
    ) -> Outcome<u32> {
        let chars = chars_of(memory, buf, numchars)?;
        let text: String = chars.into_iter().nfd().collect();

        store_chars(memory, buf, &text)
    }

    /// Decompose, then canonically compose -- Unicode NFC.
    pub fn glk_buffer_canon_normalize_uni(
        &self,
        memory: &mut Memory,
        buf: Option<MemArray>,
        numchars: u32,
    ) -> Outcome<u32> {
        let chars = chars_of(memory, buf, numchars)?;
        let text: String = chars.into_iter().nfc().collect();

        store_chars(memory, buf, &text)
    }
}

// -- helpers ----------------------------------------------------------------

/// Build a window kind for a type, or None for a type not on
/// offer. The pair type only splitting creates (Glk: Pair
/// Windows); graphics without a drawing display answers null
/// rather than a fault, so a game can probe by trying.
fn make_window_kind(wtype: u32, graphics: bool) -> Outcome<Option<WindowKind>> {
    match wtype {
        window_type::PAIR => fault("window_open: cannot open a pair window directly"),
        window_type::BLANK => Ok(Some(WindowKind::Blank)),
        window_type::TEXT_BUFFER => Ok(Some(WindowKind::Buffer(BufferData::default()))),
        window_type::TEXT_GRID => Ok(Some(WindowKind::Grid(GridData::default()))),
        window_type::GRAPHICS if graphics => {
            Ok(Some(WindowKind::Graphics(GraphicsData::default())))
        }
        _ => Ok(None),
    }
}

/// One step of an object walk (Glk: Iterating Through Opaque
/// Objects). The null object starts the walk; the object after the
/// last -- and an object no longer on the list at all -- ends it.
fn iterate(
    order: &[u32],
    rocks: impl Fn(u32) -> u32,
    current: Option<u32>,
    rockref: Option<&mut RefSlot>,
) -> Option<u32> {
    let found = match current {
        None => order.first().copied(),
        Some(key) => order
            .iter()
            .position(|held| *held == key)
            .and_then(|index| order.get(index + 1).copied()),
    };

    if let Some(slot) = rockref {
        slot.0 = Held::Word(found.map_or(0, rocks));
    }

    found
}

/// Write values into a buffer from the start; return how many fit.
///
/// Stopping at the buffer's end is what the input functions want:
/// they fill as much as fits and report that.
fn fill_array(
    memory: &mut Memory,
    buf: Option<MemArray>,
    values: impl Iterator<Item = u32>,
) -> Outcome<u32> {
    let Some(buf) = buf else {
        return Ok(0);
    };

    let mut written = 0;

    for value in values {
        if written >= buf.count {
            break;
        }

        buf.set(memory, written, value)?;
        written += 1;
    }

    Ok(written)
}

/// Latin-1 printable, plus newline (Glk: Output).
fn is_printable(ch: u32) -> bool {
    ch == NEWLINE || (0x20..0x7F).contains(&ch) || (0xA0..0x100).contains(&ch)
}

/// Event types glk_select_poll may report; never input (Glk: Other
/// Events).
fn pollable(kind: u32) -> bool {
    matches!(
        kind,
        event_type::TIMER
            | event_type::ARRANGE
            | event_type::REDRAW
            | event_type::SOUND_NOTIFY
            | event_type::VOLUME_NOTIFY
    )
}

/// One character's case mapping, where one character can hold it.
///
/// Only single-character mappings are representable; German
/// sharp-s uppercasing to "SS" is the usual offender, and stays
/// itself (Glk: Upper and Lower Case).
fn map_case(ch: u32, lower: bool) -> u32 {
    let Some(character) = char::from_u32(ch) else {
        return ch;
    };

    let mut mapped = if lower {
        character.to_lowercase().collect::<Vec<char>>()
    } else {
        character.to_uppercase().collect::<Vec<char>>()
    };

    if mapped.len() == 1 {
        u32::from(mapped.remove(0))
    } else {
        ch
    }
}

/// One character's titlecase mapping, possibly multi-character.
///
/// The digraphs carry distinct Lt forms; a ligature's uppercase
/// expansion keeps its first character's case and lowers the rest,
/// which is the reference's str.title() on one character.
fn title_char(ch: char) -> String {
    let digraph = match ch {
        '\u{01C4}' | '\u{01C5}' | '\u{01C6}' => Some('\u{01C5}'),
        '\u{01C7}' | '\u{01C8}' | '\u{01C9}' => Some('\u{01C8}'),
        '\u{01CA}' | '\u{01CB}' | '\u{01CC}' => Some('\u{01CB}'),
        '\u{01F1}' | '\u{01F2}' | '\u{01F3}' => Some('\u{01F2}'),
        _ => None,
    };

    if let Some(mapped) = digraph {
        return mapped.to_string();
    }

    let upper: Vec<char> = ch.to_uppercase().collect();
    let mut out = String::new();

    for (index, held) in upper.iter().enumerate() {
        if index == 0 {
            out.push(*held);
        } else {
            out.extend(held.to_lowercase());
        }
    }

    out
}

/// The first so-many characters of a buffer, as text.
fn chars_of(memory: &Memory, buf: Option<MemArray>, numchars: u32) -> Outcome<Vec<char>> {
    let Some(buf) = buf else {
        return Ok(Vec::new());
    };

    let count = numchars.min(buf.count);
    let mut chars = Vec::with_capacity(count as usize);

    for index in 0..count {
        chars.push(crate::glulx::glk::objects::to_char(buf.get(memory, index)?));
    }

    Ok(chars)
}

/// Write text back, truncating at the buffer's capacity.
///
/// The true converted length is returned even when it exceeds the
/// buffer, whose contents past that point are undefined (Glk:
/// Upper and Lower Case).
fn store_chars(memory: &mut Memory, buf: Option<MemArray>, text: &str) -> Outcome<u32> {
    fill_array(memory, buf, text.chars().map(u32::from))?;

    Ok(text.chars().count() as u32)
}

/// Case-map a buffer one character at a time.
///
/// Per character, not on the joined string: whole-string mapping
/// applies context-sensitive rules -- Greek sigma lowercases
/// differently at the end of a word -- while the spec asks for
/// "every character" mapped to its equivalent (Glk: Upper and
/// Lower Case).
fn map_buffer(
    memory: &mut Memory,
    buf: Option<MemArray>,
    numchars: u32,
    transform: impl Fn(char) -> String,
) -> Outcome<u32> {
    let chars = chars_of(memory, buf, numchars)?;
    let text: String = chars.into_iter().map(transform).collect();

    store_chars(memory, buf, &text)
}

/// A signed second count as the (high, low) pair of a
/// glktimeval_t. The two words are one signed 64-bit number (Glk:
/// The System Clock), so an arithmetic shift produces the high
/// word for negative times too -- and -1 falls out as all-ones in
/// both, which is the failure value.
fn split_seconds(seconds: i64) -> (u32, u32) {
    ((seconds >> 32) as u32, seconds as u32)
}

fn fill_date(slot: &mut StructSlot, fields: [i64; 8]) {
    slot.set_all(&fields.map(|value| Held::Word(value as u32)));
}

/// Howard Hinnant's civil-from-days: a day count from the epoch as
/// (year, month, day).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };

    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The inverse: days from the epoch for a civil date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;

    era * 146_097 + doe - 719_468
}

/// A fresh, empty temporary file, created so it exists.
fn fresh_temp_path() -> Outcome<PathBuf> {
    let dir = std::env::temp_dir();

    for _ in 0..64 {
        let mut noise = [0u8; 8];

        if getrandom::fill(&mut noise).is_err() {
            break;
        }

        let name = format!(
            "voxam-glk-{}",
            noise
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let path = dir.join(name);

        if std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .is_ok()
        {
            return Ok(path);
        }
    }

    fault("a temporary file could not be created")
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::blorb::Blorb;
    use crate::glulx::glk::objects::{Flow, WindowKind, window_type};
    use crate::glulx::story::Story;
    use crate::iff::chunk;

    const ABOVE_FIXED: u32 = window_method::ABOVE | window_method::FIXED;
    const BELOW_FIXED: u32 = window_method::BELOW | window_method::FIXED;
    const LEFT_PROPORTIONAL: u32 = window_method::LEFT | window_method::PROPORTIONAL;

    fn ram() -> Memory {
        let mut data = vec![0u8; 256];
        data[..4].copy_from_slice(b"Glul");
        data[4..8].copy_from_slice(&0x0003_0102u32.to_be_bytes());
        data[8..12].copy_from_slice(&256u32.to_be_bytes());
        data[12..16].copy_from_slice(&256u32.to_be_bytes());
        data[16..20].copy_from_slice(&0x300u32.to_be_bytes());
        data[20..24].copy_from_slice(&256u32.to_be_bytes());

        Memory::new(&Story::new(data).unwrap())
    }

    fn bytes_at(address: u32, count: u32) -> MemArray {
        MemArray {
            address,
            count,
            width: 1,
        }
    }

    fn words_at(address: u32, count: u32) -> MemArray {
        MemArray {
            address,
            count,
            width: 4,
        }
    }

    fn word_list(memory: &Memory, buf: MemArray, count: u32) -> Vec<u32> {
        (0..count)
            .map(|index| buf.get(memory, index).unwrap())
            .collect()
    }

    fn write_words(memory: &mut Memory, buf: MemArray, values: &[u32]) {
        for (index, value) in values.iter().enumerate() {
            buf.set(memory, index as u32, *value).unwrap();
        }
    }

    fn glk_message(stop: Stop) -> String {
        match stop {
            Stop::Fault(error) => error.to_string(),
            Stop::End => "session end".into(),
        }
    }

    /// A per-test scratch directory under the system temp dir.
    fn scratch_dir(tag: &str) -> PathBuf {
        let mut noise = [0u8; 6];

        getrandom::fill(&mut noise).unwrap();

        let name = format!(
            "voxam-glk-test-{tag}-{}",
            noise
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let path = std::env::temp_dir().join(name);

        std::fs::create_dir_all(&path).unwrap();

        path
    }

    // -- the recording displays -------------------------------------------

    #[derive(Default)]
    struct Log {
        flushes: u32,
        timers: Vec<u32>,
        calls: Vec<String>,
    }

    type SharedLog = Rc<RefCell<Log>>;

    /// A recording display: 100x50, no input of its own.
    struct Quiet {
        log: SharedLog,
    }

    impl Quiet {
        fn new() -> (Self, SharedLog) {
            let log = SharedLog::default();

            (Self { log: log.clone() }, log)
        }
    }

    impl Frontend for Quiet {
        fn size(&self) -> (i64, i64) {
            (100, 50)
        }

        fn flush(&mut self, _windows: &mut WindowMap, _root: Option<u32>) {
            self.log.borrow_mut().flushes += 1;
        }

        fn read_line(
            &mut self,
            _windows: &mut WindowMap,
            _window: u32,
            _maxlen: u32,
        ) -> Asked<(String, u32)> {
            Asked::Instead(Vec::new())
        }

        fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
            Asked::Instead(Vec::new())
        }

        fn set_timer(&mut self, millisecs: u32) {
            self.log.borrow_mut().timers.push(millisecs);
        }
    }

    /// A display that cannot block: never asked, only delivered
    /// to, with each awaitable capability its own claim.
    #[derive(Default)]
    struct Suspending {
        timer: bool,
        mouse: bool,
        hyper: bool,
        flushes: Rc<RefCell<u32>>,
    }

    impl Frontend for Suspending {
        fn suspends(&self) -> bool {
            true
        }

        fn timer_input(&self) -> bool {
            self.timer
        }

        fn mouse_input(&self) -> bool {
            self.mouse
        }

        fn hyperlink_input(&self) -> bool {
            self.hyper
        }

        fn size(&self) -> (i64, i64) {
            (100, 50)
        }

        fn flush(&mut self, _windows: &mut WindowMap, _root: Option<u32>) {
            *self.flushes.borrow_mut() += 1;
        }

        fn read_line(
            &mut self,
            _windows: &mut WindowMap,
            _window: u32,
            _maxlen: u32,
        ) -> Asked<(String, u32)> {
            unreachable!("a suspending display is never asked for a line");
        }

        fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
            unreachable!("a suspending display is never asked for a key");
        }
    }

    /// Delivers scripted lines; a None entry raises a timer
    /// instead.
    struct Typist {
        lines: Vec<Option<(String, u32)>>,
    }

    impl Typist {
        fn new(lines: &[Option<(&str, u32)>]) -> Self {
            Self {
                lines: lines
                    .iter()
                    .map(|held| held.map(|(text, term)| (text.to_string(), term)))
                    .collect(),
            }
        }
    }

    impl Frontend for Typist {
        fn size(&self) -> (i64, i64) {
            (100, 50)
        }

        fn flush(&mut self, _windows: &mut WindowMap, _root: Option<u32>) {}

        fn read_line(
            &mut self,
            _windows: &mut WindowMap,
            _window: u32,
            _maxlen: u32,
        ) -> Asked<(String, u32)> {
            match self.lines.remove(0) {
                Some(answer) => Asked::Answer(answer),
                None => Asked::Instead(vec![Event::new(event_type::TIMER, None, 0, 0)]),
            }
        }

        fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
            Asked::Instead(Vec::new())
        }
    }

    /// Delivers scripted keystrokes; None raises a timer instead.
    struct Keyist {
        graphics: bool,
        chars: Vec<Option<u32>>,
    }

    impl Frontend for Keyist {
        fn graphics(&self) -> bool {
            self.graphics
        }

        fn size(&self) -> (i64, i64) {
            (100, 50)
        }

        fn flush(&mut self, _windows: &mut WindowMap, _root: Option<u32>) {}

        fn read_line(
            &mut self,
            _windows: &mut WindowMap,
            _window: u32,
            _maxlen: u32,
        ) -> Asked<(String, u32)> {
            Asked::Instead(Vec::new())
        }

        fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
            match self.chars.remove(0) {
                Some(answer) => Asked::Answer(answer),
                None => Asked::Instead(vec![Event::new(event_type::TIMER, None, 0, 0)]),
            }
        }
    }

    /// Delivers scripted clicks; a None entry means "not yet".
    struct Clicker {
        mice: Vec<Option<(u32, u32)>>,
    }

    impl Frontend for Clicker {
        fn mouse_input(&self) -> bool {
            true
        }

        fn size(&self) -> (i64, i64) {
            (100, 50)
        }

        fn flush(&mut self, _windows: &mut WindowMap, _root: Option<u32>) {}

        fn read_line(
            &mut self,
            _windows: &mut WindowMap,
            _window: u32,
            _maxlen: u32,
        ) -> Asked<(String, u32)> {
            Asked::Instead(Vec::new())
        }

        fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
            Asked::Instead(Vec::new())
        }

        fn read_mouse(&mut self, _window: u32) -> Option<(u32, u32)> {
            self.mice.remove(0)
        }
    }

    /// Delivers scripted link selections; zero means "not yet".
    struct Linker {
        links: Vec<u32>,
    }

    impl Frontend for Linker {
        fn hyperlink_input(&self) -> bool {
            true
        }

        fn size(&self) -> (i64, i64) {
            (100, 50)
        }

        fn flush(&mut self, _windows: &mut WindowMap, _root: Option<u32>) {}

        fn read_line(
            &mut self,
            _windows: &mut WindowMap,
            _window: u32,
            _maxlen: u32,
        ) -> Asked<(String, u32)> {
            Asked::Instead(Vec::new())
        }

        fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
            Asked::Instead(Vec::new())
        }

        fn read_hyperlink(&mut self, _window: u32) -> Option<u32> {
            Some(self.links.remove(0))
        }
    }

    /// A display that plays sound, scripted to accept or refuse.
    struct Sounder {
        accepts: bool,
        log: SharedLog,
    }

    impl Sounder {
        fn new(accepts: bool) -> (Self, SharedLog) {
            let log = SharedLog::default();

            (
                Self {
                    accepts,
                    log: log.clone(),
                },
                log,
            )
        }
    }

    impl Frontend for Sounder {
        fn sound(&self) -> bool {
            true
        }

        fn size(&self) -> (i64, i64) {
            (100, 50)
        }

        fn flush(&mut self, _windows: &mut WindowMap, _root: Option<u32>) {}

        fn read_line(
            &mut self,
            _windows: &mut WindowMap,
            _window: u32,
            _maxlen: u32,
        ) -> Asked<(String, u32)> {
            Asked::Instead(Vec::new())
        }

        fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
            Asked::Instead(Vec::new())
        }

        fn play_sound(
            &mut self,
            _channel: &SoundChannel,
            sound: u32,
            repeats: u32,
            notify: u32,
        ) -> bool {
            self.log
                .borrow_mut()
                .calls
                .push(format!("play {sound} {repeats} {notify}"));

            self.accepts
        }

        fn stop_sound(&mut self, _channel: &SoundChannel) {
            self.log.borrow_mut().calls.push("stop".into());
        }

        fn pause_sound(&mut self, _channel: &SoundChannel, paused: bool) {
            self.log.borrow_mut().calls.push(format!("pause {paused}"));
        }

        fn set_volume(&mut self, _channel: &SoundChannel, volume: u32, duration: u32) {
            self.log
                .borrow_mut()
                .calls
                .push(format!("volume {volume} {duration}"));
        }
    }

    /// A display that lays text around pictures, canvases or not.
    struct Weaver;

    impl Frontend for Weaver {
        fn buffer_images(&self) -> bool {
            true
        }

        fn size(&self) -> (i64, i64) {
            (100, 50)
        }

        fn flush(&mut self, _windows: &mut WindowMap, _root: Option<u32>) {}

        fn read_line(
            &mut self,
            _windows: &mut WindowMap,
            _window: u32,
            _maxlen: u32,
        ) -> Asked<(String, u32)> {
            Asked::Instead(Vec::new())
        }

        fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
            Asked::Instead(Vec::new())
        }
    }

    /// A display that draws, recording every graphics call.
    struct Artist {
        log: SharedLog,
    }

    impl Artist {
        fn new() -> (Self, SharedLog) {
            let log = SharedLog::default();

            (Self { log: log.clone() }, log)
        }
    }

    impl Frontend for Artist {
        fn graphics(&self) -> bool {
            true
        }

        fn size(&self) -> (i64, i64) {
            (100, 50)
        }

        fn flush(&mut self, _windows: &mut WindowMap, _root: Option<u32>) {}

        fn read_line(
            &mut self,
            _windows: &mut WindowMap,
            _window: u32,
            _maxlen: u32,
        ) -> Asked<(String, u32)> {
            Asked::Instead(Vec::new())
        }

        fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
            Asked::Instead(Vec::new())
        }

        fn draw_image(
            &mut self,
            _windows: &mut WindowMap,
            _window: u32,
            _image: &crate::glulx::glk::resources::ImageInfo,
            val1: i64,
            val2: i64,
            width: u32,
            height: u32,
        ) -> bool {
            self.log
                .borrow_mut()
                .calls
                .push(format!("draw {val1} {val2} {width} {height}"));

            true
        }

        fn erase_rect(&mut self, _window: u32, left: i64, top: i64, width: u32, height: u32) {
            self.log
                .borrow_mut()
                .calls
                .push(format!("erase {left} {top} {width} {height}"));
        }

        fn fill_rect(
            &mut self,
            _window: u32,
            color: u32,
            _left: i64,
            _top: i64,
            _width: u32,
            _height: u32,
        ) {
            self.log.borrow_mut().calls.push(format!("fill {color:#x}"));
        }

        fn set_background_color(&mut self, _window: u32, color: u32) {
            self.log
                .borrow_mut()
                .calls
                .push(format!("background {color:#x}"));
        }

        fn flow_break(&mut self, _window: u32) {
            self.log.borrow_mut().calls.push("flow".into());
        }
    }

    /// A display that can tell styles apart and measure them.
    struct Styler;

    impl Frontend for Styler {
        fn size(&self) -> (i64, i64) {
            (100, 50)
        }

        fn flush(&mut self, _windows: &mut WindowMap, _root: Option<u32>) {}

        fn read_line(
            &mut self,
            _windows: &mut WindowMap,
            _window: u32,
            _maxlen: u32,
        ) -> Asked<(String, u32)> {
            Asked::Instead(Vec::new())
        }

        fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
            Asked::Instead(Vec::new())
        }

        fn style_distinguish(&self, _window: &Window, _first: u32, _second: u32) -> bool {
            true
        }

        fn style_measure(&self, _window: &Window, _style: u32, _hint: u32) -> Option<u32> {
            Some(7)
        }
    }

    /// A display whose file prompt answers a scripted name.
    struct Prompter {
        name: Option<String>,
    }

    impl Frontend for Prompter {
        fn size(&self) -> (i64, i64) {
            (100, 50)
        }

        fn flush(&mut self, _windows: &mut WindowMap, _root: Option<u32>) {}

        fn read_line(
            &mut self,
            _windows: &mut WindowMap,
            _window: u32,
            _maxlen: u32,
        ) -> Asked<(String, u32)> {
            Asked::Instead(Vec::new())
        }

        fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
            Asked::Instead(Vec::new())
        }

        fn prompt_file(&mut self, _usage: u32, _fmode: u32) -> Option<String> {
            self.name.clone()
        }
    }

    /// A library with one text-buffer window open as the root.
    fn rooted(display: Box<dyn Frontend>) -> (Glk, u32) {
        let mut library = Glk::new(display);
        let window = library
            .glk_window_open(None, 0, 0, window_type::TEXT_BUFFER, 1)
            .unwrap()
            .expect("the root window opens");

        (library, window)
    }

    fn quiet_rooted() -> (Glk, u32) {
        rooted(Box::new(Quiet::new().0))
    }

    fn event_fields(slot: &StructSlot) -> (u32, Option<u32>, u32, u32) {
        let window = match slot.0[1] {
            Held::Obj(_, key) => key,
            Held::Word(_) => None,
        };

        (slot.0[0].word(), window, slot.0[2].word(), slot.0[3].word())
    }

    // -- Blorb scaffolding --------------------------------------------------

    const RIDX_ENTRY: usize = 12;
    const FORM_PRELUDE: usize = 12;

    type BlorbEntry<'a> = ([u8; 4], u32, [u8; 4], &'a [u8]);

    fn built_blorb(entries: &[BlorbEntry]) -> Blorb {
        let mut index = (entries.len() as u32).to_be_bytes().to_vec();
        let ridx = chunk(
            b"RIdx",
            &vec![0u8; 4 + RIDX_ENTRY * entries.len()][..4 + RIDX_ENTRY * entries.len()],
        );
        let mut offset = FORM_PRELUDE + ridx.len();
        let mut body = Vec::new();

        for (usage, number, chunk_id, payload) in entries {
            index.extend_from_slice(usage);
            index.extend_from_slice(&number.to_be_bytes());
            index.extend_from_slice(&(offset as u32).to_be_bytes());

            let framed = chunk(chunk_id, payload);

            offset += framed.len();
            body.extend_from_slice(&framed);
        }

        let mut form_body = b"IFRS".to_vec();
        form_body.extend_from_slice(&chunk(b"RIdx", &index));
        form_body.extend_from_slice(&body);

        Blorb::parse(&chunk(b"FORM", &form_body)).unwrap()
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut data = b"\x89PNG\r\n\x1a\n".to_vec();

        data.extend_from_slice(&13u32.to_be_bytes());
        data.extend_from_slice(b"IHDR");
        data.extend_from_slice(&width.to_be_bytes());
        data.extend_from_slice(&height.to_be_bytes());

        data
    }

    // -- the tests ----------------------------------------------------------

    // glk_exit shows what is pending and ends the session;
    // glk_tick does nothing at all.
    #[test]
    fn exit_flushes_and_ends() {
        let (display, log) = Quiet::new();
        let mut library = Glk::new(Box::new(display));

        library.glk_tick();

        assert!(matches!(library.glk_exit(), Err(Stop::End)));
        assert_eq!(log.borrow().flushes, 1);
    }

    // The gestalt selectors answer for this library over this
    // display: version, character rules, and one answer per
    // capability flag -- with unknown selectors at zero for
    // programs from the future.
    #[test]
    fn gestalt_answers_for_the_display() {
        let mut library = Glk::new(Box::new(Quiet::new().0));

        let answers = [
            (glk_gestalt::VERSION, 0, GLK_VERSION),
            (glk_gestalt::CHAR_INPUT, 0x41, 1),
            (glk_gestalt::CHAR_INPUT, key_code::RETURN, 1),
            (glk_gestalt::CHAR_INPUT, key_code::UNKNOWN, 0),
            (glk_gestalt::CHAR_INPUT, 0x07, 0),
            (glk_gestalt::LINE_INPUT, 0x41, 1),
            (glk_gestalt::LINE_INPUT, 0x0A, 0),
            (glk_gestalt::CHAR_OUTPUT, 0x41, CHAR_OUTPUT_EXACT_PRINT),
            (glk_gestalt::CHAR_OUTPUT, 0x07, CHAR_OUTPUT_CANNOT_PRINT),
            (glk_gestalt::GRAPHICS, 0, 0),
            (glk_gestalt::DRAW_IMAGE, window_type::GRAPHICS, 0),
            (glk_gestalt::SOUND, 0, 0),
            (glk_gestalt::SOUND2, 0, 0),
            (glk_gestalt::MOUSE_INPUT, window_type::TEXT_GRID, 0),
            (glk_gestalt::TIMER, 0, 0),
            (glk_gestalt::HYPERLINKS, 0, 1),
            (glk_gestalt::HYPERLINK_INPUT, 0, 0),
            (glk_gestalt::UNICODE, 0, 1),
            (glk_gestalt::UNICODE_NORM, 0, 1),
            (glk_gestalt::LINE_INPUT_ECHO, 0, 1),
            (glk_gestalt::LINE_TERMINATORS, 0, 1),
            (glk_gestalt::DATE_TIME, 0, 1),
            (glk_gestalt::RESOURCE_STREAM, 0, 1),
            (glk_gestalt::GRAPHICS_TRANSPARENCY, 0, 0),
            (glk_gestalt::GRAPHICS_CHAR_INPUT, 0, 0),
            (99, 0, 0),
        ];

        for (selector, value, expected) in answers {
            assert_eq!(
                library.glk_gestalt(selector, value),
                expected,
                "selector {selector}"
            );
        }

        // A drawing display draws images only in graphics windows
        // -- the spec's own "both, neither, or only one" -- and
        // claims transparency.
        let mut drawing = Glk::new(Box::new(Artist::new().0));

        assert_eq!(
            drawing.glk_gestalt(glk_gestalt::DRAW_IMAGE, window_type::GRAPHICS),
            1
        );
        assert_eq!(
            drawing.glk_gestalt(glk_gestalt::DRAW_IMAGE_SCALE, window_type::GRAPHICS),
            1
        );
        assert_eq!(
            drawing.glk_gestalt(glk_gestalt::DRAW_IMAGE, window_type::TEXT_BUFFER),
            0
        );
        assert_eq!(drawing.glk_gestalt(glk_gestalt::GRAPHICS, 0), 1);
        assert_eq!(
            drawing.glk_gestalt(glk_gestalt::GRAPHICS_TRANSPARENCY, 0),
            1
        );
        assert_eq!(drawing.glk_gestalt(glk_gestalt::GRAPHICS_CHAR_INPUT, 0), 1);

        // A display that lays text around pictures claims the text
        // buffer type instead.
        let mut weaving = Glk::new(Box::new(Weaver));

        assert_eq!(
            weaving.glk_gestalt(glk_gestalt::DRAW_IMAGE, window_type::TEXT_BUFFER),
            1
        );
        assert_eq!(
            weaving.glk_gestalt(glk_gestalt::DRAW_IMAGE, window_type::GRAPHICS),
            0
        );

        // A clicking display still only carries a mouse in grids
        // and graphics windows.
        let mut clicking = Glk::new(Box::new(Clicker { mice: Vec::new() }));

        assert_eq!(
            clicking.glk_gestalt(glk_gestalt::MOUSE_INPUT, window_type::TEXT_GRID),
            1
        );
        assert_eq!(
            clicking.glk_gestalt(glk_gestalt::MOUSE_INPUT, window_type::TEXT_BUFFER),
            0
        );

        // The extended form reports printability into its array,
        // when one with room arrives.
        let mut memory = ram();
        let room = words_at(0x180, 1);

        library
            .glk_gestalt_ext(&mut memory, glk_gestalt::CHAR_OUTPUT, 0x41, Some(room))
            .unwrap();

        assert_eq!(room.get(&memory, 0).unwrap(), 1);

        library
            .glk_gestalt_ext(&mut memory, glk_gestalt::CHAR_OUTPUT, 0x07, Some(room))
            .unwrap();

        assert_eq!(room.get(&memory, 0).unwrap(), 0);
        assert_eq!(
            library
                .glk_gestalt_ext(
                    &mut memory,
                    glk_gestalt::CHAR_OUTPUT,
                    0x41,
                    Some(words_at(0x190, 0))
                )
                .unwrap(),
            CHAR_OUTPUT_EXACT_PRINT
        );
    }

    // The first window opens with no split; every later one names
    // the window it splits. The tree wires pairs in above
    // whichever child was split, on either side.
    #[test]
    fn windows_split_into_a_tree() {
        let (mut library, first) = quiet_rooted();

        let second = library
            .glk_window_open(Some(first), ABOVE_FIXED, 3, window_type::TEXT_GRID, 2)
            .unwrap()
            .unwrap();
        let third = library
            .glk_window_open(Some(second), LEFT_PROPORTIONAL, 40, window_type::BLANK, 3)
            .unwrap()
            .unwrap();
        let fourth = library
            .glk_window_open(Some(first), BELOW_FIXED, 2, window_type::TEXT_GRID, 4)
            .unwrap()
            .unwrap();

        let root = library.glk_window_get_root();

        assert_eq!(library.glk_window_get_type(root), window_type::PAIR);
        assert_eq!(library.glk_window_get_parent(root), None);
        assert_eq!(library.glk_window_get_parent(None), None);

        // first and fourth share a pair; so do second and third.
        assert_eq!(library.glk_window_get_sibling(Some(first)), Some(fourth));
        assert_eq!(library.glk_window_get_sibling(Some(fourth)), Some(first));
        assert_eq!(library.glk_window_get_sibling(Some(second)), Some(third));
        assert_eq!(library.glk_window_get_sibling(root), None);
        assert_eq!(library.glk_window_get_sibling(None), None);

        // The grid got its fixed three rows of the 100x50 display.
        let mut width = RefSlot::default();
        let mut height = RefSlot::default();

        library.glk_window_get_size(Some(second), Some(&mut width), Some(&mut height));

        assert_eq!((width.0.word(), height.0.word()), (60, 3));

        library.glk_window_get_size(None, Some(&mut width), Some(&mut height));

        assert_eq!((width.0.word(), height.0.word()), (0, 0));

        library.glk_window_get_size(Some(second), None, None);

        // The walk visits every live window, newest first, and
        // answers rocks along the way.
        let mut rock = RefSlot::default();
        let mut seen = Vec::new();
        let mut current = library.glk_window_iterate(None, Some(&mut rock));

        while let Some(held) = current {
            seen.push(rock.0.word());

            current = library.glk_window_iterate(Some(held), Some(&mut rock));
        }

        assert_eq!(rock.0.word(), 0);
        assert_eq!(seen.len(), 7);

        // A window not on the list ends the walk; so does an empty
        // library.
        assert_eq!(library.glk_window_iterate(Some(9999), None), None);
        assert_eq!(
            Glk::new(Box::new(Quiet::new().0)).glk_window_iterate(None, None),
            None
        );

        assert_eq!(library.glk_window_get_rock(Some(first)), 1);
        assert_eq!(library.glk_window_get_rock(None), 0);
    }

    // Closing a window promotes its sibling into the pair's place
    // -- through the grandparent on either side, and to the root
    // when the pair was the root.
    #[test]
    fn closing_promotes_the_sibling() {
        let (mut library, first) = quiet_rooted();

        let second = library
            .glk_window_open(Some(first), ABOVE_FIXED, 3, window_type::TEXT_GRID, 2)
            .unwrap()
            .unwrap();
        let third = library
            .glk_window_open(Some(second), LEFT_PROPORTIONAL, 40, window_type::BLANK, 3)
            .unwrap()
            .unwrap();
        let fourth = library
            .glk_window_open(Some(first), BELOW_FIXED, 2, window_type::TEXT_GRID, 4)
            .unwrap()
            .unwrap();

        let mut counts = StructSlot::new(2);

        library
            .glk_window_close(Some(third), Some(&mut counts))
            .unwrap();

        assert_eq!(counts.0, [Held::Word(0), Held::Word(0)]);

        let sibling = library.glk_window_get_sibling(Some(second)).unwrap();

        assert_eq!(
            library.glk_window_get_type(Some(sibling)),
            window_type::PAIR
        );

        library.glk_window_close(Some(fourth), None).unwrap();

        assert_eq!(library.glk_window_get_sibling(Some(first)), Some(second));

        library.glk_window_close(Some(second), None).unwrap();

        assert_eq!(library.glk_window_get_root(), Some(first));

        library.glk_window_close(Some(first), None).unwrap();

        assert_eq!(library.glk_window_get_root(), None);
        assert!(library.window_order.is_empty());
        assert!(library.stream_order.is_empty());

        // A second close finds a window that no longer resolves --
        // the same refusal a stale id earns through the bridge.
        let error = library.glk_window_close(Some(first), None).unwrap_err();

        assert!(glk_message(error).contains("window_close"));
    }

    // Closing a pair closes its whole subtree, and takes the
    // current stream with it when a closed window held it.
    #[test]
    fn closing_a_pair_closes_the_subtree() {
        let (mut library, first) = quiet_rooted();

        library
            .glk_window_open(Some(first), ABOVE_FIXED, 3, window_type::TEXT_GRID, 2)
            .unwrap();

        library.glk_set_window(Some(first));

        let pair = library.glk_window_get_parent(Some(first));

        library.glk_window_close(pair, None).unwrap();

        assert_eq!(library.glk_window_get_root(), None);
        assert!(library.window_order.is_empty());
        assert_eq!(library.glk_stream_get_current(), None);

        let error = library.glk_window_close(None, None).unwrap_err();

        assert!(glk_message(error).contains("window_close"));
    }

    // A split must be coherent: the first window takes no split,
    // later ones need one, and the method must name a division and
    // a direction. Pair windows cannot be opened directly at all.
    #[test]
    fn incoherent_splits_are_refused() {
        let mut library = Glk::new(Box::new(Quiet::new().0));

        let error = library
            .glk_window_open(Some(999), ABOVE_FIXED, 1, window_type::TEXT_BUFFER, 0)
            .unwrap_err();

        assert!(glk_message(error).contains("must be null"));

        let (mut library, first) = quiet_rooted();

        let error = library
            .glk_window_open(None, 0, 0, window_type::TEXT_BUFFER, 0)
            .unwrap_err();

        assert!(glk_message(error).contains("must not be null"));

        let error = library
            .glk_window_open(Some(first), window_method::ABOVE, 1, window_type::BLANK, 0)
            .unwrap_err();

        assert!(glk_message(error).contains("neither fixed nor proportional"));

        let error = library
            .glk_window_open(
                Some(first),
                window_method::FIXED | 0x04,
                1,
                window_type::BLANK,
                0,
            )
            .unwrap_err();

        assert!(glk_message(error).contains("names no direction"));

        let error = library
            .glk_window_open(Some(first), ABOVE_FIXED, 1, window_type::PAIR, 0)
            .unwrap_err();

        assert!(glk_message(error).contains("pair window"));

        // An unsupported type answers None: graphics without a
        // drawing display, and types from a Glk yet to be written.
        assert_eq!(
            library
                .glk_window_open(Some(first), ABOVE_FIXED, 1, window_type::GRAPHICS, 0)
                .unwrap(),
            None
        );
        assert_eq!(
            library
                .glk_window_open(Some(first), ABOVE_FIXED, 1, 99, 0)
                .unwrap(),
            None
        );

        // A drawing display opens one happily.
        let (mut drawing, base) = rooted(Box::new(Artist::new().0));

        assert!(
            drawing
                .glk_window_open(Some(base), ABOVE_FIXED, 8, window_type::GRAPHICS, 0)
                .unwrap()
                .is_some()
        );
    }

    // A canvas takes character input wherever a canvas can exist
    // at all: the request arms like any other window's, the
    // keyboard answers it, and the keystroke comes back on the
    // canvas (Glk: Character Input Events).
    #[test]
    fn a_canvas_takes_character_input() {
        let (mut library, first) = rooted(Box::new(Keyist {
            graphics: true,
            chars: vec![Some(u32::from(b'm'))],
        }));
        let canvas = library
            .glk_window_open(Some(first), ABOVE_FIXED, 8, window_type::GRAPHICS, 0)
            .unwrap()
            .expect("the canvas opens");

        library.glk_request_char_event(Some(canvas)).unwrap();

        let mut memory = ram();
        let mut event = StructSlot::new(4);

        library.glk_select(&mut memory, &mut event).unwrap();

        assert_eq!(
            event_fields(&event),
            (event_type::CHAR_INPUT, Some(canvas), u32::from(b'm'), 0)
        );
        assert!(!library.windows[&canvas].char_request);
    }

    // A rearrange that moves a real canvas clears it and owes the
    // game a redraw event. Opening one owes nothing: a fresh
    // canvas is background and the game knows it.
    #[test]
    fn a_moved_canvas_earns_a_redraw_event() {
        let (mut library, first) = rooted(Box::new(Artist::new().0));
        let canvas = library
            .glk_window_open(Some(first), ABOVE_FIXED, 8, window_type::GRAPHICS, 0)
            .unwrap()
            .expect("the canvas opens");

        assert!(library.pending_events.is_empty());

        let pair = library.glk_window_get_parent(Some(canvas));

        library
            .glk_window_set_arrangement(pair, ABOVE_FIXED, 12, Some(canvas))
            .unwrap();

        let redraws: Vec<&Event> = library
            .pending_events
            .iter()
            .filter(|event| event.kind == event_type::REDRAW)
            .collect();

        assert_eq!(redraws.len(), 1);
        assert_eq!(redraws[0].window, Some(canvas));
    }

    // Changing a pair's direction moves the size constraint to the
    // other child while the glass stays where it is -- the spec's
    // own worked example (Glk: Changing Window Constraints).
    #[test]
    fn arrangements_change_and_report() {
        let (mut library, first) = quiet_rooted();

        let second = library
            .glk_window_open(Some(first), ABOVE_FIXED, 3, window_type::TEXT_GRID, 2)
            .unwrap()
            .unwrap();
        let pair = library.glk_window_get_parent(Some(first));

        library
            .glk_window_set_arrangement(pair, BELOW_FIXED, 5, Some(first))
            .unwrap();

        let mut method = RefSlot::default();
        let mut size = RefSlot::default();
        let mut key = RefSlot::default();

        library
            .glk_window_get_arrangement(pair, Some(&mut method), Some(&mut size), Some(&mut key))
            .unwrap();

        assert_eq!(method.0.word(), BELOW_FIXED);
        assert_eq!(size.0.word(), 5);
        assert_eq!(key.0, Held::Obj(CLASS_WINDOW, Some(first)));

        // The grid is still on top; the buffer below now carries
        // the fixed five rows.
        let mut width = RefSlot::default();
        let mut height = RefSlot::default();

        library.glk_window_get_size(Some(first), Some(&mut width), Some(&mut height));

        assert_eq!((width.0.word(), height.0.word()), (100, 5));

        library.glk_window_get_size(Some(second), Some(&mut width), Some(&mut height));

        assert_eq!((width.0.word(), height.0.word()), (100, 45));

        library
            .glk_window_set_arrangement(pair, ABOVE_FIXED, 2, None)
            .unwrap();
        library
            .glk_window_set_arrangement(pair, ABOVE_FIXED, 3, None)
            .unwrap();
        library
            .glk_window_get_arrangement(pair, None, None, None)
            .unwrap();

        let error = library
            .glk_window_set_arrangement(Some(second), ABOVE_FIXED, 1, None)
            .unwrap_err();

        assert!(glk_message(error).contains("not a pair"));

        let error = library
            .glk_window_get_arrangement(None, None, None, None)
            .unwrap_err();

        assert!(glk_message(error).contains("not a pair"));

        let error = library
            .glk_window_set_arrangement(pair, window_method::LEFT | window_method::FIXED, 1, None)
            .unwrap_err();

        assert!(glk_message(error).contains("cannot change its axis"));

        let error = library
            .glk_window_set_arrangement(pair, ABOVE_FIXED, 1, pair)
            .unwrap_err();

        assert!(glk_message(error).contains("cannot be a pair"));

        // A key from outside the pair's own subtree is refused.
        let inner = library
            .glk_window_open(Some(second), ABOVE_FIXED, 1, window_type::BLANK, 0)
            .unwrap()
            .unwrap();
        let inner_pair = library.glk_window_get_parent(Some(inner));

        let error = library
            .glk_window_set_arrangement(inner_pair, ABOVE_FIXED, 1, Some(first))
            .unwrap_err();

        assert!(glk_message(error).contains("under the pair"));
    }

    // The window functions that just fetch or clear behave under
    // both a window and the null window.
    #[test]
    fn window_oddments_tolerate_null() {
        let (mut library, first) = quiet_rooted();

        let grid = library
            .glk_window_open(Some(first), ABOVE_FIXED, 3, window_type::TEXT_GRID, 2)
            .unwrap()
            .unwrap();

        library.glk_window_clear(Some(first));
        library.glk_window_clear(None);

        assert!(library.windows[&first].pending_clear);

        library.glk_window_move_cursor(Some(grid), 1, 0).unwrap();

        let error = library
            .glk_window_move_cursor(Some(first), 0, 0)
            .unwrap_err();

        assert!(glk_message(error).contains("not a text grid"));

        assert_eq!(
            library.glk_window_get_stream(Some(first)),
            Some(library.windows[&first].stream)
        );
        assert_eq!(library.glk_window_get_stream(None), None);

        let echo = library
            .glk_stream_open_memory(Some(bytes_at(0x180, 8)), file_mode::WRITE, 0)
            .unwrap();

        library.glk_window_set_echo_stream(Some(first), Some(echo));
        library.glk_window_set_echo_stream(None, Some(echo));

        assert_eq!(library.glk_window_get_echo_stream(Some(first)), Some(echo));
        assert_eq!(library.glk_window_get_echo_stream(None), None);

        library.glk_set_window(None);

        assert_eq!(library.glk_stream_get_current(), None);
    }

    // Memory streams open in the three modes that fit them, join
    // the stream walk, and close with their counts reported.
    #[test]
    fn memory_streams_open_and_close() {
        let mut library = Glk::new(Box::new(Quiet::new().0));
        let mut memory = ram();

        let stream = library
            .glk_stream_open_memory(Some(bytes_at(0x180, 4)), file_mode::WRITE, 5)
            .unwrap();
        let wide = library
            .glk_stream_open_memory_uni(Some(words_at(0x190, 2)), file_mode::READ_WRITE, 6)
            .unwrap();

        assert_eq!(library.glk_stream_get_rock(Some(stream)), 5);
        assert_eq!(library.glk_stream_get_rock(None), 0);

        let mut rock = RefSlot::default();

        assert_eq!(
            library.glk_stream_iterate(None, Some(&mut rock)),
            Some(wide)
        );
        assert_eq!(rock.0.word(), 6);
        assert_eq!(library.glk_stream_iterate(Some(wide), None), Some(stream));
        assert_eq!(
            library.glk_stream_iterate(Some(stream), Some(&mut rock)),
            None
        );

        let error = library
            .glk_stream_open_memory(Some(bytes_at(0x180, 4)), file_mode::WRITE_APPEND, 0)
            .unwrap_err();

        assert!(glk_message(error).contains("illegal filemode"));

        library.glk_stream_set_current(Some(stream));
        library.glk_put_string(&mut memory, "hey").unwrap();

        let mut counts = StructSlot::new(2);

        library
            .glk_stream_close(Some(stream), Some(&mut counts))
            .unwrap();

        assert_eq!(counts.0, [Held::Word(0), Held::Word(3)]);
        assert_eq!(library.glk_stream_get_current(), None);
        assert_eq!(library.stream_order.len(), 1);

        // Closing again finds it already off the lists.
        let error = library.glk_stream_close(Some(stream), None).unwrap_err();

        assert!(glk_message(error).contains("invalid stream"));

        let error = library.glk_stream_close(None, None).unwrap_err();

        assert!(glk_message(error).contains("invalid stream"));
    }

    // The printing family reaches the current stream, masks bytes
    // where the narrow functions promise bytes, and shrugs off the
    // null stream and the null buffer.
    #[test]
    fn printing_reaches_the_current_stream() {
        let mut library = Glk::new(Box::new(Quiet::new().0));
        let mut memory = ram();

        // With no current stream, printing goes nowhere quietly.
        library.glk_put_char(&mut memory, 0x41).unwrap();
        library
            .glk_put_char_stream_uni(&mut memory, None, 0x41)
            .unwrap();
        library.glk_put_string(&mut memory, "lost").unwrap();
        library
            .glk_put_buffer(&mut memory, Some(bytes_at(0x180, 1)))
            .unwrap();

        let held = bytes_at(0x1A0, 12);
        let stream = library
            .glk_stream_open_memory(Some(held), file_mode::WRITE, 0)
            .unwrap();

        library.glk_stream_set_current(Some(stream));

        memory.write_byte(0x180, 0x64).unwrap();
        write_words(&mut memory, words_at(0x190, 1), &[0x2604]);
        memory.write_byte(0x184, 0x66).unwrap();
        write_words(&mut memory, words_at(0x198, 1), &[0x67]);

        library.glk_put_char(&mut memory, 0x141).unwrap();
        library.glk_put_char_uni(&mut memory, 0x2603).unwrap();
        library.glk_put_string(&mut memory, "ab").unwrap();
        library.glk_put_string_uni(&mut memory, "c").unwrap();
        library
            .glk_put_buffer(&mut memory, Some(bytes_at(0x180, 1)))
            .unwrap();
        library
            .glk_put_buffer_uni(&mut memory, Some(words_at(0x190, 1)))
            .unwrap();
        library
            .glk_put_char_stream(&mut memory, Some(stream), 0x145)
            .unwrap();
        library
            .glk_put_char_stream_uni(&mut memory, Some(stream), 0x2605)
            .unwrap();
        library
            .glk_put_string_stream(&mut memory, Some(stream), "d")
            .unwrap();
        library
            .glk_put_string_stream_uni(&mut memory, Some(stream), "e")
            .unwrap();
        library
            .glk_put_buffer_stream(&mut memory, Some(stream), Some(bytes_at(0x184, 1)))
            .unwrap();
        library
            .glk_put_buffer_stream_uni(&mut memory, Some(stream), Some(words_at(0x198, 1)))
            .unwrap();
        library
            .glk_put_buffer_stream(&mut memory, Some(stream), None)
            .unwrap();

        assert_eq!(
            word_list(&memory, held, 12),
            [
                0x41, 0x3F, 0x61, 0x62, 0x63, 0x64, 0x3F, 0x45, 0x3F, 0x64, 0x65, 0x66
            ]
        );

        library.glk_set_hyperlink(3);
        library.glk_set_hyperlink_stream(None, 4);

        assert_eq!(library.streams[&stream].hyperlink, 3);
    }

    // Styles land on window streams and fall silently off the
    // others.
    #[test]
    fn styles_only_land_on_windows() {
        let (mut library, first) = quiet_rooted();

        library.glk_set_window(Some(first));
        library.glk_set_style(style::HEADER);

        assert_eq!(library.windows[&first].style, style::HEADER);

        let memory_stream = library
            .glk_stream_open_memory(Some(bytes_at(0x180, 1)), file_mode::WRITE, 0)
            .unwrap();

        library.glk_set_style_stream(Some(memory_stream), style::ALERT);

        assert_eq!(library.windows[&first].style, style::HEADER);
    }

    // The reading family delegates to the stream and answers
    // "empty" for the null stream or the null buffer.
    #[test]
    fn reading_delegates_to_the_stream() {
        let mut library = Glk::new(Box::new(Quiet::new().0));
        let mut memory = ram();

        memory.write_run(0x180, &[0x61, 0x62, 0x0A, 0x63]).unwrap();

        let stream = library
            .glk_stream_open_memory(Some(bytes_at(0x180, 4)), file_mode::READ, 0)
            .unwrap();

        assert_eq!(
            library.glk_get_char_stream(&memory, Some(stream)).unwrap(),
            0x61
        );
        assert_eq!(
            library
                .glk_get_char_stream_uni(&memory, Some(stream))
                .unwrap(),
            0x62
        );
        assert_eq!(library.glk_get_char_stream(&memory, None).unwrap(), -1);

        let line = bytes_at(0x1A0, 4);

        assert_eq!(
            library
                .glk_get_line_stream(&mut memory, Some(stream), Some(line))
                .unwrap(),
            1
        );
        assert_eq!(word_list(&memory, line, 2), [0x0A, 0]);
        assert_eq!(
            library
                .glk_get_line_stream(&mut memory, None, Some(line))
                .unwrap(),
            0
        );
        assert_eq!(
            library
                .glk_get_line_stream_uni(&mut memory, Some(stream), None)
                .unwrap(),
            0
        );

        let room = bytes_at(0x1B0, 2);

        assert_eq!(
            library
                .glk_get_buffer_stream(&mut memory, Some(stream), Some(room))
                .unwrap(),
            1
        );
        assert_eq!(room.get(&memory, 0).unwrap(), 0x63);
        assert_eq!(
            library
                .glk_get_buffer_stream(&mut memory, None, Some(room))
                .unwrap(),
            0
        );
        assert_eq!(
            library
                .glk_get_buffer_stream_uni(&mut memory, Some(stream), None)
                .unwrap(),
            0
        );

        library
            .glk_stream_set_position(Some(stream), 0, seek_mode_start())
            .unwrap();
        library
            .glk_stream_set_position(None, 0, seek_mode_start())
            .unwrap();

        assert_eq!(library.glk_stream_get_position(Some(stream)).unwrap(), 0);
        assert_eq!(library.glk_stream_get_position(None).unwrap(), 0);
    }

    fn seek_mode_start() -> u32 {
        crate::glulx::glk::objects::seek_mode::START
    }

    // File references sanitize game-supplied names into the save
    // directory, wear a suffix by usage, and never escape.
    #[test]
    fn filerefs_sanitize_names() {
        let dir = scratch_dir("names");
        let mut library = Glk::new(Box::new(Quiet::new().0));

        library.save_dir = dir.clone();

        let saved = library.glk_fileref_create_by_name(file_usage::SAVED_GAME, "sa<ve>:1.dat\"", 1);
        let notes = library.glk_fileref_create_by_name(file_usage::TRANSCRIPT, "notes", 2);
        let data = library.glk_fileref_create_by_name(file_usage::DATA, "//", 3);

        assert_eq!(
            library.filerefs[&saved].filename,
            dir.join("save1.glksave").to_string_lossy()
        );
        assert_eq!(
            library.filerefs[&notes].filename,
            dir.join("notes.txt").to_string_lossy()
        );
        assert_eq!(
            library.filerefs[&data].filename,
            dir.join("null.glkdata").to_string_lossy()
        );

        assert_eq!(library.glk_fileref_get_rock(Some(saved)), 1);
        assert_eq!(library.glk_fileref_get_rock(None), 0);

        let mut rock = RefSlot::default();

        assert_eq!(
            library.glk_fileref_iterate(None, Some(&mut rock)),
            Some(data)
        );
        assert_eq!(library.glk_fileref_iterate(Some(data), None), Some(notes));

        let twin = library
            .glk_fileref_create_from_fileref(file_usage::DATA, Some(saved), 4)
            .unwrap();

        assert_eq!(
            library.filerefs[&twin].filename,
            library.filerefs[&saved].filename
        );

        let error = library
            .glk_fileref_create_from_fileref(file_usage::DATA, None, 0)
            .unwrap_err();

        assert!(glk_message(error).contains("invalid fileref"));

        let _ = std::fs::remove_dir_all(dir);
    }

    // A temporary file exists until its reference is destroyed;
    // the prompt makes a reference only when the player answers.
    #[test]
    fn temporary_and_prompted_files() {
        let dir = scratch_dir("prompted");
        let mut library = Glk::new(Box::new(Prompter {
            name: Some("chosen".into()),
        }));

        library.save_dir = dir.clone();

        let temp = library
            .glk_fileref_create_temp(file_usage::DATA, 0)
            .unwrap();

        assert_eq!(library.glk_fileref_does_file_exist(Some(temp)), 1);

        library.glk_fileref_destroy(Some(temp));

        assert_eq!(library.glk_fileref_does_file_exist(Some(temp)), 0);
        assert_eq!(library.glk_fileref_does_file_exist(None), 0);

        library.glk_fileref_destroy(None);

        let asked = library
            .glk_fileref_create_by_prompt(file_usage::DATA, file_mode::WRITE, 0)
            .expect("the prompt answers a name");

        assert_eq!(
            library.filerefs[&asked].filename,
            dir.join("chosen.glkdata").to_string_lossy()
        );

        let mut refused = Glk::new(Box::new(Prompter { name: None }));

        refused.save_dir = dir.clone();

        assert_eq!(
            refused.glk_fileref_create_by_prompt(file_usage::DATA, file_mode::WRITE, 0),
            None
        );

        // Deleting through a reference destroys the file, not the
        // reference.
        let target = library.glk_fileref_create_by_name(file_usage::DATA, "gone", 0);

        std::fs::write(&library.filerefs[&target].filename, b"x").unwrap();

        library.glk_fileref_delete_file(Some(target));
        library.glk_fileref_delete_file(None);

        assert!(!std::path::Path::new(&library.filerefs[&target].filename).exists());

        let _ = std::fs::remove_dir_all(dir);
    }

    // File streams write and read through their reference; a file
    // that will not open answers the null stream rather than
    // faulting.
    #[test]
    fn file_streams_round_trip() {
        let dir = scratch_dir("streams");
        let mut library = Glk::new(Box::new(Quiet::new().0));
        let mut memory = ram();

        library.save_dir = dir.clone();

        let fileref = library.glk_fileref_create_by_name(file_usage::DATA, "story", 0);

        let writer = library
            .glk_stream_open_file(Some(fileref), file_mode::WRITE, 0)
            .unwrap()
            .expect("the write stream opens");

        library
            .glk_put_string_stream(&mut memory, Some(writer), "hello")
            .unwrap();
        library.glk_stream_close(Some(writer), None).unwrap();

        // WriteAppend starts at the end; ReadWrite starts at the
        // top.
        let appender = library
            .glk_stream_open_file(Some(fileref), file_mode::WRITE_APPEND, 0)
            .unwrap()
            .expect("the append stream opens");

        library
            .glk_put_string_stream(&mut memory, Some(appender), "!")
            .unwrap();
        library.glk_stream_close(Some(appender), None).unwrap();

        let reader = library
            .glk_stream_open_file(Some(fileref), file_mode::READ, 0)
            .unwrap()
            .expect("the read stream opens");

        let line = bytes_at(0x1A0, 8);

        assert_eq!(
            library
                .glk_get_line_stream(&mut memory, Some(reader), Some(line))
                .unwrap(),
            6
        );
        library.glk_stream_close(Some(reader), None).unwrap();

        // ReadWrite conjures a missing file into being.
        let fresh = library.glk_fileref_create_by_name(file_usage::DATA, "fresh", 0);
        let conjured = library
            .glk_stream_open_file_uni(Some(fresh), file_mode::READ_WRITE, 0)
            .unwrap()
            .expect("the read-write stream opens");

        library.glk_stream_close(Some(conjured), None).unwrap();

        assert!(std::path::Path::new(&library.filerefs[&fresh].filename).exists());

        // A directory in the file's seat will not open.
        let blocked = library.glk_fileref_create_by_name(file_usage::DATA, "blocked", 0);

        std::fs::create_dir(&library.filerefs[&blocked].filename).unwrap();

        assert_eq!(
            library
                .glk_stream_open_file(Some(blocked), file_mode::READ, 0)
                .unwrap(),
            None
        );

        let error = library
            .glk_stream_open_file(None, file_mode::READ, 0)
            .unwrap_err();

        assert!(glk_message(error).contains("invalid fileref"));

        let error = library
            .glk_stream_open_file(Some(fileref), 9, 0)
            .unwrap_err();

        assert!(glk_message(error).contains("illegal filemode"));

        let _ = std::fs::remove_dir_all(dir);
    }

    // Resource streams open read-only over Blorb data chunks, byte
    // or word, and answer None for a number the Blorb does not
    // carry.
    #[test]
    fn resource_streams_open_over_the_blorb() {
        let mut library = Glk::new(Box::new(Quiet::new().0));
        let memory = ram();

        library.resources = Resources::new(Some(built_blorb(&[
            (*b"Data", 1, *b"TEXT", b"hi"),
            (*b"Data", 2, *b"BINA", b"\x00\x00\x26\x03"),
        ])));

        let text = library
            .glk_stream_open_resource(1, 0)
            .expect("the text resource opens");
        let words = library
            .glk_stream_open_resource_uni(2, 0)
            .expect("the word resource opens");

        assert_eq!(
            library.glk_get_char_stream(&memory, Some(text)).unwrap(),
            0x68
        );
        assert_eq!(
            library.glk_get_char_stream(&memory, Some(words)).unwrap(),
            0x2603
        );
        assert_eq!(library.glk_stream_open_resource(9, 0), None);
    }

    // Pictures are measured from the Blorb; drawing needs a
    // display that draws, a window, and a picture that exists.
    #[test]
    fn pictures_measure_and_draw() {
        let (display, log) = Artist::new();
        let mut library = Glk::new(Box::new(display));

        library.resources =
            Resources::new(Some(built_blorb(&[(*b"Pict", 1, *b"PNG ", &png(32, 16))])));

        let base = library
            .glk_window_open(None, 0, 0, window_type::TEXT_BUFFER, 0)
            .unwrap()
            .unwrap();

        let mut width = RefSlot::default();
        let mut height = RefSlot::default();

        assert_eq!(
            library.glk_image_get_info(1, Some(&mut width), Some(&mut height)),
            1
        );
        assert_eq!((width.0.word(), height.0.word()), (32, 16));
        assert_eq!(
            library.glk_image_get_info(9, Some(&mut width), Some(&mut height)),
            0
        );
        assert_eq!((width.0.word(), height.0.word()), (0, 0));
        assert_eq!(library.glk_image_get_info(1, None, None), 1);

        assert_eq!(library.glk_image_draw(Some(base), 1, 4, 5), 1);
        assert_eq!(log.borrow().calls.last().unwrap(), "draw 4 5 32 16");

        assert_eq!(
            library.glk_image_draw_scaled(Some(base), 1, 0, 0, 64, 32),
            1
        );
        assert_eq!(log.borrow().calls.last().unwrap(), "draw 0 0 64 32");

        assert_eq!(
            library.glk_image_draw_scaled_ext(Some(base), 1, 0, 0, 8, 8, 0, 0),
            1
        );

        assert_eq!(library.glk_image_draw(None, 1, 0, 0), 0);
        assert_eq!(library.glk_image_draw(Some(base), 9, 0, 0), 0);

        library.glk_window_erase_rect(Some(base), 1, 2, 3, 4);
        library.glk_window_fill_rect(Some(base), 0xFF0000, 0, 0, 1, 1);
        library.glk_window_set_background_color(Some(base), 0x00FF00);
        library.glk_window_flow_break(Some(base));

        let calls = log.borrow().calls.clone();

        assert_eq!(
            calls[calls.len() - 4..],
            [
                "erase 1 2 3 4".to_string(),
                "fill 0xff0000".to_string(),
                "background 0xff00".to_string(),
                "flow".to_string()
            ]
        );

        library.glk_window_erase_rect(None, 0, 0, 1, 1);
        library.glk_window_fill_rect(None, 0, 0, 0, 1, 1);
        library.glk_window_set_background_color(None, 0);
        library.glk_window_flow_break(None);
    }

    // Style hints are recorded and withdrawn; distinguishing and
    // measuring are the display's answers, defaulting to no.
    #[test]
    fn style_hints_and_measures() {
        let (mut library, first) = quiet_rooted();

        library.glk_stylehint_set(window_type::TEXT_BUFFER, style::HEADER, 4, 1);

        assert_eq!(library.stylehints.len(), 1);

        library.glk_stylehint_clear(window_type::TEXT_BUFFER, style::HEADER, 4);
        library.glk_stylehint_clear(window_type::TEXT_BUFFER, style::HEADER, 4);

        assert!(library.stylehints.is_empty());

        assert_eq!(
            library.glk_style_distinguish(Some(first), style::NORMAL, style::HEADER),
            0
        );
        assert_eq!(
            library.glk_style_distinguish(Some(first), style::HEADER, style::HEADER),
            0
        );
        assert_eq!(
            library.glk_style_distinguish(None, style::NORMAL, style::HEADER),
            0
        );

        let mut result = RefSlot::default();

        assert_eq!(
            library.glk_style_measure(Some(first), style::NORMAL, 0, Some(&mut result)),
            0
        );
        assert_eq!(
            library.glk_style_measure(None, style::NORMAL, 0, Some(&mut result)),
            0
        );

        let (telling, styled) = rooted(Box::new(Styler));

        assert_eq!(
            telling.glk_style_distinguish(Some(styled), style::NORMAL, style::HEADER),
            1
        );
        assert_eq!(
            telling.glk_style_measure(Some(styled), style::NORMAL, 0, Some(&mut result)),
            1
        );
        assert_eq!(result.0.word(), 7);
        assert_eq!(
            telling.glk_style_measure(Some(styled), style::NORMAL, 0, None),
            1
        );
    }

    // Music means MOD and song files; the only decoder aboard is
    // AIFF, so the music claim stays zero even where sampled sound
    // plays.
    #[test]
    fn music_is_never_claimed() {
        let mut library = Glk::new(Box::new(Sounder::new(true).0));

        assert_eq!(library.glk_gestalt(glk_gestalt::SOUND, 0), 1);
        assert_eq!(library.glk_gestalt(glk_gestalt::SOUND_MUSIC, 0), 0);
    }

    // Sound channels exist only where the display plays; playing
    // asks the display and records what the channel is doing.
    #[test]
    fn sound_channels_play_where_they_can() {
        let mut silent = Glk::new(Box::new(Quiet::new().0));

        assert_eq!(silent.glk_schannel_create(0), None);

        let (display, log) = Sounder::new(true);
        let mut library = Glk::new(Box::new(display));
        let memory = ram();

        library.resources =
            Resources::new(Some(built_blorb(&[(*b"Snd ", 3, *b"FORM", b"AIFFdata")])));

        let channel = library.glk_schannel_create(1).expect("the channel opens");
        let other = library
            .glk_schannel_create_ext(2, 0x8000)
            .expect("the channel opens");

        assert_eq!(library.channels[&channel].volume, 0x10000);
        assert_eq!(library.channels[&other].volume, 0x8000);
        assert_eq!(library.glk_schannel_get_rock(Some(channel)), 1);
        assert_eq!(library.glk_schannel_get_rock(None), 0);

        let mut rock = RefSlot::default();

        assert_eq!(
            library.glk_schannel_iterate(None, Some(&mut rock)),
            Some(other)
        );
        assert_eq!(
            library.glk_schannel_iterate(Some(other), None),
            Some(channel)
        );

        assert_eq!(library.glk_schannel_play(Some(channel), 3), 1);
        assert_eq!(library.channels[&channel].sound, 3);

        // A missing sound, zero repeats, and the null channel all
        // decline; a refusing display declines too.
        assert_eq!(library.glk_schannel_play(Some(channel), 9), 0);
        assert_eq!(library.glk_schannel_play_ext(Some(channel), 3, 0, 0), 0);
        assert_eq!(library.glk_schannel_play(None, 3), 0);

        let mut refusing = Glk::new(Box::new(Sounder::new(false).0));

        refusing.resources =
            Resources::new(Some(built_blorb(&[(*b"Snd ", 3, *b"FORM", b"AIFFdata")])));

        let denied = refusing.glk_schannel_create(0);

        assert_eq!(refusing.glk_schannel_play(denied, 3), 0);

        let sounds = words_at(0x1C0, 2);
        let mut writable = ram();

        write_words(&mut writable, sounds, &[3, 9]);

        assert_eq!(
            library
                .glk_schannel_play_multi(&writable, &[Some(channel), Some(other)], Some(sounds), 0)
                .unwrap(),
            1
        );
        assert_eq!(
            library
                .glk_schannel_play_multi(&memory, &[], None, 0)
                .unwrap(),
            0
        );

        library.glk_schannel_pause(Some(channel));
        library.glk_schannel_pause(Some(channel));
        library.glk_schannel_unpause(Some(channel));
        library.glk_schannel_unpause(Some(channel));
        library.glk_schannel_pause(None);
        library.glk_schannel_unpause(None);

        let calls = log.borrow().calls.clone();

        assert!(calls.contains(&"pause true".to_string()));
        assert!(calls.contains(&"pause false".to_string()));

        library.glk_schannel_set_volume(Some(channel), 0x4000);

        assert_eq!(library.channels[&channel].volume, 0x4000);

        library.glk_schannel_set_volume_ext(Some(channel), 0x2000, 100, 7);
        library.glk_schannel_set_volume_ext(None, 0, 0, 0);

        assert_eq!(
            library.pending_events.last().unwrap().kind,
            event_type::VOLUME_NOTIFY
        );

        library.glk_sound_load_hint(3, 1);

        library.glk_schannel_stop(Some(channel));
        library.glk_schannel_stop(Some(channel));
        library.glk_schannel_stop(None);

        assert_eq!(library.channels[&channel].sound, 0);

        library.glk_schannel_destroy(Some(other));
        library.glk_schannel_destroy(None);

        assert_eq!(library.channel_order.len(), 1);
    }

    // Requests raise flags on windows and clear again; asking
    // twice for a line, or asking nothing at all, is refused.
    #[test]
    fn input_requests_raise_and_clear() {
        let (mut library, first) = quiet_rooted();
        let mut memory = ram();
        let held = bytes_at(0x180, 8);

        library
            .glk_request_line_event(Some(first), Some(held), 0)
            .unwrap();

        let error = library
            .glk_request_line_event_uni(Some(first), Some(held), 0)
            .unwrap_err();

        assert!(glk_message(error).contains("already requested"));

        let error = library
            .glk_request_line_event(None, Some(held), 0)
            .unwrap_err();

        assert!(glk_message(error).contains("invalid window"));

        library.glk_set_echo_line_event(Some(first), 0);

        write_words(&mut memory, words_at(0x1C0, 1), &[key_code::ESCAPE]);
        library
            .glk_set_terminators_line_event(&memory, Some(first), Some(words_at(0x1C0, 1)))
            .unwrap();

        let request = library.windows[&first]
            .line_request
            .clone()
            .expect("the request stands");

        assert!(!request.echo);
        assert_eq!(request.terminators, [key_code::ESCAPE]);

        let mut cancelled = StructSlot::new(4);

        library.glk_cancel_line_event(Some(first), Some(&mut cancelled));

        assert!(library.windows[&first].line_request.is_none());
        assert_eq!(cancelled.0[0].word(), event_type::NONE);

        library.glk_cancel_line_event(None, None);

        // The echo and terminator setters shrug without a request.
        library.glk_set_echo_line_event(Some(first), 1);
        library
            .glk_set_terminators_line_event(&memory, Some(first), None)
            .unwrap();
        library.glk_set_echo_line_event(None, 1);
        library
            .glk_set_terminators_line_event(&memory, None, None)
            .unwrap();

        library.glk_request_char_event(Some(first)).unwrap();

        assert!(library.windows[&first].char_request);

        library.glk_cancel_char_event(Some(first));
        library.glk_cancel_char_event(None);

        assert!(!library.windows[&first].char_request);

        library.glk_request_char_event_uni(Some(first)).unwrap();

        assert!(library.windows[&first].char_unicode);

        let error = library.glk_request_char_event(None).unwrap_err();

        assert!(glk_message(error).contains("invalid window"));

        library.glk_request_mouse_event(Some(first));

        assert!(library.windows[&first].mouse_request);

        library.glk_cancel_mouse_event(Some(first));
        library.glk_request_mouse_event(None);
        library.glk_cancel_mouse_event(None);

        library.glk_request_hyperlink_event(Some(first));

        assert!(library.windows[&first].hyperlink_request);

        library.glk_cancel_hyperlink_event(Some(first));
        library.glk_request_hyperlink_event(None);
        library.glk_cancel_hyperlink_event(None);

        library.glk_request_timer_events(250);

        assert_eq!(library.timer_interval, 250);
    }

    // A line arrives: the buffer fills, the window echoes it in
    // the Input style, and the event carries the length.
    #[test]
    fn a_line_arrives_and_echoes() {
        let (mut library, first) = rooted(Box::new(Typist::new(&[Some(("go north", 0))])));
        let mut memory = ram();
        let held = bytes_at(0x180, 12);

        library
            .glk_request_line_event(Some(first), Some(held), 0)
            .unwrap();

        let mut event = StructSlot::new(4);

        library.glk_select(&mut memory, &mut event).unwrap();

        assert_eq!(
            event_fields(&event),
            (event_type::LINE_INPUT, Some(first), 8, 0)
        );
        assert_eq!(
            word_list(&memory, held, 8),
            "go north".bytes().map(u32::from).collect::<Vec<u32>>()
        );
        assert!(library.windows[&first].line_request.is_none());

        let echoed = match &library.windows[&first].kind {
            WindowKind::Buffer(data) => data.content.clone(),
            _ => unreachable!(),
        };

        assert!(echoed.contains(&Flow::Run {
            style: style::INPUT,
            hyperlink: 0,
            text: "go north\n".into()
        }));
    }

    // A line longer than its buffer is truncated to what fits, and
    // a terminator key rides along in the event.
    #[test]
    fn a_long_line_truncates() {
        let (mut library, first) = rooted(Box::new(Typist::new(&[Some((
            "northwest",
            key_code::ESCAPE,
        ))])));
        let mut memory = ram();
        let held = bytes_at(0x180, 5);

        library
            .glk_request_line_event(Some(first), Some(held), 0)
            .unwrap();

        let mut event = StructSlot::new(4);

        library.glk_select(&mut memory, &mut event).unwrap();

        assert_eq!(
            event_fields(&event),
            (event_type::LINE_INPUT, Some(first), 5, key_code::ESCAPE)
        );
        assert_eq!(
            word_list(&memory, held, 5),
            "north".bytes().map(u32::from).collect::<Vec<u32>>()
        );
    }

    // Echo is suppressed when the request says so and when the
    // window keeps no scrollback.
    #[test]
    fn echo_suppression() {
        let (mut library, first) = rooted(Box::new(Typist::new(&[Some(("quiet", 0))])));
        let mut memory = ram();

        library
            .glk_request_line_event(Some(first), Some(bytes_at(0x180, 8)), 0)
            .unwrap();
        library.glk_set_echo_line_event(Some(first), 0);
        library
            .glk_select(&mut memory, &mut StructSlot::new(4))
            .unwrap();

        assert_eq!(library.windows[&first].text(), "");

        let (mut grid_library, base) = rooted(Box::new(Typist::new(&[Some(("grid", 0))])));
        let grid = grid_library
            .glk_window_open(Some(base), ABOVE_FIXED, 3, window_type::TEXT_GRID, 0)
            .unwrap()
            .unwrap();

        grid_library
            .glk_request_line_event(Some(grid), Some(bytes_at(0x190, 8)), 0)
            .unwrap();
        grid_library
            .glk_select(&mut memory, &mut StructSlot::new(4))
            .unwrap();

        let rows = grid_library.windows[&grid].rows().join("");

        assert!(rows.trim().is_empty());
    }

    // A timer fires while a keystroke is pending; the request
    // survives and is answered on the next select.
    #[test]
    fn a_timer_interrupts_a_keystroke() {
        let (mut library, first) = rooted(Box::new(Keyist {
            graphics: false,
            chars: vec![None, Some(0x42)],
        }));
        let mut memory = ram();

        library.glk_request_char_event(Some(first)).unwrap();

        let mut event = StructSlot::new(4);

        library.glk_select(&mut memory, &mut event).unwrap();

        assert_eq!(event.0[0].word(), event_type::TIMER);
        assert!(library.windows[&first].char_request);

        library.glk_select(&mut memory, &mut event).unwrap();

        assert_eq!(
            event_fields(&event),
            (event_type::CHAR_INPUT, Some(first), 0x42, 0)
        );
    }

    // A keystroke arrives; delivering one nobody asked for is
    // refused.
    #[test]
    fn a_keystroke_arrives() {
        let (mut library, first) = rooted(Box::new(Keyist {
            graphics: false,
            chars: vec![Some(0x41)],
        }));
        let mut memory = ram();

        library.glk_request_char_event(Some(first)).unwrap();

        let mut event = StructSlot::new(4);

        library.glk_select(&mut memory, &mut event).unwrap();

        assert_eq!(
            event_fields(&event),
            (event_type::CHAR_INPUT, Some(first), 0x41, 0)
        );
        assert!(!library.windows[&first].char_request);

        let error = library.deliver_char(first, 0x42).unwrap_err();

        assert!(glk_message(error).contains("not expecting"));

        let error = library
            .deliver_line(&mut memory, first, "stray", 0)
            .unwrap_err();

        assert!(glk_message(error).contains("not expecting"));
    }

    // A click and a link selection deliver from outside the ask
    // too, the way a protocol display delivers them -- requests
    // consumed, and ones nobody asked for refused.
    #[test]
    fn clicks_and_links_deliver_from_outside() {
        let (mut library, first) = quiet_rooted();

        library.glk_request_mouse_event(Some(first));

        let clicked = library.deliver_mouse(first, 3, 4).unwrap();

        assert_eq!(
            (clicked.kind, clicked.window, clicked.val1, clicked.val2),
            (event_type::MOUSE_INPUT, Some(first), 3, 4)
        );
        assert!(!library.windows[&first].mouse_request);

        let error = library.deliver_mouse(first, 1, 1).unwrap_err();

        assert!(glk_message(error).contains("not expecting"));

        library.glk_request_hyperlink_event(Some(first));

        let linked = library.deliver_hyperlink(first, 7).unwrap();

        assert_eq!(
            (linked.kind, linked.window, linked.val1, linked.val2),
            (event_type::HYPERLINK, Some(first), 7, 0)
        );
        assert!(!library.windows[&first].hyperlink_request);

        let error = library.deliver_hyperlink(first, 7).unwrap_err();

        assert!(glk_message(error).contains("not expecting"));
    }

    // A suspending display is never asked for a file either: the
    // call itself stands down; the host's answer mints the
    // reference for the bridge's parked tail to store.
    #[test]
    fn a_file_prompt_suspends_the_call() {
        let dir = scratch_dir("suspend");
        let (mut library, _) = rooted(Box::new(Suspending::default()));

        library.save_dir = dir.clone();

        let answered =
            library.glk_fileref_create_by_prompt(file_usage::SAVED_GAME, file_mode::WRITE, 7);

        assert_eq!(answered, None);
        assert_eq!(
            library.waiting,
            Some(Waiting::Prompt {
                usage: file_usage::SAVED_GAME,
                fmode: file_mode::WRITE,
                rock: 7
            })
        );

        let minted = library
            .deliver_file(Some("saga"))
            .unwrap()
            .expect("the answer mints a reference");

        assert_eq!(library.filerefs[&minted].rock, 7);
        assert_eq!(library.waiting, None);

        let error = library.deliver_file(Some("saga")).unwrap_err();

        assert!(glk_message(error).contains("no prompt suspended"));

        let _ = std::fs::remove_dir_all(dir);
    }

    // A cancel stores the null reference; a file answered at a
    // select, or an event answered at a prompt, is a driver's bug
    // and loud.
    #[test]
    fn files_and_events_land_only_in_their_own_waits() {
        let (mut library, first) = rooted(Box::new(Suspending::default()));
        let mut memory = ram();

        library.glk_fileref_create_by_prompt(file_usage::DATA, file_mode::READ, 0);

        let error = library
            .deliver_event(Event::new(event_type::TIMER, None, 0, 0))
            .unwrap_err();

        assert!(glk_message(error).contains("no select suspended"));

        assert_eq!(library.deliver_file(None).unwrap(), None);

        library.glk_request_char_event(Some(first)).unwrap();
        library
            .glk_select(&mut memory, &mut StructSlot::new(4))
            .unwrap();

        let error = library.deliver_file(Some("saga")).unwrap_err();

        assert!(glk_message(error).contains("no prompt suspended"));
    }

    // The prompt's name is the player's own, never the game-name
    // jail: an absolute path is honored whole, a relative one
    // lands in the save dir, a bare one gains its usage's suffix
    // as a courtesy, and an explicit suffix is kept exactly as
    // chosen.
    #[test]
    fn a_prompted_name_is_the_players_own() {
        let dir = scratch_dir("players-own");
        let (mut library, _) = rooted(Box::new(Suspending::default()));

        library.save_dir = dir.clone();

        let mut prompted = |name: String| -> String {
            library.glk_fileref_create_by_prompt(file_usage::SAVED_GAME, file_mode::WRITE, 0);

            let minted = library
                .deliver_file(Some(&name))
                .unwrap()
                .expect("the prompt is answered");

            library.filerefs[&minted].filename.clone()
        };

        let chosen = dir.join("kept").join("expedition");

        assert_eq!(
            prompted(chosen.to_string_lossy().into_owned()),
            format!("{}.glksave", chosen.to_string_lossy())
        );
        assert_eq!(
            prompted("beside".into()),
            dir.join("beside.glksave").to_string_lossy()
        );
        assert_eq!(
            prompted("named.sav".into()),
            dir.join("named.sav").to_string_lossy()
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    // A timer fires while a line is pending: the request survives
    // the interruption and is answered on the next select.
    #[test]
    fn a_timer_interrupts_a_line() {
        let (mut library, first) = rooted(Box::new(Typist::new(&[None, Some(("after", 0))])));
        let mut memory = ram();

        library
            .glk_request_line_event(Some(first), Some(bytes_at(0x180, 8)), 0)
            .unwrap();

        let mut event = StructSlot::new(4);

        library.glk_select(&mut memory, &mut event).unwrap();

        assert_eq!(event.0[0].word(), event_type::TIMER);
        assert!(library.windows[&first].line_request.is_some());

        library.glk_select(&mut memory, &mut event).unwrap();

        assert_eq!(event.0[0].word(), event_type::LINE_INPUT);
    }

    // A click and a link selection arrive through the same loop,
    // each allowed a "not yet" round first.
    #[test]
    fn clicks_and_links_arrive() {
        let (mut library, first) = rooted(Box::new(Clicker {
            mice: vec![None, Some((3, 4))],
        }));
        let mut memory = ram();

        library.glk_request_mouse_event(Some(first));

        let mut event = StructSlot::new(4);

        library.glk_select(&mut memory, &mut event).unwrap();

        assert_eq!(
            event_fields(&event),
            (event_type::MOUSE_INPUT, Some(first), 3, 4)
        );
        assert!(!library.windows[&first].mouse_request);

        let (mut linked, page) = rooted(Box::new(Linker { links: vec![0, 7] }));

        linked.glk_request_hyperlink_event(Some(page));

        linked.glk_select(&mut memory, &mut event).unwrap();

        assert_eq!(
            event_fields(&event),
            (event_type::HYPERLINK, Some(page), 7, 0)
        );
    }

    // A select that can never be satisfied is refused rather than
    // hung: requests a display cannot answer, or no requests at
    // all.
    #[test]
    fn a_hopeless_select_is_refused() {
        let (mut library, first) = quiet_rooted();
        let mut memory = ram();

        let error = library
            .glk_select(&mut memory, &mut StructSlot::new(4))
            .unwrap_err();

        assert!(glk_message(error).contains("wait forever"));

        library.glk_request_mouse_event(Some(first));
        library.glk_request_hyperlink_event(Some(first));

        let error = library
            .glk_select(&mut memory, &mut StructSlot::new(4))
            .unwrap_err();

        assert!(glk_message(error).contains("wait forever"));
    }

    // A suspending display is never asked for input. Its select
    // records the wait instead -- the struct stays whole and
    // empty, the seat the host's event will land in.
    #[test]
    fn a_suspending_select_records_the_wait() {
        let display = Suspending::default();
        let flushes = display.flushes.clone();
        let (mut library, first) = rooted(Box::new(display));
        let mut memory = ram();

        library.glk_request_char_event(Some(first)).unwrap();

        let mut event = StructSlot::new(4);

        library.glk_select(&mut memory, &mut event).unwrap();

        assert_eq!(library.waiting, Some(Waiting::Select));
        assert_eq!(event.0, vec![Held::Word(0); 4]);
        assert!(*flushes.borrow() > 0);
    }

    // Whatever a display posted is delivered at once: a queued
    // event needs no suspension, exactly as it needs no blocking.
    #[test]
    fn a_suspending_select_serves_the_queue_first() {
        let (mut library, _) = rooted(Box::new(Suspending::default()));
        let mut memory = ram();

        library.post_event(Event::new(event_type::TIMER, None, 0, 0));

        let mut event = StructSlot::new(4);

        library.glk_select(&mut memory, &mut event).unwrap();

        assert_eq!(event_fields(&event), (event_type::TIMER, None, 0, 0));
        assert_eq!(library.waiting, None);
    }

    // The hopeless-select guard holds while suspending too:
    // requests count only where the display claims the capability,
    // and a running timer is a legitimate wait.
    #[test]
    fn a_hopeless_suspension_is_refused() {
        let (mut library, first) = rooted(Box::new(Suspending::default()));
        let mut memory = ram();

        let error = library
            .glk_select(&mut memory, &mut StructSlot::new(4))
            .unwrap_err();

        assert!(glk_message(error).contains("wait forever"));

        library.glk_request_mouse_event(Some(first));
        library.glk_request_hyperlink_event(Some(first));

        let error = library
            .glk_select(&mut memory, &mut StructSlot::new(4))
            .unwrap_err();

        assert!(glk_message(error).contains("wait forever"));

        let (mut timed, _) = rooted(Box::new(Suspending {
            timer: true,
            mouse: true,
            hyper: true,
            ..Default::default()
        }));

        let error = timed
            .glk_select(&mut memory, &mut StructSlot::new(4))
            .unwrap_err();

        assert!(glk_message(error).contains("wait forever"));

        timed.glk_request_timer_events(50);
        timed
            .glk_select(&mut memory, &mut StructSlot::new(4))
            .unwrap();

        assert!(timed.waiting.is_some());
    }

    // A claimed capability's request carries the wait; the
    // delivered event comes back for the bridge to land.
    #[test]
    fn claimed_requests_carry_the_wait() {
        let (mut library, first) = rooted(Box::new(Suspending {
            mouse: true,
            ..Default::default()
        }));
        let mut memory = ram();

        library.glk_request_mouse_event(Some(first));

        let mut event = StructSlot::new(4);

        library.glk_select(&mut memory, &mut event).unwrap();

        assert!(library.waiting.is_some());

        let answered = library
            .deliver_event(Event::new(event_type::MOUSE_INPUT, Some(first), 3, 4))
            .unwrap();

        fill_event(&mut event, answered);

        assert_eq!(
            event_fields(&event),
            (event_type::MOUSE_INPUT, Some(first), 3, 4)
        );
        assert_eq!(library.waiting, None);

        let (mut pages, page) = rooted(Box::new(Suspending {
            hyper: true,
            ..Default::default()
        }));

        pages.glk_request_hyperlink_event(Some(page));
        pages
            .glk_select(&mut memory, &mut StructSlot::new(4))
            .unwrap();

        assert!(pages.waiting.is_some());
    }

    // The delivered event lands in its seat; an event with no seat
    // to land in is refused.
    #[test]
    fn a_delivered_event_lands_in_its_seat() {
        let (mut library, first) = rooted(Box::new(Suspending::default()));
        let mut memory = ram();
        let held = bytes_at(0x180, 8);

        library
            .glk_request_line_event(Some(first), Some(held), 0)
            .unwrap();

        let mut event = StructSlot::new(4);

        library.glk_select(&mut memory, &mut event).unwrap();

        assert!(library.waiting.is_some());

        let answered = library.deliver_line(&mut memory, first, "go", 0).unwrap();
        let answered = library.deliver_event(answered).unwrap();

        fill_event(&mut event, answered);

        assert_eq!(
            event_fields(&event),
            (event_type::LINE_INPUT, Some(first), 2, 0)
        );
        assert_eq!(
            word_list(&memory, held, 2),
            [u32::from(b'g'), u32::from(b'o')]
        );
        assert_eq!(library.waiting, None);

        let error = library.deliver_event(answered).unwrap_err();

        assert!(glk_message(error).contains("no select suspended"));
    }

    // The poll reports queued display events, skips over anything
    // that is input, and never blocks.
    #[test]
    fn polling_skips_input_events() {
        let (mut library, first) = quiet_rooted();
        let mut event = StructSlot::new(4);

        library.glk_select_poll(&mut event);

        assert_eq!(event.0[0].word(), event_type::NONE);

        library.post_event(Event::new(event_type::CHAR_INPUT, Some(first), 0x41, 0));
        library.post_event(Event::new(event_type::TIMER, None, 0, 0));

        library.glk_select_poll(&mut event);

        assert_eq!(event.0[0].word(), event_type::TIMER);

        // The input event stays queued for a real select to take.
        let mut memory = ram();

        library.glk_select(&mut memory, &mut event).unwrap();

        assert_eq!(event.0[0].word(), event_type::CHAR_INPUT);
    }

    // A resized display re-lays the tree and tells the game.
    #[test]
    fn a_resize_reaches_the_game() {
        let (mut library, _) = quiet_rooted();

        library.display_resized();

        assert_eq!(
            library.pending_events.last().unwrap().kind,
            event_type::ARRANGE
        );
    }

    // The character case functions map what a single character can
    // hold and leave the rest alone.
    #[test]
    fn case_maps_single_characters() {
        let library = Glk::new(Box::new(Quiet::new().0));

        assert_eq!(library.glk_char_to_lower(u32::from(b'A')), u32::from(b'a'));
        assert_eq!(library.glk_char_to_upper(u32::from(b'a')), u32::from(b'A'));
        assert_eq!(library.glk_char_to_upper(0xDF), 0xDF);
        assert_eq!(library.glk_char_to_lower(0x110000), 0x110000);
    }

    // The buffer case functions work in place, answer the true
    // length even past the buffer, and map per character rather
    // than per string.
    #[test]
    fn buffer_case_and_normalization() {
        let library = Glk::new(Box::new(Quiet::new().0));
        let mut memory = ram();
        let word = words_at(0x180, 4);

        write_words(
            &mut memory,
            word,
            &"Wave".chars().map(u32::from).collect::<Vec<u32>>(),
        );

        assert_eq!(
            library
                .glk_buffer_to_upper_case_uni(&mut memory, Some(word), 4)
                .unwrap(),
            4
        );
        assert_eq!(
            word_list(&memory, word, 4),
            "WAVE".chars().map(u32::from).collect::<Vec<u32>>()
        );

        assert_eq!(
            library
                .glk_buffer_to_lower_case_uni(&mut memory, Some(word), 4)
                .unwrap(),
            4
        );
        assert_eq!(
            word_list(&memory, word, 4),
            "wave".chars().map(u32::from).collect::<Vec<u32>>()
        );

        assert_eq!(
            library
                .glk_buffer_to_title_case_uni(&mut memory, Some(word), 4, 0)
                .unwrap(),
            4
        );
        assert_eq!(
            word_list(&memory, word, 4),
            "Wave".chars().map(u32::from).collect::<Vec<u32>>()
        );

        write_words(
            &mut memory,
            word,
            &"WAVE".chars().map(u32::from).collect::<Vec<u32>>(),
        );

        assert_eq!(
            library
                .glk_buffer_to_title_case_uni(&mut memory, Some(word), 4, 1)
                .unwrap(),
            4
        );
        assert_eq!(
            word_list(&memory, word, 4),
            "Wave".chars().map(u32::from).collect::<Vec<u32>>()
        );
        assert_eq!(
            library
                .glk_buffer_to_title_case_uni(&mut memory, Some(words_at(0x1C0, 0)), 0, 1)
                .unwrap(),
            0
        );

        // The digraphs carry distinct titlecase forms.
        let digraph = words_at(0x1D0, 1);

        write_words(&mut memory, digraph, &[0x01C6]);

        library
            .glk_buffer_to_title_case_uni(&mut memory, Some(digraph), 1, 0)
            .unwrap();

        assert_eq!(digraph.get(&memory, 0).unwrap(), 0x01C5);

        // ß uppercases to SS: two characters, so the true length
        // is answered while the buffer keeps what fits.
        let sharp = words_at(0x1E0, 1);

        write_words(&mut memory, sharp, &[0xDF]);

        assert_eq!(
            library
                .glk_buffer_to_upper_case_uni(&mut memory, Some(sharp), 1)
                .unwrap(),
            2
        );
        assert_eq!(sharp.get(&memory, 0).unwrap(), u32::from(b'S'));

        // é decomposes to two code points and composes back to
        // one.
        let accented = words_at(0x1F0, 2);

        write_words(&mut memory, accented, &[0xE9, 0]);

        assert_eq!(
            library
                .glk_buffer_canon_decompose_uni(&mut memory, Some(accented), 1)
                .unwrap(),
            2
        );
        assert_eq!(word_list(&memory, accented, 2), [0x65, 0x301]);
        assert_eq!(
            library
                .glk_buffer_canon_normalize_uni(&mut memory, Some(accented), 2)
                .unwrap(),
            1
        );
        assert_eq!(accented.get(&memory, 0).unwrap(), 0xE9);

        assert_eq!(
            library
                .glk_buffer_to_upper_case_uni(&mut memory, None, 4)
                .unwrap(),
            0
        );
    }

    // The real clock runs when nobody pins it.
    #[test]
    fn the_real_clock_ticks() {
        let library = Glk::new(Box::new(Quiet::new().0));

        assert!(library.glk_current_simple_time(1) > 1_700_000_000);
    }

    // The clock answers real time split into two words, and
    // divided down for the simple form.
    #[test]
    fn the_clock_answers_now() {
        let mut library = Glk::new(Box::new(Quiet::new().0));

        library.now_override = Some(1_700_000_000.5);

        let mut time = StructSlot::new(3);

        library.glk_current_time(Some(&mut time));

        assert_eq!(
            time.0,
            [
                Held::Word(0),
                Held::Word(1_700_000_000),
                Held::Word(500_000)
            ]
        );

        library.glk_current_time(None);

        assert_eq!(library.glk_current_simple_time(60), 1_700_000_000 / 60);
        assert_eq!(library.glk_current_simple_time(0), -1);
    }

    // A timestamp explodes into date fields -- weekdays counted
    // from Sunday -- and collapses back, normalizing out-of-range
    // fields.
    #[test]
    fn dates_explode_and_collapse() {
        let library = Glk::new(Box::new(Quiet::new().0));
        let mut time = StructSlot::new(3);
        let mut date = StructSlot::new(8);

        // 2023-11-14 22:13:20 UTC, a Tuesday.
        time.set_all(&[Held::Word(0), Held::Word(1_700_000_000), Held::Word(250)]);

        library.glk_time_to_date_utc(Some(&time), Some(&mut date));

        assert_eq!(
            date.0.iter().map(Held::word).collect::<Vec<u32>>(),
            [2023, 11, 14, 2, 22, 13, 20, 250]
        );

        library.glk_date_to_time_utc(Some(&date), Some(&mut time));

        assert_eq!(
            time.0,
            [Held::Word(0), Held::Word(1_700_000_000), Held::Word(250)]
        );

        // Month 14 normalizes into the next year.
        date.set_all(&[
            Held::Word(2023),
            Held::Word(14),
            Held::Word(1),
            Held::Word(0),
            Held::Word(0),
            Held::Word(0),
            Held::Word(0),
            Held::Word(0),
        ]);

        library.glk_date_to_time_utc(Some(&date), Some(&mut time));
        library.glk_time_to_date_utc(Some(&time), Some(&mut date));

        assert_eq!(
            date.0[..3].iter().map(Held::word).collect::<Vec<u32>>(),
            [2024, 2, 1]
        );

        // The simple forms divide down and multiply back.
        library.glk_simple_time_to_date_utc(19675, 86400, Some(&mut date));

        assert_eq!(
            date.0[..3].iter().map(Held::word).collect::<Vec<u32>>(),
            [2023, 11, 14]
        );
        assert_eq!(
            library.glk_date_to_simple_time_utc(Some(&date), 86400),
            19675
        );

        // The local forms answer through the configured offset --
        // five hours west of UTC puts this stamp the previous
        // evening.
        let mut west = Glk::new(Box::new(Quiet::new().0));

        west.local_offset_seconds = -5 * 3600;

        time.set_all(&[Held::Word(0), Held::Word(1_700_000_000), Held::Word(250)]);

        west.glk_time_to_date_local(Some(&time), Some(&mut date));

        assert_eq!(
            date.0[..3].iter().map(Held::word).collect::<Vec<u32>>(),
            [2023, 11, 14]
        );
        assert_eq!(date.0[4].word(), 17);

        west.glk_date_to_time_local(Some(&date), Some(&mut time));

        assert_eq!(time.0[1].word(), 1_700_000_000);
        assert_eq!(
            west.glk_date_to_simple_time_local(Some(&date), 60),
            1_700_000_000 / 60
        );
    }

    // The bridge hears about every disposal, once it asks: closing
    // a window reports the window and its stream, so stale ids
    // stop resolving.
    #[test]
    fn disposals_are_reported() {
        let (mut library, first) = quiet_rooted();
        let stream = library.windows[&first].stream;

        library.take_disposals();

        library.glk_window_close(Some(first), None).unwrap();

        let reported = library.take_disposals();

        assert!(reported.contains(&(CLASS_WINDOW, first)));
        assert!(reported.contains(&(CLASS_STREAM, stream)));
    }

    // Destroying what is already gone stays quiet: a fileref or a
    // channel off the lists is simply let go.
    #[test]
    fn double_destroys_stay_quiet() {
        let dir = scratch_dir("double");
        let mut library = Glk::new(Box::new(Sounder::new(true).0));

        library.save_dir = dir.clone();

        let kept = library.glk_fileref_create_by_name(file_usage::DATA, "kept", 0);

        library.glk_fileref_destroy(Some(kept));
        library.glk_fileref_destroy(Some(kept));

        assert!(!library.filerefs.contains_key(&kept));

        let channel = library.glk_schannel_create(0);

        library.glk_schannel_destroy(channel);
        library.glk_schannel_destroy(channel);

        assert!(library.channel_order.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }

    // The unanswerable clock questions answer their failure values
    // rather than faulting: null refs, impossible years, zero
    // factors.
    #[test]
    fn impossible_dates_fail_softly() {
        let library = Glk::new(Box::new(Quiet::new().0));
        let mut time = StructSlot::new(3);
        let mut date = StructSlot::new(8);

        library.glk_time_to_date_utc(None, Some(&mut date));

        assert_eq!(date.0, vec![Held::Word(0); 8]);

        library.glk_time_to_date_utc(Some(&time), None);
        library.glk_date_to_time_utc(Some(&date), None);

        // A year past every calendar collapses to the -1 sentinel.
        date.set_all(&[
            Held::Word(999_999_999),
            Held::Word(1),
            Held::Word(1),
            Held::Word(0),
            Held::Word(0),
            Held::Word(0),
            Held::Word(0),
            Held::Word(0),
        ]);

        library.glk_date_to_time_utc(Some(&date), Some(&mut time));

        assert_eq!(
            time.0,
            [
                Held::Word(0xFFFF_FFFF),
                Held::Word(0xFFFF_FFFF),
                Held::Word(0)
            ]
        );

        library.glk_date_to_time_utc(None, Some(&mut time));

        assert_eq!(
            time.0,
            [
                Held::Word(0xFFFF_FFFF),
                Held::Word(0xFFFF_FFFF),
                Held::Word(0)
            ]
        );

        assert_eq!(library.glk_date_to_simple_time_utc(Some(&date), 60), -1);
        assert_eq!(library.glk_date_to_simple_time_utc(None, 60), -1);
        assert_eq!(library.glk_date_to_simple_time_utc(Some(&date), 0), -1);

        // And a second count past every timestamp explodes to
        // zeros.
        library.glk_simple_time_to_date_utc(1 << 40, 1 << 22, Some(&mut date));

        assert_eq!(date.0, vec![Held::Word(0); 8]);

        library.glk_simple_time_to_date_local(0, 1, Some(&mut date));

        assert!(date.0[0].word() >= 1969);

        library.glk_simple_time_to_date_utc(0, 1, None);
    }

    // The null display shows nothing and, asked for input that can
    // never arrive, ends the session rather than hanging forever.
    #[test]
    fn the_null_display_ends_rather_than_hangs() {
        let mut display = NullFrontend;
        let mut windows = WindowMap::new();

        assert_eq!(display.size(), (80, 24));

        display.flush(&mut windows, None);

        assert_eq!(display.read_line(&mut windows, 1, 80), Asked::End);
        assert_eq!(display.read_char(&mut windows, 1), Asked::End);

        // And through a select: the session ends as a value.
        let (mut library, first) = rooted(Box::new(NullFrontend));
        let mut memory = ram();

        library.glk_request_char_event(Some(first)).unwrap();

        assert!(matches!(
            library.glk_select(&mut memory, &mut StructSlot::new(4)),
            Err(Stop::End)
        ));
    }
}
