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
//! that need both (render, accept) live on the Session.
//!
//! The Version 6 stage joins as the same struct wearing a stage
//! half: the StageFrontend the reference subclasses becomes
//! `stage: Option<StageHalf>`, and every seam that differs
//! branches on it. One scaled canvas carries the whole §8.8
//! screen -- the StageModel's unit-positioned paints become the
//! stage dialect's draw ops in the art's own coordinate space,
//! pictures plot through the gallery's adaptive-palette dance,
//! and §8.3.1's under-cursor sample reads the painted stage
//! itself, minting codes past the named ones. A display that
//! never learned the dialect is refused loudly at the door.

use std::cell::{RefCell, RefMut};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::rc::Rc;

use crate::babel::ifiction;
use crate::base64::b64;
use crate::errors::VoxamError;
use crate::frontend::{
    ARC_MODES, ARC_PIXEL_ROWS, ARC_REFERENCE_WIDTH, Frontend, Status, colour_value,
};
use crate::gallery::Gallery;
use crate::glkote::json::{Object, Value};
use crate::glkote::{
    Ink, LineSpec, Page, Run, TextRun, WindowSpec, carded, measured, partials, read_stanza,
    write_stanza,
};
use crate::glulx::glk::resources::{ImageInfo, Resources, pictured};
use crate::png;
use crate::screen::{BOLD, ITALIC};
use crate::screen::{CURRENT_COLOUR, DEFAULT_COLOUR, FIXED_PITCH, REVERSE, ScreenModel, UPPER};
use crate::stage::{Paint, StageModel, TextPaint};
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

/// The stage without a Reso chunk: MCGA's 320 by 200, the screen
/// Infocom's Version 6 art was drawn for -- and the Blorb rule for
/// art without a Reso is one image pixel per screen pixel, so the
/// art's own space is the only default that draws it true (Blorb:
/// The Resolution Chunk).
const STANDARD_STAGE: (u32, u32) = (320, 200);

/// One stage cell in units: the 8-by-8 character of the MCGA
/// presentation (§8.8.1).
const CELL: i64 = 8;

/// §8.3.1's "the colour of the pixel under the cursor", passed
/// through signed by the machine.
const PIXEL_COLOUR: i32 = -1;

/// Where the stage's minted colour codes begin: past every §8.3.1
/// code the spec names, exactly as the pygame glass mints its
/// sampled colours.
const FIRST_SAMPLED: i32 = 16;

/// The stage's own §8.3.1 code-1 defaults: white ink on black
/// paper, the machine's home look, matching the pygame glass.
const INK_DEFAULT: &str = "#ffffff";
const PAPER_DEFAULT: &str = "#000000";

/// The Version 6 stage's half of the face -- the reference's
/// StageFrontend subclass, spelled as extra state the same struct
/// wears: the StageModel, the gallery, the one scaled canvas, the
/// repaint journal, the adaptive-palette seam, and the minted
/// under-cursor colours.
struct StageHalf {
    stage: StageModel,
    gallery: Option<Gallery>,
    has_pictures: bool,
    canvas_ident: Option<i64>,
    // The cycle's draw ops, and the journal a repaint replays:
    // everything since the last whole-stage fill, since nothing
    // before one can ever show again.
    ops: Vec<Object>,
    journal: Vec<Object>,
    repaint_owed: bool,
    // The adaptive-palette seam: each picture's encoding
    // remembered per palette era, the standing chrome's positions
    // in insertion order (the reference's dict), and the last
    // Current Palette serial seen -- a change re-dresses the
    // chrome (Blorb: The Adaptive Palette Chunk).
    urls: HashMap<u32, (i64, String)>,
    chrome: Vec<(u32, (u16, u16))>,
    palette_serial: u32,
    // The minted colours: §8.3.1's under-cursor samples, each
    // distinct CSS colour given a code past the named ones -- the
    // wire's twin of the glass's sampled palette.
    minted: HashMap<i32, String>,
    codes: HashMap<String, i32>,
    next_code: i32,
}

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
    // The Version 6 stage half, None on the two-window face; every
    // seam that differs branches on it -- the reference's subclass
    // override, the borrow's way around.
    stage: Option<StageHalf>,
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
            stage: None,
        }
    }

    /// The Version 6 stage at the far end of the protocol.
    ///
    /// One scaled canvas in the stage dialect's words: the same
    /// StageModel the pygame glass paints from reduces the
    /// eight-window screen to unit-positioned paints, and each
    /// becomes a draw op -- placed text, fills, sliding rectangles
    /// -- in the art's own logical space, the display magnifying
    /// it to fit (§8.8). The stage is pinned to the Blorb's Reso
    /// standard window, or MCGA's 320 by 200 without one, so
    /// layouts land exactly where the artists put them and the
    /// Reso arithmetic collapses to each picture's own standard
    /// ratio.
    ///
    /// The doorway courtesies stay off the stage -- a Version 6
    /// game paints its own opening -- and the [MORE] budget stays
    /// unarmed: a suspending face cannot hold a scroll mid-print,
    /// so long passages flow uninterrupted, the scrollback road
    /// not yet taken. Fails only when the Blorb's art census
    /// cannot be hung.
    pub fn staged(version: u8, resources: Option<Resources>) -> Result<Self, VoxamError> {
        let mut face = Self::new(version, resources);
        let (mut width, mut height) = STANDARD_STAGE;

        if let Some(resolution) = face
            .resources
            .as_ref()
            .and_then(|held| held.blorb.as_ref())
            .and_then(|blorb| blorb.resolution.as_ref())
        {
            width = resolution.width;
            height = resolution.height;
        }

        face.screen_columns = (width / CELL as u32).clamp(1, 255) as u8;
        face.screen_lines = (height / CELL as u32).clamp(1, 255) as u8;
        // The stage paints its own ink into the ops, so the §8.3
        // claim needs no display grant.
        face.has_colours = true;

        // The gallery rules the picture claims exactly as it does
        // at the glass: placards measured, Reso understood, and a
        // count of zero leaving the header's offer unclaimed.
        let gallery = match face.resources.as_ref().and_then(|held| held.blorb.as_ref()) {
            Some(blorb) => Some(blorb.gallery()?),
            None => None,
        };
        let has_pictures = gallery.as_ref().is_some_and(|held| held.count() > 0);

        face.stage = Some(StageHalf {
            stage: StageModel::new(
                usize::from(face.screen_columns),
                usize::from(face.screen_lines),
                CELL as usize,
                CELL as usize,
            ),
            gallery,
            has_pictures,
            canvas_ident: None,
            ops: Vec::new(),
            journal: Vec::new(),
            repaint_owed: false,
            urls: HashMap::new(),
            chrome: Vec::new(),
            palette_serial: 0,
            minted: HashMap::new(),
            codes: HashMap::new(),
            next_code: FIRST_SAMPLED,
        });

        // The opening curtain: the stage's own paper before any
        // game paints, the setcolor keeping a rescaled canvas's
        // clear the same colour.
        let mut setcolor = Object::new();

        setcolor.set("special", "setcolor");
        setcolor.set("color", PAPER_DEFAULT);

        let curtain = face.whole_fill(PAPER_DEFAULT);
        let stage = face.stage.as_mut().expect("just staged");

        stage.ops.push(setcolor);
        stage.ops.push(curtain);

        Ok(face)
    }

    /// The stage's size in its own units.
    fn stage_logical(&self) -> (i64, i64) {
        (
            i64::from(self.screen_columns) * CELL,
            i64::from(self.screen_lines) * CELL,
        )
    }

    /// A fill covering the whole stage.
    fn whole_fill(&self, color: &str) -> Object {
        let (width, height) = self.stage_logical();
        let mut op = Object::new();

        op.set("special", "fill");
        op.set("x", 0i64);
        op.set("y", 0i64);
        op.set("width", width);
        op.set("height", height);
        op.set("color", color);

        op
    }

    /// Open the staged session; the stage needs the dialect
    /// spoken. The screen's size never comes from the metrics
    /// here: the stage is pinned to the art's own space and the
    /// display scales it. Only the box is taken.
    fn begin_staged(&mut self, stanza: &Object) -> Result<(), VoxamError> {
        let support: Vec<String> = match stanza.get("support") {
            Some(Value::List(held)) => held
                .iter()
                .filter_map(|word| word.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        let supports = |word: &str| support.iter().any(|held| held == word);

        if !supports("stage") {
            return Err(glkote_error(
                "the display never learned the stage; the Version 6 screen needs the \
                 dialect's own word"
                    .into(),
            ));
        }

        self.has_timed_input = supports("timer");
        self.speaks_sound = supports("sound");
        self.has_sounds = self.speaks_sound
            && self
                .resources
                .as_ref()
                .is_some_and(|held| held.blorb.is_some());

        self.sized(stanza)?;

        Ok(())
    }

    /// Drain the stage's pending paints into the cycle's ops.
    ///
    /// Called ahead of every picture op too, so the canvas keeps
    /// the turn's true order -- text written before a picture
    /// lands under it, text after lands over.
    fn stage_flowed(&mut self) {
        let Some(stage) = self.stage.as_mut() else {
            return;
        };
        let paints = stage.stage.paints();

        stage.ops.extend(oped(&paints, CELL, &stage.minted));
        stage.stage.sweep();
    }

    /// §8.3.1's -1 as the colour showing under the cursor.
    ///
    /// The painted stage itself answers, as the glass's real pixel
    /// does: the drawn ops walked newest-first, an image's own
    /// pixel or a fill's colour, and the found colour minted as a
    /// code past the named ones -- how Zork Zero's status text
    /// sits on its ribbons without a seam.
    fn stage_sampled(&mut self, code: i32) -> Result<i32, VoxamError> {
        if code != PIXEL_COLOUR {
            return Ok(code);
        }

        self.stage_flowed();

        let stage = self.stage.as_mut().expect("a staged face samples");
        let (line, column) = stage.stage.screen_cursor();
        let css = plotted(
            &stage.journal,
            &stage.ops,
            i64::from(column) - 1,
            i64::from(line) - 1,
            stage.gallery.as_mut(),
        )?;

        Ok(minted_code(stage, css))
    }

    /// Fold the cycle's ops into the repaint journal.
    ///
    /// A fill covering the whole stage starts the journal over, a
    /// setcolor restated ahead of it so a rescaled canvas's clear
    /// wears the right paper. Games repaper the stage at every
    /// scene, so the journal stays a scene deep.
    fn stage_journaled(&mut self, ops: &[Object]) {
        let (width, height) = self.stage_logical();
        let stage = self.stage.as_mut().expect("a staged face journals");

        for op in ops {
            if op.get("special").and_then(Value::as_str) == Some("fill")
                && op.get("x").and_then(Value::as_int) == Some(0)
                && op.get("y").and_then(Value::as_int) == Some(0)
                && op.get("width").and_then(Value::as_int) == Some(width)
                && op.get("height").and_then(Value::as_int) == Some(height)
            {
                let mut setcolor = Object::new();

                setcolor.set("special", "setcolor");
                setcolor.set("color", op.get("color").cloned().unwrap_or(Value::Null));

                stage.journal = vec![setcolor, op.clone()];
            } else {
                stage.journal.push(op.clone());
            }
        }
    }

    /// A picture's drawn height and width (§15 picture_data).
    ///
    /// The Reso arithmetic is the gallery's, exactly as at the
    /// glass -- though on a stage pinned to the standard window
    /// the Elbow Room Factor is one, and only each picture's own
    /// standard ratio remains (Blorb: The Resolution Chunk).
    fn stage_picture_data(&self, number: u16) -> Result<Option<(u16, u16)>, VoxamError> {
        let Some(gallery) = self.stage.as_ref().and_then(|held| held.gallery.as_ref()) else {
            return Ok(None);
        };
        let Some((height, width)) = gallery.size(u32::from(number))? else {
            return Ok(None);
        };
        let (logical_w, logical_h) = self.stage_logical();
        let factor = gallery.scale(u32::from(number), logical_w as u32, logical_h as u32);
        let drawn = |value: u32| -> u16 {
            ((i64::from(value) * factor.numerator()) / factor.denominator()) as u16
        };

        Ok(Some((drawn(height), drawn(width))))
    }

    /// §15 draw_picture as an image op at its unit position.
    ///
    /// A Rect placard has no bytes to send and draws nothing --
    /// invisible by design, its size still spoken for layout. The
    /// plotting runs through the gallery's adaptive-palette seam:
    /// a scene's plot absorbs its palette, the chrome wears the
    /// Current Palette -- and a plot that changes the palette
    /// re-plots the standing chrome in it, the wire's spelling of
    /// the hardware recolouring Infocom's interpreters did (Blorb:
    /// The Adaptive Palette Chunk).
    fn stage_draw_picture(
        &mut self,
        number: u16,
        line: u16,
        column: u16,
    ) -> Result<(), VoxamError> {
        if self
            .stage
            .as_ref()
            .is_none_or(|held| held.gallery.is_none())
        {
            return Ok(());
        }

        let Some((height, width)) = self.stage_picture_data(number)? else {
            return Ok(());
        };
        let Some(url) = self.stage_pictured(u32::from(number))? else {
            return Ok(());
        };

        self.stage_flowed();

        let stage = self.stage.as_mut().expect("a staged face draws");
        let mut op = Object::new();

        op.set("special", "image");
        op.set("image", i64::from(number));
        op.set("url", url);
        op.set("x", i64::from(column) - 1);
        op.set("y", i64::from(line) - 1);
        op.set("width", i64::from(width));
        op.set("height", i64::from(height));
        stage.ops.push(op);

        let serial = stage.gallery.as_ref().expect("checked above").serial();
        let adaptive = stage
            .gallery
            .as_ref()
            .expect("checked above")
            .adaptive()
            .contains(&u32::from(number));

        if adaptive {
            match stage
                .chrome
                .iter_mut()
                .find(|(held, _)| *held == u32::from(number))
            {
                Some((_, seat)) => *seat = (line, column),
                None => stage.chrome.push((u32::from(number), (line, column))),
            }
        }

        let redress = serial != stage.palette_serial;

        if redress {
            stage.palette_serial = serial;

            self.stage_redressed()?;
        }

        Ok(())
    }

    /// The picture plotted for the wire, its palette truly worn.
    ///
    /// The gallery decodes through the adaptive dance and the
    /// plotted pixels are re-encoded whole -- a display handed an
    /// adaptive stub's own bytes would paint the placeholder
    /// palette. Encodings are remembered per palette era, so the
    /// chrome only pays its decode bill again when a scene
    /// re-dresses it.
    fn stage_pictured(&mut self, number: u32) -> Result<Option<String>, VoxamError> {
        let stage = self.stage.as_mut().expect("a staged face plots");
        let Some(gallery) = stage.gallery.as_mut() else {
            return Ok(None);
        };
        let Some(picture) = gallery.picture(number)? else {
            return Ok(None);
        };
        let era = if gallery.adaptive().contains(&number) {
            i64::from(gallery.serial())
        } else {
            -1
        };

        if stage.urls.get(&number).is_none_or(|(held, _)| *held != era) {
            let spelled = b64(&png::encoded(&picture));

            stage
                .urls
                .insert(number, (era, format!("data:image/png;base64,{spelled}")));
        }

        Ok(Some(stage.urls[&number].1.clone()))
    }

    /// Re-plot the standing chrome in the fresh Current Palette.
    ///
    /// Infocom's interpreters recoloured the chrome through the
    /// hardware palette without replotting; the wire has no
    /// palette hardware, so the chrome replots -- the same
    /// positions, the new dress.
    fn stage_redressed(&mut self) -> Result<(), VoxamError> {
        let standing = self
            .stage
            .as_ref()
            .expect("a staged face redresses")
            .chrome
            .clone();

        for (number, (line, column)) in standing {
            self.stage_draw_picture(number as u16, line, column)?;
        }

        Ok(())
    }

    /// §15 erase_picture: the picture's rectangle, papered over.
    fn stage_erase_picture(
        &mut self,
        number: u16,
        line: u16,
        column: u16,
    ) -> Result<(), VoxamError> {
        let Some((height, width)) = self.stage_picture_data(number)? else {
            return Ok(());
        };

        self.stage_flowed();

        let stage = self.stage.as_mut().expect("a staged face erases");
        let color = coloured(stage.stage.background(), &stage.minted, PAPER_DEFAULT);
        let mut op = Object::new();

        op.set("special", "fill");
        op.set("x", i64::from(column) - 1);
        op.set("y", i64::from(line) - 1);
        op.set("width", i64::from(width));
        op.set("height", i64::from(height));
        op.set("color", color);
        stage.ops.push(op);

        Ok(())
    }

    // -- the conversation's opening ----------------------------------------

    /// Open the session on the init event's word.
    ///
    /// The screen's size in cells is settled here, before any
    /// machine is booted over this display -- the header reads it
    /// once at boot (§8.4). Fails when the metrics carry no size.
    pub fn begin(&mut self, stanza: &Object) -> Result<(), VoxamError> {
        if self.stage.is_some() {
            return self.begin_staged(stanza);
        }

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

    /// The typed line and its newline, in the input dress -- or,
    /// on a stage, onto the stage at the read's own cursor: a
    /// Version 6 interpreter echoes into the window itself
    /// (§7.1.2). The wire's editor showed the typing, and the
    /// landed line prints here so the screen keeps it.
    fn echoed(&mut self, line: &str) {
        if let Some(stage) = self.stage.as_mut() {
            stage.stage.write(&format!("{line}\n"));

            return;
        }

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
    ///
    /// The columns read are the model's own where an arrange has
    /// grown the display past them: the model keeps its boot size
    /// until a reload boots a machine at the new one (§8.4), so a
    /// wider measure shows the boot-sized grid rather than reading
    /// past its edge. (The reference reads the new width and falls
    /// over the edge -- a latent crash the desktop shell's Measure
    /// menu found -- so this is the one arrange path with no
    /// reference behavior to mirror.)
    fn faced(&mut self, rows: usize) -> Vec<Vec<TextRun>> {
        let mut face: Vec<Vec<TextRun>> = Vec::new();
        let columns = usize::from(self.screen_columns).min(self.model.columns());

        for row in 1..=rows {
            let mut spans: Vec<TextRun> = Vec::new();

            for column in 1..=columns {
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

        if let Some(stage) = face.stage.as_mut() {
            stage.stage.write(text);

            return;
        }

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

        if let Some(stage) = face.stage.as_mut() {
            stage.stage.write_rectangle(rows);

            return;
        }

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
        let result = match face.stage.as_ref() {
            // §8.2 has no line on a stage; the model says so
            // loudly.
            Some(stage) => stage.stage.show_status(status),
            None => face.model.show_status(status),
        };

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

        if let Some(stage) = face.stage.as_mut() {
            // §8.7.1: the stage keeps every window's own dress.
            stage.stage.set_style(style);

            return;
        }

        face.model.set_style(style);
        face.style = if style == 0 { 0 } else { face.style | style };
    }

    /// Fonts route to the model; the dress keys on styles.
    fn set_font(&mut self, font: u16) {
        let mut face = self.face.borrow_mut();

        match face.stage.as_mut() {
            Some(stage) => stage.stage.set_font(font),
            None => face.model.set_font(font),
        }
    }

    /// The display wraps for itself; the model need not -- but the
    /// stage wraps for itself, so its buffering is real (§7.2.2).
    fn set_buffering(&mut self, buffered: bool) {
        if let Some(stage) = self.face.borrow_mut().stage.as_mut() {
            stage.stage.set_buffering(buffered);
        }
    }

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

        if let Some(stage) = face.stage.as_mut() {
            // §8.8.4.1's tiling, the stage's own arithmetic.
            // Splitting clears nothing on a stage, so a quote
            // box's pixels stand without any high-water courtesy.
            stage.stage.split_window(i32::from(lines));

            return;
        }

        let result = face.model.split_window(i32::from(lines));

        face.noted(result);

        if face.version > STATUS_FLAGS_VERSION {
            face.peak_split = face.peak_split.max(usize::from(lines));
        }
    }

    /// Select the window taking the next printing (§8.7.2).
    fn set_window(&mut self, window: u16) {
        let mut face = self.face.borrow_mut();
        let result = match face.stage.as_mut() {
            // Select among all eight (§8.8.3).
            Some(stage) => stage.stage.set_window(i32::from(window)),
            None => face.model.set_window(window),
        };

        face.noted(result);
    }

    /// Place the upper window's cursor (§8.7.2.3).
    fn set_cursor(&mut self, line: u16, column: u16) {
        let mut face = self.face.borrow_mut();

        if let Some(stage) = face.stage.as_mut() {
            // The selected window's cursor, in its own units.
            stage.stage.set_cursor(i32::from(line), i32::from(column));

            return;
        }

        let result = face.model.set_cursor(line, column);

        face.noted(result);
    }

    /// What get_cursor reads back: the model's own ledger.
    fn cursor_position(&self) -> (u16, u16) {
        let mut face = self.face.borrow_mut();

        if let Some(stage) = face.stage.as_mut() {
            let (line, column) = stage.stage.get_cursor();

            return (line as u16, column as u16);
        }

        let (line, column) = face.model.get_cursor();

        (line as u16, column as u16)
    }

    /// An erasure of the lower half clears the buffer whole.
    ///
    /// The whole-screen forms are a deliberate teardown, not a
    /// quote box: the high water recedes with the split
    /// (§8.7.3.3).
    fn erase_window(&mut self, window: i32) {
        let mut face = self.face.borrow_mut();

        if face.stage.is_some() {
            // §8.7.3: the stage fills; its paint carries the
            // erasure. A whole-screen erasure takes the drawn
            // chrome with it, as the glass's does -- nothing is
            // left to re-dress.
            let stage = face.stage.as_mut().expect("staged");
            let result = stage.stage.erase_window(window).map(|_| ());

            if window < 0 {
                stage.chrome.clear();
            }

            face.noted(result);

            return;
        }

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

    /// To the end of the line -- meaningful in the grid alone,
    /// though the stage honours the Version 6 pixel-width form too
    /// (§8.8.5.2).
    fn erase_line(&mut self, pixels: Option<i32>) {
        let mut face = self.face.borrow_mut();

        if let Some(stage) = face.stage.as_mut() {
            stage.stage.erase_line(pixels);

            return;
        }

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

        if face.stage.is_some() {
            // §8.3.1, the under-cursor sample resolved to a cell
            // truth.
            let sampled = face
                .stage_sampled(foreground)
                .and_then(|fg| face.stage_sampled(background).map(|bg| (fg, bg)));

            match sampled {
                Ok((fg, bg)) => {
                    face.stage
                        .as_mut()
                        .expect("staged")
                        .stage
                        .set_colour(fg, bg);
                }
                Err(error) => face.noted(Err(error)),
            }

            return;
        }

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

    fn has_pictures(&self) -> bool {
        self.face
            .borrow()
            .stage
            .as_ref()
            .is_some_and(|held| held.has_pictures)
    }

    fn has_stage(&self) -> bool {
        self.face.borrow().stage.is_some()
    }

    fn font_width(&self) -> u16 {
        if self.face.borrow().stage.is_some() {
            CELL as u16
        } else {
            1
        }
    }

    fn font_height(&self) -> u16 {
        if self.face.borrow().stage.is_some() {
            CELL as u16
        } else {
            1
        }
    }

    /// A picture's drawn height and width (§15 picture_data),
    /// Reso-scaled by the gallery's own arithmetic.
    fn picture_data(&self, number: u16) -> Option<(u16, u16)> {
        let mut face = self.face.borrow_mut();

        match face.stage_picture_data(number) {
            Ok(held) => held,
            Err(error) => {
                face.noted(Err(error));

                None
            }
        }
    }

    /// The count of drawable pictures and the art's release.
    fn picture_census(&self) -> (u16, u16) {
        let face = self.face.borrow();

        match face.stage.as_ref().and_then(|held| held.gallery.as_ref()) {
            Some(gallery) => (gallery.count() as u16, gallery.release),
            None => (0, 0),
        }
    }

    fn draw_picture(&mut self, number: u16, line: u16, column: u16) {
        let mut face = self.face.borrow_mut();
        let result = face.stage_draw_picture(number, line, column);

        face.noted(result);
    }

    fn erase_picture(&mut self, number: u16, line: u16, column: u16) {
        let mut face = self.face.borrow_mut();

        if face.stage.is_none() {
            return;
        }

        let result = face.stage_erase_picture(number, line, column);

        face.noted(result);
    }

    /// §8.8 geometry, forwarded whole to the stage's ledger.
    fn place_window(&mut self, window: u16, line: u16, column: u16, height: u16, width: u16) {
        let mut face = self.face.borrow_mut();

        if face.stage.is_none() {
            return;
        }

        let result = face.stage.as_mut().expect("staged").stage.place_window(
            i32::from(window),
            i32::from(line),
            i32::from(column),
            i32::from(height),
            i32::from(width),
        );

        face.noted(result);
    }

    /// §15 scroll_window, in units.
    fn scroll_window(&mut self, window: u16, pixels: i32) {
        let mut face = self.face.borrow_mut();

        if face.stage.is_none() {
            return;
        }

        let result = face
            .stage
            .as_mut()
            .expect("staged")
            .stage
            .scroll_window(i32::from(window), pixels);

        face.noted(result);
    }

    /// §8.8.3.2.1 margins, in units.
    fn set_margins(&mut self, window: u16, left: u16, right: u16) {
        let mut face = self.face.borrow_mut();

        if face.stage.is_none() {
            return;
        }

        let result = face.stage.as_mut().expect("staged").stage.set_margins(
            i32::from(window),
            i32::from(left),
            i32::from(right),
        );

        face.noted(result);
    }

    /// §8.8.3.2.6's budget, the game's own hand on it.
    fn set_line_count(&mut self, window: u16, count: i32) {
        let mut face = self.face.borrow_mut();

        if face.stage.is_none() {
            return;
        }

        let result = face
            .stage
            .as_mut()
            .expect("staged")
            .stage
            .set_line_count(i32::from(window), count);

        face.noted(result);
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
/// The §8.8 stage for Version 6, the two-window picture for every
/// other version -- one seam, so the CLI and the web shell route
/// alike. Fails only when a stage Blorb's art census cannot be
/// hung.
pub fn fronted(version: u8, resources: Option<Resources>) -> Result<GlkOteFrontend, VoxamError> {
    if version == STAGE_VERSION {
        return GlkOteFrontend::staged(version, resources);
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
        Self::open_claimed(story, frontend, seed, Identity::default())
    }

    /// Boot with a claimed identity: the platform number and the
    /// legendary Tandy bit the CLI's own flags carry (S11.1.3-4).
    pub fn open_claimed(
        story: Story,
        frontend: GlkOteFrontend,
        seed: Option<u32>,
        identity: Identity,
    ) -> Result<Self, VoxamError> {
        let face = Rc::new(RefCell::new(frontend));
        let machine = Machine::new(
            story,
            Box::new(SharedFace { face: face.clone() }),
            seed,
            identity,
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

        if self.face.borrow().stage.is_some() {
            return self.render_staged(exit);
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

        clocked(&self.machine, &mut self.last_read, &mut face);
        face.sung();

        let refresh = std::mem::replace(&mut face.refresh_owed, false);

        face.page.update(exit, refresh)
    }

    /// Compose the stage into one scaled canvas update.
    ///
    /// The window entry names the art's logical space under the
    /// display's box; the cycle's draw ops travel in the turn's
    /// true order, and a repaint replays the journal -- everything
    /// since the last whole-stage fill.
    fn render_staged(&mut self, exit: bool) -> Result<Object, VoxamError> {
        let mut face = self.face.borrow_mut();
        let (width, height) = face.size;
        let logical = face.stage_logical();
        let ident = match face.stage.as_ref().expect("staged").canvas_ident {
            Some(held) => held,
            None => {
                let minted = face.next_ident;

                face.next_ident += 1;
                face.stage.as_mut().expect("staged").canvas_ident = Some(minted);

                minted
            }
        };

        face.page.window(
            ident,
            "graphics",
            0,
            (0, 0, width, height),
            WindowSpec {
                graphsize: Some(logical),
                scaled: true,
                ..WindowSpec::default()
            },
        )?;

        face.stage_flowed();

        let held = face.stage.as_ref().expect("staged").ops.clone();

        face.stage_journaled(&held);

        let refresh = std::mem::replace(&mut face.refresh_owed, false);
        let stage = face.stage.as_mut().expect("staged");
        let repaint = std::mem::replace(&mut stage.repaint_owed, false) || refresh;
        let ops = if repaint {
            stage.ops.clear();

            stage.journal.clone()
        } else {
            std::mem::take(&mut stage.ops)
        };

        if !ops.is_empty() {
            face.page.draw(ident, ops)?;
        }

        match self.machine.waiting() {
            Some(Waiting::File(filing)) => {
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
                // An input pause rests the scroll budgets, as
                // every face's read does (§8.8.3.2.6).
                face.stage.as_mut().expect("staged").stage.rest();

                if held.wants == Wants::Line {
                    let (line, column) = face.stage.as_mut().expect("staged").stage.screen_cursor();
                    let mut codes: Vec<u16> = held.terminators.iter().copied().collect();

                    codes.sort_unstable();

                    // The editor writes in the window's own ink --
                    // without it the field wears the browser's
                    // default black, invisible on a dark stage.
                    let ink = {
                        let stage = face.stage.as_ref().expect("staged");

                        coloured(stage.stage.foreground(), &stage.minted, INK_DEFAULT)
                    };

                    face.page.line_input(
                        ident,
                        held.capacity as i64,
                        LineSpec {
                            terminators: codes.into_iter().filter_map(terminator_name).collect(),
                            cursor: Some((i64::from(column) - 1, i64::from(line) - 1)),
                            cell: Some((CELL, CELL)),
                            ink: Some(ink),
                            mouse: held.terminators.contains(&SINGLE_CLICK),
                            ..LineSpec::default()
                        },
                    )?;
                } else {
                    // A keystroke read hears a click the way it
                    // hears any key (§10.3.3).
                    face.page.char_input(ident, None, false, true)?;
                }
            }
            None => {}
        }

        clocked(&self.machine, &mut self.last_read, &mut face);
        face.sung();

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
        if self.face.borrow().stage.is_some() {
            return self.stage_pointed(stanza);
        }

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

    /// A click on the stage, §10.3-spelled in units.
    ///
    /// The canvas hears clicks in the stage's own logical units,
    /// 0-based; the header extension counts from (1,1), one step
    /// over (§10.3.2).
    fn stage_pointed(&mut self, stanza: &Object) -> Result<Verdict, VoxamError> {
        let canvas = self
            .face
            .borrow()
            .stage
            .as_ref()
            .expect("staged")
            .canvas_ident;

        let Some(canvas) = canvas else {
            return Ok(Verdict::Pass);
        };

        if stanza.get("window").and_then(Value::as_int) != Some(canvas) {
            return Ok(Verdict::Pass);
        }

        if !matches!(self.machine.waiting(), Some(Waiting::Read(_))) {
            // No read stands to hear a click: the misaimed-event
            // shrug, as at the grid.
            return Ok(Verdict::Pass);
        }

        let typed = partials(stanza.get("partial"))
            .get(&canvas)
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
        if self.face.borrow().stage.is_some() {
            // The stage's units never move with the display's box,
            // so the machine hears nothing of an arrange -- and a
            // redraw means the display cleared its rescaled
            // canvas, so the whole journal is owed (GlkOte: Redraw
            // Events).
            let mut face = self.face.borrow_mut();

            if kind == "redraw" {
                face.stage.as_mut().expect("staged").repaint_owed = true;

                return Ok(Verdict::Stand);
            }

            face.sized(stanza)?;

            return Ok(Verdict::Stand);
        }

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
    serve_claimed(story, frontend, reader, writer, seed, Identity::default())
}

/// Serve with a claimed identity, the CLI flags' own seam.
pub fn serve_claimed(
    story: Story,
    frontend: GlkOteFrontend,
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
    seed: Option<u32>,
    identity: Identity,
) -> bool {
    match served(story, frontend, reader, writer, seed, identity) {
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
    identity: Identity,
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

    let mut session = Session::open_claimed(story, frontend, seed, identity)?;

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

/// The cycle's timer field, from the standing wait: a fresh timed
/// read restarts the display's clock even at the same cadence, as
/// §15 restarts its own.
fn clocked(machine: &Machine, last_read: &mut Option<u64>, face: &mut GlkOteFrontend) {
    let serial = machine.wait_serial();

    match machine.waiting() {
        Some(Waiting::Read(held)) if held.time != 0 && held.routine != 0 => {
            face.page
                .timer(i64::from(held.time) * TENTH_MS, *last_read != Some(serial));

            *last_read = Some(serial);
        }
        Some(_) => {
            face.page.timer(0, false);

            *last_read = Some(serial);
        }
        None => {
            face.page.timer(0, false);

            *last_read = None;
        }
    }
}

/// The code a sampled colour wears, minted once per colour.
fn minted_code(stage: &mut StageHalf, css: String) -> i32 {
    if let Some(&held) = stage.codes.get(&css) {
        return held;
    }

    let held = stage.next_code;

    stage.next_code += 1;
    stage.codes.insert(css.clone(), held);
    stage.minted.insert(held, css);

    held
}

/// Stage paints as the dialect's draw ops, 0-based on the canvas.
///
/// Text paints arrive one dressed character at a time; runs along
/// a row in the same dress coalesce into one op, the wire staying
/// light. Fills and shifts translate one to one, the §8.3.1 codes
/// becoming the shared palette's CSS -- the minted sampled colours
/// included.
fn oped(paints: &[Paint], cell: i64, minted: &HashMap<i32, String>) -> Vec<Object> {
    let mut ops: Vec<Object> = Vec::new();

    for paint in paints {
        match paint {
            Paint::Text(held) => {
                let op = texted(held, cell, minted);
                let continued = ops.last().is_some_and(|last| joins(last, &op, cell));

                if continued {
                    let last = ops.last_mut().expect("checked above");
                    let text = format!(
                        "{}{}",
                        last.get("text").and_then(Value::as_str).unwrap_or_default(),
                        op.get("text").and_then(Value::as_str).unwrap_or_default()
                    );

                    last.set("text", text);
                } else {
                    ops.push(op);
                }
            }
            Paint::Fill(held) => {
                let mut op = Object::new();

                op.set("special", "fill");
                op.set("x", i64::from(held.column) - 1);
                op.set("y", i64::from(held.line) - 1);
                op.set("width", i64::from(held.width));
                op.set("height", i64::from(held.height));
                op.set("color", coloured(held.background, minted, PAPER_DEFAULT));
                ops.push(op);
            }
            Paint::Shift(held) => {
                let mut op = Object::new();

                op.set("special", "shift");
                op.set("x", i64::from(held.column) - 1);
                op.set("y", i64::from(held.line) - 1);
                op.set("width", i64::from(held.width));
                op.set("height", i64::from(held.height));
                op.set("rise", i64::from(held.rise));
                ops.push(op);
            }
        }
    }

    ops
}

/// One placed character as a text op, reverse pre-swapped.
///
/// The dress travels resolved: ink and paper as CSS with the
/// stage's own defaults for code 1, reverse video already swapped,
/// bold and italic as the op's flags (§8.7.1).
fn texted(paint: &TextPaint, cell: i64, minted: &HashMap<i32, String>) -> Object {
    let held = paint.cell;
    let mut ink = coloured(held.foreground, minted, INK_DEFAULT);
    let mut paper = coloured(held.background, minted, PAPER_DEFAULT);

    if held.style & REVERSE != 0 {
        std::mem::swap(&mut ink, &mut paper);
    }

    let mut op = Object::new();

    op.set("special", "text");
    op.set("x", i64::from(paint.column) - 1);
    op.set("y", i64::from(paint.line) - 1);
    op.set("text", held.character.to_string());
    op.set(
        "cell",
        Value::List(vec![Value::Int(cell), Value::Int(cell)]),
    );
    op.set("fg", ink);
    op.set("bg", paper);

    if held.style & BOLD != 0 {
        op.set("bold", true);
    }

    if held.style & ITALIC != 0 {
        op.set("italic", true);
    }

    op
}

/// Whether a fresh text op continues the last one's run.
fn joins(last: &Object, op: &Object, cell: i64) -> bool {
    let length = last
        .get("text")
        .and_then(Value::as_str)
        .map_or(0, |text| text.chars().count() as i64);

    last.get("special").and_then(Value::as_str) == Some("text")
        && op.get("y") == last.get("y")
        && op.get("x").and_then(Value::as_int)
            == last
                .get("x")
                .and_then(Value::as_int)
                .map(|x| x + cell * length)
        && ["fg", "bg", "bold", "italic"]
            .iter()
            .all(|key| op.get(key) == last.get(key))
}

/// The colour showing at a canvas point, newest paint first.
///
/// §8.3.1's sample asks for the pixel under the cursor, and the
/// drawn ops are the stage's pixels: an image's own pixel answers
/// -- a transparent hole deferring to what shows through beneath
/// -- a fill answers its colour, and paint never laid answers the
/// stage's default paper. Text ops are passed over: a game samples
/// its art and its fills, not its letters.
fn plotted(
    journal: &[Object],
    ops: &[Object],
    x: i64,
    y: i64,
    mut gallery: Option<&mut Gallery>,
) -> Result<String, VoxamError> {
    for op in ops.iter().rev().chain(journal.iter().rev()) {
        let special = op
            .get("special")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if special == "image" {
            if let Some(gallery) = gallery.as_deref_mut()
                && let Some(css) = art_pixel(op, x, y, gallery)?
            {
                return Ok(css);
            }
        } else if special == "fill" && within(op, x, y) {
            return Ok(op
                .get("color")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string());
        }
    }

    Ok(PAPER_DEFAULT.to_string())
}

/// One drawn image's pixel at a canvas point, None to look on.
///
/// The point maps back through the op's drawn size to the art's
/// own pixels -- Reso scaling undone by the same ratio that
/// applied it -- and a fully transparent pixel defers to whatever
/// the point shows through to.
fn art_pixel(
    op: &Object,
    x: i64,
    y: i64,
    gallery: &mut Gallery,
) -> Result<Option<String>, VoxamError> {
    if !within(op, x, y) {
        return Ok(None);
    }

    let number = op.get("image").and_then(Value::as_int).unwrap_or_default();
    let Some(picture) = gallery.picture(number as u32)? else {
        return Ok(None);
    };
    let corner = |key: &str| op.get(key).and_then(Value::as_int).unwrap_or_default();
    let px = (x - corner("x")) * i64::from(picture.width) / corner("width");
    let py = (y - corner("y")) * i64::from(picture.height) / corner("height");

    if let Some(clear) = &picture.clear
        && clear[py as usize][px as usize]
    {
        return Ok(None);
    }

    let (red, green, blue) = picture.rows[py as usize][px as usize];

    Ok(Some(format!("#{red:02x}{green:02x}{blue:02x}")))
}

/// Whether a drawn op's rectangle covers a canvas point.
fn within(op: &Object, x: i64, y: i64) -> bool {
    let corner = |key: &str| op.get(key).and_then(Value::as_int).unwrap_or_default();

    corner("x") <= x
        && x < corner("x") + corner("width")
        && corner("y") <= y
        && y < corner("y") + corner("height")
}

/// A colour code as CSS, the minted samples consulted first --
/// then the shared palette, and the stage's own default for
/// everything else, code 1 included.
fn coloured(code: i32, minted: &HashMap<i32, String>, default: &str) -> String {
    if let Some(held) = minted.get(&code) {
        return held.clone();
    }

    css(code).unwrap_or_else(|| default.to_string())
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
