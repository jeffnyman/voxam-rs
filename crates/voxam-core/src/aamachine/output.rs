//! The Å-machine's output subsystem: the Voice the engine speaks through.
//!
//! The specification splits an interpreter into an engine and a
//! device-specific output subsystem joined by an API of output
//! calls (Aa-machine: Output model). Voice is that API as a trait,
//! and PlainVoice is the dumb-terminal subsystem: an 80-column
//! word-wrapping collector that ignores every status area,
//! matching the reference Node frontend's io object line for line
//! -- which is what lets a Voxam transcript diff clean against the
//! community fork's own gold output.
//!
//! Style classes arrive as indexes into the LOOK chunk's table;
//! the plain voice honors only the em-sized vertical margins, the
//! way the reference terminal does.
//!
//! One reshaping from the reference: the StyledVoice subclass and
//! its _fitted hook fold into the terminal voice that was its only
//! wearer -- the wardrobe itself stays here, face-neutral.

use crate::aamachine::story::{Story, asciied};
use crate::errors::VoxamError;

/// The columns a plain telling wraps at, the reference default.
pub const WIDTH: i64 = 80;

// A LOOK table opens with its two-byte style count.
const COUNT_SIZE: usize = 2;

fn output_error(message: String) -> VoxamError {
    VoxamError::AAMachine(message)
}

/// One style class's dress: CSS-shaped key-value pairs, in source
/// order, later spellings of a key overwriting in place.
pub type Pairs = Vec<(String, String)>;

fn valued<'a>(pairs: &'a Pairs, key: &str) -> &'a str {
    pairs
        .iter()
        .find(|(held, _)| held == key)
        .map_or("", |(_, value)| value.as_str())
}

/// The output API an Å-machine engine speaks through.
///
/// Every call mirrors one output_* function of the specification's
/// coupling API (Aa-machine: Output model). The has_* answers
/// mirror the VM_INFO interpreter-feature selectors that concern
/// output.
pub trait Voice {
    fn has_links(&self) -> bool {
        false
    }

    fn has_styles(&self) -> bool {
        false
    }

    fn has_color(&self) -> bool {
        false
    }

    fn has_alignment(&self) -> bool {
        false
    }

    fn has_top_status(&self) -> bool {
        false
    }

    fn has_inline_status(&self) -> bool {
        false
    }

    fn has_saves(&self) -> bool {
        false
    }

    /// Print text, wrappable at its spaces and hyphens.
    fn say(&mut self, text: &str);

    /// Print a space no wrap may break at.
    fn nbsp(&mut self);

    /// Print one breakable space.
    fn space(&mut self);

    /// Print a run of forced spaces.
    fn spaces(&mut self, count: i64);

    /// Break the line.
    fn line(&mut self);

    /// End the paragraph.
    fn par(&mut self);

    /// Open a div of the given style class.
    fn enter_div(&mut self, style: i64);

    /// Close the current div, its class restated.
    fn leave_div(&mut self, style: i64);

    /// Open a span of the given style class.
    fn enter_span(&mut self, style: i64);

    /// Close the current span.
    fn leave_span(&mut self);

    /// Dress the document body in a style class.
    fn set_body(&mut self, style: i64);

    /// Enter a status area, clearing it.
    fn enter_status(&mut self, area: i64, style: i64);

    /// Leave the status area.
    fn leave_status(&mut self);

    /// Open a link whose click types the given words.
    fn enter_link(&mut self, words: &str);

    /// Close the current link.
    fn leave_link(&mut self);

    /// Open a link to a resource.
    fn enter_link_res(&mut self, resource: i64);

    /// Close the resource link.
    fn leave_link_res(&mut self);

    /// Open a link whose click types its own text.
    fn enter_self_link(&mut self);

    /// Close the self link.
    fn leave_self_link(&mut self);

    /// Embed a resource in the stream.
    fn embed_res(&mut self, resource: i64);

    /// Whether the resource could be embedded.
    fn can_embed_res(&self, resource: i64) -> bool;

    /// Draw a progress bar at amount of total.
    fn progress(&mut self, amount: i64, total: i64);

    /// Turn on the deprecated style bits.
    fn set_style(&mut self, bits: i64);

    /// Turn off the deprecated style bits.
    fn reset_style(&mut self, bits: i64);

    /// Return to the default text style.
    fn unstyle(&mut self);

    /// Clear the main area, the div stack kept.
    fn clear(&mut self);

    /// Clear the main area and hide the status areas.
    fn clear_all(&mut self);

    /// Hide all status areas.
    fn clear_status(&mut self);

    /// Turn old links into static text.
    fn clear_links(&mut self);

    /// Clear text the player has already read.
    fn clear_old(&mut self);

    /// Clear or fold away the current div.
    fn clear_div(&mut self);

    /// Return to the initial output state.
    fn leave_all(&mut self);

    /// Bring the display up to date.
    fn sync(&mut self);

    /// Start a transcript; true on success.
    fn script_on(&mut self) -> bool;

    /// Stop the transcript.
    fn script_off(&mut self);

    /// Whether a transcript is running.
    fn script_active(&self) -> bool;

    /// Forget everything; the display starts over.
    fn reset(&mut self);

    /// The current div's width (0) or height (1) in characters.
    fn measured(&self, dimension: i64) -> i64;

    /// Print one debug tracepoint on its own line.
    fn trace(&mut self, text: &str);

    /// Keep a savefile; true on success.
    fn save(&mut self, data: &[u8]) -> bool;

    /// A previously kept savefile, None when there is none.
    fn restore(&mut self) -> Option<Vec<u8>>;
}

/// The dumb-terminal voice: 80 columns, no dress, no status.
///
/// Words buffer until a space or hyphen lets them break, a line
/// wraps when the pending word would overrun the width, and the
/// status areas are silently swallowed -- the same behavior as the
/// reference Node frontend, whose transcripts this voice's
/// tellings diff against.
#[derive(Debug)]
pub struct PlainVoice {
    pub(crate) styles: Vec<Pairs>,
    pub(crate) width: i64,
    pub(crate) told: String,
    pub(crate) hidden: bool,
    saves: bool,
    word: String,
    spaces: i64,
    x: i64,
    newlines: i64,
}

impl PlainVoice {
    /// Ready an empty telling for one story's output, at the
    /// classic width.
    pub fn new(story: &Story) -> Result<Self, VoxamError> {
        let mut voice = Self {
            styles: styled(story)?,
            width: WIDTH,
            told: String::new(),
            hidden: false,
            saves: false,
            word: String::new(),
            spaces: 0,
            x: 0,
            newlines: 1,
        };

        voice.reset();

        Ok(voice)
    }

    /// Declare savefile support without keeping any files -- the
    /// reference's per-instance has_saves override, for the walks
    /// whose gold transcripts carry the SAVEFILE feature lines.
    pub fn keeping(mut self) -> Self {
        self.saves = true;

        self
    }

    /// Choose a width other than the classic 80 columns.
    pub fn sized(mut self, width: i64) -> Self {
        self.width = width;

        self
    }

    /// Everything said so far, the pending word flushed out.
    pub fn told(&mut self) -> &str {
        self.flushed();

        &self.told
    }

    /// Land an input echo raw, straight past the word-wrapper.
    ///
    /// The reference frontend's readline echoes typed characters
    /// without telling its own io; a diff-faithful telling does
    /// the same. No newline lands here -- that is the Enter key's
    /// doing, which only a line input's prompted() models.
    pub fn echoed(&mut self, text: &str) {
        self.flushed();
        self.told.push_str(text);
    }

    /// Note that a sent line's echo reset the cursor.
    ///
    /// The reference frontend resets its wrap state on delivering
    /// a line -- and, deliberately, not on delivering keys.
    pub fn prompted(&mut self) {
        self.x = 0;
        self.spaces = 0;
        self.newlines = 1;
    }

    /// Land the pending spaces and word, wrapping first if needed.
    pub(crate) fn flushed(&mut self) {
        let pending = self.x + self.spaces + self.word.chars().count() as i64;

        if self.width > 0 && pending > self.width {
            self.spaced(0);
        }

        while self.spaces > 0 {
            if self.x != 0 {
                self.told.push(' ');
                self.x += 1;
            }

            self.spaces -= 1;
        }

        if !self.word.is_empty() {
            self.told.push_str(&self.word);
            self.x += self.word.chars().count() as i64;
            self.newlines = 0;
            self.word.clear();
        }
    }

    /// Ensure wanted + 1 newlines stand at the telling's end.
    fn spaced(&mut self, wanted: i64) {
        while self.newlines < wanted + 1 {
            self.told.push('\n');
            self.newlines += 1;
        }

        self.x = 0;
        self.spaces = 0;
    }

    /// A style's em-sized margin, zero when it names none.
    fn margined(&self, style: i64, edge: &str) -> i64 {
        if style >= 0 && (style as usize) < self.styles.len() {
            let claim = valued(&self.styles[style as usize], edge).trim();

            if let Some(count) = claim.strip_suffix("em") {
                let count = count.trim();

                if !count.is_empty() && count.chars().all(|held| held.is_ascii_digit()) {
                    return count.parse().unwrap_or(0);
                }
            }
        }

        0
    }
}

impl Voice for PlainVoice {
    fn has_saves(&self) -> bool {
        self.saves
    }

    fn say(&mut self, text: &str) {
        if self.hidden {
            return;
        }

        for piece in text.chars() {
            if piece == ' ' {
                self.flushed();
                self.spaces += 1;
            } else if piece == '-' {
                self.word.push(piece);
                self.flushed();
            } else {
                self.word.push(piece);
            }
        }
    }

    fn nbsp(&mut self) {
        if !self.hidden {
            self.word.push(' ');
        }
    }

    fn space(&mut self) {
        self.say(" ");
    }

    /// Print a run of forced spaces, clamped to the line.
    fn spaces(&mut self, count: i64) {
        if self.hidden {
            return;
        }

        self.flushed();

        let count = if self.width > 0 {
            count.min(self.width - self.x)
        } else {
            count
        };

        self.told.push_str(&" ".repeat(count.max(0) as usize));
        self.x += count;
        self.newlines = 0;
    }

    fn line(&mut self) {
        if !self.hidden {
            self.flushed();
            self.spaced(0);
        }
    }

    fn par(&mut self) {
        if !self.hidden {
            self.flushed();
            self.spaced(1);
        }
    }

    /// Open a div: break the line, honoring its top margin.
    fn enter_div(&mut self, style: i64) {
        if !self.hidden {
            self.flushed();

            let margin = self.margined(style, "margin-top");

            self.spaced(margin);
        }
    }

    /// Close a div: break the line, honoring its bottom margin.
    fn leave_div(&mut self, style: i64) {
        if !self.hidden {
            self.flushed();

            let margin = self.margined(style, "margin-bottom");

            self.spaced(margin);
        }
    }

    /// Open a span; plain text carries no dress.
    fn enter_span(&mut self, _style: i64) {}

    fn leave_span(&mut self) {}

    /// Dress the body; plain text carries no dress.
    fn set_body(&mut self, _style: i64) {}

    /// Enter a status area: swallowed whole on a plain telling.
    fn enter_status(&mut self, _area: i64, _style: i64) {
        self.line();
        self.hidden = true;
    }

    /// Leave the status area; the telling speaks again.
    fn leave_status(&mut self) {
        self.hidden = false;
    }

    /// Open a link; plain text renders it static.
    fn enter_link(&mut self, _words: &str) {}

    fn leave_link(&mut self) {}

    fn enter_link_res(&mut self, _resource: i64) {}

    fn leave_link_res(&mut self) {}

    fn enter_self_link(&mut self) {}

    fn leave_self_link(&mut self) {}

    /// Embed nothing; a plain telling cannot.
    fn embed_res(&mut self, _resource: i64) {}

    /// A plain telling embeds nothing.
    fn can_embed_res(&self, _resource: i64) -> bool {
        false
    }

    /// Draw the progress bar as an ASCII gauge on its own line.
    fn progress(&mut self, amount: i64, total: i64) {
        if self.hidden {
            return;
        }

        let room = (if self.width > 0 { self.width } else { WIDTH }) - 3;
        let filled = if total != 0 {
            (room as f64 * amount as f64 / total as f64 + 0.5) as i64
        } else {
            0
        };

        self.enter_div(-1);

        let gauge = format!(
            "[{}{}]",
            "=".repeat(filled.max(0) as usize),
            " ".repeat((room - filled).max(0) as usize)
        );

        self.say(&gauge);
        self.leave_div(-1);
    }

    /// Set nothing; plain text carries no styles.
    fn set_style(&mut self, _bits: i64) {}

    fn reset_style(&mut self, _bits: i64) {}

    fn unstyle(&mut self) {}

    /// Clear by paragraph break; a telling keeps its past.
    fn clear(&mut self) {
        self.par();
    }

    fn clear_all(&mut self) {
        self.par();
    }

    /// No status areas stand to hide.
    fn clear_status(&mut self) {}

    /// No links stand to retire.
    fn clear_links(&mut self) {}

    /// A telling keeps its past.
    fn clear_old(&mut self) {}

    fn clear_div(&mut self) {}

    /// Return to the initial state: line broken, nothing hidden.
    fn leave_all(&mut self) {
        self.line();
        self.hidden = false;
    }

    /// Flush the pending word to the telling.
    fn sync(&mut self) {
        self.flushed();
    }

    /// A plain telling is already its own transcript.
    fn script_on(&mut self) -> bool {
        false
    }

    /// No transcript stands to stop.
    fn script_off(&mut self) {}

    /// No transcript is ever running.
    fn script_active(&self) -> bool {
        false
    }

    fn reset(&mut self) {
        self.hidden = false;
        self.word.clear();
        self.spaces = 0;
        self.x = 0;
        self.newlines = 1;
    }

    /// The width in columns; the height is unknowable.
    fn measured(&self, dimension: i64) -> i64 {
        if dimension == 0 {
            return self.width.max(0);
        }

        0
    }

    /// Print one debug tracepoint raw on its own line.
    fn trace(&mut self, text: &str) {
        if !self.hidden {
            self.flushed();
            self.spaced(0);
            self.told.push_str(text);
            self.x = text.chars().count() as i64;
            self.newlines = 0;
            self.flushed();
            self.spaced(0);
        }
    }

    /// A plain telling keeps no files.
    fn save(&mut self, _data: &[u8]) -> bool {
        false
    }

    fn restore(&mut self) -> Option<Vec<u8>> {
        None
    }
}

/// The LOOK chunk's style classes, each a key-value dress.
///
/// Each class is a run of null-terminated CSS-shaped pairs ended
/// by a blank entry; keys keep their source case, so readers must
/// compare charitably (Aa-machine: LOOK). Fails for a table the
/// chunk cannot hold whole.
pub fn styled(story: &Story) -> Result<Vec<Pairs>, VoxamError> {
    let payload = &story.summed(b"LOOK").payload;

    if payload.len() < COUNT_SIZE {
        return Err(output_error(
            "the LOOK chunk is too short for its own count (Aa-machine: LOOK)".into(),
        ));
    }

    let count = usize::from(u16::from_be_bytes([payload[0], payload[1]]));

    if 2 + count * 2 > payload.len() {
        return Err(output_error(format!(
            "the LOOK table claims {count} styles, past the chunk's {} bytes \
             (Aa-machine: LOOK)",
            payload.len()
        )));
    }

    let mut styles = Vec::with_capacity(count);

    for seat in 0..count {
        let mut at = usize::from(u16::from_be_bytes([
            payload[2 + seat * 2],
            payload[3 + seat * 2],
        ]));
        let mut dress: Pairs = Vec::new();

        while at < payload.len() && payload[at] != 0 {
            let Some(ended) = payload[at..].iter().position(|&byte| byte == 0) else {
                return Err(output_error(format!(
                    "style {seat} is missing its null ending (Aa-machine: LOOK)"
                )));
            };
            let ended = at + ended;
            let told = asciied(&payload[at..ended]);

            if let Some((key, value)) = told.split_once(':') {
                let key = key.trim().to_string();
                let value = value.trim().to_string();

                match dress.iter_mut().find(|(held, _)| *held == key) {
                    Some(seat) => seat.1 = value,
                    None => dress.push((key, value)),
                }
            }

            at = ended + 1;
        }

        styles.push(dress);
    }

    Ok(styles)
}

/// The named colors Dialog's style sheets actually use, as the
/// CSS basics a display can mix (Aa-machine: LOOK).
const NAMED_COLORS: [(&str, (i64, i64, i64)); 13] = [
    ("black", (0, 0, 0)),
    ("red", (205, 49, 49)),
    ("green", (13, 188, 121)),
    ("yellow", (229, 229, 16)),
    ("blue", (36, 114, 200)),
    ("magenta", (188, 63, 188)),
    ("cyan", (17, 168, 205)),
    ("white", (229, 229, 229)),
    ("gray", (128, 128, 128)),
    ("grey", (128, 128, 128)),
    ("orange", (255, 165, 0)),
    ("purple", (128, 0, 128)),
    ("brown", (165, 42, 42)),
];

/// The deprecated SET_STYLE bits (Aa-machine: SET_STYLE).
pub const BIT_REVERSE: i64 = 1;
pub const BIT_BOLD: i64 = 2;
pub const BIT_ITALIC: i64 = 4;

/// One folded outfit: bold, italic, reverse, ink, paper.
pub type Outfit = (
    bool,
    bool,
    bool,
    Option<(i64, i64, i64)>,
    Option<(i64, i64, i64)>,
);

// The CSS color spellings a display can mix: #rrggbb, #rgb, and
// rgb() with its three channels.
const LONG_HEX: usize = 7;
const SHORT_HEX: usize = 4;
const CHANNELS: usize = 3;

/// One style class's wearable claims (Aa-machine: LOOK).
///
/// Bold and italic are tri-state: None inherits, and an explicit
/// font-style of normal turns italics off -- Miss Gosling's own
/// sheets say normal!important inside italic quotations.
#[derive(Debug, Clone, Default)]
pub struct Dress {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub ink: Option<(i64, i64, i64)>,
    pub paper: Option<(i64, i64, i64)>,
}

impl Dress {
    /// Read one class's pairs for what a display can wear.
    pub fn new(pairs: &Pairs) -> Self {
        let mut dress = Self {
            bold: None,
            italic: None,
            ink: tinted(valued(pairs, "color")),
            paper: tinted(valued(pairs, "background-color")),
        };

        let weight = plained(valued(pairs, "font-weight"));

        if weight.starts_with("bold") {
            dress.bold = Some(true);
        } else if weight == "normal" {
            dress.bold = Some(false);
        }

        let style = plained(valued(pairs, "font-style"));

        if style == "italic" || style == "oblique" {
            dress.italic = Some(true);
        } else if style == "normal" {
            dress.italic = Some(false);
        }

        dress
    }
}

/// The dress state a styled voice wears, face-neutrally.
///
/// The body dress lies beneath everything and survives leave_all,
/// being the document's rather than any division's; the worn
/// stack carries the open divs and spans; and the deprecated
/// SET_STYLE bits ride lowest of all. folded() tells the whole
/// outfit, for whichever attributes a face can render.
#[derive(Debug)]
pub struct Wardrobe {
    classes: Vec<Dress>,
    body: Option<Dress>,
    worn: Vec<Dress>,
    bits: i64,
}

impl Wardrobe {
    /// Cut one dress per LOOK class.
    pub fn new(styles: &[Pairs]) -> Self {
        Self {
            classes: styles.iter().map(Dress::new).collect(),
            body: None,
            worn: Vec::new(),
            bits: 0,
        }
    }

    /// One class's dress, a bare one for a class LOOK never named.
    pub fn classed(&self, style: i64) -> Dress {
        if style >= 0 && (style as usize) < self.classes.len() {
            return self.classes[style as usize].clone();
        }

        Dress::default()
    }

    /// Wear a division's or span's dress.
    pub fn entered(&mut self, style: i64) {
        let dress = self.classed(style);

        self.worn.push(dress);
    }

    /// Drop the newest dress; an unworn leave stays calm.
    pub fn left(&mut self) {
        self.worn.pop();
    }

    /// Dress the document body; every later dress layers on it.
    pub fn bodied(&mut self, style: i64) {
        self.body = Some(self.classed(style));
    }

    /// Turn on deprecated style bits (Aa-machine: SET_STYLE).
    pub fn styled(&mut self, bits: i64) {
        self.bits |= bits;
    }

    /// Turn off deprecated style bits.
    pub fn unstyled(&mut self, bits: i64) {
        self.bits &= !bits;
    }

    /// Drop every deprecated style bit.
    pub fn bared(&mut self) {
        self.bits = 0;
    }

    /// Drop the whole worn stack and the bits; the body stays.
    pub fn dropped(&mut self) {
        self.worn.clear();
        self.bits = 0;
    }

    /// The outfit as worn: bold, italic, reverse, ink, paper.
    pub fn folded(&self) -> Outfit {
        let mut bold = self.bits & BIT_BOLD != 0;
        let mut italic = self.bits & BIT_ITALIC != 0;
        let mut ink = None;
        let mut paper = None;

        for dress in self.body.iter().chain(self.worn.iter()) {
            bold = dress.bold.unwrap_or(bold);
            italic = dress.italic.unwrap_or(italic);
            ink = dress.ink.or(ink);
            paper = dress.paper.or(paper);
        }

        (bold, italic, self.bits & BIT_REVERSE != 0, ink, paper)
    }
}

/// A CSS value with its !important insistence stripped.
pub fn plained(value: &str) -> String {
    value.replace("!important", "").trim().to_lowercase()
}

/// A CSS color as RGB: names, #hex, and rgb() all mix.
///
/// A hex spelling with letters past f answers None -- the one
/// charity past the reference, which would fall over rather than
/// answer at all.
pub fn tinted(value: &str) -> Option<(i64, i64, i64)> {
    let told = plained(value);

    if let Some(&(_, mixed)) = NAMED_COLORS.iter().find(|&&(name, _)| name == told) {
        return Some(mixed);
    }

    let chars: Vec<char> = told.chars().collect();

    if told.starts_with('#') && chars.len() == LONG_HEX {
        let channel = |at: usize| i64::from_str_radix(&told[at..at + 2], 16).ok();

        return Some((channel(1)?, channel(3)?, channel(5)?));
    }

    if told.starts_with('#') && chars.len() == SHORT_HEX {
        let channel =
            |at: usize| i64::from_str_radix(&format!("{}{}", chars[at], chars[at]), 16).ok();

        return Some((channel(1)?, channel(2)?, channel(3)?));
    }

    if let Some(inner) = told
        .strip_prefix("rgb(")
        .and_then(|held| held.strip_suffix(')'))
    {
        let pieces: Vec<&str> = inner.split(',').collect();

        if pieces.len() == CHANNELS {
            let channel = |at: usize| pieces[at].trim().parse::<i64>().ok();

            return Some((channel(0)?, channel(1)?, channel(2)?));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aamachine::story::{SUMMED, crc32};
    use crate::iff::chunk as iff_chunk;

    // A minimal LANG: the four offsets and an empty extended table.
    fn lang() -> Vec<u8> {
        let mut held = Vec::new();

        for offset in [8u16, 8, 9, 10] {
            held.extend_from_slice(&offset.to_be_bytes());
        }

        held.extend_from_slice(&[0, 0, 0, 0, 0]);

        held
    }

    // A minimal story wearing the given LOOK chunk.
    fn dressed(look: &[u8]) -> Story {
        let summed = |name: &[u8; 4]| -> Vec<u8> {
            match name {
                b"LANG" => lang(),
                b"DICT" => vec![0, 0],
                b"LOOK" => look.to_vec(),
                _ => Vec::new(),
            }
        };

        let mut crc = 0;

        for name in &SUMMED {
            crc = crc32(&summed(name), crc);
        }

        let mut head = vec![0, 5, 2, 0];

        head.extend_from_slice(&1u16.to_be_bytes());
        head.extend_from_slice(b"260827");
        head.extend_from_slice(&crc.to_be_bytes());
        head.extend_from_slice(&[0; 6]);

        let mut pieces = iff_chunk(b"HEAD", &head);

        for name in &SUMMED {
            pieces.extend(iff_chunk(name, &summed(name)));
        }

        let mut body = b"AAVM".to_vec();

        body.extend(pieces);

        Story::new(&iff_chunk(b"FORM", &body)).unwrap()
    }

    fn plain() -> Story {
        dressed(&[0, 0])
    }

    // A story whose LOOK holds one style built from the entries.
    fn styled_story(entries: &[&[u8]]) -> Story {
        let mut definition = Vec::new();

        for piece in entries {
            definition.extend_from_slice(piece);
            definition.push(0);
        }

        definition.push(0);

        let mut look = 1u16.to_be_bytes().to_vec();

        look.extend_from_slice(&4u16.to_be_bytes());
        look.extend(definition);

        dressed(&look)
    }

    // A LOOK too short for its own count is refused at the door.
    #[test]
    fn a_short_look_is_refused() {
        let told = styled(&dressed(&[0])).expect_err("too short").to_string();

        assert!(told.contains("too short for its own count"), "{told}");
    }

    // A count claiming styles past the chunk is refused whole.
    #[test]
    fn an_overclaiming_look_is_refused() {
        let told = styled(&dressed(&9u16.to_be_bytes()))
            .expect_err("an overclaim")
            .to_string();

        assert!(told.contains("claims 9 styles"), "{told}");
    }

    // A style definition missing its null ending is refused by seat.
    #[test]
    fn an_unterminated_style_is_refused() {
        let mut look = 1u16.to_be_bytes().to_vec();

        look.extend_from_slice(&4u16.to_be_bytes());
        look.extend_from_slice(b"width: 1em");

        let told = styled(&dressed(&look))
            .expect_err("unterminated")
            .to_string();

        assert!(told.contains("style 0 is missing"), "{told}");
    }

    // Key-value pairs land trimmed; an entry without a colon is
    // passed over, the way the spec asks readers to be charitable.
    #[test]
    fn styles_read_their_pairs_charitably() {
        let story = styled_story(&[b"margin-top:  2em ", b"nonsense", b"font-weight: bold"]);

        assert_eq!(
            styled(&story).unwrap(),
            vec![vec![
                ("margin-top".to_string(), "2em".to_string()),
                ("font-weight".to_string(), "bold".to_string()),
            ]]
        );
    }

    // The em-sized margins parse; anything else answers zero.
    #[test]
    fn margins_parse_only_whole_ems() {
        let voice =
            PlainVoice::new(&styled_story(&[b"margin-top: 2em", b"margin-bottom: 12px"])).unwrap();

        assert_eq!(voice.margined(0, "margin-top"), 2);
        assert_eq!(voice.margined(0, "margin-bottom"), 0);
        assert_eq!(voice.margined(0, "margin-left"), 0);
        assert_eq!(voice.margined(9, "margin-top"), 0);
        assert_eq!(voice.margined(-1, "margin-top"), 0);
    }

    // Inside a status area the plain voice swallows everything:
    // text, breaks, forced spaces, bars, and traces alike.
    #[test]
    fn a_status_area_swallows_everything() {
        let mut voice = PlainVoice::new(&plain()).unwrap();

        voice.say("before ");
        voice.enter_status(0, 0);
        voice.say("hidden");
        voice.nbsp();
        voice.space();
        voice.spaces(4);
        voice.line();
        voice.par();
        voice.enter_div(0);
        voice.leave_div(0);
        voice.progress(1, 2);
        voice.trace("hidden too");
        voice.leave_status();
        voice.say("after");

        assert_eq!(voice.told(), "before \nafter");
    }

    // A trace lands raw on its own line, wrapped by nothing.
    #[test]
    fn a_trace_lands_on_its_own_line() {
        let mut voice = PlainVoice::new(&plain()).unwrap();

        voice.say("text");
        voice.trace("query(x) file:9");
        voice.say("more");

        assert_eq!(voice.told(), "text\nquery(x) file:9\nmore");
    }

    // The plain voice's flat answers: no files, no transcript, no
    // height, and a width that clamps at zero.
    #[test]
    fn the_flat_answers() {
        let mut voice = PlainVoice::new(&plain()).unwrap();

        assert!(!voice.save(b"data"));
        assert_eq!(voice.restore(), None);
        assert!(!voice.script_on());
        assert!(!voice.script_active());
        assert_eq!(voice.measured(0), 80);
        assert_eq!(voice.measured(1), 0);
        assert_eq!(PlainVoice::new(&plain()).unwrap().sized(-1).measured(0), 0);

        voice.script_off();
    }

    // An echo lands past the wrapper even with a word pending, and
    // prompted resets the wrap state for the output that follows.
    #[test]
    fn echoes_land_raw_and_prompted_resets() {
        let mut voice = PlainVoice::new(&plain()).unwrap().sized(10);

        voice.say("pending");
        voice.echoed("typed\n");
        voice.prompted();
        voice.say("next");

        assert_eq!(voice.told(), "pendingtyped\nnext");
    }

    // A word past the width wraps to a fresh line; forced spaces
    // clamp to the room that remains.
    #[test]
    fn the_wrap_and_the_clamp() {
        let mut voice = PlainVoice::new(&plain()).unwrap().sized(10);

        voice.say("first overlong");
        voice.spaces(99);
        voice.say("x");

        assert_eq!(voice.told(), "first\noverlong  \nx");
    }

    // A progress bar with a zero total draws empty rather than
    // dividing by nothing.
    #[test]
    fn a_zero_total_progress_bar_draws_empty() {
        let mut voice = PlainVoice::new(&plain()).unwrap().sized(13);

        voice.progress(0, 0);

        assert!(voice.told().contains("[          ]"));
    }

    // At width zero -- the wire's shape -- forced spaces go out
    // whole, unclamped: the display owns the wrapping.
    #[test]
    fn width_zero_never_clamps_spaces() {
        let mut voice = PlainVoice::new(&plain()).unwrap().sized(0);

        voice.say("x");
        voice.spaces(5);

        assert_eq!(voice.told(), "x     ");
    }
}
