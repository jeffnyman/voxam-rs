//! The Glulx machine: the fetch-decode-execute loop.
//!
//! This is the loop the whole module was built toward. Each step
//! reads an opcode number, looks up its operand signature, decodes
//! the operands, and executes -- the reference's combined dispatch
//! table rendered as the port's usual matches, one for the
//! signature and one for the handler. The float and double
//! families dispatch through the same matches via the shared
//! combinators at the bottom, mirroring the prebuilt table the
//! reference merges in.
//!
//! The handlers receive their operands as a plain slice: unsigned
//! 32-bit values for loads, StoreTargets for stores, in exactly
//! the shapes the signature match promises. The 32-bit discipline
//! was enforced a layer down, so the arithmetic here wraps where
//! Python's unbounded integers were masked on the way out.
//!
//! The output system -- a mode and its rock (Glulx: Output) --
//! folds in here as the IOSystem type, since only the selection is
//! state: filter mode calls back into the VM and Glk mode goes out
//! through the dispatch layer. No Glk library exists in this port
//! yet, so the machine behaves throughout as the reference does
//! with none installed: the capability answers false, setiosys
//! falls back to the null system, and the glk opcode refuses by
//! name.

use crate::errors::VoxamError;
use crate::glulx::accel::{AVAILABLE, Accelerator};
use crate::glulx::floats::{
    close, decode_double, decode_float, encode_double, encode_float, modulo, pow, to_int,
};
use crate::glulx::funcs;
use crate::glulx::gestalt::{self, Capabilities};
use crate::glulx::heap::Heap;
use crate::glulx::memory::Memory;
use crate::glulx::opcodes::{name, op};
use crate::glulx::operand::{
    Arg, OperandList, StoreTarget, decode_opcode, decode_operands, operands, sign_extend, store,
};
use crate::glulx::rng::Randomizer;
use crate::glulx::search;
use crate::glulx::serial;
use crate::glulx::stack::{Stack, dest_type};
use crate::glulx::story::Story;
use crate::glulx::strings;

const SIGN_BIT: u32 = 0x8000_0000;

/// What a popped save stub stores after a restore: "you have just
/// been restored and are continuing from this instruction" (Glulx:
/// Game State).
const RESTORED: u32 = 0xFFFF_FFFF;

/// A branch offset of 0 or 1 does not jump: it returns 0 or 1 from
/// the current function (Glulx: Branches).
const RETURN_ZERO_OFFSET: u32 = 0;
const RETURN_ONE_OFFSET: u32 = 1;

/// The branch bias: offsets count from just past the instruction,
/// less two (Glulx: Branches).
const BRANCH_ADJUSTMENT: u32 = 2;

const SHIFT_LIMIT: i32 = 32;
const BIT_INDEX_MASK: i32 = 0b111;

const WORD_WIDTH: u32 = 4;

fn instruction_error(message: String) -> VoxamError {
    VoxamError::GlulxInstruction(message)
}

/// The output systems setiosys accepts (Glulx: Output).
pub mod io_mode {
    /// Output is discarded -- the mode the machine starts in.
    pub const NULL: u32 = 0;
    /// Each character passes to the Glulx function the rock names.
    pub const FILTER: u32 = 1;
    /// Output goes to the current Glk stream.
    pub const GLK: u32 = 2;
}

/// Which output system is current, and its rock.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IOSystem {
    /// The current io_mode value.
    pub mode: u32,
    /// Filter mode's function address; otherwise decoration.
    pub rock: u32,
}

impl IOSystem {
    /// Select an output system.
    ///
    /// An unrecognized mode is not an error: the spec says setting
    /// an unsupported system selects the null system instead,
    /// which is exactly what a program probing with an unknown
    /// mode should find (Glulx: Output).
    pub fn select(&mut self, mode: u32, rock: u32) {
        if matches!(mode, io_mode::NULL | io_mode::FILTER | io_mode::GLK) {
            self.mode = mode;
            self.rock = rock;
        } else {
            self.mode = io_mode::NULL;
            self.rock = 0;
        }
    }

    /// Return to the null system -- restart's share.
    pub fn reset(&mut self) {
        self.mode = io_mode::NULL;
        self.rock = 0;
    }
}

/// An unsigned 32-bit value reread as the signed one it spells.
pub fn signed(value: u32) -> i32 {
    value as i32
}

/// Signed division truncating toward zero (Glulx: Integer Math) --
/// Rust's own `/`, guarded against the zero divisor and the one
/// overflow: the most negative value divided by -1.
fn divided(a: u32, b: u32) -> Result<u32, VoxamError> {
    let (x, y) = (signed(a), signed(b));

    if y == 0 {
        return Err(instruction_error(
            "division by zero (Glulx: Integer Math)".into(),
        ));
    }

    if y == -1 && x == i32::MIN {
        return Err(instruction_error(
            "division overflow: the most negative value by -1".into(),
        ));
    }

    Ok((x / y) as u32)
}

/// Signed remainder, its sign the dividend's (Glulx: Integer Math)
/// -- Rust's own `%`, guarded the same way.
fn remainder(a: u32, b: u32) -> Result<u32, VoxamError> {
    let (x, y) = (signed(a), signed(b));

    if y == 0 {
        return Err(instruction_error(
            "division by zero taking a remainder (Glulx: Integer Math)".into(),
        ));
    }

    if y == -1 && x == i32::MIN {
        return Err(instruction_error(
            "division overflow taking a remainder".into(),
        ));
    }

    Ok((x % y) as u32)
}

/// Whether a call-stub type resumes a string rather than storing a
/// result (Glulx: Call Stubs) -- the strings module's business.
fn resumes_a_string(desttype: u32) -> bool {
    matches!(
        desttype,
        dest_type::RESUME_COMPRESSED
            | dest_type::RESUME_NUMBER
            | dest_type::RESUME_CSTRING
            | dest_type::RESUME_UNICODE
    )
}

/// A Glulx virtual machine, booted and ready to step.
pub struct Machine {
    story: Story,
    /// The live memory map.
    pub memory: Memory,
    /// The allocation heap.
    pub heap: Heap,
    /// The acceleration table.
    pub accel: Accelerator,
    /// The undo chain, newest state last.
    pub undo_chain: Vec<Vec<u8>>,
    /// The value stack.
    pub stack: Stack,
    /// The current output system.
    pub iosys: IOSystem,
    /// What this build can do.
    pub capabilities: Capabilities,
    /// The string-decoding table's address.
    pub string_table: u32,
    /// The program counter.
    pub pc: u32,
    running: bool,
    // The generator is deliberately not reseeded by restart: it is
    // no part of saved state either (Glulx: The Random Number
    // Generator).
    random: Randomizer,
}

impl Machine {
    /// Boot the machine: memory laid, stack raised, start called.
    ///
    /// A seed makes the dice reproducible; None means true
    /// entropy.
    pub fn new(story: Story, seed: Option<u32>) -> Result<Self, VoxamError> {
        let memory = Memory::new(&story);
        let stack = Stack::new(story.stack_size())?;

        let mut machine = Self {
            story,
            memory,
            heap: Heap::new(),
            accel: Accelerator::new(),
            undo_chain: Vec::new(),
            stack,
            iosys: IOSystem::default(),
            capabilities: Capabilities {
                glk: false,
                ..Default::default()
            },
            string_table: 0,
            pc: 0,
            running: true,
            random: Randomizer::new(seed),
        };

        machine.restart()?;

        Ok(machine)
    }

    /// Whether execution has not yet been halted by quit.
    pub fn running(&self) -> bool {
        self.running
    }

    /// Return to the load state and call the start function.
    ///
    /// The protected range deliberately survives -- memory.reset
    /// honors it -- and execution begins by calling the header's
    /// start function with no arguments (Glulx: Game State, Glulx:
    /// The Header).
    pub fn restart(&mut self) -> Result<(), VoxamError> {
        // The heap goes first, before the map is rebuilt: it does
        // not survive a restart (Glulx: Memory Allocation Heap).
        self.heap.clear(&mut self.memory)?;
        self.memory.reset();
        self.stack.reset();
        self.iosys.reset();
        self.string_table = self.story.decoding_table();
        self.running = true;
        self.pc = funcs::push_call_frame(
            &self.memory,
            &mut self.stack,
            self.story.start_function(),
            &[],
        )?;

        Ok(())
    }

    /// Fetch, decode, and execute a single instruction.
    pub fn step(&mut self) -> Result<(), VoxamError> {
        let pc = self.pc;

        if pc >= self.memory.endmem() {
            return Err(VoxamError::GlulxMemory(format!(
                "execution ran off the memory map at ${pc:x} (Glulx: The Memory Map)"
            )));
        }

        let (opcode, pc) = decode_opcode(&self.memory, pc)?;

        let Some(oplist) = signature(opcode) else {
            // Every opcode Glulx 3.1.3 defines is dispatched, so an
            // unmatched number is one the spec has never named.
            return Err(instruction_error(format!(
                "executed opcode {}, which Glulx 3.1.3 does not define (Glulx: \
                 Dictionary of Opcodes)",
                name(opcode)
            )));
        };

        let (args, pc) = decode_operands(&self.memory, &mut self.stack, pc, &oplist)?;
        self.pc = pc;

        self.execute(opcode, &args)
    }

    /// Execute until the story quits; the step count comes back.
    ///
    /// The limit is a test and debugging guard, not a spec
    /// feature: a runaway loop in a broken story should fail
    /// rather than hang.
    pub fn run(&mut self, limit: Option<u64>) -> Result<u64, VoxamError> {
        let mut steps = 0;

        while self.running {
            if let Some(limit) = limit
                && steps >= limit
            {
                return Err(instruction_error(format!(
                    "execution exceeded {limit} instructions"
                )));
            }

            self.step()?;

            steps += 1;
        }

        Ok(steps)
    }

    /// Store through the operand machinery, at full width.
    pub(crate) fn store(&mut self, target: StoreTarget, value: u32) -> Result<(), VoxamError> {
        store(&mut self.memory, &mut self.stack, target, value, WORD_WIDTH)
    }

    fn store_width(
        &mut self,
        target: StoreTarget,
        value: u32,
        width: u32,
    ) -> Result<(), VoxamError> {
        store(&mut self.memory, &mut self.stack, target, value, width)
    }

    /// Branch by an offset -- or return 0 or 1 (Glulx: Branches).
    fn jump(&mut self, offset: u32) -> Result<(), VoxamError> {
        if offset == RETURN_ZERO_OFFSET || offset == RETURN_ONE_OFFSET {
            self.leave(offset)
        } else {
            // The pc already sits past the instruction, hence the
            // bias of two.
            self.pc = self.pc.wrapping_add(offset).wrapping_sub(BRANCH_ADJUSTMENT);

            Ok(())
        }
    }

    /// Leave the current function; an empty stack ends the story.
    fn leave(&mut self, value: u32) -> Result<(), VoxamError> {
        self.stack.leave_frame();

        if self.stack.sp == 0 {
            self.running = false;

            return Ok(());
        }

        self.pop_stub(value)
    }

    /// Pop a call stub and act on it (Glulx: Call Stubs).
    pub(crate) fn pop_stub(&mut self, value: u32) -> Result<(), VoxamError> {
        let stub = self.stack.pop_stub()?;
        self.pc = stub.pc;

        if stub.desttype == dest_type::RESUME_FUNCTION {
            return Err(instruction_error(
                "a string-terminator call stub arrived where a function result \
                 belongs (Glulx: Call Stubs)"
                    .into(),
            ));
        }

        if resumes_a_string(stub.desttype) {
            // A function called from inside a string has returned:
            // its value is discarded and the print picks up where
            // it left off (Glulx: Calling and Returning Within
            // Strings).
            return strings::resume(self, stub);
        }

        self.store(
            StoreTarget {
                desttype: stub.desttype,
                addr: stub.destaddr,
            },
            value,
        )
    }

    /// Push the come-home stub, then enter the function.
    fn call(&mut self, addr: u32, args: &[u32], target: StoreTarget) -> Result<(), VoxamError> {
        self.stack
            .push_stub(target.desttype, target.addr, self.pc)?;
        self.enter_function(addr, args)
    }

    /// Begin a call: every way of invoking a function lands here.
    ///
    /// This is what the spec means by a call including "any
    /// function invocation of that address" -- so the accelerated
    /// replacements intercept here, covering the call opcodes,
    /// tailcall, and the string-decoding table's function nodes
    /// alike (Glulx: Accelerated Functions). An accelerated
    /// function produces its result immediately, and the come-home
    /// stub the caller just pushed pops straight back off.
    pub(crate) fn enter_function(&mut self, addr: u32, args: &[u32]) -> Result<(), VoxamError> {
        if let Some(index) = self.accel.lookup(addr) {
            let value = self.accel.call(&self.memory, index, args)?;

            return self.pop_stub(value);
        }

        self.pc = funcs::push_call_frame(&self.memory, &mut self.stack, addr, args)?;

        Ok(())
    }

    /// A bit number resolved to its byte address and bit within.
    ///
    /// Bits number sequentially in both directions from the least
    /// significant bit of the base (Glulx: Array Data). Rust's
    /// arithmetic shift and two's-complement mask floor for
    /// negative operands, which is exactly that rule.
    fn bit_address(&self, base: u32, index: u32) -> (u32, u32) {
        let offset = signed(index);

        (
            base.wrapping_add((offset >> 3) as u32),
            (offset & BIT_INDEX_MASK) as u32,
        )
    }

    fn execute(&mut self, opcode: u32, args: &[Arg]) -> Result<(), VoxamError> {
        match opcode {
            op::NOP => Ok(()),

            // Integer math (Glulx: Integer Math).
            op::ADD => self.store(
                args[2].target(),
                args[0].value().wrapping_add(args[1].value()),
            ),
            op::SUB => self.store(
                args[2].target(),
                args[0].value().wrapping_sub(args[1].value()),
            ),
            op::MUL => self.store(
                args[2].target(),
                args[0].value().wrapping_mul(args[1].value()),
            ),
            op::DIV => {
                let value = divided(args[0].value(), args[1].value())?;

                self.store(args[2].target(), value)
            }
            op::MOD => {
                let value = remainder(args[0].value(), args[1].value())?;

                self.store(args[2].target(), value)
            }
            op::NEG => self.store(args[1].target(), 0u32.wrapping_sub(args[0].value())),
            op::BITAND => self.store(args[2].target(), args[0].value() & args[1].value()),
            op::BITOR => self.store(args[2].target(), args[0].value() | args[1].value()),
            op::BITXOR => self.store(args[2].target(), args[0].value() ^ args[1].value()),
            op::BITNOT => self.store(args[1].target(), !args[0].value()),
            op::SHIFTL => {
                // 32 places or more leave nothing (Glulx: Integer
                // Math).
                let places = signed(args[1].value());
                let value = if (0..SHIFT_LIMIT).contains(&places) {
                    args[0].value() << places
                } else {
                    0
                };

                self.store(args[2].target(), value)
            }
            op::USHIFTR => {
                // Shift right filling with zeros.
                let places = signed(args[1].value());
                let value = if (0..SHIFT_LIMIT).contains(&places) {
                    args[0].value() >> places
                } else {
                    0
                };

                self.store(args[2].target(), value)
            }
            op::SSHIFTR => {
                // Shift right replicating the sign bit -- Rust's
                // shift on a signed value, natively.
                let places = signed(args[1].value());
                let value = if (0..SHIFT_LIMIT).contains(&places) {
                    (signed(args[0].value()) >> places) as u32
                } else if args[0].value() & SIGN_BIT != 0 {
                    u32::MAX
                } else {
                    0
                };

                self.store(args[2].target(), value)
            }

            // Branches (Glulx: Branches).
            op::JUMP => self.jump(args[0].value()),
            op::JUMPABS => {
                // An absolute address, no bias, no return codes.
                self.pc = args[0].value();

                Ok(())
            }
            op::JZ => self.branch_if(args[0].value() == 0, args[1]),
            op::JNZ => self.branch_if(args[0].value() != 0, args[1]),
            op::JEQ => self.branch_if(args[0].value() == args[1].value(), args[2]),
            op::JNE => self.branch_if(args[0].value() != args[1].value(), args[2]),
            op::JLT => self.branch_if(signed(args[0].value()) < signed(args[1].value()), args[2]),
            op::JGE => self.branch_if(signed(args[0].value()) >= signed(args[1].value()), args[2]),
            op::JGT => self.branch_if(signed(args[0].value()) > signed(args[1].value()), args[2]),
            op::JLE => self.branch_if(signed(args[0].value()) <= signed(args[1].value()), args[2]),
            op::JLTU => self.branch_if(args[0].value() < args[1].value(), args[2]),
            op::JGEU => self.branch_if(args[0].value() >= args[1].value(), args[2]),
            op::JGTU => self.branch_if(args[0].value() > args[1].value(), args[2]),
            op::JLEU => self.branch_if(args[0].value() <= args[1].value(), args[2]),

            // Functions and continuations (Glulx: Calling and
            // Returning, Glulx: Continuations).
            op::CALL => {
                let call_args =
                    funcs::pop_arguments(&mut self.stack, args[1].value(), &self.memory, 0)?;

                self.call(args[0].value(), &call_args, args[2].target())
            }
            op::CALLF => self.call(args[0].value(), &[], args[1].target()),
            op::CALLFI => self.call(args[0].value(), &[args[1].value()], args[2].target()),
            op::CALLFII => self.call(
                args[0].value(),
                &[args[1].value(), args[2].value()],
                args[3].target(),
            ),
            op::CALLFIII => self.call(
                args[0].value(),
                &[args[1].value(), args[2].value(), args[3].value()],
                args[4].target(),
            ),
            op::RETURN => self.leave(args[0].value()),
            op::TAILCALL => {
                // Replace the frame without touching the stub
                // below it.
                let call_args =
                    funcs::pop_arguments(&mut self.stack, args[1].value(), &self.memory, 0)?;

                self.stack.leave_frame();
                self.enter_function(args[0].value(), &call_args)
            }
            op::CATCH => {
                // Push a stub, store its token, then branch -- the
                // spec's own order, which matters when either
                // lives on the stack (Glulx: Continuations).
                let target = args[0].target();

                self.stack
                    .push_stub(target.desttype, target.addr, self.pc)?;
                self.store(target, self.stack.sp)?;
                self.jump(args[1].value())
            }
            op::THROW => {
                // Unwind to a catch token and deliver a value
                // there.
                let (value, token) = (args[0].value(), args[1].value());

                if token % WORD_WIDTH != 0 || token > self.stack.size() {
                    return Err(instruction_error(format!(
                        "a throw's catch token of {token} is not a place on this \
                         stack (Glulx: Continuations)"
                    )));
                }

                self.stack.sp = token;

                self.pop_stub(value)
            }

            // Moving data and array data (Glulx: Array Data).
            op::COPY => self.store(args[1].target(), args[0].value()),
            op::COPYS => self.store_width(args[1].target(), args[0].value(), 2),
            op::COPYB => self.store_width(args[1].target(), args[0].value(), 1),
            op::SEXS => self.store(args[1].target(), sign_extend(args[0].value(), 16)),
            op::SEXB => self.store(args[1].target(), sign_extend(args[0].value(), 8)),
            op::ALOAD => {
                let value = self.memory.read_word(
                    args[0]
                        .value()
                        .wrapping_add(args[1].value().wrapping_mul(4)),
                )?;

                self.store(args[2].target(), value)
            }
            op::ALOADS => {
                let value = self.memory.read_short(
                    args[0]
                        .value()
                        .wrapping_add(args[1].value().wrapping_mul(2)),
                )?;

                self.store(args[2].target(), u32::from(value))
            }
            op::ALOADB => {
                let value = self
                    .memory
                    .read_byte(args[0].value().wrapping_add(args[1].value()))?;

                self.store(args[2].target(), u32::from(value))
            }
            op::ALOADBIT => {
                let (addr, bit) = self.bit_address(args[0].value(), args[1].value());
                let value = u32::from(self.memory.read_byte(addr)? & (1 << bit) != 0);

                self.store(args[2].target(), value)
            }
            op::ASTORE => self.memory.write_word(
                args[0]
                    .value()
                    .wrapping_add(args[1].value().wrapping_mul(4)),
                args[2].value(),
            ),
            op::ASTORES => self.memory.write_short(
                args[0]
                    .value()
                    .wrapping_add(args[1].value().wrapping_mul(2)),
                args[2].value() as u16,
            ),
            op::ASTOREB => self.memory.write_byte(
                args[0].value().wrapping_add(args[1].value()),
                args[2].value() as u8,
            ),
            op::ASTOREBIT => {
                let (addr, bit) = self.bit_address(args[0].value(), args[1].value());
                let mut value = self.memory.read_byte(addr)?;

                if args[2].value() != 0 {
                    value |= 1 << bit;
                } else {
                    value &= !(1 << bit);
                }

                self.memory.write_byte(addr, value)
            }

            // The stack (Glulx: The Stack).
            op::STKCOUNT => {
                let count = self.stack.count();

                self.store(args[0].target(), count)
            }
            op::STKPEEK => {
                // Peek by index; the index must name a value that
                // exists.
                let index = signed(args[0].value());

                if index < 0 || index as u32 >= self.stack.count() {
                    return Err(instruction_error(format!(
                        "stkpeek at {index} reaches outside the current stack range \
                         (Glulx: The Stack)"
                    )));
                }

                let value = self.stack.peek(index as u32)?;

                self.store(args[1].target(), value)
            }
            op::STKSWAP => {
                if self.stack.count() < 2 {
                    return Err(instruction_error(
                        "stkswap with fewer than two values (Glulx: The Stack)".into(),
                    ));
                }

                let top = self.stack.pop()?;
                let below = self.stack.pop()?;

                self.stack.push(top)?;
                self.stack.push(below)
            }
            op::STKCOPY => {
                let count = signed(args[0].value());

                if count < 0 {
                    return Err(instruction_error(
                        "stkcopy with a negative count (Glulx: The Stack)".into(),
                    ));
                }

                let count = count as u32;

                if count == 0 {
                    return Ok(());
                }

                if self.stack.count() < count {
                    return Err(instruction_error(format!(
                        "stkcopy of {count} exceeds the values above the frame"
                    )));
                }

                let values: Vec<u32> = (0..count)
                    .map(|at| self.stack.peek(count - 1 - at))
                    .collect::<Result<_, _>>()?;

                for value in values {
                    self.stack.push(value)?;
                }

                Ok(())
            }
            op::STKROLL => {
                // Rotate the top values by places, either
                // direction; (-places).rem_euclid(count) is the
                // rotate-down distance for either sign (Glulx: The
                // Stack).
                let (count, places) = (signed(args[0].value()), signed(args[1].value()));

                if count < 0 {
                    return Err(instruction_error(
                        "stkroll with a negative count (Glulx: The Stack)".into(),
                    ));
                }

                if self.stack.count() < count as u32 {
                    return Err(instruction_error(format!(
                        "stkroll of {count} exceeds the values above the frame"
                    )));
                }

                if count == 0 {
                    return Ok(());
                }

                let shift = (-places).rem_euclid(count) as usize;

                if shift == 0 {
                    return Ok(());
                }

                let count = count as u32;
                let base = self.stack.sp - WORD_WIDTH * count;
                let mut values: Vec<u32> = (0..count)
                    .map(|at| self.stack.read_word(base + WORD_WIDTH * at))
                    .collect::<Result<_, _>>()?;

                values.rotate_left(shift);

                for (at, value) in values.into_iter().enumerate() {
                    self.stack
                        .write_word(base + WORD_WIDTH * at as u32, value)?;
                }

                Ok(())
            }

            // Output (Glulx: Output).
            op::STREAMCHAR => {
                // One character, its low byte.
                strings::put_char(self, args[0].value() & 0xFF)
            }
            op::STREAMUNICHAR => strings::put_char(self, args[0].value()),
            op::STREAMNUM => strings::stream_num(self, args[0].value(), false, 0),
            op::STREAMSTR => strings::stream_string(self, args[0].value(), 0, 0),
            op::GETSTRINGTBL => {
                let table = self.string_table;

                self.store(args[0].target(), table)
            }
            op::SETSTRINGTBL => {
                // The address is taken on trust, exactly as the
                // spec allows: a broken table announces itself at
                // the next compressed print, not here.
                self.string_table = args[0].value();

                Ok(())
            }
            op::GETIOSYS => {
                let (mode, rock) = (self.iosys.mode, self.iosys.rock);

                self.store(args[0].target(), mode)?;
                self.store(args[1].target(), rock)
            }
            op::SETIOSYS => {
                // Selecting an unsupported system selects the null
                // system -- and Glk without a library installed is
                // exactly that, so the fallback here tells the
                // same truth the gestalt answer does.
                let (mut mode, mut rock) = (args[0].value(), args[1].value());

                if mode == io_mode::GLK {
                    (mode, rock) = (io_mode::NULL, 0);
                }

                self.iosys.select(mode, rock);

                Ok(())
            }

            // Miscellaneous (Glulx: Miscellaneous).
            op::GESTALT => {
                let answer = gestalt::answer(
                    &self.capabilities,
                    self.heap.start,
                    &AVAILABLE,
                    args[0].value(),
                    args[1].value(),
                );

                self.store(args[2].target(), answer)
            }
            op::GLK => {
                // The opcode always functions when a library is
                // installed -- and none is, yet.
                Err(VoxamError::GlulxGlk(
                    "the glk opcode needs a Glk library, and none is installed".into(),
                ))
            }
            op::DEBUGTRAP => {
                // Halt loudly: this interpreter has no debugger to
                // hand off to, and the spec directs one without a
                // debugging faculty to treat the value as a fatal
                // error and print it.
                Err(instruction_error(format!(
                    "debugtrap with value {} (Glulx: Miscellaneous)",
                    args[0].value()
                )))
            }

            // The memory map (Glulx: Game State).
            op::GETMEMSIZE => {
                let endmem = self.memory.endmem();

                self.store(args[0].target(), endmem)
            }
            op::SETMEMSIZE => {
                if self.heap.active() {
                    return Err(VoxamError::GlulxMemory(
                        "setmemsize is illegal while the allocation heap is active".into(),
                    ));
                }

                self.memory.set_size(args[0].value())?;
                self.store(args[1].target(), 0)
            }
            op::MZERO => self.memory.fill(args[1].value(), args[0].value(), 0),
            op::MCOPY => self
                .memory
                .copy(args[2].value(), args[1].value(), args[0].value()),
            op::PROTECT => {
                self.memory.set_protection(args[0].value(), args[1].value());

                Ok(())
            }

            // The random number generator (Glulx: The Random
            // Number Generator).
            op::RANDOM => {
                // A zero range asks for a full 32-bit value; a
                // positive one for 0 through the range less one; a
                // negative one for the mirror: range plus one
                // through 0.
                let limit = signed(args[0].value());

                let value = if limit == 0 {
                    self.random.word()
                } else if limit > 0 {
                    self.random.below(limit as u32)
                } else {
                    0u32.wrapping_sub(self.random.below(limit.unsigned_abs()))
                };

                self.store(args[1].target(), value)
            }
            op::SETRANDOM => {
                // Zero asks for genuine unpredictability.
                self.random.seed(args[0].value());

                Ok(())
            }

            // Game state (Glulx: Game State).
            op::QUIT => {
                self.running = false;

                Ok(())
            }
            op::VERIFY => {
                // Recompute the checksum: 0 for sound, 1 for not.
                let answer = if self.story.verify() { 0 } else { 1 };

                self.store(args[0].target(), answer)
            }
            op::RESTART => self.restart(),
            op::SAVE => {
                // The call stub is pushed first, so it lands
                // inside the save's own stack chunk; with no Glk
                // stream to write to, the spoken result is the
                // failure (Glulx: Game State).
                let target = args[1].target();

                self.stack
                    .push_stub(target.desttype, target.addr, self.pc)?;

                self.pop_stub(serial::FAILED)
            }
            op::RESTORE => {
                // With no Glk stream to read from, failure speaks
                // 1 in place.
                self.store(args[1].target(), serial::FAILED)
            }
            op::SAVEUNDO => {
                // The stub lands inside the saved stack, so that
                // after a later restoreundo the same stub stores
                // -1 and execution continues from this very
                // instruction.
                let target = args[0].target();

                self.stack
                    .push_stub(target.desttype, target.addr, self.pc)?;

                let result = serial::save_undo(self)?;

                self.pop_stub(result)
            }
            op::RESTOREUNDO => {
                let result = serial::restore_undo(self)?;

                if result == serial::SUCCEEDED {
                    self.pop_stub(RESTORED)
                } else {
                    self.store(args[0].target(), result)
                }
            }
            op::HASUNDO => {
                // Whether an undo state waits: 0 yes, 1 no.
                let answer = serial::has_undo(self);

                self.store(args[0].target(), answer)
            }
            op::DISCARDUNDO => {
                serial::discard_undo(self);

                Ok(())
            }

            // Searching (Glulx: Searching).
            op::LINEARSEARCH => {
                let found = search::linear_search(
                    &self.memory,
                    args[0].value(),
                    args[1].value(),
                    args[2].value(),
                    args[3].value(),
                    args[4].value(),
                    args[5].value(),
                    args[6].value(),
                )?;

                self.store(args[7].target(), found)
            }
            op::BINARYSEARCH => {
                let found = search::binary_search(
                    &self.memory,
                    args[0].value(),
                    args[1].value(),
                    args[2].value(),
                    args[3].value(),
                    args[4].value(),
                    args[5].value(),
                    args[6].value(),
                )?;

                self.store(args[7].target(), found)
            }
            op::LINKEDSEARCH => {
                let found = search::linked_search(
                    &self.memory,
                    args[0].value(),
                    args[1].value(),
                    args[2].value(),
                    args[3].value(),
                    args[4].value(),
                    args[5].value(),
                )?;

                self.store(args[6].target(), found)
            }

            // Block copy and clear arrived with mzero/mcopy above;
            // the memory allocation heap (Glulx: Memory Allocation
            // Heap).
            op::MALLOC => {
                // The address stores, or zero for a refusal --
                // allocation is never guaranteed.
                let address = self.heap.alloc(&mut self.memory, args[0].value())?;

                self.store(args[1].target(), address)
            }
            op::MFREE => self.heap.free(&mut self.memory, args[0].value()),

            // Accelerated functions (Glulx: Accelerated
            // Functions).
            op::ACCELFUNC => {
                self.accel.set_func(args[0].value(), args[1].value());

                Ok(())
            }
            op::ACCELPARAM => {
                self.accel.set_param(args[0].value(), args[1].value());

                Ok(())
            }

            // Floating-point math (Glulx: Floating-Point Math).
            op::NUMTOF => {
                // A signed integer becomes the nearest single.
                let value = f64::from(signed(args[0].value()));

                self.store(args[1].target(), encode_float(value))
            }
            op::FTONUMZ => self.store(
                args[1].target(),
                to_int(decode_float(args[0].value()), false),
            ),
            op::FTONUMN => self.store(
                args[1].target(),
                to_int(decode_float(args[0].value()), true),
            ),
            op::CEIL => self.float_unary(args, f64::ceil),
            op::FLOOR => self.float_unary(args, f64::floor),
            op::FADD => self.float_binary(args, |a, b| a + b),
            op::FSUB => self.float_binary(args, |a, b| a - b),
            op::FMUL => self.float_binary(args, |a, b| a * b),
            op::FDIV => self.float_binary(args, |a, b| a / b),
            op::FMOD => {
                // Remainder and quotient at once; a zero quotient
                // has lost its sign in the arithmetic, so the
                // reference recovers it from the arguments' signs.
                let (a, b) = (args[0].value(), args[1].value());
                let (rem, quot) = modulo(decode_float(a), decode_float(b));
                let mut encoded = encode_float(quot);

                if encoded == 0 || encoded == SIGN_BIT {
                    encoded = (a ^ b) & SIGN_BIT;
                }

                self.store(args[2].target(), encode_float(rem))?;
                self.store(args[3].target(), encoded)
            }
            op::SQRT => self.float_unary(args, f64::sqrt),
            op::EXP => self.float_unary(args, f64::exp),
            op::LOG => self.float_unary(args, f64::ln),
            op::POW => self.float_binary(args, pow),
            op::SIN => self.float_unary(args, f64::sin),
            op::COS => self.float_unary(args, f64::cos),
            op::TAN => self.float_unary(args, f64::tan),
            op::ASIN => self.float_unary(args, f64::asin),
            op::ACOS => self.float_unary(args, f64::acos),
            op::ATAN => self.float_unary(args, f64::atan),
            op::ATAN2 => self.float_binary(args, f64::atan2),

            // Floating-point comparisons (Glulx: Floating-Point
            // Comparisons).
            op::JFEQ => {
                let holds = close(
                    decode_float(args[0].value()),
                    decode_float(args[1].value()),
                    decode_float(args[2].value()),
                );

                self.branch_if(holds, args[3])
            }
            op::JFNE => {
                // The reverse of jfeq, so any NaN branches.
                let holds = close(
                    decode_float(args[0].value()),
                    decode_float(args[1].value()),
                    decode_float(args[2].value()),
                );

                self.branch_if(!holds, args[3])
            }
            op::JFLT => self.float_compare(args, |a, b| a < b),
            op::JFLE => self.float_compare(args, |a, b| a <= b),
            op::JFGT => self.float_compare(args, |a, b| a > b),
            op::JFGE => self.float_compare(args, |a, b| a >= b),
            op::JISNAN => self.branch_if(decode_float(args[0].value()).is_nan(), args[1]),
            op::JISINF => self.branch_if(decode_float(args[0].value()).is_infinite(), args[1]),

            // Double-precision math (Glulx: Double-Precision
            // Math).
            op::NUMTOD => {
                // A signed integer becomes a double, exactly.
                let (high, low) = encode_double(f64::from(signed(args[0].value())));

                self.store(args[1].target(), low)?;
                self.store(args[2].target(), high)
            }
            op::DTONUMZ => {
                let value = to_int(decode_double(args[0].value(), args[1].value()), false);

                self.store(args[2].target(), value)
            }
            op::DTONUMN => {
                let value = to_int(decode_double(args[0].value(), args[1].value()), true);

                self.store(args[2].target(), value)
            }
            op::FTOD => {
                // Every single widens exactly.
                let (high, low) = encode_double(decode_float(args[0].value()));

                self.store(args[1].target(), low)?;
                self.store(args[2].target(), high)
            }
            op::DTOF => {
                // A double narrows, rounding to the nearest
                // single.
                let value = encode_float(decode_double(args[0].value(), args[1].value()));

                self.store(args[2].target(), value)
            }
            op::DCEIL => self.double_unary(args, f64::ceil),
            op::DFLOOR => self.double_unary(args, f64::floor),
            op::DADD => self.double_binary(args, |a, b| a + b),
            op::DSUB => self.double_binary(args, |a, b| a - b),
            op::DMUL => self.double_binary(args, |a, b| a * b),
            op::DDIV => self.double_binary(args, |a, b| a / b),
            op::DMODR => self.double_mod(args, false),
            op::DMODQ => self.double_mod(args, true),
            op::DSQRT => self.double_unary(args, f64::sqrt),
            op::DEXP => self.double_unary(args, f64::exp),
            op::DLOG => self.double_unary(args, f64::ln),
            op::DPOW => self.double_binary(args, pow),
            op::DSIN => self.double_unary(args, f64::sin),
            op::DCOS => self.double_unary(args, f64::cos),
            op::DTAN => self.double_unary(args, f64::tan),
            op::DASIN => self.double_unary(args, f64::asin),
            op::DACOS => self.double_unary(args, f64::acos),
            op::DATAN => self.double_unary(args, f64::atan),
            op::DATAN2 => self.double_binary(args, f64::atan2),

            // Double-precision comparisons (Glulx: Double-
            // Precision Comparisons).
            op::JDEQ => {
                let holds = close(
                    decode_double(args[0].value(), args[1].value()),
                    decode_double(args[2].value(), args[3].value()),
                    decode_double(args[4].value(), args[5].value()),
                );

                self.branch_if(holds, args[6])
            }
            op::JDNE => {
                // The reverse of jdeq, so any NaN branches.
                let holds = close(
                    decode_double(args[0].value(), args[1].value()),
                    decode_double(args[2].value(), args[3].value()),
                    decode_double(args[4].value(), args[5].value()),
                );

                self.branch_if(!holds, args[6])
            }
            op::JDLT => self.double_compare(args, |a, b| a < b),
            op::JDLE => self.double_compare(args, |a, b| a <= b),
            op::JDGT => self.double_compare(args, |a, b| a > b),
            op::JDGE => self.double_compare(args, |a, b| a >= b),
            op::JDISNAN => {
                let value = decode_double(args[0].value(), args[1].value());

                self.branch_if(value.is_nan(), args[2])
            }
            op::JDISINF => {
                let value = decode_double(args[0].value(), args[1].value());

                self.branch_if(value.is_infinite(), args[2])
            }

            // The signature match already refused everything else.
            _ => unreachable!("an opcode with a signature but no handler"),
        }
    }

    fn branch_if(&mut self, condition: bool, offset: Arg) -> Result<(), VoxamError> {
        if condition {
            self.jump(offset.value())
        } else {
            Ok(())
        }
    }

    // -- the float combinators, mirroring the reference's ------------------
    //
    // Doubles arrive high word first and store low word first
    // (Glulx: Double-Precision Math). A NaN operand passes
    // straight through the unary operations, bits and all, which
    // is what IEEE 754 says of a quiet NaN -- applied before the
    // function is called so no library's NaN-sign quirks leak
    // through.

    fn float_unary(&mut self, args: &[Arg], function: fn(f64) -> f64) -> Result<(), VoxamError> {
        let bits = args[0].value();
        let value = decode_float(bits);

        if value.is_nan() {
            return self.store(args[1].target(), bits);
        }

        self.store(args[1].target(), encode_float(function(value)))
    }

    fn float_binary(
        &mut self,
        args: &[Arg],
        function: fn(f64, f64) -> f64,
    ) -> Result<(), VoxamError> {
        let result = function(decode_float(args[0].value()), decode_float(args[1].value()));

        self.store(args[2].target(), encode_float(result))
    }

    fn float_compare(
        &mut self,
        args: &[Arg],
        test: fn(f64, f64) -> bool,
    ) -> Result<(), VoxamError> {
        let holds = test(decode_float(args[0].value()), decode_float(args[1].value()));

        self.branch_if(holds, args[2])
    }

    fn double_unary(&mut self, args: &[Arg], function: fn(f64) -> f64) -> Result<(), VoxamError> {
        let value = decode_double(args[0].value(), args[1].value());

        let (high, low) = if value.is_nan() {
            (args[0].value(), args[1].value())
        } else {
            encode_double(function(value))
        };

        self.store(args[2].target(), low)?;
        self.store(args[3].target(), high)
    }

    fn double_binary(
        &mut self,
        args: &[Arg],
        function: fn(f64, f64) -> f64,
    ) -> Result<(), VoxamError> {
        let result = function(
            decode_double(args[0].value(), args[1].value()),
            decode_double(args[2].value(), args[3].value()),
        );
        let (high, low) = encode_double(result);

        self.store(args[4].target(), low)?;
        self.store(args[5].target(), high)
    }

    fn double_compare(
        &mut self,
        args: &[Arg],
        test: fn(f64, f64) -> bool,
    ) -> Result<(), VoxamError> {
        let holds = test(
            decode_double(args[0].value(), args[1].value()),
            decode_double(args[2].value(), args[3].value()),
        );

        self.branch_if(holds, args[4])
    }

    /// The engine of dmodr and dmodq: remainder or quotient. As in
    /// fmod, a zero quotient takes its sign from the arguments.
    fn double_mod(&mut self, args: &[Arg], quotient_wanted: bool) -> Result<(), VoxamError> {
        let (rem, quot) = modulo(
            decode_double(args[0].value(), args[1].value()),
            decode_double(args[2].value(), args[3].value()),
        );
        let (mut high, low) = encode_double(if quotient_wanted { quot } else { rem });

        if quotient_wanted && low == 0 && (high == 0 || high == SIGN_BIT) {
            high = (args[0].value() ^ args[2].value()) & SIGN_BIT;
        }

        self.store(args[4].target(), low)?;
        self.store(args[5].target(), high)
    }
}

/// An opcode's operand signature, or None for a number the spec
/// does not define (Glulx: Dictionary of Opcodes).
fn signature(opcode: u32) -> Option<OperandList> {
    const NONE: OperandList = operands("", 4);
    const L: OperandList = operands("L", 4);
    const LL: OperandList = operands("LL", 4);
    const LLL: OperandList = operands("LLL", 4);
    const LLLL: OperandList = operands("LLLL", 4);
    const LLLLL: OperandList = operands("LLLLL", 4);
    const LLLLLLL: OperandList = operands("LLLLLLL", 4);
    const S: OperandList = operands("S", 4);
    const SS: OperandList = operands("SS", 4);
    const LS: OperandList = operands("LS", 4);
    const SL: OperandList = operands("SL", 4);
    const LSS: OperandList = operands("LSS", 4);
    const LLS: OperandList = operands("LLS", 4);
    const LLSS: OperandList = operands("LLSS", 4);
    const LLLS: OperandList = operands("LLLS", 4);
    const LLLLS: OperandList = operands("LLLLS", 4);
    const LLLLSS: OperandList = operands("LLLLSS", 4);
    const LLLLLLS: OperandList = operands("LLLLLLS", 4);
    const LLLLLLLS: OperandList = operands("LLLLLLLS", 4);
    const LS_SHORT: OperandList = operands("LS", 2);
    const LS_BYTE: OperandList = operands("LS", 1);

    Some(match opcode {
        op::NOP | op::STKSWAP | op::QUIT | op::RESTART | op::DISCARDUNDO => NONE,

        op::JUMP
        | op::JUMPABS
        | op::RETURN
        | op::STREAMCHAR
        | op::STREAMUNICHAR
        | op::STREAMNUM
        | op::STREAMSTR
        | op::SETSTRINGTBL
        | op::SETRANDOM
        | op::MFREE
        | op::STKCOPY
        | op::DEBUGTRAP => L,

        op::JZ
        | op::JNZ
        | op::TAILCALL
        | op::THROW
        | op::SETIOSYS
        | op::ACCELFUNC
        | op::ACCELPARAM
        | op::MZERO
        | op::PROTECT
        | op::STKROLL
        | op::JISNAN
        | op::JISINF => LL,

        op::JEQ
        | op::JNE
        | op::JLT
        | op::JGE
        | op::JGT
        | op::JLE
        | op::JLTU
        | op::JGEU
        | op::JGTU
        | op::JLEU
        | op::ASTORE
        | op::ASTORES
        | op::ASTOREB
        | op::ASTOREBIT
        | op::MCOPY
        | op::JFLT
        | op::JFLE
        | op::JFGT
        | op::JFGE
        | op::JDISNAN
        | op::JDISINF => LLL,

        op::JFEQ | op::JFNE => LLLL,
        op::JDLT | op::JDLE | op::JDGT | op::JDGE => LLLLL,
        op::JDEQ | op::JDNE => LLLLLLL,

        op::GETSTRINGTBL
        | op::STKCOUNT
        | op::GETMEMSIZE
        | op::VERIFY
        | op::SAVEUNDO
        | op::RESTOREUNDO
        | op::HASUNDO => S,

        op::GETIOSYS => SS,
        op::CATCH => SL,

        op::CALLF
        | op::NEG
        | op::BITNOT
        | op::SEXS
        | op::SEXB
        | op::COPY
        | op::STKPEEK
        | op::MALLOC
        | op::SAVE
        | op::RESTORE
        | op::RANDOM
        | op::SETMEMSIZE
        | op::NUMTOF
        | op::FTONUMZ
        | op::FTONUMN
        | op::CEIL
        | op::FLOOR
        | op::SQRT
        | op::EXP
        | op::LOG
        | op::SIN
        | op::COS
        | op::TAN
        | op::ASIN
        | op::ACOS
        | op::ATAN => LS,

        op::COPYS => LS_SHORT,
        op::COPYB => LS_BYTE,

        op::NUMTOD | op::FTOD => LSS,

        op::ADD
        | op::SUB
        | op::MUL
        | op::DIV
        | op::MOD
        | op::BITAND
        | op::BITOR
        | op::BITXOR
        | op::SHIFTL
        | op::SSHIFTR
        | op::USHIFTR
        | op::CALL
        | op::CALLFI
        | op::ALOAD
        | op::ALOADS
        | op::ALOADB
        | op::ALOADBIT
        | op::GLK
        | op::GESTALT
        | op::FADD
        | op::FSUB
        | op::FMUL
        | op::FDIV
        | op::POW
        | op::ATAN2
        | op::DTONUMZ
        | op::DTONUMN
        | op::DTOF => LLS,

        op::FMOD
        | op::DCEIL
        | op::DFLOOR
        | op::DSQRT
        | op::DEXP
        | op::DLOG
        | op::DSIN
        | op::DCOS
        | op::DTAN
        | op::DASIN
        | op::DACOS
        | op::DATAN => LLSS,

        op::CALLFII => LLLS,
        op::CALLFIII => LLLLS,

        op::DADD
        | op::DSUB
        | op::DMUL
        | op::DDIV
        | op::DMODR
        | op::DMODQ
        | op::DPOW
        | op::DATAN2 => LLLLSS,

        op::LINKEDSEARCH => LLLLLLS,
        op::LINEARSEARCH | op::BINARYSEARCH => LLLLLLLS,

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glulx::testing::{image, image_with_checksum};

    const BOOT_PC: u32 = 0x4B;
    const PLANT: u32 = 0x180;
    const RESULT: u32 = 0x140;

    // A do-nothing start function: C0, no locals, quit.
    const IDLE: &[u8] = &[0xC0, 0x00, 0x00, 0x81, 0x20];

    // The word-mode store target every plant writes its answer to.
    const TO_RESULT: &[u8] = &[0x00, 0x00, 0x01, 0x40];

    fn boot(code: &[u8]) -> Machine {
        Machine::new(Story::new(image(code)).unwrap(), None).unwrap()
    }

    /// Write one instruction into RAM and step the machine over
    /// it.
    fn planted(machine: &mut Machine, code: &[u8]) -> Result<(), VoxamError> {
        machine.memory.write_run(PLANT, code).unwrap();
        machine.pc = PLANT;

        machine.step()
    }

    fn result(machine: &Machine) -> u32 {
        machine.memory.read_word(RESULT).unwrap()
    }

    // Boot calls the header's start function with no arguments:
    // the frame stands and the pc rests on the first instruction.
    #[test]
    fn boot_calls_the_start_function() {
        let machine = boot(IDLE);

        assert_eq!(machine.pc, BOOT_PC);
        assert_eq!(machine.stack.frameptr, 0);
        assert!(machine.running());
    }

    // The smallest whole story: add two constants into memory and
    // quit. Two instructions, one answer, a stopped machine.
    #[test]
    fn a_story_runs_to_quit() {
        let mut program = vec![0xC0, 0x00, 0x00, 0x10, 0x11, 0x07, 0x03, 0x04];
        program.extend_from_slice(TO_RESULT);
        program.extend_from_slice(&[0x81, 0x20]);

        let mut machine = boot(&program);

        assert_eq!(machine.run(None).unwrap(), 2);
        assert_eq!(result(&machine), 7);
        assert!(!machine.running());
    }

    // callfi carries one argument into a C1 function, whose return
    // value comes home through the call stub to the caller's
    // target.
    #[test]
    fn calls_return_through_their_stubs() {
        let mut main = vec![0xC0, 0x00, 0x00];
        main.extend_from_slice(&[0x81, 0x61, 0x13, 0x07, 0x00, 0x00, 0x00, 0x60, 0x05]);
        main.extend_from_slice(TO_RESULT);
        main.extend_from_slice(&[0x81, 0x20]);

        let func: &[u8] = &[
            0xC1, 0x04, 0x01, 0x00, 0x00, 0x10, 0x19, 0x08, 0x00, 0x01, 0x31, 0x08,
        ];

        let mut code = main.clone();
        code.resize(24, 0);
        code.extend_from_slice(func);

        let mut machine = boot(&code);

        machine.run(None).unwrap();

        assert_eq!(result(&machine), 6);

        // The general call takes its arguments off the stack; the
        // two- and three-argument conveniences carry theirs
        // inline. The one-local function drops the extras
        // silently.
        let mut varied = boot(&code);
        let word_func: &[u8] = &[0x00, 0x00, 0x00, 0x60];

        // The call is one step; the two-instruction callee is two
        // more before its return brings the result home.
        fn called_home(machine: &mut Machine, plant: &[u8]) {
            machine.memory.write_run(PLANT, plant).unwrap();
            machine.pc = PLANT;

            machine.step().unwrap();
            machine.step().unwrap();
            machine.step().unwrap();
        }

        varied.stack.push(5).unwrap();

        let mut plant = vec![0x30, 0x13, 0x07];
        plant.extend_from_slice(word_func);
        plant.push(0x01);
        plant.extend_from_slice(TO_RESULT);
        called_home(&mut varied, &plant);

        assert_eq!(result(&varied), 6);

        varied.memory.write_word(RESULT, 0).unwrap();

        let mut plant = vec![0x81, 0x62, 0x13, 0x71];
        plant.extend_from_slice(word_func);
        plant.extend_from_slice(&[0x05, 0x09]);
        plant.extend_from_slice(TO_RESULT);
        called_home(&mut varied, &plant);

        assert_eq!(result(&varied), 6);

        varied.memory.write_word(RESULT, 0).unwrap();

        let mut plant = vec![0x81, 0x63, 0x13, 0x11, 0x07];
        plant.extend_from_slice(word_func);
        plant.extend_from_slice(&[0x05, 0x09, 0x02]);
        plant.extend_from_slice(TO_RESULT);
        called_home(&mut varied, &plant);

        assert_eq!(result(&varied), 6);
    }

    // tailcall replaces the frame without touching the stub below
    // it: the tail-called function's return lands in the ORIGINAL
    // caller's target, one stub for two calls.
    #[test]
    fn tailcall_replaces_the_frame() {
        let mut main = vec![0xC0, 0x00, 0x00];
        main.extend_from_slice(&[0x81, 0x60, 0x73, 0x00, 0x00, 0x00, 0x60]);
        main.extend_from_slice(TO_RESULT);
        main.extend_from_slice(&[0x81, 0x20]);

        let first: &[u8] = &[
            0xC0, 0x00, 0x00, 0x40, 0x81, 0x09, 0x34, 0x13, 0x00, 0x00, 0x00, 0x70, 0x01,
        ];
        let second: &[u8] = &[
            0xC1, 0x04, 0x01, 0x00, 0x00, 0x10, 0x19, 0x08, 0x00, 0x01, 0x31, 0x08,
        ];

        let mut code = main.clone();
        code.resize(24, 0);
        code.extend_from_slice(first);
        code.resize(40, 0);
        code.extend_from_slice(second);

        let mut machine = boot(&code);

        machine.run(None).unwrap();

        assert_eq!(result(&machine), 10);
    }

    // jump skips the debugtrap it is aimed over; a branch offset
    // of 1 is not a jump but a return, which at the top level ends
    // the story (Glulx: Branches). jumpabs takes its address
    // whole.
    #[test]
    fn branches_jump_and_return() {
        let jumper: &[u8] = &[
            0xC0, 0x00, 0x00, 0x20, 0x01, 0x06, 0x81, 0x01, 0x01, 0x07, 0x81, 0x20,
        ];
        let mut machine = boot(jumper);

        assert_eq!(machine.run(None).unwrap(), 2);

        let mut returner = boot(&[0xC0, 0x00, 0x00, 0x22, 0x11, 0x00, 0x01]);

        assert_eq!(returner.run(None).unwrap(), 1);

        let mut absolute = boot(&[
            0xC0, 0x00, 0x00, 0x81, 0x04, 0x03, 0x00, 0x00, 0x00, 0x52, 0x81, 0x20,
        ]);

        assert_eq!(absolute.run(None).unwrap(), 2);

        // The same opcode in its four-byte dress, and a branch
        // offset of 0: the other return code.
        let mut long_form = boot(&[
            0xC0, 0x00, 0x00, 0xC0, 0x00, 0x01, 0x04, 0x03, 0x00, 0x00, 0x00, 0x55, 0x81, 0x20,
        ]);

        assert_eq!(long_form.run(None).unwrap(), 2);

        let mut zero_return = boot(&[0xC0, 0x00, 0x00, 0x22, 0x11, 0x00, 0x00]);

        assert_eq!(zero_return.run(None).unwrap(), 1);
    }

    // Every conditional branch fires on its own comparison --
    // signed where the spec says signed, unsigned where it says
    // unsigned.
    #[test]
    fn conditional_branches_compare_their_way() {
        // Each plant branches with offset 1 -- return -- so a
        // taken branch empties the stack and stops the machine;
        // reboot after.
        let taken: &[&[u8]] = &[
            &[0x22, 0x11, 0x00, 0x01],
            &[0x23, 0x11, 0x05, 0x01],
            &[0x24, 0x11, 0x01, 0x07, 0x07, 0x01],
            &[0x25, 0x11, 0x01, 0x07, 0x08, 0x01],
            &[0x26, 0x11, 0x01, 0xFF, 0x02, 0x01],
            &[0x27, 0x11, 0x01, 0x02, 0xFF, 0x01],
            &[0x28, 0x11, 0x01, 0x02, 0xFF, 0x01],
            &[0x29, 0x11, 0x01, 0xFF, 0x02, 0x01],
            &[0x2A, 0x11, 0x01, 0x02, 0xFF, 0x01],
            &[0x2B, 0x11, 0x01, 0xFF, 0x02, 0x01],
            &[0x2C, 0x11, 0x01, 0xFF, 0x02, 0x01],
            &[0x2D, 0x11, 0x01, 0x02, 0xFF, 0x01],
        ];

        for plant in taken {
            let mut machine = boot(IDLE);

            planted(&mut machine, plant).unwrap();

            assert!(!machine.running(), "{plant:?}");
        }

        // And every untaken side: the condition fails, the branch
        // stays home, and the machine keeps running.
        let untaken: &[&[u8]] = &[
            &[0x22, 0x11, 0x05, 0x01],
            &[0x23, 0x11, 0x00, 0x01],
            &[0x24, 0x11, 0x01, 0x07, 0x08, 0x01],
            &[0x25, 0x11, 0x01, 0x07, 0x07, 0x01],
            &[0x26, 0x11, 0x01, 0x02, 0xFF, 0x01],
            &[0x27, 0x11, 0x01, 0xFF, 0x02, 0x01],
            &[0x28, 0x11, 0x01, 0xFF, 0x02, 0x01],
            &[0x29, 0x11, 0x01, 0x02, 0xFF, 0x01],
            &[0x2A, 0x11, 0x01, 0xFF, 0x02, 0x01],
            &[0x2B, 0x11, 0x01, 0x02, 0xFF, 0x01],
            &[0x2C, 0x11, 0x01, 0x02, 0xFF, 0x01],
            &[0x2D, 0x11, 0x01, 0xFF, 0x02, 0x01],
        ];

        for plant in untaken {
            let mut quiet = boot(IDLE);

            planted(&mut quiet, plant).unwrap();

            assert!(quiet.running(), "{plant:?}");
        }
    }

    // Division truncates toward zero and remainders follow the
    // dividend, and the two impossible cases halt loudly (Glulx:
    // Integer Math).
    #[test]
    fn division_truncates_toward_zero() {
        let mut machine = boot(IDLE);

        let mut plant = vec![0x13, 0x11, 0x07, 0xF9, 0x02];
        plant.extend_from_slice(TO_RESULT);
        planted(&mut machine, &plant).unwrap();

        assert_eq!(result(&machine), 0xFFFF_FFFD);

        let mut plant = vec![0x14, 0x11, 0x07, 0xF9, 0x02];
        plant.extend_from_slice(TO_RESULT);
        planted(&mut machine, &plant).unwrap();

        assert_eq!(result(&machine), 0xFFFF_FFFF);

        let mut plant = vec![0x13, 0x11, 0x07, 0x07, 0x00];
        plant.extend_from_slice(TO_RESULT);
        let error = planted(&mut machine, &plant).unwrap_err();
        assert!(error.to_string().contains("division by zero"));

        let mut plant = vec![0x14, 0x11, 0x07, 0x07, 0x00];
        plant.extend_from_slice(TO_RESULT);
        let error = planted(&mut machine, &plant).unwrap_err();
        assert!(error.to_string().contains("zero taking a remainder"));

        let minimum: &[u8] = &[0x80, 0x00, 0x00, 0x00, 0xFF];

        let mut plant = vec![0x13, 0x13, 0x07];
        plant.extend_from_slice(minimum);
        plant.extend_from_slice(TO_RESULT);
        let error = planted(&mut machine, &plant).unwrap_err();
        assert!(error.to_string().contains("division overflow"));

        let mut plant = vec![0x14, 0x13, 0x07];
        plant.extend_from_slice(minimum);
        plant.extend_from_slice(TO_RESULT);
        let error = planted(&mut machine, &plant).unwrap_err();
        assert!(error.to_string().contains("overflow taking"));
    }

    // The rest of the integer family: negation and bitwork land
    // masked, and every shift of 32 or more places leaves what the
    // spec says -- zeros, except the signed right shift of a
    // negative value, which fills with its sign (Glulx: Integer
    // Math).
    #[test]
    fn integers_negate_bitwork_and_shift() {
        let mut machine = boot(IDLE);
        let cases: &[(&[u8], u32)] = &[
            (&[0x11, 0x11, 0x07, 0x09, 0x03], 6),
            (&[0x12, 0x11, 0x07, 0x06, 0x07], 42),
            (&[0x15, 0x71, 0x05], 0xFFFF_FFFB),
            (&[0x18, 0x11, 0x07, 0x0F, 0x09], 9),
            (&[0x19, 0x11, 0x07, 0x0C, 0x03], 0x0F),
            (&[0x1A, 0x11, 0x07, 0x0F, 0x09], 6),
            (&[0x1B, 0x71, 0x00], 0xFFFF_FFFF),
            (&[0x1C, 0x11, 0x07, 0x01, 0x04], 0x10),
            (&[0x1C, 0x11, 0x07, 0x01, 0x20], 0),
            (&[0x1E, 0x11, 0x07, 0x80, 0x04], 0x0FFF_FFF8),
            (&[0x1E, 0x11, 0x07, 0x80, 0x21], 0),
            (&[0x1D, 0x11, 0x07, 0x80, 0x04], 0xFFFF_FFF8),
            (&[0x1D, 0x11, 0x07, 0x80, 0x21], 0xFFFF_FFFF),
            (&[0x1D, 0x11, 0x07, 0x01, 0x21], 0),
        ];

        for (plant, expected) in cases {
            let mut whole = plant.to_vec();
            whole.extend_from_slice(TO_RESULT);
            planted(&mut machine, &whole).unwrap();

            assert_eq!(result(&machine), *expected, "{plant:?}");
        }
    }

    // copy moves words, copys and copyb move their narrowed widths
    // through their narrowed indirections, and the sign-extenders
    // widen what they are given (Glulx: Moving Data).
    #[test]
    fn data_moves_at_its_widths() {
        let mut machine = boot(IDLE);

        let mut plant = vec![0x40, 0x71, 0x2A];
        plant.extend_from_slice(TO_RESULT);
        planted(&mut machine, &plant).unwrap();

        assert_eq!(result(&machine), 0x2A);

        planted(
            &mut machine,
            &[0x41, 0x63, 0x00, 0x01, 0x23, 0x45, 0x01, 0x40],
        )
        .unwrap();

        assert_eq!(machine.memory.read_short(RESULT).unwrap(), 0x2345);

        planted(&mut machine, &[0x42, 0x61, 0xAB, 0x01, 0x44]).unwrap();

        assert_eq!(machine.memory.read_byte(0x144).unwrap(), 0xAB);

        let mut plant = vec![0x44, 0x72, 0x80, 0x00];
        plant.extend_from_slice(TO_RESULT);
        planted(&mut machine, &plant).unwrap();

        assert_eq!(result(&machine), 0xFFFF_8000);

        let mut plant = vec![0x45, 0x71, 0x80];
        plant.extend_from_slice(TO_RESULT);
        planted(&mut machine, &plant).unwrap();

        assert_eq!(result(&machine), 0xFFFF_FF80);
    }

    // The array family: words, shorts, and bytes by index --
    // indexes wrapping at 32 bits, so -1 reaches backward -- and
    // single bits numbered in both directions from the base's
    // least significant bit (Glulx: Array Data).
    #[test]
    fn arrays_index_and_bits_count_both_ways() {
        let mut machine = boot(IDLE);
        let base: &[u8] = &[0x00, 0x00, 0x01, 0x40];

        let mut plant = vec![0x4C, 0x13, 0x03];
        plant.extend_from_slice(base);
        plant.extend_from_slice(&[0x01, 0x11, 0x22, 0x33, 0x44]);
        planted(&mut machine, &plant).unwrap();

        assert_eq!(machine.memory.read_word(0x144).unwrap(), 0x1122_3344);

        let mut plant = vec![0x48, 0x13, 0x07];
        plant.extend_from_slice(base);
        plant.push(0x01);
        plant.extend_from_slice(TO_RESULT);
        planted(&mut machine, &plant).unwrap();

        assert_eq!(result(&machine), 0x1122_3344);

        let mut plant = vec![0x4D, 0x13, 0x02];
        plant.extend_from_slice(base);
        plant.extend_from_slice(&[0x04, 0xBE, 0xEF]);
        planted(&mut machine, &plant).unwrap();

        let mut plant = vec![0x49, 0x13, 0x07];
        plant.extend_from_slice(base);
        plant.push(0x04);
        plant.extend_from_slice(TO_RESULT);
        planted(&mut machine, &plant).unwrap();

        assert_eq!(result(&machine), 0xBEEF);

        planted(
            &mut machine,
            &[0x4E, 0x13, 0x01, 0x00, 0x00, 0x01, 0x49, 0xFF, 0x77],
        )
        .unwrap();

        let mut plant = vec![0x4A, 0x13, 0x07, 0x00, 0x00, 0x01, 0x49, 0xFF];
        plant.extend_from_slice(TO_RESULT);
        planted(&mut machine, &plant).unwrap();

        assert_eq!(result(&machine), 0x77);
        assert_eq!(machine.memory.read_byte(0x148).unwrap(), 0x77);

        machine.memory.write_byte(0x150, 0).unwrap();
        planted(
            &mut machine,
            &[0x4F, 0x13, 0x01, 0x00, 0x00, 0x01, 0x51, 0xFD, 0x01],
        )
        .unwrap();

        assert_eq!(machine.memory.read_byte(0x150).unwrap(), 0b0010_0000);

        let mut plant = vec![0x4B, 0x13, 0x07, 0x00, 0x00, 0x01, 0x51, 0xFD];
        plant.extend_from_slice(TO_RESULT);
        planted(&mut machine, &plant).unwrap();

        assert_eq!(result(&machine), 1);

        planted(
            &mut machine,
            &[0x4F, 0x13, 0x01, 0x00, 0x00, 0x01, 0x51, 0xFD, 0x00],
        )
        .unwrap();

        assert_eq!(machine.memory.read_byte(0x150).unwrap(), 0);
    }

    // The stack family: count, peek by index, swap, copy, and roll
    // in both directions -- with every abuse the spec forbids
    // halting loudly (Glulx: The Stack).
    #[test]
    fn the_stack_family_counts_swaps_copies_rolls() {
        let mut machine = boot(IDLE);

        // A C0 boot already pushed its zero argument count, so the
        // stack starts one deep.
        for value in [1, 2, 3] {
            machine.stack.push(value).unwrap();
        }

        let mut plant = vec![0x50, 0x07];
        plant.extend_from_slice(TO_RESULT);
        planted(&mut machine, &plant).unwrap();

        assert_eq!(result(&machine), 4);

        let mut plant = vec![0x51, 0x71, 0x01];
        plant.extend_from_slice(TO_RESULT);
        planted(&mut machine, &plant).unwrap();

        assert_eq!(result(&machine), 2);

        planted(&mut machine, &[0x52]).unwrap();

        assert_eq!(machine.stack.peek(0).unwrap(), 2);
        assert_eq!(machine.stack.peek(1).unwrap(), 3);

        planted(&mut machine, &[0x54, 0x01, 0x02]).unwrap();

        assert_eq!(machine.stack.count(), 6);
        assert_eq!(machine.stack.peek(0).unwrap(), 2);
        assert_eq!(machine.stack.peek(1).unwrap(), 3);

        planted(&mut machine, &[0x53, 0x11, 0x03, 0x01]).unwrap();

        assert_eq!(machine.stack.peek(0).unwrap(), 3);
        assert_eq!(machine.stack.peek(1).unwrap(), 2);
        assert_eq!(machine.stack.peek(2).unwrap(), 2);

        planted(&mut machine, &[0x53, 0x11, 0x03, 0xFF]).unwrap();

        assert_eq!(machine.stack.peek(0).unwrap(), 2);

        planted(&mut machine, &[0x53, 0x11, 0x00, 0x01]).unwrap();
        planted(&mut machine, &[0x53, 0x11, 0x02, 0x02]).unwrap();
        planted(&mut machine, &[0x54, 0x01, 0x00]).unwrap();

        let mut peek_far = vec![0x51, 0x71, 0x63];
        peek_far.extend_from_slice(TO_RESULT);

        let wrongs: &[(&[u8], &str)] = &[
            (&peek_far, "stkpeek"),
            (&[0x54, 0x01, 0xFF], "negative count"),
            (&[0x54, 0x01, 0x63], "exceeds the values"),
            (&[0x53, 0x11, 0xFF, 0x01], "negative count"),
            (&[0x53, 0x11, 0x63, 0x01], "exceeds the values"),
        ];

        for (wrong, complaint) in wrongs {
            let error = planted(&mut machine, wrong).unwrap_err();

            assert!(error.to_string().contains(complaint), "{wrong:?}");
        }

        let mut empty = boot(IDLE);
        let error = planted(&mut empty, &[0x52]).unwrap_err();

        assert!(error.to_string().contains("fewer than two"));
    }

    // catch stores its token and branches to the protected code;
    // throw unwinds to that token and delivers its value to the
    // catch's own target, execution resuming just past the catch.
    #[test]
    fn catch_and_throw_round_trip() {
        let mut program = vec![0xC0, 0x00, 0x00];
        program.extend_from_slice(&[0x32, 0x17, 0x00, 0x00, 0x01, 0x44, 0x0B]);
        program.extend_from_slice(&[0x40, 0x71, 0x63, 0x00, 0x00, 0x01, 0x48]);
        program.extend_from_slice(&[0x81, 0x20]);
        program.extend_from_slice(&[0x33, 0x61, 0x37, 0x01, 0x44]);

        let mut machine = boot(&program);

        assert_eq!(machine.run(None).unwrap(), 4);
        assert_eq!(machine.memory.read_word(0x144).unwrap(), 55);
        assert_eq!(machine.memory.read_word(0x148).unwrap(), 99);

        let mut broken = boot(IDLE);
        let error = planted(&mut broken, &[0x33, 0x11, 0x01, 0x03]).unwrap_err();

        assert!(error.to_string().contains("catch token"));
    }

    // The lifecycle and map family: verify judges the checksum
    // both ways, getmemsize and setmemsize speak to the map,
    // protect guards a range across the restart opcode, and
    // debugtrap halts loudly as the spec directs an interpreter
    // with no debugger to.
    #[test]
    fn lifecycle_and_map_opcodes() {
        let mut machine = boot(IDLE);

        let mut plant = vec![0x81, 0x21, 0x07];
        plant.extend_from_slice(TO_RESULT);
        planted(&mut machine, &plant).unwrap();

        assert_eq!(result(&machine), 0);

        let mut doctored =
            Machine::new(Story::new(image_with_checksum(IDLE, 7)).unwrap(), None).unwrap();

        let mut plant = vec![0x81, 0x21, 0x07];
        plant.extend_from_slice(TO_RESULT);
        planted(&mut doctored, &plant).unwrap();

        assert_eq!(doctored.memory.read_word(RESULT).unwrap(), 1);

        let mut plant = vec![0x81, 0x02, 0x07];
        plant.extend_from_slice(TO_RESULT);
        planted(&mut machine, &plant).unwrap();

        assert_eq!(result(&machine), 0x300);

        planted(
            &mut machine,
            &[
                0x81, 0x03, 0x73, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x01, 0x44,
            ],
        )
        .unwrap();

        assert_eq!(machine.memory.endmem(), 0x400);
        assert_eq!(machine.memory.read_word(0x144).unwrap(), 0);

        machine.memory.write_word(0x160, 0xFEED_F00D).unwrap();
        planted(&mut machine, &[0x81, 0x70, 0x21, 0x04, 0x01, 0x60]).unwrap();

        assert_eq!(machine.memory.read_run(0x160, 4).unwrap(), [0, 0, 0, 0]);

        machine.memory.write_word(0x160, 0xFEED_F00D).unwrap();
        planted(
            &mut machine,
            &[0x81, 0x71, 0x21, 0x02, 0x04, 0x01, 0x60, 0x01, 0x64],
        )
        .unwrap();

        assert_eq!(machine.memory.read_word(0x164).unwrap(), 0xFEED_F00D);

        machine.memory.write_word(0x140, 7).unwrap();
        planted(
            &mut machine,
            &[0x81, 0x27, 0x13, 0x00, 0x00, 0x01, 0x40, 0x04],
        )
        .unwrap();
        planted(&mut machine, &[0x81, 0x22]).unwrap();

        assert_eq!(machine.pc, BOOT_PC);
        assert_eq!(machine.memory.read_word(0x140).unwrap(), 7);
        assert_eq!(machine.memory.endmem(), 0x300);

        let mut trapped = boot(IDLE);
        let error = planted(&mut trapped, &[0x81, 0x01, 0x01, 0x07]).unwrap_err();

        assert!(error.to_string().contains("debugtrap with value 7"));
    }

    // The roster is whole: every opcode Glulx 3.1.3 defines has a
    // signature, and only those -- the name table and the
    // signature match agree number for number. An undefined number
    // says the spec does not know it; a pc off the map says where
    // it ran; and a runaway loop trips the run limit.
    #[test]
    fn frontiers_and_faults_are_loud() {
        for number in 0..=0x240u32 {
            assert_eq!(
                signature(number).is_some(),
                !name(number).starts_with('$'),
                "opcode {number:#x}"
            );
        }

        let mut machine = boot(IDLE);
        let error = planted(&mut machine, &[0x7F]).unwrap_err();

        assert!(error.to_string().contains("does not define"));

        machine.pc = machine.memory.endmem();

        let error = machine.step().unwrap_err();

        assert!(error.to_string().contains("ran off the memory map"));

        let mut looper = boot(&[0xC0, 0x00, 0x00, 0x81, 0x04, 0x03, 0x00, 0x00, 0x00, 0x4B]);
        let error = looper.run(Some(5)).unwrap_err();

        assert!(error.to_string().contains("exceeded 5"));
    }

    // A string-terminator stub where a function result belongs is
    // an error in any era (Glulx: Call Stubs); the resume stubs
    // proper are the strings module's business, tested with it.
    #[test]
    fn a_misplaced_terminator_stub_is_loud() {
        let mut machine = boot(IDLE);

        machine
            .stack
            .push_stub(dest_type::RESUME_FUNCTION, 0, 0)
            .unwrap();

        let error = machine.pop_stub(1).unwrap_err();

        assert!(error.to_string().contains("string-terminator"));
    }
}
