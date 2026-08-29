//! Saving and restoring the machine state.
//!
//! The format is Quetzal with Glulx's own chunks (Glulx: The
//! Save-Game Format): IFhd identifies the story as the first 128
//! bytes of memory, CMem holds dynamic memory XOR-compressed
//! against the game file, MAll the allocation heap, and Stks the
//! stack whole.
//!
//! What is *not* saved matters as much (Glulx: State Not Saved):
//! Glk state, the protected range, the random number generator,
//! the I/O system, and the string-decoding table address all
//! survive a restore untouched.
//!
//! The stack chunk is a straight copy. The spec requires stack
//! values written big-endian, and the reference glulxe has to walk
//! each frame's locals format to byte-swap them -- but Voxam's
//! stack chose big-endian storage in its own era for exactly this
//! moment, so saving is a snapshot and restoring is the reverse,
//! the locals format never consulted.
//!
//! The stream-facing save and restore arrive with the Glk era; the
//! machine speaks their failure directly until then.

use crate::errors::VoxamError;
use crate::glulx::machine::Machine;
use crate::iff::{IffChunk, parse_form, write_form};

/// The Quetzal FORM type, and Glulx's chunks within it (Glulx: The
/// Save-Game Format).
const SAVE_FORM: &[u8; 4] = b"IFZS";
const IDENTITY: &[u8; 4] = b"IFhd";
const COMPRESSED: &[u8; 4] = b"CMem";
const UNCOMPRESSED: &[u8; 4] = b"UMem";
const HEAP: &[u8; 4] = b"MAll";
const STACK: &[u8; 4] = b"Stks";

/// IFhd is the first 128 bytes of memory -- always in ROM, since
/// RAMSTART is at least 256 (Glulx: Associated Story File).
const IDENTITY_LENGTH: u32 = 128;

/// How many undo states to keep; the reference glulxe keeps the
/// same number.
const MAX_UNDO_LEVELS: usize = 8;

/// The opcodes' spoken results: zero for success, one for failure
/// (Glulx: Game State).
pub const SUCCEEDED: u32 = 0;
pub const FAILED: u32 = 1;

const LENGTH_SIZE: usize = 4;
const LONGEST_RUN: usize = 0x100;

fn save_error(message: String) -> VoxamError {
    VoxamError::GlulxSave(message)
}

/// RAM as a CMem body: XOR'd against the original, then packed.
///
/// A run of zeroes is written as a zero byte followed by the run
/// length minus one, so one byte encodes up to 256; a trailing run
/// is dropped entirely, because the decoder treats anything past
/// the chunk's end as unchanged (Glulx: Contents of Dynamic
/// Memory).
fn encode_memory(machine: &Machine) -> Result<Vec<u8>, VoxamError> {
    let memory = &machine.memory;
    let length = memory.endmem() - memory.ramstart();
    let mut out = memory.endmem().to_be_bytes().to_vec();

    let current = memory.read_run(memory.ramstart(), length)?;
    let original = memory.original_run(memory.ramstart(), length);
    let difference: Vec<u8> = current
        .iter()
        .zip(&original)
        .map(|(now, then)| now ^ then)
        .collect();

    let mut cursor = 0;

    while cursor < difference.len() {
        if difference[cursor] != 0 {
            out.push(difference[cursor]);
            cursor += 1;

            continue;
        }

        let run_start = cursor;

        while cursor < difference.len() && difference[cursor] == 0 {
            cursor += 1;
        }

        if cursor == difference.len() {
            // The trailing run is dropped entirely.
            break;
        }

        let mut remaining = cursor - run_start;

        while remaining > 0 {
            let step = remaining.min(LONGEST_RUN);

            out.push(0);
            out.push((step - 1) as u8);

            remaining -= step;
        }
    }

    Ok(out)
}

/// Undo the CMem encoding into the live memory map.
fn decode_memory(machine: &mut Machine, body: &[u8]) -> Result<(), VoxamError> {
    let new_size = memory_size(body)?;

    machine.memory.set_size(new_size)?;

    let length = (new_size - machine.memory.ramstart()) as usize;

    // Expand the run-length encoding. This loop runs over the
    // *compressed* data, which is mostly runs, not over every byte
    // of RAM.
    let mut difference: Vec<u8> = Vec::new();
    let mut cursor = LENGTH_SIZE;

    while cursor < body.len() && difference.len() < length {
        let byte = body[cursor];
        cursor += 1;

        if byte == 0 {
            if cursor >= body.len() {
                return Err(save_error(
                    "a zero byte ends the memory chunk with no run length".into(),
                ));
            }

            difference.extend(std::iter::repeat_n(0u8, usize::from(body[cursor]) + 1));
            cursor += 1;
        } else {
            difference.push(byte);
        }
    }

    difference.truncate(length);
    difference.resize(length, 0);

    let original = machine
        .memory
        .original_run(machine.memory.ramstart(), length as u32);
    let contents: Vec<u8> = difference
        .iter()
        .zip(&original)
        .map(|(diff, then)| diff ^ then)
        .collect();

    machine.memory.overwrite_ram(&contents);

    Ok(())
}

/// A UMem chunk: the new size, then raw RAM.
fn decode_uncompressed(machine: &mut Machine, body: &[u8]) -> Result<(), VoxamError> {
    let new_size = memory_size(body)?;

    machine.memory.set_size(new_size)?;

    let length = (new_size - machine.memory.ramstart()) as usize;
    let end = (LENGTH_SIZE + length).min(body.len());

    machine.memory.overwrite_ram(&body[LENGTH_SIZE..end]);

    Ok(())
}

/// The four-byte size a memory chunk opens with.
fn memory_size(body: &[u8]) -> Result<u32, VoxamError> {
    if body.len() < LENGTH_SIZE {
        return Err(save_error(
            "the save file's memory chunk cannot hold its own size".into(),
        ));
    }

    Ok(u32::from_be_bytes([body[0], body[1], body[2], body[3]]))
}

/// A complete save file for the current state.
///
/// The caller must already have pushed the four-value call stub
/// the spec requires, since it forms part of the stack chunk
/// (Glulx: Contents of the Stack). An MAll chunk is written only
/// while the heap is active; an inactive heap's chunk may be
/// omitted (Glulx: Memory Allocation Heap).
pub fn serialize(machine: &Machine) -> Result<Vec<u8>, VoxamError> {
    let piece = |chunk_id: &[u8; 4], payload: Vec<u8>| IffChunk {
        chunk_id: *chunk_id,
        payload,
        offset: 0,
    };

    let mut pieces = vec![
        piece(IDENTITY, machine.memory.read_run(0, IDENTITY_LENGTH)?),
        piece(COMPRESSED, encode_memory(machine)?),
    ];

    let summary = machine.heap.summary();

    if !summary.is_empty() {
        pieces.push(piece(
            HEAP,
            summary.iter().flat_map(|word| word.to_be_bytes()).collect(),
        ));
    }

    pieces.push(piece(STACK, machine.stack.snapshot()));

    Ok(write_form(SAVE_FORM, &pieces))
}

/// Restore a state a save file holds.
///
/// Order matters: the live heap is dropped first -- it does not
/// survive into the restored state, and its shrink must land
/// before the memory chunk sets the size -- then memory, then the
/// heap summary above it, then the stack.
pub fn deserialize(machine: &mut Machine, data: &[u8]) -> Result<(), VoxamError> {
    let (form_type, pieces) = parse_form(data)
        .map_err(|error| save_error(format!("the save file is not an IFF container: {error}")))?;

    if &form_type != SAVE_FORM {
        return Err(save_error(format!(
            "the save file is a b'{}' FORM, not Quetzal's IFZS",
            form_type.escape_ascii()
        )));
    }

    let payload = |id: &[u8; 4]| {
        pieces
            .iter()
            .find(|piece| &piece.chunk_id == id)
            .map(|piece| &piece.payload)
    };

    let Some(identity) = payload(IDENTITY) else {
        return Err(save_error(
            "the save file has no IFhd chunk to name its story".into(),
        ));
    };

    if *identity != machine.memory.read_run(0, IDENTITY_LENGTH)? {
        return Err(save_error(
            "the save file belongs to a different story".into(),
        ));
    }

    machine.heap.clear(&mut machine.memory)?;

    if let Some(body) = payload(COMPRESSED) {
        let body = body.clone();

        decode_memory(machine, &body)?;
    } else if let Some(body) = payload(UNCOMPRESSED) {
        let body = body.clone();

        decode_uncompressed(machine, &body)?;
    } else {
        return Err(save_error("the save file has no memory chunk".into()));
    }

    let heap_words: Vec<u32> = payload(HEAP)
        .map(|body| {
            body.as_chunks::<4>()
                .0
                .iter()
                .map(|word| u32::from_be_bytes(*word))
                .collect()
        })
        .unwrap_or_default();

    machine.heap.apply_summary(&machine.memory, &heap_words)?;

    let Some(stack) = payload(STACK) else {
        return Err(save_error("the save file has no Stks chunk".into()));
    };

    let stack = stack.clone();

    machine.stack.restore(&stack)
}

/// The save opcode's work: the state onto a Glk stream.
///
/// A stream that is missing or unwritable fails with 1 rather
/// than faulting -- the spoken failure is how a game learns to
/// prompt again (Glulx: Game State).
pub fn save(machine: &mut Machine, stream: Option<u32>) -> Result<u32, VoxamError> {
    let writable = match (&machine.bridge, stream) {
        (Some(bridge), Some(key)) => bridge
            .library
            .streams
            .get(&key)
            .is_some_and(|held| held.writable),
        _ => false,
    };

    if !writable {
        return Ok(FAILED);
    }

    let data = serialize(machine)?;
    let bridge = machine.bridge.as_mut().expect("checked above");
    let held = bridge
        .library
        .streams
        .get_mut(&stream.expect("checked above"))
        .expect("checked above");

    for byte in data {
        held.put_char(&mut machine.memory, u32::from(byte))?;
    }

    Ok(SUCCEEDED)
}

/// The restore opcode's work: the state off a Glk stream.
///
/// On success the whole machine state -- stack included -- has
/// been replaced, and the caller pops the call stub that was
/// saved with it. Failure speaks 1 and changes nothing.
pub fn restore(machine: &mut Machine, stream: Option<u32>) -> Result<u32, VoxamError> {
    let readable = match (&machine.bridge, stream) {
        (Some(bridge), Some(key)) => bridge
            .library
            .streams
            .get(&key)
            .is_some_and(|held| held.readable),
        _ => false,
    };

    if !readable {
        return Ok(FAILED);
    }

    let mut data = Vec::new();

    {
        let bridge = machine.bridge.as_mut().expect("checked above");
        let held = bridge
            .library
            .streams
            .get_mut(&stream.expect("checked above"))
            .expect("checked above");

        loop {
            let value = held.get_char(&machine.memory)?;

            if value < 0 {
                break;
            }

            data.push(value as u8);
        }
    }

    match deserialize(machine, &data) {
        Ok(()) => Ok(SUCCEEDED),
        Err(VoxamError::GlulxSave(_)) => Ok(FAILED),
        Err(error) => Err(error),
    }
}

/// The saveundo opcode's work: the state into the undo chain.
///
/// The chain keeps the newest handful of states; saving past the
/// limit lets the oldest go, the way the reference does.
pub fn save_undo(machine: &mut Machine) -> Result<u32, VoxamError> {
    let state = serialize(machine)?;

    machine.undo_chain.push(state);

    let excess = machine.undo_chain.len().saturating_sub(MAX_UNDO_LEVELS);

    machine.undo_chain.drain(..excess);

    Ok(SUCCEEDED)
}

/// The restoreundo opcode's work: the newest undo state back.
///
/// An empty chain fails with 1; a successful restore consumes the
/// state it restored.
pub fn restore_undo(machine: &mut Machine) -> Result<u32, VoxamError> {
    let Some(state) = machine.undo_chain.pop() else {
        return Ok(FAILED);
    };

    deserialize(machine, &state)?;

    Ok(SUCCEEDED)
}

/// The hasundo opcode's answer: 0 with a state waiting, 1 bare.
///
/// A zero here is a promise that restoreundo will succeed (Glulx:
/// Game State).
pub fn has_undo(machine: &Machine) -> u32 {
    if machine.undo_chain.is_empty() {
        FAILED
    } else {
        SUCCEEDED
    }
}

/// The discardundo opcode's work: let the newest state go.
pub fn discard_undo(machine: &mut Machine) {
    machine.undo_chain.pop();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glulx::stack::dest_type;
    use crate::glulx::story::Story;
    use crate::glulx::testing::image;

    const IDLE: &[u8] = &[0xC0, 0x00, 0x00, 0x81, 0x20];
    const PLANT: u32 = 0x180;
    const RESULT: u32 = 0x140;
    const SECOND: u32 = 0x148;
    const MARKER: u32 = 0x160;

    const RESTORED: u32 = 0xFFFF_FFFF;

    fn booted() -> Machine {
        Machine::new(Story::new(image(IDLE)).unwrap(), None).unwrap()
    }

    fn identity(machine: &Machine) -> Vec<u8> {
        machine.memory.read_run(0, IDENTITY_LENGTH).unwrap()
    }

    fn piece(chunk_id: &[u8; 4], payload: Vec<u8>) -> IffChunk {
        IffChunk {
            chunk_id: *chunk_id,
            payload,
            offset: 0,
        }
    }

    // The whole state comes back: changed RAM, memory grown past
    // the boot size -- zero-extended above EXTSTART in the file's
    // stead -- and the stack, stub and all, byte for byte.
    #[test]
    fn the_state_survives_the_bottle() {
        let mut machine = booted();

        machine.memory.write_byte(0x150, 0x42).unwrap();
        machine.memory.set_size(0x400).unwrap();
        machine.memory.write_byte(0x350, 0x77).unwrap();
        machine.stack.push(123).unwrap();
        machine
            .stack
            .push_stub(dest_type::MEMORY, RESULT, 0x1234)
            .unwrap();

        let saved = serialize(&machine).unwrap();

        machine.memory.write_byte(0x150, 0).unwrap();
        machine.memory.set_size(0x300).unwrap();
        machine.stack.pop_stub().unwrap();
        machine.stack.pop().unwrap();

        deserialize(&mut machine, &saved).unwrap();

        assert_eq!(machine.memory.endmem(), 0x400);
        assert_eq!(machine.memory.read_byte(0x150).unwrap(), 0x42);
        assert_eq!(machine.memory.read_byte(0x350).unwrap(), 0x77);

        let stub = machine.stack.pop_stub().unwrap();

        assert_eq!(
            (stub.desttype, stub.destaddr, stub.pc),
            (dest_type::MEMORY, RESULT, 0x1234)
        );
        assert_eq!(machine.stack.pop().unwrap(), 123);
    }

    // The compression earns its name: an unchanged machine's
    // memory chunk is nothing but its four size bytes, and a long
    // gap between two changes packs into 256-zero runs rather than
    // a byte apiece.
    #[test]
    fn the_memory_chunk_compresses() {
        let mut machine = booted();

        machine.stack.push_stub(dest_type::DISCARD, 0, 0).unwrap();

        let (_, pieces) = parse_form(&serialize(&machine).unwrap()).unwrap();
        let body = pieces
            .iter()
            .find(|chunk| &chunk.chunk_id == COMPRESSED)
            .unwrap();

        assert_eq!(body.payload.len(), 4);

        machine.memory.write_byte(0x100, 0x11).unwrap();
        machine.memory.write_byte(0x2B0, 0x22).unwrap();

        let saved = serialize(&machine).unwrap();

        assert!(saved.len() < 0x150);

        machine.memory.write_byte(0x100, 0).unwrap();
        machine.memory.write_byte(0x2B0, 0).unwrap();

        deserialize(&mut machine, &saved).unwrap();

        assert_eq!(machine.memory.read_byte(0x100).unwrap(), 0x11);
        assert_eq!(machine.memory.read_byte(0x2B0).unwrap(), 0x22);
    }

    // An uncompressed UMem chunk restores the same way the packed
    // form does; a memory body holding more bytes than RAM is
    // trimmed to fit.
    #[test]
    fn the_uncompressed_form_restores() {
        let mut machine = booted();

        machine.memory.write_byte(MARKER, 0x5A).unwrap();
        machine.stack.push_stub(dest_type::DISCARD, 0, 0).unwrap();

        let raw = machine.memory.read_run(0x100, 0x200).unwrap();

        let mut body = 0x300u32.to_be_bytes().to_vec();
        body.extend_from_slice(&raw);

        let file = write_form(
            SAVE_FORM,
            &[
                piece(IDENTITY, identity(&machine)),
                piece(UNCOMPRESSED, body),
                piece(STACK, machine.stack.snapshot()),
            ],
        );

        machine.memory.write_byte(MARKER, 0).unwrap();

        deserialize(&mut machine, &file).unwrap();

        assert_eq!(machine.memory.read_byte(MARKER).unwrap(), 0x5A);

        // A compressed body longer than RAM: the surplus is
        // trimmed.
        let mut body = 0x300u32.to_be_bytes().to_vec();
        body.extend(std::iter::repeat_n(0x41u8, 0x250));

        let stuffed = write_form(
            SAVE_FORM,
            &[
                piece(IDENTITY, identity(&machine)),
                piece(COMPRESSED, body),
                piece(STACK, machine.stack.snapshot()),
            ],
        );

        deserialize(&mut machine, &stuffed).unwrap();

        assert_eq!(machine.memory.read_byte(0x2FF).unwrap(), 0x41);
    }

    // The protected range is silently unaffected by a restore, at
    // every position it can sit: inside RAM, flush against its
    // start, flush against its end, and entirely outside it.
    #[test]
    fn protection_survives_a_restore() {
        for (start, length, kept) in [
            (0x180u32, 0x10u32, 0x185u32),
            (0x100, 0x10, 0x105),
            (0x2F0, 0x10, 0x2F5),
        ] {
            let mut machine = booted();

            machine.stack.push_stub(dest_type::DISCARD, 0, 0).unwrap();

            let saved = serialize(&machine).unwrap();

            machine.memory.set_protection(start, length);
            machine.memory.write_byte(kept, 0x55).unwrap();
            machine.memory.write_byte(0x1C0, 0x66).unwrap();

            deserialize(&mut machine, &saved).unwrap();

            assert_eq!(machine.memory.read_byte(kept).unwrap(), 0x55);
            assert_eq!(machine.memory.read_byte(0x1C0).unwrap(), 0);
        }

        // A range beyond the map protects nothing, and the write
        // goes through whole.
        let mut outside = booted();

        outside.stack.push_stub(dest_type::DISCARD, 0, 0).unwrap();

        let saved = serialize(&outside).unwrap();

        outside.memory.set_protection(0x500, 4);
        outside.memory.write_byte(0x1C0, 0x66).unwrap();

        deserialize(&mut outside, &saved).unwrap();

        assert_eq!(outside.memory.read_byte(0x1C0).unwrap(), 0);
    }

    // Every way a save file can be wrong is refused by name: not
    // IFF, the wrong FORM, no story identity, someone else's
    // story, no memory, no stack, a memory chunk cut short, and a
    // zero byte with no run length behind it.
    #[test]
    fn wrong_save_files_are_refused() {
        let mut machine = booted();

        machine.stack.push_stub(dest_type::DISCARD, 0, 0).unwrap();

        let whole = identity(&machine);
        let memory = piece(COMPRESSED, 0x300u32.to_be_bytes().to_vec());
        let stack = piece(STACK, machine.stack.snapshot());

        let mut short_body = 0x300u32.to_be_bytes().to_vec();
        short_body.extend_from_slice(&[0x41, 0x00]);

        let wrongs: Vec<(Vec<u8>, &str)> = vec![
            (b"junk".to_vec(), "not an IFF container"),
            (
                write_form(b"IFRS", std::slice::from_ref(&memory)),
                "not Quetzal's IFZS",
            ),
            (
                write_form(SAVE_FORM, &[memory.clone(), stack.clone()]),
                "no IFhd",
            ),
            (
                write_form(
                    SAVE_FORM,
                    &[piece(IDENTITY, vec![0; 128]), memory.clone(), stack.clone()],
                ),
                "different story",
            ),
            (
                write_form(SAVE_FORM, &[piece(IDENTITY, whole.clone()), stack.clone()]),
                "no memory chunk",
            ),
            (
                write_form(SAVE_FORM, &[piece(IDENTITY, whole.clone()), memory.clone()]),
                "no Stks chunk",
            ),
            (
                write_form(
                    SAVE_FORM,
                    &[
                        piece(IDENTITY, whole.clone()),
                        piece(COMPRESSED, vec![0x00]),
                        stack.clone(),
                    ],
                ),
                "cannot hold its own size",
            ),
            (
                write_form(
                    SAVE_FORM,
                    &[
                        piece(IDENTITY, whole.clone()),
                        piece(COMPRESSED, short_body),
                        stack.clone(),
                    ],
                ),
                "no run length",
            ),
        ];

        for (data, complaint) in wrongs {
            let error = deserialize(&mut machine, &data).unwrap_err();

            assert!(error.to_string().contains(complaint), "{complaint}");
        }
    }

    // The heap rides the save: an active heap writes its MAll
    // chunk and comes back rebuilt, blocks and gaps alike; an
    // inactive one writes no chunk at all, and restoring an
    // inactive save onto an active heap deactivates it.
    #[test]
    fn the_heap_rides_the_save() {
        let mut machine = booted();

        machine.stack.push_stub(dest_type::DISCARD, 0, 0).unwrap();

        let bare = serialize(&machine).unwrap();

        let (_, pieces) = parse_form(&bare).unwrap();

        assert!(!pieces.iter().any(|chunk| &chunk.chunk_id == HEAP));

        let first = machine.heap.alloc(&mut machine.memory, 0x40).unwrap();
        let second = machine.heap.alloc(&mut machine.memory, 0x30).unwrap();

        let saved = serialize(&machine).unwrap();

        let (_, pieces) = parse_form(&saved).unwrap();

        assert!(pieces.iter().any(|chunk| &chunk.chunk_id == HEAP));

        machine.heap.free(&mut machine.memory, first).unwrap();

        deserialize(&mut machine, &saved).unwrap();

        assert_eq!(
            machine.heap.summary(),
            [0x300, 2, first, 0x40, second, 0x30]
        );

        // An inactive-heap save lands on an active heap by
        // clearing it.
        deserialize(&mut machine, &bare).unwrap();

        assert!(!machine.heap.active());
        assert_eq!(machine.memory.endmem(), 0x300);
    }

    // The undo chain holds the newest handful of states and no
    // more; restoring consumes, discarding drops, and an empty
    // chain answers honestly.
    #[test]
    fn the_undo_chain_keeps_the_newest() {
        let mut machine = booted();

        machine.stack.push_stub(dest_type::DISCARD, 0, 0).unwrap();

        assert_eq!(has_undo(&machine), FAILED);
        assert_eq!(restore_undo(&mut machine).unwrap(), FAILED);

        discard_undo(&mut machine);

        for turn in 0..10u8 {
            machine.memory.write_byte(MARKER, turn).unwrap();

            save_undo(&mut machine).unwrap();
        }

        assert_eq!(machine.undo_chain.len(), MAX_UNDO_LEVELS);
        assert_eq!(has_undo(&machine), SUCCEEDED);

        discard_undo(&mut machine);

        assert_eq!(restore_undo(&mut machine).unwrap(), SUCCEEDED);
        assert_eq!(machine.memory.read_byte(MARKER).unwrap(), 8);
        assert_eq!(machine.undo_chain.len(), 6);
    }

    // The saveundo dance, through the opcodes: the first pass
    // stores zero and walks on; after a restoreundo elsewhere,
    // execution is back at the instruction after saveundo with -1
    // stored and the turn's changes gone.
    #[test]
    fn the_saveundo_dance() {
        let mut machine = booted();

        let mut saveundo = vec![0x81, 0x25, 0x07];
        saveundo.extend_from_slice(&RESULT.to_be_bytes());

        machine.memory.write_run(PLANT, &saveundo).unwrap();

        machine.pc = PLANT;

        machine.step().unwrap();

        let resumed = PLANT + saveundo.len() as u32;

        assert_eq!(machine.memory.read_word(RESULT).unwrap(), 0);
        assert_eq!(machine.pc, resumed);

        machine.memory.write_byte(MARKER, 0x99).unwrap();

        let mut restoreundo = vec![0x81, 0x26, 0x07];
        restoreundo.extend_from_slice(&SECOND.to_be_bytes());
        machine
            .memory
            .write_run(PLANT + 0x20, &restoreundo)
            .unwrap();

        machine.pc = PLANT + 0x20;

        machine.step().unwrap();

        assert_eq!(machine.pc, resumed);
        assert_eq!(machine.memory.read_word(RESULT).unwrap(), RESTORED);
        assert_eq!(machine.memory.read_byte(MARKER).unwrap(), 0);

        // The restore reverted the second plant along with
        // everything else -- it was written after the save -- so
        // it needs planting again. With the chain now spent,
        // restoreundo speaks failure in place and walks on.
        machine
            .memory
            .write_run(PLANT + 0x20, &restoreundo)
            .unwrap();

        machine.pc = PLANT + 0x20;

        machine.step().unwrap();

        assert_eq!(machine.memory.read_word(SECOND).unwrap(), 1);
    }

    // hasundo and discardundo through the opcodes: a state waits,
    // is let go, and waits no more.
    #[test]
    fn hasundo_and_discardundo_dispatch() {
        let mut machine = booted();

        let mut plant = vec![0x81, 0x25, 0x00];
        plant.extend_from_slice(&[0x81, 0x28, 0x07]);
        plant.extend_from_slice(&RESULT.to_be_bytes());
        plant.extend_from_slice(&[0x81, 0x29]);
        plant.extend_from_slice(&[0x81, 0x28, 0x07]);
        plant.extend_from_slice(&SECOND.to_be_bytes());
        plant.extend_from_slice(&[0x81, 0x20]);

        machine.memory.write_run(PLANT, &plant).unwrap();

        machine.pc = PLANT;

        machine.run(Some(10)).unwrap();

        assert_eq!(machine.memory.read_word(RESULT).unwrap(), 0);
        assert_eq!(machine.memory.read_word(SECOND).unwrap(), 1);
    }

    // With no Glk library at all, the save and restore opcodes
    // speak 1 rather than faulting -- the stream can never
    // resolve. (Their stream-riding successes wait for the Glk
    // era.)
    #[test]
    fn bare_saves_speak_one() {
        let mut machine = booted();

        let mut save_unknown = vec![0x81, 0x23, 0x71, 0x63];
        save_unknown.extend_from_slice(&RESULT.to_be_bytes());

        machine.memory.write_run(PLANT, &save_unknown).unwrap();

        machine.pc = PLANT;

        machine.step().unwrap();

        assert_eq!(machine.memory.read_word(RESULT).unwrap(), 1);

        let mut restore_unknown = vec![0x81, 0x24, 0x71, 0x63];
        restore_unknown.extend_from_slice(&SECOND.to_be_bytes());

        machine.memory.write_run(PLANT, &restore_unknown).unwrap();

        machine.pc = PLANT;

        machine.step().unwrap();

        assert_eq!(machine.memory.read_word(SECOND).unwrap(), 1);
    }

    // Undo states survive a restart: the chain is not part of the
    // reset, so a state saved before the restart still pours back
    // after it.
    #[test]
    fn undo_survives_restart() {
        let mut machine = booted();

        machine.memory.write_byte(MARKER, 0x42).unwrap();
        machine.stack.push_stub(dest_type::DISCARD, 0, 0).unwrap();

        save_undo(&mut machine).unwrap();

        machine.restart().unwrap();

        assert_eq!(machine.memory.read_byte(MARKER).unwrap(), 0);
        assert_eq!(restore_undo(&mut machine).unwrap(), SUCCEEDED);
        assert_eq!(machine.memory.read_byte(MARKER).unwrap(), 0x42);
    }
}
