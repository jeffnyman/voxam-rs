//! The opcode tables: names, version spans, and rider flags (§14).
//!
//! Each entry records only what full decoding needs to know about
//! an opcode: its name, which versions define it, and whether a
//! store byte, branch data, or literal text follows its operands.
//! Semantics arrive with execution, not here.
//!
//! Where the Python reference keeps dicts of version-span tuples,
//! this port writes each table as a match whose guards carry the
//! spans -- the compiler checks the shape, and a version fork reads
//! as two guarded arms. The knowledge is the same §14 table data,
//! hand-checked against the known version forks.

use crate::errors::VoxamError;

/// Which of the five §14 tables an opcode number is looked up in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpcodeKind {
    ZeroOp,
    OneOp,
    TwoOp,
    Var,
    Ext,
}

impl OpcodeKind {
    fn label(self) -> &'static str {
        match self {
            Self::ZeroOp => "0OP",
            Self::OneOp => "1OP",
            Self::TwoOp => "2OP",
            Self::Var => "VAR",
            Self::Ext => "EXT",
        }
    }
}

/// What decoding must know about one opcode (§14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opcode {
    /// The Inform name of the opcode.
    pub name: &'static str,
    /// Whether a store byte follows the operands (§4.6).
    pub stores: bool,
    /// Whether branch data follows (§4.7).
    pub branches: bool,
    /// Whether an encoded string follows (§3.2); true only for
    /// print and print_ret.
    pub has_text: bool,
}

const fn op(name: &'static str) -> Opcode {
    Opcode {
        name,
        stores: false,
        branches: false,
        has_text: false,
    }
}

impl Opcode {
    const fn stores(mut self) -> Self {
        self.stores = true;
        self
    }

    const fn branches(mut self) -> Self {
        self.branches = true;
        self
    }

    const fn text(mut self) -> Self {
        self.has_text = true;
        self
    }
}

/// Standard 1.1 reserves EXT opcodes 128-255 for private use
/// (§14.2) and asks that unknown extended opcodes from EXT:30 up be
/// simply ignored, perhaps with a warning somewhere off-screen
/// (§14.2.1). So the private band decodes as a silent rider-less
/// no-op -- a game carrying an extension for some other interpreter
/// passes through quietly -- and the band reserved for future
/// Standards, 30 through 127, decodes as its own no-op whose
/// handler warns off-screen as it passes. Below 30 is the
/// Standard's own table, where an unknown number stays the loud
/// error §14.2 asks for.
const PRIVATE_EXT_FLOOR: u8 = 0x80;
const RESERVED_EXT_FLOOR: u8 = 0x1E;
const PRIVATE_EXT: Opcode = op("ext_private");
const RESERVED_EXT: Opcode = op("ext_reserved");

fn two_op(number: u8, v: u8) -> Option<Opcode> {
    Some(match number {
        0x01 => op("je").branches(),
        0x02 => op("jl").branches(),
        0x03 => op("jg").branches(),
        0x04 => op("dec_chk").branches(),
        0x05 => op("inc_chk").branches(),
        0x06 => op("jin").branches(),
        0x07 => op("test").branches(),
        0x08 => op("or").stores(),
        0x09 => op("and").stores(),
        0x0A => op("test_attr").branches(),
        0x0B => op("set_attr"),
        0x0C => op("clear_attr"),
        0x0D => op("store"),
        0x0E => op("insert_obj"),
        0x0F => op("loadw").stores(),
        0x10 => op("loadb").stores(),
        0x11 => op("get_prop").stores(),
        0x12 => op("get_prop_addr").stores(),
        0x13 => op("get_next_prop").stores(),
        0x14 => op("add").stores(),
        0x15 => op("sub").stores(),
        0x16 => op("mul").stores(),
        0x17 => op("div").stores(),
        0x18 => op("mod").stores(),
        0x19 if v >= 4 => op("call_2s").stores(),
        0x1A if v >= 5 => op("call_2n"),
        0x1B if v >= 5 => op("set_colour"),
        0x1C if v >= 5 => op("throw"),
        _ => return None,
    })
}

fn one_op(number: u8, v: u8) -> Option<Opcode> {
    Some(match number {
        0x0 => op("jz").branches(),
        0x1 => op("get_sibling").stores().branches(),
        0x2 => op("get_child").stores().branches(),
        0x3 => op("get_parent").stores(),
        0x4 => op("get_prop_len").stores(),
        0x5 => op("inc"),
        0x6 => op("dec"),
        0x7 => op("print_addr"),
        0x8 if v >= 4 => op("call_1s").stores(),
        0x9 => op("remove_obj"),
        0xA => op("print_obj"),
        0xB => op("ret"),
        // jump's destination is an ordinary operand, not branch
        // data, so despite the "?(label)" syntax it carries no
        // branch rider (§14).
        0xC => op("jump"),
        0xD => op("print_paddr"),
        0xE => op("load").stores(),
        0xF if v <= 4 => op("not").stores(),
        0xF => op("call_1n"),
        _ => return None,
    })
}

fn zero_op(number: u8, v: u8) -> Option<Opcode> {
    Some(match number {
        0x0 => op("rtrue"),
        0x1 => op("rfalse"),
        0x2 => op("print").text(),
        0x3 => op("print_ret").text(),
        0x4 => op("nop"),
        // save and restore branch in Versions 1 to 3, store in
        // Version 4, and leave the 0OP table entirely in Version 5
        // (§14).
        0x5 if v <= 3 => op("save").branches(),
        0x5 if v == 4 => op("save").stores(),
        0x6 if v <= 3 => op("restore").branches(),
        0x6 if v == 4 => op("restore").stores(),
        0x7 => op("restart"),
        0x8 => op("ret_popped"),
        0x9 if v <= 4 => op("pop"),
        0x9 => op("catch").stores(),
        0xA => op("quit"),
        0xB => op("new_line"),
        0xC if v == 3 => op("show_status"),
        0xD if v >= 3 => op("verify").branches(),
        // 0OP:14 is deliberately absent: byte 0xBE marks an
        // extended-form instruction from Version 5 (§4.3), and is
        // not an opcode earlier.
        0xF if v >= 5 => op("piracy").branches(),
        _ => return None,
    })
}

fn var(number: u8, v: u8) -> Option<Opcode> {
    Some(match number {
        0x00 if v <= 3 => op("call").stores(),
        0x00 => op("call_vs").stores(),
        0x01 => op("storew"),
        0x02 => op("storeb"),
        0x03 => op("put_prop"),
        0x04 if v <= 4 => op("sread"),
        0x04 => op("aread").stores(),
        0x05 => op("print_char"),
        0x06 => op("print_num"),
        0x07 => op("random").stores(),
        0x08 => op("push"),
        // pull stores only in Version 6; Versions 7 and 8 revert to
        // the Version 5 behaviour, making the spans non-contiguous
        // (§14).
        0x09 if v == 6 => op("pull").stores(),
        0x09 => op("pull"),
        0x0A if v >= 3 => op("split_window"),
        0x0B if v >= 3 => op("set_window"),
        0x0C if v >= 4 => op("call_vs2").stores(),
        0x0D if v >= 4 => op("erase_window"),
        0x0E if v >= 4 => op("erase_line"),
        0x0F if v >= 4 => op("set_cursor"),
        0x10 if v >= 4 => op("get_cursor"),
        0x11 if v >= 4 => op("set_text_style"),
        0x12 if v >= 4 => op("buffer_mode"),
        0x13 if v >= 3 => op("output_stream"),
        0x14 if v >= 3 => op("input_stream"),
        // Officially Version 5, but The Lurking Horror uses it in 3
        // and the §14 table records that reality.
        0x15 if v >= 3 => op("sound_effect"),
        0x16 if v >= 4 => op("read_char").stores(),
        0x17 if v >= 4 => op("scan_table").stores().branches(),
        0x18 if v >= 5 => op("not").stores(),
        0x19 if v >= 5 => op("call_vn"),
        0x1A if v >= 5 => op("call_vn2"),
        0x1B if v >= 5 => op("tokenise"),
        0x1C if v >= 5 => op("encode_text"),
        0x1D if v >= 5 => op("copy_table"),
        0x1E if v >= 5 => op("print_table"),
        0x1F if v >= 5 => op("check_arg_count").branches(),
        _ => return None,
    })
}

fn ext(number: u8, v: u8) -> Option<Opcode> {
    Some(match number {
        0x00 if v >= 5 => op("save").stores(),
        0x01 if v >= 5 => op("restore").stores(),
        0x02 if v >= 5 => op("log_shift").stores(),
        0x03 if v >= 5 => op("art_shift").stores(),
        0x04 if v >= 5 => op("set_font").stores(),
        0x05 if v >= 6 => op("draw_picture"),
        0x06 if v >= 6 => op("picture_data").branches(),
        0x07 if v >= 6 => op("erase_picture"),
        0x08 if v >= 6 => op("set_margins"),
        0x09 if v >= 5 => op("save_undo").stores(),
        0x0A if v >= 5 => op("restore_undo").stores(),
        0x0B if v >= 5 => op("print_unicode"),
        0x0C if v >= 5 => op("check_unicode").stores(),
        0x0D if v >= 5 => op("set_true_colour"),
        0x10 if v >= 6 => op("move_window"),
        0x11 if v >= 6 => op("window_size"),
        0x12 if v >= 6 => op("window_style"),
        0x13 if v >= 6 => op("get_wind_prop").stores(),
        0x14 if v >= 6 => op("scroll_window"),
        0x15 if v >= 6 => op("pop_stack"),
        0x16 if v >= 6 => op("read_mouse"),
        0x17 if v >= 6 => op("mouse_window"),
        0x18 if v >= 6 => op("push_stack").branches(),
        0x19 if v >= 6 => op("put_wind_prop"),
        0x1A if v >= 6 => op("print_form"),
        0x1B if v >= 6 => op("make_menu").branches(),
        0x1C if v >= 6 => op("picture_table"),
        0x1D if v >= 6 => op("buffer_screen").stores(),
        // The arc_image band's one opcode, in the private range the
        // Standard reserves (§14.2): defined for Versions 5, 7, and
        // 8 as the contract defines it (arc_image: the contract,
        // part A). In Version 6 the number stays private and skips.
        0x80 if v == 5 || v >= 7 => op("draw_image"),
        _ => return None,
    })
}

/// Find the opcode a number means in a given version (§14).
pub fn lookup(kind: OpcodeKind, number: u8, version: u8) -> Result<Opcode, VoxamError> {
    let found = match kind {
        OpcodeKind::ZeroOp => zero_op(number, version),
        OpcodeKind::OneOp => one_op(number, version),
        OpcodeKind::TwoOp => two_op(number, version),
        OpcodeKind::Var => var(number, version),
        OpcodeKind::Ext => ext(number, version),
    };

    if let Some(opcode) = found {
        return Ok(opcode);
    }

    if kind == OpcodeKind::Ext && number >= PRIVATE_EXT_FLOOR {
        return Ok(PRIVATE_EXT);
    }

    if kind == OpcodeKind::Ext && number >= RESERVED_EXT_FLOOR {
        return Ok(RESERVED_EXT);
    }

    Err(VoxamError::ZMachineInstruction(format!(
        "{}:{number} is not an opcode in version {version} (§14)",
        kind.label()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_opcode_spans_all_versions() {
        for version in 1..=8 {
            let opcode = lookup(OpcodeKind::TwoOp, 0x14, version).unwrap();

            assert_eq!(opcode.name, "add");
            assert!(opcode.stores);
            assert!(!opcode.branches);
        }
    }

    #[test]
    fn jump_does_not_carry_a_branch_rider() {
        let opcode = lookup(OpcodeKind::OneOp, 0xC, 3).unwrap();

        assert_eq!(opcode.name, "jump");
        assert!(!opcode.branches);
    }

    #[test]
    fn scan_table_both_stores_and_branches() {
        let opcode = lookup(OpcodeKind::Var, 0x17, 5).unwrap();

        assert!(opcode.stores);
        assert!(opcode.branches);
    }

    #[test]
    fn private_ext_opcodes_pass_unclaimed() {
        let opcode = lookup(OpcodeKind::Ext, 0xC1, 5).unwrap();

        assert_eq!(opcode.name, "ext_private");
        assert!(!opcode.stores && !opcode.branches);

        let reserved = lookup(OpcodeKind::Ext, 0x40, 5).unwrap();
        assert_eq!(reserved.name, "ext_reserved");

        assert!(lookup(OpcodeKind::Ext, 0x0E, 5).is_err());
    }

    #[test]
    fn draw_image_skips_version_6() {
        for (version, name) in [(5, "draw_image"), (6, "ext_private"), (7, "draw_image")] {
            assert_eq!(lookup(OpcodeKind::Ext, 0x80, version).unwrap().name, name);
        }
    }

    #[test]
    fn only_the_print_opcodes_carry_text() {
        assert!(lookup(OpcodeKind::ZeroOp, 0x2, 3).unwrap().has_text);
        assert!(lookup(OpcodeKind::ZeroOp, 0x3, 3).unwrap().has_text);
        assert!(!lookup(OpcodeKind::ZeroOp, 0xB, 3).unwrap().has_text);
    }

    #[test]
    fn zero_op_9_forks_at_version_5() {
        assert_eq!(lookup(OpcodeKind::ZeroOp, 0x9, 4).unwrap().name, "pop");

        let catch = lookup(OpcodeKind::ZeroOp, 0x9, 5).unwrap();
        assert_eq!(catch.name, "catch");
        assert!(catch.stores);
    }

    #[test]
    fn zero_op_save_changes_rider_by_version() {
        assert!(lookup(OpcodeKind::ZeroOp, 0x5, 3).unwrap().branches);
        assert!(lookup(OpcodeKind::ZeroOp, 0x5, 4).unwrap().stores);
        assert!(lookup(OpcodeKind::ZeroOp, 0x5, 5).is_err());
    }

    #[test]
    fn one_op_15_forks_at_version_5() {
        assert_eq!(lookup(OpcodeKind::OneOp, 0xF, 4).unwrap().name, "not");
        assert_eq!(lookup(OpcodeKind::OneOp, 0xF, 5).unwrap().name, "call_1n");
    }

    #[test]
    fn var_0_is_renamed_at_version_4() {
        assert_eq!(lookup(OpcodeKind::Var, 0x00, 3).unwrap().name, "call");
        assert_eq!(lookup(OpcodeKind::Var, 0x00, 4).unwrap().name, "call_vs");
    }

    #[test]
    fn var_4_forks_at_version_5() {
        assert!(!lookup(OpcodeKind::Var, 0x04, 4).unwrap().stores);
        assert!(lookup(OpcodeKind::Var, 0x04, 5).unwrap().stores);
    }

    #[test]
    fn pull_stores_only_in_version_6() {
        assert!(!lookup(OpcodeKind::Var, 0x09, 5).unwrap().stores);
        assert!(lookup(OpcodeKind::Var, 0x09, 6).unwrap().stores);
        assert!(!lookup(OpcodeKind::Var, 0x09, 7).unwrap().stores);
    }
}
