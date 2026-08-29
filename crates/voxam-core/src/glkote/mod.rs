//! The GlkOte update protocol, spoken from the game's side.
//!
//! GlkOte is a display library: a web page that draws windows and
//! raises input events, designed to be driven by a server that
//! does the window arithmetic -- the role RemGlk plays for the C
//! interpreters, and the role Voxam plays here (GlkOte: What is
//! GlkOte?). The conversation is JSON both ways; this module
//! builds the game's half of it, the update: which windows stand
//! where, what text arrived, which inputs are wanted (GlkOte:
//! Output: Updating the Display).
//!
//! Nothing here knows about any one machine. A Page is fed plain
//! facts -- boxes, styled runs, requests -- and keeps the
//! protocol's own state: the generation number, what the display
//! has already been shown, which input fields it holds. Each
//! machine feeds it the same way from its own screen model.
//!
//! Two reshapings from the reference, in the standing manner: the
//! stanzas are this crate's own insertion-ordered [`json`] values
//! rather than dicts (the module doc there says why), and the
//! iFiction record `carded` reads arrives as a small [`Card`]
//! until the Babel work lands to feed it.

pub mod json;

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};

use crate::errors::VoxamError;
use json::{Object, Value};

/// The eleven style names, in the order Glk numbers them; the
/// display renders each as a CSS class, so a style IS its name
/// here (GlkOte: The Line Data Array). Note blockquote is one
/// word.
pub const STYLES: [&str; 11] = [
    "normal",
    "emphasized",
    "preformatted",
    "header",
    "subheader",
    "alert",
    "note",
    "blockquote",
    "input",
    "user1",
    "user2",
];

/// The keys a line request may name as terminators; the protocol
/// knows no others (GlkOte: The Input Update Array).
pub fn terminator(name: &str) -> bool {
    name == "escape"
        || name
            .strip_prefix("func")
            .and_then(|number| number.parse::<u8>().ok())
            .is_some_and(|number| (1..=12).contains(&number))
}

/// How many sent paragraphs each buffer window keeps for a
/// refresh's re-telling: a display that lost its picture gets the
/// recent scrollback, not the whole session.
pub const KEPT_PARAGRAPHS: usize = 200;

// The window kinds the protocol draws; pairs and blanks are the
// server's business and never appear (GlkOte: The Windows Update
// Array).
const KINDS: [&str; 3] = ["buffer", "grid", "graphics"];

// What a file prompt may ask, by the protocol's own names
// (GlkOte: Special Input Requests).
const FILE_MODES: [&str; 4] = ["read", "write", "readwrite", "writeappend"];
const FILE_KINDS: [&str; 4] = ["data", "save", "transcript", "command"];

// The drawing operations a graphics content entry may carry.
// GlkOte names the first three (GlkOte: Graphics Window Updates);
// text and shift are the stage dialect's own -- placed characters
// and sliding rectangles, VΘXΔM's words for a §8.8 screen on a
// canvas whose both wire ends are ours.
const SPECIALS: [&str; 5] = ["setcolor", "fill", "image", "text", "shift"];
const RECT: [&str; 4] = ["x", "y", "width", "height"];

// What the dialect's own operations must name: a text op places a
// string of cells, a shift op slides a whole rectangle by a rise.
const TEXT_FIELDS: [&str; 4] = ["x", "y", "text", "cell"];
const SHIFT_FIELDS: [&str; 5] = ["x", "y", "width", "height", "rise"];

fn glkote_error(message: String) -> VoxamError {
    VoxamError::GlkOte(message)
}

/// A text run's ink: (fg, bg) CSS colours, each None where the
/// display's own theme rules.
pub type Ink = (Option<String>, Option<String>);

/// One styled text run: style name, link value, text -- and
/// optionally the colour dialect's ink.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub style: String,
    pub link: i64,
    pub text: String,
    pub ink: Option<Ink>,
}

impl TextRun {
    pub fn new(style: &str, link: i64, text: &str) -> Self {
        Self {
            style: style.to_string(),
            link,
            text: text.to_string(),
            ink: None,
        }
    }

    pub fn inked(style: &str, link: i64, text: &str, ink: Ink) -> Self {
        Self {
            style: style.to_string(),
            link,
            text: text.to_string(),
            ink: Some(ink),
        }
    }
}

/// One element of a buffer window's feed: a styled run, the flow
/// break that moves the next paragraph below the margin images,
/// or a ready-made special span -- a picture set into the flow.
#[derive(Debug, Clone)]
pub enum Run {
    Text(TextRun),
    Flowbreak,
    Special(Object),
}

impl Run {
    pub fn text(style: &str, link: i64, text: &str) -> Self {
        Run::Text(TextRun::new(style, link, text))
    }
}

/// The dress a declared window may wear beyond its box: a grid's
/// columns and rows, a graphics window's drawable size, the
/// colour dialect's paper, and the stage dialect's scaled flag.
#[derive(Debug, Clone, Default)]
pub struct WindowSpec {
    pub gridsize: Option<(i64, i64)>,
    pub graphsize: Option<(i64, i64)>,
    pub bg: Option<String>,
    pub scaled: bool,
}

/// The dress a line-input request may wear: an initial text,
/// terminator keys, a cursor, and the stage dialect's cell and
/// ink.
#[derive(Debug, Clone, Default)]
pub struct LineSpec {
    pub initial: String,
    pub terminators: Vec<String>,
    pub cursor: Option<(i64, i64)>,
    pub cell: Option<(i64, i64)>,
    pub ink: Option<String>,
    pub hyperlink: bool,
    pub mouse: bool,
}

/// The timer field's three states: silent, cancelled, or set.
enum TimerField {
    Unset,
    Cancel,
    Set(i64),
}

/// The display's picture of the session, update by update.
///
/// Fed each cycle -- every visible window declared, content and
/// requests alongside -- and asked for the update stanza, a Page
/// sends only what changed. The distinction is load-bearing: an
/// absent windows or input array means "unchanged", while an
/// empty one closes every window or cancels every field, and an
/// update where nothing changed at all is the pass stanza, its
/// generation unbumped (GlkOte: Output: Updating the Display;
/// GlkOte: The Generation Number).
#[derive(Default)]
pub struct Page {
    generation: i64,
    // None rather than an empty list: the first update always
    // carries the full windows array, even the empty one that
    // closes nothing.
    shown: Option<Vec<Object>>,
    rows: HashMap<i64, Vec<Vec<Object>>>,
    open: HashMap<i64, bool>,
    flowing: HashMap<i64, bool>,
    asked: HashMap<i64, Object>,
    typed: HashMap<i64, String>,
    timer_shown: i64,
    retired: HashSet<i64>,
    kept: HashMap<i64, Vec<Object>>,

    // The cycle in progress, cleared by every update. Declared
    // keeps its insertion order: the windows and content arrays
    // travel in declaration order.
    declared: Vec<(i64, Object)>,
    texts: HashMap<i64, Object>,
    changed: HashMap<i64, Vec<Object>>,
    draws: HashMap<i64, Vec<Object>>,
    requests: Vec<(i64, Object)>,
    timer_request: Option<(i64, bool)>,
    prompt: Option<Object>,
    sounds: Vec<Object>,
}

impl Page {
    /// Open before the first update, at generation zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// The generation of the last update sent -- zero before the
    /// first, matching the display's own init event. The event
    /// half of the protocol reads it to judge inbound event
    /// generations.
    pub fn generation(&self) -> i64 {
        self.generation
    }

    fn declared_entry(&self, ident: i64) -> Option<&Object> {
        self.declared
            .iter()
            .find(|(held, _)| *held == ident)
            .map(|(_, entry)| entry)
    }

    // -- feeding the cycle (GlkOte: Output: Updating the Display) --------

    /// Declare a window that stands visible this cycle.
    ///
    /// Every visible window is declared every cycle; one left out
    /// is closed, its id retired for good -- the protocol forbids
    /// reuse (GlkOte: The Windows Update Array). The box arrives
    /// as (left, top, right, bottom) in pixels, the shape the
    /// window models already keep; a grid names its columns and
    /// rows, a graphics window its drawable size. A bg is the
    /// dialect's own word: the window's paper as a CSS colour,
    /// absent when the display's own theme is the paper. Scaled
    /// is the stage dialect's word, for a graphics window alone:
    /// the drawable size is a logical space -- §8.8's own units
    /// -- and the display magnifies it to fill the box, rather
    /// than showing it pixel for pixel.
    pub fn window(
        &mut self,
        ident: i64,
        kind: &str,
        rock: i64,
        box_: (i64, i64, i64, i64),
        spec: WindowSpec,
    ) -> Result<(), VoxamError> {
        if !KINDS.contains(&kind) {
            return Err(glkote_error(format!(
                "a window cannot be a {kind:?} (GlkOte: The Windows Update Array)"
            )));
        }

        if self.retired.contains(&ident) {
            return Err(glkote_error(format!(
                "window id {ident} was closed and may never return \
                 (GlkOte: The Windows Update Array)"
            )));
        }

        if self.declared_entry(ident).is_some() {
            return Err(glkote_error(format!(
                "window {ident} declared twice in one cycle"
            )));
        }

        let (left, top, right, bottom) = box_;
        let mut entry = Object::new();

        entry.set("id", ident);
        entry.set("type", kind);
        entry.set("rock", rock);
        entry.set("left", left);
        entry.set("top", top);
        entry.set("width", right - left);
        entry.set("height", bottom - top);

        if spec.gridsize.is_none() != (kind != "grid") {
            return Err(glkote_error(
                "a grid names its columns and rows, and only a grid does".into(),
            ));
        }

        if spec.graphsize.is_none() != (kind != "graphics") {
            return Err(glkote_error(
                "a graphics window names its drawable size, and only one does".into(),
            ));
        }

        if let Some((columns, rows)) = spec.gridsize {
            entry.set("gridwidth", columns);
            entry.set("gridheight", rows);

            self.resized(ident, &entry);
        }

        if let Some((width, height)) = spec.graphsize {
            entry.set("graphwidth", width);
            entry.set("graphheight", height);
        }

        if spec.scaled {
            if kind != "graphics" {
                return Err(glkote_error(
                    "only a graphics window draws in a scaled logical space".into(),
                ));
            }

            entry.set("scaled", true);
        }

        if let Some(bg) = spec.bg {
            entry.set("bg", bg);
        }

        self.declared.push((ident, entry));

        Ok(())
    }

    /// Drop a grid's row cache when its cell grid changed.
    ///
    /// What the display does with retained lines across a resize
    /// is unspecified, so every row is resent -- idempotent and
    /// always correct.
    fn resized(&mut self, ident: i64, entry: &Object) {
        for held in self.shown.as_deref().unwrap_or(&[]) {
            if held.get("id") == Some(&Value::Int(ident))
                && (held.get("gridwidth"), held.get("gridheight"))
                    != (entry.get("gridwidth"), entry.get("gridheight"))
            {
                self.rows.remove(&ident);
            }
        }
    }

    /// Feed a buffer window's new text, one cycle's worth.
    ///
    /// Runs carry newlines embedded; the split into paragraph
    /// entries happens here, because the append flag is state the
    /// display remembers between updates: text after the last
    /// newline leaves its paragraph open, and the next cycle's
    /// first entry continues it (GlkOte: Buffer Window Updates).
    /// A clear closes the open paragraph and rides the entry; a
    /// flow break in the stream closes it too, and the paragraph
    /// after it is moved below the margin images. A special span
    /// joins the open paragraph as it stands (GlkOte: The Line
    /// Data Array).
    pub fn buffer(&mut self, ident: i64, runs: &[Run], clear: bool) -> Result<(), VoxamError> {
        if self.texts.contains_key(&ident) {
            return Err(glkote_error(format!(
                "window {ident} was fed text twice in one cycle"
            )));
        }

        let mut entry = Object::new();

        entry.set("id", ident);

        if clear {
            entry.set("clear", true);
            self.open.insert(ident, false);
        }

        let (segments, breaks) = self.segmented(runs)?;
        let entries = self.paragraphs(ident, segments, &breaks);

        if !entries.is_empty() {
            entry.set(
                "text",
                Value::List(entries.into_iter().map(Value::Object).collect()),
            );
        }

        // An empty helping is the same as none at all: only a
        // substantive entry is kept, since an empty content array
        // equals an omitted one (GlkOte: The Content Update Array).
        if entry.contains("text") || entry.contains("clear") {
            self.texts.insert(ident, entry);
        }

        Ok(())
    }

    /// Split a run stream into paragraphs at newlines and breaks.
    ///
    /// Returns the paragraphs as span lists, and the indices of
    /// those that follow a flow break.
    fn segmented(&self, runs: &[Run]) -> Result<(Vec<Vec<Object>>, HashSet<usize>), VoxamError> {
        let mut segments: Vec<Vec<Object>> = vec![Vec::new()];
        let mut breaks: HashSet<usize> = HashSet::new();

        for run in runs {
            match run {
                Run::Flowbreak => {
                    // A break right after a newline flags the
                    // fresh paragraph rather than minting a blank
                    // one.
                    if !segments.last().expect("one segment stands").is_empty() {
                        segments.push(Vec::new());
                    }

                    breaks.insert(segments.len() - 1);
                }
                Run::Special(span) => {
                    // A ready-made special span joins the
                    // paragraph where it stands, copied so the
                    // caller's object stays its own.
                    segments
                        .last_mut()
                        .expect("one segment stands")
                        .push(span.clone());
                }
                Run::Text(held) => {
                    let pieces: Vec<&str> = held.text.split('\n').collect();

                    spanned(
                        segments.last_mut().expect("one segment stands"),
                        &held.style,
                        held.link,
                        pieces[0],
                        held.ink.as_ref(),
                    )?;

                    for piece in &pieces[1..] {
                        segments.push(Vec::new());
                        spanned(
                            segments.last_mut().expect("just pushed"),
                            &held.style,
                            held.link,
                            piece,
                            held.ink.as_ref(),
                        )?;
                    }
                }
            }
        }

        Ok((segments, breaks))
    }

    /// Turn paragraphs into the text entries of a content update.
    ///
    /// The rules of the seams: a trailing empty paragraph is the
    /// stream ending at a line boundary and emits nothing; a
    /// leading empty one on an open paragraph only closes it; an
    /// empty one anywhere else is a blank line, the empty object
    /// (GlkOte: Buffer Window Updates).
    fn paragraphs(
        &mut self,
        ident: i64,
        segments: Vec<Vec<Object>>,
        breaks: &HashSet<usize>,
    ) -> Vec<Object> {
        let mut opened = self.open.get(&ident).copied().unwrap_or(false);
        let flowing = self.flowing.remove(&ident).unwrap_or(false);
        let mut entries: Vec<Object> = Vec::new();
        let count = segments.len();

        for (index, spans) in segments.into_iter().enumerate() {
            let flagged = breaks.contains(&index) || (index == 0 && flowing);

            if index == count - 1 && spans.is_empty() {
                // The stream ended at a boundary; a flow break
                // right at the end waits for the next helping.
                if flagged {
                    self.flowing.insert(ident, true);
                }

                opened = false;

                continue;
            }

            if index == 0 && spans.is_empty() && opened {
                opened = false;

                continue;
            }

            let mut piece = Object::new();

            if index == 0 && !spans.is_empty() && opened {
                piece.set("append", true);
            }

            if flagged {
                piece.set("flowbreak", true);
            }

            if !spans.is_empty() {
                piece.set(
                    "content",
                    Value::List(spans.into_iter().map(Value::Object).collect()),
                );
            }

            entries.push(piece);
            opened = index == count - 1;
        }

        self.open.insert(ident, opened);

        entries
    }

    /// Feed a grid window's whole face; only changed rows travel.
    ///
    /// Rows arrive as coalesced runs. Trailing plain whitespace
    /// is stripped before comparing, because the display pads
    /// short lines with it anyway -- so a blank row equals an
    /// empty line, a fresh grid sends only what shows, and a
    /// cleared grid needs no flag at all (GlkOte: Grid Window
    /// Updates; GlkOte: The Line Data Array).
    pub fn grid(&mut self, ident: i64, rows: &[Vec<TextRun>]) -> Result<(), VoxamError> {
        if self.changed.contains_key(&ident) {
            return Err(glkote_error(format!(
                "window {ident} was fed rows twice in one cycle"
            )));
        }

        let held = self.rows.get(&ident).cloned().unwrap_or_default();
        let mut normalized: Vec<Vec<Object>> = Vec::new();
        let mut updates: Vec<Object> = Vec::new();

        for (index, row) in rows.iter().enumerate() {
            let mut spans: Vec<Object> = Vec::new();

            for run in row {
                spanned(
                    &mut spans,
                    &run.style,
                    run.link,
                    &run.text,
                    run.ink.as_ref(),
                )?;
            }

            let spans = trimmed(spans);
            let empty: Vec<Object> = Vec::new();
            let before = held.get(index).unwrap_or(&empty);

            if spans != *before {
                let mut line = Object::new();

                line.set("line", index as i64);

                if !spans.is_empty() {
                    line.set(
                        "content",
                        Value::List(spans.iter().cloned().map(Value::Object).collect()),
                    );
                }

                updates.push(line);
            }

            normalized.push(spans);
        }

        self.rows.insert(ident, normalized);
        self.changed.insert(ident, updates);

        Ok(())
    }

    /// Feed drawing operations for a graphics window.
    ///
    /// Operations accumulate across a cycle -- a turn's fills and
    /// images arrive as they happen -- and travel in order
    /// (GlkOte: Graphics Window Updates). Text and shift are the
    /// stage dialect's own words: a text op places a string of
    /// dressed cells at a unit position, a shift op slides a
    /// rectangle's pixels vertically -- GlkOte never grew either,
    /// but both ends of this wire are ours.
    pub fn draw(&mut self, ident: i64, ops: Vec<Object>) -> Result<(), VoxamError> {
        for op in &ops {
            let special = op.get("special").and_then(Value::as_str);

            if !special.is_some_and(|name| SPECIALS.contains(&name)) {
                return Err(glkote_error(format!(
                    "no drawing operation is named {special:?} \
                     (GlkOte: Graphics Window Updates)"
                )));
            }

            let special = special.expect("checked named");
            let corners = RECT.iter().filter(|side| op.contains(side)).count();

            if special == "fill" && corners != 0 && corners != RECT.len() {
                // "All four of these fields must be specified if
                // any is" (GlkOte: Graphics Window Updates).
                return Err(glkote_error(
                    "a fill names its whole rectangle or none of it".into(),
                ));
            }

            if special == "text" && TEXT_FIELDS.iter().any(|field| !op.contains(field)) {
                return Err(glkote_error(
                    "a text op places its string in cells: x, y, text, cell".into(),
                ));
            }

            if special == "shift" && SHIFT_FIELDS.iter().any(|field| !op.contains(field)) {
                return Err(glkote_error(
                    "a shift op slides a whole rectangle by a rise".into(),
                ));
            }
        }

        self.draws.entry(ident).or_default().extend(ops);

        Ok(())
    }

    /// Ask for a line of input in a window.
    ///
    /// A cell is the stage dialect's word: the editor's cell size
    /// in the canvas's own logical units, so the display can
    /// place and dress the field at the game's cursor. An ink is
    /// the editor's own text colour -- without one the field
    /// writes in the browser's default, which on a dark stage is
    /// invisible ink. A stage's line request names its cursor and
    /// its cell.
    pub fn line_input(
        &mut self,
        ident: i64,
        maxlen: i64,
        spec: LineSpec,
    ) -> Result<(), VoxamError> {
        let mut entry = Object::new();

        entry.set("id", ident);
        entry.set("type", "line");
        entry.set("maxlen", maxlen);

        if !spec.initial.is_empty() {
            entry.set("initial", spec.initial);
        }

        if let Some((width, height)) = spec.cell {
            entry.set(
                "cell",
                Value::List(vec![Value::Int(width), Value::Int(height)]),
            );
        }

        if let Some(ink) = spec.ink {
            entry.set("ink", ink);
        }

        if !spec.terminators.is_empty() {
            for name in &spec.terminators {
                if !terminator(name) {
                    return Err(glkote_error(format!(
                        "no terminator key is named {name:?} \
                         (GlkOte: The Input Update Array)"
                    )));
                }
            }

            entry.set(
                "terminators",
                Value::List(
                    spec.terminators
                        .iter()
                        .map(|name| Value::Str(name.clone()))
                        .collect(),
                ),
            );
        }

        self.requested(entry, spec.cursor, spec.hyperlink, spec.mouse)
    }

    /// Ask for a single keystroke in a window.
    pub fn char_input(
        &mut self,
        ident: i64,
        cursor: Option<(i64, i64)>,
        hyperlink: bool,
        mouse: bool,
    ) -> Result<(), VoxamError> {
        let mut entry = Object::new();

        entry.set("id", ident);
        entry.set("type", "char");

        self.requested(entry, cursor, hyperlink, mouse)
    }

    /// Ask for clicks or link selections alone, no typing.
    ///
    /// With neither flag raised this asks for nothing, which the
    /// protocol spells by leaving the window out entirely
    /// (GlkOte: The Input Update Array).
    pub fn passive_input(
        &mut self,
        ident: i64,
        hyperlink: bool,
        mouse: bool,
    ) -> Result<(), VoxamError> {
        if hyperlink || mouse {
            let mut entry = Object::new();

            entry.set("id", ident);

            return self.requested(entry, None, hyperlink, mouse);
        }

        Ok(())
    }

    /// File one window's input request for the cycle.
    fn requested(
        &mut self,
        mut entry: Object,
        cursor: Option<(i64, i64)>,
        hyperlink: bool,
        mouse: bool,
    ) -> Result<(), VoxamError> {
        let ident = entry.get("id").and_then(Value::as_int).expect("an id");

        if self.requests.iter().any(|(held, _)| *held == ident) {
            return Err(glkote_error(format!(
                "window {ident} asked for input twice in one cycle"
            )));
        }

        if let Some((x, y)) = cursor {
            entry.set("xpos", x);
            entry.set("ypos", y);
        }

        if hyperlink {
            entry.set("hyperlink", true);
        }

        if mouse {
            entry.set("mouse", true);
        }

        self.requests.push((ident, entry));

        Ok(())
    }

    /// Ask the player for a file, through the display's own ask.
    ///
    /// The update carries it as special input, and the display
    /// disables the game until the answer comes back (GlkOte:
    /// Special Input Requests).
    pub fn prompt(&mut self, filemode: &str, filetype: &str) -> Result<(), VoxamError> {
        if self.prompt.is_some() {
            return Err(glkote_error("one file may be asked for per cycle".into()));
        }

        if !FILE_MODES.contains(&filemode) || !FILE_KINDS.contains(&filetype) {
            return Err(glkote_error(format!(
                "no file prompt asks {filemode:?} of a {filetype:?} \
                 (GlkOte: Special Input Requests)"
            )));
        }

        let mut ask = Object::new();

        ask.set("type", "fileref_prompt");
        ask.set("filemode", filemode);
        ask.set("filetype", filetype);

        self.prompt = Some(ask);

        Ok(())
    }

    /// Feed sound channel operations, one cycle's worth.
    ///
    /// The dialect is VΘXΔM's own: GlkOte never grew a sound
    /// vocabulary, but both ends of this wire are ours, so the
    /// update carries channel ops -- play, stop, volume -- in the
    /// order they happened, each play with its sound inlined
    /// whole as a data: url. A display that never learned the
    /// word simply ignores the field, which is the conforming
    /// quiet every sound game ships ready to accept.
    pub fn sounds(&mut self, ops: Vec<Object>) {
        self.sounds.extend(ops);
    }

    /// Note the timer cadence in milliseconds, zero for none.
    ///
    /// Sent only when it changes -- resending even the same value
    /// restarts the display's clock, so a caller that means to
    /// restart says so (GlkOte: The Timer Update).
    pub fn timer(&mut self, interval: i64, restart: bool) {
        self.timer_request = Some((interval, restart));
    }

    /// Note what the player has typed so far, window by window.
    ///
    /// Replaced whole each time: every event that can carry
    /// partial input carries the complete current picture, and a
    /// finished line's window is absent from its own event
    /// (GlkOte: Partial Input). A field that must be recreated --
    /// content reached its window, or its dress changed -- takes
    /// the noted text as its initial, so an interruption never
    /// eats a half-typed command; a carried field is left alone,
    /// since the display preserves its editing state itself.
    pub fn typed(&mut self, partials: HashMap<i64, String>) {
        self.typed = partials;
    }

    // -- the update itself -----------------------------------------------

    /// Assemble the cycle into an update stanza, or the pass.
    ///
    /// A refresh assembles the whole picture instead: the display
    /// lost its state, so every window travels, buffers replay
    /// their kept scrollback behind a clear, grids resend every
    /// row, standing input fields are stamped anew, and a running
    /// timer is renamed -- an ordinary update in form, complete
    /// in content (GlkOte: the refresh input event).
    pub fn update(&mut self, exit: bool, refresh: bool) -> Result<Object, VoxamError> {
        self.validated()?;

        let windows: Vec<Object> = self
            .declared
            .iter()
            .map(|(_, entry)| entry.clone())
            .collect();
        let windows_changed = self.shown.as_ref() != Some(&windows);
        let mut content = self.content();

        self.retold(&content);

        if refresh {
            content = self.retold_whole();
        }

        let conflicted: HashSet<i64> = content
            .iter()
            .filter_map(|entry| entry.get("id").and_then(Value::as_int))
            .collect();
        let input_changed = self.input_changed(&conflicted);
        let timer_field = self.timer_field();

        let changed = windows_changed
            || !content.is_empty()
            || input_changed
            || !matches!(timer_field, TimerField::Unset)
            || self.prompt.is_some()
            || !self.sounds.is_empty()
            || refresh
            || exit;

        if !changed {
            self.rested();

            let mut stanza = Object::new();

            stanza.set("type", "pass");

            return Ok(stanza);
        }

        let generation = self.generation + 1;
        let mut stanza = Object::new();

        stanza.set("type", "update");
        stanza.set("gen", generation);

        if windows_changed || refresh {
            stanza.set(
                "windows",
                Value::List(windows.iter().cloned().map(Value::Object).collect()),
            );
        }

        if !content.is_empty() {
            stanza.set(
                "content",
                Value::List(content.into_iter().map(Value::Object).collect()),
            );
        }

        if input_changed || refresh {
            stanza.set(
                "input",
                Value::List(
                    self.roster(generation, &conflicted)
                        .into_iter()
                        .map(Value::Object)
                        .collect(),
                ),
            );
        }

        match timer_field {
            TimerField::Set(interval) => {
                stanza.set("timer", interval);
                self.timer_shown = interval;
            }
            TimerField::Cancel => {
                stanza.set("timer", Value::Null);
                self.timer_shown = 0;
            }
            TimerField::Unset => {
                if refresh && self.timer_shown != 0 {
                    stanza.set("timer", self.timer_shown);
                }
            }
        }

        if let Some(ask) = &self.prompt {
            stanza.set("specialinput", ask.clone());
        }

        if !self.sounds.is_empty() {
            stanza.set(
                "sounds",
                Value::List(
                    std::mem::take(&mut self.sounds)
                        .into_iter()
                        .map(Value::Object)
                        .collect(),
                ),
            );
        }

        if exit {
            stanza.set("exit", true);
        }

        self.buried();

        self.generation = generation;
        self.shown = Some(windows);

        self.rested();

        Ok(stanza)
    }

    /// Refuse a cycle whose pieces contradict each other.
    fn validated(&self) -> Result<(), VoxamError> {
        let feeds: [(Vec<i64>, &str, &str); 3] = [
            (self.texts.keys().copied().collect(), "buffer", "text"),
            (self.changed.keys().copied().collect(), "grid", "rows"),
            (self.draws.keys().copied().collect(), "graphics", "drawing"),
        ];

        for (fed, wanted, what) in feeds {
            for ident in fed {
                let Some(held) = self.declared_entry(ident) else {
                    return Err(glkote_error(format!(
                        "{what} arrived for window {ident}, never declared"
                    )));
                };

                if held.get("type").and_then(Value::as_str) != Some(wanted) {
                    return Err(glkote_error(format!(
                        "{what} arrived for window {ident}, not a {wanted}"
                    )));
                }
            }
        }

        for (ident, entry) in &self.requests {
            let Some(held) = self.declared_entry(*ident) else {
                return Err(glkote_error(format!(
                    "input was asked of window {ident}, never declared"
                )));
            };
            let kind = held.get("type").and_then(Value::as_str).expect("a kind");

            if entry.get("mouse").is_some_and(Value::is_true) && kind == "buffer" {
                // "Buffer windows do not support mouse-click
                // input" (GlkOte: The Input Update Array).
                return Err(glkote_error(format!(
                    "window {ident} is a buffer, and a buffer takes no clicks"
                )));
            }

            if kind == "grid" && entry.contains("type") && !entry.contains("xpos") {
                return Err(glkote_error(format!(
                    "grid window {ident} takes input at a cursor, and none came"
                )));
            }

            // The stage dialect: a canvas's editor is placed and
            // sized by the game, or it cannot be drawn.
            if kind == "graphics"
                && entry.get("type").and_then(Value::as_str) == Some("line")
                && (!entry.contains("xpos") || !entry.contains("cell"))
            {
                return Err(glkote_error(format!(
                    "graphics window {ident} takes its editor at a placed cell, \
                     and none came"
                )));
            }

            if (entry.contains("cell") || entry.contains("ink")) && kind != "graphics" {
                return Err(glkote_error(format!(
                    "window {ident} is no stage; only a canvas's editor has a cell"
                )));
            }
        }

        Ok(())
    }

    /// The content array: every window with something to show.
    fn content(&self) -> Vec<Object> {
        let mut content: Vec<Object> = Vec::new();

        for (ident, _) in &self.declared {
            if let Some(entry) = self.texts.get(ident) {
                content.push(entry.clone());
            } else if self
                .changed
                .get(ident)
                .is_some_and(|lines| !lines.is_empty())
            {
                let mut entry = Object::new();

                entry.set("id", *ident);
                entry.set(
                    "lines",
                    Value::List(
                        self.changed[ident]
                            .iter()
                            .cloned()
                            .map(Value::Object)
                            .collect(),
                    ),
                );
                content.push(entry);
            } else if self.draws.get(ident).is_some_and(|ops| !ops.is_empty()) {
                let mut entry = Object::new();

                entry.set("id", *ident);
                entry.set(
                    "draw",
                    Value::List(
                        self.draws[ident]
                            .iter()
                            .cloned()
                            .map(Value::Object)
                            .collect(),
                    ),
                );
                content.push(entry);
            }
        }

        content
    }

    /// Whether the input array must travel this update.
    ///
    /// The array is sent when a field was posted or cancelled --
    /// which is exactly when the roster differs from what the
    /// display holds -- and when content reached a window whose
    /// field would otherwise be carried, since a carried field
    /// forbids content and must be recreated at the new
    /// generation (GlkOte: The Input Update Array).
    fn input_changed(&self, conflicted: &HashSet<i64>) -> bool {
        let requested: HashSet<i64> = self.requests.iter().map(|(ident, _)| *ident).collect();
        let held: HashSet<i64> = self.asked.keys().copied().collect();

        if requested != held {
            return true;
        }

        self.requests.iter().any(|(ident, entry)| {
            let memo = &self.asked[ident];

            memo.without("gen") != *entry || (memo.contains("gen") && conflicted.contains(ident))
        })
    }

    /// The input array, each field wearing its generation.
    ///
    /// A field carried unchanged keeps the generation it was
    /// created at; one posted, altered, or standing in a window
    /// that received content is stamped anew -- the protocol's
    /// "new version of the input field at the current
    /// generation", which is also what makes echoing a line and
    /// asking again in one update legal. A cancel-and-reask with
    /// identical parameters and no content is indistinguishable
    /// from a carried field here, and carries.
    fn roster(&mut self, generation: i64, conflicted: &HashSet<i64>) -> Vec<Object> {
        let mut roster: Vec<Object> = Vec::new();
        let mut asked: HashMap<i64, Object> = HashMap::new();

        for (ident, candidate) in &self.requests {
            let mut entry = candidate.clone();
            let mut carried = false;

            if entry.contains("type") {
                let held = self.asked.get(ident);

                carried = held.is_some_and(|held| {
                    held.contains("gen")
                        && held.without("gen") == *candidate
                        && !conflicted.contains(ident)
                });
                let stamped = if carried {
                    held.expect("carried means held")
                        .get("gen")
                        .and_then(Value::as_int)
                        .expect("a generation")
                } else {
                    generation
                };

                entry.set("gen", stamped);
            }

            // The memo keeps the game's own dress, so a steady
            // request stays carried; only the spoken entry takes
            // the player's half-typed text as its initial, and
            // only when the field is being made anew (GlkOte:
            // Partial Input).
            asked.insert(*ident, entry.clone());

            let mut spoken = entry;

            if !carried
                && spoken.get("type").and_then(Value::as_str) == Some("line")
                && let Some(typed) = self.typed.get(ident)
                && !typed.is_empty()
            {
                spoken.set("initial", typed.clone());
            }

            roster.push(spoken);
        }

        roster.sort_by_key(|entry| entry.get("id").and_then(Value::as_int).unwrap_or(0));

        self.asked = asked;

        roster
    }

    /// The timer field to send, or the unset sentinel.
    ///
    /// A change travels, a cancel travels as null, and a steady
    /// cadence stays silent -- resending would restart the
    /// display's clock (GlkOte: The Timer Update).
    fn timer_field(&self) -> TimerField {
        let Some((interval, restart)) = self.timer_request else {
            return TimerField::Unset;
        };

        if interval > 0 && (interval != self.timer_shown || restart) {
            return TimerField::Set(interval);
        }

        if interval == 0 && self.timer_shown != 0 {
            return TimerField::Cancel;
        }

        TimerField::Unset
    }

    /// Keep each buffer's sent paragraphs for a refresh's
    /// re-telling.
    ///
    /// Bounded at KEPT_PARAGRAPHS: a display that reconnects gets
    /// the recent scrollback, not the whole session -- and a
    /// clear starts the keeping over, exactly as it starts the
    /// display over.
    fn retold(&mut self, content: &[Object]) {
        for entry in content {
            let ident = entry.get("id").and_then(Value::as_int).expect("an id");

            if !self.texts.contains_key(&ident) {
                continue;
            }

            let held = self.kept.entry(ident).or_default();

            if entry.get("clear").is_some_and(Value::is_true) {
                held.clear();
            }

            if let Some(Value::List(pieces)) = entry.get("text") {
                held.extend(pieces.iter().filter_map(Value::as_object).cloned());
            }

            if held.len() > KEPT_PARAGRAPHS {
                held.drain(..held.len() - KEPT_PARAGRAPHS);
            }
        }
    }

    /// The complete picture, for a display that lost its own.
    ///
    /// Buffers replay their kept scrollback behind a clear --
    /// pictures and covers ride along, since their data: urls
    /// were kept with the text -- grids resend every row, the
    /// blank ones as bare line numbers, and canvases carry
    /// whatever this cycle's re-feed drew, because pixels are the
    /// game's to repaint (GlkOte: Redraw Events).
    fn retold_whole(&self) -> Vec<Object> {
        let mut content: Vec<Object> = Vec::new();

        for (ident, held) in &self.declared {
            let kind = held.get("type").and_then(Value::as_str).expect("a kind");

            if kind == "buffer" {
                let mut entry = Object::new();

                entry.set("id", *ident);
                entry.set("clear", true);

                if let Some(kept) = self.kept.get(ident)
                    && !kept.is_empty()
                {
                    entry.set(
                        "text",
                        Value::List(kept.iter().cloned().map(Value::Object).collect()),
                    );
                }

                content.push(entry);
            } else if kind == "grid" {
                let mut entry = Object::new();
                let rows = self.rows.get(ident).cloned().unwrap_or_default();

                entry.set("id", *ident);
                entry.set(
                    "lines",
                    Value::List(
                        rows.into_iter()
                            .enumerate()
                            .map(|(index, spans)| {
                                let mut line = Object::new();

                                line.set("line", index as i64);

                                if !spans.is_empty() {
                                    line.set(
                                        "content",
                                        Value::List(spans.into_iter().map(Value::Object).collect()),
                                    );
                                }

                                Value::Object(line)
                            })
                            .collect(),
                    ),
                );
                content.push(entry);
            } else if self.draws.get(ident).is_some_and(|ops| !ops.is_empty()) {
                let mut entry = Object::new();

                entry.set("id", *ident);
                entry.set(
                    "draw",
                    Value::List(
                        self.draws[ident]
                            .iter()
                            .cloned()
                            .map(Value::Object)
                            .collect(),
                    ),
                );
                content.push(entry);
            }
        }

        content
    }

    /// Retire the windows this cycle no longer declares.
    fn buried(&mut self) {
        let previously: HashSet<i64> = self
            .shown
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter_map(|held| held.get("id").and_then(Value::as_int))
            .collect();
        let standing: HashSet<i64> = self.declared.iter().map(|(ident, _)| *ident).collect();

        for ident in previously.difference(&standing) {
            self.rows.remove(ident);
            self.open.remove(ident);
            self.flowing.remove(ident);
            self.asked.remove(ident);
            self.typed.remove(ident);
            self.kept.remove(ident);
            self.retired.insert(*ident);
        }
    }

    /// Clear the cycle, ready for the next round of feeding.
    fn rested(&mut self) {
        self.declared.clear();
        self.texts.clear();
        self.changed.clear();
        self.draws.clear();
        self.requests.clear();
        self.timer_request = None;
        self.prompt = None;
    }
}

/// Add one piece of styled text to a paragraph's spans.
///
/// Adjacent pieces wearing the same dress coalesce -- two machine
/// styles may share one protocol name -- and the hyperlink key
/// appears only on a real link, the fg and bg of the colour
/// dialect only on real ink (GlkOte: The Line Data Array).
fn spanned(
    spans: &mut Vec<Object>,
    style: &str,
    link: i64,
    text: &str,
    ink: Option<&Ink>,
) -> Result<(), VoxamError> {
    if !STYLES.contains(&style) {
        return Err(glkote_error(format!(
            "no style is named {style:?} (GlkOte: The Line Data Array)"
        )));
    }

    if text.is_empty() {
        return Ok(());
    }

    let (fg, bg) = ink.cloned().unwrap_or((None, None));

    // A special span has no style name, so text after a placed
    // picture starts its own span rather than coalescing.
    if let Some(last) = spans.last_mut()
        && last.get("style").and_then(Value::as_str) == Some(style)
        && last.get("hyperlink").and_then(Value::as_int).unwrap_or(0) == link
        && last.get("fg").and_then(Value::as_str) == fg.as_deref()
        && last.get("bg").and_then(Value::as_str) == bg.as_deref()
    {
        let held = last.get("text").and_then(Value::as_str).expect("a text");

        last.set("text", format!("{held}{text}"));

        return Ok(());
    }

    let mut span = Object::new();

    span.set("style", style);
    span.set("text", text);

    if link != 0 {
        span.set("hyperlink", link);
    }

    if let Some(fg) = fg {
        span.set("fg", fg);
    }

    if let Some(bg) = bg {
        span.set("bg", bg);
    }

    spans.push(span);

    Ok(())
}

/// Strip a grid row's trailing plain whitespace.
///
/// The display pads short lines with exactly this (GlkOte: The
/// Line Data Array), so stripping it first makes equal rows
/// compare equal however the padding fell.
fn trimmed(mut spans: Vec<Object>) -> Vec<Object> {
    while let Some(last) = spans.last() {
        if last.get("style").and_then(Value::as_str) != Some("normal") || last.contains("hyperlink")
        {
            break;
        }

        let held = last.get("text").and_then(Value::as_str).expect("a text");
        let text = held.trim_end_matches(' ');

        if !text.is_empty() {
            if text != held {
                let text = text.to_string();

                spans.last_mut().expect("checked last").set("text", text);
            }

            break;
        }

        spans.pop();
    }

    spans
}

/// One window kind's cell measures, with the spec's fallback
/// chain.
///
/// (width, height, margin x, margin y): a partial metrics object
/// falls back from the qualified name to the generic to the
/// default, the rules RemGlk reads by (GlkOte: The Metrics
/// Object). Shared because every machine measures the same way.
pub fn measured(metrics: &Object, prefix: &str) -> (f64, f64, f64, f64) {
    let field = |name: &str, fallbacks: &[&str], default: f64| -> f64 {
        let qualified = format!("{prefix}{name}");

        for key in std::iter::once(qualified.as_str()).chain(fallbacks.iter().copied()) {
            if let Some(value) = metrics.get(key).and_then(Value::as_float) {
                return value;
            }
        }

        default
    };

    (
        field("charwidth", &["charwidth"], 1.0),
        field("charheight", &["charheight"], 1.0),
        field("marginx", &["marginx", "margin"], 0.0),
        field("marginy", &["marginy", "margin"], 0.0),
    )
}

/// The bibliography fields the iFiction card shows -- a stand-in
/// for the Babel record until that work lands to feed it.
#[derive(Debug, Clone, Default)]
pub struct Card {
    pub title: Option<String>,
    pub headline: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
}

/// The iFiction card as (style name, text) runs.
///
/// The four fields WinFrotz's own little window shows: the title
/// in the header dress, the headline and author emphasized, then
/// the description's paragraphs separated by blank lines -- each
/// <br/>-broken line its own paragraph -- and a closing blank
/// line before the story begins. A record with none of them makes
/// no card at all (Babel: The iFiction format).
pub fn carded(record: &Card) -> Vec<(String, String)> {
    let mut lines: Vec<(String, String)> = Vec::new();

    if let Some(title) = &record.title {
        lines.push(("header".to_string(), format!("{title}\n")));
    }

    if let Some(headline) = &record.headline {
        lines.push(("emphasized".to_string(), format!("{headline}\n")));
    }

    if let Some(author) = &record.author {
        lines.push(("emphasized".to_string(), format!("{author}\n")));
    }

    if let Some(description) = &record.description {
        let paragraphs: Vec<&str> = description
            .split('\n')
            .filter(|held| !held.is_empty())
            .collect();

        lines.push((
            "normal".to_string(),
            format!("\n{}\n", paragraphs.join("\n\n")),
        ));
    }

    if !lines.is_empty() {
        lines.push(("normal".to_string(), "\n".to_string()));
    }

    lines
}

/// An event's partial-input object as ident-keyed text.
///
/// JSON spells the window ids as object keys -- strings -- and
/// anything not shaped like typing is quietly no typing at all
/// (GlkOte: Partial Input).
pub fn partials(partial: Option<&Value>) -> HashMap<i64, String> {
    let mut stashed: HashMap<i64, String> = HashMap::new();

    if let Some(Value::Object(held)) = partial {
        for (key, text) in held.iter() {
            if let (Value::Str(text), Ok(ident)) = (text, key.parse::<i64>()) {
                stashed.insert(ident, text.clone());
            }
        }
    }

    stashed
}

/// The next stanza from the display, or None when it hung up.
///
/// Fails for what is not JSON, and for JSON that is not an
/// object.
pub fn read_stanza(reader: &mut dyn BufRead) -> Result<Option<Object>, VoxamError> {
    let mut line = String::new();

    loop {
        line.clear();

        let read = reader
            .read_line(&mut line)
            .map_err(|error| glkote_error(format!("the display's stream failed: {error}")))?;

        if read == 0 {
            return Ok(None);
        }

        if line.trim().is_empty() {
            continue;
        }

        let parsed = json::loads(&line)?;

        let Value::Object(stanza) = parsed else {
            return Err(glkote_error("a stanza is a JSON object".into()));
        };

        return Ok(Some(stanza));
    }
}

/// One stanza out, compact, on its own line, flushed.
///
/// Flushed every time: an update parked in a pipe's buffer is a
/// display waiting forever.
pub fn write_stanza(writer: &mut dyn Write, stanza: &Object) {
    let _ = writer.write_all(json::dumps(&Value::Object(stanza.clone())).as_bytes());
    let _ = writer.write_all(b"\n");
    let _ = writer.flush();
}

#[cfg(test)]
#[path = "page_tests.rs"]
mod tests;
