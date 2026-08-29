//! The Å-machine engine: registers, twin heaps, and the Prolog heart.
//!
//! The machine is a register machine with a main heap that carries
//! terms, environment frames, and choice frames, and an auxiliary
//! heap that carries a scratch stack from one end and the trail
//! from the other (Aa-machine: Runtime data). Failure unwinds to
//! the newest choice frame, undoing trailed bindings on the way; a
//! runtime error restarts the machine at the shared entry point
//! with the error's number in R00 (Aa-machine: Opcode semantics).
//!
//! run() executes until the story quits or asks for input,
//! answering a Wait; deliver_line and deliver_key parse the
//! player's answer into the machine's own values and resume. The
//! whole instruction set is dispatched by full opcode byte from
//! one match, each handler holding its own spec citation.
//!
//! The dice are the reference implementation's own multiplier
//! carried deliberately -- state times $015a4e35 plus one, the top
//! bits told -- so a seeded Voxam run is comparable word for word
//! with a seeded run of the community fork's engine.
//!
//! Two reshapings from the reference, in the standing manner:
//! failure and the runtime faults are error values threaded
//! through the handlers rather than exceptions, and the machine is
//! generic over its Voice so the telling can be read back out of
//! `machine.voice` when the run pauses.

use std::collections::{HashMap, HashSet};

use crate::aamachine::output::{PlainVoice, Voice};
use crate::aamachine::saves::{self, State};
use crate::aamachine::story::Story;
use crate::aamachine::text::Speech;
use crate::errors::VoxamError;

/// The unused-word stamp, handy for measuring peak memory
/// (Aa-machine: Runtime data).
pub const UNUSED: u16 = 0x3F3F;

/// The empty list literal (Aa-machine: Runtime data).
pub const EMPTY: u16 = 0x3F00;

// The runtime error numbers, already dressed as tagged integers
// (Aa-machine: Runtime data).
pub const HEAP_FULL: u16 = 0x4001;
pub const AUX_FULL: u16 = 0x4002;
pub const EXPECTED_OBJECT: u16 = 0x4003;
pub const EXPECTED_BOUND: u16 = 0x4004;
pub const LONGTERM_FULL: u16 = 0x4006;
pub const BAD_OUTPUT_STATE: u16 = 0x4007;

// The whitespace states, in their ordering (Aa-machine: Runtime
// data).
const AUTO: u8 = 0;
const NOSPACE: u8 = 1;
const NBSP: u8 = 2;
const PENDING: u8 = 3;
const SPACE: u8 = 4;
const LINE: u8 = 5;
const PAR: u8 = 6;

// How many undo states the machine keeps before pruning, the
// reference engine's own allowance.
const UNDO_KEPT: usize = 50;

// The biggest boxed integer (Aa-machine: Runtime data).
const NUMBER_TOP: u16 = 0x3FFF;

// A word is sixteen flags, numbered from the most significant bit
// (Aa-machine: Runtime data).
const BITS_PER_WORD: u16 = 16;

// The reference random stream: a 32-bit linear congruence whose
// top bits are told.
const DICE_STEP: u32 = 0x015A_4E35;

// Operand encoding markers (Aa-machine: Story file): an operand's
// first byte says where the rest of it lives.
const FROM_ENV: u8 = 0xC0;
const FROM_REGISTER: u8 = 0x80;
const CLOSE_TOP: u8 = 0x40;
const RELATIVE_TOP: u8 = 0x80;
const UNIFY_DEST: u8 = 0x80;
const ENV_DEST: u8 = 0x40;

// The opcode bytes a shared handler tells apart by name.
const PAIR_OF_DESTS: u8 = 0x12;
const PAIR_OF_WORD: u8 = 0x13;
const RAW_ZERO: u8 = 0x94;
const RAW_WORD: u8 = 0x15;
const OLD_LEAVE_STATUS: u8 = 0xE7;

// A SIM at or above this is no simple cut at all: choice frames
// never naturally dip below it (Aa-machine: PROCEED).
const NO_CUT: usize = 0x8000;

// The serialized stream's markers (Aa-machine: Runtime data).
const UNBOUND_MARK: u16 = 0x8000;
const EXTDICT_MARK: u16 = 0x8100;
const LIST_MARK: u16 = 0xC000;

// The character set's landmarks (Aa-machine: Text).
const SPACE_CODE: u8 = 0x20;
const PRINTABLE_START: u32 = 0x20;
const PRINTABLE_TOP: u32 = 0x7F;
const UPPER_A: u32 = 0x41;
const UPPER_Z: u32 = 0x5A;
const EXTENDED_START: u16 = 0x80;

// The endings decoder keeps at least this many characters in the
// stem (Aa-machine: GET_INPUT).
const STEM_KEPT: usize = 2;

// A wordmap payload byte at or above this opens a two-byte object
// id (Aa-machine: MAPS).
const WIDE_SEAT: u16 = 0xE0;

// VM_INFO's frame: the peak-memory areas, and the last defined
// selector (Aa-machine: VM_INFO).
const PEAK_AREAS: u8 = 3;
const SELECTOR_TOP: u8 = 0x7F;

// The LANG special characters come in three null-terminated sets
// from format 0.4 (Aa-machine: LANG).
const SPECIAL_SETS: usize = 3;

// The live-value tags: a variable number of upper bits, the value
// in the rest (Aa-machine: Runtime data).
const TAG_MASK: u16 = 0xE000;
const REFERENCE_TAG: u16 = 0x8000;
const PAIR_TAG: u16 = 0xC000;
const EXTDICT_TAG: u16 = 0xE000;
const NUMBER_TAG: u16 = 0x4000;
const WORD_TAG: u16 = 0x2000;
const CHAR_TAG: u16 = 0x3E00;
const CHAR_MASK: u16 = 0xFF00;
const VALUE_MASK: u16 = 0x1FFF;
const DIGIT_LOW: u8 = 0x30;
const DIGIT_HIGH: u8 = 0x39;

/// Whether a value is an indirect reference (Aa-machine tag 100).
fn referenced(value: u16) -> bool {
    (value & TAG_MASK) == REFERENCE_TAG
}

/// Whether a value is a pair (Aa-machine tag 110).
fn paired(value: u16) -> bool {
    (value & TAG_MASK) == PAIR_TAG
}

/// Whether a value is an extended dict word (Aa-machine tag 111).
fn extdicted(value: u16) -> bool {
    value >= EXTDICT_TAG
}

/// Whether a value is a boxed integer (Aa-machine tag 01).
fn numbered(value: u16) -> bool {
    (NUMBER_TAG..REFERENCE_TAG).contains(&value)
}

/// Whether a value is a character literal (Aa-machine: Words).
fn chared(value: u16) -> bool {
    (value & CHAR_MASK) == CHAR_TAG
}

/// Whether a value is a dictionary word literal (Aa-machine: Words).
fn dicted(value: u16) -> bool {
    (WORD_TAG..CHAR_TAG).contains(&value)
}

/// Whether a value is an object literal (Aa-machine: Words).
fn objected(value: u16) -> bool {
    (1..=VALUE_MASK).contains(&value)
}

/// Whether a charset code is a decimal digit.
fn digited(code: u8) -> bool {
    (DIGIT_LOW..=DIGIT_HIGH).contains(&code)
}

/// Whether IF_WORD counts the value: dict, char, or extdict.
fn wordish(value: u16) -> bool {
    (WORD_TAG..EMPTY).contains(&value) || extdicted(value)
}

/// Whether ENTER_LINK spells the value into its click words.
fn linkable(value: u16) -> bool {
    (WORD_TAG..REFERENCE_TAG).contains(&value) || extdicted(value)
}

/// How execution leaves the straight path: a Prolog failure, a
/// numbered runtime fault, or a refusal that ends the session.
/// The reference raises these; here they thread as error values.
enum Slip {
    Missed,
    Fault(u16),
    Refused(VoxamError),
}

impl From<VoxamError> for Slip {
    fn from(error: VoxamError) -> Self {
        Slip::Refused(error)
    }
}

type Fall<T> = Result<T, Slip>;

fn machine_error(message: String) -> VoxamError {
    VoxamError::AAMachine(message)
}

/// What the machine is waiting for when run() returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// The story has quit.
    Quit,
    /// GET_INPUT wants a whole line.
    Line,
    /// GET_KEY wants one keypress.
    Key,
}

/// One running Å-machine, speaking through a Voice.
pub struct Machine<V: Voice> {
    /// The story being run, still consultable.
    pub story: Story,
    /// The voice everything is spoken through.
    pub voice: V,
    /// False once the story has quit.
    pub running: bool,
    seed: Option<u32>,
    speech: Speech,
    major: u8,
    code: Vec<u8>,
    init: Vec<u8>,
    maps: Vec<u8>,
    dict: Vec<u8>,
    lang: Vec<u8>,
    tags: Option<Vec<u8>>,
    heap: Vec<u16>,
    aux: Vec<u16>,
    ram: Vec<u16>,
    endings_at: usize,
    cased: HashMap<char, u8>,
    upcased: HashMap<char, String>,
    stops: HashSet<u8>,
    unspaced_before: HashSet<u8>,
    unspaced_after: HashSet<u8>,
    sought: HashMap<Vec<u8>, u16>,
    regs: Vec<u16>,
    inst: usize,
    cont: usize,
    top: usize,
    env: usize,
    cho: usize,
    sim: usize,
    auxp: usize,
    trl: usize,
    sta: usize,
    stc: usize,
    cwl: i64,
    spc: u8,
    tmp: usize,
    nob: u16,
    ltb: usize,
    ltt: usize,
    divs: Vec<u16>,
    upper: bool,
    trace: bool,
    in_status: usize,
    n_span: i64,
    n_link: i64,
    dice: u32,
    undo: Vec<State>,
    pruned: bool,
    held: State,
}

impl<V: Voice> Machine<V> {
    /// Ready a story for its first run.
    ///
    /// Fails for a LANG chunk whose decoder or special-character
    /// offsets lie outside it.
    pub fn new(story: Story, voice: V, seed: Option<u32>) -> Result<Self, VoxamError> {
        let speech = Speech::new(&story)?;
        let lang = story.summed(b"LANG").payload.clone();
        let dict = story.summed(b"DICT").payload.clone();
        let mut machine = Self {
            speech,
            major: story.version.0,
            code: story.summed(b"CODE").payload.clone(),
            init: story.summed(b"INIT").payload.clone(),
            maps: story.summed(b"MAPS").payload.clone(),
            tags: story.chunk(b"TAGS").map(|held| held.payload.clone()),
            heap: vec![UNUSED; usize::from(story.heap_size)],
            aux: vec![UNUSED; usize::from(story.aux_size)],
            ram: vec![UNUSED; usize::from(story.ram_size)],
            endings_at: be(&lang, 4, 6),
            cased: cased(&lang, &story.extended),
            upcased: upcased(&lang, &story.extended),
            stops: HashSet::new(),
            unspaced_before: HashSet::new(),
            unspaced_after: HashSet::new(),
            sought: sought(&dict),
            running: true,
            seed,
            regs: vec![0; 64],
            inst: 1,
            cont: 0,
            top: 0,
            env: 0,
            cho: 0,
            sim: 0,
            auxp: 0,
            trl: 0,
            sta: 0,
            stc: 0,
            cwl: 0,
            spc: LINE,
            tmp: 0,
            nob: 0,
            ltb: 0,
            ltt: 0,
            divs: Vec::new(),
            upper: false,
            trace: false,
            in_status: 0,
            n_span: 0,
            n_link: 0,
            dice: 0,
            undo: Vec::new(),
            pruned: false,
            held: State {
                counted: (0, 0, 0),
                ram: Vec::new(),
                aux: Vec::new(),
                heap: Vec::new(),
                regs: Vec::new(),
                flow: (0, 0, 0, 0, 0, 0),
                stacks: (0, 0, 0, 0, 0, 0),
                divs: Vec::new(),
            },
            lang,
            dict,
            story,
            voice,
        };

        let (stops, before, after) = stopped(&machine.lang, machine.story.version)?;

        machine.stops = stops;
        machine.unspaced_before = before;
        machine.unspaced_after = after;
        machine.reinit();
        machine.reset(0, true);
        machine.held = machine.captured(1);

        Ok(machine)
    }

    /// Execute until the story quits or waits.
    ///
    /// A delivered value is stored through the pending input
    /// instruction's destination first (Aa-machine: Opcode
    /// semantics). Fails for an instruction the engine does not
    /// carry, or a failure with no choice frame standing.
    pub fn run(&mut self, mut delivered: Option<u16>) -> Result<Wait, VoxamError> {
        loop {
            match self.stepped(delivered.take()) {
                Ok(Some(wait)) => return Ok(wait),
                Ok(None) => {}
                Err(Slip::Missed) => self.fallen()?,
                Err(Slip::Fault(code)) => self.faulted(code),
                Err(Slip::Refused(error)) => return Err(error),
            }
        }
    }

    fn stepped(&mut self, delivered: Option<u16>) -> Fall<Option<Wait>> {
        if let Some(answered) = delivered {
            let dest = self.fetched();

            self.store(dest, answered)?;
        }

        let op = self.fetched();

        self.op(op)
    }

    /// Answer a waiting GET_INPUT with the player's line.
    ///
    /// The line is lowercased through the story's own case table,
    /// split at whitespace and stop characters, and parsed word by
    /// word into machine values (Aa-machine: GET_INPUT).
    pub fn deliver_line(&mut self, text: &str) -> Result<Wait, VoxamError> {
        let codes: Vec<u8> = text.chars().map(|piece| self.encased(piece)).collect();
        let mut pieces: Vec<Vec<u8>> = Vec::new();
        let mut start = 0;

        for (at, &code) in codes.iter().enumerate() {
            if code == SPACE_CODE {
                if at != start {
                    pieces.push(codes[start..at].to_vec());
                }

                start = at + 1;
            } else if self.stops.contains(&code) {
                if at != start {
                    pieces.push(codes[start..at].to_vec());
                }

                pieces.push(vec![code]);
                start = at + 1;
            }
        }

        if start != codes.len() {
            pieces.push(codes[start..].to_vec());
        }

        let told = (|| -> Fall<u16> {
            let mut told = EMPTY;

            for piece in pieces.iter().rev() {
                let parsed = self.parsed(piece)?;

                told = self.pair(parsed, told)?;
            }

            Ok(told)
        })();

        match told {
            Ok(told) => {
                self.spc = LINE;

                self.run(Some(told))
            }
            Err(Slip::Fault(code)) => {
                self.faulted(code);
                self.spc = LINE;

                self.run(None)
            }
            Err(Slip::Missed) => unreachable!("parsing a line never fails, only faults"),
            Err(Slip::Refused(error)) => Err(error),
        }
    }

    /// Answer a waiting GET_KEY with one keypress.
    ///
    /// The code is a Unicode codepoint, or one of the reserved
    /// keypress codes -- $08, $0d, $10 to $13 (Aa-machine: Text).
    /// A key the story's character set cannot spell leaves the
    /// wait standing.
    pub fn deliver_key(&mut self, code: u32) -> Result<Wait, VoxamError> {
        let told: u8 = if (PRINTABLE_START..PRINTABLE_TOP).contains(&code) {
            if (UPPER_A..=UPPER_Z).contains(&code) {
                (code ^ 0x20) as u8
            } else {
                code as u8
            }
        } else if [0x08, 0x0D, 0x10, 0x11, 0x12, 0x13].contains(&code) {
            code as u8
        } else {
            char::from_u32(code)
                .and_then(|piece| self.cased.get(&piece).copied())
                .unwrap_or(0)
        };

        if told == 0 {
            return Ok(Wait::Key);
        }

        self.spc = SPACE;

        if digited(told) {
            return self.run(Some(0x4000 + u16::from(told) - 0x30));
        }

        self.run(Some(0x3E00 | u16::from(told)))
    }

    // -- the fetch stage -------------------------------------------------

    /// The next code byte, the instruction pointer advanced.
    fn fetched(&mut self) -> u8 {
        let told = self.code[self.inst];

        self.inst += 1;

        told
    }

    /// A VALUE or RAW operand (Aa-machine: Story file).
    fn value(&mut self) -> u16 {
        let told = self.fetched();

        if told >= FROM_ENV {
            return self.heap[self.env + 4 + usize::from(told & 0x3F)];
        }

        if told >= FROM_REGISTER {
            return self.regs[usize::from(told & 0x3F)];
        }

        (u16::from(told) << 8) | u16::from(self.fetched())
    }

    /// An INDEX operand (Aa-machine: Story file).
    fn index(&mut self) -> u16 {
        let told = self.fetched();

        if told >= FROM_ENV {
            return (u16::from(told & 0x3F) << 8) | u16::from(self.fetched());
        }

        u16::from(told)
    }

    /// A CODE operand, relative forms already resolved.
    fn target(&mut self) -> usize {
        let told = self.fetched();

        if told == 0 {
            return 0;
        }

        if told < CLOSE_TOP {
            return self.inst + usize::from(told);
        }

        if told < RELATIVE_TOP {
            let told = (usize::from(told & 0x3F) << 8) | usize::from(self.fetched());

            return self.inst + told - if told & 0x2000 != 0 { 0x4000 } else { 0 };
        }

        let told = (usize::from(told & 0x7F) << 16) | (usize::from(self.fetched()) << 8);

        told | usize::from(self.fetched())
    }

    /// A STRING operand: a shifted byte address into WRIT.
    fn string(&mut self) -> usize {
        let told = self.fetched();

        if told >= FROM_ENV {
            let told = (usize::from(told & 0x3F) << 16)
                | (usize::from(self.fetched()) << 8)
                | usize::from(self.fetched());

            return told << self.story.shift;
        }

        if told >= FROM_REGISTER {
            let told = (usize::from(told & 0x3F) << 8) | usize::from(self.fetched());

            return told << self.story.shift;
        }

        usize::from(told) << 1
    }

    /// A WORD or VWORD operand: two plain bytes.
    fn word(&mut self) -> u16 {
        (u16::from(self.fetched()) << 8) | u16::from(self.fetched())
    }

    // -- the store stage -------------------------------------------------

    /// The current value behind a destination byte's seat.
    fn slotted(&self, dest: u8) -> u16 {
        if dest & 0x40 != 0 {
            return self.heap[self.env + 4 + usize::from(dest & 0x3F)];
        }

        self.regs[usize::from(dest & 0x3F)]
    }

    /// Store or unify through a DEST byte (Aa-machine: Story file).
    fn store(&mut self, dest: u8, value: u16) -> Fall<()> {
        if dest >= UNIFY_DEST {
            self.unify(self.slotted(dest), value)
        } else if dest >= ENV_DEST {
            self.heap[self.env + 4 + usize::from(dest & 0x3F)] = value;

            Ok(())
        } else {
            self.regs[usize::from(dest)] = value;

            Ok(())
        }
    }

    // -- terms: deref, unify, allocation ---------------------------------

    /// Chase references to the value they hold (Aa-machine: ASSIGN).
    fn deref(&self, mut value: u16) -> u16 {
        while referenced(value) {
            let told = self.heap[usize::from(value & 0x1FFF)];

            if told == 0 {
                return value;
            }

            value = told;
        }

        value
    }

    /// Bind the variable at a heap address, the trail told.
    fn bound(&mut self, address: u16) -> Fall<()> {
        if self.trl <= self.auxp {
            return Err(Slip::Fault(AUX_FULL));
        }

        self.trl -= 1;
        self.aux[self.trl] = address;

        Ok(())
    }

    /// Make two values the same or fail (Aa-machine: ASSIGN).
    fn unify(&mut self, mut a: u16, mut b: u16) -> Fall<()> {
        loop {
            a = self.deref(a);
            b = self.deref(b);

            if referenced(a) && referenced(b) {
                if a != b {
                    let (older, newer) = if a < b { (a, b) } else { (b, a) };

                    self.bound(newer & 0x1FFF)?;
                    self.heap[usize::from(newer & 0x1FFF)] = older;
                }

                return Ok(());
            }

            if referenced(a) || referenced(b) {
                let (reference, told) = if referenced(a) { (a, b) } else { (b, a) };

                self.bound(reference & 0x1FFF)?;
                self.heap[usize::from(reference & 0x1FFF)] = told;

                return Ok(());
            }

            if extdicted(a) || extdicted(b) {
                if extdicted(a) {
                    a = self.heap[usize::from(a & 0x1FFF)];
                }

                if extdicted(b) {
                    b = self.heap[usize::from(b & 0x1FFF)];
                }
            } else if a == b {
                return Ok(());
            } else if paired(a) && paired(b) {
                self.unify(0x8000 | (a & 0x1FFF), 0x8000 | (b & 0x1FFF))?;
                a = 0x8000 | ((a & 0x1FFF) + 1);
                b = 0x8000 | ((b & 0x1FFF) + 1);
            } else {
                return Err(Slip::Missed);
            }
        }
    }

    /// Whether two values could unify, nothing bound (Aa-machine:
    /// IF_UNIFY).
    fn agreeable(&self, mut a: u16, mut b: u16) -> bool {
        loop {
            a = self.deref(a);
            b = self.deref(b);

            if referenced(a) || referenced(b) {
                return true;
            }

            if extdicted(a) || extdicted(b) {
                if extdicted(a) {
                    a = self.heap[usize::from(a & 0x1FFF)];
                }

                if extdicted(b) {
                    b = self.heap[usize::from(b & 0x1FFF)];
                }
            } else if a == b {
                return true;
            } else if paired(a) && paired(b) {
                if !self.agreeable(0x8000 | (a & 0x1FFF), 0x8000 | (b & 0x1FFF)) {
                    return false;
                }

                a = 0x8000 | ((a & 0x1FFF) + 1);
                b = 0x8000 | ((b & 0x1FFF) + 1);
            } else {
                return false;
            }
        }
    }

    /// Claim words at the heap's top; the old top comes back.
    fn claimed(&mut self, count: usize) -> Fall<usize> {
        let told = self.top;

        self.top += count;

        if self.top > self.env.min(self.cho) {
            return Err(Slip::Fault(HEAP_FULL));
        }

        Ok(told)
    }

    /// A fresh pair cell holding head and tail (Aa-machine:
    /// Runtime data).
    fn pair(&mut self, head: u16, tail: u16) -> Fall<u16> {
        let at = self.claimed(2)?;

        self.heap[at] = head;
        self.heap[at + 1] = tail;

        Ok(0xC000 | at as u16)
    }

    /// A fresh unbound variable on the heap.
    fn variable(&mut self) -> Fall<u16> {
        let at = self.claimed(1)?;

        self.heap[at] = 0;

        Ok(0x8000 | at as u16)
    }

    // -- frames ----------------------------------------------------------

    /// Push a choice frame keeping the first registers
    /// (Aa-machine: PUSH_CHOICE).
    fn pushed_choice(&mut self, kept: usize, handler: usize) -> Fall<()> {
        let at = self.env.min(self.cho).wrapping_sub(9 + kept);

        if at > self.env.min(self.cho) || at < self.top {
            return Err(Slip::Fault(HEAP_FULL));
        }

        self.heap[at] = self.env as u16;
        self.heap[at + 1] = self.sim as u16;
        self.heap[at + 2] = (self.cont >> 16) as u16;
        self.heap[at + 3] = (self.cont & 0xFFFF) as u16;
        self.heap[at + 4] = (handler >> 16) as u16;
        self.heap[at + 5] = (handler & 0xFFFF) as u16;
        self.heap[at + 6] = self.cho as u16;
        self.heap[at + 7] = self.top as u16;
        self.heap[at + 8] = self.trl as u16;

        for seat in 0..kept {
            self.heap[at + 9 + seat] = self.regs[seat];
        }

        self.cho = at;

        Ok(())
    }

    /// Restore from the newest choice frame (Aa-machine:
    /// POP_CHOICE).
    ///
    /// The trail unwinds on the way, unbinding what was bound past
    /// the frame's mark.
    fn popped_choice(&mut self, kept: usize) {
        for seat in 0..kept {
            self.regs[seat] = self.heap[self.cho + 9 + seat];
        }

        while self.trl < usize::from(self.heap[self.cho + 8]) {
            self.heap[usize::from(self.aux[self.trl])] = 0;
            self.trl += 1;
        }

        self.top = usize::from(self.heap[self.cho + 7]);
        self.cont =
            (usize::from(self.heap[self.cho + 2]) << 16) | usize::from(self.heap[self.cho + 3]);
        self.sim = usize::from(self.heap[self.cho + 1]);
        self.env = usize::from(self.heap[self.cho]);
    }

    /// Land a failure at the newest choice frame's handler.
    ///
    /// Fails when no choice frame stands at all.
    fn fallen(&mut self) -> Result<(), VoxamError> {
        if self.cho + 6 > self.heap.len() {
            return Err(machine_error(
                "the story failed with no choice frame standing (Aa-machine: FAIL)".into(),
            ));
        }

        self.inst =
            (usize::from(self.heap[self.cho + 4]) << 16) | usize::from(self.heap[self.cho + 5]);

        Ok(())
    }

    /// Restart at the entry point with the error told in R00.
    ///
    /// The line is broken if one stands open, the div stack and
    /// status state are cleared, and only the registers restart --
    /// the random access area keeps its state (Aa-machine: Runtime
    /// data).
    fn faulted(&mut self, code: u16) {
        if self.spc < LINE {
            self.voice.line();
        }

        self.cleared_divs();
        self.reset(code, false);
    }

    // -- the random access area ------------------------------------------

    /// The RAM address of an object's field (Aa-machine: Runtime
    /// data). Faults for a non-object past the object count.
    fn field_at(&self, field: u16, obj: u16) -> Fall<usize> {
        let obj = self.deref(obj);

        if obj > self.nob {
            return Err(Slip::Fault(EXPECTED_OBJECT));
        }

        Ok(usize::from(self.ram[usize::from(obj)]) + usize::from(field))
    }

    /// An object's field read; non-objects politely read zero.
    fn field(&self, field: u16, obj: u16) -> u16 {
        let obj = self.deref(obj);

        if obj > self.nob {
            return 0;
        }

        self.ram[usize::from(self.ram[usize::from(obj)]) + usize::from(field)]
    }

    /// Remove a key object from a RAM-linked chain (Aa-machine:
    /// UNLINK).
    fn unlinked(&mut self, root: usize, field: u16, key: u16) -> Fall<()> {
        let key = self.deref(key);

        if !objected(key) {
            return Ok(());
        }

        let tail = self.ram[self.field_at(field, key)?];
        let mut at = root;

        while self.ram[at] != 0 {
            if self.ram[at] == key {
                self.ram[at] = tail;

                return Ok(());
            }

            at = self.field_at(field, self.ram[at])?;
        }

        Ok(())
    }

    // -- long-term storage -----------------------------------------------

    /// A stored value fetched, long-term data revived (Aa-machine:
    /// LOAD_VAL).
    fn lifted(&mut self, mut value: u16) -> Fall<u16> {
        if value & 0x8000 != 0 {
            self.tmp = usize::from(value & 0x7FFF);
            self.tmp += usize::from(self.ram[self.tmp]);
            value = self.popped_longterm()?;
        }

        Ok(value)
    }

    /// One value deserialized backward out of long-term storage.
    fn popped_longterm(&mut self) -> Fall<u16> {
        self.tmp -= 1;

        let mut value = self.ram[self.tmp];

        if value == UNBOUND_MARK {
            return self.variable();
        }

        if value == EXTDICT_MARK {
            let at = self.claimed(2)?;

            self.heap[at] = self.popped_longterm()?;
            self.heap[at + 1] = self.popped_longterm()?;

            return Ok(0xE000 | at as u16);
        }

        if (value & LIST_MARK) == LIST_MARK {
            let count = value & 0x1FFF;

            value = if value & 0x2000 != 0 {
                self.popped_longterm()?
            } else {
                EMPTY
            };

            for _ in 0..count {
                let head = self.popped_longterm()?;

                value = self.pair(head, value)?;
            }
        }

        Ok(value)
    }

    /// Store at a RAM address, live data serialized (Aa-machine:
    /// STORE_VAL).
    fn kept_longterm(&mut self, address: usize, value: u16) -> Fall<()> {
        self.cleared_longterm(address);

        let value = self.deref(value);

        if matches!(value & 0xE000, 0xC000 | 0xE000) || referenced(value) {
            self.tmp = self.ltt + 2;

            if self.tmp > self.ram.len() {
                return Err(Slip::Fault(LONGTERM_FULL));
            }

            self.pushed_longterm(value)?;
            self.ram[address] = 0x8000 + self.ltt as u16;
            self.ram[self.ltt] = (self.tmp - self.ltt) as u16;
            self.ram[self.ltt + 1] = address as u16;
            self.ltt = self.tmp;
        } else {
            self.ram[address] = value;
        }

        Ok(())
    }

    /// One value serialized into long-term storage.
    ///
    /// Faults for an unbound value, or storage exhausted.
    fn pushed_longterm(&mut self, value: u16) -> Fall<()> {
        let mut value = self.deref(value);

        if paired(value) {
            let mut count: u16 = 0;

            loop {
                self.pushed_longterm(self.heap[usize::from(value & 0x1FFF)])?;
                count += 1;
                value = self.deref(self.heap[usize::from(value & 0x1FFF) + 1]);

                if value == EMPTY {
                    value = 0xC000 | count;

                    break;
                }

                if !paired(value) {
                    self.pushed_longterm(value)?;
                    value = 0xE000 | count;

                    break;
                }
            }
        } else if extdicted(value) {
            self.pushed_longterm(self.heap[usize::from(value & 0x1FFF) + 1])?;
            self.pushed_longterm(self.heap[usize::from(value & 0x1FFF)])?;
            value = 0x8100;
        } else if referenced(value) {
            return Err(Slip::Fault(EXPECTED_BOUND));
        }

        if self.tmp >= self.ram.len() {
            return Err(Slip::Fault(LONGTERM_FULL));
        }

        self.ram[self.tmp] = value;
        self.tmp += 1;

        Ok(())
    }

    /// Free a long-term chunk (Aa-machine: STORE_VAL).
    ///
    /// The surviving chunks slide down and their owners are
    /// repointed through their back-references.
    fn cleared_longterm(&mut self, address: usize) {
        let value = self.ram[address];

        if value & 0x8000 == 0 {
            return;
        }

        self.ram[address] = 0;

        let mut value = usize::from(value & 0x7FFF);
        let size = usize::from(self.ram[value]);

        for at in value..self.ltt.saturating_sub(size) {
            self.ram[at] = self.ram[at + size];
        }

        self.ltt -= size;

        while value < self.ltt {
            let owner = usize::from(self.ram[value + 1]);

            self.ram[owner] = self.ram[owner].wrapping_sub(size as u16);
            value += usize::from(self.ram[value]);
        }
    }

    // -- the aux stack ---------------------------------------------------

    /// One raw word onto the aux stack.
    fn pushed_aux(&mut self, value: u16) -> Fall<()> {
        if self.auxp >= self.trl {
            return Err(Slip::Fault(AUX_FULL));
        }

        self.aux[self.auxp] = value;
        self.auxp += 1;

        Ok(())
    }

    /// One raw word off the aux stack, underflow loud.
    fn popped_aux(&mut self) -> Fall<u16> {
        if self.auxp == 0 {
            return Err(Slip::Refused(machine_error(
                "the aux stack popped past its own bottom (Aa-machine: Runtime data)".into(),
            )));
        }

        self.auxp -= 1;

        Ok(self.aux[self.auxp])
    }

    /// One value serialized onto the aux stack (Aa-machine:
    /// AUX_PUSH_VAL).
    fn serialized(&mut self, value: u16) -> Fall<()> {
        let mut value = self.deref(value);

        if paired(value) {
            let mut count: u16 = 0;

            loop {
                self.serialized(self.heap[usize::from(value & 0x1FFF)])?;
                count += 1;
                value = self.deref(self.heap[usize::from(value & 0x1FFF) + 1]);

                if value == EMPTY {
                    value = 0xC000 + count;

                    break;
                }

                if !paired(value) {
                    self.serialized(value)?;
                    value = 0xE000 + count;

                    break;
                }
            }
        } else if extdicted(value) {
            self.serialized(self.heap[usize::from(value & 0x1FFF) + 1])?;
            self.serialized(self.heap[usize::from(value & 0x1FFF)])?;
            value = 0x8100;
        } else if referenced(value) {
            value = 0x8000;
        }

        self.pushed_aux(value)
    }

    /// One value deserialized off the aux stack (Aa-machine:
    /// AUX_POP_VAL).
    fn deserialized(&mut self) -> Fall<u16> {
        let mut value = self.popped_aux()?;

        if value == UNBOUND_MARK {
            return self.variable();
        }

        if value == EXTDICT_MARK {
            let at = self.claimed(2)?;

            self.heap[at] = self.deserialized()?;
            self.heap[at + 1] = self.deserialized()?;

            return Ok(0xE000 | at as u16);
        }

        if (value & LIST_MARK) == LIST_MARK {
            let count = value & 0x1FFF;

            value = if value & 0x2000 != 0 {
                self.deserialized()?
            } else {
                EMPTY
            };

            for _ in 0..count {
                let head = self.deserialized()?;

                value = self.pair(head, value)?;
            }
        }

        Ok(value)
    }

    /// Values deserialized to the end marker (Aa-machine:
    /// AUX_POP_LIST).
    fn deserialized_list(&mut self) -> Fall<u16> {
        let mut told = EMPTY;

        loop {
            let value = self.deserialized()?;

            if value == 0 {
                break;
            }

            told = self.pair(value, told)?;
        }

        Ok(told)
    }

    // -- the dice --------------------------------------------------------

    /// The next roll of the reference dice: fifteen fair bits.
    fn rolled(&mut self) -> u16 {
        self.dice = self.dice.wrapping_mul(DICE_STEP).wrapping_add(1);

        ((self.dice >> 16) & 0x7FFF) as u16
    }

    // -- numbers ---------------------------------------------------------

    /// A tagged number's value, anything else failing (Aa-machine:
    /// ADD_NUM).
    fn unboxed(&self, value: u16) -> Fall<u16> {
        let value = self.deref(value);

        if !numbered(value) {
            return Err(Slip::Missed);
        }

        Ok(value & NUMBER_TOP)
    }

    /// A number boxed into its tag, the range enforced by failure.
    fn boxed(&self, value: i64) -> Fall<u16> {
        if !(0..=i64::from(NUMBER_TOP)).contains(&value) {
            return Err(Slip::Missed);
        }

        Ok(0x4000 | value as u16)
    }

    // -- lifecycle -------------------------------------------------------

    /// Fill the memory areas from INIT, the rest left unused
    /// (Aa-machine: INIT).
    fn reinit(&mut self) {
        self.nob = be(&self.init, 0, 2) as u16;
        self.ltb = be(&self.init, 2, 4);
        self.ltt = be(&self.init, 4, 6);
        self.heap.fill(UNUSED);
        self.aux.fill(UNUSED);

        let held = (self.init.len().saturating_sub(6)) / 2;

        for at in 0..self.ram.len() {
            self.ram[at] = if at < held {
                be(&self.init, 6 + at * 2, 8 + at * 2) as u16
            } else {
                UNUSED
            };
        }
    }

    /// Reinitialize the registers, R00 excepted (Aa-machine:
    /// Runtime data).
    fn reset(&mut self, first: u16, clear_undo: bool) {
        self.regs = vec![0; 64];
        self.regs[0] = first;
        self.inst = 1;
        self.cont = 0;
        self.top = 0;
        self.env = self.heap.len();
        self.cho = self.heap.len();
        self.sim = 0xFFFF;
        self.auxp = 0;
        self.trl = self.aux.len();
        self.sta = 0;
        self.stc = 0;
        self.cwl = 0;
        self.spc = LINE;
        self.tmp = 0;
        self.divs = Vec::new();
        self.upper = false;
        self.trace = false;
        self.in_status = 0;
        self.n_span = 0;
        self.n_link = 0;
        self.dice = self.seed.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |told| told.as_nanos() as u32)
        });

        if clear_undo {
            self.undo = Vec::new();
            self.pruned = false;
        }
    }

    /// The whole game state, unallocated regions masked unused.
    ///
    /// The landing is the instruction address a restore will
    /// resume at (Aa-machine: Savefile).
    fn captured(&self, landing: u32) -> State {
        State {
            counted: (self.nob, self.ltb as u16, self.ltt as u16),
            ram: self
                .ram
                .iter()
                .enumerate()
                .map(|(at, &value)| if at < self.ltt { value } else { UNUSED })
                .collect(),
            aux: self
                .aux
                .iter()
                .enumerate()
                .map(|(at, &value)| {
                    if at < self.auxp || at >= self.trl {
                        value
                    } else {
                        UNUSED
                    }
                })
                .collect(),
            heap: self
                .heap
                .iter()
                .enumerate()
                .map(|(at, &value)| {
                    if at < self.top || at >= self.env || at >= self.cho {
                        value
                    } else {
                        UNUSED
                    }
                })
                .collect(),
            regs: self.regs.clone(),
            flow: (
                landing,
                self.cont as u32,
                self.top as u16,
                self.env as u16,
                self.cho as u16,
                self.sim as u16,
            ),
            stacks: (
                self.auxp as u16,
                self.trl as u16,
                self.sta as u16,
                self.stc as u16,
                self.cwl as u8,
                self.spc,
            ),
            divs: self.divs.clone(),
        }
    }

    /// The whole game state put back from a capture.
    fn restored(&mut self, state: &State) {
        self.nob = state.counted.0;
        self.ltb = usize::from(state.counted.1);
        self.ltt = usize::from(state.counted.2);
        self.ram.copy_from_slice(&state.ram);
        self.aux.copy_from_slice(&state.aux);
        self.heap.copy_from_slice(&state.heap);
        self.regs = state.regs.clone();
        self.inst = state.flow.0 as usize;
        self.cont = state.flow.1 as usize;
        self.top = usize::from(state.flow.2);
        self.env = usize::from(state.flow.3);
        self.cho = usize::from(state.flow.4);
        self.sim = usize::from(state.flow.5);
        self.auxp = usize::from(state.stacks.0);
        self.trl = usize::from(state.stacks.1);
        self.sta = usize::from(state.stacks.2);
        self.stc = usize::from(state.stacks.3);
        self.cwl = i64::from(state.stacks.4);
        self.spc = state.stacks.5;
        self.divs = state.divs.clone();
    }

    /// Return the output to its initial state, the counters too.
    fn cleared_divs(&mut self) {
        self.voice.leave_all();
        self.in_status = 0;
        self.n_span = 0;
        self.n_link = 0;
        self.divs = Vec::new();
    }

    // -- speaking --------------------------------------------------------

    /// Say text through the voice, an armed UPPERCASE applied.
    fn said(&mut self, text: &str) {
        if self.upper && !text.is_empty() {
            let mut chars = text.chars();
            let first = chars.next().expect("checked nonempty");
            let raised = self
                .upcased
                .get(&first)
                .cloned()
                .unwrap_or_else(|| first.to_uppercase().collect());

            self.upper = false;

            let told = format!("{raised}{}", chars.as_str());

            self.voice.say(&told);

            return;
        }

        self.voice.say(text);
    }

    /// The usual gap before printing: auto and pending say space.
    fn spaced_auto(&mut self) {
        if self.spc == AUTO || self.spc == PENDING {
            self.voice.space();
        }
    }

    /// One charset code as text, the extended table ruling.
    fn character(&self, code: u16) -> char {
        if code < EXTENDED_START {
            return char::from(code as u8);
        }

        self.story.extended[usize::from(code - EXTENDED_START)]
    }

    /// One input character as its lowercase charset code.
    ///
    /// Unspellable characters become question marks, the reference
    /// engine's own shrug.
    fn encased(&self, piece: char) -> u8 {
        let code = u32::from(piece);

        if (UPPER_A..=UPPER_Z).contains(&code) {
            return (code ^ 0x20) as u8;
        }

        if code < u32::from(EXTENDED_START) {
            return code as u8;
        }

        self.cased.get(&piece).copied().unwrap_or(0x3F)
    }

    /// A value spelled the way PRINT_VAL spells it (Aa-machine:
    /// PRINT_VAL).
    fn valued_text(&self, value: u16) -> String {
        if chared(value) {
            return self.character(value & 0xFF).to_string();
        }

        if extdicted(value) || dicted(value) {
            return self.worded_text(value);
        }

        if paired(value) {
            return self.listed_text(value);
        }

        if referenced(value) {
            return "$".to_string();
        }

        if numbered(value) {
            return (value & NUMBER_TOP).to_string();
        }

        if value == EMPTY {
            return "[]".to_string();
        }

        self.object_text(value)
    }

    /// A dict word or extdict spelled out (Aa-machine: PRINT_VAL).
    fn worded_text(&self, value: u16) -> String {
        if dicted(value) {
            return self.speech.words[usize::from(value & 0x1FFF)].clone();
        }

        let first = self.heap[usize::from(value & 0x1FFF)];

        if matches!(first & 0xE000, 0x8000 | 0xC000) {
            return self.tail_text(first);
        }

        format!(
            "{}{}",
            self.worded_text(first),
            self.tail_text(self.heap[usize::from(value & 0x1FFF) + 1])
        )
    }

    /// A character list spelled out plainly.
    fn tail_text(&self, mut listed: u16) -> String {
        let mut pieces = String::new();

        while paired(listed) {
            pieces.push_str(&self.valued_text(self.heap[usize::from(listed & 0x1FFF)]));
            listed = self.heap[usize::from(listed & 0x1FFF) + 1];
        }

        pieces
    }

    /// A list spelled in brackets, an improper tail barred.
    fn listed_text(&self, mut value: u16) -> String {
        let mut pieces: Vec<String> = Vec::new();

        while paired(value) {
            pieces.push(self.valued_text(self.deref(self.heap[usize::from(value & 0x1FFF)])));
            value = self.deref(self.heap[usize::from(value & 0x1FFF) + 1]);
        }

        let mut told = pieces.join(" ");

        if value != EMPTY {
            told = format!("{told} | {}", self.valued_text(value));
        }

        format!("[{told}]")
    }

    /// An object spelled as its hashmark and TAGS name.
    fn object_text(&self, value: u16) -> String {
        let mut told = "#".to_string();

        if let Some(payload) = &self.tags {
            let at = be(payload, usize::from(value) * 2, usize::from(value) * 2 + 2);
            // The reference's find answers -1 when no null ends the
            // name, and the slice to -1 drops the final byte; kept.
            let ended = payload[at..]
                .iter()
                .position(|&byte| byte == 0)
                .map_or(payload.len().saturating_sub(1), |seat| at + seat);

            for &code in &payload[at..ended] {
                told.push(self.character(u16::from(code)));
            }
        }

        told
    }

    // -- input parsing ---------------------------------------------------

    /// One input word as a machine value (Aa-machine: GET_INPUT).
    fn parsed(&mut self, codes: &[u8]) -> Fall<u16> {
        if codes.len() > 1
            && let Some(&seat) = self.sought.get(codes)
        {
            return Ok(0x2000 | seat);
        }

        let mut number: u32 = 0;
        let mut whole = true;

        for &code in codes {
            if !digited(code) {
                whole = false;

                break;
            }

            number = number * 10 + u32::from(code) - 0x30;

            if number > u32::from(NUMBER_TOP) {
                whole = false;

                break;
            }
        }

        if whole {
            return Ok(0x4000 | number as u16);
        }

        if codes.len() == 1 {
            return Ok(0x3E00 | u16::from(codes[0]));
        }

        self.suffixed(codes)
    }

    /// A word run through the endings decoder (Aa-machine: LANG).
    fn suffixed(&mut self, codes: &[u8]) -> Fall<u16> {
        let mut state = self.endings_at;
        let mut ending: Vec<u8> = Vec::new();
        let mut held = codes.len();

        loop {
            let told = self.lang[state];

            state += 1;

            if told == 0 {
                ending.extend(codes[..held].iter().rev());

                let listed = self.charlist(&ending)?;

                return Ok(self.pair(listed, EMPTY)? | 0xE000);
            }

            if told == 1 {
                if let Some(&seat) = self.sought.get(&codes[..held]) {
                    let listed = self.charlist(&ending)?;

                    return Ok(self.pair(0x2000 | seat, listed)? | 0xE000);
                }
            } else {
                let landing = self.lang[state];

                state += 1;

                if held > STEM_KEPT && told == codes[held - 1] {
                    ending.push(told);
                    held -= 1;
                    state = self.endings_at + usize::from(landing);
                }
            }
        }
    }

    /// Reversed charset codes as a list, digits told as numbers.
    fn charlist(&mut self, reversed_codes: &[u8]) -> Fall<u16> {
        let mut told = EMPTY;

        for &code in reversed_codes {
            told = if digited(code) {
                self.pair(0x4000 + u16::from(code) - 0x30, told)?
            } else {
                self.pair(0x3E00 | u16::from(code), told)?
            };
        }

        Ok(told)
    }

    /// A word list as charset codes; None asks the caller to fail
    /// (Aa-machine: JOIN_WORDS).
    fn joined_codes(&self, mut listed: u16) -> Option<Vec<u8>> {
        let mut codes: Vec<u8> = Vec::new();

        loop {
            let value = self.deref(self.heap[usize::from(listed & 0x1FFF)]);

            if extdicted(value) {
                let first = self.heap[usize::from(value & 0x1FFF)];
                let inner = self.joined_codes(if first >= UNBOUND_MARK { first } else { value })?;

                codes.extend(inner);
            } else if numbered(value) {
                codes.extend((value & NUMBER_TOP).to_string().bytes());
            } else if chared(value) {
                let code = (value & 0xFF) as u8;

                if code <= SPACE_CODE || self.stops.contains(&code) {
                    return None;
                }

                codes.push(code);
            } else if dicted(value) {
                codes.extend(self.worded_codes(usize::from(value & 0x1FFF)));
            } else {
                return None;
            }

            listed = self.deref(self.heap[usize::from(listed & 0x1FFF) + 1]);

            if !paired(listed) {
                break;
            }
        }

        if listed != EMPTY {
            return None;
        }

        Some(codes)
    }

    /// One dictionary word's raw charset bytes.
    fn worded_codes(&self, seat: usize) -> Vec<u8> {
        let length = usize::from(self.dict[2 + seat * 3]);
        let at = be(&self.dict, 3 + seat * 3, 5 + seat * 3);

        self.dict[at..at + length].to_vec()
    }

    /// A dictionary word's characters prepended to a list.
    fn prepended(&mut self, seat: usize, tail: u16) -> Fall<u16> {
        let mut told = tail;

        for &code in self.worded_codes(seat).iter().rev() {
            told = if digited(code) {
                self.pair(0x4000 + u16::from(code) - 0x30, told)?
            } else {
                self.pair(0x3E00 | u16::from(code), told)?
            };
        }

        Ok(told)
    }

    // -- the wordmaps ----------------------------------------------------

    /// Search a wordmap for IDX; true asks for the jump
    /// (Aa-machine: MAPS).
    fn mapped(&mut self, seat: u16) -> Fall<bool> {
        let table = be(
            &self.maps,
            2 + usize::from(seat) * 2,
            4 + usize::from(seat) * 2,
        );
        let mut low = 0;
        let mut high = be(&self.maps, table, table + 2);
        let wanted = self.regs[0x3F];

        while low < high {
            let mid = (low + high) / 2;
            let at = table + 2 + mid * 4;
            let told = be(&self.maps, at, at + 2) as u16;

            if told == wanted {
                let entry = be(&self.maps, at + 2, at + 4);

                return self.map_told(entry);
            }

            if told > wanted {
                high = mid;
            } else {
                low = mid + 1;
            }
        }

        Ok(true)
    }

    /// Act on one wordmap entry: wildcard, one object, or many.
    fn map_told(&mut self, entry: usize) -> Fall<bool> {
        if entry == 0 {
            return Ok(false);
        }

        if entry & 0xE000 != 0 {
            self.pushed_aux(entry as u16 & 0x1FFF)?;

            return Ok(true);
        }

        let mut entry = entry;

        loop {
            let code = u16::from(self.maps[entry]);

            if code == 0 {
                break;
            }

            entry += 1;

            let code = if code >= WIDE_SEAT {
                let wide = ((code & 0x1F) << 8) | u16::from(self.maps[entry]);

                entry += 1;

                wide
            } else {
                code
            };

            self.pushed_aux(code)?;
        }

        Ok(true)
    }

    // -- the dispatch ----------------------------------------------------

    /// One instruction dispatched by its full opcode byte, the
    /// reference's _OPS table as a match (Aa-machine: Story file).
    #[allow(clippy::too_many_lines)]
    fn op(&mut self, op: u8) -> Fall<Option<Wait>> {
        match op {
            0x00 => Ok(None),
            0x01 => Err(Slip::Missed),
            0x02 => {
                self.cont = self.target();

                Ok(None)
            }
            0x03 => {
                self.op_proceed();

                Ok(None)
            }
            0x04 => {
                self.inst = self.target();

                Ok(None)
            }
            0x05 | 0x85 => {
                self.op_jmp_multi(op);

                Ok(None)
            }
            0x06 | 0x86 => {
                self.op_jmp_simple(op);

                Ok(None)
            }
            0x07 => {
                self.op_jmp_tail();

                Ok(None)
            }
            0x87 => {
                self.op_tail();

                Ok(None)
            }
            0x08 | 0x88 => self.op_push_env(op).map(|()| None),
            0x09 => {
                self.op_pop_env();

                Ok(None)
            }
            0x89 => {
                self.op_pop_env_proceed();

                Ok(None)
            }
            0x0A | 0x8A => self.op_push_choice(op).map(|()| None),
            0x0B | 0x8B => {
                self.op_pop_choice(op);

                Ok(None)
            }
            0x0C | 0x8C => {
                self.op_pop_push_choice(op);

                Ok(None)
            }
            0x0D => {
                self.cho = usize::from(self.heap[self.cho + 6]);

                Ok(None)
            }
            0x0E => {
                let dest = self.fetched();

                self.store(dest, self.cho as u16)?;

                Ok(None)
            }
            0x0F => {
                self.cho = usize::from(self.value());

                Ok(None)
            }
            0x10 | 0x90 => self.op_assign(op).map(|()| None),
            0x11 => {
                let dest = self.fetched();
                let told = self.variable()?;

                self.store(dest, told)?;

                Ok(None)
            }
            0x12 | 0x13 | 0x93 => self.op_make_pair(op).map(|()| None),
            0x14 => {
                let value = self.value();

                self.serialized(value).map(|()| None)
            }
            0x94 | 0x15 | 0x95 => self.op_aux_push_raw(op).map(|()| None),
            0x16 => {
                let dest = self.fetched();
                let told = self.deserialized()?;

                self.store(dest, told)?;

                Ok(None)
            }
            0x17 => {
                let dest = self.fetched();
                let told = self.deserialized_list()?;

                self.store(dest, told)?;

                Ok(None)
            }
            0x18 => self.op_aux_pop_list_chk().map(|()| None),
            0x19 => self.op_aux_pop_list_match().map(|()| None),
            0x1B => self.op_split_list().map(|()| None),
            0x1C => {
                self.cho = self.stc;

                Err(Slip::Missed)
            }
            0x1D => self.op_push_stop().map(|()| None),
            0x1E => self.op_pop_stop().map(|()| None),
            0x1F => self.op_split_word().map(|()| None),
            0x9F => self.op_join_words().map(|()| None),
            0x20 | 0xA0 => self.op_load_word(op).map(|()| None),
            0x21 | 0xA1 => self.op_load_byte(op).map(|()| None),
            0x22 | 0xA2 => self.op_load_val(op).map(|()| None),
            0x24 | 0xA4 => self.op_store_word(op).map(|()| None),
            0x25 | 0xA5 => self.op_store_byte(op).map(|()| None),
            0x26 | 0xA6 => self.op_store_val(op).map(|()| None),
            0x28 | 0xA8 => self.op_set_flag(op).map(|()| None),
            0x29 | 0xA9 => self.op_reset_flag(op).map(|()| None),
            0x2D | 0xAD => self.op_unlink(op).map(|()| None),
            0x2E | 0xAE | 0x2F | 0xAF => self.op_set_parent(op).map(|()| None),
            0x30 | 0xB0 | 0x40 | 0xC0 => self.op_if_raw_eq(op).map(|()| None),
            0x31 | 0x41 => {
                let told = self.value();
                let bound = !referenced(self.deref(told));

                self.jumped(op, bound);

                Ok(None)
            }
            0x32 | 0x42 => {
                let told = self.value();
                let empty = self.deref(told) == EMPTY;

                self.jumped(op, empty);

                Ok(None)
            }
            0x33 | 0x43 => {
                let told = self.value();
                let told = numbered(self.deref(told));

                self.jumped(op, told);

                Ok(None)
            }
            0x34 | 0x44 => {
                let told = self.value();
                let told = paired(self.deref(told));

                self.jumped(op, told);

                Ok(None)
            }
            0x35 | 0x45 => {
                let told = self.value();
                let told = objected(self.deref(told));

                self.jumped(op, told);

                Ok(None)
            }
            0x36 | 0x46 => {
                let told = self.value();
                let told = wordish(self.deref(told));

                self.jumped(op, told);

                Ok(None)
            }
            0xB6 | 0xC6 => {
                let told = self.value();
                let told = self.deref(told);
                let unknown = extdicted(told) && paired(self.heap[usize::from(told & 0x1FFF)]);

                self.jumped(op, unknown);

                Ok(None)
            }
            0x37 | 0x47 => {
                let first = self.value();
                let second = self.value();
                let told = self.agreeable(first, second);

                self.jumped(op, told);

                Ok(None)
            }
            0x38 | 0x48 => {
                let first = self.value();
                let first = self.deref(first);
                let second = self.value();
                let second = self.deref(second);
                let told = numbered(first) && numbered(second) && first > second;

                self.jumped(op, told);

                Ok(None)
            }
            0x39 | 0xB9 | 0x49 | 0xC9 => {
                let first = if op & 0x80 != 0 {
                    u16::from(self.fetched())
                } else {
                    self.word()
                };
                let second = self.value();
                let told = first == self.deref(second);

                self.jumped(op, told);

                Ok(None)
            }
            0x3A | 0xBA | 0x4A | 0xCA => {
                let obj = if op & 0x80 != 0 { 0 } else { self.value() };
                let field = self.index();
                let second = self.value();
                let told = self.field(field, obj) == second;

                self.jumped(op, told);

                Ok(None)
            }
            0x3B | 0xBB | 0x4B | 0xCB => {
                let obj = if op & 0x80 != 0 { 0 } else { self.value() };
                let flag = self.index();
                let told = self.field(flag / BITS_PER_WORD, obj);
                let mask = 0x8000 >> (flag % BITS_PER_WORD);

                self.jumped(op, told & mask != 0);

                Ok(None)
            }
            0x3C | 0x4C => {
                let told = self.cwl != 0;

                self.jumped(op, told);

                Ok(None)
            }
            0x3D | 0xBD | 0x4D | 0xCD => {
                let obj = if op & 0x80 != 0 { 0 } else { self.value() };
                let field = self.index();
                let second = u16::from(self.fetched());
                let told = self.field(field, obj) == second;

                self.jumped(op, told);

                Ok(None)
            }
            0x50 => {
                let first = self.value();
                let second = self.value();
                let dest = self.fetched();

                self.store(dest, first.wrapping_add(second))?;

                Ok(None)
            }
            0xD0 => {
                let told = self.value();
                let dest = self.fetched();

                self.store(dest, told.wrapping_add(1))?;

                Ok(None)
            }
            0x51 => {
                let first = self.value();
                let second = self.value();
                let dest = self.fetched();

                self.store(dest, first.wrapping_sub(second))?;

                Ok(None)
            }
            0xD1 => {
                let told = self.value();
                let dest = self.fetched();

                self.store(dest, told.wrapping_sub(1))?;

                Ok(None)
            }
            0x52 => {
                let ceiling = self.fetched();
                let dest = self.fetched();
                let told = self.rolled() % (u16::from(ceiling) + 1);

                self.store(dest, told)?;

                Ok(None)
            }
            0x58 => {
                let first = self.value();
                let first = self.unboxed(first)?;
                let second = self.value();
                let second = self.unboxed(second)?;
                let dest = self.fetched();
                let told = self.boxed(i64::from(first) + i64::from(second))?;

                self.store(dest, told)?;

                Ok(None)
            }
            0xD8 => {
                let told = self.value();
                let told = self.unboxed(told)?;
                let dest = self.fetched();
                let told = self.boxed(i64::from(told) + 1)?;

                self.store(dest, told)?;

                Ok(None)
            }
            0x59 => {
                let first = self.value();
                let first = self.unboxed(first)?;
                let second = self.value();
                let second = self.unboxed(second)?;
                let dest = self.fetched();
                let told = self.boxed(i64::from(first) - i64::from(second))?;

                self.store(dest, told)?;

                Ok(None)
            }
            0xD9 => {
                let told = self.value();
                let told = self.unboxed(told)?;
                let dest = self.fetched();
                let told = self.boxed(i64::from(told) - 1)?;

                self.store(dest, told)?;

                Ok(None)
            }
            0x5A => {
                let start = self.value();
                let start = self.unboxed(start)?;
                let bound = self.value();
                let span = i64::from(self.unboxed(bound)?) - i64::from(start) + 1;

                if span < 1 {
                    return Err(Slip::Missed);
                }

                let dest = self.fetched();
                let roll = i64::from(self.rolled());
                let told = self.boxed(i64::from(start) + roll % span)?;

                self.store(dest, told)?;

                Ok(None)
            }
            0x5B => {
                let first = self.value();
                let first = self.unboxed(first)?;
                let second = self.value();
                let second = self.unboxed(second)?;
                let dest = self.fetched();
                let told = self.boxed(i64::from((first.wrapping_mul(second)) & NUMBER_TOP))?;

                self.store(dest, told)?;

                Ok(None)
            }
            0x5C => {
                let first = self.value();
                let first = self.unboxed(first)?;
                let second = self.value();
                let second = self.unboxed(second)?;

                if second == 0 {
                    return Err(Slip::Missed);
                }

                let dest = self.fetched();
                let told = self.boxed(i64::from(first / second))?;

                self.store(dest, told)?;

                Ok(None)
            }
            0x5D => {
                let first = self.value();
                let first = self.unboxed(first)?;
                let second = self.value();
                let second = self.unboxed(second)?;

                if second == 0 {
                    return Err(Slip::Missed);
                }

                let dest = self.fetched();
                let told = self.boxed(i64::from(first % second))?;

                self.store(dest, told)?;

                Ok(None)
            }
            0x60 | 0xE0 | 0x61 | 0xE1 => self.op_print_str(op).map(|()| None),
            0x62 => {
                if self.cwl == 0 {
                    self.spc = self.spc.max(NOSPACE);
                }

                Ok(None)
            }
            0xE2 => {
                if self.cwl == 0 {
                    self.spc = self.spc.max(PENDING);
                }

                Ok(None)
            }
            0x63 => {
                if self.cwl == 0 && self.spc < LINE {
                    self.voice.line();
                    self.spc = LINE;
                }

                Ok(None)
            }
            0xE3 => {
                if self.cwl == 0 && self.spc < PAR {
                    self.voice.par();
                    self.spc = PAR;
                }

                Ok(None)
            }
            0x64 => {
                let value = self.value();
                let value = self.deref(value);

                if self.cwl == 0 && numbered(value) {
                    self.voice.spaces(i64::from(value & NUMBER_TOP));
                    self.spc = SPACE;
                }

                Ok(None)
            }
            0x65 => self.op_print_val().map(|()| None),
            0x66 => self.op_enter_div().map(|()| None),
            0xE6 => {
                if self.cwl == 0 {
                    let style = self.divs.pop().expect("an open div to leave");

                    self.voice.leave_div(i64::from(style));
                    self.spc = LINE;
                }

                Ok(None)
            }
            0x67 => self.op_status_or_body().map(|()| None),
            0xE7 | 0xEF => self.op_leave_status(op).map(|()| None),
            0x68 => {
                let resource = self.value();
                let resource = self.deref(resource);

                if self.cwl == 0 {
                    if self.n_link == 0 {
                        self.spaced_auto();
                        self.voice.enter_link_res(i64::from(resource));
                        self.spc = NOSPACE;
                    }

                    self.n_link += 1;
                    self.n_span += 1;
                }

                Ok(None)
            }
            0xE8 => {
                if self.cwl == 0 {
                    self.n_link -= 1;
                    self.n_span -= 1;

                    if self.n_link == 0 {
                        self.voice.leave_link_res();
                    }
                }

                Ok(None)
            }
            0x69 => self.op_enter_link().map(|()| None),
            0xE9 => {
                if self.cwl == 0 {
                    self.n_link -= 1;
                    self.n_span -= 1;

                    if self.n_link == 0 {
                        self.voice.leave_link();
                    }
                }

                Ok(None)
            }
            0x6A => {
                if self.cwl == 0 {
                    if self.n_link == 0 {
                        self.spaced_auto();
                        self.voice.enter_self_link();
                        self.spc = SPACE;
                    }

                    self.n_link += 1;
                    self.n_span += 1;
                }

                Ok(None)
            }
            0xEA => {
                if self.cwl == 0 {
                    self.n_link -= 1;
                    self.n_span -= 1;

                    if self.n_link == 0 {
                        self.voice.leave_self_link();
                    }
                }

                Ok(None)
            }
            0x6B | 0xEB => {
                let bits = self.fetched();

                if self.cwl == 0 {
                    if op & 0x80 != 0 {
                        self.voice.reset_style(i64::from(bits));
                    } else {
                        self.spaced_auto();
                        self.voice.set_style(i64::from(bits));
                        self.spc = SPACE;
                    }
                }

                Ok(None)
            }
            0x6C => {
                let resource = self.value();
                let resource = self.deref(resource);

                if self.cwl == 0 {
                    self.voice.embed_res(i64::from(resource));
                }

                Ok(None)
            }
            0xEC => {
                let resource = self.value();
                let resource = self.deref(resource);
                let dest = self.fetched();
                let told = u16::from(self.voice.can_embed_res(i64::from(resource)));

                self.store(dest, told)?;

                Ok(None)
            }
            0x6D => {
                let amount = self.value();
                let amount = self.deref(amount);
                let total = self.value();
                let total = self.deref(total);

                if self.cwl == 0 && numbered(amount) && numbered(total) {
                    self.voice.progress(
                        i64::from(amount & NUMBER_TOP),
                        i64::from(total & NUMBER_TOP),
                    );
                }

                Ok(None)
            }
            0x6E => {
                let style = self.index();

                if self.cwl == 0 {
                    self.spaced_auto();
                    self.voice.enter_span(i64::from(style));
                    self.spc = NOSPACE;
                    self.n_span += 1;
                }

                Ok(None)
            }
            0xEE => {
                if self.cwl == 0 {
                    self.voice.leave_span();
                    self.spc = AUTO;
                    self.n_span -= 1;
                }

                Ok(None)
            }
            0x6F => {
                let area = self.fetched();
                let style = self.index();

                self.entered_status(area, style).map(|()| None)
            }
            0x70 => self.ext0(),
            0x72 => self.op_save().map(|()| None),
            0xF2 => self.op_save_undo().map(|()| None),
            0x73 => {
                self.spaced_input();

                Ok(Some(Wait::Line))
            }
            0xF3 => {
                self.spaced_input();

                Ok(Some(Wait::Key))
            }
            0x74 => self.op_vm_info().map(|()| None),
            0x78 => {
                let value = self.value();
                let mut value = self.deref(value);

                if extdicted(value) {
                    value = self.heap[usize::from(value & 0x1FFF)];
                }

                self.regs[0x3F] = value;

                Ok(None)
            }
            0x79 | 0xF9 => {
                let first = if op & 0x80 != 0 {
                    u16::from(self.fetched())
                } else {
                    self.word()
                };
                let landing = self.target();

                if self.regs[0x3F] == first {
                    self.inst = landing;
                }

                Ok(None)
            }
            0x7A | 0xFA => {
                let first = if op & 0x80 != 0 {
                    u16::from(self.fetched())
                } else {
                    self.word()
                };
                let above = self.target();
                let equal = self.target();

                if self.regs[0x3F] > first {
                    self.inst = above;
                } else if self.regs[0x3F] == first {
                    self.inst = equal;
                }

                Ok(None)
            }
            0x7B | 0xFB => {
                let first = if op & 0x80 != 0 {
                    u16::from(self.fetched())
                } else {
                    self.value()
                };
                let landing = self.target();

                if self.regs[0x3F] > first {
                    self.inst = landing;
                }

                Ok(None)
            }
            0x7C => {
                let seat = self.index();
                let landing = self.target();

                if self.mapped(seat)? {
                    self.inst = landing;
                }

                Ok(None)
            }
            0x7D | 0xFD => {
                let (first, second) = if op & 0x80 != 0 {
                    (u16::from(self.fetched()), u16::from(self.fetched()))
                } else {
                    (self.word(), self.word())
                };
                let landing = self.target();

                if self.regs[0x3F] == first || self.regs[0x3F] == second {
                    self.inst = landing;
                }

                Ok(None)
            }
            0x7F => self.op_tracepoint().map(|()| None),
            _ => Err(Slip::Refused(machine_error(format!(
                "reached opcode {op:#04x} at ${:06x}, which this engine does \
                 not carry (Aa-machine: Story file)",
                self.inst - 1
            )))),
        }
    }

    // -- execution flow opcodes ------------------------------------------

    /// PROCEED: resume at the continuation, a simple cut landing.
    fn op_proceed(&mut self) {
        if self.sim < NO_CUT {
            self.cho = self.sim;
        }

        self.inst = self.cont;
    }

    /// JMP_MULTI and JMPL_MULTI: a multi-call, SIM invalidated.
    fn op_jmp_multi(&mut self, op: u8) {
        let landing = self.target();

        if op & 0x80 != 0 {
            self.cont = self.inst;
        }

        self.sim = 0xFFFF;
        self.inst = landing;
    }

    /// JMP_SIMPLE and JMPL_SIMPLE: a simple call, SIM caught.
    fn op_jmp_simple(&mut self, op: u8) {
        let landing = self.target();

        if op & 0x80 != 0 {
            self.cont = self.inst;
        }

        self.sim = self.cho;
        self.inst = landing;
    }

    /// JMP_TAIL: a tail call, SIM caught if not already
    /// (Aa-machine: JMP_TAIL).
    fn op_jmp_tail(&mut self) {
        if self.sim >= NO_CUT {
            self.sim = self.cho;
        }

        self.inst = self.target();
    }

    /// TAIL: catch SIM without jumping (Aa-machine: TAIL).
    fn op_tail(&mut self) {
        if self.sim >= NO_CUT {
            self.sim = self.cho;
        }
    }

    /// PUSH_ENV: an environment frame with local slots
    /// (Aa-machine: PUSH_ENV).
    fn op_push_env(&mut self, op: u8) -> Fall<()> {
        let slots = if op & 0x80 != 0 {
            0
        } else {
            usize::from(self.fetched())
        };
        let at = self.env.min(self.cho).wrapping_sub(4 + slots);

        if at > self.env.min(self.cho) || at < self.top {
            return Err(Slip::Fault(HEAP_FULL));
        }

        self.heap[at] = self.env as u16;
        self.heap[at + 1] = self.sim as u16;
        self.heap[at + 2] = (self.cont >> 16) as u16;
        self.heap[at + 3] = (self.cont & 0xFFFF) as u16;
        self.env = at;

        Ok(())
    }

    /// POP_ENV: leave the environment frame (Aa-machine: POP_ENV).
    fn op_pop_env(&mut self) {
        self.cont =
            (usize::from(self.heap[self.env + 2]) << 16) | usize::from(self.heap[self.env + 3]);
        self.sim = usize::from(self.heap[self.env + 1]);
        self.env = usize::from(self.heap[self.env]);
    }

    /// POP_ENV_PROCEED: leave the frame straight into its
    /// continuation.
    fn op_pop_env_proceed(&mut self) {
        self.inst =
            (usize::from(self.heap[self.env + 2]) << 16) | usize::from(self.heap[self.env + 3]);

        if usize::from(self.heap[self.env + 1]) < NO_CUT {
            self.cho = usize::from(self.heap[self.env + 1]);
        }

        self.env = usize::from(self.heap[self.env]);
    }

    /// PUSH_CHOICE: a choice frame keeping the first registers.
    fn op_push_choice(&mut self, op: u8) -> Fall<()> {
        let kept = if op & 0x80 != 0 {
            0
        } else {
            usize::from(self.fetched())
        };
        let handler = self.target();

        self.pushed_choice(kept, handler)
    }

    /// POP_CHOICE: restore and discard the newest choice frame.
    fn op_pop_choice(&mut self, op: u8) {
        let kept = if op & 0x80 != 0 {
            0
        } else {
            usize::from(self.fetched())
        };

        self.popped_choice(kept);
        self.cho = usize::from(self.heap[self.cho + 6]);
    }

    /// POP_PUSH_CHOICE: restore the frame and re-aim its handler.
    fn op_pop_push_choice(&mut self, op: u8) {
        let kept = if op & 0x80 != 0 {
            0
        } else {
            usize::from(self.fetched())
        };
        let landing = self.target();

        self.heap[self.cho + 4] = (landing >> 16) as u16;
        self.heap[self.cho + 5] = (landing & 0xFFFF) as u16;
        self.popped_choice(kept);
    }

    // -- live data opcodes -----------------------------------------------

    /// ASSIGN: store or unify a value (Aa-machine: ASSIGN).
    fn op_assign(&mut self, op: u8) -> Fall<()> {
        let value = if op & 0x80 != 0 {
            u16::from(self.fetched())
        } else {
            self.value()
        };
        let dest = self.fetched();

        self.store(dest, value)
    }

    /// MAKE_PAIR: build or take apart a pair (Aa-machine:
    /// MAKE_PAIR).
    fn op_make_pair(&mut self, op: u8) -> Fall<()> {
        let (literal, first) = if op == PAIR_OF_DESTS {
            (None, self.fetched())
        } else if op == PAIR_OF_WORD {
            (Some(self.word()), 0)
        } else {
            (Some(u16::from(self.fetched())), 0)
        };
        let second = self.fetched();
        let third = self.fetched();

        if third & 0x80 != 0 {
            self.made_against(literal, first, second, third)
        } else {
            let at = self.built(literal, first, second)?;

            self.store(third, 0xC000 | at as u16)
        }
    }

    /// A fresh pair cell filled per MAKE_PAIR's argument shapes.
    fn built(&mut self, literal: Option<u16>, first: u8, second: u8) -> Fall<usize> {
        let at = self.claimed(2)?;

        self.filled(literal, first, at)?;
        self.filled(None, second, at + 1)?;

        Ok(at)
    }

    /// One cell word: a literal lands, a destination is served.
    fn filled(&mut self, literal: Option<u16>, dest: u8, at: usize) -> Fall<()> {
        if let Some(literal) = literal {
            self.heap[at] = literal;

            Ok(())
        } else if dest & 0x80 != 0 {
            self.heap[at] = self.slotted(dest);

            Ok(())
        } else {
            self.heap[at] = 0;

            self.store(dest, 0x8000 | at as u16)
        }
    }

    /// MAKE_PAIR's unify shape: match an existing value.
    fn made_against(&mut self, literal: Option<u16>, first: u8, second: u8, third: u8) -> Fall<()> {
        let value = self.deref(self.slotted(third));

        if paired(value) {
            if let Some(literal) = literal {
                self.unify(literal, 0x8000 | (value & 0x1FFF))?;
            } else {
                self.store(first, 0x8000 | (value & 0x1FFF))?;
            }

            self.store(second, 0x8000 | ((value & 0x1FFF) + 1))
        } else if referenced(value) {
            let at = self.built(literal, first, second)?;

            self.unify(value, 0xC000 | at as u16)
        } else {
            Err(Slip::Missed)
        }
    }

    /// AUX_PUSH_RAW: one raw word onto the aux stack (Aa-machine:
    /// AUX_PUSH_RAW).
    fn op_aux_push_raw(&mut self, op: u8) -> Fall<()> {
        if op == RAW_ZERO {
            self.pushed_aux(0)
        } else if op == RAW_WORD {
            let told = self.word();

            self.pushed_aux(told)
        } else {
            let told = u16::from(self.fetched());

            self.pushed_aux(told)
        }
    }

    /// AUX_POP_LIST_CHK: drain the stack, failing unless the key
    /// appears.
    fn op_aux_pop_list_chk(&mut self) -> Fall<()> {
        let key = self.value();
        let key = self.deref(key);
        let mut found = false;

        loop {
            let value = self.popped_aux()?;

            if value == 0 {
                break;
            }

            if value == key {
                found = true;
            }
        }

        if !found {
            return Err(Slip::Missed);
        }

        Ok(())
    }

    /// AUX_POP_LIST_MATCH: every key element must match the stack.
    ///
    /// Each element of the key list must unify with some element
    /// of the stacked list, or the whole instruction fails.
    fn op_aux_pop_list_match(&mut self) -> Fall<()> {
        let kept = self.top;
        let key = self.value();
        let mut key = self.deref(key);
        let listed = self.deserialized_list()?;

        while paired(key) {
            let mut probe = listed;
            let mut matched = false;

            while paired(probe) && !matched {
                if self.agreeable(0x8000 | (probe & 0x1FFF), 0x8000 | (key & 0x1FFF)) {
                    matched = true;
                }

                probe = self.heap[usize::from(probe & 0x1FFF) + 1];
            }

            if !matched {
                return Err(Slip::Missed);
            }

            key = self.heap[usize::from(key & 0x1FFF) + 1];
        }

        self.top = kept;

        Ok(())
    }

    /// SPLIT_LIST: copy a list up to a given tail (Aa-machine:
    /// SPLIT_LIST).
    fn op_split_list(&mut self) -> Fall<()> {
        let listed = self.value();
        let mut listed = self.deref(listed);
        let ended = self.value();
        let ended = self.deref(ended);
        let dest = self.fetched();

        if listed == ended || !paired(listed) {
            return self.store(dest, EMPTY);
        }

        let first = self.claimed(2)?;
        let mut current = first;

        loop {
            self.heap[current] = self.heap[usize::from(listed & 0x1FFF)];
            listed = self.deref(self.heap[usize::from(listed & 0x1FFF) + 1]);

            if listed == ended || !paired(listed) {
                break;
            }

            let following = self.claimed(2)?;

            self.heap[current + 1] = 0xC000 | following as u16;
            current = following;
        }

        self.heap[current + 1] = EMPTY;

        self.store(dest, 0xC000 | first as u16)
    }

    /// PUSH_STOP: a stop frame and its catching choice point.
    fn op_push_stop(&mut self) -> Fall<()> {
        if self.auxp + 2 > self.trl {
            return Err(Slip::Fault(AUX_FULL));
        }

        self.pushed_aux(self.stc as u16)?;
        self.pushed_aux(self.sta as u16)?;
        self.sta = self.auxp;

        let handler = self.target();

        self.pushed_choice(0, handler)?;
        self.stc = self.cho;

        Ok(())
    }

    /// POP_STOP: leave the stop frame (Aa-machine: POP_STOP).
    fn op_pop_stop(&mut self) -> Fall<()> {
        self.auxp = self.sta;
        self.sta = usize::from(self.popped_aux()?);
        self.stc = usize::from(self.popped_aux()?);

        Ok(())
    }

    /// SPLIT_WORD: a word as its list of characters (Aa-machine:
    /// SPLIT_WORD).
    fn op_split_word(&mut self) -> Fall<()> {
        let value = self.value();
        let value = self.deref(value);

        let told = if dicted(value) {
            self.prepended(usize::from(value & 0x1FFF), EMPTY)?
        } else if chared(value) {
            self.pair(value, EMPTY)?
        } else if numbered(value) {
            let mut number = value & NUMBER_TOP;
            let mut told = EMPTY;

            loop {
                told = self.pair(0x4000 | (number % 10), told)?;
                number /= 10;

                if number == 0 {
                    break;
                }
            }

            told
        } else if extdicted(value) {
            let first = self.heap[usize::from(value & 0x1FFF)];

            if first >= UNBOUND_MARK {
                first
            } else {
                let tail = self.heap[usize::from(value & 0x1FFF) + 1];

                self.prepended(usize::from(first & 0x1FFF), tail)?
            }
        } else {
            return Err(Slip::Missed);
        };
        let dest = self.fetched();

        self.store(dest, told)
    }

    /// JOIN_WORDS: parse a character list back into a word.
    fn op_join_words(&mut self) -> Fall<()> {
        let value = self.value();
        let value = self.deref(value);

        if !paired(value) {
            return Err(Slip::Missed);
        }

        let first = self.deref(self.heap[usize::from(value & 0x1FFF)]);

        if chared(first) {
            let tail = self.deref(self.heap[usize::from(value & 0x1FFF) + 1]);

            if tail == EMPTY {
                let dest = self.fetched();

                return self.store(dest, first);
            }
        }

        let Some(codes) = self.joined_codes(value) else {
            return Err(Slip::Missed);
        };
        let dest = self.fetched();
        let told = self.parsed(&codes)?;

        self.store(dest, told)
    }

    // -- random access opcodes -------------------------------------------

    /// LOAD_WORD: read an object's field (Aa-machine: LOAD_WORD).
    fn op_load_word(&mut self, op: u8) -> Fall<()> {
        let obj = if op & 0x80 != 0 { 0 } else { self.value() };
        let field = self.index();
        let dest = self.fetched();
        let told = self.field(field, obj);

        self.store(dest, told)
    }

    /// LOAD_BYTE: read half an object's field (Aa-machine:
    /// LOAD_BYTE).
    fn op_load_byte(&mut self, op: u8) -> Fall<()> {
        let obj = if op & 0x80 != 0 { 0 } else { self.value() };
        let field = self.index();
        let told = self.field(field >> 1, obj);
        let dest = self.fetched();
        let told = if field & 1 != 0 {
            told & 0xFF
        } else {
            told >> 8
        };

        self.store(dest, told)
    }

    /// LOAD_VAL: read a stored value, long-term data revived.
    fn op_load_val(&mut self, op: u8) -> Fall<()> {
        let obj = if op & 0x80 != 0 { 0 } else { self.value() };
        let field = self.index();
        let dest = self.fetched();
        let held = self.field(field, obj);
        let told = self.lifted(held)?;

        if told == 0 {
            return Err(Slip::Missed);
        }

        self.store(dest, told)
    }

    /// STORE_WORD: write an object's field (Aa-machine:
    /// STORE_WORD).
    fn op_store_word(&mut self, op: u8) -> Fall<()> {
        let obj = if op & 0x80 != 0 { 0 } else { self.value() };
        let field = self.index();
        let value = self.value();
        let at = self.field_at(field, obj)?;

        self.ram[at] = value;

        Ok(())
    }

    /// STORE_BYTE: write half an object's field (Aa-machine:
    /// STORE_BYTE).
    fn op_store_byte(&mut self, op: u8) -> Fall<()> {
        let obj = if op & 0x80 != 0 { 0 } else { self.value() };
        let field = self.index();
        let value = self.value();
        let at = self.field_at(field >> 1, obj)?;

        if field & 1 != 0 {
            self.ram[at] = (self.ram[at] & 0xFF00) | (value & 0xFF);
        } else {
            self.ram[at] = (self.ram[at] & 0x00FF) | ((value & 0xFF) << 8);
        }

        Ok(())
    }

    /// STORE_VAL: keep a value in an object's field.
    ///
    /// Live heap data is serialized into long-term storage so it
    /// survives the heap's unwinding.
    fn op_store_val(&mut self, op: u8) -> Fall<()> {
        let obj = if op & 0x80 != 0 {
            0
        } else {
            let held = self.value();

            self.deref(held)
        };
        let field = self.index();
        let value = self.value();

        if obj <= self.nob || value != 0 {
            let at = self.field_at(field, obj)?;

            self.kept_longterm(at, value)?;
        }

        Ok(())
    }

    /// SET_FLAG: raise one of an object's flags (Aa-machine:
    /// SET_FLAG).
    fn op_set_flag(&mut self, op: u8) -> Fall<()> {
        let obj = if op & 0x80 != 0 { 0 } else { self.value() };
        let flag = self.index();
        let at = self.field_at(flag / BITS_PER_WORD, obj)?;

        self.ram[at] |= 0x8000 >> (flag % BITS_PER_WORD);

        Ok(())
    }

    /// RESET_FLAG: lower one of an object's flags (Aa-machine:
    /// RESET_FLAG).
    fn op_reset_flag(&mut self, op: u8) -> Fall<()> {
        let obj = if op & 0x80 != 0 {
            0
        } else {
            let held = self.value();

            self.deref(held)
        };
        let flag = self.index();

        if obj <= self.nob {
            let at = self.field_at(flag / BITS_PER_WORD, obj)?;

            self.ram[at] &= !(0x8000 >> (flag % BITS_PER_WORD));
        }

        Ok(())
    }

    /// UNLINK: remove an object from a linked field chain
    /// (Aa-machine: UNLINK).
    fn op_unlink(&mut self, op: u8) -> Fall<()> {
        let obj = if op & 0x80 != 0 { 0 } else { self.value() };
        let root = self.index();
        let field = self.index();
        let at = self.field_at(root, obj)?;
        let key = self.value();
        let key = self.deref(key);

        self.unlinked(at, field, key)
    }

    /// SET_PARENT: move an object in the tree (Aa-machine:
    /// SET_PARENT).
    fn op_set_parent(&mut self, op: u8) -> Fall<()> {
        let first = if op & 0x80 != 0 {
            u16::from(self.fetched())
        } else {
            let held = self.value();

            self.deref(held)
        };
        let second = if op & 0x01 != 0 {
            u16::from(self.fetched())
        } else {
            let held = self.value();

            self.deref(held)
        };

        if second != 0 && (!objected(first) || !objected(second)) {
            return Err(Slip::Fault(EXPECTED_OBJECT));
        }

        if objected(first) {
            let parent = self.ram[self.field_at(0, first)?];

            if parent != 0 {
                let at = self.field_at(1, parent)?;

                self.unlinked(at, 2, first)?;
            }

            let at = self.field_at(0, first)?;

            self.ram[at] = second;

            if second != 0 {
                let sibling_at = self.field_at(2, first)?;
                let child_at = self.field_at(1, second)?;

                self.ram[sibling_at] = self.ram[child_at];

                let child_at = self.field_at(1, second)?;

                self.ram[child_at] = first;
            }
        }

        Ok(())
    }

    // -- conditional branches --------------------------------------------

    /// Take the CODE operand's jump when the test says to.
    ///
    /// The negated opcodes carry bit 6: IF jumps on true, IFN on
    /// false (Aa-machine: Opcode semantics).
    fn jumped(&mut self, op: u8, told: bool) {
        let landing = self.target();

        if told != (op & 0x40 != 0) {
            self.inst = landing;
        }
    }

    /// IF_RAW_EQ and IFN_RAW_EQ (Aa-machine: IF_RAW_EQ).
    fn op_if_raw_eq(&mut self, op: u8) -> Fall<()> {
        let first = if op & 0x80 != 0 { 0 } else { self.word() };
        let second = self.value();

        self.jumped(op, first == second);

        Ok(())
    }

    // -- output ----------------------------------------------------------

    /// The four PRINT_*_STR_* opcodes (Aa-machine: PRINT_A_STR_A).
    ///
    /// A WRIT string lands with its whitespace discipline: the A
    /// and N halves of the name say whether a space may lead and
    /// whether one may follow.
    fn op_print_str(&mut self, op: u8) -> Fall<()> {
        let address = self.string();

        if self.spc == PENDING || (self.spc == AUTO && op & 0x80 == 0) {
            self.voice.space();
        } else if self.spc == NBSP {
            self.voice.nbsp();
        }

        let told = self.speech.spelled(address)?;

        self.said(&told);
        self.spc = if op & 0x01 != 0 { NOSPACE } else { AUTO };

        Ok(())
    }

    /// PRINT_VAL: spell a value out (Aa-machine: PRINT_VAL).
    ///
    /// While words are being collected, the value is serialized
    /// onto the aux stack instead of spoken.
    fn op_print_val(&mut self) -> Fall<()> {
        let value = self.value();
        let value = self.deref(value);

        if self.cwl != 0 {
            self.serialized(value)?;
        } else if chared(value) {
            self.printed_char((value & 0xFF) as u8);
        } else {
            self.spaced_auto();

            if !(dicted(value) || extdicted(value)) {
                self.upper = false;
            }

            let told = self.valued_text(value);

            self.said(&told);
            self.spc = AUTO;
        }

        Ok(())
    }

    /// One character with the LANG chunk's spacing manners.
    fn printed_char(&mut self, code: u8) {
        if self.spc == PENDING || (self.spc == AUTO && !self.unspaced_before.contains(&code)) {
            self.voice.space();
        }

        let told = self.character(u16::from(code)).to_string();

        self.said(&told);
        self.spc = if self.unspaced_after.contains(&code) {
            NOSPACE
        } else {
            AUTO
        };
    }

    /// ENTER_DIV (Aa-machine: ENTER_DIV).
    fn op_enter_div(&mut self) -> Fall<()> {
        let style = self.index();

        if self.cwl == 0 {
            if self.n_span != 0 {
                return Err(Slip::Fault(BAD_OUTPUT_STATE));
            }

            self.voice.enter_div(i64::from(style));
            self.divs.push(style);
            self.spc = PAR;
        }

        Ok(())
    }

    /// Opcode $67: ENTER_STATUS before 1.0, SET_BODY from it
    /// (Aa-machine: ENTER_STATUS; SET_BODY).
    fn op_status_or_body(&mut self) -> Fall<()> {
        if self.major < 1 {
            let style = self.index();

            return self.entered_status(0, style);
        }

        let style = self.index();

        if self.in_status != 0 || self.n_span != 0 {
            return Err(Slip::Fault(BAD_OUTPUT_STATE));
        }

        self.voice.set_body(i64::from(style));

        Ok(())
    }

    /// Enter a status area, the illegal states loud.
    fn entered_status(&mut self, area: u8, style: u16) -> Fall<()> {
        if self.in_status != 0 || self.n_span != 0 {
            return Err(Slip::Fault(BAD_OUTPUT_STATE));
        }

        if self.cwl == 0 {
            self.voice.enter_status(i64::from(area), i64::from(style));
            self.in_status = usize::from(area) + 1;
            self.spc = PAR;
        }

        Ok(())
    }

    /// LEAVE_STATUS, at either of its two historical seats.
    fn op_leave_status(&mut self, op: u8) -> Fall<()> {
        if (op == OLD_LEAVE_STATUS) == (self.major >= 1) {
            return Err(Slip::Refused(machine_error(format!(
                "opcode {op:#04x} is not LEAVE_STATUS in a format {}.x story \
                 (Aa-machine: LEAVE_STATUS)",
                self.major
            ))));
        }

        if self.cwl == 0 {
            self.voice.leave_status();
            self.in_status = 0;
            self.spc = PAR;
        }

        Ok(())
    }

    /// ENTER_LINK: a link whose click types its word list.
    fn op_enter_link(&mut self) -> Fall<()> {
        let listed = self.value();
        let mut listed = self.deref(listed);

        if self.cwl == 0 {
            if self.n_link == 0 {
                self.spaced_auto();

                let held = self.upper;

                self.upper = false;

                let mut pieces: Vec<String> = Vec::new();

                while paired(listed) {
                    let value = self.deref(self.heap[usize::from(listed & 0x1FFF)]);

                    if linkable(value) {
                        pieces.push(self.valued_text(value));
                    }

                    listed = self.deref(self.heap[usize::from(listed & 0x1FFF) + 1]);
                }

                self.voice.enter_link(&pieces.join(" "));
                self.upper = held;
                self.spc = NOSPACE;
            }

            self.n_link += 1;
            self.n_span += 1;
        }

        Ok(())
    }

    // -- system control --------------------------------------------------

    /// EXT0: the single-byte system operations (Aa-machine: Opcode
    /// semantics).
    fn ext0(&mut self) -> Fall<Option<Wait>> {
        let selector = self.fetched();

        match selector {
            0x00 => {
                self.voice.sync();
                self.running = false;

                Ok(Some(Wait::Quit))
            }
            0x01 => {
                self.ext_restart();

                Ok(None)
            }
            0x02 => {
                self.ext_restore();

                Ok(None)
            }
            0x03 => self.ext_undo().map(|()| None),
            0x04 => {
                if self.cwl == 0 {
                    self.voice.unstyle();
                }

                Ok(None)
            }
            0x05 => {
                if self.cwl == 0 {
                    self.spaced_auto();
                    self.voice.say(&self.story.serial.clone());
                    self.spc = AUTO;
                }

                Ok(None)
            }
            0x06 => self.cleared(false).map(|()| None),
            0x07 => self.cleared(true).map(|()| None),
            0x08 => {
                if !self.voice.script_on() {
                    return Err(Slip::Missed);
                }

                Ok(None)
            }
            0x09 => {
                self.voice.script_off();

                Ok(None)
            }
            0x0A => {
                self.trace = true;

                Ok(None)
            }
            0x0B => {
                self.trace = false;

                Ok(None)
            }
            0x0C => {
                self.cwl += 1;

                Ok(None)
            }
            0x0D => {
                self.cwl -= 1;

                Ok(None)
            }
            0x0E => {
                if self.cwl == 0 {
                    self.upper = true;
                }

                Ok(None)
            }
            0x0F => {
                self.voice.clear_links();

                Ok(None)
            }
            0x10 => {
                if self.n_span != 0 {
                    return Err(Slip::Fault(BAD_OUTPUT_STATE));
                }

                self.voice.clear_old();

                Ok(None)
            }
            0x11 => {
                self.voice.clear_div();

                Ok(None)
            }
            0x12 => {
                if self.in_status != 0 {
                    return Err(Slip::Fault(BAD_OUTPUT_STATE));
                }

                self.voice.clear_status();

                Ok(None)
            }
            0x13 => {
                if self.cwl == 0 {
                    self.spc = self.spc.max(NBSP);
                }

                Ok(None)
            }
            _ => Err(Slip::Refused(machine_error(format!(
                "reached EXT0 {selector:#04x} at ${:06x}, which this engine \
                 does not carry (Aa-machine: Story file)",
                self.inst - 2
            )))),
        }
    }

    /// RESTART: the whole game state reborn (Aa-machine: RESTART).
    fn ext_restart(&mut self) {
        self.cleared_divs();
        self.reset(0, true);

        let held = self.held.clone();

        self.restored(&held);
        self.voice.reset();
    }

    /// RESTORE: revive a kept savefile (Aa-machine: RESTORE).
    ///
    /// A voice with no file, or a file that does not belong to
    /// this story, is a failed restore: execution simply
    /// continues, the spec's own shape. A revived state resumes at
    /// the address its SAVE named, the output returned to its base
    /// and the saved divs re-entered.
    fn ext_restore(&mut self) {
        if !self.voice.has_saves() {
            return;
        }

        let Some(told) = self.voice.restore() else {
            return;
        };
        let Ok(state) = saves::revived(&self.story, &told) else {
            return;
        };

        self.restored(&state);
        self.voice.leave_all();
        self.in_status = 0;
        self.n_span = 0;
        self.n_link = 0;

        for style in self.divs.clone() {
            self.voice.enter_div(i64::from(style));
        }
    }

    /// UNDO: step back to the last kept moment (Aa-machine: UNDO).
    fn ext_undo(&mut self) -> Fall<()> {
        if let Some(state) = self.undo.pop() {
            self.cleared_divs();
            self.restored(&state);
        } else if !self.pruned {
            return Err(Slip::Missed);
        }

        Ok(())
    }

    /// CLEAR and CLEAR_ALL share the div-restating dance
    /// (Aa-machine: CLEAR).
    fn cleared(&mut self, all: bool) -> Fall<()> {
        if self.cwl == 0 {
            if self.in_status != 0 || self.n_span != 0 {
                return Err(Slip::Fault(BAD_OUTPUT_STATE));
            }

            let kept = std::mem::take(&mut self.divs);

            self.cleared_divs();

            if all {
                self.voice.clear_all();
            } else {
                self.voice.clear();
            }

            for &style in &kept {
                self.voice.enter_div(i64::from(style));
            }

            self.divs = kept;
        }

        Ok(())
    }

    /// SAVE: keep the whole state through the voice (Aa-machine:
    /// SAVE).
    ///
    /// A voice that keeps no files, or one whose keeping is
    /// refused or cancelled, fails the instruction; success
    /// continues past it, and a later restore lands at the CODE
    /// operand (Aa-machine: Savefile).
    fn op_save(&mut self) -> Fall<()> {
        let landing = self.target();

        if self.in_status != 0 || self.n_span != 0 {
            return Err(Slip::Fault(BAD_OUTPUT_STATE));
        }

        if !self.voice.has_saves() {
            return Err(Slip::Missed);
        }

        let told = saves::kept(&self.story, &self.captured(landing as u32));

        if !self.voice.save(&told) {
            return Err(Slip::Missed);
        }

        Ok(())
    }

    /// SAVE_UNDO: keep this moment in memory (Aa-machine:
    /// SAVE_UNDO).
    fn op_save_undo(&mut self) -> Fall<()> {
        let landing = self.target();

        if self.in_status != 0 || self.n_span != 0 {
            return Err(Slip::Fault(BAD_OUTPUT_STATE));
        }

        if self.undo.len() > UNDO_KEPT {
            self.undo.remove(0);
            self.pruned = true;
        }

        self.undo.push(self.captured(landing as u32));

        Ok(())
    }

    /// Settle the whitespace and the display before a wait.
    fn spaced_input(&mut self) {
        if self.spc == AUTO || self.spc == PENDING {
            self.voice.space();
        } else if self.spc == NBSP {
            self.voice.nbsp();
        }

        self.voice.sync();
    }

    /// VM_INFO: the interpreter examined (Aa-machine: VM_INFO).
    fn op_vm_info(&mut self) -> Fall<()> {
        let selector = self.fetched();

        if selector > SELECTOR_TOP {
            return Err(Slip::Refused(machine_error(format!(
                "VM_INFO selector {selector:#04x} is undefined (Aa-machine: VM_INFO)"
            ))));
        }

        let told: u16 = if selector & 0x40 != 0 {
            u16::from(self.featured(selector & 0x3F))
        } else if selector & 0x20 != 0 {
            0x4000 | (self.voice.measured(i64::from(selector & 0x1F)) as u16)
        } else if selector < PEAK_AREAS {
            let counted = match selector {
                0 => self.heap.iter().filter(|&&value| value != UNUSED).count(),
                1 => self.aux.iter().filter(|&&value| value != UNUSED).count(),
                _ => self.ram[self.ltb..]
                    .iter()
                    .filter(|&&value| value != UNUSED)
                    .count(),
            };

            0x4000 + counted as u16
        } else {
            0x4000
        };
        let dest = self.fetched();

        self.store(dest, told)
    }

    /// One interpreter-feature answer (Aa-machine: VM_INFO).
    fn featured(&self, feature: u8) -> bool {
        match feature {
            0x00 | 0x03 => true,
            0x01 => self.voice.has_saves(),
            0x02 => self.voice.has_links(),
            0x04 => self.voice.has_styles(),
            0x05 => self.voice.has_color(),
            0x06 => self.voice.has_alignment(),
            0x10 => self.voice.script_active(),
            0x20 => self.voice.has_top_status(),
            0x21 => self.voice.has_inline_status(),
            _ => false,
        }
    }

    /// TRACEPOINT: a debug mark, told only while tracing.
    fn op_tracepoint(&mut self) -> Fall<()> {
        let event = self.string();
        let shape = self.string();
        let source = self.string();
        let line = self.word();

        if self.trace {
            let mut pieces = String::new();
            let mut seat = 0;

            for piece in self.speech.spelled(shape)?.chars() {
                if piece == '$' {
                    pieces.push_str(&self.valued_text(self.deref(self.regs[seat])));
                    seat += 1;
                } else {
                    pieces.push(piece);
                }
            }

            let told = format!(
                "{}({pieces}) {}:{line}",
                self.speech.spelled(event)?,
                self.speech.spelled(source)?
            );

            self.voice.trace(&told);
        }

        Ok(())
    }
}

/// Play a story through a plain voice; the whole telling comes
/// back.
///
/// The drill is the reference Node frontend's own: each script
/// line is echoed raw into the telling with the line ending it
/// arrived with, a line wait takes the line whole, a key wait
/// takes it a keypress at a time with a return to finish, and the
/// telling closes on a broken line -- which is what makes the
/// result diff clean against the reference engine's transcripts.
pub fn walked(story: Story, script: &str, seed: Option<u32>) -> Result<String, VoxamError> {
    let voice = PlainVoice::new(&story)?;
    let mut machine = Machine::new(story, voice, seed)?;
    let mut feed = split_kept(script).into_iter();
    let mut waiting = machine.run(None)?;

    while waiting != Wait::Quit {
        let Some(told) = feed.next() else {
            break;
        };
        let line = told.trim_end_matches(['\r', '\n']);
        let echo = if told == line {
            line.to_string()
        } else {
            format!("{line}\n")
        };

        machine.voice.echoed(&echo);

        if waiting == Wait::Line {
            machine.voice.prompted();
            waiting = machine.deliver_line(line)?;
        } else {
            let mut keys = line.chars();

            while waiting == Wait::Key {
                let Some(key) = keys.next() else {
                    break;
                };

                waiting = machine.deliver_key(u32::from(key))?;
            }

            if waiting == Wait::Key {
                waiting = machine.deliver_key(0x0D)?;
            }
        }
    }

    machine.voice.line();

    Ok(machine.voice.told().to_string())
}

/// The reference's splitlines(keepends=True), for the line endings
/// a script actually carries: \n, \r\n, and a lone \r.
fn split_kept(script: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut held = String::new();
    let mut chars = script.chars().peekable();

    while let Some(piece) = chars.next() {
        held.push(piece);

        if piece == '\n' || (piece == '\r' && chars.peek() != Some(&'\n')) {
            lines.push(std::mem::take(&mut held));
        }
    }

    if !held.is_empty() {
        lines.push(held);
    }

    lines
}

/// Big-endian bytes as an integer, short slices included -- the
/// reference's int.from_bytes over a Python slice.
fn be(data: &[u8], from: usize, to: usize) -> usize {
    data[from.min(data.len())..to.min(data.len())]
        .iter()
        .fold(0, |told, &byte| (told << 8) | usize::from(byte))
}

/// Each extended character to its lowercase charset code
/// (Aa-machine: LANG).
fn cased(lang: &[u8], extended: &[char]) -> HashMap<char, u8> {
    let at = be(lang, 2, 4);

    (0..usize::from(lang[at]))
        .map(|seat| (extended[seat], lang[at + 1 + seat * 5]))
        .collect()
}

/// Each extended character to its uppercase self (Aa-machine:
/// LANG).
fn upcased(lang: &[u8], extended: &[char]) -> HashMap<char, String> {
    let at = be(lang, 2, 4);
    let mut told = HashMap::new();

    for seat in 0..usize::from(lang[at]) {
        let upper = lang[at + 1 + seat * 5 + 1];
        let raised = if upper < 0x80 {
            char::from(upper).to_string()
        } else {
            extended[usize::from(upper & 0x7F)].to_string()
        };

        told.insert(extended[seat], raised);
    }

    told
}

type Specials = (HashSet<u8>, HashSet<u8>, HashSet<u8>);

/// The special characters: stops and the whitespace inhibitors
/// (Aa-machine: LANG). Fails for a set running past the chunk.
fn stopped(lang: &[u8], version: (u8, u8)) -> Result<Specials, VoxamError> {
    let mut at = be(lang, 6, 8);
    let mut sets: Vec<HashSet<u8>> = Vec::new();
    let wanted = if version >= (0, 4) { 3 } else { 1 };

    for _ in 0..wanted {
        let Some(ended) = lang[at.min(lang.len())..]
            .iter()
            .position(|&byte| byte == 0)
            .map(|seat| at + seat)
        else {
            return Err(machine_error(
                "a LANG special-character set is missing its null ending \
                 (Aa-machine: LANG)"
                    .into(),
            ));
        };

        sets.push(lang[at..ended].iter().copied().collect());
        at = ended + 1;
    }

    while sets.len() < SPECIAL_SETS {
        sets.push(HashSet::new());
    }

    let mut sets = sets.into_iter();

    Ok((
        sets.next().expect("three sets stand"),
        sets.next().expect("three sets stand"),
        sets.next().expect("three sets stand"),
    ))
}

/// Each dictionary word's raw bytes to its seat (Aa-machine:
/// DICT).
fn sought(dictionary: &[u8]) -> HashMap<Vec<u8>, u16> {
    let count = be(dictionary, 0, 2);
    let mut told = HashMap::new();

    for seat in 0..count {
        let length = usize::from(dictionary[2 + seat * 3]);
        let at = be(dictionary, 3 + seat * 3, 5 + seat * 3);

        told.insert(dictionary[at..at + length].to_vec(), seat as u16);
    }

    told
}

#[cfg(test)]
#[path = "machine_tests.rs"]
mod tests;
