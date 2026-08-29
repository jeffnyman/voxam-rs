//! The Glulx stack: call frames, locals, and call stubs.
//!
//! Byte-addressed and growing upward from zero (Glulx: The Stack),
//! the stack is where every function call builds its frame -- a
//! header, a locals-format list, the zeroed locals themselves --
//! and where every call leaves a four-word stub saying how to come
//! home (Glulx: The Call Frame, Glulx: Call Stubs). Unlike main
//! memory, stack access is strictly aligned: shorts at even
//! offsets, words at multiples of four. A program that breaks that
//! has undefined behavior, and undefined behavior gets caught
//! here, not tolerated.
//!
//! Two settled rulings ride along from the reference. The byte
//! order is big-endian even though the spec leaves it to the
//! interpreter and the vendored glulxe uses native order: the save
//! format stores the stack big-endian (Glulx: Contents of the
//! Stack), so storing it that way in the first place makes saving
//! a straight copy. And local references are bounds-checked: the
//! spec is explicit that a local reference "must not point outside
//! the range of the current function's locals segment", a check
//! glulxe skips with a note that a strict interpreter probably
//! should make. Voxam is that strict interpreter.

use crate::errors::VoxamError;

/// Boundaries sit on 256-byte seats (Glulx: The Stack).
const BOUNDARY: u32 = 256;

/// A call stub is four 32-bit words: DestType, DestAddr, PC, and
/// FramePtr (Glulx: Call Stubs).
const STUB_SIZE: u32 = 16;

/// A frame opens with FrameLen and LocalsPos, four bytes each
/// (Glulx: The Call Frame).
const FRAME_HEADER_SIZE: u32 = 8;

/// A locals-format entry is a LocalType byte and a LocalCount
/// byte; the legal types are 1, 2, and 4 (Glulx: The Call Frame).
const FORMAT_ENTRY_SIZE: u32 = 2;

/// Where a call stub's result lands (Glulx: Call Stubs).
///
/// The spec prints the string-resume values as "10" through "14"
/// with no radix marker, in a document that writes hex bare
/// everywhere else. Both reference implementations read them as
/// hexadecimal, so they are 16 through 20, not 10 through 14.
pub mod dest_type {
    pub const DISCARD: u32 = 0;
    pub const MEMORY: u32 = 1;
    pub const LOCAL: u32 = 2;
    pub const STACK: u32 = 3;

    /// Resuming an E1 compressed string; DestAddr holds the bit
    /// number within the byte.
    pub const RESUME_COMPRESSED: u32 = 0x10;
    /// Resuming function code after a string finishes.
    pub const RESUME_FUNCTION: u32 = 0x11;
    /// Resuming a signed decimal print; PC holds the number.
    pub const RESUME_NUMBER: u32 = 0x12;
    /// Resuming an E0 C-string.
    pub const RESUME_CSTRING: u32 = 0x13;
    /// Resuming an E2 Unicode string.
    pub const RESUME_UNICODE: u32 = 0x14;
}

/// The four words a call, catch, or string print leaves behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallStub {
    /// Where the result goes, a dest_type value.
    pub desttype: u32,
    /// The address or offset that destination reads.
    pub destaddr: u32,
    /// Where execution resumes.
    pub pc: u32,
    /// The frame to come home to.
    pub frameptr: u32,
}

/// One LocalType/LocalCount pair from a locals-format list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalsFormat {
    /// The type: a width of 1, 2, or 4 bytes.
    pub size: u8,
    /// How many locals of that width, 0 through 255.
    pub count: u8,
}

fn stack_error(message: String) -> VoxamError {
    VoxamError::GlulxStack(message)
}

/// Why a stack access was refused: off the stack, or unaligned.
fn refused(position: u32, width: u32, size: u32) -> VoxamError {
    if position > size.saturating_sub(width) {
        return stack_error(format!(
            "a {width}-byte access at {position} is off the {size}-byte stack \
             (Glulx: The Stack)"
        ));
    }

    stack_error(format!(
        "a {width}-byte stack access at {position} is off its natural alignment \
         (Glulx: The Call Frame)"
    ))
}

/// The value rounded up to its width's natural alignment.
fn aligned(value: u32, alignment: u32) -> u32 {
    let remainder = value % alignment;

    if remainder == 0 {
        value
    } else {
        value + alignment - remainder
    }
}

/// The Glulx value stack, its registers public for the machine.
pub struct Stack {
    size: u32,
    data: Vec<u8>,
    /// The stack pointer, counting bytes from zero.
    pub sp: u32,
    /// Where the current call frame begins.
    pub frameptr: u32,
    /// Where its locals segment begins -- what the locals
    /// addressing modes and DestType 2 offset from.
    pub localsbase: u32,
    /// Where its value stack begins -- the floor pops may not
    /// pass.
    pub valstackbase: u32,
}

impl Stack {
    /// Raise an empty stack of the header's declared size: a
    /// multiple of 256 at least 256 tall (Glulx: The Stack).
    pub fn new(size: u32) -> Result<Self, VoxamError> {
        if size < BOUNDARY || !size.is_multiple_of(BOUNDARY) {
            return Err(stack_error(format!(
                "a stack of {size} bytes is not a multiple of {BOUNDARY} at least \
                 {BOUNDARY} tall (Glulx: The Stack)"
            )));
        }

        Ok(Self {
            size,
            data: vec![0; size as usize],
            sp: 0,
            frameptr: 0,
            localsbase: 0,
            valstackbase: 0,
        })
    }

    /// The stack's full height in bytes.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Clear the stack whole -- restart's share of the work.
    pub fn reset(&mut self) {
        self.data.fill(0);
        self.sp = 0;
        self.frameptr = 0;
        self.localsbase = 0;
        self.valstackbase = 0;
    }

    /// The live bytes, ready for a save file's stack chunk: a
    /// straight copy, since the save format wants big-endian
    /// values and that is already how the stack stores them.
    pub fn snapshot(&self) -> Vec<u8> {
        self.data[..self.sp as usize].to_vec()
    }

    /// Replace the stack from a snapshot. The frame registers stay
    /// zeroed: a restore is completed by popping the call stub the
    /// saver pushed, and until then the bases mean nothing.
    pub fn restore(&mut self, data: &[u8]) -> Result<(), VoxamError> {
        if data.len() > self.size as usize {
            return Err(stack_error(format!(
                "a saved stack of {} bytes cannot fit this interpreter's {}-byte \
                 stack (Glulx: Contents of the Stack)",
                data.len(),
                self.size
            )));
        }

        if !data.len().is_multiple_of(4) {
            return Err(stack_error(format!(
                "a saved stack of {} bytes is not a whole number of words (Glulx: \
                 Contents of the Stack)",
                data.len()
            )));
        }

        self.data.fill(0);
        self.data[..data.len()].copy_from_slice(data);
        self.sp = data.len() as u32;
        self.frameptr = 0;
        self.localsbase = 0;
        self.valstackbase = 0;

        Ok(())
    }

    /// Read one byte of the stack.
    pub fn read_byte(&self, position: u32) -> Result<u8, VoxamError> {
        if position >= self.size {
            return Err(refused(position, 1, self.size));
        }

        Ok(self.data[position as usize])
    }

    /// Read a big-endian short at an even position.
    pub fn read_short(&self, position: u32) -> Result<u16, VoxamError> {
        if position > self.size - 2 || position & 1 != 0 {
            return Err(refused(position, 2, self.size));
        }

        let at = position as usize;

        Ok(u16::from_be_bytes([self.data[at], self.data[at + 1]]))
    }

    /// Read a big-endian word at a multiple of four.
    pub fn read_word(&self, position: u32) -> Result<u32, VoxamError> {
        if position > self.size - 4 || position & 3 != 0 {
            return Err(refused(position, 4, self.size));
        }

        let at = position as usize;

        Ok(u32::from_be_bytes([
            self.data[at],
            self.data[at + 1],
            self.data[at + 2],
            self.data[at + 3],
        ]))
    }

    /// Read at a local's width: 1, 2, or 4 bytes.
    pub fn read(&self, position: u32, width: u32) -> Result<u32, VoxamError> {
        match width {
            4 => self.read_word(position),
            1 => Ok(u32::from(self.read_byte(position)?)),
            _ => Ok(u32::from(self.read_short(position)?)),
        }
    }

    /// Write one byte of the stack.
    pub fn write_byte(&mut self, position: u32, value: u8) -> Result<(), VoxamError> {
        if position >= self.size {
            return Err(refused(position, 1, self.size));
        }

        self.data[position as usize] = value;

        Ok(())
    }

    /// Write a big-endian short at an even position.
    pub fn write_short(&mut self, position: u32, value: u16) -> Result<(), VoxamError> {
        if position > self.size - 2 || position & 1 != 0 {
            return Err(refused(position, 2, self.size));
        }

        let at = position as usize;
        self.data[at..at + 2].copy_from_slice(&value.to_be_bytes());

        Ok(())
    }

    /// Write a big-endian word at a multiple of four.
    pub fn write_word(&mut self, position: u32, value: u32) -> Result<(), VoxamError> {
        if position > self.size - 4 || position & 3 != 0 {
            return Err(refused(position, 4, self.size));
        }

        let at = position as usize;
        self.data[at..at + 4].copy_from_slice(&value.to_be_bytes());

        Ok(())
    }

    /// Write at a local's width, the value masked to it.
    pub fn write(&mut self, position: u32, width: u32, value: u32) -> Result<(), VoxamError> {
        match width {
            4 => self.write_word(position, value),
            1 => self.write_byte(position, (value & 0xFF) as u8),
            _ => self.write_short(position, (value & 0xFFFF) as u16),
        }
    }

    /// Push one word (Glulx: The Stack).
    pub fn push(&mut self, value: u32) -> Result<(), VoxamError> {
        if self.sp + 4 > self.size {
            return Err(stack_error(format!(
                "the {}-byte stack overflowed (Glulx: The Stack)",
                self.size
            )));
        }

        self.data[self.sp as usize..self.sp as usize + 4].copy_from_slice(&value.to_be_bytes());
        self.sp += 4;

        Ok(())
    }

    /// Pop one word; popping past the frame's value stack would
    /// eat the call frame (Glulx: The Call Frame).
    pub fn pop(&mut self) -> Result<u32, VoxamError> {
        if self.sp < self.valstackbase + 4 {
            return Err(stack_error(
                "the stack underflowed: popping past the value stack would eat the \
                 call frame (Glulx: The Call Frame)"
                    .into(),
            ));
        }

        self.sp -= 4;
        let at = self.sp as usize;

        Ok(u32::from_be_bytes([
            self.data[at],
            self.data[at + 1],
            self.data[at + 2],
            self.data[at + 3],
        ]))
    }

    /// Read a value without popping; depth 0 is the topmost --
    /// stkpeek's own error case for a depth past the value stack.
    pub fn peek(&self, depth: u32) -> Result<u32, VoxamError> {
        let below = 4u32
            .checked_mul(depth + 1)
            .and_then(|bytes| self.sp.checked_sub(bytes));

        match below {
            Some(position) if position >= self.valstackbase => {
                let at = position as usize;

                Ok(u32::from_be_bytes([
                    self.data[at],
                    self.data[at + 1],
                    self.data[at + 2],
                    self.data[at + 3],
                ]))
            }
            _ => Err(stack_error(format!(
                "a peek {depth} deep reaches past the value stack (Glulx: The Call \
                 Frame)"
            ))),
        }
    }

    /// Words above the current frame -- stkcount's answer.
    pub fn count(&self) -> u32 {
        (self.sp - self.valstackbase) / 4
    }

    /// Push DestType, DestAddr, PC, and FramePtr (Glulx: Call
    /// Stubs).
    pub fn push_stub(&mut self, desttype: u32, destaddr: u32, pc: u32) -> Result<(), VoxamError> {
        if self.sp + STUB_SIZE > self.size {
            return Err(stack_error(format!(
                "the {}-byte stack overflowed pushing a call stub (Glulx: Call Stubs)",
                self.size
            )));
        }

        self.write_word(self.sp, desttype)?;
        self.write_word(self.sp + 4, destaddr)?;
        self.write_word(self.sp + 8, pc)?;
        self.write_word(self.sp + 12, self.frameptr)?;
        self.sp += STUB_SIZE;

        Ok(())
    }

    /// Pop a call stub, restoring frameptr and the derived bases.
    /// The program counter and the storing of any result stay the
    /// caller's business: what those mean depends on the DestType.
    pub fn pop_stub(&mut self) -> Result<CallStub, VoxamError> {
        if self.sp < STUB_SIZE {
            return Err(stack_error(
                "the stack underflowed popping a call stub (Glulx: Call Stubs)".into(),
            ));
        }

        self.sp -= STUB_SIZE;
        let stub = CallStub {
            desttype: self.read_word(self.sp)?,
            destaddr: self.read_word(self.sp + 4)?,
            pc: self.read_word(self.sp + 8)?,
            frameptr: self.read_word(self.sp + 12)?,
        };

        self.frameptr = stub.frameptr;
        self.valstackbase = self.frameptr + self.read_word(self.frameptr)?;
        self.localsbase = self.frameptr + self.read_word(self.frameptr + 4)?;

        Ok(stub)
    }

    /// Build a call frame at sp and make it current.
    ///
    /// The locals arrive zeroed; placing arguments is the caller's
    /// business. Each run of locals pads up to its own natural
    /// alignment before it starts, the segment pads to a word, and
    /// the written format list ends with a zero pair -- twice when
    /// needed to stay word-aligned (Glulx: The Call Frame).
    pub fn push_frame(&mut self, locals_format: &[LocalsFormat]) -> Result<(), VoxamError> {
        for entry in locals_format {
            if !matches!(entry.size, 1 | 2 | 4) {
                return Err(stack_error(format!(
                    "a locals-format list may hold types 1, 2, and 4, not {} (Glulx: \
                     The Call Frame)",
                    entry.size
                )));
            }
        }

        let mut locals_length: u32 = 0;

        for entry in locals_format {
            locals_length = aligned(locals_length, u32::from(entry.size))
                + u32::from(entry.size) * u32::from(entry.count);
        }

        locals_length = aligned(locals_length, 4);

        // The written list ends with a zero pair, twice when needed
        // to stay word-aligned.
        let mut written: Vec<LocalsFormat> = locals_format.to_vec();
        written.push(LocalsFormat { size: 0, count: 0 });

        if !written.len().is_multiple_of(2) {
            written.push(LocalsFormat { size: 0, count: 0 });
        }

        let format_length = FORMAT_ENTRY_SIZE * written.len() as u32;
        let frameptr = self.sp;
        let localsbase = frameptr + FRAME_HEADER_SIZE + format_length;
        let valstackbase = localsbase + locals_length;

        if valstackbase >= self.size {
            return Err(stack_error(format!(
                "the {}-byte stack overflowed building a call frame (Glulx: The Call \
                 Frame)",
                self.size
            )));
        }

        self.frameptr = frameptr;
        self.localsbase = localsbase;
        self.valstackbase = valstackbase;

        self.write_word(frameptr, FRAME_HEADER_SIZE + format_length + locals_length)?;
        self.write_word(frameptr + 4, FRAME_HEADER_SIZE + format_length)?;

        let mut position = (frameptr + FRAME_HEADER_SIZE) as usize;

        for entry in &written {
            self.data[position] = entry.size;
            self.data[position + 1] = entry.count;
            position += FORMAT_ENTRY_SIZE as usize;
        }

        self.data[localsbase as usize..valstackbase as usize].fill(0);
        self.sp = valstackbase;

        Ok(())
    }

    /// Discard the current frame and everything pushed above it.
    pub fn leave_frame(&mut self) {
        self.sp = self.frameptr;
    }

    /// The current frame's whole length, off its own header.
    pub fn frame_len(&self) -> Result<u32, VoxamError> {
        self.read_word(self.frameptr)
    }

    /// Where the locals sit within the frame, off its header.
    pub fn locals_pos(&self) -> Result<u32, VoxamError> {
        self.read_word(self.frameptr + 4)
    }

    /// The locals segment's length in bytes, padding included.
    pub fn locals_length(&self) -> u32 {
        self.valstackbase - self.localsbase
    }

    /// Read the current frame's format list back off the stack.
    pub fn locals_format(&self) -> Vec<LocalsFormat> {
        let mut entries = Vec::new();
        let mut position = (self.frameptr + FRAME_HEADER_SIZE) as usize;

        while position + 2 <= self.localsbase as usize {
            let size = self.data[position];
            let count = self.data[position + 1];

            if size == 0 {
                break;
            }

            entries.push(LocalsFormat { size, count });
            position += 2;
        }

        entries
    }

    /// Read a local by its offset from localsbase -- what the
    /// locals addressing modes and a call stub's DestType 2 both
    /// carry.
    pub fn get_local(&self, offset: u32, width: u32) -> Result<u32, VoxamError> {
        self.require_local(offset, width)?;
        self.read(self.localsbase + offset, width)
    }

    /// Write a local by its offset from localsbase, masked.
    pub fn set_local(&mut self, offset: u32, value: u32, width: u32) -> Result<(), VoxamError> {
        self.require_local(offset, width)?;
        self.write(self.localsbase + offset, width, value)
    }

    /// Hold a local reference inside the locals segment: the
    /// spec's "must not point outside" made a real check, where
    /// glulxe skips it with a note that a strict interpreter
    /// probably should -- silent corruption made a diagnosable
    /// fault.
    fn require_local(&self, offset: u32, width: u32) -> Result<(), VoxamError> {
        if offset > self.locals_length().saturating_sub(width) {
            return Err(stack_error(format!(
                "a local reference at offset {offset} points outside the current \
                 function's locals segment (Glulx: The Call Frame)"
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack() -> Stack {
        Stack::new(512).unwrap()
    }

    #[test]
    fn the_dest_types_read_the_spec_in_hex() {
        assert_eq!(dest_type::RESUME_COMPRESSED, 16);
        assert_eq!(dest_type::RESUME_UNICODE, 20);
    }

    #[test]
    fn the_stack_is_raised_on_the_256_byte_convenience() {
        assert!(Stack::new(0).is_err());
        assert!(Stack::new(100).is_err());
        assert!(Stack::new(300).is_err());
        assert!(Stack::new(256).is_ok());
    }

    #[test]
    fn pushes_pops_and_peeks_hold_their_edges() {
        let mut stack = stack();

        stack.push(0x1111_2222).unwrap();
        stack.push(0x3333_4444).unwrap();

        assert_eq!(stack.count(), 2);
        assert_eq!(stack.peek(0).unwrap(), 0x3333_4444);
        assert_eq!(stack.peek(1).unwrap(), 0x1111_2222);
        assert!(stack.peek(2).is_err());

        assert_eq!(stack.pop().unwrap(), 0x3333_4444);
        assert_eq!(stack.pop().unwrap(), 0x1111_2222);
        assert!(stack.pop().is_err());

        for _ in 0..128 {
            stack.push(1).unwrap();
        }

        assert!(stack.push(1).is_err());
    }

    #[test]
    fn raw_access_is_aligned_or_loud() {
        let mut stack = stack();

        stack.write_word(4, 0xAABB_CCDD).unwrap();
        assert_eq!(stack.read_word(4).unwrap(), 0xAABB_CCDD);
        assert_eq!(stack.read_short(6).unwrap(), 0xCCDD);
        assert_eq!(stack.read_byte(5).unwrap(), 0xBB);

        assert!(stack.read_word(6).is_err());
        assert!(stack.read_short(5).is_err());
        assert!(stack.write_word(2, 0).is_err());
        assert!(stack.read_word(512).is_err());
    }

    #[test]
    fn a_frame_lays_down_by_the_spec() {
        let mut stack = stack();

        // Two format entries: 4x2 words, 1x3 bytes. The locals run
        // 8 bytes of words, then 3 bytes padded to 12; the format
        // list is 2 entries plus one zero pair, padded to 4 pairs.
        stack
            .push_frame(&[
                LocalsFormat { size: 4, count: 2 },
                LocalsFormat { size: 1, count: 3 },
            ])
            .unwrap();

        assert_eq!(stack.locals_pos().unwrap(), 8 + 8);
        assert_eq!(stack.frame_len().unwrap(), 8 + 8 + 12);
        assert_eq!(stack.localsbase, 16);
        assert_eq!(stack.valstackbase, 28);
        assert_eq!(stack.sp, 28);
        assert_eq!(
            stack.locals_format(),
            [
                LocalsFormat { size: 4, count: 2 },
                LocalsFormat { size: 1, count: 3 },
            ]
        );
    }

    #[test]
    fn locals_live_within_their_segment() {
        let mut stack = stack();
        stack
            .push_frame(&[LocalsFormat { size: 4, count: 2 }])
            .unwrap();

        stack.set_local(4, 0xDEAD_BEEF, 4).unwrap();
        assert_eq!(stack.get_local(4, 4).unwrap(), 0xDEAD_BEEF);
        assert_eq!(stack.get_local(0, 4).unwrap(), 0);

        assert!(stack.get_local(8, 4).is_err());
        assert!(stack.get_local(5, 4).is_err());
    }

    #[test]
    fn impossible_frames_are_refused() {
        let mut stack = stack();

        assert!(
            stack
                .push_frame(&[LocalsFormat { size: 3, count: 1 }])
                .is_err()
        );

        // A frame taller than the whole stack.
        let wide = vec![
            LocalsFormat {
                size: 4,
                count: 255
            };
            2
        ];
        assert!(stack.push_frame(&wide).is_err());
    }

    #[test]
    fn call_stubs_come_home() {
        let mut stack = stack();

        stack
            .push_frame(&[LocalsFormat { size: 4, count: 1 }])
            .unwrap();

        let home = stack.frameptr;
        stack.push_stub(dest_type::LOCAL, 0, 0x1234).unwrap();

        // A callee frame above the stub.
        stack
            .push_frame(&[LocalsFormat { size: 4, count: 2 }])
            .unwrap();
        stack.push(7).unwrap();

        stack.sp = stack.frameptr;
        let stub = stack.pop_stub().unwrap();

        assert_eq!(stub.desttype, dest_type::LOCAL);
        assert_eq!(stub.pc, 0x1234);
        assert_eq!(stub.frameptr, home);
        assert_eq!(stack.frameptr, home);
        assert_eq!(stack.localsbase, home + 8 + 4);
        assert_eq!(stack.valstackbase, home + 8 + 4 + 4);
    }

    #[test]
    fn snapshots_restore_and_reset_clears() {
        let mut stack = stack();
        stack.push(0x0102_0304).unwrap();
        stack.push(0x0506_0708).unwrap();

        let held = stack.snapshot();
        assert_eq!(held, [1, 2, 3, 4, 5, 6, 7, 8]);

        stack.reset();
        assert_eq!(stack.sp, 0);

        stack.restore(&held).unwrap();
        assert_eq!(stack.sp, 8);
        assert_eq!(stack.pop().unwrap(), 0x0506_0708);

        assert!(stack.restore(&[0; 3]).is_err());
        assert!(stack.restore(&vec![0; 516]).is_err());
    }
}
