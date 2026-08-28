//! Decoding complete Z-Machine instructions (§4).
//!
//! An instruction is opcode, operand types, operands, and then
//! riders -- store variable, branch offset, text -- whose presence
//! depends on which opcode it is (§4.1). The bit patterns determine
//! everything up to the riders; the opcode tables (§14) supply the
//! knowledge of what follows.

use crate::errors::VoxamError;
use crate::zmachine::memory::Memory;
use crate::zmachine::opcodes::{Opcode, OpcodeKind, lookup};
use crate::zmachine::riders::{Branch, read_branch, read_store_variable, text_end};

/// The top two bits of the opcode byte select the form (§4.3).
const FORM_MASK: u8 = 0b1100_0000;
const VARIABLE_FORM_BITS: u8 = 0b1100_0000;
const SHORT_FORM_BITS: u8 = 0b1000_0000;

/// Where each form keeps its opcode number (§4.3.1-3).
const BOTTOM_FIVE_MASK: u8 = 0b0001_1111;
const BOTTOM_FOUR_MASK: u8 = 0b0000_1111;

/// In variable form, bit 5 chooses between 2OP and VAR (§4.3.3).
const VAR_COUNT_BIT: u8 = 0b0010_0000;

/// Opcode 190 in Version 5 or later begins an extended-form
/// instruction (§4.3); below Version 5 the same byte is ordinary
/// short form.
const EXTENDED_OPCODE: u8 = 0xBE;
const EXTENDED_MIN_VERSION: u8 = 5;

/// In short form, bits 4 and 5 hold the operand type (§4.4.1). In
/// long form, bit 6 types the first operand and bit 5 the second
/// (§4.4.2).
const SHORT_TYPE_SHIFT: u8 = 4;
const LONG_FIRST_TYPE_BIT: u8 = 0b0100_0000;
const LONG_SECOND_TYPE_BIT: u8 = 0b0010_0000;

/// A type byte holds four 2-bit fields, first field in bits 7 and
/// 6, fourth in bits 1 and 0 (§4.4.3).
const TYPE_FIELD_SHIFTS: [u8; 4] = [6, 4, 2, 0];
const TYPE_MASK: u8 = 0b11;

/// call_vs2 (VAR:12) and call_vn2 (VAR:26) take a second type byte
/// for operands five through eight (§4.4.3.1).
const DOUBLE_TYPE_OPCODES: [u8; 2] = [0x0C, 0x1A];

/// The four instruction forms (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Long,
    Short,
    Variable,
    Extended,
}

/// The four operand counts (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandCount {
    Op0,
    Op1,
    Op2,
    Var,
}

/// The operand types, valued by their 2-bit codes (§4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandType {
    LargeConstant = 0b00,
    SmallConstant = 0b01,
    Variable = 0b10,
    Omitted = 0b11,
}

impl OperandType {
    fn from_bits(bits: u8) -> Self {
        match bits & TYPE_MASK {
            0b00 => Self::LargeConstant,
            0b01 => Self::SmallConstant,
            0b10 => Self::Variable,
            _ => Self::Omitted,
        }
    }
}

/// A decoded operand: how it was encoded and its raw value (§4.2).
/// For a `Variable` operand the value is a variable number, not yet
/// that variable's contents (§4.2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Operand {
    pub kind: OperandType,
    pub value: u16,
}

/// A single fully decoded instruction (§4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    /// The byte address the instruction begins at.
    pub address: usize,
    /// The instruction form (§4.3).
    pub form: Form,
    /// The operand count label (§4.3).
    pub operand_count: OperandCount,
    /// The opcode number within that count (§4.3).
    pub opcode_number: u8,
    /// What §14 knows about the opcode: its name and which riders
    /// it carries.
    pub opcode: Opcode,
    /// The decoded operands, in the order given (§4.5.2).
    pub operands: Vec<Operand>,
    /// The first byte address past the operands, where the riders
    /// begin.
    pub operands_end: usize,
    /// The variable number a result goes to, when the opcode stores
    /// (§4.6).
    pub store_variable: Option<u8>,
    /// The branch rider, when the opcode branches (§4.7).
    pub branch: Option<Branch>,
    /// The byte span of the literal string, when the opcode carries
    /// one (§3.2).
    pub text: Option<(usize, usize)>,
    /// The first byte address past the whole instruction: where
    /// execution continues.
    pub next_address: usize,
}

impl Instruction {
    /// Decode the instruction beginning at an address (§4.1).
    ///
    /// Fails if a type byte specifies an operand after an omitted
    /// one (§4.4.3), no opcode is defined for the decoded number in
    /// this version (§14), or the instruction runs outside the
    /// story file (§1.1).
    pub fn decode(memory: &Memory, address: usize) -> Result<Self, VoxamError> {
        let opcode_byte = memory.fetch_byte(address)?;
        let version = memory.header().version();
        let mut position = address + 1;

        let (form, operand_count, opcode_number, kinds) =
            if opcode_byte == EXTENDED_OPCODE && version >= EXTENDED_MIN_VERSION {
                let opcode_number = memory.fetch_byte(position)?;
                let kinds = field_types(&[memory.fetch_byte(position + 1)?])?;
                position += 2;

                (Form::Extended, OperandCount::Var, opcode_number, kinds)
            } else if opcode_byte & FORM_MASK == VARIABLE_FORM_BITS {
                let operand_count = if opcode_byte & VAR_COUNT_BIT != 0 {
                    OperandCount::Var
                } else {
                    OperandCount::Op2
                };
                let opcode_number = opcode_byte & BOTTOM_FIVE_MASK;

                let kinds = if operand_count == OperandCount::Var
                    && DOUBLE_TYPE_OPCODES.contains(&opcode_number)
                {
                    let kinds = field_types(&[
                        memory.fetch_byte(position)?,
                        memory.fetch_byte(position + 1)?,
                    ])?;
                    position += 2;

                    kinds
                } else {
                    let kinds = field_types(&[memory.fetch_byte(position)?])?;
                    position += 1;

                    kinds
                };

                (Form::Variable, operand_count, opcode_number, kinds)
            } else if opcode_byte & FORM_MASK == SHORT_FORM_BITS {
                let kind = OperandType::from_bits(opcode_byte >> SHORT_TYPE_SHIFT);
                let omitted = kind == OperandType::Omitted;
                let operand_count = if omitted {
                    OperandCount::Op0
                } else {
                    OperandCount::Op1
                };
                let kinds = if omitted { Vec::new() } else { vec![kind] };

                (
                    Form::Short,
                    operand_count,
                    opcode_byte & BOTTOM_FOUR_MASK,
                    kinds,
                )
            } else {
                let kinds = vec![
                    long_type(opcode_byte & LONG_FIRST_TYPE_BIT),
                    long_type(opcode_byte & LONG_SECOND_TYPE_BIT),
                ];

                (
                    Form::Long,
                    OperandCount::Op2,
                    opcode_byte & BOTTOM_FIVE_MASK,
                    kinds,
                )
            };

        let (operands, operands_end) = read_operands(memory, position, &kinds)?;
        let opcode = lookup(opcode_kind(form, operand_count), opcode_number, version)?;

        let mut position = operands_end;
        let mut store_variable = None;
        let mut branch = None;
        let mut text = None;

        if opcode.stores {
            let (variable, after) = read_store_variable(memory, position)?;
            store_variable = Some(variable);
            position = after;
        }

        if opcode.branches {
            let (decoded, after) = read_branch(memory, position)?;
            branch = Some(decoded);
            position = after;
        }

        if opcode.has_text {
            let end = text_end(memory, position)?;
            text = Some((position, end));
            position = end;
        }

        Ok(Self {
            address,
            form,
            operand_count,
            opcode_number,
            opcode,
            operands,
            operands_end,
            store_variable,
            branch,
            text,
            next_address: position,
        })
    }
}

/// Split type bytes into their operand types, first field first
/// (§4.4.3), without the omitted tail.
///
/// The double-variable opcodes pass two bytes here, giving eight
/// fields; the omitted-tail rule then applies across both
/// (§4.4.3.1). Fails if a field specifies an operand after an
/// omitted one, which §4.4.3 forbids.
fn field_types(type_bytes: &[u8]) -> Result<Vec<OperandType>, VoxamError> {
    let mut fields = Vec::with_capacity(4 * type_bytes.len());

    for type_byte in type_bytes {
        for shift in TYPE_FIELD_SHIFTS {
            fields.push(OperandType::from_bits(type_byte >> shift));
        }
    }

    let mut omitted_from = None;

    for (position, kind) in fields.iter().enumerate() {
        if *kind == OperandType::Omitted {
            omitted_from.get_or_insert(position);
        } else if omitted_from.is_some() {
            let shown: Vec<String> = type_bytes
                .iter()
                .map(|type_byte| format!("${type_byte:02x}"))
                .collect();

            return Err(VoxamError::ZMachineInstruction(format!(
                "type bytes {} specify an operand after an omitted one (§4.4.3)",
                shown.join(" ")
            )));
        }
    }

    let keep = omitted_from.unwrap_or(fields.len());
    fields.truncate(keep);

    Ok(fields)
}

/// Map a long-form type bit: 0 is small constant, 1 is variable
/// (§4.4.2).
fn long_type(bit: u8) -> OperandType {
    if bit != 0 {
        OperandType::Variable
    } else {
        OperandType::SmallConstant
    }
}

/// Pick the §14 table: extended form has its own, all else by count.
fn opcode_kind(form: Form, operand_count: OperandCount) -> OpcodeKind {
    if form == Form::Extended {
        return OpcodeKind::Ext;
    }

    match operand_count {
        OperandCount::Op0 => OpcodeKind::ZeroOp,
        OperandCount::Op1 => OpcodeKind::OneOp,
        OperandCount::Op2 => OpcodeKind::TwoOp,
        OperandCount::Var => OpcodeKind::Var,
    }
}

/// Read operand values of the given types starting at a position
/// (§4.5), returning them and the first address past them.
fn read_operands(
    memory: &Memory,
    position: usize,
    kinds: &[OperandType],
) -> Result<(Vec<Operand>, usize), VoxamError> {
    let mut operands = Vec::with_capacity(kinds.len());
    let mut position = position;

    for kind in kinds {
        if *kind == OperandType::LargeConstant {
            operands.push(Operand {
                kind: *kind,
                value: memory.fetch_word(position)?,
            });
            position += 2;
        } else {
            operands.push(Operand {
                kind: *kind,
                value: u16::from(memory.fetch_byte(position)?),
            });
            position += 1;
        }
    }

    Ok((operands, position))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zmachine::testing::planted_memory;

    fn decoded(version: u8, code: &[u8]) -> Instruction {
        let memory = planted_memory(version, &[(0x40, code)]);

        Instruction::decode(&memory, 0x40).unwrap()
    }

    #[test]
    fn decodes_long_form() {
        // add small small -> store
        let instruction = decoded(3, &[0x14, 0x05, 0x07, 0x00]);

        assert_eq!(instruction.address, 0x40);
        assert_eq!(instruction.form, Form::Long);
        assert_eq!(instruction.operand_count, OperandCount::Op2);
        assert_eq!(instruction.opcode.name, "add");
        assert_eq!(instruction.operands.len(), 2);
        assert_eq!(instruction.operands[0].kind, OperandType::SmallConstant);
        assert_eq!(instruction.operands[0].value, 5);
        assert_eq!(instruction.store_variable, Some(0));
        assert_eq!(instruction.next_address, 0x44);
    }

    #[test]
    fn long_form_types_come_from_bits_6_and_5() {
        // add with bit 6 set: first operand is a variable number.
        let instruction = decoded(3, &[0x54, 0x01, 0x07, 0x00]);

        assert_eq!(instruction.operands[0].kind, OperandType::Variable);
        assert_eq!(instruction.operands[1].kind, OperandType::SmallConstant);

        let instruction = decoded(3, &[0x74, 0x01, 0x02, 0x00]);

        assert_eq!(instruction.operands[0].kind, OperandType::Variable);
        assert_eq!(instruction.operands[1].kind, OperandType::Variable);
    }

    #[test]
    fn decodes_short_form_1op() {
        // jz with a large constant: short form, type bits 00.
        let instruction = decoded(3, &[0x80, 0x12, 0x34, 0xC5]);

        assert_eq!(instruction.form, Form::Short);
        assert_eq!(instruction.operand_count, OperandCount::Op1);
        assert_eq!(instruction.opcode.name, "jz");
        assert_eq!(instruction.operands[0].kind, OperandType::LargeConstant);
        assert_eq!(instruction.operands[0].value, 0x1234);
        assert!(instruction.branch.unwrap().on_true);
    }

    #[test]
    fn decodes_short_form_0op() {
        let instruction = decoded(3, &[0xB0]);

        assert_eq!(instruction.form, Form::Short);
        assert_eq!(instruction.operand_count, OperandCount::Op0);
        assert_eq!(instruction.opcode.name, "rtrue");
        assert!(instruction.operands.is_empty());
        assert_eq!(instruction.next_address, 0x41);
    }

    #[test]
    fn opcode_190_is_not_an_instruction_before_version_5() {
        // In v3, $BE decodes as short form 0OP:14 -- a number the
        // table deliberately leaves absent (§4.3, §14).
        let memory = planted_memory(3, &[(0x40, &[0xBE, 0x02, 0x0F])]);
        let error = Instruction::decode(&memory, 0x40).unwrap_err();

        assert!(error.to_string().contains("§14"));
    }

    #[test]
    fn opcode_190_is_extended_form_from_version_5() {
        // EXT:2 log_shift, one small constant and one omitted tail.
        let instruction = decoded(5, &[0xBE, 0x02, 0b0101_1111, 0x03, 0x02, 0x00]);

        assert_eq!(instruction.form, Form::Extended);
        assert_eq!(instruction.operand_count, OperandCount::Var);
        assert_eq!(instruction.opcode.name, "log_shift");
        assert_eq!(instruction.operands.len(), 2);
        assert_eq!(instruction.store_variable, Some(0));
    }

    #[test]
    fn decodes_variable_form_var() {
        // call with a large constant and two smalls (v3).
        let instruction = decoded(3, &[0xE0, 0b0001_0111, 0x12, 0x34, 0x05, 0x06, 0x00]);

        assert_eq!(instruction.form, Form::Variable);
        assert_eq!(instruction.operand_count, OperandCount::Var);
        assert_eq!(instruction.opcode.name, "call");
        assert_eq!(instruction.operands.len(), 3);
        assert_eq!(instruction.operands[0].value, 0x1234);
        assert_eq!(instruction.store_variable, Some(0));
    }

    #[test]
    fn variable_form_2op_can_carry_extra_operands() {
        // je in variable form with three operands (§4.3.3.1).
        let instruction = decoded(3, &[0xC1, 0b0101_0111, 0x01, 0x02, 0x03, 0x80]);

        assert_eq!(instruction.operand_count, OperandCount::Op2);
        assert_eq!(instruction.opcode.name, "je");
        assert_eq!(instruction.operands.len(), 3);
    }

    #[test]
    fn decodes_a_text_rider() {
        // print "hi" -- the encoded word B5C5 follows the opcode.
        let instruction = decoded(3, &[0xB2, 0xB5, 0xC5]);

        assert_eq!(instruction.opcode.name, "print");
        assert_eq!(instruction.text, Some((0x41, 0x43)));
        assert_eq!(instruction.next_address, 0x43);
    }

    #[test]
    fn decodes_the_double_type_bytes_of_call_vs2() {
        // call_vs2 with five small-constant operands across two
        // type bytes (§4.4.3.1).
        let instruction = decoded(
            5,
            &[
                0xEC,
                0b0101_0101,
                0b0111_1111,
                0x01,
                0x02,
                0x03,
                0x04,
                0x05,
                0x00,
            ],
        );

        assert_eq!(instruction.opcode.name, "call_vs2");
        assert_eq!(instruction.operands.len(), 5);
        assert_eq!(instruction.operands[4].value, 5);
        assert_eq!(instruction.store_variable, Some(0));
    }

    #[test]
    fn rejects_operand_specified_after_an_omitted_one() {
        let memory = planted_memory(3, &[(0x40, &[0xE0, 0b0011_0101, 0x01])]);
        let error = Instruction::decode(&memory, 0x40).unwrap_err();

        assert!(error.to_string().contains("§4.4.3"));
    }

    #[test]
    fn rejects_numbers_that_are_not_opcodes() {
        // 0OP:14 is not an opcode in version 3.
        let memory = planted_memory(3, &[(0x40, &[0xBD])]);

        assert!(Instruction::decode(&memory, 0x40).is_ok());

        let memory = planted_memory(1, &[(0x40, &[0xBC])]);
        let error = Instruction::decode(&memory, 0x40).unwrap_err();

        assert!(error.to_string().contains("§14"));
    }

    #[test]
    fn operands_cannot_run_past_readable_memory() {
        let memory = planted_memory(3, &[(0x1FF, &[0x14])]);

        assert!(Instruction::decode(&memory, 0x1FF).is_err());
    }
}
