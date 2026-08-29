//! Accelerated functions (Glulx: Accelerated Functions).
//!
//! A game may ask that calls to one of its own functions be
//! replaced by a built-in equivalent. These are Inform library
//! veneer routines -- property lookup, ofclass, and friends --
//! which dominate its running time. The spec calls the idea
//! "outrageously CISC".
//!
//! Functions 2 through 7 are deprecated: they assume
//! NUM_ATTR_BYTES has its default value of 7 and misbehave
//! otherwise. Functions 8 through 13 are the same routines with
//! that assumption removed. Both sets are carried, because an
//! older game file will ask for the older ones.
//!
//! On errors: the spec allows an accelerated function to report
//! them "by some convenient means", and notes that discarding them
//! is the safer choice when the I/O system is not Glk. Since every
//! error here means the game asked about an address that is not
//! what it claims to be, Voxam discards them and answers what the
//! Inform original would -- each discarded report is marked in
//! place. Following the port's usual arrangement, the accelerator
//! does not hold the memory map: calls take it as an argument, and
//! lookup answers the installed function number rather than a
//! closure.

use std::collections::HashMap;

use crate::errors::VoxamError;
use crate::glulx::memory::Memory;
use crate::glulx::search::binary_search;

const WORD: u32 = 4;

// Inform's own layout constants, as the veneer compiles them: the
// type bytes an address is classified by, the RAMSTART word in the
// header, and the property-entry shape.
const HEADER_END: u32 = 36;
const RAMSTART_AT: u32 = 8;
const STRING_TYPE: u8 = 0xE0;
const FUNCTION_TYPE: u8 = 0xC0;
const OBJECT_TYPE_LOW: u8 = 0x70;
const OBJECT_TYPE_HIGH: u8 = 0x7F;
const PROPERTY_ENTRY: u32 = 10;
const INDIV_RANGE: u64 = 8;

// The classified regions Z__Region answers.
const OBJECT: u32 = 1;
const ROUTINE: u32 = 2;
const STRING: u32 = 3;

/// The parameter count (Glulx: Accelerated Functions): 0
/// classes_table, 1 indiv_prop_start, 2 class_metaclass, 3
/// object_metaclass, 4 routine_metaclass, 5 string_metaclass, 6
/// self, 7 num_attr_bytes, 8 cpv__start. Every entry starts at
/// zero.
const PARAM_COUNT: usize = 9;

/// The function numbers this interpreter implements.
pub const AVAILABLE: [u32; 13] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];

/// One call argument, zero where none arrived -- as it would read
/// in a real call with unfilled locals.
fn arg(args: &[u32], index: usize) -> u32 {
    args.get(index).copied().unwrap_or(0)
}

/// The acceleration table for one machine.
///
/// Neither the installed functions nor the parameter values are
/// part of saved state (Glulx: Accelerated Functions), so nothing
/// here is serialized -- and nothing survives into a save or out
/// of a restore.
#[derive(Debug, Default)]
pub struct Accelerator {
    /// The parameter values, by number.
    pub params: [u32; PARAM_COUNT],
    installed: HashMap<u32, u32>,
}

impl Accelerator {
    /// A table with nothing installed.
    pub fn new() -> Self {
        Self::default()
    }

    /// The accelfunc opcode's work.
    ///
    /// Index zero cancels. Asking for a function Voxam does not
    /// implement is silently ignored, which is what lets a game
    /// request acceleration unconditionally and trust the gestalt
    /// (Glulx: Accelerated Functions).
    pub fn set_func(&mut self, index: u32, address: u32) {
        self.installed.remove(&address);

        if index != 0 && AVAILABLE.contains(&index) {
            self.installed.insert(address, index);
        }
    }

    /// The accelparam opcode's work; unknown numbers ignored.
    pub fn set_param(&mut self, index: u32, value: u32) {
        if let Some(slot) = self.params.get_mut(index as usize) {
            *slot = value;
        }
    }

    /// The installed replacement for a function address, if any:
    /// the function number to hand to `call`.
    pub fn lookup(&self, address: u32) -> Option<u32> {
        self.installed.get(&address).copied()
    }

    /// Run one accelerated function by number.
    pub fn call(&self, memory: &Memory, index: u32, args: &[u32]) -> Result<u32, VoxamError> {
        let obj = arg(args, 0);
        let prop_id = arg(args, 1);

        match index {
            1 => self.z_region(memory, obj),
            2 => self.cp_tab(memory, obj, prop_id, false),
            3 => self.ra_pr(memory, obj, prop_id, false),
            4 => self.rl_pr(memory, obj, prop_id, false),
            5 => self.oc_cl(memory, obj, prop_id, false),
            6 => self.rv_pr(memory, obj, prop_id, false),
            7 => self.op_pr(memory, obj, prop_id, false),
            8 => self.cp_tab(memory, obj, prop_id, true),
            9 => self.ra_pr(memory, obj, prop_id, true),
            10 => self.rl_pr(memory, obj, prop_id, true),
            11 => self.oc_cl(memory, obj, prop_id, true),
            12 => self.rv_pr(memory, obj, prop_id, true),
            13 => self.op_pr(memory, obj, prop_id, true),
            // Unreachable through lookup: set_func filters to the
            // available numbers.
            _ => Ok(0),
        }
    }

    // -- the parameters, named --------------------------------------------

    fn classes_table(&self) -> u32 {
        self.params[0]
    }

    fn indiv_prop_start(&self) -> u32 {
        self.params[1]
    }

    fn class_metaclass(&self) -> u32 {
        self.params[2]
    }

    fn object_metaclass(&self) -> u32 {
        self.params[3]
    }

    fn routine_metaclass(&self) -> u32 {
        self.params[4]
    }

    fn string_metaclass(&self) -> u32 {
        self.params[5]
    }

    fn self_addr(&self) -> u32 {
        self.params[6]
    }

    fn num_attr_bytes(&self) -> u32 {
        self.params[7]
    }

    fn cpv_start(&self) -> u32 {
        self.params[8]
    }

    // -- shared machinery -------------------------------------------------

    /// Whether an object is a class -- in Class, not of it.
    fn obj_in_class(&self, memory: &Memory, obj: u32) -> Result<bool, VoxamError> {
        let at = obj.wrapping_add(13).wrapping_add(self.num_attr_bytes());

        Ok(memory.read_word(at)? == self.class_metaclass())
    }

    /// Function 1: an address as object, routine, or string.
    fn z_region(&self, memory: &Memory, address: u32) -> Result<u32, VoxamError> {
        if address < HEADER_END || address >= memory.endmem() {
            return Ok(0);
        }

        let kind = memory.read_byte(address)?;

        if kind >= STRING_TYPE {
            return Ok(STRING);
        }

        if kind >= FUNCTION_TYPE {
            return Ok(ROUTINE);
        }

        // 0x70..0x7F is Inform's object type byte, but only in
        // RAM; the header word at address 8 is RAMSTART.
        if (OBJECT_TYPE_LOW..=OBJECT_TYPE_HIGH).contains(&kind)
            && address >= memory.read_word(RAMSTART_AT)?
        {
            return Ok(OBJECT);
        }

        Ok(0)
    }

    /// Functions 2 and 8: a property entry in an object's table.
    ///
    /// The two differ only in where the table pointer lives: the
    /// older form hardcodes obj-->4, right only when
    /// NUM_ATTR_BYTES is 7; the newer derives it.
    fn cp_tab(
        &self,
        memory: &Memory,
        obj: u32,
        prop_id: u32,
        new: bool,
    ) -> Result<u32, VoxamError> {
        if self.z_region(memory, obj)? != OBJECT {
            // ERROR, discarded: asked for the property table of a
            // non-object.
            return Ok(0);
        }

        let offset = if new {
            4 * (3 + self.num_attr_bytes() / 4)
        } else {
            16
        };
        let table = memory.read_word(obj.wrapping_add(offset))?;

        if table == 0 {
            return Ok(0);
        }

        let count = memory.read_word(table)?;

        binary_search(
            memory,
            prop_id,
            2,
            table.wrapping_add(4),
            PROPERTY_ENTRY,
            count,
            0,
            0,
        )
    }

    /// The property-entry core RA__Pr, RL__Pr, and OP__Pr share.
    fn get_prop(
        &self,
        memory: &Memory,
        obj: u32,
        prop_id: u32,
        new: bool,
    ) -> Result<u32, VoxamError> {
        let mut obj = obj;
        let mut prop_id = prop_id;
        let mut cla = 0;

        if prop_id & 0xFFFF_0000 != 0 {
            // A composite id: the low half indexes the classes
            // table, the high half is the property itself.
            cla = memory.read_word(
                self.classes_table()
                    .wrapping_add((prop_id & 0xFFFF).wrapping_mul(4)),
            )?;

            if self.oc_cl(memory, obj, cla, new)? == 0 {
                return Ok(0);
            }

            prop_id >>= 16;
            obj = cla;
        }

        let prop = self.cp_tab(memory, obj, prop_id, new)?;

        if prop == 0 {
            return Ok(0);
        }

        if self.obj_in_class(memory, obj)? && cla == 0 {
            // A class only shows its individual properties when
            // asked directly.
            let start = u64::from(self.indiv_prop_start());

            if !(start..start + INDIV_RANGE).contains(&u64::from(prop_id)) {
                return Ok(0);
            }
        }

        // A property flagged as protected is invisible unless the
        // global self is this object -- the veneer's "@aloadbit
        // prop 72", which is bit 0 of the byte at prop+9.
        if memory.read_word(self.self_addr())? != obj
            && memory.read_byte(prop.wrapping_add(9))? & 1 != 0
        {
            return Ok(0);
        }

        Ok(prop)
    }

    /// Functions 3 and 9: a property's data address, or 0.
    fn ra_pr(&self, memory: &Memory, obj: u32, prop_id: u32, new: bool) -> Result<u32, VoxamError> {
        let prop = self.get_prop(memory, obj, prop_id, new)?;

        if prop == 0 {
            Ok(0)
        } else {
            memory.read_word(prop.wrapping_add(4))
        }
    }

    /// Functions 4 and 10: a property's length in bytes, or 0.
    fn rl_pr(&self, memory: &Memory, obj: u32, prop_id: u32, new: bool) -> Result<u32, VoxamError> {
        let prop = self.get_prop(memory, obj, prop_id, new)?;

        if prop == 0 {
            return Ok(0);
        }

        Ok(WORD * u32::from(memory.read_short(prop.wrapping_add(2))?))
    }

    /// Functions 5 and 11: Inform's ofclass.
    fn oc_cl(&self, memory: &Memory, obj: u32, cla: u32, new: bool) -> Result<u32, VoxamError> {
        let region = self.z_region(memory, obj)?;

        if region == STRING {
            return Ok(u32::from(cla == self.string_metaclass()));
        }

        if region == ROUTINE {
            return Ok(u32::from(cla == self.routine_metaclass()));
        }

        if region != OBJECT {
            return Ok(0);
        }

        let metaclasses = [
            self.class_metaclass(),
            self.string_metaclass(),
            self.routine_metaclass(),
            self.object_metaclass(),
        ];

        if cla == self.class_metaclass() {
            return Ok(u32::from(
                self.obj_in_class(memory, obj)? || metaclasses.contains(&obj),
            ));
        }

        if cla == self.object_metaclass() {
            return Ok(u32::from(
                !(self.obj_in_class(memory, obj)? || metaclasses.contains(&obj)),
            ));
        }

        if cla == self.string_metaclass() || cla == self.routine_metaclass() {
            return Ok(0);
        }

        if !self.obj_in_class(memory, cla)? {
            // ERROR, discarded: ofclass applied to a non-class.
            return Ok(0);
        }

        let inlist = self.ra_pr(memory, obj, 2, new)?;

        if inlist == 0 {
            return Ok(0);
        }

        let count = self.rl_pr(memory, obj, 2, new)? / WORD;

        for index in 0..count {
            if memory.read_word(inlist.wrapping_add(4u32.wrapping_mul(index)))? == cla {
                return Ok(1);
            }
        }

        Ok(0)
    }

    /// Functions 6 and 12: a property's value, or its default.
    fn rv_pr(&self, memory: &Memory, obj: u32, prop_id: u32, new: bool) -> Result<u32, VoxamError> {
        let address = self.ra_pr(memory, obj, prop_id, new)?;

        if address == 0 {
            if 0 < prop_id && prop_id < self.indiv_prop_start() {
                return memory.read_word(self.cpv_start().wrapping_add(prop_id.wrapping_mul(4)));
            }

            // ERROR, discarded: read of a property the object does
            // not have.
            return Ok(0);
        }

        memory.read_word(address)
    }

    /// Functions 7 and 13: Inform's provides.
    fn op_pr(&self, memory: &Memory, obj: u32, prop_id: u32, new: bool) -> Result<u32, VoxamError> {
        let region = self.z_region(memory, obj)?;
        let start = u64::from(self.indiv_prop_start());
        let prop = u64::from(prop_id);

        if region == STRING {
            // A string provides print and print_to_array.
            return Ok(u32::from(prop == start + 6 || prop == start + 7));
        }

        if region == ROUTINE {
            // A routine provides call.
            return Ok(u32::from(prop == start + 5));
        }

        if region != OBJECT {
            return Ok(0);
        }

        if (start..start + INDIV_RANGE).contains(&prop) && self.obj_in_class(memory, obj)? {
            return Ok(1);
        }

        Ok(u32::from(self.ra_pr(memory, obj, prop_id, new)? != 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glulx::story::Story;

    // The miniature Inform world, laid out in RAM.
    const SELF_GLOBAL: u32 = 0x110;
    const CPV_START: u32 = 0x120;
    const CLASSES_TABLE: u32 = 0x140;
    const K: u32 = 0x160;
    const K2: u32 = 0x190;
    const OBJ: u32 = 0x1B0;
    const CLASS_MC: u32 = 0x1E0;
    const O_PTABLE: u32 = 0x200;
    const K_PTABLE: u32 = 0x228;
    const INLIST: u32 = 0x250;
    const VAL3: u32 = 0x258;
    const VAL5: u32 = 0x260;
    const KVAL3: u32 = 0x270;
    const KVAL42: u32 = 0x278;
    const STRINGISH: u32 = 0x290;
    const FUNC: u32 = 0x2C8;

    const OBJ_MC: u32 = 0x0111;
    const ROUT_MC: u32 = 0x0222;
    const STR_MC: u32 = 0x0333;
    const INDIV: u32 = 0x40;

    fn entry(prop_id: u16, length: u16, address: u32, protected: bool) -> Vec<u8> {
        let mut bytes = Vec::new();

        bytes.extend_from_slice(&prop_id.to_be_bytes());
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(&address.to_be_bytes());
        bytes.extend_from_slice(if protected {
            &[0x00, 0x01]
        } else {
            &[0x00, 0x00]
        });

        bytes
    }

    /// A memory and accelerator holding a two-class, one-object
    /// Inform world, standing in for the reference suite's booted
    /// machine.
    fn world() -> (Memory, Accelerator) {
        let mut data = vec![0u8; 256];
        data[..4].copy_from_slice(b"Glul");
        data[4..8].copy_from_slice(&0x0003_0102u32.to_be_bytes());
        data[8..12].copy_from_slice(&256u32.to_be_bytes());
        data[12..16].copy_from_slice(&256u32.to_be_bytes());
        data[16..20].copy_from_slice(&0x300u32.to_be_bytes());
        data[20..24].copy_from_slice(&256u32.to_be_bytes());

        // The idle main's C0 byte at $48, the 0x70 byte at $50.
        data[0x48..0x4D].copy_from_slice(&[0xC0, 0x00, 0x00, 0x81, 0x20]);
        data[0x50] = 0x70;

        let mut memory = Memory::new(&Story::new(data).unwrap());

        for (address, type_byte, table, metaclass) in [
            (K, 0x70, K_PTABLE, CLASS_MC),
            (K2, 0x70, 0, CLASS_MC),
            (OBJ, 0x71, O_PTABLE, 0),
            (CLASS_MC, 0x70, 0, CLASS_MC),
        ] {
            memory.write_byte(address, type_byte).unwrap();
            memory.write_word(address + 16, table).unwrap();
            memory.write_word(address + 20, metaclass).unwrap();
        }

        memory.write_word(CLASSES_TABLE + 4, K).unwrap();

        memory.write_word(O_PTABLE, 3).unwrap();

        let mut entries = entry(2, 1, INLIST, false);
        entries.extend(entry(3, 2, VAL3, false));
        entries.extend(entry(5, 1, VAL5, true));
        memory.write_run(O_PTABLE + 4, &entries).unwrap();

        memory.write_word(K_PTABLE, 2).unwrap();

        let mut entries = entry(3, 1, KVAL3, false);
        entries.extend(entry((INDIV + 2) as u16, 1, KVAL42, false));
        memory.write_run(K_PTABLE + 4, &entries).unwrap();

        memory.write_word(INLIST, K).unwrap();
        memory.write_word(VAL3, 0x1111).unwrap();
        memory.write_word(VAL3 + 4, 0x2222).unwrap();
        memory.write_word(VAL5, 0x5555).unwrap();
        memory.write_word(KVAL3, 0x3333).unwrap();
        memory.write_word(KVAL42, 0x4242).unwrap();
        memory.write_byte(STRINGISH, 0xE0).unwrap();
        memory
            .write_run(FUNC, &[0xC1, 0x00, 0x00, 0x31, 0x01, 0x2A])
            .unwrap();
        memory.write_word(CPV_START + 4 * 4, 0xD4D4).unwrap();

        let mut accel = Accelerator::new();

        for (index, value) in [
            CLASSES_TABLE,
            INDIV,
            CLASS_MC,
            OBJ_MC,
            ROUT_MC,
            STR_MC,
            SELF_GLOBAL,
            7,
            CPV_START,
        ]
        .into_iter()
        .enumerate()
        {
            accel.set_param(index as u32, value);
        }

        (memory, accel)
    }

    // Z__Region sorts every address: the header and beyond-memory
    // are nothing, E0 is a string, C0 a routine, and 0x70 an
    // object -- but only in RAM, where the header's own RAMSTART
    // word draws the line.
    #[test]
    fn z_region_sorts_addresses() {
        let (memory, accel) = world();

        let answers = [
            (35, 0),
            (0x300, 0),
            (STRINGISH, 3),
            (0x48, 2),
            (OBJ, 1),
            (K, 1),
            (0x50, 0),
            (SELF_GLOBAL, 0),
        ];

        for (address, expected) in answers {
            assert_eq!(accel.call(&memory, 1, &[address]).unwrap(), expected);
        }

        // A missing argument reads as zero, like an unfilled
        // local.
        assert_eq!(accel.call(&memory, 1, &[]).unwrap(), 0);
    }

    // CP__Tab finds a property entry by binary search -- or
    // answers 0 for a non-object, an object with no table, or an
    // absent id. The old and new forms agree at the default
    // attribute width.
    #[test]
    fn property_entries_are_found() {
        let (memory, accel) = world();
        let third = O_PTABLE + 4 + 10;

        assert_eq!(accel.call(&memory, 2, &[OBJ, 3]).unwrap(), third);
        assert_eq!(accel.call(&memory, 8, &[OBJ, 3]).unwrap(), third);
        assert_eq!(accel.call(&memory, 2, &[OBJ, 4]).unwrap(), 0);
        assert_eq!(accel.call(&memory, 2, &[STRINGISH, 3]).unwrap(), 0);
        assert_eq!(accel.call(&memory, 2, &[CLASS_MC, 3]).unwrap(), 0);
    }

    // RA__Pr and RL__Pr answer a property's data address and byte
    // length; a protected property is invisible until the global
    // self is the object itself, and a class hides all but its
    // individual properties.
    #[test]
    fn addresses_lengths_and_protection() {
        let (mut memory, accel) = world();

        assert_eq!(accel.call(&memory, 3, &[OBJ, 3]).unwrap(), VAL3);
        assert_eq!(accel.call(&memory, 9, &[OBJ, 3]).unwrap(), VAL3);
        assert_eq!(accel.call(&memory, 4, &[OBJ, 3]).unwrap(), 8);
        assert_eq!(accel.call(&memory, 10, &[OBJ, 3]).unwrap(), 8);
        assert_eq!(accel.call(&memory, 3, &[OBJ, 4]).unwrap(), 0);
        assert_eq!(accel.call(&memory, 4, &[OBJ, 4]).unwrap(), 0);

        assert_eq!(accel.call(&memory, 3, &[OBJ, 5]).unwrap(), 0);

        memory.write_word(SELF_GLOBAL, OBJ).unwrap();

        assert_eq!(accel.call(&memory, 3, &[OBJ, 5]).unwrap(), VAL5);

        // K is a class: its common property is hidden, its
        // individual one is not.
        assert_eq!(accel.call(&memory, 3, &[K, 3]).unwrap(), 0);
        assert_eq!(accel.call(&memory, 3, &[K, INDIV + 2]).unwrap(), KVAL42);
    }

    // OC__Cl is ofclass, region by region: strings and routines
    // match only their metaclasses, Class holds the classes and
    // the metaclasses, Object holds the plain objects, and real
    // membership walks the inheritance list.
    #[test]
    fn ofclass_walks_every_region() {
        let (memory, accel) = world();

        let answers = [
            ([STRINGISH, STR_MC], 1),
            ([STRINGISH, K], 0),
            ([0x48, ROUT_MC], 1),
            ([0x48, K], 0),
            ([35, K], 0),
            ([K, CLASS_MC], 1),
            ([CLASS_MC, CLASS_MC], 1),
            ([OBJ, CLASS_MC], 0),
            ([OBJ, OBJ_MC], 1),
            ([K, OBJ_MC], 0),
            ([OBJ, STR_MC], 0),
            ([OBJ, OBJ], 0),
            ([OBJ, K], 1),
            ([OBJ, K2], 0),
            ([K, K], 0),
        ];

        for (args, expected) in answers {
            assert_eq!(accel.call(&memory, 5, &args).unwrap(), expected, "{args:?}");
            assert_eq!(accel.call(&memory, 11, &args).unwrap(), expected);
        }
    }

    // RV__Pr reads a value, or the common default for a missing
    // common property; a missing individual property -- and
    // property zero -- read as zero.
    #[test]
    fn values_fall_back_to_defaults() {
        let (memory, accel) = world();

        assert_eq!(accel.call(&memory, 6, &[OBJ, 3]).unwrap(), 0x1111);
        assert_eq!(accel.call(&memory, 12, &[OBJ, 3]).unwrap(), 0x1111);
        assert_eq!(accel.call(&memory, 6, &[OBJ, 4]).unwrap(), 0xD4D4);
        assert_eq!(accel.call(&memory, 6, &[OBJ, INDIV + 5]).unwrap(), 0);
        assert_eq!(accel.call(&memory, 6, &[OBJ, 0]).unwrap(), 0);
    }

    // OP__Pr is provides: strings offer print and print_to_array,
    // routines offer call, classes offer the individual range, and
    // an object offers what its table holds.
    #[test]
    fn provides_answers_by_region() {
        let (memory, accel) = world();

        let answers = [
            ([STRINGISH, INDIV + 6], 1),
            ([STRINGISH, INDIV + 7], 1),
            ([STRINGISH, INDIV + 5], 0),
            ([0x48, INDIV + 5], 1),
            ([0x48, INDIV + 6], 0),
            ([35, 3], 0),
            ([K, INDIV + 4], 1),
            ([OBJ, 3], 1),
            ([OBJ, 4], 0),
            ([OBJ, INDIV + 4], 0),
        ];

        for (args, expected) in answers {
            assert_eq!(accel.call(&memory, 7, &args).unwrap(), expected, "{args:?}");
            assert_eq!(accel.call(&memory, 13, &args).unwrap(), expected);
        }
    }

    // A composite property id names a class by table index in its
    // low half and the property in its high half -- resolved only
    // when the object really is of that class.
    #[test]
    fn composite_ids_reach_the_class() {
        let (memory, accel) = world();
        let composite = (3 << 16) | 1;

        assert_eq!(accel.call(&memory, 3, &[OBJ, composite]).unwrap(), KVAL3);
        assert_eq!(accel.call(&memory, 3, &[STRINGISH, composite]).unwrap(), 0);
    }

    // The table's own bookkeeping: what is available, what
    // installing and cancelling do, and how unknown numbers are
    // shrugged off.
    #[test]
    fn the_accelerator_bookkeeping() {
        let (memory, mut accel) = world();

        assert_eq!(AVAILABLE, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
        assert_eq!(accel.lookup(FUNC), None);

        accel.set_func(99, FUNC);

        assert_eq!(accel.lookup(FUNC), None);

        accel.set_func(1, FUNC);

        let replacement = accel.lookup(FUNC).expect("the replacement is installed");

        assert_eq!(accel.call(&memory, replacement, &[OBJ]).unwrap(), 1);

        accel.set_func(0, FUNC);

        assert_eq!(accel.lookup(FUNC), None);

        // Unknown parameter numbers are shrugged off; a real one
        // lands. (The reference masks the value to 32 bits, which
        // u32 operands make inherent here.)
        accel.set_param(99, 5);
        accel.set_param(6, 5);

        assert_eq!(accel.params[6], 5);
    }
}
