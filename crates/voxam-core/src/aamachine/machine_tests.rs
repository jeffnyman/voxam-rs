//! Synthetic tests for the Å-machine's opcodes and refusals.
//!
//! Each story here is a tiny hand-assembled program built around
//! one seam: the code bytes are spelled out operand by operand,
//! and the outcome is read back out of the plain voice's telling.
//! The guard pattern leans on the machine's own error contract --
//! a runtime fault restarts execution at address 1 with the
//! error's number in R00, so a guarded program simply prints R00
//! when it comes back nonzero (Aa-machine: Runtime data).

use super::*;
use crate::aamachine::story::{SUMMED, Story, crc32};
use crate::iff::chunk as iff_chunk;

// A two-entry decoding table: entry 0 spells $ or jumps on; entry
// 1 spells the letter a or ends the string.
const TABLE: &[u8] = &[0x04, 0x81, 0x41, 0x80];

// WRIT built on that table: "$" at byte 0, "a" at byte 2 -- both
// on even addresses, where tiny string pointers can reach.
const WRIT: &[u8] = &[0b0110_0000, 0x00, 0b1011_0000];

const QUIT: &[u8] = &[0x70, 0x00];

// A VALUE, RAW, VWORD, or WORD immediate operand.
fn immediate(value: u16) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

// An absolute CODE operand.
fn absolute(at: usize) -> Vec<u8> {
    vec![0x80 | (at >> 16) as u8, (at >> 8) as u8, at as u8]
}

// PRINT_VAL of a register.
fn printed(reg: u8) -> Vec<u8> {
    vec![0x65, 0x80 | reg]
}

// PRINT_VAL of an immediate.
fn shown(value: u16) -> Vec<u8> {
    let mut told = vec![0x65];

    told.extend(immediate(value));

    told
}

// A program whose entry reports a nonzero R00 and quits.
//
// The engine restarts at address 1 with the error in R00 after a
// runtime fault; the guard prints it and stops, so a test can
// read the error number straight out of the telling.
fn guarded(main: &[u8]) -> Vec<u8> {
    let err_at = 1 + 7 + main.len();
    let mut told = vec![0x40, 0x00, 0x00, 0x80];

    told.extend(absolute(err_at));
    told.extend_from_slice(main);
    told.extend(printed(0));
    told.extend_from_slice(QUIT);

    told
}

// A program whose failures land at a printing choice point.
fn caught(main: &[u8]) -> Vec<u8> {
    let handler_at = 1 + 5 + main.len() + QUIT.len();
    let mut told = vec![0x0A, 0x00];

    told.extend(absolute(handler_at));
    told.extend_from_slice(main);
    told.extend_from_slice(QUIT);
    told.extend(shown(0x4009));
    told.extend_from_slice(QUIT);

    told
}

#[derive(Clone)]
struct Langed {
    table: Vec<u8>,
    // (lower, upper, codepoint) triples, the chunk's own shape.
    extended: Vec<(u8, u8, u32)>,
    endings: Vec<u8>,
    stops: Vec<u8>,
    before: Vec<u8>,
    after: Vec<u8>,
}

impl Default for Langed {
    fn default() -> Self {
        Self {
            table: TABLE.to_vec(),
            extended: Vec::new(),
            endings: vec![0x00],
            stops: Vec::new(),
            before: Vec::new(),
            after: Vec::new(),
        }
    }
}

// A LANG payload with all four decoder regions present.
fn langed(shape: Langed) -> Vec<u8> {
    let mut charactered = vec![shape.extended.len() as u8];

    for &(lower, upper, point) in &shape.extended {
        charactered.push(lower);
        charactered.push(upper);
        charactered.extend_from_slice(&point.to_be_bytes()[1..]);
    }

    let table_at: u16 = 8;
    let ext_at = table_at + shape.table.len() as u16;
    let endings_at = ext_at + charactered.len() as u16;
    let special_at = endings_at + shape.endings.len() as u16;
    let mut told = Vec::new();

    told.extend_from_slice(&table_at.to_be_bytes());
    told.extend_from_slice(&ext_at.to_be_bytes());
    told.extend_from_slice(&endings_at.to_be_bytes());
    told.extend_from_slice(&special_at.to_be_bytes());
    told.extend(shape.table);
    told.extend(charactered);
    told.extend(shape.endings);
    told.extend(shape.stops);
    told.push(0);
    told.extend(shape.before);
    told.push(0);
    told.extend(shape.after);
    told.push(0);

    told
}

// A DICT payload holding the given words.
fn worded(words: &[&[u8]]) -> Vec<u8> {
    let table_end = 2 + 3 * words.len();
    let mut entries = Vec::new();
    let mut arrays = Vec::new();
    let mut at = table_end;

    for word in words {
        entries.push(word.len() as u8);
        entries.extend_from_slice(&(at as u16).to_be_bytes());
        arrays.extend_from_slice(word);
        at += word.len();
    }

    let mut told = (words.len() as u16).to_be_bytes().to_vec();

    told.extend(entries);
    told.extend(arrays);

    told
}

// An INIT payload with an object table, and the ram size to fit.
//
// The global data sits after the offsets, each object's fields
// after that, and the long-term area stands empty at the tail.
fn roomy(nob: u16, fields: u16, top: u16, longterm: u16) -> (Vec<u8>, u16) {
    let mut offsets = vec![nob + 1];

    for seat in 0..nob {
        offsets.push(nob + 1 + top + seat * fields);
    }

    let data = vec![0u16; usize::from(top + nob * fields)];
    let ltb = (offsets.len() + data.len()) as u16;
    let mut payload = Vec::new();

    payload.extend_from_slice(&nob.to_be_bytes());
    payload.extend_from_slice(&ltb.to_be_bytes());
    payload.extend_from_slice(&ltb.to_be_bytes());

    for word in offsets.into_iter().chain(data) {
        payload.extend_from_slice(&word.to_be_bytes());
    }

    (payload, ltb + longterm)
}

fn snug() -> (Vec<u8>, u16) {
    roomy(0, 8, 8, 16)
}

struct Crafted {
    code: Vec<u8>,
    version: (u8, u8),
    heap: u16,
    aux: u16,
    init: Option<Vec<u8>>,
    ram: Option<u16>,
    lang: Option<Vec<u8>>,
    dictionary: Vec<u8>,
    maps: Vec<u8>,
    writ: Vec<u8>,
    tags: Option<Vec<u8>>,
}

impl Crafted {
    fn of(code: Vec<u8>) -> Self {
        Self {
            code,
            version: (0, 5),
            heap: 64,
            aux: 32,
            init: None,
            ram: None,
            lang: None,
            dictionary: vec![0, 0],
            maps: vec![0, 0],
            writ: WRIT.to_vec(),
            tags: None,
        }
    }
}

// A story around a code body; address 0 gains its FAIL.
fn crafted(shape: &Crafted) -> Story {
    let (init, ram) = match &shape.init {
        Some(init) => (init.clone(), shape.ram.unwrap_or(64)),
        None => {
            let (init, sized) = snug();

            (init, shape.ram.unwrap_or(sized))
        }
    };
    let lang = shape
        .lang
        .clone()
        .unwrap_or_else(|| langed(Langed::default()));
    let mut code = vec![0x01];

    code.extend(&shape.code);

    let summed = |name: &[u8; 4]| -> Vec<u8> {
        match name {
            b"LANG" => lang.clone(),
            b"DICT" => shape.dictionary.clone(),
            b"MAPS" => shape.maps.clone(),
            b"LOOK" => vec![0, 0],
            b"WRIT" => shape.writ.clone(),
            b"INIT" => init.clone(),
            b"CODE" => code.clone(),
            _ => Vec::new(),
        }
    };

    let mut crc = 0;

    for name in &SUMMED {
        crc = crc32(&summed(name), crc);
    }

    let mut head = vec![shape.version.0, shape.version.1, 2, 0];

    head.extend_from_slice(&1u16.to_be_bytes());
    head.extend_from_slice(b"260827");
    head.extend_from_slice(&crc.to_be_bytes());
    head.extend_from_slice(&shape.heap.to_be_bytes());
    head.extend_from_slice(&shape.aux.to_be_bytes());
    head.extend_from_slice(&ram.to_be_bytes());

    let mut pieces = iff_chunk(b"HEAD", &head);

    for name in &SUMMED {
        pieces.extend(iff_chunk(name, &summed(name)));
    }

    if let Some(tags) = &shape.tags {
        pieces.extend(iff_chunk(b"TAGS", tags));
    }

    let mut body = b"AAVM".to_vec();

    body.extend(pieces);

    Story::new(&iff_chunk(b"FORM", &body)).unwrap()
}

fn plain(code: Vec<u8>) -> Story {
    crafted(&Crafted::of(code))
}

// Run a crafted story, feeding lines; the telling comes back.
fn spoken(story: Story, lines: &[&str]) -> String {
    let voice = PlainVoice::new(&story).unwrap();
    let mut machine = Machine::new(story, voice, Some(7)).unwrap();
    let mut waiting = machine.run(None).unwrap();

    for line in lines {
        waiting = if waiting == Wait::Line {
            machine.deliver_line(line).unwrap()
        } else {
            machine
                .deliver_key(u32::from(line.chars().next().unwrap()))
                .unwrap()
        };
    }

    machine.voice.told().to_string()
}

fn refused(story: Story) -> String {
    let voice = PlainVoice::new(&story).unwrap();

    match Machine::new(story, voice, None) {
        Err(error) => error.to_string(),
        Ok(mut machine) => machine
            .run(None)
            .expect_err("the engine should refuse")
            .to_string(),
    }
}

// The delegation boilerplate a wrapping test voice needs: every
// Voice method not named in the invocation passes through to
// self.plain.
macro_rules! delegated {
    ($($name:ident),*) => {
        delegated!(@say $($name),*);
        delegated!(@nbsp $($name),*);
        delegated!(@space $($name),*);
        delegated!(@spaces $($name),*);
        delegated!(@line $($name),*);
        delegated!(@par $($name),*);
        delegated!(@enter_div $($name),*);
        delegated!(@leave_div $($name),*);
        delegated!(@enter_span $($name),*);
        delegated!(@leave_span $($name),*);
        delegated!(@set_body $($name),*);
        delegated!(@enter_status $($name),*);
        delegated!(@leave_status $($name),*);
        delegated!(@enter_link $($name),*);
        delegated!(@leave_link $($name),*);
        delegated!(@enter_link_res $($name),*);
        delegated!(@leave_link_res $($name),*);
        delegated!(@enter_self_link $($name),*);
        delegated!(@leave_self_link $($name),*);
        delegated!(@embed_res $($name),*);
        delegated!(@can_embed_res $($name),*);
        delegated!(@progress $($name),*);
        delegated!(@set_style $($name),*);
        delegated!(@reset_style $($name),*);
        delegated!(@unstyle $($name),*);
        delegated!(@clear $($name),*);
        delegated!(@clear_all $($name),*);
        delegated!(@clear_status $($name),*);
        delegated!(@clear_links $($name),*);
        delegated!(@clear_old $($name),*);
        delegated!(@clear_div $($name),*);
        delegated!(@leave_all $($name),*);
        delegated!(@sync $($name),*);
        delegated!(@script_on $($name),*);
        delegated!(@script_off $($name),*);
        delegated!(@script_active $($name),*);
        delegated!(@reset $($name),*);
        delegated!(@measured $($name),*);
        delegated!(@trace $($name),*);
        delegated!(@save $($name),*);
        delegated!(@restore $($name),*);
    };
    (@$method:ident $($name:ident),*) => {
        delegated!(@@skip $method; $($name),*);
    };
    (@@skip $method:ident; $first:ident $(, $rest:ident)*) => {
        delegated!(@@matches $method $first; $($rest),*);
    };
    (@@skip $method:ident;) => {
        delegated!(@@emit $method);
    };
    (@@matches say say; $($rest:ident),*) => {};
    (@@matches nbsp nbsp; $($rest:ident),*) => {};
    (@@matches space space; $($rest:ident),*) => {};
    (@@matches spaces spaces; $($rest:ident),*) => {};
    (@@matches line line; $($rest:ident),*) => {};
    (@@matches par par; $($rest:ident),*) => {};
    (@@matches enter_div enter_div; $($rest:ident),*) => {};
    (@@matches leave_div leave_div; $($rest:ident),*) => {};
    (@@matches enter_span enter_span; $($rest:ident),*) => {};
    (@@matches leave_span leave_span; $($rest:ident),*) => {};
    (@@matches set_body set_body; $($rest:ident),*) => {};
    (@@matches enter_status enter_status; $($rest:ident),*) => {};
    (@@matches leave_status leave_status; $($rest:ident),*) => {};
    (@@matches enter_link enter_link; $($rest:ident),*) => {};
    (@@matches leave_link leave_link; $($rest:ident),*) => {};
    (@@matches enter_link_res enter_link_res; $($rest:ident),*) => {};
    (@@matches leave_link_res leave_link_res; $($rest:ident),*) => {};
    (@@matches enter_self_link enter_self_link; $($rest:ident),*) => {};
    (@@matches leave_self_link leave_self_link; $($rest:ident),*) => {};
    (@@matches embed_res embed_res; $($rest:ident),*) => {};
    (@@matches can_embed_res can_embed_res; $($rest:ident),*) => {};
    (@@matches progress progress; $($rest:ident),*) => {};
    (@@matches set_style set_style; $($rest:ident),*) => {};
    (@@matches reset_style reset_style; $($rest:ident),*) => {};
    (@@matches unstyle unstyle; $($rest:ident),*) => {};
    (@@matches clear clear; $($rest:ident),*) => {};
    (@@matches clear_all clear_all; $($rest:ident),*) => {};
    (@@matches clear_status clear_status; $($rest:ident),*) => {};
    (@@matches clear_links clear_links; $($rest:ident),*) => {};
    (@@matches clear_old clear_old; $($rest:ident),*) => {};
    (@@matches clear_div clear_div; $($rest:ident),*) => {};
    (@@matches leave_all leave_all; $($rest:ident),*) => {};
    (@@matches sync sync; $($rest:ident),*) => {};
    (@@matches script_on script_on; $($rest:ident),*) => {};
    (@@matches script_off script_off; $($rest:ident),*) => {};
    (@@matches script_active script_active; $($rest:ident),*) => {};
    (@@matches reset reset; $($rest:ident),*) => {};
    (@@matches measured measured; $($rest:ident),*) => {};
    (@@matches trace trace; $($rest:ident),*) => {};
    (@@matches save save; $($rest:ident),*) => {};
    (@@matches restore restore; $($rest:ident),*) => {};
    (@@matches $method:ident $other:ident; $($rest:ident),*) => {
        delegated!(@@skip $method; $($rest),*);
    };
    (@@emit say) => {
        fn say(&mut self, text: &str) {
            self.plain.say(text);
        }
    };
    (@@emit nbsp) => {
        fn nbsp(&mut self) {
            self.plain.nbsp();
        }
    };
    (@@emit space) => {
        fn space(&mut self) {
            self.plain.space();
        }
    };
    (@@emit spaces) => {
        fn spaces(&mut self, count: i64) {
            self.plain.spaces(count);
        }
    };
    (@@emit line) => {
        fn line(&mut self) {
            self.plain.line();
        }
    };
    (@@emit par) => {
        fn par(&mut self) {
            self.plain.par();
        }
    };
    (@@emit enter_div) => {
        fn enter_div(&mut self, style: i64) {
            self.plain.enter_div(style);
        }
    };
    (@@emit leave_div) => {
        fn leave_div(&mut self, style: i64) {
            self.plain.leave_div(style);
        }
    };
    (@@emit enter_span) => {
        fn enter_span(&mut self, style: i64) {
            self.plain.enter_span(style);
        }
    };
    (@@emit leave_span) => {
        fn leave_span(&mut self) {
            self.plain.leave_span();
        }
    };
    (@@emit set_body) => {
        fn set_body(&mut self, style: i64) {
            self.plain.set_body(style);
        }
    };
    (@@emit enter_status) => {
        fn enter_status(&mut self, area: i64, style: i64) {
            self.plain.enter_status(area, style);
        }
    };
    (@@emit leave_status) => {
        fn leave_status(&mut self) {
            self.plain.leave_status();
        }
    };
    (@@emit enter_link) => {
        fn enter_link(&mut self, words: &str) {
            self.plain.enter_link(words);
        }
    };
    (@@emit leave_link) => {
        fn leave_link(&mut self) {
            self.plain.leave_link();
        }
    };
    (@@emit enter_link_res) => {
        fn enter_link_res(&mut self, resource: i64) {
            self.plain.enter_link_res(resource);
        }
    };
    (@@emit leave_link_res) => {
        fn leave_link_res(&mut self) {
            self.plain.leave_link_res();
        }
    };
    (@@emit enter_self_link) => {
        fn enter_self_link(&mut self) {
            self.plain.enter_self_link();
        }
    };
    (@@emit leave_self_link) => {
        fn leave_self_link(&mut self) {
            self.plain.leave_self_link();
        }
    };
    (@@emit embed_res) => {
        fn embed_res(&mut self, resource: i64) {
            self.plain.embed_res(resource);
        }
    };
    (@@emit can_embed_res) => {
        fn can_embed_res(&self, resource: i64) -> bool {
            self.plain.can_embed_res(resource)
        }
    };
    (@@emit progress) => {
        fn progress(&mut self, amount: i64, total: i64) {
            self.plain.progress(amount, total);
        }
    };
    (@@emit set_style) => {
        fn set_style(&mut self, bits: i64) {
            self.plain.set_style(bits);
        }
    };
    (@@emit reset_style) => {
        fn reset_style(&mut self, bits: i64) {
            self.plain.reset_style(bits);
        }
    };
    (@@emit unstyle) => {
        fn unstyle(&mut self) {
            self.plain.unstyle();
        }
    };
    (@@emit clear) => {
        fn clear(&mut self) {
            self.plain.clear();
        }
    };
    (@@emit clear_all) => {
        fn clear_all(&mut self) {
            self.plain.clear_all();
        }
    };
    (@@emit clear_status) => {
        fn clear_status(&mut self) {
            self.plain.clear_status();
        }
    };
    (@@emit clear_links) => {
        fn clear_links(&mut self) {
            self.plain.clear_links();
        }
    };
    (@@emit clear_old) => {
        fn clear_old(&mut self) {
            self.plain.clear_old();
        }
    };
    (@@emit clear_div) => {
        fn clear_div(&mut self) {
            self.plain.clear_div();
        }
    };
    (@@emit leave_all) => {
        fn leave_all(&mut self) {
            self.plain.leave_all();
        }
    };
    (@@emit sync) => {
        fn sync(&mut self) {
            self.plain.sync();
        }
    };
    (@@emit script_on) => {
        fn script_on(&mut self) -> bool {
            self.plain.script_on()
        }
    };
    (@@emit script_off) => {
        fn script_off(&mut self) {
            self.plain.script_off();
        }
    };
    (@@emit script_active) => {
        fn script_active(&self) -> bool {
            self.plain.script_active()
        }
    };
    (@@emit reset) => {
        fn reset(&mut self) {
            self.plain.reset();
        }
    };
    (@@emit measured) => {
        fn measured(&self, dimension: i64) -> i64 {
            self.plain.measured(dimension)
        }
    };
    (@@emit trace) => {
        fn trace(&mut self, text: &str) {
            self.plain.trace(text);
        }
    };
    (@@emit save) => {
        fn save(&mut self, data: &[u8]) -> bool {
            self.plain.save(data)
        }
    };
    (@@emit restore) => {
        fn restore(&mut self) -> Option<Vec<u8>> {
            self.plain.restore()
        }
    };
}

// A plain voice that also notes the machine's structural calls.
struct RecordingVoice {
    plain: PlainVoice,
    noted: Vec<(String, String)>,
}

impl RecordingVoice {
    fn new(story: &Story) -> Self {
        Self {
            plain: PlainVoice::new(story).unwrap(),
            noted: Vec::new(),
        }
    }

    fn note(&mut self, name: &str, args: String) {
        self.noted.push((name.to_string(), args));
    }
}

fn noted(name: &str, args: &str) -> (String, String) {
    (name.to_string(), args.to_string())
}

impl Voice for RecordingVoice {
    delegated!(
        enter_div,
        enter_span,
        leave_span,
        set_body,
        enter_link,
        leave_link,
        enter_link_res,
        leave_link_res,
        enter_self_link,
        leave_self_link,
        embed_res,
        progress,
        set_style,
        reset_style,
        unstyle,
        clear,
        clear_all,
        clear_status,
        clear_links,
        clear_old,
        clear_div,
        trace
    );

    fn enter_div(&mut self, style: i64) {
        self.note("enter_div", style.to_string());
        self.plain.enter_div(style);
    }

    fn enter_span(&mut self, style: i64) {
        self.note("enter_span", style.to_string());
    }

    fn leave_span(&mut self) {
        self.note("leave_span", String::new());
    }

    fn set_body(&mut self, style: i64) {
        self.note("set_body", style.to_string());
    }

    fn enter_link(&mut self, words: &str) {
        self.note("enter_link", words.to_string());
    }

    fn leave_link(&mut self) {
        self.note("leave_link", String::new());
    }

    fn enter_link_res(&mut self, resource: i64) {
        self.note("enter_link_res", resource.to_string());
    }

    fn leave_link_res(&mut self) {
        self.note("leave_link_res", String::new());
    }

    fn enter_self_link(&mut self) {
        self.note("enter_self_link", String::new());
    }

    fn leave_self_link(&mut self) {
        self.note("leave_self_link", String::new());
    }

    fn embed_res(&mut self, resource: i64) {
        self.note("embed_res", resource.to_string());
    }

    fn progress(&mut self, amount: i64, total: i64) {
        self.note("progress", format!("{amount} {total}"));
    }

    fn set_style(&mut self, bits: i64) {
        self.note("set_style", bits.to_string());
    }

    fn reset_style(&mut self, bits: i64) {
        self.note("reset_style", bits.to_string());
    }

    fn unstyle(&mut self) {
        self.note("unstyle", String::new());
    }

    fn clear(&mut self) {
        self.note("clear", String::new());
        self.plain.clear();
    }

    fn clear_all(&mut self) {
        self.note("clear_all", String::new());
        self.plain.clear_all();
    }

    fn clear_status(&mut self) {
        self.note("clear_status", String::new());
    }

    fn clear_links(&mut self) {
        self.note("clear_links", String::new());
    }

    fn clear_old(&mut self) {
        self.note("clear_old", String::new());
    }

    fn clear_div(&mut self) {
        self.note("clear_div", String::new());
    }

    fn trace(&mut self, text: &str) {
        self.note("trace", text.to_string());
    }
}

// Run a crafted story against a recording voice.
fn recorded(story: Story) -> RecordingVoice {
    let voice = RecordingVoice::new(&story);
    let mut machine = Machine::new(story, voice, Some(7)).unwrap();

    machine.run(None).unwrap();

    machine.voice
}

// How a keeping voice answers a restore.
enum Answering {
    Kept,
    Hollow,
    Corrupt,
}

// A voice keeping one savefile in memory, or refusing to.
struct KeepingVoice {
    plain: PlainVoice,
    kept: Option<Vec<u8>>,
    granting: bool,
    answering: Answering,
}

impl KeepingVoice {
    fn new(story: &Story) -> Self {
        Self {
            plain: PlainVoice::new(story).unwrap(),
            kept: None,
            granting: true,
            answering: Answering::Kept,
        }
    }
}

impl Voice for KeepingVoice {
    delegated!(save, restore);

    fn has_saves(&self) -> bool {
        true
    }

    fn save(&mut self, data: &[u8]) -> bool {
        if !self.granting {
            return false;
        }

        self.kept = Some(data.to_vec());

        true
    }

    fn restore(&mut self) -> Option<Vec<u8>> {
        match self.answering {
            Answering::Kept => self.kept.clone(),
            Answering::Hollow => None,
            Answering::Corrupt => Some(b"junk".to_vec()),
        }
    }
}

// -- refusals ----------------------------------------------------------

// An opcode the engine does not carry is a loud frontier report,
// named by address, not a silent skip.
#[test]
fn an_unknown_opcode_is_refused() {
    let told = refused(plain(vec![0x71]));

    assert!(told.contains("opcode 0x71 at $000001"), "{told}");
}

// An EXT0 selector the engine does not carry is equally loud.
#[test]
fn an_unknown_ext0_is_refused() {
    let told = refused(plain(vec![0x70, 0x7E]));

    assert!(told.contains("EXT0 0x7e"), "{told}");
}

// A failure with no choice frame anywhere has no handler to name;
// the engine refuses rather than reading past the heap.
#[test]
fn a_failure_with_no_choice_frame_is_refused() {
    let told = refused(plain(vec![0x01]));

    assert!(told.contains("no choice frame"), "{told}");
}

// A VM_INFO selector past $7f is undefined by the spec's own word.
#[test]
fn an_undefined_vm_info_selector_is_refused() {
    let told = refused(plain(vec![0x74, 0x80, 0x00]));

    assert!(told.contains("selector 0x80"), "{told}");
}

// Popping the aux stack past its own bottom is a wiring fault the
// engine names rather than wrapping around.
#[test]
fn an_aux_underflow_is_refused() {
    let told = refused(plain(vec![0x16, 0x00]));

    assert!(told.contains("past its own bottom"), "{told}");
}

// LEAVE_STATUS moved seats at 1.0: the old byte in a new story --
// and the new byte in an old story -- are both refused by era.
#[test]
fn leave_status_is_era_checked() {
    let mut shape = Crafted::of(vec![0xE7]);

    shape.version = (1, 0);

    let told = refused(crafted(&shape));

    assert!(told.contains("not LEAVE_STATUS in a format 1"), "{told}");

    let told = refused(plain(vec![0xEF]));

    assert!(told.contains("not LEAVE_STATUS in a format 0"), "{told}");
}

// A LANG special-character set without its null ending is refused
// at the machine's door.
#[test]
fn an_unterminated_special_set_is_refused() {
    let mut lang = langed(Langed::default());

    lang.pop();

    let mut shape = Crafted::of(QUIT.to_vec());

    shape.lang = Some(lang);

    let told = refused(crafted(&shape));

    assert!(told.contains("missing its null"), "{told}");
}

// Before format 0.4 the LANG chunk carries only the stop set; the
// whitespace inhibitors stand empty and the story still runs.
#[test]
fn an_old_story_has_one_special_set() {
    let mut lang = langed(Langed::default());

    lang.truncate(lang.len() - 2);

    let mut shape = Crafted::of(QUIT.to_vec());

    shape.version = (0, 3);
    shape.lang = Some(lang);

    let story = crafted(&shape);
    let voice = PlainVoice::new(&story).unwrap();
    let mut machine = Machine::new(story, voice, None).unwrap();

    assert_eq!(machine.run(None).unwrap(), Wait::Quit);
}

// -- the runtime error reports -----------------------------------------

// A heap too small for an environment frame restarts the machine
// with error 1 in R00, which the guard program prints.
#[test]
fn a_full_heap_env_reports_error_one() {
    let mut shape = Crafted::of(guarded(&[0x08, 0x3F]));

    shape.heap = 8;

    assert!(spoken(crafted(&shape), &[]).contains('1'));
}

// A choice frame past the heap reports the same exhaustion.
#[test]
fn a_full_heap_choice_reports_error_one() {
    let mut main = vec![0x0A, 0x3F];

    main.extend(absolute(0));

    let mut shape = Crafted::of(guarded(&main));

    shape.heap = 8;

    assert!(spoken(crafted(&shape), &[]).contains('1'));
}

// A stop frame with no aux room reports exhaustion 2.
#[test]
fn a_full_aux_stop_reports_error_two() {
    let mut main = vec![0x1D];

    main.extend(absolute(0));

    let mut shape = Crafted::of(guarded(&main));

    shape.aux = 0;

    assert!(spoken(crafted(&shape), &[]).contains('2'));
}

// Binding a variable with the trail already against the stack
// reports exhaustion 2 as well.
#[test]
fn a_full_trail_reports_error_two() {
    let main = vec![0x11, 0x01, 0x11, 0x02, 0x10, 0x81, 0x82];
    let mut shape = Crafted::of(guarded(&main));

    shape.aux = 0;

    assert!(spoken(crafted(&shape), &[]).contains('2'));
}

// SET_PARENT of a number reports a type error 3.
#[test]
fn set_parent_of_a_number_reports_error_three() {
    let (init, ram) = roomy(2, 8, 8, 16);
    let mut main = vec![0x2E];

    main.extend(immediate(0x4001));
    main.extend(immediate(0x0001));

    let mut shape = Crafted::of(guarded(&main));

    shape.init = Some(init);
    shape.ram = Some(ram);

    assert!(spoken(crafted(&shape), &[]).contains('3'));
}

// Writing a field of an object past the count reports error 3.
#[test]
fn a_field_of_a_missing_object_reports_error_three() {
    let (init, ram) = roomy(1, 8, 8, 16);
    let mut main = vec![0x24];

    main.extend(immediate(0x0005));
    main.push(0x00);
    main.extend(immediate(1));

    let mut shape = Crafted::of(guarded(&main));

    shape.init = Some(init);
    shape.ram = Some(ram);

    assert!(spoken(crafted(&shape), &[]).contains('3'));
}

// Storing a value that still holds an unbound variable long-term
// reports error 4: only bound data survives the heap.
#[test]
fn storing_an_unbound_value_reports_error_four() {
    let (init, ram) = roomy(1, 8, 8, 16);
    let main = vec![0x11, 0x01, 0x12, 0x81, 0x02, 0x03, 0xA6, 0x00, 0x83];
    let mut shape = Crafted::of(guarded(&main));

    shape.init = Some(init);
    shape.ram = Some(ram);

    assert!(spoken(crafted(&shape), &[]).contains('4'));
}

// A long-term area too small for a serialized list reports 6.
#[test]
fn a_full_longterm_area_reports_error_six() {
    let (init, ram) = roomy(0, 8, 8, 1);
    let mut main = vec![0x13, 0x40, 0x01, 0x02, 0x03, 0x10];

    main.extend(immediate(0x3F00));
    main.push(0x82);
    main.extend_from_slice(&[0xA6, 0x00, 0x83]);

    let mut shape = Crafted::of(guarded(&main));

    shape.init = Some(init);
    shape.ram = Some(ram);

    assert!(spoken(crafted(&shape), &[]).contains('6'));
}

// ENTER_DIV inside a span is an invalid output state, error 7.
#[test]
fn a_div_inside_a_span_reports_error_seven() {
    assert!(spoken(plain(guarded(&[0x6E, 0x00, 0x66, 0x00])), &[]).contains('7'));
}

// ENTER_STATUS inside a span reports the same error 7.
#[test]
fn a_status_inside_a_span_reports_error_seven() {
    assert!(spoken(plain(guarded(&[0x6E, 0x00, 0x6F, 0x00, 0x00])), &[]).contains('7'));
}

// SET_BODY inside a span, the 1.0 shape of $67, reports error 7.
#[test]
fn set_body_inside_a_span_reports_error_seven() {
    let mut shape = Crafted::of(guarded(&[0x6E, 0x00, 0x67, 0x00]));

    shape.version = (1, 0);

    assert!(spoken(crafted(&shape), &[]).contains('7'));
}

// SAVE inside a span reports error 7 before any file is asked for.
#[test]
fn save_inside_a_span_reports_error_seven() {
    let mut main = vec![0x6E, 0x00, 0x72];

    main.extend(absolute(0));

    assert!(spoken(plain(guarded(&main)), &[]).contains('7'));
}

// SAVE_UNDO inside a span reports error 7 as well.
#[test]
fn save_undo_inside_a_span_reports_error_seven() {
    let mut main = vec![0x6E, 0x00, 0xF2];

    main.extend(absolute(0));

    assert!(spoken(plain(guarded(&main)), &[]).contains('7'));
}

// CLEAR_OLD inside a span reports error 7.
#[test]
fn clear_old_inside_a_span_reports_error_seven() {
    assert!(spoken(plain(guarded(&[0x6E, 0x00, 0x70, 0x10])), &[]).contains('7'));
}

// CLEAR_STATUS from inside a status area reports error 7.
#[test]
fn clear_status_inside_a_status_reports_error_seven() {
    assert!(spoken(plain(guarded(&[0x6F, 0x00, 0x00, 0x70, 0x12])), &[]).contains('7'));
}

// CLEAR from inside a span reports error 7.
#[test]
fn clear_inside_a_span_reports_error_seven() {
    assert!(spoken(plain(guarded(&[0x6E, 0x00, 0x70, 0x06])), &[]).contains('7'));
}

// -- arithmetic --------------------------------------------------------

// ADD_RAW and SUB_RAW work on raw sixteen-bit words: adding 3 to
// the tagged number 5 lands on the tagged number 8.
#[test]
fn raw_arithmetic_carries_whole_words() {
    let mut main = vec![0x50];

    main.extend(immediate(0x4005));
    main.extend(immediate(3));
    main.push(0x01);
    main.extend(printed(1));
    main.extend_from_slice(&[0x51, 0x81]);
    main.extend(immediate(3));
    main.push(0x02);
    main.extend(printed(2));
    main.extend_from_slice(QUIT);

    let told = spoken(plain(main), &[]);

    assert!(told.contains('8') && told.contains('5'), "{told}");
}

// RAND_RAW with a zero bound can only roll zero.
#[test]
fn rand_raw_with_a_zero_bound_rolls_zero() {
    let mut main = vec![0x52, 0x00, 0x01, 0x50, 0x81];

    main.extend(immediate(0x4000));
    main.push(0x01);
    main.extend(printed(1));
    main.extend_from_slice(QUIT);

    assert!(spoken(plain(main), &[]).contains('0'));
}

// DIV_NUM by zero fails to the choice point rather than crashing.
#[test]
fn division_by_zero_fails() {
    let mut main = vec![0x5C];

    main.extend(immediate(0x4006));
    main.extend(immediate(0x4000));
    main.push(0x01);

    assert!(spoken(plain(caught(&main)), &[]).contains('9'));
}

// MOD_NUM by zero fails the same way.
#[test]
fn remainder_by_zero_fails() {
    let mut main = vec![0x5D];

    main.extend(immediate(0x4006));
    main.extend(immediate(0x4000));
    main.push(0x01);

    assert!(spoken(plain(caught(&main)), &[]).contains('9'));
}

// RAND_NUM with an empty range fails.
#[test]
fn a_backward_random_range_fails() {
    let mut main = vec![0x5A];

    main.extend(immediate(0x4005));
    main.extend(immediate(0x4002));
    main.push(0x01);

    assert!(spoken(plain(caught(&main)), &[]).contains('9'));
}

// ADD_NUM of a non-number fails through the unboxing.
#[test]
fn adding_an_object_fails() {
    let mut main = vec![0x58];

    main.extend(immediate(0x0001));
    main.extend(immediate(0x4001));
    main.push(0x01);

    assert!(spoken(plain(caught(&main)), &[]).contains('9'));
}

// -- values spelled out ------------------------------------------------

// The empty list, an unbound variable, and an improper list all
// have PRINT_VAL spellings of their own.
#[test]
fn odd_values_spell_themselves() {
    let mut main = shown(0x3F00);

    main.extend_from_slice(&[0x11, 0x01]);
    main.extend(printed(1));
    main.extend_from_slice(&[0x13, 0x40, 0x01, 0x02, 0x03, 0x10]);
    main.extend(immediate(0x4002));
    main.push(0x82);
    main.extend(printed(3));
    main.extend_from_slice(QUIT);

    let told = spoken(plain(main), &[]);

    assert!(
        told.contains("[]") && told.contains('$') && told.contains("[1 | 2]"),
        "{told}"
    );
}

// An object prints its TAGS name after the hashmark when the
// chunk is aboard, and the bare hashmark when it is not.
#[test]
fn objects_print_their_tags_names() {
    let (init, ram) = roomy(1, 8, 8, 16);
    let mut main = shown(0x0001);

    main.extend_from_slice(QUIT);

    let mut tags = 1u16.to_be_bytes().to_vec();

    tags.extend_from_slice(&4u16.to_be_bytes());
    tags.extend_from_slice(b"lamp\x00");

    let mut shape = Crafted::of(main.clone());

    shape.init = Some(init.clone());
    shape.ram = Some(ram);
    shape.tags = Some(tags);

    assert!(spoken(crafted(&shape), &[]).contains("#lamp"));

    let mut bare = Crafted::of(main);

    bare.init = Some(init);
    bare.ram = Some(ram);

    assert!(spoken(crafted(&bare), &[]).contains('#'));
}

// An extended character prints through the story's own table.
#[test]
fn an_extended_character_prints() {
    let mut main = shown(0x3E80);

    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.lang = Some(langed(Langed {
        extended: vec![(0x80, 0x80, 0xC5)],
        ..Langed::default()
    }));

    assert!(spoken(crafted(&shape), &[]).contains('\u{c5}'));
}

// UPPERCASE arms exactly one character: an ASCII letter rises,
// and an extended letter rises through the table's upper seat.
#[test]
fn uppercase_arms_one_character() {
    let lang = langed(Langed {
        extended: vec![(0x80, 0x81, 0xE5), (0x81, 0x81, 0xC5)],
        ..Langed::default()
    });
    let mut main = vec![0x70, 0x0E];

    main.extend(shown(0x3E61));
    main.extend_from_slice(&[0x70, 0x0E]);
    main.extend(shown(0x3E80));
    main.extend(shown(0x3E61));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.lang = Some(lang);

    let told = spoken(crafted(&shape), &[]);

    assert!(
        told.contains('A') && told.contains('\u{c5}') && told.contains('a'),
        "{told}"
    );
}

// -- lists and words ---------------------------------------------------

// SPLIT_WORD spells a dictionary word, a character, and a number
// into their lists; digits inside a word arrive as numbers.
#[test]
fn split_word_spells_lists() {
    let mut main = vec![0x1F];

    main.extend(immediate(0x2001));
    main.push(0x01);
    main.extend(printed(1));
    main.push(0x1F);
    main.extend(immediate(0x3E62));
    main.push(0x02);
    main.extend(printed(2));
    main.push(0x1F);
    main.extend(immediate(0x4159));
    main.push(0x03);
    main.extend(printed(3));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.dictionary = worded(&[b"go", b"a1"]);

    let told = spoken(crafted(&shape), &[]);

    assert!(
        told.contains("[a 1]") && told.contains("[b]") && told.contains("[3 4 5]"),
        "{told}"
    );
}

// SPLIT_WORD of something wordless fails.
#[test]
fn split_word_of_a_number_pair_fails() {
    let mut main = vec![0x13, 0x40, 0x01, 0x02, 0x03, 0x10];

    main.extend(immediate(0x3F00));
    main.push(0x82);
    main.extend_from_slice(&[0x1F, 0x83, 0x04]);

    assert!(spoken(plain(caught(&main)), &[]).contains('9'));
}

// SPLIT_LIST copies a list up to a tail; with the empty list as
// the end, the whole list is copied cell by cell.
#[test]
fn split_list_copies_the_whole_list() {
    let mut main = vec![0x73, 0x00, 0x1B, 0x80];

    main.extend(immediate(0x3F00));
    main.push(0x01);
    main.extend(printed(1));
    main.extend_from_slice(QUIT);

    assert!(spoken(plain(main), &["a b c"]).contains("[a b c]"));
}

// JOIN_WORDS glues a word list back into one word: letters,
// numbers, and dictionary words all flatten.
#[test]
fn join_words_glues_a_word() {
    let mut main = vec![0x73, 0x00, 0x9F, 0x80, 0x01];

    main.extend(printed(1));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.dictionary = worded(&[b"go"]);

    assert!(spoken(crafted(&shape), &["go 12"]).contains("go12"));
}

// JOIN_WORDS of a lone character is that character, even one the
// story treats as a stop.
#[test]
fn join_words_keeps_a_lone_character() {
    let mut main = vec![0x73, 0x00, 0x9F, 0x80, 0x01];

    main.extend(printed(1));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.lang = Some(langed(Langed {
        stops: b".".to_vec(),
        ..Langed::default()
    }));

    assert!(spoken(crafted(&shape), &["."]).contains('.'));
}

// JOIN_WORDS fails on a stop character inside a longer list.
#[test]
fn join_words_refuses_an_inner_stop() {
    let main = vec![0x73, 0x00, 0x9F, 0x80, 0x01];
    let mut shape = Crafted::of(caught(&main));

    shape.lang = Some(langed(Langed {
        stops: b".".to_vec(),
        ..Langed::default()
    }));
    shape.dictionary = worded(&[b"go"]);

    assert!(spoken(crafted(&shape), &[". go"]).contains('9'));
}

// JOIN_WORDS fails on anything but a pair.
#[test]
fn join_words_refuses_a_bare_number() {
    let mut main = vec![0x9F];

    main.extend(immediate(0x4001));
    main.push(0x01);

    assert!(spoken(plain(caught(&main)), &[]).contains('9'));
}

// JOIN_WORDS fails on an improper list.
#[test]
fn join_words_refuses_an_improper_list() {
    let mut main = vec![0x13, 0x3E, 0x61, 0x02, 0x03, 0x10];

    main.extend(immediate(0x4002));
    main.push(0x82);
    main.extend_from_slice(&[0x9F, 0x83, 0x04]);

    assert!(spoken(plain(caught(&main)), &[]).contains('9'));
}

// The aux stack serializes an unbound variable to a marker and
// revives it as a fresh variable.
#[test]
fn aux_serialization_carries_the_unbound() {
    let mut main = vec![0x94, 0x11, 0x01, 0x14, 0x81, 0x16, 0x02];

    main.extend(printed(2));
    main.extend_from_slice(QUIT);

    assert!(spoken(plain(main), &[]).contains('$'));
}

// The aux stack serializes an improper list and revives it whole.
#[test]
fn aux_serialization_carries_the_improper() {
    let mut main = vec![0x13, 0x40, 0x01, 0x02, 0x03, 0x10];

    main.extend(immediate(0x4002));
    main.push(0x82);
    main.extend_from_slice(&[0x14, 0x83, 0x16, 0x04]);
    main.extend(printed(4));
    main.extend_from_slice(QUIT);

    assert!(spoken(plain(main), &[]).contains("[1 | 2]"));
}

// -- long-term storage -------------------------------------------------

// A stored list survives a rearrangement: freeing an earlier
// chunk slides the later one down and repoints its owner.
#[test]
fn longterm_storage_survives_a_slide() {
    let (init, ram) = roomy(1, 8, 8, 32);
    let mut main = vec![0x73, 0x00, 0xA6, 0x00, 0x80, 0xA6, 0x01, 0x80, 0xA6, 0x00];

    main.extend(immediate(0x4001));
    main.extend_from_slice(&[0xA2, 0x01, 0x01]);
    main.extend(printed(1));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.init = Some(init);
    shape.ram = Some(ram);

    assert!(spoken(crafted(&shape), &["a zz"]).contains("[a zz]"));
}

// An improper list survives long-term storage too.
#[test]
fn longterm_storage_carries_the_improper() {
    let (init, ram) = roomy(1, 8, 8, 16);
    let mut main = vec![0x13, 0x40, 0x01, 0x02, 0x03, 0x10];

    main.extend(immediate(0x4002));
    main.push(0x82);
    main.extend_from_slice(&[0xA6, 0x00, 0x83, 0xA2, 0x00, 0x04]);
    main.extend(printed(4));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.init = Some(init);
    shape.ram = Some(ram);

    assert!(spoken(crafted(&shape), &[]).contains("[1 | 2]"));
}

// LOAD_VAL of a never-written field fails.
#[test]
fn loading_an_empty_field_fails() {
    let (init, ram) = roomy(1, 8, 8, 16);
    let main = vec![0xA2, 0x02, 0x01];
    let mut shape = Crafted::of(caught(&main));

    shape.init = Some(init);
    shape.ram = Some(ram);

    assert!(spoken(crafted(&shape), &[]).contains('9'));
}

// -- the object tree ---------------------------------------------------

// UNLINK removes an object from a chain rooted in another field,
// and quietly ignores a non-object key.
#[test]
fn unlink_removes_from_a_chain() {
    let (init, ram) = roomy(2, 8, 8, 16);
    let mut main = vec![0xA4, 0x04];

    main.extend(immediate(0x0001));
    main.push(0x24);
    main.extend(immediate(0x0001));
    main.push(0x04);
    main.extend(immediate(0x0002));
    main.extend_from_slice(&[0xAD, 0x04, 0x04]);
    main.extend(immediate(0x0002));
    main.push(0x20);
    main.extend(immediate(0x0001));
    main.extend_from_slice(&[0x04, 0x01, 0x50, 0x81]);
    main.extend(immediate(0x4000));
    main.push(0x01);
    main.extend(printed(1));
    main.push(0x2D);
    main.extend(immediate(0));
    main.extend_from_slice(&[0x04, 0x04]);
    main.extend(immediate(0x4001));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.init = Some(init);
    shape.ram = Some(ram);

    assert!(spoken(crafted(&shape), &[]).contains('0'));
}

// -- branches and checks -----------------------------------------------

// IF_MEM_EQ jumps when the field matches its raw operand.
#[test]
fn if_mem_eq_jumps_on_a_match() {
    let (init, ram) = roomy(1, 8, 8, 16);
    let mut head = vec![0xA4, 0x00];

    head.extend(immediate(5));
    head.push(0x3A);
    head.extend(immediate(0));
    head.push(0x00);
    head.extend(immediate(5));

    let mut landing = shown(0x4002);

    landing.extend_from_slice(QUIT);

    let mut fallthrough = shown(0x4001);

    fallthrough.extend_from_slice(QUIT);

    let at = 1 + head.len() + 3 + fallthrough.len();
    let mut main = head;

    main.extend(absolute(at));
    main.extend(fallthrough);
    main.extend(landing);

    let mut shape = Crafted::of(main);

    shape.init = Some(init);
    shape.ram = Some(ram);

    assert!(spoken(crafted(&shape), &[]).contains('2'));
}

// CHECK_GT_EQ splits three ways on IDX.
#[test]
fn check_gt_eq_splits_three_ways() {
    let program = |idx: u16| -> Story {
        let mut head = vec![0x78];

        head.extend(immediate(idx));
        head.push(0x7A);
        head.extend(immediate(0x4004));

        let mut third = shown(0x4003);

        third.extend_from_slice(QUIT);

        let above_at = 1 + head.len() + 6 + third.len();
        let equal_at = above_at + third.len();
        let mut body = head;

        body.extend(absolute(above_at));
        body.extend(absolute(equal_at));
        body.extend(shown(0x4003));
        body.extend_from_slice(QUIT);
        body.extend(shown(0x4001));
        body.extend_from_slice(QUIT);
        body.extend(shown(0x4002));
        body.extend_from_slice(QUIT);

        plain(body)
    };

    assert!(spoken(program(0x4005), &[]).contains('1'));
    assert!(spoken(program(0x4004), &[]).contains('2'));
    assert!(spoken(program(0x4003), &[]).contains('3'));
}

// CHECK_GT jumps in both its RAW and BYTE shapes.
#[test]
fn check_gt_jumps_in_both_shapes() {
    let mut fall = shown(0x4001);

    fall.extend_from_slice(QUIT);

    let mut head = vec![0x78];

    head.extend(immediate(0x4005));
    head.push(0x7B);
    head.extend(immediate(0x4001));

    let at = 1 + head.len() + 3 + fall.len();
    let mut body = head;

    body.extend(absolute(at));
    body.extend(shown(0x4001));
    body.extend_from_slice(QUIT);
    body.extend(shown(0x4002));
    body.extend_from_slice(QUIT);

    assert!(spoken(plain(body), &[]).contains('2'));

    let mut head = vec![0x78];

    head.extend(immediate(0x0005));
    head.extend_from_slice(&[0xFB, 0x01]);

    let at = 1 + head.len() + 3 + fall.len();
    let mut body = head;

    body.extend(absolute(at));
    body.extend(shown(0x4001));
    body.extend_from_slice(QUIT);
    body.extend(shown(0x4002));
    body.extend_from_slice(QUIT);

    assert!(spoken(plain(body), &[]).contains('2'));
}

// IF_UNIFY sees two separately-typed unknown words as one.
#[test]
fn if_unify_matches_twin_unknown_words() {
    let head = vec![
        0x73, 0x00, 0x12, 0x01, 0x02, 0x80, 0x12, 0x03, 0x04, 0x82, 0x37, 0x81, 0x83,
    ];
    let mut fall = shown(0x4001);

    fall.extend_from_slice(QUIT);

    let at = 1 + head.len() + 3 + fall.len();
    let mut body = head;

    body.extend(absolute(at));
    body.extend(shown(0x4001));
    body.extend_from_slice(QUIT);
    body.extend(shown(0x4002));
    body.extend_from_slice(QUIT);

    assert!(spoken(plain(body), &["qq qq"]).contains('2'));
}

// -- the wordmaps ------------------------------------------------------

// A story consulting a three-entry wordmap for the given IDX.
//
// The map knows 'go' as a wildcard, dict word 1 as one object,
// and the period as a payload of two -- one of them wide.
fn mapped_story(idx: u16) -> Story {
    let mut entries = immediate(0x2000);

    entries.extend(immediate(0));
    entries.extend(immediate(0x2001));
    entries.extend(immediate(0xE000 | 2));
    entries.extend(immediate(0x3E2E));
    entries.extend(immediate(18));

    let mut table = immediate(3);

    table.extend(entries);
    table.extend_from_slice(&[0x01, 0xE0, 0x02, 0x00]);

    let mut maps = immediate(1);

    maps.extend(immediate(4));
    maps.extend(table);

    let mut head = vec![0x94, 0x78];

    head.extend(immediate(idx));
    head.extend_from_slice(&[0x7C, 0x00]);

    let mut fall = shown(0x4001);

    fall.extend_from_slice(QUIT);

    let at = 1 + head.len() + 3 + fall.len();
    let mut landing = vec![0x17, 0x01];

    landing.extend(printed(1));
    landing.extend_from_slice(QUIT);

    let mut body = head;

    body.extend(absolute(at));
    body.extend(shown(0x4001));
    body.extend_from_slice(QUIT);
    body.extend(landing);

    let (init, ram) = roomy(2, 8, 8, 16);
    let mut shape = Crafted::of(body);

    shape.maps = maps;
    shape.init = Some(init);
    shape.ram = Some(ram);
    shape.dictionary = worded(&[b"go", b"at"]);

    crafted(&shape)
}

// A wildcard word matches everything: no jump, nothing pushed.
#[test]
fn a_wildcard_word_stays_on_the_path() {
    assert!(spoken(mapped_story(0x2000), &[]).contains('1'));
}

// A single-object word pushes its object and jumps.
#[test]
fn a_single_object_word_jumps_with_its_object() {
    assert!(spoken(mapped_story(0x2001), &[]).contains("[#]"));
}

// A payload word pushes its whole list, wide ids included.
#[test]
fn a_payload_word_jumps_with_its_objects() {
    assert!(spoken(mapped_story(0x3E2E), &[]).contains("[# #]"));
}

// A word missing from the map jumps with nothing pushed.
#[test]
fn a_missing_word_jumps_empty() {
    assert!(spoken(mapped_story(0x3E2F), &[]).contains("[]"));
}

// -- the output tour ---------------------------------------------------

// Spans, styles, and resources travel to the voice with their
// operands; a two-byte INDEX reaches its full range.
#[test]
fn the_output_tour_reaches_the_voice() {
    let mut main = vec![
        0x6E, 0xC1, 0x05, 0xEE, 0x6B, 0x02, 0xEB, 0x02, 0x70, 0x04, 0x6C,
    ];

    main.extend(immediate(0x4001));
    main.push(0xEC);
    main.extend(immediate(0x4001));
    main.push(0x01);
    main.push(0x6D);
    main.extend(immediate(0x4001));
    main.extend(immediate(0x4004));
    main.push(0x64);
    main.extend(immediate(0x4003));
    main.extend_from_slice(&[0x70, 0x0F, 0x70, 0x11]);
    main.extend_from_slice(QUIT);

    let voice = recorded(plain(main));

    assert!(voice.noted.contains(&noted("enter_span", "261")));
    assert!(voice.noted.contains(&noted("leave_span", "")));
    assert!(voice.noted.contains(&noted("set_style", "2")));
    assert!(voice.noted.contains(&noted("reset_style", "2")));
    assert!(voice.noted.contains(&noted("unstyle", "")));
    assert!(voice.noted.contains(&noted("embed_res", "16385")));
    assert!(voice.noted.contains(&noted("progress", "1 4")));
    assert!(voice.noted.contains(&noted("clear_links", "")));
    assert!(voice.noted.contains(&noted("clear_div", "")));
}

// A link built from a word list reaches the voice with its click
// words spelled; a nested link stays silent inside the outer one.
#[test]
fn links_reach_the_voice_once() {
    let mut main = vec![0x13, 0x20, 0x00, 0x02, 0x03, 0x10];

    main.extend(immediate(0x3F00));
    main.push(0x82);
    main.extend_from_slice(&[0x69, 0x83, 0x69, 0x83, 0xE9, 0xE9, 0x68]);
    main.extend(immediate(0x4005));
    main.push(0x68);
    main.extend(immediate(0x4005));
    main.extend_from_slice(&[0xE8, 0xE8, 0x6A, 0x6A, 0xEA, 0xEA]);
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.dictionary = worded(&[b"go"]);

    let voice = recorded(crafted(&shape));
    let links: Vec<_> = voice
        .noted
        .iter()
        .filter(|(name, _)| name.ends_with("link"))
        .collect();

    assert!(voice.noted.contains(&noted("enter_link", "go")));
    assert!(voice.noted.contains(&noted("enter_link_res", "16389")));
    assert!(voice.noted.contains(&noted("enter_self_link", "")));
    assert_eq!(links.len(), 4);
}

// CLEAR keeps the div stack: the voice sees the clear, then every
// open div entered again.
#[test]
fn clear_restates_the_open_divs() {
    let voice = recorded(plain(vec![0x66, 0x03, 0x70, 0x06, 0x70, 0x07, 0x70, 0x00]));
    let told: Vec<_> = voice
        .noted
        .iter()
        .filter(|(name, _)| matches!(name.as_str(), "enter_div" | "clear" | "clear_all"))
        .cloned()
        .collect();

    assert_eq!(
        told,
        vec![
            noted("enter_div", "3"),
            noted("clear", ""),
            noted("enter_div", "3"),
            noted("clear_all", ""),
            noted("enter_div", "3"),
        ]
    );
}

// SET_BODY, the 1.0 shape of $67, reaches the voice.
#[test]
fn set_body_reaches_the_voice() {
    let mut shape = Crafted::of(vec![0x67, 0x04, 0x70, 0x00]);

    shape.version = (1, 0);

    let voice = recorded(crafted(&shape));

    assert!(voice.noted.contains(&noted("set_body", "4")));
}

// TRACEPOINT speaks only while tracing, with the registers
// substituted into the shape's dollar signs.
#[test]
fn tracepoint_speaks_only_while_tracing() {
    let mut main = vec![0x10];

    main.extend(immediate(0x4007));
    main.push(0x00);
    main.extend_from_slice(&[0x70, 0x0A, 0x7F, 0x01, 0x00, 0x01]);
    main.extend(immediate(42));
    main.extend_from_slice(&[0x70, 0x0B, 0x7F, 0x01, 0x00, 0x01]);
    main.extend(immediate(42));
    main.extend_from_slice(QUIT);

    let voice = recorded(plain(main));
    let traces: Vec<_> = voice
        .noted
        .iter()
        .filter(|(name, _)| name == "trace")
        .cloned()
        .collect();

    assert_eq!(traces, vec![noted("trace", "a(7) a:42")]);
}

// SCRIPT_ON fails when the voice refuses a transcript; SCRIPT_OFF
// passes quietly.
#[test]
fn script_on_fails_without_a_transcript() {
    let main = vec![0x70, 0x09, 0x70, 0x08];

    assert!(spoken(plain(caught(&main)), &[]).contains('9'));
}

// -- VM_INFO -----------------------------------------------------------

// The div width comes back as a boxed number; unknown numeric
// selectors politely answer zero; feature answers land raw.
#[test]
fn vm_info_answers_by_selector() {
    let mut main = vec![0x74, 0x20, 0x01];

    main.extend(printed(1));
    main.extend_from_slice(&[0x74, 0x3E, 0x02]);
    main.extend(printed(2));
    main.extend_from_slice(&[0x74, 0x40, 0x03, 0x50, 0x83]);
    main.extend(immediate(0x4000));
    main.push(0x03);
    main.extend(printed(3));
    main.extend_from_slice(&[0x74, 0x42, 0x04, 0x50, 0x84]);
    main.extend(immediate(0x4000));
    main.push(0x04);
    main.extend(printed(4));
    main.extend_from_slice(&[0x74, 0x00, 0x05]);
    main.extend(printed(5));
    main.extend_from_slice(QUIT);

    let told = spoken(plain(main), &[]);

    assert!(told.contains("80"), "{told}");
    assert!(told.contains('1'), "{told}");
    assert!(told.contains('0'), "{told}");
}

// The peak-memory selectors count every non-unused word, and the
// height and transcript selectors answer their zeros.
#[test]
fn vm_info_counts_the_peaks() {
    let mut main = vec![0x74, 0x01, 0x01];

    main.extend(printed(1));
    main.extend_from_slice(&[0x74, 0x02, 0x02]);
    main.extend(printed(2));
    main.extend_from_slice(&[0x74, 0x21, 0x03]);
    main.extend(printed(3));
    main.extend_from_slice(&[0x74, 0x50, 0x04, 0x50, 0x84]);
    main.extend(immediate(0x4000));
    main.push(0x04);
    main.extend(printed(4));
    main.extend_from_slice(QUIT);

    assert!(spoken(plain(main), &[]).contains('0'));
}

// The numeric VM_INFO selectors between the peaks and the div
// measures answer a boxed zero.
#[test]
fn vm_info_middle_selectors_answer_zero() {
    let mut main = vec![0x74, 0x1F, 0x01];

    main.extend(printed(1));
    main.extend_from_slice(QUIT);

    assert!(spoken(plain(main), &[]).contains('0'));
}

// -- save, undo, restart, restore --------------------------------------

// SAVE fails politely when the voice keeps no files.
#[test]
fn save_fails_without_a_file_keeper() {
    let mut main = vec![0x72];

    main.extend(absolute(0));

    assert!(spoken(plain(caught(&main)), &[]).contains('9'));
}

// A story that saves, restores, and reports which path ran.
//
// The save continues to print 1, a later restore lands at the
// saved address to print 2, and a failed restore falls through to
// print 3.
fn saving_story() -> Story {
    let landing = 1 + 1 + 3 + shown(0x4001).len() + shown(0x4003).len() + QUIT.len() + 2;
    let mut body = vec![0x72];

    body.extend(absolute(landing));
    body.extend(shown(0x4001));
    body.extend_from_slice(&[0x70, 0x02]);
    body.extend(shown(0x4003));
    body.extend_from_slice(QUIT);
    body.extend(shown(0x4002));
    body.extend_from_slice(QUIT);

    plain(body)
}

// A granted save continues, and the restore that follows revives
// the kept file, landing at the address the save named.
#[test]
fn a_kept_savefile_revives_at_its_landing() {
    let story = saving_story();
    let voice = KeepingVoice::new(&story);
    let mut machine = Machine::new(story, voice, Some(7)).unwrap();

    machine.run(None).unwrap();

    let told = machine.voice.plain.told().to_string();

    assert!(
        told.contains('1') && told.contains('2') && !told.contains('3'),
        "{told}"
    );
}

// A refused save fails to the choice point.
#[test]
fn a_refused_save_fails() {
    let mut main = vec![0x72];

    main.extend(absolute(0));

    let story = plain(caught(&main));
    let mut voice = KeepingVoice::new(&story);

    voice.granting = false;

    let mut machine = Machine::new(story, voice, Some(7)).unwrap();

    machine.run(None).unwrap();

    assert!(machine.voice.plain.told().contains('9'));
}

// RESTORE with no saves continues as a failed restore.
#[test]
fn restore_without_saves_continues() {
    let mut main = vec![0x70, 0x02];

    main.extend(shown(0x4005));
    main.extend_from_slice(QUIT);

    assert!(spoken(plain(main), &[]).contains('5'));
}

// A restore with nothing kept, and one handed unreadable bytes,
// both continue as failed restores.
#[test]
fn an_empty_or_unreadable_restore_continues() {
    for answering in [Answering::Hollow, Answering::Corrupt] {
        let story = saving_story();
        let mut voice = KeepingVoice::new(&story);

        voice.answering = answering;

        let mut machine = Machine::new(story, voice, Some(7)).unwrap();

        machine.run(None).unwrap();

        let told = machine.voice.plain.told().to_string();

        assert!(told.contains('1') && told.contains('3'), "{told}");
    }
}

// UNDO with nothing kept and nothing pruned fails.
#[test]
fn undo_with_nothing_kept_fails() {
    assert!(spoken(plain(caught(&[0x70, 0x03])), &[]).contains('9'));
}

// A long chain of SAVE_UNDOs prunes its oldest moments; draining
// the stack then lands on the pruned answer: a quiet continue.
#[test]
fn undo_prunes_and_then_continues() {
    let saves = 54;
    let landing = 1 + saves * 4;
    let mut main = Vec::new();

    for _ in 0..saves {
        main.push(0xF2);
        main.extend(absolute(landing));
    }

    main.extend_from_slice(&[0x70, 0x03]);
    main.extend(shown(0x4008));
    main.extend_from_slice(QUIT);

    assert!(spoken(plain(main), &[]).contains('8'));
}

// RESTART rewinds the whole machine to its opening state; the
// story asks again, and a different key ends it.
#[test]
fn restart_rewinds_to_the_opening() {
    let mut head = vec![0xF3, 0x00];

    head.extend(shown(0x3E2A));
    head.push(0x39);
    head.extend(immediate(0x3E72));

    let at = 1 + head.len() + 1 + 3 + QUIT.len();
    let mut body = head;

    body.push(0x80);
    body.extend(absolute(at));
    body.extend_from_slice(QUIT);
    body.extend_from_slice(&[0x70, 0x01]);

    assert!(spoken(plain(body), &["r", "q"]).contains("*\n*"));
}

// A restore revives the divs that were open at the save, entering
// them again on the voice in order.
#[test]
fn a_restore_reenters_the_open_divs() {
    let head = vec![0x66, 0x05, 0x72];
    let landing = 1 + head.len() + 3 + 1 + shown(0x4001).len() + shown(0x4003).len() + 4;
    let mut body = head;

    body.extend(absolute(landing));
    body.push(0xE6);
    body.extend(shown(0x4001));
    body.extend_from_slice(&[0x70, 0x02]);
    body.extend(shown(0x4003));
    body.extend_from_slice(QUIT);
    body.extend(shown(0x4002));
    body.extend_from_slice(QUIT);

    let story = plain(body);
    let voice = KeepingVoice::new(&story);
    let mut machine = Machine::new(story, voice, Some(7)).unwrap();

    machine.run(None).unwrap();

    let told = machine.voice.plain.told().to_string();

    assert!(
        told.contains('1') && told.contains('2') && !told.contains('3'),
        "{told}"
    );
}

// -- input delivery ----------------------------------------------------

// GET_KEY takes special keys by their reserved codes, extended
// characters through the table, and digits as numbers.
#[test]
fn keys_arrive_by_kind() {
    let mut main = vec![0xF3, 0x00];

    main.extend(printed(0));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main.clone());

    shape.lang = Some(langed(Langed {
        extended: vec![(0x80, 0x80, 0x2192)],
        ..Langed::default()
    }));

    let story = crafted(&shape);
    let voice = PlainVoice::new(&story).unwrap();
    let mut machine = Machine::new(story, voice, Some(7)).unwrap();

    machine.run(None).unwrap();
    machine.deliver_key(0x2192).unwrap();

    assert!(machine.voice.told().contains('\u{2192}'));

    assert!(spoken(plain(main), &["7"]).contains('7'));
}

// A key the story cannot spell leaves the wait standing.
#[test]
fn an_unspellable_key_leaves_the_wait() {
    let mut main = vec![0xF3, 0x00];

    main.extend(printed(0));
    main.extend_from_slice(QUIT);

    let story = plain(main);
    let voice = PlainVoice::new(&story).unwrap();
    let mut machine = Machine::new(story, voice, Some(7)).unwrap();

    machine.run(None).unwrap();

    assert_eq!(machine.deliver_key(0x3A9).unwrap(), Wait::Key);
    assert_eq!(machine.deliver_key(0x10).unwrap(), Wait::Quit);
}

// An unspellable character in a line becomes the question mark,
// the reference engine's own shrug.
#[test]
fn an_unspellable_line_character_shrugs() {
    let mut main = vec![0x73, 0x00, 0x12, 0x01, 0x02, 0x80];

    main.extend(printed(1));
    main.extend_from_slice(QUIT);

    assert!(spoken(plain(main), &["\u{3c9}"]).contains('?'));
}

// A heap too small to hold the parsed input reports exhaustion
// through the usual restart, and the wait is answered by the
// error entry instead.
#[test]
fn input_past_the_heap_reports_error_one() {
    let mut main = vec![0x73, 0x00];

    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(guarded(&main));

    shape.heap = 6;

    assert!(spoken(crafted(&shape), &["a b c d e f g h"]).contains('1'));
}

// The endings decoder takes a suffix off and finds the stem; an
// unknown word keeps every letter, digits told as numbers.
#[test]
fn the_endings_decoder_finds_stems() {
    let endings = vec![b's', 3, 0x00, 0x01, 0x00];
    let lang = langed(Langed {
        endings,
        ..Langed::default()
    });
    let mut main = vec![0x73, 0x00, 0x12, 0x01, 0x02, 0x80];

    main.extend(printed(1));
    main.extend_from_slice(&[0x1F, 0x81, 0x03]);
    main.extend(printed(3));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main.clone());

    shape.lang = Some(lang.clone());
    shape.dictionary = worded(&[b"look"]);

    let told = spoken(crafted(&shape), &["looks"]);

    assert!(
        told.contains("looks") && told.contains("[l o o k s]"),
        "{told}"
    );

    let mut unknown = Crafted::of(main);

    unknown.lang = Some(lang);
    unknown.dictionary = worded(&[b"look"]);

    let told = spoken(crafted(&unknown), &["zz9"]);

    assert!(told.contains("zz9") && told.contains("[z z 9]"), "{told}");
}

// walked() closes the telling on a script that runs dry.
#[test]
fn a_dry_script_closes_the_walk() {
    let mut main = vec![0x73, 0x00];

    main.extend_from_slice(QUIT);

    assert_eq!(walked(plain(main), "", Some(7)).unwrap(), "");
}

// -- output whitespace -------------------------------------------------

// The NBSP state survives to the next print, and a pending space
// before a key read lands through the voice.
#[test]
fn nbsp_rides_to_the_next_print() {
    let main = vec![
        0x60, 0x01, 0x70, 0x13, 0x60, 0x00, 0xE2, 0xF3, 0x00, 0x70, 0x00,
    ];

    assert!(spoken(plain(main), &["q"]).contains("a $"));
}

// -- the collect-words level -------------------------------------------

// With the collect-words level raised, every output opcode holds
// its tongue: nothing reaches the voice until the level drops.
#[test]
fn a_raised_collect_level_silences_output() {
    let mut main = vec![0x70, 0x0C, 0x62, 0xE2, 0x63, 0xE3, 0x64];

    main.extend(immediate(0x4003));
    main.extend_from_slice(&[0x66, 0x00, 0xE6, 0x6E, 0x00, 0xEE, 0x6F, 0x00, 0x00, 0xE7]);
    main.push(0x69);
    main.extend(immediate(0x4001));
    main.push(0xE9);
    main.push(0x68);
    main.extend(immediate(0x4001));
    main.extend_from_slice(&[0xE8, 0x6A, 0xEA, 0x6B, 0x02, 0xEB, 0x02, 0x70, 0x04, 0x6C]);
    main.extend(immediate(0x4001));
    main.push(0x6D);
    main.extend(immediate(0x4001));
    main.extend(immediate(0x4004));
    main.extend_from_slice(&[0x70, 0x05, 0x70, 0x0E, 0x70, 0x13, 0x70, 0x06, 0x70, 0x0D]);
    main.extend(shown(0x4008));
    main.extend_from_slice(QUIT);

    let mut voice = recorded(plain(main));

    assert_eq!(voice.plain.told(), "8");
    assert!(voice.noted.is_empty());
}

// PRINT_VAL under a raised level collects the value onto the aux
// stack instead of speaking it.
#[test]
fn print_val_under_the_level_collects() {
    let mut main = vec![0x94, 0x70, 0x0C];

    main.extend(shown(0x4005));
    main.extend_from_slice(&[0x70, 0x0D, 0x17, 0x01]);
    main.extend(printed(1));
    main.extend_from_slice(QUIT);

    assert_eq!(spoken(plain(main), &[]), "[5]");
}

// -- serialization corners ---------------------------------------------

// An extended dict word rides the aux stack whole, both ways.
#[test]
fn aux_serialization_carries_the_extdict() {
    let endings = vec![b's', 3, 0x00, 0x01, 0x00];
    let mut main = vec![
        0x73, 0x00, 0x12, 0x01, 0x02, 0x80, 0x94, 0x14, 0x81, 0x16, 0x03,
    ];

    main.extend(printed(3));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.lang = Some(langed(Langed {
        endings,
        ..Langed::default()
    }));
    shape.dictionary = worded(&[b"look"]);

    assert!(spoken(crafted(&shape), &["looks"]).contains("looks"));
}

// A raw aux push with no room reports exhaustion 2.
#[test]
fn a_full_aux_raw_push_reports_error_two() {
    let mut shape = Crafted::of(guarded(&[0x95, 0x05]));

    shape.aux = 0;

    assert!(spoken(crafted(&shape), &[]).contains('2'));
}

// A long-term push past the area's end mid-list reports 6.
#[test]
fn a_longterm_push_past_the_end_reports_error_six() {
    let (init, ram) = roomy(0, 8, 8, 3);
    let mut main = vec![0x73, 0x00, 0xA6, 0x00, 0x80];

    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(guarded(&main));

    shape.init = Some(init);
    shape.ram = Some(ram);

    assert!(spoken(crafted(&shape), &["a b c"]).contains('6'));
}

// A number too long for the tag parses as an unknown word, every
// digit kept.
#[test]
fn an_oversized_number_stays_a_word() {
    let mut main = vec![0x73, 0x00, 0x12, 0x01, 0x02, 0x80];

    main.extend(printed(1));
    main.extend_from_slice(QUIT);

    assert!(spoken(plain(main), &["99999"]).contains("99999"));
}

// JOIN_WORDS flattens an extended dict word -- stem and ending --
// back into its spelled characters.
#[test]
fn join_words_flattens_an_extdict() {
    let endings = vec![b's', 3, 0x00, 0x01, 0x00];
    let mut main = vec![0x73, 0x00, 0x9F, 0x80, 0x01];

    main.extend(printed(1));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.lang = Some(langed(Langed {
        endings,
        ..Langed::default()
    }));
    shape.dictionary = worded(&[b"look"]);

    assert!(spoken(crafted(&shape), &["looks zz"]).contains("lookszz"));
}

// -- unification corners -----------------------------------------------

// Unifying a variable with itself binds nothing and succeeds.
#[test]
fn a_variable_unifies_with_itself() {
    let mut main = vec![0x11, 0x01, 0x10, 0x81, 0x81];

    main.extend(shown(0x4006));
    main.extend_from_slice(QUIT);

    assert!(spoken(plain(main), &[]).contains('6'));
}

// An extended dict word unifies with its own stem word under
// IF_UNIFY's would-unify walk.
#[test]
fn an_extdict_unifies_with_its_stem() {
    let endings = vec![b's', 3, 0x00, 0x01, 0x00];
    let mut head = vec![0x73, 0x00, 0x12, 0x01, 0x02, 0x80, 0x37, 0x81];

    head.extend(immediate(0x2000));

    let mut fall = shown(0x4001);

    fall.extend_from_slice(QUIT);

    let at = 1 + head.len() + 3 + fall.len();
    let mut body = head;

    body.extend(absolute(at));
    body.extend(shown(0x4001));
    body.extend_from_slice(QUIT);
    body.extend(shown(0x4002));
    body.extend_from_slice(QUIT);

    let mut shape = Crafted::of(body);

    shape.lang = Some(langed(Langed {
        endings,
        ..Langed::default()
    }));
    shape.dictionary = worded(&[b"look"]);

    assert!(spoken(crafted(&shape), &["looks"]).contains('2'));
}

// ASSIGN's unify variant accepts an extdict against its stem too.
#[test]
fn assign_unifies_an_extdict_with_its_stem() {
    let endings = vec![b's', 3, 0x00, 0x01, 0x00];
    let mut main = vec![0x73, 0x00, 0x12, 0x01, 0x02, 0x80, 0x10];

    main.extend(immediate(0x2000));
    main.push(0x81);
    main.extend(shown(0x4006));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.lang = Some(langed(Langed {
        endings,
        ..Langed::default()
    }));
    shape.dictionary = worded(&[b"look"]);

    assert!(spoken(crafted(&shape), &["looks"]).contains('6'));
}

// UNLINK walks a chain to its end without finding the key, and
// leaves it standing.
#[test]
fn unlink_passes_a_missing_key() {
    let (init, ram) = roomy(2, 8, 8, 16);
    let mut main = vec![0xA4, 0x04];

    main.extend(immediate(0x0001));
    main.extend_from_slice(&[0xAD, 0x04, 0x04]);
    main.extend(immediate(0x0002));
    main.extend(shown(0x4006));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.init = Some(init);
    shape.ram = Some(ram);

    assert!(spoken(crafted(&shape), &[]).contains('6'));
}

// -- half-word and flag variants ---------------------------------------

// LOAD_BYTE and STORE_BYTE reach both halves of a word, and a
// raised flag answers IF_FLAG on a named object.
#[test]
fn bytes_and_flags_reach_their_halves() {
    let (init, ram) = roomy(1, 8, 8, 16);
    let mut main = vec![0x25];

    main.extend(immediate(0x0001));
    main.push(0x00);
    main.extend(immediate(0xAB));
    main.push(0x25);
    main.extend(immediate(0x0001));
    main.push(0x01);
    main.extend(immediate(0xCD));
    main.push(0x21);
    main.extend(immediate(0x0001));
    main.extend_from_slice(&[0x00, 0x01, 0x50, 0x81]);
    main.extend(immediate(0x4000));
    main.push(0x01);
    main.extend(printed(1));
    main.push(0x21);
    main.extend(immediate(0x0001));
    main.extend_from_slice(&[0x01, 0x02, 0x50, 0x82]);
    main.extend(immediate(0x4000));
    main.push(0x02);
    main.extend(printed(2));
    main.push(0x28);
    main.extend(immediate(0x0001));
    main.push(0x21);
    main.push(0x4B);
    main.extend(immediate(0x0001));
    main.push(0x21);
    main.extend(absolute(0));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.init = Some(init);
    shape.ram = Some(ram);

    let told = spoken(crafted(&shape), &[]);

    assert!(told.contains("171") && told.contains("205"), "{told}");
}

// -- the trace and info tails ------------------------------------------

// A tracepoint shape without dollar signs passes through plain.
#[test]
fn a_plain_tracepoint_shape_passes_through() {
    let mut main = vec![0x70, 0x0A, 0x7F, 0x01, 0x01, 0x01];

    main.extend(immediate(9));
    main.extend_from_slice(QUIT);

    let voice = recorded(plain(main));

    assert!(voice.noted.contains(&noted("trace", "a(a) a:9")));
}

// CLEAR_OLD and CLEAR_STATUS pass to the voice outside a span.
#[test]
fn clear_old_and_status_reach_the_voice() {
    let voice = recorded(plain(vec![0x70, 0x10, 0x70, 0x12, 0x70, 0x00]));

    assert!(voice.noted.contains(&noted("clear_old", "")));
    assert!(voice.noted.contains(&noted("clear_status", "")));
}

// -- the last corners --------------------------------------------------

// A long-term chunk holding the unbound marker revives as a fresh
// variable -- reachable only by a story writing its own long-term
// words, which STORE_WORD's reach across RAM permits.
#[test]
fn a_handwritten_longterm_variable_revives() {
    let (init, ram) = snug();
    let mut main = vec![0x50];

    main.extend(immediate(0x7FFF));
    main.extend(immediate(0x0A));
    main.push(0x01);
    main.extend_from_slice(&[0xA4, 0x00, 0x81, 0xA4, 0x08]);
    main.extend(immediate(3));
    main.push(0x50);
    main.extend(immediate(0x7FFF));
    main.extend(immediate(1));
    main.push(0x02);
    main.extend_from_slice(&[0xA4, 0x0A, 0x82, 0xA2, 0x00, 0x03]);
    main.extend(printed(3));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.init = Some(init);
    shape.ram = Some(ram);

    assert!(spoken(crafted(&shape), &[]).contains('$'));
}

// JOIN_WORDS fails on a nested word list that itself refuses --
// here an unknown word hand-built around a stop character.
#[test]
fn join_words_refuses_a_nested_stop() {
    let mut main = vec![0x15];

    main.extend(immediate(0x3F00));
    main.push(0x15);
    main.extend(immediate(0x3E2E));
    main.extend_from_slice(&[0x15, 0xC0, 0x01, 0x15, 0x81, 0x00, 0x16, 0x01]);
    main.extend_from_slice(&[0x12, 0x81, 0x02, 0x03, 0x10]);
    main.extend(immediate(0x3F00));
    main.push(0x82);
    main.extend_from_slice(&[0x9F, 0x83, 0x04]);

    let mut shape = Crafted::of(caught(&main));

    shape.lang = Some(langed(Langed {
        stops: b".".to_vec(),
        ..Langed::default()
    }));

    assert!(spoken(crafted(&shape), &[]).contains('9'));
}

// JOIN_WORDS fails on a list element no word could ever hold.
#[test]
fn join_words_refuses_a_wordless_element() {
    let mut main = vec![0x13, 0x00, 0x01, 0x02, 0x03, 0x10];

    main.extend(immediate(0x3F00));
    main.push(0x82);
    main.extend_from_slice(&[0x9F, 0x83, 0x04]);

    assert!(spoken(plain(caught(&main)), &[]).contains('9'));
}

// The polite skips: STORE_VAL of null to a non-object, RESET_FLAG
// of a non-object, and SET_PARENT of a non-object all pass over.
#[test]
fn non_objects_are_politely_skipped() {
    let (init, ram) = roomy(1, 8, 8, 16);
    let mut main = vec![0x26];

    main.extend(immediate(0x4005));
    main.push(0x00);
    main.extend(immediate(0));
    main.push(0x29);
    main.extend(immediate(0x4005));
    main.push(0x00);
    main.push(0x2E);
    main.extend(immediate(0x4005));
    main.extend(immediate(0));
    main.extend(shown(0x4006));
    main.extend_from_slice(QUIT);

    let mut shape = Crafted::of(main);

    shape.init = Some(init);
    shape.ram = Some(ram);

    assert!(spoken(crafted(&shape), &[]).contains('6'));
}

// ENTER_LINK passes over list elements that spell nothing.
#[test]
fn a_link_skips_the_unspellable() {
    let mut main = vec![0x12, 0x01, 0x02, 0x03, 0x10];

    main.extend(immediate(0x3F00));
    main.push(0x82);
    main.extend_from_slice(&[0x69, 0x83, 0xE9]);
    main.extend_from_slice(QUIT);

    let voice = recorded(plain(main));

    assert!(voice.noted.contains(&noted("enter_link", "")));
}

// SCRIPT_ON continues when the voice grants a transcript.
#[test]
fn script_on_continues_when_granted() {
    struct ScriptingVoice {
        plain: PlainVoice,
    }

    impl Voice for ScriptingVoice {
        delegated!(script_on);

        fn script_on(&mut self) -> bool {
            true
        }
    }

    let mut main = vec![0x70, 0x08];

    main.extend(shown(0x4006));
    main.extend_from_slice(QUIT);

    let story = plain(main);
    let voice = ScriptingVoice {
        plain: PlainVoice::new(&story).unwrap(),
    };
    let mut machine = Machine::new(story, voice, Some(7)).unwrap();

    machine.run(None).unwrap();

    assert!(machine.voice.plain.told().contains('6'));
}

// -- the savefile round trip -------------------------------------------

// The whole round trip: a mid-game state encodes to an AASV form
// and revives identical, landing where it was told to; the open
// divs travel; a foreign HEAD is refused; short DATA is refused.
#[test]
fn a_state_survives_the_round_trip() {
    let mut main = vec![0x73, 0x00];

    main.extend_from_slice(QUIT);

    let story = plain(main.clone());
    let voice = PlainVoice::new(&story).unwrap();
    let mut machine = Machine::new(story, voice, Some(7)).unwrap();

    machine.run(None).unwrap();
    machine.deliver_line("west").unwrap();

    let mut state = machine.captured(9);

    state.divs = vec![3, 7];

    let data = saves::kept(&machine.story, &state);

    assert_eq!(&data[..4], b"FORM");
    assert_eq!(&data[8..12], b"AASV");
    assert_eq!(saves::revived(&machine.story, &data).unwrap(), state);

    let mut foreign = Crafted::of(main);

    foreign.heap = 65;

    let other = crafted(&foreign);
    let told = saves::revived(&other, &data)
        .expect_err("foreign")
        .to_string();

    assert!(told.contains("another game or another release"), "{told}");

    let mut hollow = b"AASV".to_vec();

    hollow.extend(iff_chunk(b"HEAD", &machine.story.summed(b"HEAD").payload));

    let told = saves::revived(&machine.story, &iff_chunk(b"FORM", &hollow))
        .expect_err("hollow")
        .to_string();

    assert!(told.contains("missing its DATA"), "{told}");

    let mut short = b"AASV".to_vec();

    short.extend(iff_chunk(b"HEAD", &machine.story.summed(b"HEAD").payload));
    short.extend(iff_chunk(b"DATA", &[0x01, 0x02]));
    short.extend(iff_chunk(b"REGS", &[0u8; 156]));

    let told = saves::revived(&machine.story, &iff_chunk(b"FORM", &short))
        .expect_err("short")
        .to_string();

    assert!(told.contains("unpacks to 2 bytes"), "{told}");

    let wrong = saves::revived(&machine.story, &iff_chunk(b"FORM", b"AAVM"))
        .expect_err("wrong form")
        .to_string();

    assert!(wrong.contains("FORM AASV"), "{wrong}");
}

// An INIT already long enough pads nothing.
#[test]
fn a_long_init_needs_no_padding() {
    let story = plain(QUIT.to_vec());

    assert_eq!(
        saves::grounded(&story, 4),
        story.summed(b"INIT").payload[..4].to_vec()
    );
}
