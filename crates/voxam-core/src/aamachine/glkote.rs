//! The Å-machine over the GlkOte wire: the document in a buffer.
//!
//! The face is deliberately the reference terminal's document
//! model on the wire: one buffer window carries the whole telling
//! through the certified plain voice at width zero -- the display
//! does the wrapping -- and the status areas stay honestly
//! unclaimed, the way the reference Node frontend leaves them. A
//! line wait asks for a line, a key wait for a keystroke, and the
//! story's META bibliography opens the page as the doorway card,
//! the house courtesy every machine's face extends.
//!
//! The document travels dressed: the wardrobe's bold and italic
//! ride the display's own stock styles -- subheader and
//! emphasized, with alert for both at once, which the
//! specification's bar permits to equal bold -- and the sheet's
//! colors ride as per-span ink under the display's colors grant,
//! the same dialect word the Z-Machine's §8.3 colors travel by.
//! VM_INFO answers the styling question with yes on any display
//! and the color question with the grant's own truth (Aa-machine:
//! VM_INFO).
//!
//! Savefiles stay with the blocking faces for now: a save over the
//! wire needs the suspended-file dance the Z-Machine's Filing wait
//! performs, and that is a named road, not this rung.
//!
//! One reshaping from the reference, in the wire faces' standing
//! manner: the reference's face holds the voice the machine also
//! holds -- the same object, twice referenced. Here the machine
//! owns its voice outright (`Machine<WireVoice>`), so the face
//! holds none, and begin, render, and accept take the voice or the
//! machine as arguments.

use std::io::{BufRead, Write};

use crate::aamachine::machine::{Machine, Wait};
use crate::aamachine::output::{Outfit, PlainVoice, Voice, Wardrobe};
use crate::aamachine::story::Story;
use crate::errors::VoxamError;
use crate::glkote::json::{Object, Value};
use crate::glkote::{
    Ink, LineSpec, Page, Run, TextRun, WindowSpec, partials, read_stanza, write_stanza,
};

/// The verdicts accept hands back: run on, redraw the standing
/// wait, or answer the protocol's pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Advance,
    Stand,
    Pass,
}

/// The one window the document lives in.
const BUFFER: i64 = 1;

/// The longest line the input field accepts, the wire's own cap.
const CAPACITY: i64 = 256;

/// A reserved keypress code by its GlkOte name (Aa-machine: Text;
/// GlkOte: Char Input Events).
fn named_key(value: &str) -> Option<u32> {
    match value {
        "return" => Some(0x0D),
        "delete" => Some(0x08),
        "up" => Some(0x10),
        "down" => Some(0x11),
        "left" => Some(0x12),
        "right" => Some(0x13),
        _ => None,
    }
}

/// Events that never carry a partial-input field.
const NO_PARTIAL: [&str; 4] = ["init", "specialresponse", "refresh", "debuginput"];

/// The bare outfit every session opens in.
const PLAIN: Outfit = (false, false, false, None, None);

fn glkote_error(message: String) -> VoxamError {
    VoxamError::GlkOte(message)
}

/// The wire's voice: the plain document, its dress marked.
///
/// The telling stays exactly the certified document at width zero;
/// each style change lands as a mark -- an offset into the telling
/// and the outfit worn from there -- and the face cuts the drained
/// text into styled runs along them. Styling is always claimable
/// on the wire, the display's stock styles rendering bold and
/// italic; color waits on the display's own grant, which the face
/// sets at begin.
pub struct WireVoice {
    plain: PlainVoice,
    wardrobe: Wardrobe,
    /// The outfit changes, each at its telling offset.
    pub marks: Vec<(usize, Outfit)>,
    /// The display's colors grant, set at begin.
    pub has_color: bool,
}

impl WireVoice {
    /// Speak at width zero; the display wraps.
    pub fn new(story: &Story) -> Result<Self, VoxamError> {
        let plain = PlainVoice::new(story)?.sized(0);
        let wardrobe = Wardrobe::new(&plain.styles);

        Ok(Self {
            plain,
            wardrobe,
            marks: Vec::new(),
            has_color: false,
        })
    }

    /// Everything said so far, the pending word flushed out.
    pub fn told(&mut self) -> &str {
        self.plain.told()
    }

    /// Note that a sent line's echo reset the cursor.
    pub fn prompted(&mut self) {
        self.plain.prompted();
    }

    /// Mark the outfit change at the telling's current end.
    fn fitted(&mut self) {
        self.plain.flushed();
        self.marks
            .push((self.plain.told.len(), self.wardrobe.folded()));
    }
}

impl Voice for WireVoice {
    fn has_styles(&self) -> bool {
        true
    }

    fn has_color(&self) -> bool {
        self.has_color
    }

    fn say(&mut self, text: &str) {
        self.plain.say(text);
    }

    fn nbsp(&mut self) {
        self.plain.nbsp();
    }

    fn space(&mut self) {
        self.plain.space();
    }

    fn spaces(&mut self, count: i64) {
        self.plain.spaces(count);
    }

    fn line(&mut self) {
        self.plain.line();
    }

    fn par(&mut self) {
        self.plain.par();
    }

    /// Open a div: the break as ever, then its class's dress.
    fn enter_div(&mut self, style: i64) {
        self.plain.enter_div(style);
        self.wardrobe.entered(style);
        self.fitted();
    }

    /// Close a div: the dress beneath first, then the break.
    fn leave_div(&mut self, style: i64) {
        self.wardrobe.left();
        self.fitted();
        self.plain.leave_div(style);
    }

    /// Open a span, wearing its class's dress.
    fn enter_span(&mut self, style: i64) {
        self.wardrobe.entered(style);
        self.fitted();
    }

    /// Close the span, the dress beneath restored.
    fn leave_span(&mut self) {
        self.wardrobe.left();
        self.fitted();
    }

    /// Dress the document body; every later dress layers on it.
    fn set_body(&mut self, style: i64) {
        self.wardrobe.bodied(style);
        self.fitted();
    }

    fn enter_status(&mut self, area: i64, style: i64) {
        self.plain.enter_status(area, style);
    }

    fn leave_status(&mut self) {
        self.plain.leave_status();
    }

    fn enter_link(&mut self, words: &str) {
        self.plain.enter_link(words);
    }

    fn leave_link(&mut self) {
        self.plain.leave_link();
    }

    fn enter_link_res(&mut self, resource: i64) {
        self.plain.enter_link_res(resource);
    }

    fn leave_link_res(&mut self) {
        self.plain.leave_link_res();
    }

    fn enter_self_link(&mut self) {
        self.plain.enter_self_link();
    }

    fn leave_self_link(&mut self) {
        self.plain.leave_self_link();
    }

    fn embed_res(&mut self, resource: i64) {
        self.plain.embed_res(resource);
    }

    fn can_embed_res(&self, resource: i64) -> bool {
        self.plain.can_embed_res(resource)
    }

    fn progress(&mut self, amount: i64, total: i64) {
        self.plain.progress(amount, total);
    }

    /// Turn on the deprecated style bits (Aa-machine: SET_STYLE).
    fn set_style(&mut self, bits: i64) {
        self.wardrobe.styled(bits);
        self.fitted();
    }

    /// Turn off the deprecated style bits.
    fn reset_style(&mut self, bits: i64) {
        self.wardrobe.unstyled(bits);
        self.fitted();
    }

    /// Return to the default text style.
    fn unstyle(&mut self) {
        self.wardrobe.bared();
        self.fitted();
    }

    fn clear(&mut self) {
        self.plain.clear();
    }

    fn clear_all(&mut self) {
        self.plain.clear_all();
    }

    fn clear_status(&mut self) {
        self.plain.clear_status();
    }

    fn clear_links(&mut self) {
        self.plain.clear_links();
    }

    fn clear_old(&mut self) {
        self.plain.clear_old();
    }

    fn clear_div(&mut self) {
        self.plain.clear_div();
    }

    /// Return to the initial state, the spans' dresses dropped.
    fn leave_all(&mut self) {
        self.plain.leave_all();
        self.wardrobe.dropped();
        self.fitted();
    }

    fn sync(&mut self) {
        self.plain.sync();
    }

    fn script_on(&mut self) -> bool {
        self.plain.script_on()
    }

    fn script_off(&mut self) {
        self.plain.script_off();
    }

    fn script_active(&self) -> bool {
        self.plain.script_active()
    }

    fn reset(&mut self) {
        self.plain.reset();
    }

    fn measured(&self, dimension: i64) -> i64 {
        self.plain.measured(dimension)
    }

    fn trace(&mut self, text: &str) {
        self.plain.trace(text);
    }

    fn save(&mut self, data: &[u8]) -> bool {
        self.plain.save(data)
    }

    fn restore(&mut self) -> Option<Vec<u8>> {
        self.plain.restore()
    }
}

/// One Å-machine session's face on the wire.
///
/// The machine speaks through the certified plain voice at width
/// zero, its telling drained into the buffer window a cycle at a
/// time; the face keeps only the wire's own state.
pub struct GlkOteFrontend {
    /// The update builder.
    pub page: Page,
    /// The machine's standing wait, kept here so render can ask
    /// for the right input; None before the machine first runs.
    pub waiting: Option<Wait>,
    mark: usize,
    size: (i64, i64),
    refresh: bool,
    // The sidecar seam: granted by the display's "voxam" token,
    // carrying the last line this face delivered (PORT: What the
    // sidecar carries).
    speaks_voxam: bool,
    last_command: Option<String>,
    outfit: Outfit,
    opening: Vec<Run>,
}

impl GlkOteFrontend {
    /// Ready the page for one story, its card at the door.
    pub fn new(story: &Story) -> Self {
        Self {
            page: Page::new(),
            waiting: None,
            mark: 0,
            size: (0, 0),
            refresh: false,
            outfit: PLAIN,
            opening: carded(story),
            speaks_voxam: false,
            last_command: None,
        }
    }

    /// Open the session on the init event's word. Fails when the
    /// metrics carry no size.
    pub fn begin(&mut self, voice: &mut WireVoice, stanza: &Object) -> Result<(), VoxamError> {
        let metrics = match stanza.get("metrics") {
            Some(Value::Object(held)) => held.clone(),
            _ => Object::new(),
        };
        let Some(width) = metrics.get("width").and_then(Value::as_float) else {
            return Err(glkote_error(
                "the init event's metrics carry no size (GlkOte: The Metrics Object)".into(),
            ));
        };
        let height = metrics
            .get("height")
            .and_then(Value::as_float)
            .unwrap_or(0.0);

        self.size = (width as i64, height as i64);

        // Color is the dialect's own word: per-span ink travels
        // only to a display that says it renders it, the same
        // grant the Z-Machine's colors ride (Aa-machine: VM_INFO).
        self.speaks_voxam = match stanza.get("support") {
            Some(Value::List(held)) => held.iter().any(|word| word.as_str() == Some("voxam")),
            _ => false,
        };
        voice.has_color = match stanza.get("support") {
            Some(Value::List(held)) => held.iter().any(|word| word.as_str() == Some("colors")),
            _ => false,
        };

        Ok(())
    }

    /// Compose everything told since the last update.
    pub fn render(&mut self, voice: &mut WireVoice, exit: bool) -> Result<Object, VoxamError> {
        self.render_with(voice, exit, None)
    }

    /// The voxam block, the Å-machine's honest share of it: no
    /// location or score registers to read, so only the wire's own
    /// facts travel -- the last delivered line and the machine's
    /// discontinuity bit, read once and rested, handed in by the
    /// serving loop (PORT: What the sidecar carries).
    pub fn sidecar(&mut self, discontinuity: &mut bool) -> Option<Object> {
        if !self.speaks_voxam {
            return None;
        }

        let mut block = Object::new();

        if let Some(command) = self.last_command.clone() {
            block.set("command", command);
        }

        if *discontinuity {
            *discontinuity = false;
            block.set("discontinuity", true);
        }

        Some(block)
    }

    /// The reference's render carries a default voxam argument;
    /// Rust spells the default as this delegating pair.
    pub fn render_with(
        &mut self,
        voice: &mut WireVoice,
        exit: bool,
        voxam: Option<Object>,
    ) -> Result<Object, VoxamError> {
        let (width, height) = self.size;

        self.page.window(
            BUFFER,
            "buffer",
            0,
            (0, 0, width, height),
            WindowSpec::default(),
        )?;

        let has_color = voice.has_color;
        let marks = std::mem::take(&mut voice.marks);
        let told = voice.told();
        let mut runs: Vec<Run> = std::mem::take(&mut self.opening);

        for (at, outfit) in marks {
            if at > self.mark {
                runs.push(dressed(&told[self.mark..at], self.outfit, has_color));
                self.mark = at;
            }

            self.outfit = outfit;
        }

        if !told[self.mark..].is_empty() {
            runs.push(dressed(&told[self.mark..], self.outfit, has_color));
        }

        self.mark = told.len();

        if !runs.is_empty() {
            self.page.buffer(BUFFER, &runs, false)?;
        }

        if !exit {
            match self.waiting {
                Some(Wait::Line) => {
                    self.page
                        .line_input(BUFFER, CAPACITY, LineSpec::default())?;
                }
                Some(Wait::Key) => {
                    self.page.char_input(BUFFER, None, false, false)?;
                }
                _ => {}
            }
        }

        let refresh = std::mem::replace(&mut self.refresh, false);

        self.page.update(exit, refresh, voxam)
    }

    /// Translate one event; a delivery runs the machine on.
    ///
    /// A misaimed event -- input the machine is not waiting for --
    /// earns the polite pass, never a fault: a stale display is a
    /// display to answer, not a session to end.
    pub fn accept(
        &mut self,
        machine: &mut Machine<WireVoice>,
        stanza: &Object,
    ) -> Result<Verdict, VoxamError> {
        let kind = stanza
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        if !NO_PARTIAL.contains(&kind.as_str()) {
            self.page.typed(partials(stanza.get("partial")));
        }

        if kind == "refresh" {
            self.refresh = true;

            return Ok(Verdict::Stand);
        }

        if kind == "arrange" {
            if let Some(Value::Object(metrics)) = stanza.get("metrics")
                && let Some(width) = metrics.get("width").and_then(Value::as_float)
            {
                let height = metrics
                    .get("height")
                    .and_then(Value::as_float)
                    .unwrap_or(0.0);

                self.size = (width as i64, height as i64);

                return Ok(Verdict::Stand);
            }

            return Ok(Verdict::Pass);
        }

        if kind == "line" && self.waiting == Some(Wait::Line) {
            let value = stringy(stanza.get("value"));

            machine.voice.prompted();
            self.waiting = Some(machine.deliver_line(&value)?);
            self.last_command = Some(value);

            return Ok(Verdict::Advance);
        }

        if kind == "char" && self.waiting == Some(Wait::Key) {
            let Some(code) = keyed(&stringy(stanza.get("value"))) else {
                return Ok(Verdict::Pass);
            };

            self.waiting = Some(machine.deliver_key(code)?);

            return Ok(Verdict::Advance);
        }

        Ok(Verdict::Pass)
    }
}

/// One run of telling, worn as the given outfit.
///
/// Bold rides the display's subheader style and italic its
/// emphasized; both at once ride alert, which the stock sheet
/// renders bold -- the specification allows bold italic to equal
/// either (Aa-machine: VM_INFO). The sheet's colors ride as the
/// dialect's per-span ink, under the display's own grant.
fn dressed(text: &str, outfit: Outfit, has_color: bool) -> Run {
    let (bold, italic, _, ink, paper) = outfit;
    let style = if bold && italic {
        "alert"
    } else if bold {
        "subheader"
    } else if italic {
        "emphasized"
    } else {
        "normal"
    };

    if has_color && (ink.is_some() || paper.is_some()) {
        let tint: Ink = (css(ink), css(paper));

        return Run::Text(TextRun::inked(style, 0, text, tint));
    }

    Run::text(style, 0, text)
}

/// An RGB tint as the CSS the ink rides in, None riding whole.
fn css(tint: Option<(i64, i64, i64)>) -> Option<String> {
    tint.map(|(r, g, b)| format!("rgb({r},{g},{b})"))
}

/// A char event's value as a machine keypress, or None.
fn keyed(value: &str) -> Option<u32> {
    let mut characters = value.chars();

    if let (Some(character), None) = (characters.next(), characters.next()) {
        return Some(u32::from(character));
    }

    named_key(value)
}

/// The META bibliography as the page's doorway card.
///
/// The title stands as a header, the author beneath it, and the
/// blurb as its own paragraphs -- the same card every face opens
/// with, drawn from the chunk instead of the treaty record
/// (Aa-machine: META).
fn carded(story: &Story) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();

    if let Some(title) = story.meta_field("title").filter(|held| !held.is_empty()) {
        runs.push(Run::text("header", 0, &format!("{title}\n")));
    }

    if let Some(author) = story.meta_field("author").filter(|held| !held.is_empty()) {
        runs.push(Run::text("emphasized", 0, &format!("by {author}\n")));
    }

    if let Some(blurb) = story.meta_field("blurb").filter(|held| !held.is_empty()) {
        runs.push(Run::text(
            "normal",
            0,
            &format!("{}\n", blurb.replace('\u{10}', "\n")),
        ));
    }

    if !runs.is_empty() {
        runs.push(Run::text("normal", 0, "\n"));
    }

    runs
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

/// Drive one Å session over the protocol, stanza by stanza.
///
/// The init comes first; thereafter the burst model: the machine
/// runs to a wait, the update goes out, the answer is delivered.
/// True is a session that ended cleanly; a broken conversation
/// answers the protocol's own error stanza and is false.
pub fn serve(
    story: Story,
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
    seed: Option<u32>,
) -> bool {
    match served(story, reader, writer, seed) {
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

    let mut face = GlkOteFrontend::new(&story);
    let mut voice = WireVoice::new(&story)?;

    face.begin(&mut voice, &opening)?;

    let mut machine = Machine::new(story, voice, seed)?;

    face.waiting = Some(machine.run(None)?);

    loop {
        let exit = face.waiting == Some(Wait::Quit);
        let voxam = face.sidecar(&mut machine.discontinuity);
        let update = face.render_with(&mut machine.voice, exit, voxam)?;

        write_stanza(writer, &update);

        if exit {
            return Ok(true);
        }

        loop {
            let Some(stanza) = read_stanza(reader)? else {
                return Ok(true);
            };

            match face.accept(&mut machine, &stanza)? {
                Verdict::Advance => break,
                Verdict::Stand => {
                    let voxam = face.sidecar(&mut machine.discontinuity);
                    let update = face.render_with(&mut machine.voice, false, voxam)?;

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

#[cfg(test)]
#[path = "glkote_tests.rs"]
mod tests;
