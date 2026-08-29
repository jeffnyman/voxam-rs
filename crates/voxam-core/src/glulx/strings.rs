//! String decoding and the output opcodes (Glulx: Strings).
//!
//! Three string types share one entry point: E0, plain bytes; E2,
//! 32-bit characters; and E1, Huffman-compressed against the
//! string-decoding table (Glulx: The String-Decoding Table). Only
//! E1 is interesting: its tree can hold nodes that print other
//! strings or call functions, so decoding is not a loop that runs
//! to completion but a coroutine that may suspend into the machine
//! and resume later (Glulx: Calling and Returning Within Strings).
//!
//! In filter mode every character is a function call, and a
//! compressed string may call a function at any node. Either way
//! the decoder stops, records where it was as a call stub -- the
//! resume types the stack module names -- and lets the machine
//! run; resume() is the other half, called when one of those stubs
//! comes back off the stack. Glk mode never suspends, since output
//! there is a direct call: that is the path real games take, and
//! it stays a plain loop -- one that refuses by name until the Glk
//! era arrives. The null mode decodes and discards.

use crate::errors::VoxamError;
use crate::glulx::funcs;
use crate::glulx::machine::{Machine, io_mode};
use crate::glulx::stack::{CallStub, dest_type};

/// The three string types; E3 through FF are reserved for future
/// kinds of string (Glulx: Strings).
pub const CSTRING: u32 = 0xE0;
pub const COMPRESSED: u32 = 0xE1;
pub const UNICODE_STRING: u32 = 0xE2;
const STRING_FIRST: u32 = 0xE0;
const STRING_LAST: u32 = 0xFF;

const FUNCTION_FIRST: u8 = 0xC0;
const FUNCTION_LAST: u8 = 0xDF;

/// The node types a decoding table may hold (Glulx: The
/// String-Decoding Table).
const NODE_BRANCH: u8 = 0x00;
const NODE_TERMINATOR: u8 = 0x01;
const NODE_CHAR: u8 = 0x02;
const NODE_CSTR: u8 = 0x03;
const NODE_UNICHAR: u8 = 0x04;
const NODE_UNISTR: u8 = 0x05;
const NODE_INDIRECT: u8 = 0x08;
const NODE_DOUBLE_INDIRECT: u8 = 0x09;
const NODE_INDIRECT_ARGS: u8 = 0x0A;
const NODE_DOUBLE_INDIRECT_ARGS: u8 = 0x0B;

/// The root node's address sits at the table's ninth byte, after
/// the length and node-count words (Glulx: The String-Decoding
/// Table).
const ROOT_AT: u32 = 8;

const LAST_BIT: u32 = 7;

fn string_error(message: String) -> VoxamError {
    VoxamError::GlulxString(message)
}

/// The engine of streamchar and streamunichar.
///
/// In filter mode this enters the filter function and returns; the
/// machine carries on from there, and the ordinary function-return
/// path brings it back -- the stub discards the filter's result,
/// exactly as the reference glulxe arranges it.
pub fn put_char(machine: &mut Machine, character: u32) -> Result<(), VoxamError> {
    let mode = machine.iosys.mode;

    if mode == io_mode::NULL {
        return Ok(());
    }

    if mode == io_mode::FILTER {
        machine.stack.push_stub(dest_type::DISCARD, 0, machine.pc)?;

        return machine.enter_function(machine.iosys.rock, &[character]);
    }

    put_glk(machine, character)
}

/// The engine of streamnum: print a signed decimal.
///
/// charnum counts the characters already printed, nonzero only
/// when resuming a filter-mode print. The resume stub's PC field
/// carries the number itself, so resuming needs it stored nowhere
/// else (Glulx: Calling and Returning Within Strings).
pub fn stream_num(
    machine: &mut Machine,
    value: u32,
    in_middle: bool,
    charnum: u32,
) -> Result<(), VoxamError> {
    let text = (value as i32).to_string();
    let digits = text.as_bytes();
    let mode = machine.iosys.mode;
    let mut in_middle = in_middle;

    if mode == io_mode::GLK {
        for digit in &digits[charnum as usize..] {
            put_glk(machine, u32::from(*digit))?;
        }
    } else if mode == io_mode::FILTER {
        if !in_middle {
            machine
                .stack
                .push_stub(dest_type::RESUME_FUNCTION, 0, machine.pc)?;

            in_middle = true;
        }

        if (charnum as usize) < digits.len() {
            machine
                .stack
                .push_stub(dest_type::RESUME_NUMBER, charnum + 1, value)?;

            return machine
                .enter_function(machine.iosys.rock, &[u32::from(digits[charnum as usize])]);
        }
    }

    if in_middle {
        let stub = machine.stack.pop_stub()?;
        machine.pc = stub.pc;

        if stub.desttype != dest_type::RESUME_FUNCTION {
            return Err(string_error(
                "a string-on-string call stub arrived while printing a number \
                 (Glulx: Calling and Returning Within Strings)"
                    .into(),
            ));
        }
    }

    Ok(())
}

/// The engine of streamstr, and the landing for resumed strings.
pub fn stream_string(
    machine: &mut Machine,
    addr: u32,
    in_middle: u32,
    bitnum: u32,
) -> Result<(), VoxamError> {
    if addr == 0 {
        return Err(string_error(
            "streamstr with a null address (Glulx: Output)".into(),
        ));
    }

    Printer::new(machine, addr, in_middle, bitnum).run()
}

/// Continue a suspended print from its popped stub.
///
/// The machine's stub-popping filtered the types already, so the
/// four resume kinds are exhaustive here (Glulx: Calling and
/// Returning Within Strings).
pub fn resume(machine: &mut Machine, stub: CallStub) -> Result<(), VoxamError> {
    machine.pc = stub.pc;

    match stub.desttype {
        dest_type::RESUME_COMPRESSED => stream_string(machine, stub.pc, COMPRESSED, stub.destaddr),
        dest_type::RESUME_CSTRING => stream_string(machine, stub.pc, CSTRING, 0),
        dest_type::RESUME_UNICODE => stream_string(machine, stub.pc, UNICODE_STRING, 0),
        _ => stream_num(machine, stub.pc, true, stub.destaddr),
    }
}

/// Emit one character through the machine's Glk library.
///
/// No library is installed in this port yet -- setiosys falls back
/// to the null system, so only forcing the mode can arrange this
/// call.
fn put_glk(_machine: &mut Machine, _character: u32) -> Result<(), VoxamError> {
    Err(VoxamError::GlulxGlk(
        "Glk output selected, but no Glk library is installed".into(),
    ))
}

/// One streamstr in progress.
///
/// The mutable state the reference glulxe keeps in the locals of a
/// three-hundred-line function: where the walk stands, which bit,
/// whether the terminator stub is down yet, and whether control
/// was handed back to the machine.
struct Printer<'a> {
    machine: &'a mut Machine,
    addr: u32,
    in_middle: u32,
    bitnum: u32,
    substring: bool,
    suspended: bool,
}

impl<'a> Printer<'a> {
    fn new(machine: &'a mut Machine, addr: u32, in_middle: u32, bitnum: u32) -> Self {
        Self {
            machine,
            addr,
            in_middle,
            bitnum,
            // Entering mid-string means the terminator stub is
            // already on the stack.
            substring: in_middle != 0,
            suspended: false,
        }
    }

    /// Print until the string ends or the machine must run.
    fn run(&mut self) -> Result<(), VoxamError> {
        loop {
            let kind = if self.in_middle == 0 {
                let kind = u32::from(self.machine.memory.read_byte(self.addr)?);
                // E2 strings pad to a four-byte boundary; the
                // others start right after their type byte.
                self.addr = self
                    .addr
                    .wrapping_add(if kind == UNICODE_STRING { 4 } else { 1 });
                self.bitnum = 0;

                kind
            } else {
                let kind = self.in_middle;
                self.in_middle = 0;

                kind
            };

            let restart = match kind {
                COMPRESSED => self.compressed()?,
                CSTRING => self.cstring()?,
                UNICODE_STRING => self.unicode_string()?,
                _ if (STRING_FIRST..=STRING_LAST).contains(&kind) => {
                    return Err(string_error(format!(
                        "the type byte ${kind:x} names a kind of string reserved \
                         for the future (Glulx: Strings)"
                    )));
                }
                _ => {
                    return Err(string_error(format!(
                        "the type byte ${kind:x} is not a string at all (Glulx: \
                         Strings)"
                    )));
                }
            };

            if self.suspended {
                return Ok(());
            }

            if restart {
                continue;
            }

            if !self.substring {
                return Ok(());
            }

            if !self.pop_string_stub()? {
                return Ok(());
            }

            self.in_middle = COMPRESSED;
        }
    }

    /// An E0 string: bytes to a zero terminator.
    fn cstring(&mut self) -> Result<bool, VoxamError> {
        let mode = self.machine.iosys.mode;

        if mode == io_mode::FILTER {
            self.begin_substring()?;

            let character = self.machine.memory.read_byte(self.addr)?;
            self.addr = self.addr.wrapping_add(1);

            if character != 0 {
                self.call_filter(
                    u32::from(character),
                    dest_type::RESUME_CSTRING,
                    0,
                    self.addr,
                )?;
            }

            return Ok(false);
        }

        loop {
            let character = self.machine.memory.read_byte(self.addr)?;
            self.addr = self.addr.wrapping_add(1);

            if character == 0 {
                return Ok(false);
            }

            if mode == io_mode::GLK {
                put_glk(self.machine, u32::from(character))?;
            }
        }
    }

    /// An E2 string: 32-bit characters to a zero terminator.
    fn unicode_string(&mut self) -> Result<bool, VoxamError> {
        let mode = self.machine.iosys.mode;

        if mode == io_mode::FILTER {
            self.begin_substring()?;

            let character = self.machine.memory.read_word(self.addr)?;
            self.addr = self.addr.wrapping_add(4);

            if character != 0 {
                self.call_filter(character, dest_type::RESUME_UNICODE, 0, self.addr)?;
            }

            return Ok(false);
        }

        loop {
            let character = self.machine.memory.read_word(self.addr)?;
            self.addr = self.addr.wrapping_add(4);

            if character == 0 {
                return Ok(false);
            }

            if mode == io_mode::GLK {
                put_glk(self.machine, character)?;
            }
        }
    }

    /// Walk the Huffman tree until the string ends or we suspend.
    ///
    /// True means a sub-object was set up and the outer loop
    /// should start again on it. The reference glulxe keeps a
    /// multi-bit cache of the tree; this is the plain walk it
    /// falls back on -- one memory read per bit. The cache is a
    /// worthwhile optimization later, but it must cope with a
    /// table in RAM the game can rewrite, so correctness first.
    fn compressed(&mut self) -> Result<bool, VoxamError> {
        let table = self.machine.string_table;

        if table == 0 {
            return Err(string_error(
                "a compressed string cannot print with no decoding table set \
                 (Glulx: The String-Decoding Table)"
                    .into(),
            ));
        }

        let root = self.machine.memory.read_word(table.wrapping_add(ROOT_AT))?;
        let mut byte = self.machine.memory.read_byte(self.addr)?;

        if self.bitnum != 0 {
            byte >>= self.bitnum;
        }

        let mut node = root;

        loop {
            let nodetype = self.machine.memory.read_byte(node)?;
            node = node.wrapping_add(1);

            match nodetype {
                NODE_BRANCH => {
                    // Bits read low bit first (Glulx: Strings).
                    node = if byte & 1 != 0 {
                        self.machine.memory.read_word(node.wrapping_add(4))?
                    } else {
                        self.machine.memory.read_word(node)?
                    };

                    if self.bitnum == LAST_BIT {
                        self.bitnum = 0;
                        self.addr = self.addr.wrapping_add(1);
                        byte = self.machine.memory.read_byte(self.addr)?;
                    } else {
                        self.bitnum += 1;
                        byte >>= 1;
                    }
                }
                NODE_TERMINATOR => return Ok(false),
                NODE_CHAR => {
                    let character = self.machine.memory.read_byte(node)?;

                    if !self.emit(u32::from(character))? {
                        return Ok(false);
                    }

                    node = root;
                }
                NODE_UNICHAR => {
                    let character = self.machine.memory.read_word(node)?;

                    if !self.emit(character)? {
                        return Ok(false);
                    }

                    node = root;
                }
                NODE_CSTR => {
                    if self.emit_substring(node, CSTRING)? {
                        return Ok(true);
                    }

                    node = root;
                }
                NODE_UNISTR => {
                    if self.emit_substring(node, UNICODE_STRING)? {
                        return Ok(true);
                    }

                    node = root;
                }
                NODE_INDIRECT
                | NODE_DOUBLE_INDIRECT
                | NODE_INDIRECT_ARGS
                | NODE_DOUBLE_INDIRECT_ARGS => {
                    // Either restarts on a referenced string or
                    // suspends into a referenced function; both
                    // end this walk.
                    return self.indirect(nodetype, node);
                }
                _ => {
                    return Err(string_error(format!(
                        "node type ${nodetype:x} is not one the decoding table may \
                         hold (Glulx: The String-Decoding Table)"
                    )));
                }
            }
        }
    }

    /// Print one character; false means we suspended into a
    /// filter.
    fn emit(&mut self, character: u32) -> Result<bool, VoxamError> {
        let mode = self.machine.iosys.mode;

        if mode == io_mode::GLK {
            put_glk(self.machine, character)?;

            return Ok(true);
        }

        if mode == io_mode::FILTER {
            self.begin_substring()?;
            self.call_filter(
                character,
                dest_type::RESUME_COMPRESSED,
                self.bitnum,
                self.addr,
            )?;

            return Ok(false);
        }

        // The null mode: decoded and discarded.
        Ok(true)
    }

    /// A node holding a whole string; true restarts on it.
    fn emit_substring(&mut self, node: u32, kind: u32) -> Result<bool, VoxamError> {
        let mode = self.machine.iosys.mode;

        if mode == io_mode::FILTER {
            // Hand the sub-string to the top-level loop, with a
            // stub remembering where the compressed stream picks
            // back up.
            self.begin_substring()?;

            self.machine.pc = self.addr;

            self.machine
                .stack
                .push_stub(dest_type::RESUME_COMPRESSED, self.bitnum, self.addr)?;

            self.in_middle = kind;
            self.addr = node;

            return Ok(true);
        }

        if mode == io_mode::GLK {
            let mut node = node;

            if kind == CSTRING {
                loop {
                    let character = self.machine.memory.read_byte(node)?;

                    if character == 0 {
                        break;
                    }

                    put_glk(self.machine, u32::from(character))?;

                    node = node.wrapping_add(1);
                }
            } else {
                loop {
                    let character = self.machine.memory.read_word(node)?;

                    if character == 0 {
                        break;
                    }

                    put_glk(self.machine, character)?;

                    node = node.wrapping_add(4);
                }
            }
        }

        Ok(false)
    }

    /// Follow an indirect reference to a string or a function.
    ///
    /// True restarts the outer loop on a referenced string; a
    /// referenced function suspends instead.
    fn indirect(&mut self, nodetype: u8, node: u32) -> Result<bool, VoxamError> {
        let mut target = self.machine.memory.read_word(node)?;

        if matches!(nodetype, NODE_DOUBLE_INDIRECT | NODE_DOUBLE_INDIRECT_ARGS) {
            target = self.machine.memory.read_word(target)?;
        }

        let target_type = self.machine.memory.read_byte(target)?;

        self.begin_substring()?;

        if (STRING_FIRST..=STRING_LAST).contains(&u32::from(target_type)) {
            self.machine.pc = self.addr;

            self.machine
                .stack
                .push_stub(dest_type::RESUME_COMPRESSED, self.bitnum, self.addr)?;

            self.in_middle = 0;
            self.addr = target;

            return Ok(true);
        }

        if (FUNCTION_FIRST..=FUNCTION_LAST).contains(&target_type) {
            let args = if matches!(nodetype, NODE_INDIRECT_ARGS | NODE_DOUBLE_INDIRECT_ARGS) {
                let count = self.machine.memory.read_word(node.wrapping_add(4))?;

                funcs::pop_arguments(
                    &mut self.machine.stack,
                    count,
                    &self.machine.memory,
                    node.wrapping_add(8),
                )?
            } else {
                Vec::new()
            };

            self.machine
                .stack
                .push_stub(dest_type::RESUME_COMPRESSED, self.bitnum, self.addr)?;
            self.machine.enter_function(target, &args)?;

            self.suspended = true;

            return Ok(false);
        }

        Err(string_error(format!(
            "an indirect node reaches ${target:x}, which holds neither a string \
             nor a function (Glulx: The String-Decoding Table)"
        )))
    }

    /// Lay the terminator stub that marks where this print began.
    fn begin_substring(&mut self) -> Result<(), VoxamError> {
        if !self.substring {
            self.machine
                .stack
                .push_stub(dest_type::RESUME_FUNCTION, 0, self.machine.pc)?;

            self.substring = true;
        }

        Ok(())
    }

    /// Suspend into the filter function with one character.
    fn call_filter(
        &mut self,
        character: u32,
        desttype: u32,
        destaddr: u32,
        pc: u32,
    ) -> Result<(), VoxamError> {
        self.machine.stack.push_stub(desttype, destaddr, pc)?;
        self.machine
            .enter_function(self.machine.iosys.rock, &[character])?;

        self.suspended = true;

        Ok(())
    }

    /// Pop a resume or terminator stub; false ends the print.
    fn pop_string_stub(&mut self) -> Result<bool, VoxamError> {
        let stub = self.machine.stack.pop_stub()?;
        self.machine.pc = stub.pc;

        if stub.desttype == dest_type::RESUME_FUNCTION {
            return Ok(false);
        }

        if stub.desttype == dest_type::RESUME_COMPRESSED {
            self.addr = stub.pc;
            self.bitnum = stub.destaddr;

            return Ok(true);
        }

        Err(string_error(
            "a function-terminator call stub arrived at the end of a string \
             (Glulx: Calling and Returning Within Strings)"
                .into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glulx::machine::IOSystem;
    use crate::glulx::story::Story;
    use crate::glulx::testing::image;

    const PLANT: u32 = 0x140;
    const TABLE: u32 = 0x190;
    const TEXT: u32 = 0x1C0;
    const BUFFER: u32 = 0x260;
    const CURSOR: u32 = 0x2A0;
    const FILTER: u32 = 0x58;

    // setiosys 1, $58 -- select the filter -- and quit, for
    // plants.
    const SELECT_FILTER: &[u8] = &[0x81, 0x49, 0x21, 0x01, 0x00, 0x58];
    const QUIT: &[u8] = &[0x81, 0x20];

    /// The observable-output harness: an idle main, then at $58 a
    /// C1 filter function that appends its one character argument
    /// to a RAM buffer and advances a cursor -- so every
    /// suspension-and-resume round trip leaves evidence.
    fn code() -> Vec<u8> {
        let mut code = vec![0xC0, 0x00, 0x00, 0x81, 0x20];
        code.resize(16, 0);
        code.extend_from_slice(&[0xC1, 0x04, 0x01, 0x00, 0x00]);
        code.extend_from_slice(&[0x4E, 0x63, 0x09, 0x00, 0x00, 0x02, 0x60, 0x02, 0xA0, 0x00]);
        code.extend_from_slice(&[0x10, 0x16, 0x06, 0x02, 0xA0, 0x01, 0x02, 0xA0]);
        code.extend_from_slice(&[0x31, 0x01, 0x00]);

        code
    }

    fn booted() -> Machine {
        Machine::new(Story::new(image(&code())).unwrap(), None).unwrap()
    }

    /// Run a plant under the filter; the captured output comes
    /// back.
    fn spoken(plant: &[u8]) -> (Machine, Vec<u8>) {
        let mut machine = booted();

        let mut program = SELECT_FILTER.to_vec();
        program.extend_from_slice(plant);
        program.extend_from_slice(QUIT);

        machine.memory.write_run(PLANT, &program).unwrap();

        machine.pc = PLANT;

        machine.run(Some(2000)).unwrap();

        let count = machine.memory.read_word(CURSOR).unwrap();
        let held = machine.memory.read_run(BUFFER, count).unwrap();

        (machine, held)
    }

    /// Lay a decoding table at TABLE: header words, then the
    /// nodes.
    fn planted_table(machine: &mut Machine, nodes: &[u8], root: u32) {
        let mut table = (12 + nodes.len() as u32).to_be_bytes().to_vec();
        table.extend_from_slice(&0u32.to_be_bytes());
        table.extend_from_slice(&root.to_be_bytes());
        table.extend_from_slice(nodes);

        machine.memory.write_run(TABLE, &table).unwrap();
    }

    /// The branch-and-char tree the walk tests share: root sends
    /// bit 0 to 'a', bit 1 to a second branch of 'b' and the
    /// terminator.
    fn ab_tree() -> Vec<u8> {
        let mut nodes = vec![0x00];
        nodes.extend_from_slice(&(TABLE + 21).to_be_bytes());
        nodes.extend_from_slice(&(TABLE + 23).to_be_bytes());
        nodes.extend_from_slice(&[0x02, 0x61]);
        nodes.push(0x00);
        nodes.extend_from_slice(&(TABLE + 32).to_be_bytes());
        nodes.extend_from_slice(&(TABLE + 34).to_be_bytes());
        nodes.extend_from_slice(&[0x02, 0x62]);
        nodes.push(0x01);

        nodes
    }

    /// A one-node chain: root sends bit 0 to the node, bit 1 to
    /// the terminator.
    fn chain(node: &[u8]) -> Vec<u8> {
        let mut nodes = vec![0x00];
        nodes.extend_from_slice(&(TABLE + 21).to_be_bytes());
        nodes.extend_from_slice(&(TABLE + 21 + node.len() as u32).to_be_bytes());
        nodes.extend_from_slice(node);
        nodes.push(0x01);

        nodes
    }

    // The io system: three modes, and an unknown one selects the
    // null system rather than erring -- what a probing program
    // should find.
    #[test]
    fn the_io_system_selects_and_normalizes() {
        let mut iosys = IOSystem::default();

        iosys.select(io_mode::FILTER, 0x58);

        assert_eq!((iosys.mode, iosys.rock), (1, 0x58));

        iosys.select(9, 7);

        assert_eq!((iosys.mode, iosys.rock), (0, 0));

        iosys.select(io_mode::GLK, 3);
        iosys.reset();

        assert_eq!((iosys.mode, iosys.rock), (0, 0));
    }

    // streamchar keeps its low byte, streamunichar the whole
    // value, and streamnum prints a signed decimal one suspension
    // at a time -- every character a filter call, every resume
    // picking up exactly where the print left off.
    #[test]
    fn characters_and_numbers_speak_through_the_filter() {
        let (_, out) = spoken(&[0x70, 0x02, 0x01, 0x48]);

        assert_eq!(out, b"H");

        let (_, out) = spoken(&[0x73, 0x01, 0x69]);

        assert_eq!(out, b"i");

        let (_, out) = spoken(&[0x71, 0x01, 0xD6]);

        assert_eq!(out, b"-42");

        let (_, out) = spoken(&[0x71, 0x01, 0x00]);

        assert_eq!(out, b"0");
    }

    // The uncompressed strings: E0 bytes and E2 words, each
    // character a suspension in filter mode, the resume stubs
    // walking the string to its terminator.
    #[test]
    fn uncompressed_strings_stream_whole() {
        let mut machine = booted();

        machine
            .memory
            .write_run(TEXT, &[0xE0, 0x41, 0x42, 0x00])
            .unwrap();

        let mut program = SELECT_FILTER.to_vec();
        program.extend_from_slice(&[0x72, 0x02, 0x01, 0xC0]);
        program.extend_from_slice(QUIT);
        machine.memory.write_run(PLANT, &program).unwrap();

        machine.pc = PLANT;

        machine.run(Some(2000)).unwrap();

        assert_eq!(machine.memory.read_run(BUFFER, 2).unwrap(), b"AB");

        let mut wide = booted();

        let mut text = vec![0xE2, 0x00, 0x00, 0x00];
        text.extend_from_slice(&0x43u32.to_be_bytes());
        text.extend_from_slice(&0x44u32.to_be_bytes());
        text.extend_from_slice(&[0, 0, 0, 0]);
        wide.memory.write_run(0x1D0, &text).unwrap();

        let mut program = SELECT_FILTER.to_vec();
        program.extend_from_slice(&[0x72, 0x02, 0x01, 0xD0]);
        program.extend_from_slice(QUIT);
        wide.memory.write_run(PLANT, &program).unwrap();

        wide.pc = PLANT;

        wide.run(Some(2000)).unwrap();

        assert_eq!(wide.memory.read_run(BUFFER, 2).unwrap(), b"CD");
    }

    // A branch-and-char tree: the bits read low bit first, so "ab"
    // is the byte 0x1A -- and in filter mode every character
    // suspends mid-tree, the resume stub carrying the bit number
    // back (Glulx: The String-Decoding Table).
    #[test]
    fn compressed_strings_walk_their_tree() {
        let mut machine = booted();

        planted_table(&mut machine, &ab_tree(), TABLE + 12);
        machine.memory.write_run(TEXT, &[0xE1, 0x1A]).unwrap();

        let mut program = SELECT_FILTER.to_vec();
        program.extend_from_slice(&[0x81, 0x41, 0x02, 0x01, 0x90]);
        program.extend_from_slice(&[0x72, 0x02, 0x01, 0xC0]);
        program.extend_from_slice(QUIT);
        machine.memory.write_run(PLANT, &program).unwrap();

        machine.pc = PLANT;

        machine.run(Some(2000)).unwrap();

        assert_eq!(machine.memory.read_run(BUFFER, 2).unwrap(), b"ab");

        // The same string in the null mode decodes and discards.
        let mut quiet = booted();

        planted_table(&mut quiet, &ab_tree(), TABLE + 12);
        quiet.memory.write_run(TEXT, &[0xE1, 0x1A]).unwrap();
        quiet.string_table = TABLE;

        stream_string(&mut quiet, TEXT, 0, 0).unwrap();

        assert_eq!(quiet.memory.read_word(CURSOR).unwrap(), 0);
    }

    // The richer nodes: a unichar, an embedded C string, an
    // indirect reference to a whole string, a double-indirect one,
    // and an indirect function call carrying arguments from the
    // node itself -- each suspending into the filter and resuming
    // mid-tree.
    #[test]
    fn the_richer_nodes_print_and_call() {
        let unichar = {
            let mut node = vec![0x04];
            node.extend_from_slice(&0x21u32.to_be_bytes());
            node
        };
        let embedded = vec![0x03, 0x58, 0x59, 0x00];
        let indirect = {
            let mut node = vec![0x08];
            node.extend_from_slice(&(TEXT + 8).to_be_bytes());
            node
        };
        let doubly = {
            let mut node = vec![0x09];
            node.extend_from_slice(&(TEXT + 16).to_be_bytes());
            node
        };
        let calling = {
            let mut node = vec![0x0A];
            node.extend_from_slice(&FILTER.to_be_bytes());
            node.extend_from_slice(&1u32.to_be_bytes());
            node.extend_from_slice(&0x23u32.to_be_bytes());
            node
        };

        let cases: &[(&[u8], &[u8])] = &[
            (&unichar, b"!"),
            (&embedded, b"XY"),
            (&indirect, b"Q"),
            (&doubly, b"Q"),
            (&calling, b"#"),
        ];

        for (node, expected) in cases {
            let mut fresh = booted();

            planted_table(&mut fresh, &chain(node), TABLE + 12);
            // An E0 target for the indirect nodes, and a pointer
            // cell for the double-indirect one.
            fresh
                .memory
                .write_run(TEXT + 8, &[0xE0, 0x51, 0x00])
                .unwrap();
            fresh.memory.write_word(TEXT + 16, TEXT + 8).unwrap();
            // The string: bit 0 (the node), then bit 1 (the
            // terminator) -- the byte 0b00000010.
            fresh.memory.write_run(TEXT, &[0xE1, 0x02]).unwrap();

            let mut program = SELECT_FILTER.to_vec();
            program.extend_from_slice(&[0x81, 0x41, 0x02, 0x01, 0x90]);
            program.extend_from_slice(&[0x72, 0x02, 0x01, 0xC0]);
            program.extend_from_slice(QUIT);
            fresh.memory.write_run(PLANT, &program).unwrap();

            fresh.pc = PLANT;

            fresh.run(Some(2000)).unwrap();

            let count = fresh.memory.read_word(CURSOR).unwrap();

            assert_eq!(fresh.memory.read_run(BUFFER, count).unwrap(), *expected);
        }
    }

    // Every lie a string can tell halts loudly: a null address, a
    // type byte that is no string or a reserved future one, a
    // compressed print with no table, a node the table may not
    // hold, and an indirect reference to something neither string
    // nor function.
    #[test]
    fn broken_strings_halt_loudly() {
        let mut machine = booted();

        let error = stream_string(&mut machine, 0, 0, 0).unwrap_err();
        assert!(error.to_string().contains("null address"));

        machine.memory.write_byte(TEXT, 0x40).unwrap();

        let error = stream_string(&mut machine, TEXT, 0, 0).unwrap_err();
        assert!(error.to_string().contains("not a string at all"));

        machine.memory.write_byte(TEXT, 0xE5).unwrap();

        let error = stream_string(&mut machine, TEXT, 0, 0).unwrap_err();
        assert!(error.to_string().contains("reserved for the future"));

        machine.memory.write_run(TEXT, &[0xE1, 0x00]).unwrap();
        machine.string_table = 0;

        let error = stream_string(&mut machine, TEXT, 0, 0).unwrap_err();
        assert!(error.to_string().contains("no decoding table"));

        planted_table(&mut machine, &[0x07], TABLE + 12);

        machine.string_table = TABLE;

        let error = stream_string(&mut machine, TEXT, 0, 0).unwrap_err();
        assert!(error.to_string().contains("not one the decoding table"));

        machine.memory.write_byte(0x250, 0x50).unwrap();

        let mut node = vec![0x08];
        node.extend_from_slice(&0x250u32.to_be_bytes());
        planted_table(&mut machine, &node, TABLE + 12);

        let error = stream_string(&mut machine, TEXT, 0, 0).unwrap_err();
        assert!(error.to_string().contains("neither a string nor"));
    }

    // The stub-discipline errors: a number print interrupted by
    // the wrong stub, and a string ending into a stub that belongs
    // to neither kind of resume.
    #[test]
    fn stub_discipline_is_enforced() {
        let mut machine = booted();

        machine.iosys.select(io_mode::FILTER, FILTER);
        machine
            .stack
            .push_stub(dest_type::RESUME_COMPRESSED, 0, 0)
            .unwrap();

        let error = stream_num(&mut machine, 5, true, 1).unwrap_err();
        assert!(error.to_string().contains("string-on-string"));

        let mut wrong = booted();

        wrong.memory.write_byte(TEXT, 0).unwrap();
        wrong.stack.push_stub(dest_type::MEMORY, 0, 0).unwrap();

        let error = stream_string(&mut wrong, TEXT, CSTRING, 0).unwrap_err();
        assert!(error.to_string().contains("function-terminator"));
    }

    // Without a library, Glk mode can only be forced, and forcing
    // it is refused by name. (The library-backed half of the
    // reference test waits for the Glk era.)
    #[test]
    fn forced_glk_output_is_refused_by_name() {
        let mut bare = booted();

        bare.iosys.select(io_mode::GLK, 0);

        let error = put_char(&mut bare, 0x41).unwrap_err();
        assert!(error.to_string().contains("no Glk library"));
    }

    // The null system decodes everything and prints nothing:
    // characters, numbers, byte strings, and a compressed string
    // long enough to roll the bit cursor into its second byte --
    // "bbbb" is ten bits.
    #[test]
    fn the_null_system_decodes_and_discards() {
        let mut machine = booted();

        put_char(&mut machine, 0x41).unwrap();
        stream_num(&mut machine, 0x2A, false, 0).unwrap();
        machine.memory.write_run(TEXT, &[0xE0, 0x41, 0x00]).unwrap();
        stream_string(&mut machine, TEXT, 0, 0).unwrap();

        let mut text = vec![0xE2, 0x00, 0x00, 0x00];
        text.extend_from_slice(&0x41u32.to_be_bytes());
        text.extend_from_slice(&[0, 0, 0, 0]);
        machine.memory.write_run(0x1D0, &text).unwrap();
        stream_string(&mut machine, 0x1D0, 0, 0).unwrap();

        planted_table(&mut machine, &ab_tree(), TABLE + 12);
        machine.memory.write_run(TEXT, &[0xE1, 0x55, 0x03]).unwrap();

        machine.string_table = TABLE;

        stream_string(&mut machine, TEXT, 0, 0).unwrap();

        assert_eq!(machine.memory.read_word(CURSOR).unwrap(), 0);
    }

    // The richer nodes in the null system walk without printing:
    // the unichar, both embedded strings, and an argumentless
    // indirect function call that still runs its function.
    #[test]
    fn the_richer_nodes_decode_in_the_null_system() {
        let unichar = {
            let mut node = vec![0x04];
            node.extend_from_slice(&0x2603u32.to_be_bytes());
            node
        };
        let embedded = vec![0x03, 0x58, 0x00];
        let wide = {
            let mut node = vec![0x05];
            node.extend_from_slice(&0x59u32.to_be_bytes());
            node.extend_from_slice(&[0, 0, 0, 0]);
            node
        };

        for node in [&unichar[..], &embedded, &wide] {
            let mut machine = booted();

            planted_table(&mut machine, &chain(node), TABLE + 12);
            machine.memory.write_run(TEXT, &[0xE1, 0x02]).unwrap();

            machine.string_table = TABLE;

            stream_string(&mut machine, TEXT, 0, 0).unwrap();

            assert_eq!(machine.memory.read_word(CURSOR).unwrap(), 0);
        }

        // A unistr node under the filter prints its low bytes.
        let mut fresh = booted();

        planted_table(&mut fresh, &chain(&wide), TABLE + 12);
        fresh.memory.write_run(TEXT, &[0xE1, 0x02]).unwrap();

        let mut program = SELECT_FILTER.to_vec();
        program.extend_from_slice(&[0x81, 0x41, 0x02, 0x01, 0x90]);
        program.extend_from_slice(&[0x72, 0x02, 0x01, 0xC0]);
        program.extend_from_slice(QUIT);
        fresh.memory.write_run(PLANT, &program).unwrap();

        fresh.pc = PLANT;

        fresh.run(Some(2000)).unwrap();

        assert_eq!(fresh.memory.read_run(BUFFER, 1).unwrap(), b"Y");

        // An argumentless indirect function call enters its
        // function with no arguments at all.
        let mut called = booted();
        let caller = {
            let mut node = vec![0x08];
            node.extend_from_slice(&FILTER.to_be_bytes());
            node
        };

        planted_table(&mut called, &chain(&caller), TABLE + 12);
        called.memory.write_run(TEXT, &[0xE1, 0x02]).unwrap();

        let mut program = SELECT_FILTER.to_vec();
        program.extend_from_slice(&[0x81, 0x41, 0x02, 0x01, 0x90]);
        program.extend_from_slice(&[0x72, 0x02, 0x01, 0xC0]);
        program.extend_from_slice(QUIT);
        called.memory.write_run(PLANT, &program).unwrap();

        called.pc = PLANT;

        called.run(Some(2000)).unwrap();

        assert_eq!(called.memory.read_run(BUFFER, 1).unwrap(), [0x00]);
    }

    // Glk mode walks empty embedded strings without a character to
    // refuse, and a number already fully printed has nothing left
    // to hand Glk either. (The delivering half of the reference
    // test waits for the Glk era.)
    #[test]
    fn glk_mode_survives_what_prints_nothing() {
        let empty_narrow = [0x03, 0x00];
        let empty_wide = {
            let mut node = vec![0x05];
            node.extend_from_slice(&[0, 0, 0, 0]);
            node
        };

        for node in [&empty_narrow[..], &empty_wide] {
            let mut machine = booted();

            planted_table(&mut machine, &chain(node), TABLE + 12);
            machine.memory.write_run(TEXT, &[0xE1, 0x02]).unwrap();

            machine.string_table = TABLE;
            machine.iosys.select(io_mode::GLK, 0);

            stream_string(&mut machine, TEXT, 0, 0).unwrap();
        }

        let mut spent = booted();

        spent.iosys.select(io_mode::GLK, 0);
        stream_num(&mut spent, 7, false, 1).unwrap();
    }

    // The bookkeeping opcodes: the table and io system read back
    // what was set, and an unknown mode lands as the null system.
    #[test]
    fn the_bookkeeping_opcodes_answer() {
        let mut machine = booted();

        let mut program = vec![0x81, 0x49, 0x21, 0x09, 0x00, 0x07];
        program.extend_from_slice(&[
            0x81, 0x48, 0x77, 0x00, 0x00, 0x02, 0xB0, 0x00, 0x00, 0x02, 0xB4,
        ]);
        program.extend_from_slice(&[0x81, 0x41, 0x02, 0x01, 0x90]);
        program.extend_from_slice(&[0x81, 0x40, 0x07, 0x00, 0x00, 0x02, 0xB8]);
        program.extend_from_slice(QUIT);

        machine.memory.write_run(PLANT, &program).unwrap();

        machine.pc = PLANT;

        machine.run(Some(100)).unwrap();

        assert_eq!(machine.memory.read_word(0x2B0).unwrap(), 0);
        assert_eq!(machine.memory.read_word(0x2B4).unwrap(), 0);
        assert_eq!(machine.memory.read_word(0x2B8).unwrap(), TABLE);
    }

    // A restart returns the machine to the null system and the
    // header's own decoding table.
    #[test]
    fn restart_resets_the_output_state() {
        let mut machine = booted();

        machine.iosys.select(io_mode::FILTER, FILTER);
        machine.string_table = TABLE;

        machine.restart().unwrap();

        assert_eq!(machine.iosys.mode, 0);
        assert_eq!(machine.string_table, 0x54);
    }
}
