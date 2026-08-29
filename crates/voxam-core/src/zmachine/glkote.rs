//! The Z-Machine spoken over the GlkOte protocol, both ways.
//!
//! The same Page the Glulx composer feeds, fed from the §8 screen
//! model: the upper window and the Version 1 to 3 status line
//! travel as the protocol's grid window, read out of the
//! ScreenModel that already knows every splitting and cursor rule,
//! while the lower window's text -- which GlkOte wraps and scrolls
//! itself -- never enters the model at all and accumulates as
//! styled runs instead.
//!
//! The reads ride the suspension seam: the machine stands down at
//! a read, the update carries the ask, and the display's answer is
//! delivered straight to the machine, echoed here first, since the
//! machine never echoes and the display owes the typed line and
//! its newline. The saves stand down the same way: a §15 save or
//! restore asks for its file through the protocol's special input,
//! and the answered path -- or the cancel -- runs the parked
//! rider.
//!
//! The arc_image band hangs here too: a story whose sidecar
//! carries pictures plays them in a graphics window above the
//! whole screen -- the picture inlined as a data: url, the grid
//! and buffer re-based below, the header's rows updated as the
//! contract asks (arc_image: the contract, part A).
//!
//! The rest of the eras' claims live here too, each under the
//! display's own grant: the §10.5.2.1 terminating characters the
//! wire can name, §10.3's clicks on the grid, §9's sounds in the
//! dialect's channel ops, and §8.3's colours as per-span ink with
//! the window's own paper.
//!
//! One reshaping from the reference, in the standing manner: the
//! reference's face holds its machine and pokes it directly, a
//! cycle Rust refuses. Here the face is shared behind a cell --
//! the machine holds one handle as its [`Frontend`], a [`Session`]
//! holds the other beside the machine itself -- and the halves
//! that need both (render, accept) live on the Session. The
//! Version 6 stage face arrives with the stage rung.

use std::cell::{RefCell, RefMut};
use std::io::{BufRead, Write};
use std::rc::Rc;

use crate::babel::ifiction;
use crate::errors::VoxamError;
use crate::frontend::{
    ARC_MODES, ARC_PIXEL_ROWS, ARC_REFERENCE_WIDTH, Frontend, Status, colour_value,
};
use crate::glkote::json::{Object, Value};
use crate::glkote::{
    Ink, LineSpec, Page, Run, TextRun, WindowSpec, carded, measured, partials, read_stanza,
    write_stanza,
};
use crate::glulx::glk::resources::{ImageInfo, Resources, pictured};
use crate::screen::{BOLD, ITALIC};
use crate::screen::{CURRENT_COLOUR, DEFAULT_COLOUR, FIXED_PITCH, REVERSE, ScreenModel, UPPER};
use crate::zmachine::header::STATUS_FLAGS_VERSION;
use crate::zmachine::machine::{
    FULL_VOLUME, Identity, Machine, Purpose, SINGLE_CLICK, Waiting, Wants,
};
use crate::zmachine::story::Story;

/// The verdicts accept hands the serving loop: run the machine
/// on, render the standing picture, or answer the pass stanza.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Advance,
    Stand,
    Pass,
}

/// A named key of a char event, as the §3.8 input character ZSCII
/// spells it; a name outside this table is a key the story cannot
/// hear.
fn zscii_key(name: &str) -> Option<char> {
    match name {
        "up" => Some('\u{81}'),
        "down" => Some('\u{82}'),
        "left" => Some('\u{83}'),
        "right" => Some('\u{84}'),
        // Return spells as the newline ZSCII knows, not a raw
        // carriage return -- CR falls through every char_to_zscii
        // branch and is refused, leaving Enter silently dead at
        // the machine.
        "return" => Some('\n'),
        "delete" => Some('\u{8}'),
        "escape" => Some('\u{1b}'),
        _ => {
            // The twelve function keys: func1 to func12 are §3.8's
            // codes 133 to 144.
            let number: u32 = name.strip_prefix("func")?.parse().ok()?;

            if (1..=12).contains(&number) {
                char::from_u32(132 + number)
            } else {
                None
            }
        }
    }
}

/// A §10.5.2.1 terminating code as the name the wire can say: the
/// twelve function keys alone -- a table's cursor, keypad, and
/// click codes have no terminator names in the protocol's
/// vocabulary, so those stay unoffered here (GlkOte: Input
/// Events).
fn terminator_name(code: u16) -> Option<String> {
    if (133..=144).contains(&code) {
        Some(format!("func{}", code - 132))
    } else {
        None
    }
}

/// The same keys read back: a line event's terminator name as the
/// ZSCII code the read stores; any other name reads as a plain
/// new-line ending.
fn terminator_code(name: &str) -> u16 {
    name.strip_prefix("func")
        .and_then(|number| number.parse::<u16>().ok())
        .filter(|number| (1..=12).contains(number))
        .map_or(0, |number| 132 + number)
}

/// One §15 tenth of a second, in the protocol's milliseconds.
const TENTH_MS: i64 = 100;

/// The events that never carry the player's partial input (GlkOte:
/// Partial Input).
const NO_PARTIAL: [&str; 4] = ["init", "specialresponse", "refresh", "debuginput"];

/// The buffer window's protocol id; the grid's ids are minted
/// fresh at every reopening, since the protocol forbids reuse.
const BUFFER: i64 = 1;

/// The screen model that plays on the §8.8 stage.
pub const STAGE_VERSION: u8 = 6;

fn glkote_error(message: String) -> VoxamError {
    VoxamError::GlkOte(message)
}

/// The Z display at the far end of the protocol.
///
/// Suspends like its Glk twin: never asked for input, its picture
/// gathered whole at render. The upper half of the screen lives in
/// a ScreenModel; the lower half is a stream of styled runs.
pub struct GlkOteFrontend {
    version: u8,
    /// The display's picture of the session, update by update.
    pub page: Page,
    resources: Option<Resources>,
    model: ScreenModel,
    runs: Vec<Run>,
    cleared: bool,
    style: u16,
    size: (i64, i64),
    cell: (f64, f64),
    margins: (f64, f64),
    grid_ident: Option<i64>,
    next_ident: i64,
    // The arc_image band: the hanging (picture, mode), the canvas
    // id it wears -- minted fresh at every reopening -- and
    // whether its drawing still owes the display.
    band: Option<(u16, u16)>,
    band_ident: Option<i64>,
    band_dirty: bool,
    // The sound seam: the cycle's queued channel ops, the number
    // sounding on the wire's one channel, and the once-only flag a
    // natural ending raises for poll_sound.
    sound_ops: Vec<Object>,
    sounding: Option<u16>,
    sound_done: bool,
    speaks_sound: bool,
    // The current §8.3.1 pair, inking the lower window's runs; the
    // model keeps the grid's cells dressed on its own.
    ink: (i32, i32),
    // The turn's tallest split: what keeps a quote box on the
    // screen after its shrink (see split_window).
    peak_split: usize,
    // A display that lost its picture asked for it whole; the next
    // render answers with everything.
    refresh_owed: bool,
    screen_columns: u8,
    screen_lines: u8,
    has_timed_input: bool,
    has_arc_images: bool,
    has_colours: bool,
    has_sounds: bool,
    // A §8 refusal raised mid-call: the trait's methods cannot
    // carry an error, so the first is held here and surfaces at
    // the next render -- the serving loop's error stanza, one
    // cycle late at worst.
    fault: Option<VoxamError>,
}

impl GlkOteFrontend {
    /// Open unmeasured, before any init; the model comes sized.
    pub fn new(version: u8, resources: Option<Resources>) -> Self {
        Self {
            version,
            page: Page::new(),
            resources,
            model: ScreenModel::new(80, 24, version),
            runs: Vec::new(),
            cleared: false,
            style: 0,
            size: (0, 0),
            cell: (1.0, 1.0),
            margins: (0.0, 0.0),
            grid_ident: None,
            next_ident: BUFFER + 1,
            band: None,
            band_ident: None,
            band_dirty: false,
            sound_ops: Vec::new(),
            sounding: None,
            sound_done: false,
            speaks_sound: false,
            ink: (DEFAULT_COLOUR, DEFAULT_COLOUR),
            peak_split: 0,
            refresh_owed: false,
            screen_columns: 80,
            screen_lines: 24,
            has_timed_input: false,
            has_arc_images: false,
            has_colours: false,
            has_sounds: false,
            fault: None,
        }
    }

    // -- the conversation's opening ----------------------------------------

    /// Open the session on the init event's word.
    ///
    /// The screen's size in cells is settled here, before any
    /// machine is booted over this display -- the header reads it
    /// once at boot (§8.4). Fails when the metrics carry no size.
    pub fn begin(&mut self, stanza: &Object) -> Result<(), VoxamError> {
        let support: Vec<String> = match stanza.get("support") {
            Some(Value::List(held)) => held
                .iter()
                .filter_map(|word| word.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        let supports = |word: &str| support.iter().any(|held| held == word);
        let blorbed = self
            .resources
            .as_ref()
            .is_some_and(|held| held.blorb.is_some());

        self.has_timed_input = supports("timer");

        // The band's claim is honest twice over: pictures must
        // actually hang behind the story, and the display must
        // speak graphics windows (arc_image: the contract).
        self.has_arc_images = blorbed && supports("graphicswin");

        // Colours are the dialect's word too: a display that says
        // it renders the ink the spans carry, and one that never
        // learned it leaves the header's §8.3 offer honestly
        // unclaimed.
        self.has_colours = supports("colors");

        // The sound claim is honest twice over as well: the
        // display must say the dialect's word, and a Blorb must
        // actually hang sounds behind the story (§9, §11.1). The
        // interpreter's own bleeps need only the display.
        self.speaks_sound = supports("sound");
        self.has_sounds = self.speaks_sound && blorbed;

        // The doorway courtesy, over the wire: the Blorb's cover
        // stands at the top of the story's text, when there is one
        // and the display grants bare graphics -- pictures laid in
        // text (Blorb: Frontispiece Chunk). Art is a courtesy,
        // never a gate: no cover, no grant, or an unmeasurable
        // picture simply plays on.
        if supports("graphics") {
            let cover = self
                .resources
                .as_mut()
                .and_then(|held| held.frontispiece())
                .map(fronted_cover);

            if let Some(cover) = cover {
                self.runs.push(Run::Special(cover));
                self.runs.push(Run::text("normal", 0, "\n"));
            }
        }

        // The record's card joins the cover at the door: the
        // bibliography WinFrotz shows in its own little window,
        // told as the page's opening text -- needing no grant,
        // since a card is only text (Babel: The iFiction format).
        let record = self
            .resources
            .as_ref()
            .and_then(|held| held.blorb.as_ref())
            .and_then(|blorb| blorb.ifiction.as_deref())
            .and_then(ifiction);

        if let Some(record) = record {
            for (name, text) in carded(&record) {
                self.runs.push(Run::text(&name, 0, &text));
            }
        }

        self.measure(stanza)?;

        self.model = ScreenModel::new(
            usize::from(self.screen_columns),
            usize::from(self.screen_lines),
            self.version,
        );

        Ok(())
    }

    /// Take the display's box from its metrics, which it must
    /// carry.
    fn sized(&mut self, stanza: &Object) -> Result<Object, VoxamError> {
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

        self.size = (width as i64, height as i64);

        Ok(metrics)
    }

    /// Take the display's size and cells from its metrics.
    fn measure(&mut self, stanza: &Object) -> Result<(), VoxamError> {
        let metrics = self.sized(stanza)?;
        let (width, height, margin_x, margin_y) = measured(&metrics, "grid");

        self.cell = (width, height);
        self.margins = (margin_x, margin_y);
        self.screen_columns =
            (((self.size.0 as f64 - margin_x) / width).floor() as i64).clamp(1, 255) as u8;
        self.screen_lines =
            (((self.size.1 as f64 - margin_y) / height).floor() as i64).clamp(1, 255) as u8;

        Ok(())
    }

    /// Hold the first §8 refusal a trait call raises; render
    /// surfaces it.
    fn noted(&mut self, result: Result<(), VoxamError>) {
        if let Err(error) = result
            && self.fault.is_none()
        {
            self.fault = Some(error);
        }
    }

    // -- the screen ops, §8 through the model ------------------------------

    /// One lower-window run in the current dress and ink.
    fn run(&self, text: &str) -> Run {
        let name = named(self.style);
        let ink = inked(self.ink, self.style & REVERSE != 0);

        match ink {
            None => Run::text(name, 0, text),
            Some(ink) => Run::Text(TextRun::inked(name, 0, text, ink)),
        }
    }

    /// The typed line and its newline, in the input dress.
    fn echoed(&mut self, line: &str) {
        self.runs.push(Run::text("input", 0, &format!("{line}\n")));
    }

    /// Hang, replace, or clear the arc_image band.
    ///
    /// Id 0 takes the band down; an id no picture answers, or a
    /// mode outside the two named, is ignored where it lands --
    /// presentation, never state (arc_image: the contract). A
    /// change re-bases the screen through the machine's own
    /// rebase, asked right after this call.
    fn hang_arc_image(&mut self, image: u16, mode: u16) {
        if !ARC_MODES.contains(&mode) {
            return;
        }

        let hung = if image == 0 {
            None
        } else {
            let found = self
                .resources
                .as_mut()
                .and_then(|held| held.image(u32::from(image)));

            if found.is_none() {
                return;
            }

            Some((image, mode))
        };

        if hung == self.band {
            return;
        }

        self.band = hung;
        self.band_dirty = true;
    }

    /// The band's height in display pixels, aspect held true.
    ///
    /// The art is mode x 8 rows tall at the 320-pixel reference
    /// width; the display's band keeps that shape at its own width
    /// (arc_image: the contract).
    fn band_height(&self) -> i64 {
        let Some((_, mode)) = self.band else {
            return 0;
        };

        let (width, _) = self.size;

        (width as f64 * f64::from(mode) * f64::from(ARC_PIXEL_ROWS)
            / f64::from(ARC_REFERENCE_WIDTH))
        .round_ties_even() as i64
    }

    /// How many text rows stand below whatever hangs: the header's
    /// claim (arc_image: the contract, part A).
    fn rows_below(&self) -> i64 {
        let (_, height) = self.size;
        let (_, cell_h) = self.cell;

        ((height as f64 - self.band_height() as f64 - self.margins.1) / cell_h).floor() as i64
    }

    // -- the §9 sounds, in the wire's own dialect ---------------------------

    /// The §9.4.3 play count in the dialect's own spelling.
    ///
    /// Zero repeats until stopped, spelled -1 on the wire; None is
    /// Version 3's silence on the matter, answered by the Blorb's
    /// Loop chunk -- how The Lurking Horror's rats hum until the
    /// valve stops them (Blorb: The Looping Chunk).
    fn repeated(&self, number: u16, repeats: Option<u16>) -> i64 {
        match repeats {
            None => {
                let looped = self
                    .resources
                    .as_ref()
                    .and_then(|held| held.blorb.as_ref())
                    .is_some_and(|blorb| blorb.loops.contains(&u32::from(number)));

                if looped { -1 } else { 1 }
            }
            Some(0) => -1,
            Some(count) => i64::from(count),
        }
    }

    /// The cycle's queued channel ops onto the page, once.
    fn sung(&mut self) {
        if !self.sound_ops.is_empty() {
            let ops = std::mem::take(&mut self.sound_ops);

            self.page.sounds(ops);
        }
    }

    // -- the render's face-side pieces --------------------------------------

    /// The grid's height: the §8.2 chrome plus the split.
    ///
    /// The split is the turn's high water, not the moment's -- the
    /// quote-box courtesy split_window explains.
    fn grid_rows(&self) -> usize {
        let chrome = if self.version <= STATUS_FLAGS_VERSION {
            1
        } else {
            0
        };

        chrome + self.model.split().max(self.peak_split)
    }

    /// Declare the band's canvas and feed any owed drawing;
    /// answers the band's height in display pixels, zero without
    /// one.
    fn banded(&mut self, width: i64) -> Result<i64, VoxamError> {
        if self.band.is_none() {
            self.band_ident = None;

            return Ok(0);
        }

        let band_h = self.band_height();
        let ident = match self.band_ident {
            Some(held) => held,
            None => {
                let minted = self.next_ident;

                self.next_ident += 1;
                self.band_ident = Some(minted);

                minted
            }
        };

        self.page.window(
            ident,
            "graphics",
            0,
            (0, 0, width, band_h),
            WindowSpec {
                graphsize: Some((width, band_h)),
                ..WindowSpec::default()
            },
        )?;

        if self.band_dirty {
            self.band_dirty = false;

            let (picture, _) = self.band.expect("a band hangs");
            let url = self
                .resources
                .as_mut()
                .and_then(|held| held.pictured(u32::from(picture)));

            let mut fill = Object::new();

            fill.set("special", "fill");

            let mut image = Object::new();

            image.set("special", "image");
            image.set("image", i64::from(picture));
            image.set("url", url.map_or(Value::Null, Value::Str));
            image.set("x", 0i64);
            image.set("y", 0i64);
            image.set("width", width);
            image.set("height", band_h);

            self.page.draw(ident, vec![fill, image])?;
        }

        Ok(band_h)
    }

    /// The grid's face, cells coalesced into named, inked runs.
    fn faced(&mut self, rows: usize) -> Vec<Vec<TextRun>> {
        let mut face: Vec<Vec<TextRun>> = Vec::new();

        for row in 1..=rows {
            let mut spans: Vec<TextRun> = Vec::new();

            for column in 1..=usize::from(self.screen_columns) {
                let held = self.model.cell(row, column);
                let name = named(held.style);
                let ink = inked(
                    (held.foreground, held.background),
                    held.style & REVERSE != 0,
                );

                match spans.last_mut() {
                    Some(last) if last.style == name && last.ink == ink => {
                        last.text.push(held.character);
                    }
                    _ => {
                        let mut text = String::new();

                        text.push(held.character);
                        spans.push(TextRun {
                            style: name.to_string(),
                            link: 0,
                            text,
                            ink,
                        });
                    }
                }
            }

            face.push(spans);
        }

        face
    }
}

impl Frontend for SharedFace {
    fn write(&mut self, text: &str) {
        let mut face = self.face.borrow_mut();

        if face.model.selected() == UPPER {
            face.model.write(text);
        } else {
            let run = face.run(text);

            face.runs.push(run);
        }
    }

    /// §15 print_table: stamped in the upper, stacked below.
    fn write_rectangle(&mut self, rows: &[String]) {
        let mut face = self.face.borrow_mut();

        if face.model.selected() == UPPER {
            face.model.write_rectangle(rows);
        } else {
            for row in rows {
                let run = face.run(&format!("{row}\n"));

                face.runs.push(run);
            }
        }
    }

    /// The §8.2 status line, drawn onto the model's top row.
    fn show_status(&mut self, status: &Status) {
        let mut face = self.face.borrow_mut();
        let result = face.model.show_status(status);

        face.noted(result);
    }

    /// Sound a bleep over the wire: 1 is high, 2 is low (§9.2).
    ///
    /// The op carries no sample: the display's own oscillator
    /// answers, the way a terminal's bell would. Only a display
    /// that said the dialect's word hears it -- no Blorb needed,
    /// since the bleeps are the interpreter's own.
    fn bleep(&mut self, high: bool) {
        let mut face = self.face.borrow_mut();

        if face.speaks_sound {
            let mut op = Object::new();

            op.set("op", "bleep");
            op.set("bleep", if high { 1i64 } else { 2i64 });
            face.sound_ops.push(op);
        }
    }

    /// §8.7.1 combining: zero clears, anything else joins.
    fn set_style(&mut self, style: u16) {
        let mut face = self.face.borrow_mut();

        face.model.set_style(style);
        face.style = if style == 0 { 0 } else { face.style | style };
    }

    /// Fonts route to the model; the dress keys on styles.
    fn set_font(&mut self, font: u16) {
        self.face.borrow_mut().model.set_font(font);
    }

    /// The display wraps for itself; the model need not.
    fn set_buffering(&mut self, _buffered: bool) {}

    /// Resize the upper window (§8.7.2.1); the model rules.
    ///
    /// The turn's tallest split is remembered: an Inform quote box
    /// splits tall, writes, and shrinks back at once, trusting
    /// §8.6.1.2's rule that splitting clears nothing from Version
    /// 4 -- on a real §8 screen the box lingers in the unsplit
    /// region, so the grid here stands at the turn's high water
    /// until the next input arrives, the same courtesy garglk and
    /// Parchment extend the same box.
    fn split_window(&mut self, lines: u16) {
        let mut face = self.face.borrow_mut();
        let result = face.model.split_window(i32::from(lines));

        face.noted(result);

        if face.version > STATUS_FLAGS_VERSION {
            face.peak_split = face.peak_split.max(usize::from(lines));
        }
    }

    /// Select the window taking the next printing (§8.7.2).
    fn set_window(&mut self, window: u16) {
        let mut face = self.face.borrow_mut();
        let result = face.model.set_window(window);

        face.noted(result);
    }

    /// Place the upper window's cursor (§8.7.2.3).
    fn set_cursor(&mut self, line: u16, column: u16) {
        let mut face = self.face.borrow_mut();
        let result = face.model.set_cursor(line, column);

        face.noted(result);
    }

    /// What get_cursor reads back: the model's own ledger.
    fn cursor_position(&self) -> (u16, u16) {
        let (line, column) = self.face.borrow_mut().model.get_cursor();

        (line as u16, column as u16)
    }

    /// An erasure of the lower half clears the buffer whole.
    ///
    /// The whole-screen forms are a deliberate teardown, not a
    /// quote box: the high water recedes with the split
    /// (§8.7.3.3).
    fn erase_window(&mut self, window: i32) {
        let mut face = self.face.borrow_mut();
        let result = face.model.erase_window(window);

        face.noted(result);

        if window < 0 {
            face.peak_split = face.model.split();
        }

        if window != i32::from(UPPER) {
            face.runs.clear();
            face.cleared = true;
        }
    }

    /// To the end of the line -- meaningful in the grid alone.
    fn erase_line(&mut self) {
        let mut face = self.face.borrow_mut();

        if face.model.selected() == UPPER {
            face.model.erase_line();
        }
    }

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
        self.face.borrow().has_timed_input
    }

    fn has_sounds(&self) -> bool {
        self.face.borrow().has_sounds
    }

    fn has_colours(&self) -> bool {
        self.face.borrow().has_colours
    }

    /// Clicks have no support token: they are core GlkOte, so the
    /// header's §10.3.1.1 request answers honestly on the wire.
    fn has_mouse(&self) -> bool {
        true
    }

    fn has_arc_images(&self) -> bool {
        self.face.borrow().has_arc_images
    }

    fn arc_rows_below(&self) -> Option<i64> {
        Some(self.face.borrow().rows_below())
    }

    fn screen_lines(&self) -> u8 {
        self.face.borrow().screen_lines
    }

    fn screen_columns(&self) -> u8 {
        self.face.borrow().screen_columns
    }

    /// Suspends like its Glk twin: never asked for input.
    fn suspends(&self) -> bool {
        true
    }

    /// Change the printing colours (§8.3.1).
    ///
    /// The model keeps the grid's cells dressed; the pair kept
    /// here inks the lower window's runs -- zero keeps a colour
    /// current, exactly as the model reads it. Only a claiming
    /// face hears the call at all (§8.3).
    fn set_colour(&mut self, foreground: i32, background: i32) {
        let mut face = self.face.borrow_mut();

        face.model.set_colour(foreground, background);

        let (fg, bg) = face.ink;

        face.ink = (
            if foreground != CURRENT_COLOUR {
                foreground
            } else {
                fg
            },
            if background != CURRENT_COLOUR {
                background
            } else {
                bg
            },
        );
    }

    fn draw_arc_image(&mut self, image: u16, mode: u16) {
        self.face.borrow_mut().hang_arc_image(image, mode);
    }

    /// Start a sampled sound on the wire's one channel (§9.4).
    ///
    /// The newest play winning is §9.4.2's own rule, and the
    /// display's channel does exactly that. The §9.3 volume maps
    /// to eighths of unit gain, and a sound the wire cannot carry
    /// starts nothing -- so its end-of-sound routine is never
    /// kept.
    fn play_sound(&mut self, number: u16, volume: u16, repeats: Option<u16>) -> bool {
        let mut face = self.face.borrow_mut();
        let url = face
            .resources
            .as_mut()
            .and_then(|held| held.audible(u32::from(number)));

        let Some(url) = url else {
            return false;
        };

        let mut op = Object::new();

        op.set("channel", 1i64);
        op.set("op", "play");
        op.set("sound", i64::from(number));
        op.set("url", url);
        op.set("repeats", face.repeated(number, repeats));
        op.set("notify", 0i64);
        op.set("volume", f64::from(volume) / f64::from(FULL_VOLUME));

        face.sound_ops.push(op);
        face.sounding = Some(number);
        face.sound_done = false;

        true
    }

    /// Stop the sounding sample, when the ask names it (§9.4).
    ///
    /// One sound plays at a time (§9.4.2), so a stop for some
    /// other number stops nothing -- and None, the stop-them-all
    /// form, always lands on whatever sounds.
    fn stop_sound(&mut self, number: Option<u16>) {
        let mut face = self.face.borrow_mut();

        if face.sounding.is_none() || (number.is_some() && number != face.sounding) {
            return;
        }

        face.sounding = None;
        face.sound_done = false;

        let mut op = Object::new();

        op.set("channel", 1i64);
        op.set("op", "stop");
        face.sound_ops.push(op);
    }

    /// Whether the wire reported a natural ending, once (§9.4.4).
    fn sound_finished(&mut self) -> bool {
        let mut face = self.face.borrow_mut();
        let done = face.sound_done;

        face.sound_done = false;

        done
    }
}

/// The face's shareable half: the machine's [`Frontend`] handle.
pub struct SharedFace {
    face: Rc<RefCell<GlkOteFrontend>>,
}

/// The face a story's screen model asks for.
///
/// The two-window picture for every version but 6; the §8.8 stage
/// face arrives with the stage rung, and until it does a Version 6
/// story is refused here rather than mis-served.
pub fn fronted(version: u8, resources: Option<Resources>) -> Result<GlkOteFrontend, VoxamError> {
    if version == STAGE_VERSION {
        return Err(glkote_error(
            "the Version 6 stage face is not yet ported; the stage rung carries it".into(),
        ));
    }

    Ok(GlkOteFrontend::new(version, resources))
}

/// One Z session's two ends held together: the machine, and the
/// face it prints through.
///
/// The reference's face holds its machine and the render and
/// accept halves live on the face; here they live on the Session,
/// which holds both ends -- the same conversation, the borrow
/// checker's way.
pub struct Session {
    machine: Machine,
    face: Rc<RefCell<GlkOteFrontend>>,
    // The §15 clock's restart ledger: the machine's wait serial at
    // the last timed render, so a fresh timed read restarts the
    // display's clock even at the same cadence -- the reference
    // compares the wait's identity, and the serial is that
    // comparison's spelling.
    last_read: Option<u64>,
}

impl Session {
    /// Boot a machine over a begun face.
    pub fn open(
        story: Story,
        frontend: GlkOteFrontend,
        seed: Option<u32>,
    ) -> Result<Self, VoxamError> {
        let face = Rc::new(RefCell::new(frontend));
        let machine = Machine::new(
            story,
            Box::new(SharedFace { face: face.clone() }),
            seed,
            Identity::default(),
            None,
        )?;

        Ok(Self {
            machine,
            face,
            last_read: None,
        })
    }

    /// The machine this session drives.
    pub fn machine(&mut self) -> &mut Machine {
        &mut self.machine
    }

    /// The face, borrowed for a direct word with it.
    pub fn face(&self) -> RefMut<'_, GlkOteFrontend> {
        self.face.borrow_mut()
    }

    // -- the two halves of the conversation --------------------------------

    /// Compose everything since the last update into a stanza.
    ///
    /// The grid is the status chrome plus the split; one that
    /// closes and reopens is a new window with a new id, the
    /// protocol forbidding reuse (GlkOte: The Windows Update
    /// Array).
    pub fn render(&mut self, exit: bool) -> Result<Object, VoxamError> {
        if let Some(fault) = self.face.borrow_mut().fault.take() {
            return Err(fault);
        }

        let mut face = self.face.borrow_mut();
        let (width, height) = face.size;
        let (_, cell_h) = face.cell;
        let rows = face.grid_rows();

        // The band hangs above everything: grid and buffer alike
        // re-base below it (arc_image: the contract, part A).
        let band_h = face.banded(width)?;

        // A grid's box carries its rows plus the display's own
        // interior margins (GlkOte: The Metrics Object); a box of
        // bare rows clips its bottom and floats the buffer up into
        // the status line.
        let brow = band_h
            + if rows > 0 {
                (rows as f64 * cell_h + face.margins.1) as i64
            } else {
                0
            };

        // The window's paper is the model's own background,
        // travelling only when a claiming display can show it and
        // a game has coloured it -- Photopia's scenes bleed to the
        // window's edge, not just under its letters (§8.3).
        let paper = if face.has_colours {
            css(face.model.background())
        } else {
            None
        };

        face.page.window(
            BUFFER,
            "buffer",
            0,
            (0, brow, width, height),
            WindowSpec {
                bg: paper.clone(),
                ..WindowSpec::default()
            },
        )?;

        if rows > 0 {
            let ident = match face.grid_ident {
                Some(held) => held,
                None => {
                    let minted = face.next_ident;

                    face.next_ident += 1;
                    face.grid_ident = Some(minted);

                    minted
                }
            };
            let columns = i64::from(face.screen_columns);

            face.page.window(
                ident,
                "grid",
                0,
                (0, band_h, width, brow),
                WindowSpec {
                    gridsize: Some((columns, rows as i64)),
                    bg: paper,
                    ..WindowSpec::default()
                },
            )?;

            let rows_faced = face.faced(rows);

            face.page.grid(ident, &rows_faced)?;
        } else {
            face.grid_ident = None;
        }

        if !face.runs.is_empty() || face.cleared {
            let runs = std::mem::take(&mut face.runs);
            let clear = std::mem::replace(&mut face.cleared, false);

            face.page.buffer(BUFFER, &runs, clear)?;
        }

        match self.machine.waiting() {
            Some(Waiting::File(filing)) => {
                // A save or restore asks for its file through the
                // protocol's special input; the display disables
                // the game until the answer comes back (GlkOte:
                // Special Input Requests).
                face.page.prompt(
                    if filing.purpose == Purpose::Save {
                        "write"
                    } else {
                        "read"
                    },
                    "save",
                )?;
            }
            Some(Waiting::Read(held)) => {
                if held.wants == Wants::Line {
                    // The field carries no §15 preload: "the game
                    // must do this" -- the held text is already
                    // printed by the story's own hand, so what the
                    // field sends back is the typed part alone,
                    // which is exactly what the machine appends
                    // after the preload it holds (§15 read).
                    let mut codes: Vec<u16> = held.terminators.iter().copied().collect();

                    codes.sort_unstable();

                    face.page.line_input(
                        BUFFER,
                        held.capacity as i64,
                        LineSpec {
                            terminators: codes.into_iter().filter_map(terminator_name).collect(),
                            ..LineSpec::default()
                        },
                    )?;
                } else {
                    face.page.char_input(BUFFER, None, false, false)?;
                }

                if let Some(grid) = face.grid_ident
                    && (held.wants == Wants::Key || held.terminators.contains(&SINGLE_CLICK))
                {
                    // A keystroke read hears a click the way it
                    // hears any key; a line read only when its
                    // table names the click code (§10.3.3). The
                    // grid is the whole clickable surface:
                    // "buffer windows do not support mouse-click
                    // input" (GlkOte: The Input Update Array).
                    face.page.passive_input(grid, false, true)?;
                }
            }
            None => {}
        }

        // The timer field for the cycle, from the standing wait: a
        // fresh timed read restarts the display's clock even at
        // the same cadence, as §15 restarts its own.
        let serial = self.machine.wait_serial();

        match self.machine.waiting() {
            Some(Waiting::Read(held)) if held.time != 0 && held.routine != 0 => {
                face.page.timer(
                    i64::from(held.time) * TENTH_MS,
                    self.last_read != Some(serial),
                );

                self.last_read = Some(serial);
            }
            Some(_) => {
                face.page.timer(0, false);

                self.last_read = Some(serial);
            }
            None => {
                face.page.timer(0, false);

                self.last_read = None;
            }
        }

        face.sung();

        let refresh = std::mem::replace(&mut face.refresh_owed, false);

        face.page.update(exit, refresh)
    }

    /// Translate one inbound stanza into a serving verdict.
    ///
    /// Advance means the machine can run on; Stand means the wait
    /// still stands but the picture may have changed -- a timer's
    /// interrupt printed -- and Pass means the stanza asked for
    /// nothing here. Delivered input begins the next turn, so a
    /// quote box's high water recedes to the real split there.
    pub fn accept(&mut self, stanza: &Object) -> Result<Verdict, VoxamError> {
        let verdict = self.accepted(stanza)?;

        if verdict == Verdict::Advance {
            let mut face = self.face.borrow_mut();

            face.peak_split = face.model.split();
        }

        Ok(verdict)
    }

    /// The verdict itself, one arm per event kind.
    fn accepted(&mut self, stanza: &Object) -> Result<Verdict, VoxamError> {
        let kind = stanza
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        if !NO_PARTIAL.contains(&kind.as_str()) {
            let typed = partials(stanza.get("partial"));

            self.face.borrow_mut().page.typed(typed);
        }

        if kind == "refresh" {
            // The display lost its picture and asks for it whole
            // -- ahead of the generation gate, since a refreshing
            // display is out of sync by definition (GlkOte: the
            // refresh input event). The band owes its drawing
            // again too.
            let mut face = self.face.borrow_mut();

            face.refresh_owed = true;

            if face.band.is_some() {
                face.band_dirty = true;
            }

            return Ok(Verdict::Stand);
        }

        if stanza.get("gen").and_then(Value::as_int) != Some(self.face.borrow().page.generation()) {
            return Ok(Verdict::Pass);
        }

        match kind.as_str() {
            "line" => self.lined(stanza),
            "char" => self.keyed(stanza),
            "mouse" => self.pointed(stanza),
            "timer" => self.ticked(),
            "sound" => self.sound_over(stanza),
            "specialresponse" => self.answered(stanza),
            "arrange" | "redraw" => self.reshaped(&kind, stanza),
            _ => Ok(Verdict::Pass),
        }
    }

    /// The standing read of this kind, or not.
    ///
    /// The guard that keeps a misaimed delivery from reaching the
    /// machine's loud wiring-fault refusals: a display can misaim
    /// one event across the roster's swap -- a keystroke landing
    /// in a field already replaced -- and a misaimed delivery is
    /// the blocking loop's shrug, never a session-fatal wiring
    /// fault.
    fn reading(&self, wants: Wants) -> bool {
        matches!(self.machine.waiting(), Some(Waiting::Read(held)) if held.wants == wants)
    }

    /// A typed line to the machine, echoed first.
    fn lined(&mut self, stanza: &Object) -> Result<Verdict, VoxamError> {
        if !self.reading(Wants::Line) {
            return Ok(Verdict::Pass);
        }

        let line = stringy(stanza.get("value"));
        let terminator = stanza
            .get("terminator")
            .and_then(Value::as_str)
            .map_or(0, terminator_code);

        // The machine never echoes: the display owes the typed
        // line and its newline -- but only a return-ended read
        // prints its return (§15 read). A terminator-ended line
        // stays uncommitted, ready for the preloaded re-read
        // Beyond Zork answers one with.
        if terminator == 0 {
            self.face.borrow_mut().echoed(&line);
        }

        self.machine.deliver_line(&line, terminator)?;

        Ok(Verdict::Advance)
    }

    /// One keystroke to the machine, §3.8-spelled.
    fn keyed(&mut self, stanza: &Object) -> Result<Verdict, VoxamError> {
        if !self.reading(Wants::Key) {
            return Ok(Verdict::Pass);
        }

        let key = match stanza.get("value") {
            Some(Value::Str(held)) if held.chars().count() == 1 => {
                held.chars().next().expect("one character")
            }
            other => match zscii_key(&stringy(other)) {
                Some(named) => named,
                None => return Ok(Verdict::Pass),
            },
        };

        if self.machine.deliver_key(key)? {
            Ok(Verdict::Advance)
        } else {
            Ok(Verdict::Pass)
        }
    }

    /// A click in the grid to the machine, §10.3-spelled.
    ///
    /// The event's cell coordinates count from the grid's own
    /// zero; the header extension counts the screen from (1,1),
    /// and the grid sits at the screen's top, so one step moves
    /// between them (§10.3.2). A click that ends a line read takes
    /// the typed text riding the event as the line composed so
    /// far; a click nothing can hear passes with the wait
    /// standing.
    fn pointed(&mut self, stanza: &Object) -> Result<Verdict, VoxamError> {
        let grid = self.face.borrow().grid_ident;

        let Some(grid) = grid else {
            return Ok(Verdict::Pass);
        };

        if stanza.get("window").and_then(Value::as_int) != Some(grid) {
            return Ok(Verdict::Pass);
        }

        if !matches!(self.machine.waiting(), Some(Waiting::Read(_))) {
            // No read stands to hear a click: the misaimed-event
            // shrug -- deliver_click's own false covers a standing
            // read that cannot hear one.
            return Ok(Verdict::Pass);
        }

        let typed = partials(stanza.get("partial"))
            .get(&BUFFER)
            .cloned()
            .unwrap_or_default();
        let x = stanza.get("x").and_then(Value::as_int).unwrap_or(0);
        let y = stanza.get("y").and_then(Value::as_int).unwrap_or(0);
        let heard = self
            .machine
            .deliver_click((x + 1) as u16, (y + 1) as u16, &typed)?;

        if heard {
            Ok(Verdict::Advance)
        } else {
            Ok(Verdict::Pass)
        }
    }

    /// A timer event: the §15 interrupt fires, or nothing does.
    ///
    /// A tick beside a file ask passes here too -- the reference
    /// reaches for the wait's routine and would fall over a
    /// Filing, a road no conforming display takes.
    fn ticked(&mut self) -> Result<Verdict, VoxamError> {
        match self.machine.waiting() {
            Some(Waiting::Read(held)) if held.routine != 0 => {}
            _ => return Ok(Verdict::Pass),
        }

        self.machine.deliver_tick()?;

        if self.machine.waiting().is_none() {
            Ok(Verdict::Advance)
        } else {
            Ok(Verdict::Stand)
        }
    }

    /// A sampled sound finished naturally on the display.
    ///
    /// The ending is noted once and §9.4.4's end-of-sound routine
    /// fires through the machine's own re-entrant loop, its prints
    /// rendered while any read stands. A report for a sound since
    /// stopped or replaced means nothing -- §9.4.4's own rule --
    /// and passes with the picture unchanged.
    fn sound_over(&mut self, stanza: &Object) -> Result<Verdict, VoxamError> {
        let sounding = self.face.borrow().sounding;

        let Some(sounding) = sounding else {
            return Ok(Verdict::Pass);
        };

        if stanza.get("sound").and_then(Value::as_int) != Some(i64::from(sounding)) {
            return Ok(Verdict::Pass);
        }

        {
            let mut face = self.face.borrow_mut();

            face.sounding = None;
            face.sound_done = true;
        }

        self.machine.poll_sound()?;

        Ok(Verdict::Stand)
    }

    /// The player's file name, or not, to the suspended ask.
    ///
    /// A response to some other ask asks nothing here (GlkOte:
    /// Special Input Requests); a non-string value is a browser
    /// dialog's fileref object, and no dialog was invited: it
    /// reads as the cancel it is, which is always legitimate.
    fn answered(&mut self, stanza: &Object) -> Result<Verdict, VoxamError> {
        if stanza.get("response").and_then(Value::as_str) != Some("fileref_prompt") {
            return Ok(Verdict::Pass);
        }

        if !matches!(self.machine.waiting(), Some(Waiting::File(_))) {
            // No file ask stands: the misaimed-event shrug.
            return Ok(Verdict::Pass);
        }

        let value = stanza
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_string);

        self.machine.deliver_file(value.as_deref())?;

        Ok(Verdict::Advance)
    }

    /// An arrange or redraw: the picture re-shapes or re-paints.
    ///
    /// An arrange re-measures -- the next reload boots a machine
    /// at the new size (§8.4), while a hanging band re-shapes now
    /// and its rows-below claim follows. A redraw re-feeds the
    /// band whole (GlkOte: Redraw Events); without one there is
    /// nothing here to redraw.
    fn reshaped(&mut self, kind: &str, stanza: &Object) -> Result<Verdict, VoxamError> {
        if kind == "redraw" {
            let mut face = self.face.borrow_mut();

            if face.band.is_none() {
                return Ok(Verdict::Pass);
            }

            face.band_dirty = true;

            return Ok(Verdict::Stand);
        }

        let banded = {
            let mut face = self.face.borrow_mut();

            face.measure(stanza)?;

            if face.band.is_some() {
                face.band_dirty = true;

                true
            } else {
                false
            }
        };

        if banded {
            self.machine.rebase_rows()?;
        }

        Ok(Verdict::Stand)
    }
}

/// Drive one Z session over the protocol, stanza by stanza.
///
/// The init comes first -- the machine boots only after it, since
/// the header reads the screen's size at boot -- and thereafter
/// the burst model: run to a suspension, the update out, the
/// answer delivered. True is a session that ended cleanly; a
/// broken conversation answers the protocol's own error stanza and
/// is false.
pub fn serve(
    story: Story,
    frontend: GlkOteFrontend,
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
    seed: Option<u32>,
) -> bool {
    match served(story, frontend, reader, writer, seed) {
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
    story: Story,
    mut frontend: GlkOteFrontend,
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
    seed: Option<u32>,
) -> Result<bool, VoxamError> {
    let opening = read_stanza(reader)?;
    let opening = opening.filter(|held| held.get("type").and_then(Value::as_str) == Some("init"));

    let Some(opening) = opening else {
        return Err(glkote_error(
            "the conversation opens with an init event (GlkOte: The Application's Life Story)"
                .into(),
        ));
    };

    frontend.begin(&opening)?;

    let mut session = Session::open(story, frontend, seed)?;

    loop {
        session.machine().run()?;

        let running = session.machine().running();
        let update = session.render(!running)?;

        write_stanza(writer, &update);

        if !running {
            return Ok(true);
        }

        loop {
            let Some(stanza) = read_stanza(reader)? else {
                return Ok(true);
            };

            match session.accept(&stanza)? {
                Verdict::Advance => break,
                Verdict::Stand => {
                    // The wait stands but the picture moved -- an
                    // interrupt printed, a resize arrived.
                    let update = session.render(false)?;

                    write_stanza(writer, &update);
                }
                Verdict::Pass => {
                    let mut pass = Object::new();

                    pass.set("type", "pass");
                    write_stanza(writer, &pass);
                }
            }
        }
    }
}

/// One error stanza out: the protocol's own answer to a broken
/// conversation.
fn error_stanza(writer: &mut dyn Write, message: &str) {
    let mut stanza = Object::new();

    stanza.set("type", "error");
    stanza.set("message", message);
    write_stanza(writer, &stanza);
}

/// The Blorb's cover as a ready-made image span.
///
/// The picture rides whole as a data: url, drawn inline at its own
/// size -- the display's proportional cap shrinks a large cover to
/// the page (Blorb: Frontispiece Chunk; GlkOte: The Line Data
/// Array).
fn fronted_cover(cover: &ImageInfo) -> Object {
    let mut span = Object::new();

    span.set("special", "image");
    span.set("image", i64::from(cover.number));
    span.set("url", pictured(cover));
    span.set("width", i64::from(cover.width));
    span.set("height", i64::from(cover.height));
    span.set("alignment", "inlineup");

    span
}

/// An event value as the text the reference's str() would read.
///
/// Displays send strings; the coercions here cover the JSON
/// values a stray one could carry instead.
fn stringy(value: Option<&Value>) -> String {
    match value {
        Some(Value::Str(held)) => held.clone(),
        Some(Value::Int(held)) => held.to_string(),
        Some(Value::Bool(held)) => if *held { "True" } else { "False" }.to_string(),
        _ => String::new(),
    }
}

/// A §8.3.1 code as CSS ink, None for the display's own default.
///
/// The values are the shared palette every face shows -- the same
/// RGB the pygame glass mixes (§8.3.7's equivalents).
fn css(code: i32) -> Option<String> {
    colour_value(code).map(|(r, g, b)| format!("#{r:02x}{g:02x}{b:02x}"))
}

/// §8.3.1 codes as the dialect's ink, None when all default.
///
/// Reverse video swaps ink and paper, as every painted face swaps
/// them (§8.7.1.1) -- a side left None keeps the display's own
/// colour for that half.
fn inked(pair: (i32, i32), reverse: bool) -> Option<Ink> {
    let (mut fg, mut bg) = pair;

    if reverse {
        std::mem::swap(&mut fg, &mut bg);
    }

    let held = (css(fg), css(bg));

    if held == (None, None) {
        None
    } else {
        Some(held)
    }
}

/// A §8.7 style bitmask as the protocol name it wears.
///
/// Priority-ordered: reverse video first (the page's own CSS
/// dresses user1 as inverse), then fixed pitch, then the weights.
fn named(style: u16) -> &'static str {
    if style & REVERSE != 0 {
        return "user1";
    }

    if style & FIXED_PITCH != 0 {
        return "preformatted";
    }

    if style & BOLD != 0 && style & ITALIC != 0 {
        return "alert";
    }

    if style & BOLD != 0 {
        return "subheader";
    }

    if style & ITALIC != 0 {
        return "emphasized";
    }

    "normal"
}

#[cfg(test)]
#[path = "glkote_tests.rs"]
mod tests;
