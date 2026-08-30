//! A running Z-Machine (§6.1).
//!
//! The state of play: the memory image, the routine call state, and
//! the program counter, advanced one instruction at a time. This
//! port covers the plain-stream machine -- the full Version 1 to 3
//! opcode set plus the version-neutral core -- with the screen
//! model, timed input, and the later eras still to arrive; an
//! opcode past the frontier reports itself by name, exactly as the
//! Python reference's frontier reporter does.
//!
//! One structural departure from the reference: Python suspends a
//! read by raising through the step loop, where this machine's
//! `step` returns [`Step::Suspended`] and `run` hands back
//! [`RunState::Waiting`]. The contract is the reference's
//! suspension seam either way -- the pc has not moved past the
//! read, its operands' side effects are parked and never repeated,
//! and the host answers through `deliver_line`.

use std::collections::{HashSet, VecDeque};

use crate::errors::VoxamError;
use crate::frontend::{Frontend, Status};
use crate::saves::SaveSlot;
use crate::zmachine::dictionary::{Dictionary, tokenize};
use crate::zmachine::frames::CallStack;
use crate::zmachine::header::{FLAGS_2, declare};
use crate::zmachine::instruction::{Instruction, Operand, OperandType};
use crate::zmachine::memory::Memory;
use crate::zmachine::objects::ObjectTable;
use crate::zmachine::packed::{routine_address, string_address};
use crate::zmachine::riders::{Branch, read_branch, read_store_variable};
use crate::zmachine::rng::Randomizer;
use crate::zmachine::routine::Routine;
use crate::zmachine::snapshot::{FrameSnapshot, Snapshot};
use crate::zmachine::story::Story;
use crate::zmachine::variables::Variables;
use crate::zmachine::windows::{
    self, CURRENT_WINDOW, LINE_COUNT, WindowLedger, X_COORDINATE, X_CURSOR, X_SIZE, Y_COORDINATE,
    Y_CURSOR, Y_SIZE,
};
use crate::zmachine::zscii::{
    Units, char_to_zscii, decode_units, extras, unit_to_zscii, units_to_string, zscii_to_units,
};

const FALSE_VALUE: u16 = 0;

/// A single mouse click's §10.3.3 input code.
pub const SINGLE_CLICK: u16 = 254;

/// The loudest §9.3 volume, "full".
pub const FULL_VOLUME: u16 = 8;

// The header word naming the extension table, and the extension
// length that carries the §10.3.2 click words.
const HEADER_EXTENSION_WORD: usize = 0x36;
const EXTENSION_CLICK_WORDS: u16 = 2;

// The §9 sound effects: the interpreter's two bleeps, then the
// sampled numbers; the effect operands and volume spellings of
// §15 sound_effect.
const FIRST_SAMPLED_SOUND: u16 = 3;
const START_EFFECT: u16 = 2;
const STOP_EFFECT: u16 = 3;
const FINISH_EFFECT: u16 = 4;
const LOUDEST_VOLUME: u16 = 255;
const LOWEST_VOLUME: u16 = 1;
const SOUND_REPEATS_VERSION: u8 = 5;
const FOREVER_BYTE: u16 = 255;
const FOREVER_REPEATS: u16 = 0;
const TRUE_VALUE: u16 = 1;

/// How many save_undo captures stack up before the oldest is
/// quietly forgotten.
const UNDO_DEPTH: usize = 16;

/// save and restore branch through Version 3 and store from 4
/// (§14); a restore answers 2 at its save's rider (§15 save).
const BRANCHING_SAVE_FINAL_VERSION: u8 = 3;
const RESTORED_VALUE: u16 = 2;

/// Calling packed address 0 does nothing and returns false (§6.4.7).
const NULL_ROUTINE: u16 = 0;

/// je with fewer than two operands is not permitted (§15 remarks).
const JE_MINIMUM_OPERANDS: usize = 2;

/// Who this interpreter says it is (§11.1.3): platform 6, "IBM PC",
/// the identity whose Version 6 assets ship in the common Blorbs,
/// revision letter V for Voxam.
const INTERPRETER_PLATFORM: u8 = 6;
const INTERPRETER_REVISION: u8 = b'V';

/// The Standard revision this interpreter obeys (§11.1.5).
const STANDARD_MAJOR: u8 = 1;
const STANDARD_MINOR: u8 = 1;

/// Version 5 defaults for colours (§8.3.1): white on blue was the
/// Amiga's; 9 and 2 are white and black, the stream's honest inks.
const DEFAULT_FOREGROUND_COLOUR: u8 = 9;
const DEFAULT_BACKGROUND_COLOUR: u8 = 2;

/// From Version 5 the text buffer is counted rather than
/// zero-terminated (§15 read), and reads may carry interrupts from
/// Version 4 (§15).
const COUNTED_TEXT_VERSION: u8 = 5;
const TIMED_READ_VERSION: u8 = 4;
const TIMED_TIME_INDEX: usize = 2;
const TIMED_ROUTINE_INDEX: usize = 3;

/// The §10.5.2.1 function key codes a terminating table may name;
/// 255 names them all.
const ANY_FUNCTION_KEY: u8 = 255;
const TERMINATING_VERSION: u8 = 5;

/// A buffer smaller than this cannot be real (§15 read).
const MINIMUM_PARSE_WORDS: u8 = 1;

/// The §7 output streams.
const SCREEN_STREAM: i32 = 1;
const TRANSCRIPT_STREAM: i32 = 2;
const MEMORY_STREAM: i32 = 3;
const COMMANDS_STREAM: i32 = 4;
const REDIRECTION_LIMIT: usize = 16;
const REDIRECTION_DATA_OFFSET: usize = 2;
const REDIRECTION_OPERANDS: usize = 2;

/// The §10.2 input streams.
const KEYBOARD_INPUT_STREAM: u16 = 0;
const FILE_INPUT_STREAM: u16 = 1;

/// 'Flags 2' bit 0 holds stream 2's status (§7.4).
const TRANSCRIPT_BIT: u16 = 0x01;

/// scan_table's optional form byte, $82 when absent (§15).
const SCAN_FORM_OPERAND: usize = 3;
const DEFAULT_SCAN_FORM: u16 = 0x82;
const SCAN_WORD_BIT: u16 = 0x80;
const SCAN_FIELD_MASK: u16 = 0x7F;

/// The §8.1.2 fonts: 0 asks which is current, 1 is normal, 3 the
/// character graphics font, 4 fixed-pitch; a font not on offer
/// stores the 0 refusal §8.1.3 builds permission on.
const CURRENT_FONT: u16 = 0;
const NORMAL_FONT: u16 = 1;
const GRAPHICS_FONT: u16 = 3;
const COURIER_FONT: u16 = 4;
const FONT_REFUSED: u16 = 0;

/// erase_line 1 erases from the cursor to the end of its line
/// (§15 erase_line).
const ERASE_TO_END: u16 = 1;

/// erase_window's signed operand: -1 unsplits and clears (§8.7.3.3).
const UNSPLIT_ERASE: i32 = -1;

/// print_table's optional operands (§15 print_table).
const PRINT_TABLE_HEIGHT_OPERAND: usize = 2;
const PRINT_TABLE_SKIP_OPERAND: usize = 3;

/// tokenise's optional operands (§15 tokenise).
const TOKENISE_DICTIONARY_OPERAND: usize = 2;
const TOKENISE_FLAG_OPERAND: usize = 3;

const SIGN_BIT: u16 = 0x8000;
const WORD_MASK: i64 = 0xFFFF;
const WORD_SIZE: usize = 2;

/// A branch destination is the address after the branch data, plus
/// the offset, minus two (§4.7.2); jump shares the arithmetic.
const BRANCH_TARGET_ADJUSTMENT: i64 = 2;

const ZSCII_NEWLINE: u16 = 13;

/// Interpret a word as a signed number (§2.2).
pub fn signed(value: u16) -> i32 {
    if value & SIGN_BIT != 0 {
        i32::from(value) - 0x10000
    } else {
        i32::from(value)
    }
}

/// Who the interpreter claims to be (§11.1.3, §11.1.4).
#[derive(Debug, Clone, Copy, Default)]
pub struct Identity {
    /// The §11.1.3 platform number to claim; None claims the
    /// default.
    pub interpreter: Option<u8>,
    /// Whether to set the legendary Tandy bit (§11.1.4 remarks).
    pub tandy: bool,
}

/// What a suspended read waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wants {
    Line,
    Key,
}

/// One suspended read: what the machine stands waiting for.
///
/// The pc has not moved past the read, and its operands' side
/// effects must never be repeated, so the whole post-input tail
/// parks here -- delivery runs the tail and steps past the
/// instruction, and nothing is ever re-executed.
pub struct Reading {
    /// Whether a whole line or a single keystroke is owed.
    pub wants: Wants,
    instruction: Instruction,
    text_buffer: usize,
    parse_buffer: usize,
    counted: bool,
    /// How many characters the text buffer can hold (§15 read) --
    /// the wire's line field carries it as the maxlen.
    pub capacity: usize,
    preloaded: usize,
    /// The preloaded text a counted buffer carried into the read
    /// (§15 read) -- the half-typed command a display should show
    /// already standing.
    pub held: String,
    /// The §10.5.2.1 codes that may end this read besides new-line.
    pub terminators: HashSet<u16>,
    /// The §15 interrupt cadence in tenths of a second, zero for an
    /// untimed read.
    pub time: u16,
    /// The packed interrupt routine, zero for none.
    pub routine: u16,
}

/// Which opcode a suspended file ask serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Save,
    Restore,
}

/// One suspended file ask: a save or restore awaiting its path.
///
/// The third Z wait, shaped like the reads: the pc has not moved
/// past the opcode and its §15 rider is still owed, so the finish
/// parks here -- the host's answer writes or reads the file and
/// runs the rider, while a cancel answers the opcode's own
/// failure, which every game already speaks.
pub struct Filing {
    /// Which opcode stands suspended.
    pub purpose: Purpose,
    instruction: Instruction,
    /// The Quetzal bytes a save carries ready to keep; None on a
    /// restore, whose bytes are still to be found.
    data: Option<Vec<u8>>,
}

/// What the machine stands waiting for: a read, or a file ask.
pub enum Waiting {
    Read(Reading),
    File(Filing),
}

/// What one step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Ran,
    Suspended,
}

/// Why run returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// The story quit.
    Halted,
    /// A read stands waiting; answer through deliver_line and run
    /// again.
    Waiting,
}

/// A running Z-Machine (§6.1).
pub struct Machine {
    story: Story,
    memory: Memory,
    calls: CallStack,
    variables: Variables,
    objects: ObjectTable,
    rng: Randomizer,
    frontend: Box<dyn Frontend>,
    identity: Identity,
    saves: Option<Box<dyn SaveSlot>>,
    pc: usize,
    running: bool,
    screen_selected: bool,
    /// Whether the story window (window 0) is selected: the only
    /// window whose text belongs in a transcript (§7.1.1).
    story_window: bool,
    // Cursor moves aimed at unselected stage windows: the move
    // reaches the stage when that window is next selected
    // (§8.8.3.5) -- until then the ledger holds it alone.
    aimed: std::collections::HashSet<u16>,
    font: u16,
    recording_commands: bool,
    file_input: bool,
    windows: WindowLedger,
    screen_buffering: u16,
    redirections: Vec<(usize, Units, Option<usize>)>,
    undo: VecDeque<Snapshot>,
    waiting: Option<Waiting>,
    wait_serial: u64,
    /// The end-of-sound routine of the sound now playing, and
    /// whether a sound has started since the last keyboard input
    /// -- The Lurking Horror's §9 pacing ledger.
    sound_routine: u16,
    sound_since_input: bool,
    /// The keystroke queue: read_char spends one scripted line a
    /// character at a time (§15 read_char).
    pending_keys: VecDeque<char>,
    /// The nimble half of the patient typist: when an interrupt
    /// terminates a timed read_char and the game loops straight
    /// back to the SAME read, the burned interval was typing time
    /// and the retry finds the keys ready. The address is the gate.
    typist_ready: Option<usize>,
    passed_reserved: HashSet<u8>,
}

impl Machine {
    /// Boot the machine into its §5.4/§5.5 starting state.
    ///
    /// Before the story wakes, the header is stamped with the
    /// frontend's honest capabilities (§11.1).
    pub fn new(
        story: Story,
        frontend: Box<dyn Frontend>,
        seed: Option<u32>,
        identity: Identity,
        saves: Option<Box<dyn SaveSlot>>,
    ) -> Result<Self, VoxamError> {
        let memory = Memory::new(&story)?;
        let variables = Variables::new(&memory);
        let objects = ObjectTable::new(&memory);
        let frontend_lines = frontend.screen_lines();
        let frontend_columns = frontend.screen_columns();
        // The ledger's numbers are units: real pixels on a
        // measuring Version 6 glass, characters everywhere else
        // (§8.4.2, §8.8.1).
        let (font_width, font_height) = if memory.header().version() == 6 {
            (frontend.font_width(), frontend.font_height())
        } else {
            (1, 1)
        };

        let mut machine = Self {
            story,
            memory,
            calls: CallStack::new(),
            variables,
            objects,
            rng: Randomizer::new(seed),
            frontend,
            identity,
            saves,
            pc: 0,
            running: true,
            screen_selected: true,
            story_window: true,
            font: NORMAL_FONT,
            recording_commands: false,
            file_input: false,
            aimed: std::collections::HashSet::new(),
            windows: WindowLedger::new(
                u16::from(frontend_lines) * font_height,
                u16::from(frontend_columns) * font_width,
                u16::from(DEFAULT_FOREGROUND_COLOUR),
                u16::from(DEFAULT_BACKGROUND_COLOUR),
                font_width,
                font_height,
            ),
            screen_buffering: 0,
            redirections: Vec::new(),
            undo: VecDeque::new(),
            waiting: None,
            wait_serial: 0,
            pending_keys: VecDeque::new(),
            typist_ready: None,
            sound_routine: 0,
            sound_since_input: false,
            passed_reserved: HashSet::new(),
        };

        machine.declare_capabilities()?;
        machine.start_execution()?;

        Ok(machine)
    }

    /// Point the machine at its first instruction (§5.4, §5.5).
    fn start_execution(&mut self) -> Result<(), VoxamError> {
        if self.memory.header().version() == 6 {
            // Version 6 instead calls the main routine (§5.4).
            let packed = self.memory.header().main_routine_packed_address()?;
            let address = routine_address(&self.memory.header(), packed)?;
            let routine = Routine::parse(&self.memory, address)?;

            self.calls.call(&routine, &[], 0, None)?;
            self.pc = routine.first_instruction;

            return Ok(());
        }

        self.pc = usize::from(self.memory.header().initial_program_counter()?);

        Ok(())
    }

    /// One character cell's width and height, in units. Version 6
    /// alone measures its screen in real pixels (§8.8.1), so only
    /// there do the frontend's font metrics become the story's
    /// units; every other version keeps one unit per character
    /// (§8.4.2): Beyond Zork lays out its windows by mixing unit
    /// arithmetic with character-cell set_cursor moves, which only
    /// agrees when the two scales are the same scale.
    fn unit_metrics(&self) -> (u16, u16) {
        if self.memory.header().version() == 6 {
            (self.frontend.font_width(), self.frontend.font_height())
        } else {
            (1, 1)
        }
    }

    /// Stamp the frontend's honest capabilities into the header
    /// (§11.1): the Rst fields, set at boot and reset after every
    /// restore and restart (§6.1.2.2).
    fn declare_capabilities(&mut self) -> Result<(), VoxamError> {
        let version = self.memory.header().version();

        declare::standard_revision(&mut self.memory, STANDARD_MAJOR, STANDARD_MINOR)?;
        declare::sound(&mut self.memory, self.frontend.has_sounds())?;

        if version == 3 {
            declare::status_line(&mut self.memory, self.frontend.has_status_line())?;
            declare::screen_splitting(&mut self.memory, self.frontend.has_screen_splitting())?;
            declare::tandy(&mut self.memory, self.identity.tandy)?;
        } else if version >= 4 {
            let platform = self.identity.interpreter.unwrap_or(INTERPRETER_PLATFORM);
            declare::interpreter(&mut self.memory, platform, INTERPRETER_REVISION)?;
            declare::screen_size(
                &mut self.memory,
                self.frontend.screen_lines(),
                self.frontend.screen_columns(),
            )?;
            declare::presentation(
                &mut self.memory,
                self.frontend.has_bold(),
                self.frontend.has_italic(),
                self.frontend.has_fixed_pitch(),
                self.frontend.has_timed_input(),
            )?;

            if version >= 5 {
                let (font_width, font_height) = self.unit_metrics();

                declare::screen_units(
                    &mut self.memory,
                    u16::from(self.frontend.screen_columns()) * font_width,
                    u16::from(self.frontend.screen_lines()) * font_height,
                )?;
                declare::font_size(&mut self.memory, font_width as u8, font_height as u8)?;
                declare::colours(
                    &mut self.memory,
                    self.frontend.has_colours(),
                    DEFAULT_FOREGROUND_COLOUR,
                    DEFAULT_BACKGROUND_COLOUR,
                )?;
                declare::mouse(&mut self.memory, self.frontend.has_mouse())?;

                if version == 6 {
                    // In Version 6, Flags 2 bit 3 asks for pictures
                    // rather than the §16 font (§11.1); the menu
                    // request falls unanswered (§11.1.2). Flags 1
                    // declares picture and sound availability
                    // outright (§11.1.4, §9.1.1).
                    declare::character_graphics(&mut self.memory, false)?;
                    declare::menus(&mut self.memory, false)?;
                    declare::pictures(&mut self.memory, self.frontend.has_pictures())?;
                    declare::sound_presence(&mut self.memory, self.frontend.has_sounds())?;
                } else {
                    declare::character_graphics(
                        &mut self.memory,
                        self.frontend.has_character_graphics(),
                    )?;
                    // The arc_image band's capability: Flags 1's
                    // picture bit, claimed in Versions 5, 7, and 8
                    // by a display that can hang the band -- and
                    // re-stamped here after restart and restore,
                    // as the contract asks (arc_image: part A).
                    declare::pictures(&mut self.memory, self.frontend.has_arc_images())?;
                }
            }
        }

        Ok(())
    }

    /// Capture the entire state of play (§6.1, §6.1.1).
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            dynamic_memory: self.memory.dynamic_snapshot(),
            pc: self.pc,
            frames: self.calls.snapshot(),
        }
    }

    /// Write a captured state of play back whole (§6.1.2).
    ///
    /// Everything is restored except 'Flags 2', whose bits belong
    /// to the player's session rather than the story's state
    /// (§6.1.2), and the Rst header fields are then re-stamped
    /// (§6.1.2.2).
    pub fn restore(&mut self, snapshot: &Snapshot) -> Result<(), VoxamError> {
        let flags2 = self.memory.read_word(FLAGS_2)?;

        self.memory.restore_dynamic(&snapshot.dynamic_memory)?;
        self.memory.write_word(FLAGS_2, flags2)?;
        self.calls.restore(&snapshot.frames)?;
        self.pc = snapshot.pc;
        self.declare_capabilities()
    }

    /// The working memory image, live as the game mutates it.
    pub fn memory_mut(&mut self) -> &mut Memory {
        &mut self.memory
    }

    /// The machine's addressable memory, for a session's own reads.
    pub fn memory(&self) -> &Memory {
        &self.memory
    }

    /// The byte address of the next instruction to execute.
    pub fn pc(&self) -> usize {
        self.pc
    }

    /// Whether execution has not yet been halted by quit.
    pub fn running(&self) -> bool {
        self.running
    }

    /// The suspended read a host must answer, if any.
    pub fn wait_serial(&self) -> u64 {
        self.wait_serial
    }

    /// What the machine stands waiting for, if anything.
    pub fn waiting(&self) -> Option<&Waiting> {
        self.waiting.as_ref()
    }

    /// Execute instructions until the story quits or a read stands
    /// waiting; the host answers through deliver_line and runs
    /// again.
    pub fn run(&mut self) -> Result<RunState, VoxamError> {
        if self.waiting.is_some() {
            return Err(instruction_error(
                "run while a read stands suspended: the input is still owed".into(),
            ));
        }

        while self.running {
            if self.step()? == Step::Suspended {
                return Ok(RunState::Waiting);
            }

            self.poll_sound()?;
        }

        Ok(RunState::Halted)
    }

    /// Fetch, decode, and execute a single instruction.
    pub fn step(&mut self) -> Result<Step, VoxamError> {
        let instruction = Instruction::decode(&self.memory, self.pc)?;

        self.execute(&instruction)
    }

    /// Complete a suspended read with the player's text.
    ///
    /// A line read takes the whole line; the terminator is zero for
    /// a plain new-line, or the §10.5.2.1 code that ended the line
    /// -- one the read's own table named. A keystroke read spends
    /// the line through the queue instead, exactly as the
    /// reference's blocking path does: an empty line is the return
    /// key alone, and a longer line queues its characters to be
    /// typed one read_char at a time.
    pub fn deliver_line(&mut self, line: &str, terminator: u16) -> Result<(), VoxamError> {
        let waiting = match self.waiting.take() {
            Some(Waiting::Read(waiting)) => waiting,
            held => {
                self.waiting = held;

                return Err(instruction_error(
                    "a line arrived with no read suspended to receive it".into(),
                ));
            }
        };

        if waiting.wants == Wants::Key {
            if line.is_empty() {
                return self.landed_key(&waiting.instruction, 13);
            }

            self.pending_keys.extend(line.chars());

            let key = self.pending_keys.pop_front().expect("a non-empty line");
            let repertoire = extras(&self.memory)?;
            let code = char_to_zscii(key, &repertoire)?;

            return self.landed_key(&waiting.instruction, code);
        }

        if terminator != 0 && !waiting.terminators.contains(&terminator) {
            self.waiting = Some(Waiting::Read(waiting));

            return Err(instruction_error(format!(
                "a line ended by code {terminator}, which the read's terminating \
                 characters table does not name (§10.5.2.1)"
            )));
        }

        self.landed_line(&waiting, line, terminator)
    }

    /// Complete a suspended keystroke read, if ZSCII can spell it.
    ///
    /// A key ZSCII has no code for is a key the story cannot hear
    /// (§3.8): the wait stands for the next one, exactly the
    /// blocking loop's shrug, and false says so.
    pub fn deliver_key(&mut self, key: char) -> Result<bool, VoxamError> {
        if !matches!(&self.waiting, Some(Waiting::Read(held)) if held.wants == Wants::Key) {
            return Err(instruction_error(
                "a key arrived with no key read suspended to receive it".into(),
            ));
        }

        let repertoire = extras(&self.memory)?;
        let Ok(code) = char_to_zscii(key, &repertoire) else {
            return Ok(false);
        };

        let Some(Waiting::Read(waiting)) = self.waiting.take() else {
            unreachable!("checked above");
        };

        self.landed_key(&waiting.instruction, code)?;

        Ok(true)
    }

    /// Complete a suspended read with a mouse click, if it hears
    /// one.
    ///
    /// §10.3.3: a click is the keyboard code 254, so a keystroke
    /// read takes it the way it takes any key; a line read ends
    /// early only when its §10.5.2.1 table names the click code,
    /// taking the text composed so far as the typed line. Either
    /// way the click's position lands in header extension words 1
    /// and 2 first (§10.3.2), at (1,1) in the top corner. A read
    /// that cannot hear a click leaves the wait standing, and
    /// false says so -- the blocking editor's shrug, suspended.
    pub fn deliver_click(&mut self, x: u16, y: u16, text: &str) -> Result<bool, VoxamError> {
        let Some(Waiting::Read(held)) = &self.waiting else {
            return Err(instruction_error(
                "a click arrived with no read suspended to receive it".into(),
            ));
        };

        if held.wants == Wants::Line && !held.terminators.contains(&SINGLE_CLICK) {
            return Ok(false);
        }

        self.note_click(x, y)?;

        let Some(Waiting::Read(waiting)) = self.waiting.take() else {
            unreachable!("checked above");
        };

        if waiting.wants == Wants::Key {
            self.landed_key(&waiting.instruction, SINGLE_CLICK)?;

            return Ok(true);
        }

        self.landed_line(&waiting, text, SINGLE_CLICK)?;

        Ok(true)
    }

    /// Complete a suspended save or restore with the player's path.
    ///
    /// The prompt's name is the player's own: an absolute path is
    /// honored whole, a relative one lands where the session runs,
    /// and a bare one gains the .sav suffix as a courtesy. A
    /// cancel -- None -- answers the opcode's own §15 failure,
    /// which every game already speaks; a restore that succeeds
    /// does not continue here at all, resuming at its save's rider
    /// (§15 save, Quetzal §5.8).
    pub fn deliver_file(&mut self, name: Option<&str>) -> Result<(), VoxamError> {
        let filing = match self.waiting.take() {
            Some(Waiting::File(filing)) => filing,
            held => {
                self.waiting = held;

                return Err(instruction_error(
                    "a file arrived with no save or restore suspended to receive it".into(),
                ));
            }
        };

        let path = name.filter(|told| !told.is_empty()).map(|told| {
            let path = std::path::PathBuf::from(told);

            if path.extension().is_none() {
                path.with_extension("sav")
            } else {
                path
            }
        });

        if filing.purpose == Purpose::Save {
            let success = match (path, &filing.data) {
                (Some(path), Some(data)) => crate::saves::FileSaveSlot { path }.write(data),
                _ => false,
            };

            return self.save_rider(&filing.instruction, success);
        }

        let data = path.and_then(|path| crate::saves::FileSaveSlot { path }.read());
        let snapshot =
            data.and_then(|bytes| crate::zmachine::quetzal::read(&bytes, &self.story).ok());

        let Some(snapshot) = snapshot else {
            if self.memory.header().version() > BRANCHING_SAVE_FINAL_VERSION {
                self.store_result(filing.instruction.store_variable, FALSE_VALUE)?;
            }

            self.pc = filing.instruction.next_address;

            return Ok(());
        };

        self.restore(&snapshot)?;
        self.resume_from_save(snapshot.pc)
    }

    /// Fire a timed read's §15 interrupt routine, mid-wait.
    ///
    /// The nested step loop returns to the pc it left, so the read
    /// still stands unless the routine asked otherwise: a true
    /// return -- a quit along the way included -- erases the input
    /// and ends the read the interrupt way.
    pub fn deliver_tick(&mut self) -> Result<(), VoxamError> {
        let routine = match &self.waiting {
            Some(Waiting::Read(held)) if held.routine != 0 => held.routine,
            _ => {
                return Err(instruction_error(
                    "a tick arrived with no timed read suspended to hear it".into(),
                ));
            }
        };

        if self.interrupt(routine)? != 0 {
            let Some(Waiting::Read(waiting)) = self.waiting.take() else {
                unreachable!("checked above");
            };

            if waiting.wants == Wants::Line {
                self.abandoned_line(
                    &waiting.instruction,
                    waiting.text_buffer,
                    waiting.parse_buffer,
                    waiting.counted,
                )?;
            } else {
                self.abandoned_key(&waiting.instruction)?;
            }
        }

        Ok(())
    }

    /// Record a click's position in header extension words 1 and 2
    /// -- only when the story provides an extension that large
    /// (§10.3.2): a story without one still hears the click, it
    /// just cannot ask where.
    fn note_click(&mut self, x: u16, y: u16) -> Result<(), VoxamError> {
        let extension = usize::from(self.memory.read_word(HEADER_EXTENSION_WORD)?);

        if extension != 0 && self.memory.read_word(extension)? >= EXTENSION_CLICK_WORDS {
            self.memory.write_word(extension + 2, x)?;
            self.memory.write_word(extension + 4, y)?;
        }

        Ok(())
    }

    /// Resolve an operand to a value, reading variables (§4.2.2).
    fn value(&mut self, operand: &Operand) -> Result<u16, VoxamError> {
        if operand.kind == OperandType::Variable {
            return self
                .variables
                .read(&self.memory, &mut self.calls, operand.value as u8);
        }

        Ok(operand.value)
    }

    /// Resolve every operand, first to last (§4.5.2).
    fn values(&mut self, instruction: &Instruction) -> Result<Vec<u16>, VoxamError> {
        instruction
            .operands
            .iter()
            .map(|operand| self.value(operand))
            .collect()
    }

    /// Leave the current routine, delivering its result (§6.4.5).
    fn ret(&mut self, value: u16) -> Result<(), VoxamError> {
        let direction = self.calls.pop_frame()?;
        self.pc = direction.return_address;

        if let Some(variable) = direction.store_variable {
            self.variables
                .write(&mut self.memory, &mut self.calls, variable, value)?;
        }

        Ok(())
    }

    /// Deliver an instruction's result, discarding one with no home.
    fn store_result(&mut self, variable: Option<u8>, value: u16) -> Result<(), VoxamError> {
        if let Some(variable) = variable {
            self.variables
                .write(&mut self.memory, &mut self.calls, variable, value)?;
        }

        Ok(())
    }

    /// Send story text down the selected output streams (§7).
    ///
    /// While stream 3 is selected, text goes into the newest memory
    /// table and nowhere else (§7.1.2.2). The screen boundary is
    /// where surrogate halves fuse into their astral characters:
    /// stream 3 keeps the raw 16-bit units a game may read back.
    fn print_units(&mut self, units: &[u16]) -> Result<(), VoxamError> {
        if let Some((_, buffer, _)) = self.redirections.last_mut() {
            buffer.extend_from_slice(units);

            return Ok(());
        }

        if self.story_window && self.transcripting()? {
            // The transcript file is the scribe's, and this session
            // carries none yet: the frontier reports itself.
            return Err(unimplemented("output stream 2", self.pc));
        }

        if self.screen_selected {
            self.frontend.write(&units_to_string(units));
        }

        Ok(())
    }

    fn print_str(&mut self, text: &str) -> Result<(), VoxamError> {
        let units: Units = text.chars().map(|ch| ch as u16).collect();

        self.print_units(&units)
    }

    /// Whether stream 2 is on: 'Flags 2' bit 0, the §7.4 truth.
    fn transcripting(&self) -> Result<bool, VoxamError> {
        Ok(self.memory.read_word(FLAGS_2)? & TRANSCRIPT_BIT != 0)
    }

    /// Dispatch a decoded instruction to its handler.
    fn execute(&mut self, instruction: &Instruction) -> Result<Step, VoxamError> {
        let ran = |result: Result<(), VoxamError>| result.map(|()| Step::Ran);

        match instruction.opcode.name {
            "add" => ran(self.binary(instruction, |left, right| left + right)),
            "sub" => ran(self.binary(instruction, |left, right| left - right)),
            "mul" => ran(self.binary(instruction, |left, right| left * right)),
            "div" => ran(self.divide(instruction, |left, right| left / right)),
            "mod" => ran(self.divide(instruction, |left, right| left % right)),
            "and" => ran(self.bitwise(instruction, |left, right| left & right)),
            "or" => ran(self.bitwise(instruction, |left, right| left | right)),
            "not" => ran(self.op_not(instruction)),
            "log_shift" => ran(self.shift(instruction, false)),
            "art_shift" => ran(self.shift(instruction, true)),
            "je" => ran(self.op_je(instruction)),
            "jl" => ran(self.compare(instruction, |left, right| left < right)),
            "jg" => ran(self.compare(instruction, |left, right| left > right)),
            "jz" => ran(self.op_jz(instruction)),
            "test" => ran(self.op_test(instruction)),
            "jin" => ran(self.op_jin(instruction)),
            "jump" => ran(self.op_jump(instruction)),
            "inc" => ran(self.step_variable(instruction, 1)),
            "dec" => ran(self.step_variable(instruction, -1)),
            "inc_chk" => ran(self.check_step(instruction, 1)),
            "dec_chk" => ran(self.check_step(instruction, -1)),
            "load" => ran(self.op_load(instruction)),
            "store" => ran(self.op_store(instruction)),
            "loadw" => ran(self.op_loadw(instruction)),
            "loadb" => ran(self.op_loadb(instruction)),
            "storew" => ran(self.op_storew(instruction)),
            "storeb" => ran(self.op_storeb(instruction)),
            "push" => ran(self.op_push(instruction)),
            "pull" => ran(self.op_pull(instruction)),
            "pop" => ran(self.op_pop(instruction)),
            "ret_popped" => ran(self.op_ret_popped()),
            "catch" => ran(self.op_catch(instruction)),
            "throw" => ran(self.op_throw(instruction)),
            "call" | "call_vs" | "call_vs2" | "call_1s" | "call_2s" | "call_1n" | "call_2n"
            | "call_vn" | "call_vn2" => ran(self.op_call(instruction)),
            "ret" => ran(self.op_ret(instruction)),
            "rtrue" => ran(self.ret(TRUE_VALUE)),
            "rfalse" => ran(self.ret(FALSE_VALUE)),
            "check_arg_count" => ran(self.op_check_arg_count(instruction)),
            "get_parent" => ran(self.op_get_parent(instruction)),
            "get_sibling" => ran(self.op_get_sibling(instruction)),
            "get_child" => ran(self.op_get_child(instruction)),
            "test_attr" => ran(self.op_test_attr(instruction)),
            "set_attr" => ran(self.op_change_attr(instruction, true)),
            "clear_attr" => ran(self.op_change_attr(instruction, false)),
            "insert_obj" => ran(self.op_insert_obj(instruction)),
            "remove_obj" => ran(self.op_remove_obj(instruction)),
            "print_obj" => ran(self.op_print_obj(instruction)),
            "put_prop" => ran(self.op_put_prop(instruction)),
            "get_prop" => ran(self.op_get_prop(instruction)),
            "get_prop_addr" => ran(self.op_get_prop_addr(instruction)),
            "get_prop_len" => ran(self.op_get_prop_len(instruction)),
            "get_next_prop" => ran(self.op_get_next_prop(instruction)),
            "print" => ran(self.op_print(instruction)),
            "print_ret" => ran(self.op_print_ret(instruction)),
            "print_addr" => ran(self.op_print_addr(instruction)),
            "print_paddr" => ran(self.op_print_paddr(instruction)),
            "print_char" => ran(self.op_print_char(instruction)),
            "print_num" => ran(self.op_print_num(instruction)),
            "new_line" => ran(self.op_new_line(instruction)),
            "random" => ran(self.op_random(instruction)),
            "verify" => ran(self.op_verify(instruction)),
            "piracy" => ran(self.branch(instruction, true)),
            "show_status" => ran(self.op_show_status(instruction)),
            "sound_effect" => ran(self.op_sound_effect(instruction)),
            "output_stream" => ran(self.op_output_stream(instruction)),
            "input_stream" => ran(self.op_input_stream(instruction)),
            "scan_table" => ran(self.op_scan_table(instruction)),
            "save" => self.op_save(instruction),
            "restore" => self.op_restore(instruction),
            "save_undo" => ran(self.op_save_undo(instruction)),
            "restore_undo" => ran(self.op_restore_undo(instruction)),
            "restart" => ran(self.op_restart()),
            "set_text_style" => ran(self.op_set_text_style(instruction)),
            "set_font" => ran(self.op_set_font(instruction)),
            "erase_window" => ran(self.op_erase_window(instruction)),
            "erase_line" => ran(self.op_erase_line(instruction)),
            "buffer_mode" => ran(self.op_buffer_mode(instruction)),
            "split_window" => ran(self.op_split_window(instruction)),
            "set_window" => ran(self.op_set_window(instruction)),
            "set_cursor" => ran(self.op_set_cursor(instruction)),
            "get_cursor" => ran(self.op_get_cursor(instruction)),
            "print_table" => ran(self.op_print_table(instruction)),
            "copy_table" => ran(self.op_copy_table(instruction)),
            "tokenise" => ran(self.op_tokenise(instruction)),
            "encode_text" => ran(self.op_encode_text(instruction)),
            "move_window" => ran(self.op_move_window(instruction)),
            "window_size" => ran(self.op_window_size(instruction)),
            "window_style" => ran(self.op_window_style(instruction)),
            "get_wind_prop" => ran(self.op_get_wind_prop(instruction)),
            "put_wind_prop" => ran(self.op_put_wind_prop(instruction)),
            "set_margins" => ran(self.op_set_margins(instruction)),
            "scroll_window" => ran(self.op_scroll_window(instruction)),
            "read_mouse" => ran(self.op_read_mouse(instruction)),
            "make_menu" => ran(self.op_make_menu(instruction)),
            "print_form" => ran(self.op_print_form(instruction)),
            "picture_data" => ran(self.op_picture_data(instruction)),
            "buffer_screen" => ran(self.op_buffer_screen(instruction)),
            "push_stack" => ran(self.op_push_stack(instruction)),
            "pop_stack" => ran(self.op_pop_stack(instruction)),
            "draw_picture" => ran(self.op_draw_picture(instruction, true)),
            "erase_picture" => ran(self.op_draw_picture(instruction, false)),
            "draw_image" => ran(self.op_draw_image(instruction)),
            "picture_table" | "mouse_window" => {
                // Presentation a frontend without pictures or a
                // mouse lets pass in the conforming quiet; the
                // operands were already spelled by their own types.
                self.values(instruction)?;
                self.pc = instruction.next_address;
                Ok(Step::Ran)
            }
            "set_colour" => ran(self.op_set_colour(instruction)),
            "set_true_colour" => {
                // A frontend that truthfully declared no colours
                // makes the request a legitimate no-op (§8.3.1).
                self.pc = instruction.next_address;
                Ok(Step::Ran)
            }
            "nop" => {
                self.pc = instruction.next_address;
                Ok(Step::Ran)
            }
            "ext_private" => {
                self.pc = instruction.next_address;
                Ok(Step::Ran)
            }
            "ext_reserved" => ran(self.op_ext_reserved(instruction)),
            "sread" | "aread" => self.op_sread(instruction),
            "read_char" => self.op_read_char(instruction),
            "quit" => {
                self.running = false;
                Ok(Step::Ran)
            }
            name => Err(unimplemented(name, instruction.address)),
        }
    }

    /// Run a signed two-operand operation, wrapping the result
    /// (§2.2).
    fn binary(
        &mut self,
        instruction: &Instruction,
        operation: fn(i64, i64) -> i64,
    ) -> Result<(), VoxamError> {
        let left = i64::from(signed(self.value(&instruction.operands[0])?));
        let right = i64::from(signed(self.value(&instruction.operands[1])?));

        self.store_result(
            instruction.store_variable,
            (operation(left, right) & WORD_MASK) as u16,
        )?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Run a signed division-family operation, policing §2.3.1.
    /// Operands resolve first-to-last before the divisor is
    /// examined (§4.5.2). Rust's `/` and `%` truncate toward zero
    /// with the dividend's sign, exactly the §2.2.1 rules.
    fn divide(
        &mut self,
        instruction: &Instruction,
        operation: fn(i64, i64) -> i64,
    ) -> Result<(), VoxamError> {
        let left = i64::from(signed(self.value(&instruction.operands[0])?));
        let right = i64::from(signed(self.value(&instruction.operands[1])?));

        if right == 0 {
            return Err(VoxamError::ZMachineArithmetic(format!(
                "division by zero at ${:04x} (§2.3.1)",
                instruction.address
            )));
        }

        self.store_result(
            instruction.store_variable,
            (operation(left, right) & WORD_MASK) as u16,
        )?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn bitwise(
        &mut self,
        instruction: &Instruction,
        operation: fn(u16, u16) -> u16,
    ) -> Result<(), VoxamError> {
        let left = self.value(&instruction.operands[0])?;
        let right = self.value(&instruction.operands[1])?;

        self.store_result(instruction.store_variable, operation(left, right))?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_not(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let value = self.value(&instruction.operands[0])?;

        self.store_result(instruction.store_variable, !value)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Shift left for positive places, right for negative (§15);
    /// log_shift zeroes the sign in, art_shift preserves it. Places
    /// past 16 settle every outcome, so the distance clamps there.
    fn shift(&mut self, instruction: &Instruction, arithmetic: bool) -> Result<(), VoxamError> {
        let number = self.value(&instruction.operands[0])?;
        let places = signed(self.value(&instruction.operands[1])?);
        let distance = places.unsigned_abs().min(16);

        let result = if places >= 0 {
            ((u32::from(number) << distance) & 0xFFFF) as u16
        } else if arithmetic {
            ((i64::from(signed(number)) >> distance) & WORD_MASK) as u16
        } else {
            (u32::from(number) >> distance) as u16
        };

        self.store_result(instruction.store_variable, result)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Act on a branch rider after a test (§4.7).
    fn branch(&mut self, instruction: &Instruction, condition: bool) -> Result<(), VoxamError> {
        match instruction.branch {
            Some(branch) if condition == branch.on_true => {
                self.take_branch(&branch, instruction.next_address)
            }
            _ => {
                self.pc = instruction.next_address;

                Ok(())
            }
        }
    }

    /// Apply a decoded branch to a tested condition (§4.7): the
    /// rider-at-hand twin of branch, for resuming a restore at a
    /// save's branch data.
    fn apply_branch(
        &mut self,
        branch: &Branch,
        after: usize,
        condition: bool,
    ) -> Result<(), VoxamError> {
        if condition != branch.on_true {
            self.pc = after;

            Ok(())
        } else {
            self.take_branch(branch, after)
        }
    }

    /// Follow a branch that applies: jump, or return (§4.7.1).
    fn take_branch(&mut self, branch: &Branch, after: usize) -> Result<(), VoxamError> {
        if branch.returns_false() {
            self.ret(FALSE_VALUE)
        } else if branch.returns_true() {
            self.ret(TRUE_VALUE)
        } else {
            self.pc = branch.target(after)?;

            Ok(())
        }
    }

    /// Branch if the first operand equals any of the others (§15).
    fn op_je(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;

        if values.len() < JE_MINIMUM_OPERANDS {
            return Err(instruction_error(format!(
                "je at ${:04x} has {} operand(s), but needs at least two (§15)",
                instruction.address,
                values.len()
            )));
        }

        self.branch(instruction, values[1..].contains(&values[0]))
    }

    fn compare(
        &mut self,
        instruction: &Instruction,
        comparison: fn(i32, i32) -> bool,
    ) -> Result<(), VoxamError> {
        let left = signed(self.value(&instruction.operands[0])?);
        let right = signed(self.value(&instruction.operands[1])?);

        self.branch(instruction, comparison(left, right))
    }

    fn op_jz(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let value = self.value(&instruction.operands[0])?;

        self.branch(instruction, value == 0)
    }

    /// Branch if every flag in the bitmap is set (§15).
    fn op_test(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let bitmap = self.value(&instruction.operands[0])?;
        let flags = self.value(&instruction.operands[1])?;

        self.branch(instruction, bitmap & flags == flags)
    }

    /// Branch if the first object's parent is the second (§15);
    /// nothing's parent is nothing.
    fn op_jin(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let obj = self.value(&instruction.operands[0])?;
        let parent = self.value(&instruction.operands[1])?;
        let parent_of = if obj != 0 {
            self.objects.parent(&self.memory, obj)?
        } else {
            0
        };

        self.branch(instruction, parent_of == parent)
    }

    /// Move execution unconditionally by a signed offset (§15);
    /// branch arithmetic, ordinary operand.
    fn op_jump(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let offset = signed(self.value(&instruction.operands[0])?);
        let target = instruction.next_address as i64 + i64::from(offset) - BRANCH_TARGET_ADJUSTMENT;

        self.pc = usize::try_from(target).map_err(|_| {
            instruction_error(format!(
                "jump at ${:04x} lands before the story begins (§4.7.2)",
                instruction.address
            ))
        })?;

        Ok(())
    }

    /// Add a signed delta to the referenced variable (§15, §6.3.4).
    fn step_variable(&mut self, instruction: &Instruction, delta: i32) -> Result<(), VoxamError> {
        let reference = self.value(&instruction.operands[0])? as u8;
        let value = signed(self.variables.read_in_place(
            &self.memory,
            &mut self.calls,
            reference,
        )?);

        self.variables.write_in_place(
            &mut self.memory,
            &mut self.calls,
            reference,
            ((i64::from(value) + i64::from(delta)) & WORD_MASK) as u16,
        )?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Step a referenced variable and compare it (§15, §6.3.4):
    /// branch above the comparison after incrementing, below it
    /// after decrementing.
    fn check_step(&mut self, instruction: &Instruction, delta: i32) -> Result<(), VoxamError> {
        let reference = self.value(&instruction.operands[0])? as u8;
        let comparison = signed(self.value(&instruction.operands[1])?);
        let stepped = ((i64::from(signed(self.variables.read_in_place(
            &self.memory,
            &mut self.calls,
            reference,
        )?)) + i64::from(delta))
            & WORD_MASK) as u16;

        self.variables
            .write_in_place(&mut self.memory, &mut self.calls, reference, stepped)?;

        let passed = if delta > 0 {
            signed(stepped) > comparison
        } else {
            signed(stepped) < comparison
        };

        self.branch(instruction, passed)
    }

    /// Store the referenced variable's value (§15, §6.3.4).
    fn op_load(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let reference = self.value(&instruction.operands[0])? as u8;
        let value = self
            .variables
            .read_in_place(&self.memory, &mut self.calls, reference)?;

        self.store_result(instruction.store_variable, value)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Write a value into the referenced variable (§15, §6.3.4).
    fn op_store(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let reference = self.value(&instruction.operands[0])? as u8;
        let value = self.value(&instruction.operands[1])?;

        self.variables
            .write_in_place(&mut self.memory, &mut self.calls, reference, value)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// The address of a table entry, on a 16-bit bus (§15 loadw):
    /// the index is signed and the sum wraps to a 16-bit address.
    fn table_address(array: u16, index: u16, scale: i64) -> usize {
        ((i64::from(array) + scale * i64::from(signed(index))) & WORD_MASK) as usize
    }

    fn op_loadw(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let array = self.value(&instruction.operands[0])?;
        let index = self.value(&instruction.operands[1])?;
        let value = self
            .memory
            .read_word(Self::table_address(array, index, 2))?;

        self.store_result(instruction.store_variable, value)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_loadb(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let array = self.value(&instruction.operands[0])?;
        let index = self.value(&instruction.operands[1])?;
        let value = self
            .memory
            .read_byte(Self::table_address(array, index, 1))?;

        self.store_result(instruction.store_variable, u16::from(value))?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_storew(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let array = self.value(&instruction.operands[0])?;
        let index = self.value(&instruction.operands[1])?;
        let value = self.value(&instruction.operands[2])?;

        self.memory
            .write_word(Self::table_address(array, index, 2), value)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Write a byte at array + byte-index (§15); a large operand
    /// lands its least significant byte, the settlement Sherlock
    /// earned.
    fn op_storeb(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let array = self.value(&instruction.operands[0])?;
        let index = self.value(&instruction.operands[1])?;
        let value = self.value(&instruction.operands[2])?;

        self.memory
            .write_byte(Self::table_address(array, index, 1), (value & 0xFF) as u8)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_push(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let value = self.value(&instruction.operands[0])?;

        self.calls.push(value)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Pull the stack into a referenced variable (§15, §6.3.4); the
    /// Version 6 user-stack form waits with the rest of Version 6.
    fn op_pull(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        if instruction.opcode.stores {
            // Version 6 turns the opcode around: it stores its
            // result, and an operand names a §6.6 user stack to
            // pull from instead of the game stack.
            let value = if instruction.operands.is_empty() {
                self.calls.pop()?
            } else {
                let stack = usize::from(self.value(&instruction.operands[0])?);
                let spare = self.memory.read_word(stack)? + 1;

                self.memory.write_word(stack, spare)?;
                self.memory.read_word(stack + 2 * usize::from(spare))?
            };

            self.store_result(instruction.store_variable, value)?;
            self.pc = instruction.next_address;

            return Ok(());
        }

        let reference = self.value(&instruction.operands[0])? as u8;
        let value = self.calls.pop()?;

        self.variables
            .write_in_place(&mut self.memory, &mut self.calls, reference, value)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_pop(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        self.calls.pop()?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_ret_popped(&mut self) -> Result<(), VoxamError> {
        let value = self.calls.pop()?;

        self.ret(value)
    }

    /// Store the magic cookie naming this stack frame: the number
    /// of frames on the call stack (§15 catch, Quetzal §6.2).
    fn op_catch(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        self.store_result(instruction.store_variable, self.calls.depth() as u16)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Unwind to a caught frame and return from it (§15 throw).
    fn op_throw(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let value = self.value(&instruction.operands[0])?;
        let frame = usize::from(self.value(&instruction.operands[1])?);

        if frame > self.calls.depth() || frame < 1 {
            return Err(VoxamError::ZMachineStack(format!(
                "cannot throw to stack frame {frame}: the call stack is {} deep, so \
                 that catch has already returned (§15 throw)",
                self.calls.depth()
            )));
        }

        while self.calls.depth() > frame {
            self.calls.pop_frame()?;
        }

        self.ret(value)
    }

    /// Call a routine, or return false for address 0 (§6.4).
    fn op_call(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;
        let packed = values[0];

        if packed == NULL_ROUTINE {
            self.store_result(instruction.store_variable, FALSE_VALUE)?;
            self.pc = instruction.next_address;

            return Ok(());
        }

        let address = routine_address(&self.memory.header(), packed)?;
        let routine = Routine::parse(&self.memory, address)?;

        self.calls.call(
            &routine,
            &values[1..],
            instruction.next_address,
            instruction.store_variable,
        )?;

        self.pc = routine.first_instruction;

        Ok(())
    }

    fn op_ret(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let value = self.value(&instruction.operands[0])?;

        self.ret(value)
    }

    /// Branch if the numbered argument was supplied (§6.4.4.1).
    fn op_check_arg_count(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let wanted = self.value(&instruction.operands[0])?;

        self.branch(
            instruction,
            usize::from(wanted) <= self.calls.argument_count(),
        )
    }

    // Object 0 means "nothing" (§12.3), and the object opcodes
    // answer questions about it in kind, exactly as the reference
    // settles them: reads answer nothing, quiet writes change
    // nothing; print_obj 0 and put_prop 0 remain unearned and loud.

    fn op_get_parent(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let obj = self.value(&instruction.operands[0])?;
        let parent = if obj != 0 {
            self.objects.parent(&self.memory, obj)?
        } else {
            0
        };

        self.store_result(instruction.store_variable, parent)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_get_sibling(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let obj = self.value(&instruction.operands[0])?;
        let sibling = if obj != 0 {
            self.objects.sibling(&self.memory, obj)?
        } else {
            0
        };

        self.store_result(instruction.store_variable, sibling)?;
        self.branch(instruction, sibling != 0)
    }

    fn op_get_child(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let obj = self.value(&instruction.operands[0])?;
        let child = if obj != 0 {
            self.objects.child(&self.memory, obj)?
        } else {
            0
        };

        self.store_result(instruction.store_variable, child)?;
        self.branch(instruction, child != 0)
    }

    fn op_test_attr(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let obj = self.value(&instruction.operands[0])?;
        let attribute = self.value(&instruction.operands[1])?;
        let held = obj != 0 && self.objects.attribute(&self.memory, obj, attribute)?;

        self.branch(instruction, held)
    }

    /// Set or clear the object's attribute (§15); out-of-range
    /// attributes change nothing, the settlement Sherlock's melting
    /// wax head earned.
    fn op_change_attr(&mut self, instruction: &Instruction, on: bool) -> Result<(), VoxamError> {
        let obj = self.value(&instruction.operands[0])?;
        let attribute = self.value(&instruction.operands[1])?;

        if obj != 0 && self.objects.attribute_exists(attribute) {
            self.objects
                .set_attribute(&mut self.memory, obj, attribute, on)?;
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_insert_obj(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let obj = self.value(&instruction.operands[0])?;
        let destination = self.value(&instruction.operands[1])?;

        if obj != 0 && destination != 0 {
            self.objects.insert(&mut self.memory, obj, destination)?;
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_remove_obj(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let obj = self.value(&instruction.operands[0])?;

        if obj != 0 {
            self.objects.remove(&mut self.memory, obj)?;
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_print_obj(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let obj = self.value(&instruction.operands[0])?;
        let address = self.objects.short_name_address(&self.memory, obj)?;
        let (units, _) = decode_units(&self.memory, address)?;

        self.print_units(&units)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_put_prop(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let obj = self.value(&instruction.operands[0])?;
        let number = self.value(&instruction.operands[1])?;
        let value = self.value(&instruction.operands[2])?;

        self.objects
            .put_property(&mut self.memory, obj, number, value)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_get_prop(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let obj = self.value(&instruction.operands[0])?;
        let number = self.value(&instruction.operands[1])?;
        let value = if obj != 0 {
            self.objects.property_value(&self.memory, obj, number)?
        } else {
            0
        };

        self.store_result(instruction.store_variable, value)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_get_prop_addr(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let obj = self.value(&instruction.operands[0])?;
        let number = self.value(&instruction.operands[1])?;
        let found = if obj != 0 {
            self.objects.find_property(&self.memory, obj, number)?
        } else {
            None
        };

        self.store_result(
            instruction.store_variable,
            found.map_or(0, |(data, _)| data as u16),
        )?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Store a property's length from its data address (§15);
    /// address 0 must give 0.
    fn op_get_prop_len(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let address = self.value(&instruction.operands[0])?;
        let length = if address == 0 {
            0
        } else {
            self.objects
                .property_length_at(&self.memory, usize::from(address))? as u16
        };

        self.store_result(instruction.store_variable, length)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_get_next_prop(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let obj = self.value(&instruction.operands[0])?;
        let number = self.value(&instruction.operands[1])?;
        let found = if obj != 0 {
            self.objects.next_property(&self.memory, obj, number)?
        } else {
            0
        };

        self.store_result(instruction.store_variable, found)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_print(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let (units, _) = decode_units(&self.memory, instruction.operands_end)?;

        self.print_units(&units)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_print_ret(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let (mut units, _) = decode_units(&self.memory, instruction.operands_end)?;
        units.push(u16::from(b'\n'));

        self.print_units(&units)?;
        self.ret(TRUE_VALUE)
    }

    fn op_print_addr(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let address = self.value(&instruction.operands[0])?;
        let (units, _) = decode_units(&self.memory, usize::from(address))?;

        self.print_units(&units)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_print_paddr(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let packed = self.value(&instruction.operands[0])?;
        let address = string_address(&self.memory.header(), packed)?;
        let (units, _) = decode_units(&self.memory, address)?;

        self.print_units(&units)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_print_char(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let code = self.value(&instruction.operands[0])?;
        let version = self.memory.header().version();
        let repertoire = extras(&self.memory)?;
        let units = zscii_to_units(code, &repertoire, version)?;

        self.print_units(&units)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_print_num(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let value = signed(self.value(&instruction.operands[0])?);

        self.print_str(&value.to_string())?;
        self.pc = instruction.next_address;

        Ok(())
    }

    fn op_new_line(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        self.print_units(&[u16::from(b'\n')])?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Roll, seed, or re-randomize the generator (§2.4, §15).
    fn op_random(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let value = signed(self.value(&instruction.operands[0])?);

        let result = if value > 0 {
            self.rng.roll(value as u32) as u16
        } else if value < 0 {
            self.rng.seed(value.unsigned_abs());

            0
        } else {
            self.rng.randomize();

            0
        };

        self.store_result(instruction.store_variable, result)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Branch if the pristine story's checksum is correct (§15):
    /// verification reads the file as shipped, not the mutated
    /// working image.
    fn op_verify(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let verified = self.story.header().verify();

        self.branch(instruction, verified)
    }

    /// Assemble what the status line shows (§8.2).
    fn status(&mut self) -> Result<Status, VoxamError> {
        let location = self.variables.read(&self.memory, &mut self.calls, 0x10)?;
        let address = self.objects.short_name_address(&self.memory, location)?;
        let (units, _) = decode_units(&self.memory, address)?;
        let score = self.variables.read(&self.memory, &mut self.calls, 0x11)?;
        let turns = self.variables.read(&self.memory, &mut self.calls, 0x12)?;

        Ok(Status {
            location: units_to_string(&units),
            score: signed(score),
            turns,
            time_game: self.memory.header().time_game()?,
        })
    }

    fn op_show_status(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        if self.frontend.has_status_line() {
            let status = self.status()?;
            self.frontend.show_status(&status);
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Sound a bleep, or let a sampled request pass in the
    /// conforming silence of a frontend that declared no sounds
    /// (§9).
    fn op_sound_effect(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;
        let number = values.first().copied().unwrap_or(1);

        if (1..FIRST_SAMPLED_SOUND).contains(&number) {
            self.frontend.bleep(number == 1);
        } else if self.frontend.has_sounds() {
            self.sampled_sound(number, &values)?;
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Drive one sampled-sound effect through the frontend (§9.4).
    ///
    /// The §9 remarks name The Lurking Horror's own bugs, and
    /// those are the pardons here: an effect outside the four
    /// passes quietly, a number no resource answers starts nothing
    /// and keeps any current sound playing, and an oversized
    /// volume clamps to §9.3's 8. The remarks also set the pacing:
    /// a new sound, started while one begun since the last
    /// keyboard input still plays, waits for that one to finish a
    /// cycle first.
    fn sampled_sound(&mut self, number: u16, values: &[u16]) -> Result<(), VoxamError> {
        let effect = values.get(1).copied().unwrap_or(START_EFFECT);

        if effect == STOP_EFFECT || effect == FINISH_EFFECT {
            self.frontend
                .stop_sound(if number != 0 { Some(number) } else { None });

            return Ok(());
        }

        if effect != START_EFFECT {
            // Prepare asks for nothing here -- every sound is
            // already decoded (§9.4.1) -- and any other effect is
            // the pardoned bug above.
            return Ok(());
        }

        let word = values.get(2).copied().unwrap_or(LOUDEST_VOLUME);
        let volume = if word & 0xFF == LOUDEST_VOLUME {
            // 255 is "loudest possible" (§9.3, §15 sound_effect).
            FULL_VOLUME
        } else {
            (word & 0xFF).clamp(LOWEST_VOLUME, FULL_VOLUME)
        };

        let repeats = if self.memory.header().version() >= SOUND_REPEATS_VERSION {
            let high = word >> 8;

            // 255 repeats until stopped; zero is illegal here, and
            // §15's own suggestion is to read it as once.
            Some(if high == FOREVER_BYTE {
                FOREVER_REPEATS
            } else {
                high.max(1)
            })
        } else {
            // Version 3 cannot say -- §15 keeps the high byte 0 --
            // so None lets the Blorb's Loop chunk decide.
            None
        };

        if self.sound_since_input && self.frontend.sound_playing() {
            self.frontend.wait_for_sound();
        }

        let routine = values.get(3).copied().unwrap_or(0);

        if self.frontend.play_sound(number, volume, repeats) {
            self.sound_routine = routine;
            self.sound_since_input = true;
        }

        Ok(())
    }

    /// Fire the end-of-sound routine of a sound that just ended.
    ///
    /// The routine runs only after the sound has played its
    /// requested number of times -- the frontend's finished
    /// answers exactly that, once -- and never for a sound stopped
    /// or replaced (§9.4.4). The result is discarded: an
    /// end-of-sound routine is not a §15 read interrupt, and
    /// terminates nothing.
    pub fn poll_sound(&mut self) -> Result<(), VoxamError> {
        if self.sound_routine != 0 && self.frontend.sound_finished() {
            let routine = std::mem::take(&mut self.sound_routine);

            self.interrupt(routine)?;
        }

        Ok(())
    }

    /// Select or deselect an output stream (§7, §15).
    fn op_output_stream(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;
        let stream = signed(values[0]);

        if stream == 0 {
            // Selecting nothing does nothing.
        } else if stream == SCREEN_STREAM {
            self.screen_selected = true;
        } else if stream == -SCREEN_STREAM {
            self.screen_selected = false;
        } else if stream == MEMORY_STREAM {
            self.redirect_into(instruction, &values)?;
        } else if stream == -MEMORY_STREAM {
            self.end_redirection(instruction)?;
        } else if stream.abs() == TRANSCRIPT_STREAM {
            self.transcript_switch(instruction, stream > 0)?;
        } else if stream.abs() == COMMANDS_STREAM {
            if stream > 0 {
                return Err(unimplemented("output stream 4", instruction.address));
            }

            self.recording_commands = false;
        } else {
            return Err(instruction_error(format!(
                "output_stream at ${:04x} names stream {stream}, but §7.1 defines \
                 only 1 to 4",
                instruction.address
            )));
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Work stream 2 by setting or clearing the §7.4 flag; the
    /// session carries no transcript file yet, so selecting the
    /// stream reports the frontier.
    fn transcript_switch(&mut self, instruction: &Instruction, on: bool) -> Result<(), VoxamError> {
        if on {
            return Err(unimplemented("output stream 2", instruction.address));
        }

        let flags = self.memory.read_word(FLAGS_2)?;

        self.memory.write_word(FLAGS_2, flags & !TRANSCRIPT_BIT)
    }

    fn op_input_stream(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let stream = self.value(&instruction.operands[0])?;

        if stream == FILE_INPUT_STREAM {
            return Err(unimplemented("input stream 1", instruction.address));
        }

        if stream != KEYBOARD_INPUT_STREAM {
            return Err(instruction_error(format!(
                "input_stream at ${:04x} names stream {stream}, but §10.2 defines \
                 only 0 and 1",
                instruction.address
            )));
        }

        self.file_input = false;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Open a stream 3 redirection into a table (§7.1.2.1).
    fn redirect_into(
        &mut self,
        instruction: &Instruction,
        values: &[u16],
    ) -> Result<(), VoxamError> {
        if values.len() < REDIRECTION_OPERANDS {
            return Err(instruction_error(format!(
                "output_stream 3 at ${:04x} names no table to redirect into (§7.1.2.1)",
                instruction.address
            )));
        }

        if self.redirections.len() >= REDIRECTION_LIMIT {
            return Err(instruction_error(format!(
                "output_stream 3 at ${:04x} would nest {} deep; §7.1.2.1.1 allows \
                 {REDIRECTION_LIMIT} at most",
                instruction.address,
                REDIRECTION_LIMIT + 1
            )));
        }

        let limit = self.redirection_limit(values)?;

        self.redirections
            .push((usize::from(values[1]), Vec::new(), limit));

        Ok(())
    }

    /// The wrap width a Version 6 redirection asked for, if any
    /// (§15 output_stream): zero or positive names a window whose
    /// current width in units is the limit, negative means a box
    /// of -width units; no width -- or any version but 6 -- is the
    /// flat, unformatted table. The wrap arithmetic counts
    /// characters, so the unit width divides by the font width: on
    /// the 1-by-1 character glasses the numbers are the same,
    /// while a measuring glass turns 720 pixels back into 80
    /// characters.
    fn redirection_limit(&self, values: &[u16]) -> Result<Option<usize>, VoxamError> {
        if values.len() <= 2 || self.memory.header().version() != 6 {
            return Ok(None);
        }

        let (font_width, _) = self.unit_metrics();
        let width = signed(values[2]);

        if width < 0 {
            return Ok(Some(((-width) / i32::from(font_width)).max(1) as usize));
        }

        let window_width = self.windows.property(width, X_SIZE)?;

        Ok(Some(
            (usize::from(window_width) / usize::from(font_width)).max(1),
        ))
    }

    /// Close the newest stream 3 table, writing its count
    /// (§7.1.2.1). New-lines are written as ZSCII 13 (§7.1.2.2.1);
    /// other characters carry their ZSCII codes.
    fn end_redirection(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let Some((table, units, limit)) = self.redirections.pop() else {
            return Err(instruction_error(format!(
                "output_stream -3 at ${:04x}, but stream 3 is not selected (§7.1.2)",
                instruction.address
            )));
        };

        let repertoire = extras(&self.memory)?;
        let widest;

        if let Some(limit) = limit {
            widest = self.write_formatted(table, &units, limit)?;
        } else {
            for (offset, unit) in units.iter().enumerate() {
                let code = unit_to_zscii(*unit, &repertoire)?;
                self.memory.write_byte(
                    table + REDIRECTION_DATA_OFFSET + offset,
                    (code & 0xFF) as u8,
                )?;
            }

            self.memory.write_word(table, units.len() as u16)?;
            widest = units
                .split(|unit| *unit == u16::from(b'\n'))
                .map(|part| part.len())
                .max()
                .unwrap_or(0);
        }

        if self.memory.header().version() == 6 {
            // The $30 word is "total width in pixels" (§11's
            // table) -- characters times the font width, real
            // pixels on a measuring glass (§8.8.1).
            let (font_width, _) = self.unit_metrics();

            self.memory.write_word(0x30, widest as u16 * font_width)?;
        }

        Ok(())
    }

    /// Write print_form's line shape: counted lines, a zero end
    /// (§15 print_form). The count doubles as the terminator, so a
    /// blank line travels as a single space. Returns the widest
    /// line written, for the header's $30 word.
    fn write_formatted(
        &mut self,
        table: usize,
        units: &[u16],
        limit: usize,
    ) -> Result<usize, VoxamError> {
        let repertoire = extras(&self.memory)?;
        let text = units_to_string(units);
        let mut position = table;
        let mut widest = 0;

        for line in wrapped(&text, limit) {
            let carried = if line.is_empty() {
                " ".to_string()
            } else {
                line
            };
            let count = carried.chars().count();
            widest = widest.max(count);

            self.memory.write_word(position, count as u16)?;
            position += REDIRECTION_DATA_OFFSET;

            for character in carried.chars() {
                let code = char_to_zscii(character, &repertoire)?;
                self.memory.write_byte(position, (code & 0xFF) as u8)?;
                position += 1;
            }
        }

        self.memory.write_word(position, 0)?;

        Ok(widest)
    }

    /// Search a table for a value, delivering its address (§15).
    fn op_scan_table(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;
        let target = values[0];
        let count = values[2];
        let form = values
            .get(SCAN_FORM_OPERAND)
            .copied()
            .unwrap_or(DEFAULT_SCAN_FORM);

        let width = usize::from(form & SCAN_FIELD_MASK);
        let words = form & SCAN_WORD_BIT != 0;

        let mut address = usize::from(values[1]);
        let mut found: u16 = 0;

        for _ in 0..count {
            let entry = if words {
                self.memory.read_word(address)?
            } else {
                u16::from(self.memory.read_byte(address)?)
            };

            if entry == target {
                found = address as u16;
                break;
            }

            address += width;
        }

        self.store_result(instruction.store_variable, found)?;
        self.branch(instruction, found != 0)
    }

    /// Save the state of play (§15 save): this session carries no
    /// save slot yet, so every save reports failure -- an answer
    /// the story already knows how to hear.
    fn op_save(&mut self, instruction: &Instruction) -> Result<Step, VoxamError> {
        if !instruction.operands.is_empty() {
            return Err(unimplemented(
                "the auxiliary-table save",
                instruction.address,
            ));
        }

        // The snapshot's PC is this instruction's own rider -- the
        // branch data through Version 3, the store byte from 4
        // (Quetzal §5.8) -- so a restore resumes right there.
        let snapshot = Snapshot {
            dynamic_memory: self.memory.dynamic_snapshot(),
            pc: instruction.operands_end,
            frames: self.calls.snapshot(),
        };
        let data = crate::zmachine::quetzal::write(&snapshot, &self.story)?;

        // A display that cannot block asks through its own file
        // prompt: the finish parks, and the host's answer lands
        // through deliver_file -- the same standing-down the reads
        // learned (§15 save).
        if self.frontend.suspends() {
            self.wait_serial += 1;
            self.waiting = Some(Waiting::File(Filing {
                purpose: Purpose::Save,
                instruction: instruction.clone(),
                data: Some(data),
            }));

            return Ok(Step::Suspended);
        }

        let success = match &mut self.saves {
            Some(slot) => slot.write(&data),
            None => false,
        };

        self.save_rider(instruction, success)?;

        Ok(Step::Ran)
    }

    /// Answer a save the §15 way: a branch through Version 3, a
    /// stored result from Version 4.
    fn save_rider(&mut self, instruction: &Instruction, success: bool) -> Result<(), VoxamError> {
        if self.memory.header().version() <= BRANCHING_SAVE_FINAL_VERSION {
            self.branch(instruction, success)
        } else {
            self.store_result(instruction.store_variable, u16::from(success))?;
            self.pc = instruction.next_address;

            Ok(())
        }
    }

    /// Restore a saved state of play (§15 restore, §6.1.2). On
    /// success the machine does not continue here at all: the
    /// restored state resumes at the save's rider (Quetzal §5.8).
    /// Failure is a result the story hears -- no branch through
    /// Version 3, a stored 0 from Version 4 -- whether the slot
    /// was empty, the bytes were not a save, or the save names
    /// another game.
    fn op_restore(&mut self, instruction: &Instruction) -> Result<Step, VoxamError> {
        if !instruction.operands.is_empty() {
            return Err(unimplemented(
                "the auxiliary-table restore",
                instruction.address,
            ));
        }

        // The restore stands down for its file the way the save
        // does; the bytes are still to be found (§15 restore).
        if self.frontend.suspends() {
            self.wait_serial += 1;
            self.waiting = Some(Waiting::File(Filing {
                purpose: Purpose::Restore,
                instruction: instruction.clone(),
                data: None,
            }));

            return Ok(Step::Suspended);
        }

        let data = match &mut self.saves {
            Some(slot) => slot.read(),
            None => None,
        };

        let snapshot =
            data.and_then(|bytes| crate::zmachine::quetzal::read(&bytes, &self.story).ok());

        let Some(snapshot) = snapshot else {
            if self.memory.header().version() > BRANCHING_SAVE_FINAL_VERSION {
                self.store_result(instruction.store_variable, FALSE_VALUE)?;
            }

            self.pc = instruction.next_address;

            return Ok(Step::Ran);
        };

        self.restore(&snapshot)?;
        self.resume_from_save(snapshot.pc)?;

        Ok(Step::Ran)
    }

    /// Save the state of play into the interpreter's hand (§15):
    /// the PC captured is this instruction's own store byte,
    /// exactly as save records its rider (Quetzal §5.8.2).
    fn op_save_undo(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        if self.undo.len() == UNDO_DEPTH {
            self.undo.pop_front();
        }

        self.undo.push_back(Snapshot {
            dynamic_memory: self.memory.dynamic_snapshot(),
            pc: instruction.operands_end,
            frames: self.calls.snapshot(),
        });

        self.store_result(instruction.store_variable, TRUE_VALUE)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Restore the state save_undo holds (§15); with nothing in
    /// hand it stores 0 and moves on, the quiet option the spec
    /// offers.
    fn op_restore_undo(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let Some(snapshot) = self.undo.pop_back() else {
            self.store_result(instruction.store_variable, FALSE_VALUE)?;
            self.pc = instruction.next_address;

            return Ok(());
        };

        self.restore(&snapshot)?;
        self.resume_from_save(snapshot.pc)
    }

    /// Pick up execution at the rider of the save that made us
    /// (Quetzal §5.8): through Version 3 the branch data, taken as
    /// the successful save it was; from Version 4 the store byte,
    /// answered with 2 (§15 save).
    fn resume_from_save(&mut self, pc: usize) -> Result<(), VoxamError> {
        if self.memory.header().version() <= BRANCHING_SAVE_FINAL_VERSION {
            let (branch, after) = read_branch(&self.memory, pc)?;

            self.apply_branch(&branch, after, true)
        } else {
            let (variable, after) = read_store_variable(&self.memory, pc)?;

            self.variables
                .write(&mut self.memory, &mut self.calls, variable, RESTORED_VALUE)?;
            self.pc = after;

            Ok(())
        }
    }

    /// Start the story over from the pristine file (§6.1.3):
    /// everything reloads, but 'Flags 2' survives and the Rst
    /// header fields are re-stamped.
    fn op_restart(&mut self) -> Result<(), VoxamError> {
        let flags2 = self.memory.read_word(FLAGS_2)?;
        let static_base = usize::from(self.story.header().static_memory_base());
        let pristine = self.story.data()[..static_base].to_vec();

        self.memory.restore_dynamic(&pristine)?;
        self.memory.write_word(FLAGS_2, flags2)?;
        self.calls.restore(&[FrameSnapshot {
            return_address: 0,
            store_variable: None,
            locals: Vec::new(),
            argument_count: 0,
            stack: Vec::new(),
        }])?;
        self.redirections.clear();
        self.screen_selected = true;

        // The current font is interpreter bookkeeping, so a restart
        // returns it to normal along with the rest.
        self.font = NORMAL_FONT;
        self.frontend.set_font(NORMAL_FONT);

        // The §8.8 window ledger returns to its boot state with
        // the rest of the interpreter's own memory, aimed cursors
        // included.
        let (font_width, font_height) = self.unit_metrics();

        self.aimed.clear();
        self.windows = WindowLedger::new(
            u16::from(self.frontend.screen_lines()) * font_height,
            u16::from(self.frontend.screen_columns()) * font_width,
            u16::from(DEFAULT_FOREGROUND_COLOUR),
            u16::from(DEFAULT_BACKGROUND_COLOUR),
            font_width,
            font_height,
        );

        self.declare_capabilities()?;
        self.start_execution()
    }

    /// Hand the requested type style to the frontend (§8.7); each
    /// frontend renders the styles it claimed and ignores the rest.
    fn op_set_text_style(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let style = self.value(&instruction.operands[0])?;

        self.frontend.set_style(style);
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Choose a §8.1.2 font, storing the one it replaces (§15).
    /// Font 0 asks which is current; one not on offer changes
    /// nothing and stores 0, the refusal §8.1.3 builds on.
    fn op_set_font(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let font = self.value(&instruction.operands[0])?;

        if font == CURRENT_FONT {
            let current = self.font;
            self.store_result(instruction.store_variable, current)?;
        } else if self.font_available(font) {
            let previous = self.font;
            self.font = font;

            self.frontend.set_font(font);
            self.store_result(instruction.store_variable, previous)?;
        } else {
            self.store_result(instruction.store_variable, FONT_REFUSED)?;
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Whether the frontend has a §8.1.2 font to offer: normal and
    /// fixed-pitch are one face on a character terminal, the §16
    /// font belongs to frontends that claimed it, and everything
    /// else is refused (§8.1.4, §8.1.6).
    fn font_available(&self, font: u16) -> bool {
        if font == NORMAL_FONT || font == COURIER_FONT {
            return true;
        }

        font == GRAPHICS_FONT && self.frontend.has_character_graphics()
    }

    /// Hand a window erasure to the frontend (§8.7): -1 unsplits
    /// and clears everything, leaving the lower window selected
    /// (§8.7.3.3). The Version 6 forms wait with Version 6.
    fn op_erase_window(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let mut window = signed(self.value(&instruction.operands[0])?);

        if self.memory.header().version() == 6 {
            if window >= 0 || window == CURRENT_WINDOW {
                let target = self.windows.resolve(window)?;

                // A character glass renders only windows 0 and 1;
                // a stage erases any of the eight (§8.8.5.3).
                if !self.frontend.has_stage() && target > 1 {
                    self.pc = instruction.next_address;

                    return Ok(());
                }

                window = i32::from(target);
            }

            if window == UNSPLIT_ERASE {
                // §8.8.5.3.1: erasing -1 selects window 0.
                self.windows.selected = 0;
            }
        }

        if window == UNSPLIT_ERASE {
            self.story_window = true;
        }

        self.frontend.erase_window(window);
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Erase rightward from the cursor (§15 erase_line): value 1
    /// erases to the end of the line in every version that has the
    /// opcode. Any other value does nothing before Version 6; in
    /// Version 6 it instead erases the value minus one pixels
    /// rightward, clipped to stay inside the right margin
    /// (§8.8.5.2).
    fn op_erase_line(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let value = self.value(&instruction.operands[0])?;

        if value == ERASE_TO_END {
            self.frontend.erase_line(None);
        } else if self.memory.header().version() == 6 {
            self.frontend.erase_line(Some(i32::from(value) - 1));
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Hand the word-wrap buffering toggle to the frontend (§8.7).
    fn op_buffer_mode(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let buffered = self.value(&instruction.operands[0])?;

        self.frontend.set_buffering(buffered != 0);
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Hand the upper window's new height to the frontend (§8.7.2).
    fn op_split_window(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let height = self.value(&instruction.operands[0])?;

        if self.memory.header().version() == 6 {
            // Tile ledger windows 1 and 0 vertically (§8.8.4.1):
            // window 1 takes the top at the given height in units
            // and window 0 the rest; widths stay put.
            let (_, font_height) = self.unit_metrics();
            let screen_height = u16::from(self.frontend.screen_lines()) * font_height;

            self.windows.write_property(1, windows::Y_COORDINATE, 1)?;
            self.windows.write_property(1, windows::Y_SIZE, height)?;
            self.windows
                .write_property(0, windows::Y_COORDINATE, height + 1)?;
            self.windows.write_property(
                0,
                windows::Y_SIZE,
                screen_height.saturating_sub(height),
            )?;
        }

        self.frontend.split_window(height);
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Hand the window selection to the frontend (§8.7.2).
    fn op_set_window(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let window = self.value(&instruction.operands[0])?;

        if self.memory.header().version() == 6 {
            // Any of the eight may be chosen and -3 keeps the
            // current one; the character glass hears only about
            // windows 0 and 1, the two it renders (§8.8.3).
            let selected = self.windows.resolve(i32::from(window))?;
            self.windows.selected = selected;
            self.story_window = selected == 0;

            if self.frontend.has_stage() {
                // The stage hears every selection. A cursor the
                // game aimed at this window while it was unselected
                // rides along now; otherwise the window's own
                // remembered cursor stands (§8.8.3.5) -- the stage
                // tracks it through printing, which the ledger's
                // stale copy does not.
                self.frontend.set_window(selected);

                if self.aimed.remove(&selected) {
                    let line = self.windows.property(i32::from(selected), Y_CURSOR)?;
                    let column = self.windows.property(i32::from(selected), X_CURSOR)?;

                    self.frontend.set_cursor(line, column);
                }
            } else if selected <= 1 {
                self.frontend.set_window(selected);
            }
        } else {
            self.story_window = window == 0;
            self.frontend.set_window(window);
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Move the cursor (§8.7.2, §15 set_cursor); the Version 6
    /// forms wait with Version 6.
    fn op_set_cursor(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        if self.memory.header().version() == 6 {
            let values = self.values(instruction)?;

            // A line of -1 turns the blinking cursor off and -2
            // turns it back on: chrome a character glass has no
            // cursor of its own to honour, passed quietly. An
            // ordinary move may name any window, defaulting to
            // the current one (§15 set_cursor).
            let line = values[0];

            if line != 0xFFFF && line != 0xFFFE {
                let column = values.get(1).copied().unwrap_or(0);
                let window = values
                    .get(2)
                    .map_or(CURRENT_WINDOW, |value| i32::from(*value));
                let target = self.windows.resolve(window)?;

                self.windows
                    .write_property(i32::from(target), Y_CURSOR, line)?;
                self.windows
                    .write_property(i32::from(target), X_CURSOR, column)?;

                if self.frontend.has_stage() {
                    if target == self.windows.selected {
                        self.frontend.set_cursor(line, column);
                    } else {
                        // Aimed at an unselected window: the move
                        // reaches the stage when that window is
                        // next selected.
                        self.aimed.insert(target);
                    }
                }
            }
        } else {
            let line = self.value(&instruction.operands[0])?;
            let column = self.value(&instruction.operands[1])?;

            self.frontend.set_cursor(line, column);
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Write the cursor's row and column into an array (§15): word
    /// 0 the row, word 1 the column -- the upper window's cursor,
    /// the one set_cursor can move (§8.7.2.3.2).
    fn op_get_cursor(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let array = usize::from(self.value(&instruction.operands[0])?);

        let (row, column) = if self.memory.header().version() == 6 {
            if self.frontend.has_stage() {
                // The stage's cursor is the printing truth: text
                // flow moves it, which the ledger's copy never
                // sees. PunyInform saves the cursor here before
                // redrawing its status line and restores it after
                // -- a stale answer reprints a whole line.
                self.frontend.cursor_position()
            } else {
                // A character glass reads the current window's
                // cursor from the §8.8 ledger -- the same place
                // its set_cursor writes, so the round trip is
                // exact.
                (
                    self.windows.property(CURRENT_WINDOW, Y_CURSOR)?,
                    self.windows.property(CURRENT_WINDOW, X_CURSOR)?,
                )
            }
        } else {
            self.frontend.cursor_position()
        };

        self.memory.write_word(array, row)?;
        self.memory.write_word(array + 2, column)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Print a rectangle of ZSCII rows from a table (§15
    /// print_table). On the screen the rectangle spreads from the
    /// cursor; into a stream 3 table, or with the screen
    /// deselected, the rows travel as newline-separated lines --
    /// which is also what a plain transcript shows.
    fn op_print_table(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;

        let table = usize::from(values[0]);
        let width = usize::from(values[1]);
        let height = values.get(PRINT_TABLE_HEIGHT_OPERAND).copied().unwrap_or(1);
        let skip = usize::from(values.get(PRINT_TABLE_SKIP_OPERAND).copied().unwrap_or(0));

        let repertoire = extras(&self.memory)?;
        let version = self.memory.header().version();
        let mut rows: Vec<Units> = Vec::new();
        let mut position = table;

        for _ in 0..height {
            let mut row: Units = Vec::new();

            for offset in 0..width {
                let code = self.memory.read_byte(position + offset)?;
                row.extend(zscii_to_units(u16::from(code), &repertoire, version)?);
            }

            rows.push(row);
            position += width + skip;
        }

        if !self.redirections.is_empty() || !self.screen_selected {
            for (index, row) in rows.iter().enumerate() {
                if index > 0 {
                    self.print_units(&[u16::from(b'\n')])?;
                }

                self.print_units(row)?;
            }
        } else {
            let shown: Vec<String> = rows.iter().map(|row| units_to_string(row)).collect();
            self.frontend.write_rectangle(&shown);
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Copy or zero a run of table bytes (§15 copy_table): a zero
    /// second table zeroes size bytes of the first; a positive
    /// size copies without corruption however the tables overlap;
    /// a negative size forces a forward byte-at-a-time smear.
    fn op_copy_table(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;

        let first = usize::from(values[0]);
        let second = usize::from(values[1]);
        let size = signed(values[2]);

        if second == 0 {
            for offset in 0..size.unsigned_abs() as usize {
                self.memory.write_byte(first + offset, 0)?;
            }
        } else if size < 0 {
            for offset in 0..(-size) as usize {
                let value = self.memory.read_byte(first + offset)?;
                self.memory.write_byte(second + offset, value)?;
            }
        } else {
            let mut data = Vec::with_capacity(size as usize);

            for offset in 0..size as usize {
                data.push(self.memory.read_byte(first + offset)?);
            }

            for (offset, value) in data.iter().enumerate() {
                self.memory.write_byte(second + offset, *value)?;
            }
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Lexically analyse text already in the buffer (§15
    /// tokenise): the lexing half of read as its own opcode. A
    /// nonzero third operand names a custom dictionary, and a
    /// nonzero fourth leaves unrecognised words' slots untouched.
    fn op_tokenise(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;

        let text_buffer = usize::from(values[0]);
        let parse_buffer = usize::from(values[1]);
        let base = values
            .get(TOKENISE_DICTIONARY_OPERAND)
            .copied()
            .filter(|&address| address != 0)
            .map(usize::from);
        let keep = values
            .get(TOKENISE_FLAG_OPERAND)
            .is_some_and(|&flag| flag != 0);

        // tokenise exists from Version 5, so the buffer is always
        // the counted layout: length in byte 1, text from byte 2
        // (§15 read). The bytes travel as their raw codepoints,
        // exactly as the reference reads them.
        let count = usize::from(self.memory.read_byte(text_buffer + 1)?);
        let mut line = String::with_capacity(count);

        for offset in 0..count {
            let code = self.memory.read_byte(text_buffer + 2 + offset)?;
            line.push(char::from_u32(u32::from(code)).expect("a byte is always a char"));
        }

        self.parse_with(parse_buffer, &line, 2, base, keep)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Encode buffer text in dictionary form (§15 encode_text):
    /// the operands are followed to the letter, with no hunting
    /// for a terminating 0.
    fn op_encode_text(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;

        let text = usize::from(values[0]);
        let length = usize::from(values[1]);
        let start = usize::from(values[2]);
        let coded = usize::from(values[3]);

        let mut word = String::with_capacity(length);

        for offset in 0..length {
            let code = self.memory.read_byte(text + start + offset)?;
            word.push(char::from_u32(u32::from(code)).expect("a byte is always a char"));
        }

        let rows = crate::zmachine::zscii::alphabets(&self.memory)?;
        let repertoire = extras(&self.memory)?;
        let encoded = crate::zmachine::zscii::encode_word(
            self.memory.header().version(),
            &word,
            Some(&rows),
            &repertoire,
        )?;

        for (offset, value) in encoded.iter().enumerate() {
            self.memory.write_byte(coded + offset, *value)?;
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Place a window at (y, x) in the ledger (§15 move_window):
    /// §15 itself says "nothing actually happens" on screen, so
    /// the ledger is the whole of the truth.
    fn op_move_window(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;

        self.windows
            .r#move(i32::from(values[0]), values[1], values[2])?;
        self.place_staged(i32::from(values[0]))?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Send a window's ledger geometry to a staged frontend.
    fn place_staged(&mut self, window: i32) -> Result<(), VoxamError> {
        if self.frontend.has_stage() {
            let resolved = self.windows.resolve(window)?;
            let line = self.windows.property(window, Y_COORDINATE)?;
            let column = self.windows.property(window, X_COORDINATE)?;
            let height = self.windows.property(window, Y_SIZE)?;
            let width = self.windows.property(window, X_SIZE)?;

            self.frontend
                .place_window(resolved, line, column, height, width);
        }

        Ok(())
    }

    /// Resize a window in the ledger (§15 window_size): "does not
    /// change the current display", says §15.
    fn op_window_size(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;

        self.windows
            .resize(i32::from(values[0]), values[1], values[2])?;
        self.place_staged(i32::from(values[0]))?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Change a window's attribute flags (§15 window_style).
    fn op_window_style(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;
        let operation = values.get(2).copied().unwrap_or(0);

        self.windows
            .restyle(i32::from(values[0]), values[1], operation)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Store one §8.8.3.2 window property (§15 get_wind_prop).
    ///
    /// On a stage, the selected window's cursor properties answer
    /// from the frontend's flowed cursor: printing moves it, and
    /// the ledger's copy cannot know (§8.8.3.5). Shogun centres
    /// each title line by reading property 4 back between prints
    /// -- against the stale copy, every line lands on the first
    /// one's row.
    fn op_get_wind_prop(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;
        let mut value = self.windows.property(i32::from(values[0]), values[1])?;

        if self.frontend.has_stage()
            && (values[1] == Y_CURSOR || values[1] == X_CURSOR)
            && self.windows.resolve(i32::from(values[0]))? == self.windows.selected
        {
            let (line, column) = self.frontend.cursor_position();

            value = if values[1] == Y_CURSOR { line } else { column };
        }

        self.store_result(instruction.store_variable, value)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Write one window property (§15 put_wind_prop).
    fn op_put_wind_prop(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;

        self.windows
            .write_property(i32::from(values[0]), values[1], values[2])?;

        // A staged frontend hears line-count writes: games set
        // them freely to manipulate when [MORE] is printed
        // (§8.8.3.2.6).
        if self.frontend.has_stage() && values[1] == LINE_COUNT {
            let resolved = self.windows.resolve(i32::from(values[0]))?;

            self.frontend.set_line_count(resolved, signed(values[2]));
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Set a window's margin sizes (§15 set_margins); the window
    /// operand comes last and may be omitted, meaning the current
    /// window.
    fn op_set_margins(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;
        let window = values
            .get(2)
            .map_or(CURRENT_WINDOW, |value| i32::from(*value));

        self.windows.set_margins(window, values[0], values[1])?;

        if self.frontend.has_stage() {
            let resolved = self.windows.resolve(window)?;

            self.frontend.set_margins(resolved, values[0], values[1]);
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Let a pixel scroll pass in the conforming quiet (§15
    /// scroll_window): a character glass scrolls its lower window
    /// by §8.7 as text flows and has no pixels here to shift.
    fn op_scroll_window(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;

        if self.frontend.has_stage() {
            let resolved = self.windows.resolve(signed(values[0]))?;

            self.frontend.scroll_window(resolved, signed(values[1]));
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Report a mouse that never moves or clicks (§15 read_mouse):
    /// the array's four words report zeros -- parked at nowhere,
    /// no buttons down, no menu touched.
    fn op_read_mouse(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let array = usize::from(self.value(&instruction.operands[0])?);

        for word in 0..4 {
            self.memory.write_word(array + 2 * word, 0)?;
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Fail a menu request the header already refused (§15
    /// make_menu): an interpreter without menus simply never
    /// succeeds.
    fn op_make_menu(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        self.values(instruction)?;
        self.branch(instruction, false)
    }

    /// Print a formatted table, line by line (§15 print_form):
    /// each line a word holding its character count then the
    /// characters, ending at a zero word, each printed with a
    /// new-line after it.
    fn op_print_form(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let mut position = usize::from(self.value(&instruction.operands[0])?);
        let repertoire = extras(&self.memory)?;
        let version = self.memory.header().version();

        loop {
            let count = self.memory.read_word(position)?;

            if count == 0 {
                break;
            }

            position += REDIRECTION_DATA_OFFSET;

            let mut line: Units = Vec::new();

            for offset in 0..usize::from(count) {
                let code = self.memory.read_byte(position + offset)?;
                line.extend(zscii_to_units(u16::from(code), &repertoire, version)?);
            }

            line.push(u16::from(b'\n'));
            self.print_units(&line)?;
            position += usize::from(count);
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Set text colours -- where coloured text is available
    /// (§8.3.1). The spec's own conditional does the work both
    /// ways: a frontend that claimed colours in the header
    /// receives the pair, and one that truthfully declared none
    /// makes the request a legitimate no-op. The pair travels
    /// signed, so §8.3.1's colour -1 -- the pixel under the cursor
    /// -- arrives as itself for a frontend that can sample.
    fn op_set_colour(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;

        if self.frontend.has_colours() {
            self.frontend
                .set_colour(signed(values[0]), signed(values[1]));
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Answer the game's questions about pictures (§15
    /// picture_data): on a frontend without pictures, the census
    /// counts zero and no number is valid -- exactly what the
    /// header's cleared pictures bit promised (§11.1.4).
    fn op_picture_data(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;
        let number = values[0];
        let array = usize::from(values[1]);

        if number == 0 {
            let (count, release) = self.frontend.picture_census();

            self.memory.write_word(array, count)?;
            self.memory.write_word(array + 2, release)?;

            return self.branch(instruction, count > 0);
        }

        let Some((height, width)) = self.frontend.picture_data(number) else {
            return self.branch(instruction, false);
        };

        self.memory.write_word(array, height)?;
        self.memory.write_word(array + 2, width)?;
        self.branch(instruction, true)
    }

    /// Draw a picture at a units position, or paint its region to
    /// the background (§15 draw_picture, erase_picture).
    ///
    /// Without pictures the call passes in the conforming quiet:
    /// Infocom's own games draw without consulting the header --
    /// the §11.1.4 remarks name Zork Zero's Macintosh release for
    /// it. With pictures, coordinates of zero (or omitted) mean
    /// the current window's cursor, the given ones are relative to
    /// the window's own origin (§8.8.3.5), and an invalid picture
    /// number is the one thing §15 calls illegal.
    fn op_draw_picture(
        &mut self,
        instruction: &Instruction,
        drawing: bool,
    ) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;
        let number = values[0];

        if !self.frontend.has_pictures() {
            self.pc = instruction.next_address;

            return Ok(());
        }

        if self.frontend.picture_data(number).is_none() {
            return Err(VoxamError::ZMachineScreen(format!(
                "picture {number} is not in the gallery, and §15 calls drawing                  an invalid picture number illegal"
            )));
        }

        let mut line = values.get(1).copied().unwrap_or(0);
        let mut column = values.get(2).copied().unwrap_or(0);

        if line == 0 {
            line = self.windows.property(CURRENT_WINDOW, Y_CURSOR)?;
        }

        if column == 0 {
            column = self.windows.property(CURRENT_WINDOW, X_CURSOR)?;
        }

        line = line
            .wrapping_add(self.windows.property(CURRENT_WINDOW, Y_COORDINATE)?)
            .wrapping_sub(1);
        column = column
            .wrapping_add(self.windows.property(CURRENT_WINDOW, X_COORDINATE)?)
            .wrapping_sub(1);

        if drawing {
            self.frontend.draw_picture(number, line, column);
        } else {
            self.frontend.erase_picture(number, line, column);
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Hang, replace, or clear the arc_image band (EXT:0x80).
    ///
    /// Two operands, no store, no branch: the picture id -- zero
    /// clears the band -- and the mode naming its height in text
    /// rows (arc_image: the contract, part A). A game only draws
    /// after finding Flags 1's picture bit set, so a frontend that
    /// never claimed the band is never asked -- and if a stray
    /// call arrives anyway, it passes as quietly as any private
    /// opcode, because a picture is presentation, never state.
    fn op_draw_image(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;

        if self.frontend.has_arc_images() && values.len() >= 2 {
            self.frontend.draw_arc_image(values[0], values[1]);
            self.rebase_rows()?;
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Re-declare the header's rows from the display's arc claim.
    ///
    /// One byte of header, and 255 already means "infinite" there
    /// (§8.4): the claim stays inside both bounds (arc_image: the
    /// contract, part A).
    pub fn rebase_rows(&mut self) -> Result<(), VoxamError> {
        if let Some(below) = self.frontend.arc_rows_below() {
            declare::screen_size(
                &mut self.memory,
                below.clamp(1, 255) as u8,
                self.frontend.screen_columns(),
            )?;
        }

        Ok(())
    }

    /// Take the display-buffering advice §8.8.7's way (§15
    /// buffer_screen): the glasses paint every change as it
    /// happens, the conduct required of an interpreter that
    /// ignores the advice -- but the mode is remembered and the
    /// old one stored, and -1 forces an update without changing
    /// it.
    fn op_buffer_screen(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let mode = signed(self.value(&instruction.operands[0])?);

        if !matches!(mode, -1..=1) {
            return Err(instruction_error(format!(
                "buffer_screen at ${:04x} asks for mode {mode}, but §8.8.7.1 defines \
                 only 0, 1, and -1",
                instruction.address
            )));
        }

        let previous = self.screen_buffering;

        if mode != -1 {
            self.screen_buffering = mode as u16;
        }

        self.store_result(instruction.store_variable, previous)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// Push onto a user stack, branching on success (§15
    /// push_stack): the first word counts the spare slots and is
    /// also the write index; a full stack does nothing at all.
    fn op_push_stack(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;
        let (value, stack) = (values[0], usize::from(values[1]));
        let spare = self.memory.read_word(stack)?;

        if spare != 0 {
            self.memory
                .write_word(stack + 2 * usize::from(spare), value)?;
            self.memory.write_word(stack, spare - 1)?;
        }

        self.branch(instruction, spare != 0)
    }

    /// Throw items away from the top of a stack (§15 pop_stack):
    /// the game stack by default, a §6.6 user stack with a second
    /// operand, where discarding walks the spare count up.
    fn op_pop_stack(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        let values = self.values(instruction)?;
        let items = values[0];

        if let Some(&stack) = values.get(1) {
            let stack = usize::from(stack);
            let spare = self.memory.read_word(stack)?;

            self.memory.write_word(stack, spare + items)?;
        } else {
            for _ in 0..items {
                self.calls.pop()?;
            }
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Pass over a future Standard's EXT opcode, warning once
    /// off-screen (§14.2.1).
    fn op_ext_reserved(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        if self.passed_reserved.insert(instruction.opcode_number) {
            eprintln!(
                "voxam: EXT:{} is reserved for a future Standard; passed unclaimed \
                 (§14.2.1)",
                instruction.opcode_number
            );
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Read a typed command into the buffers (§15 read, §13.6):
    /// operands resolve and the buffers are policed, then the read
    /// parks its whole tail and the machine stands down for the
    /// host's line.
    fn op_sread(&mut self, instruction: &Instruction) -> Result<Step, VoxamError> {
        let values = self.values(instruction)?;
        let version = self.memory.header().version();

        // In Versions 1 to 3 the status line is redisplayed before
        // the player types (§8.2, §15 read) -- when there is one.
        if version <= 3 && self.frontend.has_status_line() {
            let status = self.status()?;
            self.frontend.show_status(&status);
        }

        let text_buffer = usize::from(values[0]);
        let parse_buffer = usize::from(values.get(1).copied().unwrap_or(0));
        let counted = version >= COUNTED_TEXT_VERSION;

        if !counted && parse_buffer == 0 {
            return Err(instruction_error(format!(
                "read at ${:04x} names no parse buffer, but lexing is not optional \
                 before Version 5 (§15 read)",
                instruction.address
            )));
        }

        let capacity = usize::from(self.memory.read_byte(text_buffer)?);
        let minimum = if counted { 1 } else { 2 };

        if capacity < minimum {
            return Err(VoxamError::ZMachineMemory(format!(
                "the text buffer at ${text_buffer:04x} claims a capacity of \
                 {capacity}: almost certainly overrun by a previous array (§15 read)"
            )));
        }

        // A line read is a fresh sitting: the interval a terminated
        // read_char burned belonged to the keystroke rhythm, not to
        // this prompt (§15 read_char).
        self.typist_ready = None;

        let (time, routine) = read_cadence(&values, version);

        // A display that cannot block stands down instead: the
        // finish parks whole, the cadence alongside, and the
        // host's timer sends the ticks (§15 read).
        if self.frontend.suspends() {
            let (preloaded, held) = self.preloaded(text_buffer, capacity, counted)?;

            self.wait_serial += 1;
            self.waiting = Some(Waiting::Read(Reading {
                wants: Wants::Line,
                instruction: instruction.clone(),
                text_buffer,
                parse_buffer,
                counted,
                capacity,
                preloaded,
                held,
                terminators: self.terminators()?,
                time,
                routine,
            }));

            return Ok(Step::Suspended);
        }

        // The patient typist lets one interval elapse before the
        // line arrives: the routine fires once, and a true return
        // erases the input and ends the read with 0 stored (§15).
        if time != 0 && routine != 0 && self.interrupt(routine)? != 0 {
            self.abandoned_line(instruction, text_buffer, parse_buffer, counted)?;

            return Ok(Step::Ran);
        }

        let (preloaded, held) = self.preloaded(text_buffer, capacity, counted)?;

        self.wait_serial += 1;
        self.waiting = Some(Waiting::Read(Reading {
            wants: Wants::Line,
            instruction: instruction.clone(),
            text_buffer,
            parse_buffer,
            counted,
            capacity,
            preloaded,
            held,
            terminators: self.terminators()?,
            time,
            routine,
        }));

        Ok(Step::Suspended)
    }

    /// Read one keystroke, storing its ZSCII code (§15 read_char).
    ///
    /// Keys already under the fingers -- a queue mid-line -- land
    /// at once. A time and routine pair runs the patient typist:
    /// the routine fires once per fresh sitting, a true return
    /// ending the read with 0 stored, and the retry at the SAME
    /// address finds the keys ready -- Custard's animation loop
    /// depends on exactly that nimbleness.
    fn op_read_char(&mut self, instruction: &Instruction) -> Result<Step, VoxamError> {
        let values = self.values(instruction)?;

        // The device operand itself may be omitted, and an absent
        // device is the keyboard, there being no other (§15).
        if let Some(&device) = values.first()
            && device != 1
        {
            return Err(instruction_error(format!(
                "read_char at ${:04x} asks for input device {device}, but the \
                 keyboard, 1, is the only device there is (§15 read_char)",
                instruction.address
            )));
        }

        let time = values.get(1).copied().unwrap_or(0);
        let routine = values.get(2).copied().unwrap_or(0);
        let version = self.memory.header().version();

        // A display that cannot block stands down at once, the
        // cadence parked only as a pair (§15 read_char).
        if self.frontend.suspends() {
            let paired = time != 0 && routine != 0;

            self.wait_serial += 1;
            self.waiting = Some(Waiting::Read(Reading {
                wants: Wants::Key,
                instruction: instruction.clone(),
                text_buffer: 0,
                parse_buffer: 0,
                counted: false,
                capacity: 0,
                preloaded: 0,
                held: String::new(),
                terminators: HashSet::new(),
                time: if paired { time } else { 0 },
                routine: if paired { routine } else { 0 },
            }));

            return Ok(Step::Suspended);
        }

        let ready = !self.pending_keys.is_empty() || self.typist_ready == Some(instruction.address);
        self.typist_ready = None;

        if !ready
            && version >= TIMED_READ_VERSION
            && time != 0
            && routine != 0
            && self.interrupt(routine)? != 0
        {
            self.typist_ready = Some(instruction.address);
            self.abandoned_key(instruction)?;

            return Ok(Step::Ran);
        }

        if let Some(key) = self.pending_keys.pop_front() {
            let repertoire = extras(&self.memory)?;
            let code = char_to_zscii(key, &repertoire)?;

            self.landed_key(instruction, code)?;

            return Ok(Step::Ran);
        }

        self.wait_serial += 1;
        self.waiting = Some(Waiting::Read(Reading {
            wants: Wants::Key,
            instruction: instruction.clone(),
            text_buffer: 0,
            parse_buffer: 0,
            counted: false,
            capacity: 0,
            preloaded: 0,
            held: String::new(),
            terminators: HashSet::new(),
            time,
            routine,
        }));

        Ok(Step::Suspended)
    }

    /// Finish a keystroke read: store the code, step past.
    fn landed_key(&mut self, instruction: &Instruction, code: u16) -> Result<(), VoxamError> {
        self.sound_since_input = false;
        self.store_result(instruction.store_variable, code)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// End a keystroke read the interrupt way: 0 stored.
    fn abandoned_key(&mut self, instruction: &Instruction) -> Result<(), VoxamError> {
        self.store_result(instruction.store_variable, 0)?;
        self.pc = instruction.next_address;

        Ok(())
    }

    /// End a line read the interrupt way: erased and over (§15
    /// read). A counted buffer reports zero letters typed, a
    /// terminated one an empty string, and the lexing sees that
    /// emptiness.
    fn abandoned_line(
        &mut self,
        instruction: &Instruction,
        text_buffer: usize,
        parse_buffer: usize,
        counted: bool,
    ) -> Result<(), VoxamError> {
        if counted {
            self.memory.write_byte(text_buffer + 1, 0)?;
        } else {
            self.write_text(text_buffer + 1, "", true)?;
        }

        if parse_buffer != 0 || !counted {
            let first_letter = if counted { 2 } else { 1 };

            self.parse(parse_buffer, "", first_letter)?;
        }

        if instruction.opcode.stores {
            self.store_result(instruction.store_variable, 0)?;
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Run a timed-input interrupt routine to completion (§15
    /// read): called with no arguments through the ordinary call
    /// machinery, its result routed through the evaluation stack,
    /// nested frames running until the interrupt's own frame
    /// unwinds. Returns the routine's value -- or true when the
    /// story quit mid-interrupt, because input has certainly ended.
    fn interrupt(&mut self, packed: u16) -> Result<u16, VoxamError> {
        let address = routine_address(&self.memory.header(), packed)?;
        let routine = Routine::parse(&self.memory, address)?;
        let floor = self.calls.depth();

        self.calls.call(&routine, &[], self.pc, Some(0))?;
        self.pc = routine.first_instruction;

        while self.running && self.calls.depth() > floor {
            if self.step()? == Step::Suspended {
                return Err(unimplemented("a read inside an interrupt routine", self.pc));
            }
        }

        if !self.running {
            return Ok(TRUE_VALUE);
        }

        self.variables.read(&self.memory, &mut self.calls, 0)
    }

    /// The §15 preload already in a counted buffer, decoded.
    fn preloaded(
        &mut self,
        text_buffer: usize,
        capacity: usize,
        counted: bool,
    ) -> Result<(usize, String), VoxamError> {
        if !counted {
            return Ok((0, String::new()));
        }

        let preloaded = usize::from(self.memory.read_byte(text_buffer + 1)?).min(capacity);
        let repertoire = extras(&self.memory)?;
        let version = self.memory.header().version();
        let mut units: Units = Vec::new();

        for offset in 0..preloaded {
            let code = self.memory.read_byte(text_buffer + 2 + offset)?;
            units.extend(zscii_to_units(u16::from(code), &repertoire, version)?);
        }

        Ok((preloaded, units_to_string(&units)))
    }

    /// The §10.5.2.1 codes that end this read besides new-line,
    /// walked fresh at every read since the game may rewrite the
    /// table between reads.
    fn terminators(&self) -> Result<HashSet<u16>, VoxamError> {
        let mut codes = HashSet::new();

        if self.memory.header().version() < TERMINATING_VERSION {
            return Ok(codes);
        }

        let mut address = usize::from(self.memory.header().terminating_table_address());

        if address == 0 {
            return Ok(codes);
        }

        loop {
            let code = self.memory.read_byte(address)?;

            if code == 0 {
                return Ok(codes);
            }

            if code == ANY_FUNCTION_KEY {
                codes.extend((129..155).chain([252, 253, 254]));

                return Ok(codes);
            }

            if (129..155).contains(&u16::from(code)) || (252..255).contains(&u16::from(code)) {
                codes.insert(u16::from(code));
            }

            address += 1;
        }
    }

    /// Finish a line read: the typed text lands everywhere it goes.
    fn landed_line(
        &mut self,
        waiting: &Reading,
        raw: &str,
        terminator: u16,
    ) -> Result<(), VoxamError> {
        self.sound_since_input = false;

        let instruction = &waiting.instruction;

        let line = if waiting.counted {
            let typed: String = raw
                .to_lowercase()
                .chars()
                .take(waiting.capacity - waiting.preloaded)
                .collect();
            let line = format!("{}{typed}", waiting.held);

            self.memory
                .write_byte(waiting.text_buffer + 1, line.chars().count() as u8)?;
            self.write_text(waiting.text_buffer + 2 + waiting.preloaded, &typed, false)?;

            line
        } else {
            // Byte 0 holds n where the buffer is a string array of
            // length n: the typed letters plus the zero terminator
            // fit inside it, so the capacity is n - 1 (§15 read).
            let line: String = raw
                .to_lowercase()
                .chars()
                .take(waiting.capacity - 1)
                .collect();

            self.write_text(waiting.text_buffer + 1, &line, true)?;

            line
        };

        // From Version 5 a zero parse buffer skips lexing (§15).
        if waiting.parse_buffer != 0 || !waiting.counted {
            let first_letter = if waiting.counted { 2 } else { 1 };

            self.parse(waiting.parse_buffer, &line, first_letter)?;
        }

        // The stored result is the terminating character: 13 for
        // the return key, or the §10.5.2.1 code that cut the line
        // short (§15 read).
        if instruction.opcode.stores {
            let result = if terminator != 0 {
                terminator
            } else {
                ZSCII_NEWLINE
            };

            self.store_result(instruction.store_variable, result)?;
        }

        self.pc = instruction.next_address;

        Ok(())
    }

    /// Lay typed text into the buffer, zero-terminated or not.
    /// Characters land as ZSCII codes (§3.8.5).
    fn write_text(
        &mut self,
        position: usize,
        line: &str,
        terminate: bool,
    ) -> Result<(), VoxamError> {
        let repertoire = extras(&self.memory)?;
        let mut position = position;

        for character in line.chars() {
            let code = char_to_zscii(character, &repertoire)?;
            self.memory.write_byte(position, (code & 0xFF) as u8)?;
            position += 1;
        }

        if terminate {
            self.memory.write_byte(position, 0)?;
        }

        Ok(())
    }

    /// Write the lexical analysis into the parse buffer (§15 read,
    /// §13.6.3): each block the word's dictionary address or 0, its
    /// letter count, and the position of its first letter.
    fn parse(
        &mut self,
        parse_buffer: usize,
        line: &str,
        first_letter: usize,
    ) -> Result<(), VoxamError> {
        self.parse_with(parse_buffer, line, first_letter, None, false)
    }

    /// The full lexing seam tokenise shares (§15 tokenise): a
    /// custom dictionary when one is named, and with
    /// keep_unrecognized an absent word's block is left untouched
    /// so successive passes accumulate.
    fn parse_with(
        &mut self,
        parse_buffer: usize,
        line: &str,
        first_letter: usize,
        dictionary_base: Option<usize>,
        keep_unrecognized: bool,
    ) -> Result<(), VoxamError> {
        let limit = self.memory.read_byte(parse_buffer)?;

        if limit < MINIMUM_PARSE_WORDS {
            return Err(VoxamError::ZMachineMemory(format!(
                "the parse buffer at ${parse_buffer:04x} claims room for {limit} \
                 words: almost certainly overrun by a previous array (§15 read)"
            )));
        }

        let entries: Vec<(usize, usize, usize)> = {
            let dictionary = Dictionary::new(&self.memory, dictionary_base)?;
            let words = tokenize(line, dictionary.separators());

            words
                .iter()
                .take(usize::from(limit))
                .map(|(word, offset)| {
                    dictionary
                        .lookup(word)
                        .map(|address| (address, word.chars().count(), *offset))
                })
                .collect::<Result<_, _>>()?
        };

        self.memory
            .write_byte(parse_buffer + 1, entries.len() as u8)?;

        let mut block = parse_buffer + WORD_SIZE;

        for (address, length, offset) in entries {
            if address != 0 || !keep_unrecognized {
                self.memory.write_word(block, address as u16)?;
                self.memory.write_byte(block + 2, length as u8)?;
                self.memory
                    .write_byte(block + 3, (offset + first_letter) as u8)?;
            }

            block += 4;
        }

        Ok(())
    }
}

/// Greedy word-wrap onto lines at most limit units wide (§7.2).
/// Forced new-lines end their lines; a word longer than the whole
/// limit breaks at the limit, §8.8.3.1.1's unbuffered fallback.
fn wrapped(text: &str, limit: usize) -> Vec<String> {
    let mut lines = Vec::new();

    for paragraph in text.split('\n') {
        let mut current = String::new();

        for word in paragraph.split(' ') {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };

            if candidate.chars().count() <= limit {
                current = candidate;

                continue;
            }

            if !current.is_empty() {
                lines.push(current);
            }

            let mut remainder: Vec<char> = word.chars().collect();

            while remainder.len() > limit {
                lines.push(remainder[..limit].iter().collect());
                remainder = remainder[limit..].to_vec();
            }

            current = remainder.iter().collect();
        }

        lines.push(current);
    }

    lines
}

/// A line read's §15 time and routine pair, or two zeros.
fn read_cadence(values: &[u16], version: u8) -> (u16, u16) {
    if version >= TIMED_READ_VERSION
        && values.len() > TIMED_ROUTINE_INDEX
        && values[TIMED_TIME_INDEX] != 0
        && values[TIMED_ROUTINE_INDEX] != 0
    {
        return (values[TIMED_TIME_INDEX], values[TIMED_ROUTINE_INDEX]);
    }

    (0, 0)
}

fn instruction_error(message: String) -> VoxamError {
    VoxamError::ZMachineInstruction(message)
}

fn unimplemented(name: &str, address: usize) -> VoxamError {
    VoxamError::ZMachineUnimplemented(format!(
        "reached {name} at ${address:04x}, which is not yet implemented"
    ))
}
