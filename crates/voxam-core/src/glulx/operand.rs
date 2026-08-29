//! Operand decoding: opcodes, addressing modes, and stores.
//!
//! Everything here is (Glulx: Instruction Format): an opcode
//! number whose own top bits say whether it spans one, two, or
//! four bytes; then the operands' addressing modes, packed two
//! nibbles per byte; then the operand data itself. Operands are
//! evaluated strictly left to right -- the spec calls that out
//! because several modes pop the stack, and order is the
//! difference between right and wrong.
//!
//! The sixteen modes decode arithmetically rather than by table:
//! they fall into four groups of four -- constant, memory, local,
//! RAM -- so mode >> 2 selects the group and mode & 3 the
//! operand's width. This is the hottest loop the machine will
//! have.

use crate::errors::VoxamError;
use crate::glulx::memory::Memory;
use crate::glulx::stack::{Stack, dest_type};

/// The opcode number's own length rides in its top bits: below
/// 0x80 one byte, below 0xC0 two bytes less 0x8000, else four
/// bytes less 0xC0000000 -- so 01, 8001, and C0000001 all name
/// opcode 1 (Glulx: Instruction Format).
const ONE_BYTE_OPCODE_LIMIT: u8 = 0x80;
const TWO_BYTE_OPCODE_LIMIT: u8 = 0xC0;
const TWO_BYTE_OPCODE_BASE: u32 = 0x8000;
const FOUR_BYTE_OPCODE_BASE: u32 = 0xC000_0000;

/// mode >> 2 is the group -- constant, memory, local, RAM -- and
/// mode & 3 the width code: none, byte, short, word.
const CONSTANT_GROUP: u8 = 0;
const MEMORY_GROUP: u8 = 1;
const LOCAL_GROUP: u8 = 2;
const STACK_MODE: u8 = 8;

fn instruction_error(message: String) -> VoxamError {
    VoxamError::GlulxInstruction(message)
}

fn unknown_mode(mode: u8, direction: &str) -> VoxamError {
    instruction_error(format!(
        "addressing mode {mode} in a {direction} operand is not one the spec \
         defines (Glulx: Instruction Format)"
    ))
}

/// An opcode's operand signature: one letter per operand, L loads
/// and S stores, with the width the indirect modes move -- 4 for
/// almost every opcode; copyb and copys narrow it to 1 and 2
/// (Glulx: Instruction Format).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperandList {
    pub spec: &'static str,
    pub arg_size: u32,
}

/// Build an OperandList from a signature like "LLS".
pub const fn operands(spec: &'static str, arg_size: u32) -> OperandList {
    OperandList { spec, arg_size }
}

/// Where a store operand's value goes, once it is known: the same
/// vocabulary call stubs speak (Glulx: Call Stubs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreTarget {
    pub desttype: u32,
    pub addr: u32,
}

pub const DISCARD: StoreTarget = StoreTarget {
    desttype: dest_type::DISCARD,
    addr: 0,
};
pub const PUSH: StoreTarget = StoreTarget {
    desttype: dest_type::STACK,
    addr: 0,
};

/// One decoded operand: a load's unsigned value, or a store's
/// target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arg {
    Value(u32),
    Target(StoreTarget),
}

impl Arg {
    /// The load value this operand carries; the caller's signature
    /// guarantees which variant stands where.
    pub fn value(&self) -> u32 {
        match self {
            Arg::Value(value) => *value,
            Arg::Target(_) => unreachable!("a store operand asked for a load value"),
        }
    }

    /// The store target this operand carries.
    pub fn target(&self) -> StoreTarget {
        match self {
            Arg::Target(target) => *target,
            Arg::Value(_) => unreachable!("a load operand asked for a store target"),
        }
    }
}

/// Read the opcode number at pc, returning it and the address
/// just past it.
pub fn decode_opcode(memory: &Memory, pc: u32) -> Result<(u32, u32), VoxamError> {
    let first = memory.read_byte(pc)?;

    if first < ONE_BYTE_OPCODE_LIMIT {
        return Ok((u32::from(first), pc + 1));
    }

    if first < TWO_BYTE_OPCODE_LIMIT {
        return Ok((
            u32::from(memory.read_short(pc)?) - TWO_BYTE_OPCODE_BASE,
            pc + 2,
        ));
    }

    Ok((
        memory.read_word(pc)?.wrapping_sub(FOUR_BYTE_OPCODE_BASE),
        pc + 4,
    ))
}

/// Decode one instruction's operands, left to right: load operands
/// come back as unsigned values, store operands as targets.
pub fn decode_operands(
    memory: &Memory,
    stack: &mut Stack,
    pc: u32,
    oplist: &OperandList,
) -> Result<(Vec<Arg>, u32), VoxamError> {
    let forms = oplist.spec.as_bytes();
    let count = forms.len() as u32;
    let width = oplist.arg_size;

    // The mode nibbles come first, packed two per byte, then the
    // operand data; both are read in step, from two cursors.
    let mut modeaddr = pc;
    let mut pc = pc + count.div_ceil(2);

    let mut args = Vec::with_capacity(forms.len());
    let mut modeval: u8 = 0;

    for (index, form) in forms.iter().enumerate() {
        let mode = if index & 1 != 0 {
            let high = modeval >> 4;
            modeaddr += 1;

            high
        } else {
            modeval = memory.read_byte(modeaddr)?;

            modeval & 0x0F
        };

        let group = mode >> 2;
        let size = mode & 0b11;

        if *form == b'L' {
            let value = if group == CONSTANT_GROUP {
                match size {
                    0 => 0,
                    1 => {
                        let value = sign_extend(u32::from(memory.read_byte(pc)?), 8);
                        pc += 1;

                        value
                    }
                    2 => {
                        let value = sign_extend(u32::from(memory.read_short(pc)?), 16);
                        pc += 2;

                        value
                    }
                    _ => {
                        let value = memory.read_word(pc)?;
                        pc += 4;

                        value
                    }
                }
            } else if size == 0 {
                if mode != STACK_MODE {
                    return Err(unknown_mode(mode, "load"));
                }

                stack.pop()?
            } else {
                let addr = match size {
                    1 => {
                        let addr = u32::from(memory.read_byte(pc)?);
                        pc += 1;

                        addr
                    }
                    2 => {
                        let addr = u32::from(memory.read_short(pc)?);
                        pc += 2;

                        addr
                    }
                    _ => {
                        let addr = memory.read_word(pc)?;
                        pc += 4;

                        addr
                    }
                };

                match group {
                    MEMORY_GROUP => memory.read(addr, width)?,
                    LOCAL_GROUP => stack.get_local(addr, width)?,
                    // Address addition truncates to 32 bits, so a
                    // RAM offset near 0xFFFFFFFF wraps around below
                    // RAMSTART (Glulx: Instruction Format).
                    _ => memory.read(addr.wrapping_add(memory.ramstart()), width)?,
                }
            };

            args.push(Arg::Value(value));

            continue;
        }

        if size == 0 {
            args.push(match mode {
                0 => Arg::Target(DISCARD),
                STACK_MODE => Arg::Target(PUSH),
                _ => return Err(unknown_mode(mode, "store")),
            });

            continue;
        }

        if group == CONSTANT_GROUP {
            return Err(instruction_error(
                "a constant addressing mode cannot serve a store operand (Glulx: \
                 Instruction Format)"
                    .into(),
            ));
        }

        let addr = match size {
            1 => {
                let addr = u32::from(memory.read_byte(pc)?);
                pc += 1;

                addr
            }
            2 => {
                let addr = u32::from(memory.read_short(pc)?);
                pc += 2;

                addr
            }
            _ => {
                let addr = memory.read_word(pc)?;
                pc += 4;

                addr
            }
        };

        args.push(Arg::Target(match group {
            MEMORY_GROUP => StoreTarget {
                desttype: dest_type::MEMORY,
                addr,
            },
            // DestType 2 is relative to localsbase, not an
            // absolute stack position, so the offset stores as
            // decoded (Glulx: Call Stubs).
            LOCAL_GROUP => StoreTarget {
                desttype: dest_type::LOCAL,
                addr,
            },
            _ => StoreTarget {
                desttype: dest_type::MEMORY,
                addr: addr.wrapping_add(memory.ramstart()),
            },
        }));
    }

    Ok((args, pc))
}

/// Write a value where the target says. Call-stub destinations
/// arrive here too: the vocabulary is the same (Glulx: Call
/// Stubs). The width narrows only for copyb and copys -- and a
/// narrowed value pushed to the stack still lands as a full
/// four-byte word, exactly as the reference's store_operand_s
/// does.
pub fn store(
    memory: &mut Memory,
    stack: &mut Stack,
    target: StoreTarget,
    value: u32,
    width: u32,
) -> Result<(), VoxamError> {
    let value = match width {
        1 => value & 0xFF,
        2 => value & 0xFFFF,
        _ => value,
    };

    match target.desttype {
        dest_type::DISCARD => Ok(()),
        dest_type::MEMORY => memory.write(target.addr, width, value),
        dest_type::LOCAL => stack.set_local(target.addr, value, width),
        dest_type::STACK => stack.push(value),
        other => Err(instruction_error(format!(
            "a store reached destination type {other}, which the spec does not \
             define (Glulx: Call Stubs)"
        ))),
    }
}

/// The low bits of a value, sign-extended to unsigned 32 bits.
///
/// The value truncates to its low bits first: the operand modes
/// feed this already-narrow values, but sexb and sexs pass full
/// words and rely on the truncation.
pub fn sign_extend(value: u32, bits: u32) -> u32 {
    let mask: u32 = if bits >= 32 {
        u32::MAX
    } else {
        (1 << bits) - 1
    };
    let sign: u32 = 1 << (bits - 1);

    ((value & mask) ^ sign).wrapping_sub(sign)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glulx::story::Story;

    /// A 512-byte map with operand bytes planted at $60 (in ROM)
    /// and a scratch RAM word at $130.
    fn scene(plant: &[u8]) -> (Memory, Stack) {
        let mut data = vec![0u8; 256];
        data[..4].copy_from_slice(b"Glul");
        data[4..8].copy_from_slice(&0x0003_0102u32.to_be_bytes());
        data[8..12].copy_from_slice(&256u32.to_be_bytes());
        data[12..16].copy_from_slice(&256u32.to_be_bytes());
        data[16..20].copy_from_slice(&512u32.to_be_bytes());
        data[20..24].copy_from_slice(&256u32.to_be_bytes());
        data[0x60..0x60 + plant.len()].copy_from_slice(plant);
        data[0x40..0x44].copy_from_slice(&0xCAFE_F00Du32.to_be_bytes());

        let memory = Memory::new(&Story::new(data).unwrap());
        let stack = Stack::new(256).unwrap();

        (memory, stack)
    }

    #[test]
    fn opcode_numbers_carry_their_own_length() {
        let (memory, _) = scene(&[0x01, 0x81, 0x02, 0xC0, 0x00, 0x01, 0x10]);

        assert_eq!(decode_opcode(&memory, 0x60).unwrap(), (0x01, 0x61));
        assert_eq!(decode_opcode(&memory, 0x61).unwrap(), (0x0102, 0x63));
        assert_eq!(decode_opcode(&memory, 0x63).unwrap(), (0x0110, 0x67));
    }

    #[test]
    fn constant_modes_sign_extend_to_unsigned_words() {
        // Modes 1,2 then 3,0: byte -2, short -3, word, zero.
        let (memory, mut stack) = scene(&[0x21, 0x03, 0xFE, 0xFF, 0xFD, 0x12, 0x34, 0x56, 0x78]);

        let (args, end) = decode_operands(&memory, &mut stack, 0x60, &operands("LLLL", 4)).unwrap();

        assert_eq!(args[0].value(), 0xFFFF_FFFE);
        assert_eq!(args[1].value(), 0xFFFF_FFFD);
        assert_eq!(args[2].value(), 0x1234_5678);
        assert_eq!(args[3].value(), 0);
        assert_eq!(end, 0x60 + 2 + 1 + 2 + 4);
    }

    #[test]
    fn the_stack_mode_pops_left_to_right() {
        let (memory, mut stack) = scene(&[0x88]);
        stack.push(11).unwrap();
        stack.push(22).unwrap();

        let (args, _) = decode_operands(&memory, &mut stack, 0x60, &operands("LL", 4)).unwrap();

        assert_eq!(args[0].value(), 22);
        assert_eq!(args[1].value(), 11);
    }

    #[test]
    fn memory_locals_and_ram_modes_read_their_bases() {
        // Mode 5: memory, byte address $40. Mode 9: local, byte
        // offset 0. Mode D: RAM-relative, byte offset 4.
        let (mut memory, mut stack) = scene(&[0x95, 0x0D, 0x40, 0x00, 0x04]);
        stack
            .push_frame(&[crate::glulx::stack::LocalsFormat { size: 4, count: 1 }])
            .unwrap();
        stack.set_local(0, 0x5555_6666, 4).unwrap();
        memory.write_word(260, 0x0BAD_CAFE).unwrap();

        let (args, _) = decode_operands(&memory, &mut stack, 0x60, &operands("LLL", 4)).unwrap();

        assert_eq!(args[0].value(), 0xCAFE_F00D);
        assert_eq!(args[1].value(), 0x5555_6666);
        assert_eq!(args[2].value(), 0x0BAD_CAFE);
    }

    #[test]
    fn store_operands_become_targets() {
        let (memory, mut stack) = scene(&[0x85, 0x09, 0x40, 0x02]);

        let (args, _) = decode_operands(&memory, &mut stack, 0x60, &operands("SSS", 4)).unwrap();

        assert_eq!(
            args[0].target(),
            StoreTarget {
                desttype: dest_type::MEMORY,
                addr: 0x40
            }
        );
        assert_eq!(args[1].target(), PUSH);
        assert_eq!(
            args[2].target(),
            StoreTarget {
                desttype: dest_type::LOCAL,
                addr: 2
            }
        );
    }

    #[test]
    fn undefined_modes_halt_loudly() {
        let (memory, mut stack) = scene(&[0x04]);
        assert!(decode_operands(&memory, &mut stack, 0x60, &operands("L", 4)).is_err());

        let (memory, mut stack) = scene(&[0x01]);
        let error = decode_operands(&memory, &mut stack, 0x60, &operands("S", 4)).unwrap_err();
        assert!(error.to_string().contains("constant"));
    }

    #[test]
    fn store_speaks_every_destination() {
        let (mut memory, mut stack) = scene(&[]);
        stack
            .push_frame(&[crate::glulx::stack::LocalsFormat { size: 4, count: 1 }])
            .unwrap();

        store(
            &mut memory,
            &mut stack,
            StoreTarget {
                desttype: dest_type::MEMORY,
                addr: 300,
            },
            0x0102_0304,
            4,
        )
        .unwrap();
        assert_eq!(memory.read_word(300).unwrap(), 0x0102_0304);

        store(
            &mut memory,
            &mut stack,
            StoreTarget {
                desttype: dest_type::LOCAL,
                addr: 0,
            },
            7,
            4,
        )
        .unwrap();
        assert_eq!(stack.get_local(0, 4).unwrap(), 7);

        store(&mut memory, &mut stack, PUSH, 0xAA, 1).unwrap();
        assert_eq!(stack.pop().unwrap(), 0xAA);

        store(&mut memory, &mut stack, DISCARD, 1, 4).unwrap();

        let bad = StoreTarget {
            desttype: 9,
            addr: 0,
        };
        assert!(store(&mut memory, &mut stack, bad, 1, 4).is_err());
    }

    #[test]
    fn sign_extend_truncates_then_widens() {
        assert_eq!(sign_extend(0x7F, 8), 0x7F);
        assert_eq!(sign_extend(0x80, 8), 0xFFFF_FF80);
        assert_eq!(sign_extend(0x1234_5680, 8), 0xFFFF_FF80);
        assert_eq!(sign_extend(0x8000, 16), 0xFFFF_8000);
        assert_eq!(sign_extend(0x7FFF, 16), 0x7FFF);
    }
}
