//! The built-in search opcodes (Glulx: Searching).
//!
//! All three look through fixed-size structures in memory for one
//! whose key matches. They exist for speed: Inform's property and
//! dictionary lookups dominate its running time, and the spec
//! notes Advent runs 15-20% faster with binary-search property
//! lookup than with the equivalent Inform code.
//!
//! Keys are compared as byte strings. The reference glulxe carries
//! two comparison paths -- short keys copied into a stack buffer,
//! long keys re-read from memory on every comparison, because
//! buffering would mean allocating. Neither the Python reference
//! nor this port has that constraint, so the key is fetched once
//! as bytes and every comparison is a single slice compare. The
//! equivalence holds because a search never writes memory, and
//! slice ordering is lexicographic over unsigned bytes -- exactly
//! the big-endian unsigned ordering the sorted form requires.

use crate::errors::VoxamError;
use crate::glulx::memory::Memory;

/// The failure answers: the index form fails with -1, the address
/// form with 0 (Glulx: Searching).
pub const NOT_FOUND_INDEX: u32 = 0xFFFF_FFFF;
pub const NOT_FOUND_ADDRESS: u32 = 0;

/// The Options flags. Not every flag applies to every search:
/// RETURN_INDEX means nothing to linkedsearch, and
/// ZERO_KEY_TERMINATES nothing to binarysearch.
pub const KEY_INDIRECT: u32 = 0x01;
pub const ZERO_KEY_TERMINATES: u32 = 0x02;
pub const RETURN_INDEX: u32 = 0x04;

/// The key operand as the bytes every entry compares against.
///
/// With KeyIndirect the operand is the key's address and any size
/// is legal; without it the operand is the key itself, sitting in
/// the low bytes big-endian, and must fit a word (Glulx:
/// Searching). Refused for a direct key of a size no word can
/// hold.
fn fetch_key(memory: &Memory, key: u32, keysize: u32, options: u32) -> Result<Vec<u8>, VoxamError> {
    if options & KEY_INDIRECT != 0 {
        return memory.read_run(key, keysize);
    }

    if !matches!(keysize, 1 | 2 | 4) {
        return Err(VoxamError::GlulxInstruction(format!(
            "a direct search key must hold one, two, or four bytes, not {keysize} \
             (Glulx: Searching)"
        )));
    }

    Ok(key.to_be_bytes()[4 - keysize as usize..].to_vec())
}

/// Search an array of structures in order (Glulx: Searching).
///
/// A count of 0xFFFFFFFF means no upper limit: the search then
/// runs until it matches or, with ZeroKeyTerminates, until it
/// meets an all-zero key.
#[allow(clippy::too_many_arguments)] // the opcode's own seven operands
pub fn linear_search(
    memory: &Memory,
    key: u32,
    keysize: u32,
    start: u32,
    structsize: u32,
    numstructs: u32,
    keyoffset: u32,
    options: u32,
) -> Result<u32, VoxamError> {
    let keybuf = fetch_key(memory, key, keysize, options)?;
    let return_index = options & RETURN_INDEX != 0;
    let zero_terminates = options & ZERO_KEY_TERMINATES != 0;

    let mut address = start;

    // The unlimited 0xFFFFFFFF needs no special case: the count
    // walks the whole range, and the match or the terminator is
    // what actually ends the search.
    for count in 0..numstructs {
        let entry = memory.read_run(address.wrapping_add(keyoffset), keysize)?;

        if entry == keybuf {
            return Ok(if return_index { count } else { address });
        }

        // Checked after the match, so a search *for* the all-zero
        // key still finds it rather than stopping short.
        if zero_terminates && entry.iter().all(|byte| *byte == 0) {
            break;
        }

        address = address.wrapping_add(structsize);
    }

    Ok(if return_index {
        NOT_FOUND_INDEX
    } else {
        NOT_FOUND_ADDRESS
    })
}

/// Search a key-ordered array of structures (Glulx: Searching).
///
/// The structures must sit in ascending key order with no
/// duplicates, and the count must be exact -- the unlimited
/// 0xFFFFFFFF is not legal here, and ZeroKeyTerminates does not
/// apply.
#[allow(clippy::too_many_arguments)] // the opcode's own seven operands
pub fn binary_search(
    memory: &Memory,
    key: u32,
    keysize: u32,
    start: u32,
    structsize: u32,
    numstructs: u32,
    keyoffset: u32,
    options: u32,
) -> Result<u32, VoxamError> {
    let keybuf = fetch_key(memory, key, keysize, options)?;
    let return_index = options & RETURN_INDEX != 0;

    let (mut low, mut high) = (0u64, u64::from(numstructs));

    while low < high {
        let middle = (low + high) / 2;
        let address = (u64::from(start) + middle * u64::from(structsize)) as u32;
        let entry = memory.read_run(address.wrapping_add(keyoffset), keysize)?;

        if entry == keybuf {
            return Ok(if return_index { middle as u32 } else { address });
        }

        if entry[..] < keybuf[..] {
            low = middle + 1;
        } else {
            high = middle;
        }
    }

    Ok(if return_index {
        NOT_FOUND_INDEX
    } else {
        NOT_FOUND_ADDRESS
    })
}

/// Follow a linked list of structures (Glulx: Searching).
///
/// A zero in the link field ends the list. ReturnIndex does not
/// apply -- a list has no indexes -- so the answer is an address
/// or 0.
pub fn linked_search(
    memory: &Memory,
    key: u32,
    keysize: u32,
    start: u32,
    keyoffset: u32,
    nextoffset: u32,
    options: u32,
) -> Result<u32, VoxamError> {
    let keybuf = fetch_key(memory, key, keysize, options)?;
    let zero_terminates = options & ZERO_KEY_TERMINATES != 0;

    let mut address = start;

    while address != 0 {
        let entry = memory.read_run(address.wrapping_add(keyoffset), keysize)?;

        if entry == keybuf {
            return Ok(address);
        }

        if zero_terminates && entry.iter().all(|byte| *byte == 0) {
            break;
        }

        address = memory.read_word(address.wrapping_add(nextoffset))?;
    }

    Ok(NOT_FOUND_ADDRESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glulx::story::Story;

    const TABLE: u32 = 0x2C0;
    const MISS_INDEX: u32 = 0xFFFF_FFFF;

    /// A memory whose RAM holds the tests' tables, standing in for
    /// the reference suite's booted machine.
    fn booted() -> Memory {
        let mut data = vec![0u8; 256];
        data[..4].copy_from_slice(b"Glul");
        data[4..8].copy_from_slice(&0x0003_0102u32.to_be_bytes());
        data[8..12].copy_from_slice(&256u32.to_be_bytes());
        data[12..16].copy_from_slice(&256u32.to_be_bytes());
        data[16..20].copy_from_slice(&0x300u32.to_be_bytes());
        data[20..24].copy_from_slice(&256u32.to_be_bytes());

        Memory::new(&Story::new(data).unwrap())
    }

    // A direct key sits in the operand's low bytes, big-endian,
    // and must fit a word; an indirect key is read from memory and
    // may be any size at all.
    #[test]
    fn keys_arrive_direct_or_indirect() {
        let mut memory = booted();

        // Direct: the upper bytes of the operand are ignored.
        memory.write_run(TABLE, &[0x78, 0x00]).unwrap();

        let found = linear_search(&memory, 0x1234_5678, 1, TABLE, 1, 2, 0, 0).unwrap();

        assert_eq!(found, TABLE);

        // Indirect: a three-byte key is legal, because the key is
        // an address, not a value.
        memory.write_run(TABLE, &[0xAA, 0xBB, 0xCC]).unwrap();
        memory.write_run(TABLE + 8, &[0xAA, 0xBB, 0xCC]).unwrap();

        let found = linear_search(&memory, TABLE + 8, 3, TABLE, 3, 1, 0, KEY_INDIRECT).unwrap();

        assert_eq!(found, TABLE);

        let error = linear_search(&memory, 0, 3, TABLE, 3, 1, 0, 0).unwrap_err();
        assert!(error.to_string().contains("one, two, or four"));
    }

    // The linear search walks structures in order: a hit answers
    // the address or the index, a miss answers 0 or -1, and the
    // key may sit anywhere inside the structure.
    #[test]
    fn linear_search_walks_in_order() {
        let mut memory = booted();

        // Four structs of four bytes; the two-byte key sits at
        // offset 2.
        for (index, key) in [0x1111u16, 0x2222, 0x3333, 0x4444].iter().enumerate() {
            let mut entry = vec![0, 0];
            entry.extend_from_slice(&key.to_be_bytes());
            memory.write_run(TABLE + 4 * index as u32, &entry).unwrap();
        }

        let hit = linear_search(&memory, 0x3333, 2, TABLE, 4, 4, 2, 0).unwrap();

        assert_eq!(hit, TABLE + 8);
        assert_eq!(
            linear_search(&memory, 0x3333, 2, TABLE, 4, 4, 2, RETURN_INDEX).unwrap(),
            2
        );

        assert_eq!(
            linear_search(&memory, 0x9999, 2, TABLE, 4, 4, 2, 0).unwrap(),
            0
        );
        assert_eq!(
            linear_search(&memory, 0x9999, 2, TABLE, 4, 4, 2, RETURN_INDEX).unwrap(),
            MISS_INDEX
        );
    }

    // A zero key ends a terminated search -- but only after the
    // match check, so a search *for* the zero key still finds it.
    // With the unlimited count, the zero terminator is what makes
    // the search finite.
    #[test]
    fn zero_keys_terminate_after_matching() {
        let mut memory = booted();

        for (index, key) in [0x11u8, 0x00, 0x33].iter().enumerate() {
            memory
                .write_run(TABLE + 2 * index as u32, &[*key, 0])
                .unwrap();
        }

        // 0x33 sits beyond the zero key, so a terminated search
        // misses it; an unterminated one still gets there.
        assert_eq!(
            linear_search(&memory, 0x33, 1, TABLE, 2, 3, 0, ZERO_KEY_TERMINATES).unwrap(),
            0
        );
        assert_eq!(
            linear_search(&memory, 0x33, 1, TABLE, 2, 3, 0, 0).unwrap(),
            TABLE + 4
        );

        // The zero key itself is findable.
        assert_eq!(
            linear_search(&memory, 0x00, 1, TABLE, 2, 3, 0, ZERO_KEY_TERMINATES).unwrap(),
            TABLE + 2
        );

        // 0xFFFFFFFF structures means "no limit": the terminator
        // is the only end the search has.
        assert_eq!(
            linear_search(
                &memory,
                0x77,
                1,
                TABLE,
                2,
                0xFFFF_FFFF,
                0,
                ZERO_KEY_TERMINATES
            )
            .unwrap(),
            0
        );
    }

    // The binary search halves a sorted array: hits at the ends
    // drive both halvings, and a miss between keys answers the
    // failure value for either form.
    #[test]
    fn binary_search_halves_a_sorted_array() {
        let mut memory = booted();
        let keys = [0x10u8, 0x20, 0x30, 0x40, 0x50];

        for (index, key) in keys.iter().enumerate() {
            memory
                .write_run(TABLE + 2 * index as u32, &[*key, 0])
                .unwrap();
        }

        for (index, key) in keys.iter().enumerate() {
            assert_eq!(
                binary_search(&memory, u32::from(*key), 1, TABLE, 2, 5, 0, 0).unwrap(),
                TABLE + 2 * index as u32
            );
        }

        assert_eq!(
            binary_search(&memory, 0x50, 1, TABLE, 2, 5, 0, RETURN_INDEX).unwrap(),
            4
        );
        assert_eq!(
            binary_search(&memory, 0x25, 1, TABLE, 2, 5, 0, 0).unwrap(),
            0
        );
        assert_eq!(
            binary_search(&memory, 0x25, 1, TABLE, 2, 5, 0, RETURN_INDEX).unwrap(),
            MISS_INDEX
        );
    }

    // The linked search follows next pointers wherever they lead:
    // a zero link ends the list, and the zero-key terminator cuts
    // it short the same way it cuts the linear walk.
    #[test]
    fn linked_search_follows_the_chain() {
        let mut memory = booted();

        // Three nodes, deliberately out of address order: key byte
        // at +0, next pointer at +4.
        let chain = [
            (TABLE, 0x11u8, TABLE + 0x20),
            (TABLE + 0x20, 0x22, TABLE + 0x10),
            (TABLE + 0x10, 0x33, 0),
        ];

        for (address, key, link) in chain {
            memory.write_byte(address, key).unwrap();
            memory.write_word(address + 4, link).unwrap();
        }

        assert_eq!(
            linked_search(&memory, 0x33, 1, TABLE, 0, 4, 0).unwrap(),
            TABLE + 0x10
        );
        assert_eq!(linked_search(&memory, 0x99, 1, TABLE, 0, 4, 0).unwrap(), 0);

        // A zero key in the middle node ends a terminated walk
        // before the tail -- and is itself findable.
        memory.write_byte(TABLE + 0x20, 0).unwrap();

        assert_eq!(
            linked_search(&memory, 0x33, 1, TABLE, 0, 4, ZERO_KEY_TERMINATES).unwrap(),
            0
        );
        assert_eq!(
            linked_search(&memory, 0x00, 1, TABLE, 0, 4, ZERO_KEY_TERMINATES).unwrap(),
            TABLE + 0x20
        );
    }
}
