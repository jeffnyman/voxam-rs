//! Quetzal, the common format for saved games (Quetzal 1.4).
//!
//! A Quetzal file is an IFF FORM of type IFZS (Quetzal §2.1)
//! carrying three required chunks (Quetzal §7.18): IFhd names the
//! story the save belongs to, CMem or UMem carries dynamic memory,
//! and Stks carries the call chain. That is exactly a Snapshot
//! with an identity stamp, so this module is a pure codec:
//! Snapshot in, bytes out, and back.
//!
//! Writing always compresses (CMem); reading accepts both forms,
//! as Quetzal §3.6 requires.

use crate::errors::VoxamError;
use crate::iff::{IffChunk, parse_form, write_form};
use crate::zmachine::snapshot::{FrameSnapshot, Snapshot};
use crate::zmachine::story::Story;

/// The FORM type of a saved game is IFZS (Quetzal §2.1).
const SAVE_FORM: &[u8; 4] = b"IFZS";

/// The chunks this codec understands; anything else is skipped
/// unread, as extension chunks must be (Quetzal §7.17, §8.6).
const IFHD_ID: [u8; 4] = *b"IFhd";
const CMEM_ID: [u8; 4] = *b"CMem";
const UMEM_ID: [u8; 4] = *b"UMem";
const STKS_ID: [u8; 4] = *b"Stks";

/// IFhd carries release, serial, checksum, and a 3-byte PC -- 13
/// bytes now, possibly more one day, but the first 13 are
/// guaranteed (Quetzal §5.4, §5.5). The first ten name the story
/// (Quetzal §5.3).
const IFHD_LENGTH: usize = 13;
const IDENTITY_SIZE: usize = 10;

/// Return PCs and the saved PC are 3-byte byte addresses (Quetzal
/// §4.3.1, §5.4.6).
const ADDRESS_LIMIT: usize = 0xFF_FFFF;

/// A frame's flags byte is 000pvvvv: p set on discard-result
/// calls, vvvv the local count (Quetzal §4.3.2, §4.6). The
/// arguments byte is 0gfedcba, one bit per supplied argument
/// (Quetzal §4.3.4, §4.7).
const DISCARD_FLAG: u8 = 0x10;
const LOCALS_MASK: u8 = 0x0F;
const FLAGS_RESERVED: u8 = 0xE0;
const ARGUMENTS_LIMIT: usize = 7;

/// The fixed part of a frame: 3 address bytes, flags, store
/// variable, arguments mask, and a word counting the evaluation
/// stack (Quetzal §4.3).
const FRAME_HEADER_SIZE: usize = 8;
const WORD_SIZE: usize = 2;

/// A zero byte in CMem data pairs with a length byte for a run of
/// n+1 zeros (Quetzal §3.2), so one pair carries at most 256.
const LONGEST_RUN: usize = 256;

fn quetzal_error(message: String) -> VoxamError {
    VoxamError::ZMachineQuetzal(message)
}

/// Serialize a state of play as a Quetzal file (Quetzal §2): a
/// complete IFZS FORM, IFhd then CMem then Stks. The story's
/// identity is stamped into IFhd and its original bytes are the
/// reference CMem compresses against (Quetzal §3.2, §5.3).
pub fn write(snapshot: &Snapshot, story: &Story) -> Result<Vec<u8>, VoxamError> {
    let dynamic = &snapshot.dynamic_memory;
    let base = usize::from(story.header().static_memory_base());

    if dynamic.len() != base {
        return Err(quetzal_error(format!(
            "cannot save a {}-byte dynamic memory image for a story whose dynamic \
             memory is {base} bytes: the snapshot belongs to a different game \
             (Quetzal §5.3)",
            dynamic.len()
        )));
    }

    Ok(write_form(
        SAVE_FORM,
        &[
            IffChunk {
                chunk_id: IFHD_ID,
                payload: encode_identity(snapshot.pc, story)?,
                offset: 0,
            },
            IffChunk {
                chunk_id: CMEM_ID,
                payload: compress(dynamic, story),
                offset: 0,
            },
            IffChunk {
                chunk_id: STKS_ID,
                payload: encode_frames(&snapshot.frames)?,
                offset: 0,
            },
        ],
    ))
}

/// Parse a Quetzal file back into a state of play (Quetzal §2),
/// ready for Machine::restore. The file's IFhd must match the
/// story's identity (Quetzal §5.3), and CMem decompresses against
/// its original bytes.
pub fn read(data: &[u8], story: &Story) -> Result<Snapshot, VoxamError> {
    let (ifhd, memory_chunk, stks) = split(data)?;
    let pc = check_identity(&ifhd, story)?;
    let dynamic = decode_memory(&memory_chunk, story)?;
    let frames = decode_frames(&stks)?;

    Ok(Snapshot {
        dynamic_memory: dynamic,
        pc,
        frames,
    })
}

/// The ten bytes naming a story: release, serial, checksum
/// (Quetzal §5.3). A story too old to store a checksum gets one
/// calculated from its file, on saving and checking alike
/// (Quetzal §5.5).
pub fn story_identity(story: &Story) -> Vec<u8> {
    let header = story.header();
    let checksum = if header.stored_checksum() != 0 {
        header.stored_checksum()
    } else {
        header.computed_checksum()
    };

    let mut identity = Vec::with_capacity(IDENTITY_SIZE);
    identity.extend_from_slice(&header.release().to_be_bytes());
    identity.extend_from_slice(header.serial_number().as_bytes());
    identity.extend_from_slice(&checksum.to_be_bytes());

    identity
}

/// Build the 13 IFhd bytes (Quetzal §5.4).
fn encode_identity(pc: usize, story: &Story) -> Result<Vec<u8>, VoxamError> {
    let mut payload = story_identity(story);
    payload.extend_from_slice(&address(pc)?);

    Ok(payload)
}

/// Encode a 3-byte byte address (Quetzal §4.3.1, §5.4.6).
fn address(value: usize) -> Result<[u8; 3], VoxamError> {
    if value > ADDRESS_LIMIT {
        return Err(quetzal_error(format!(
            "address ${value:x} does not fit in the three bytes Quetzal stores \
             (Quetzal §4.3.1)"
        )));
    }

    let bytes = (value as u32).to_be_bytes();

    Ok([bytes[1], bytes[2], bytes[3]])
}

/// Compress dynamic memory against the original (Quetzal §3.2):
/// the current bytes exclusive-ored with the pristine story's, so
/// unchanged memory becomes zero, and runs of zeros collapse to a
/// zero byte plus a count of n+1. Trailing zeros are dropped
/// whole: a reader assumes the missing tail is unchanged (§3.4).
fn compress(dynamic: &[u8], story: &Story) -> Vec<u8> {
    let mut changed: Vec<u8> = dynamic
        .iter()
        .zip(story.data())
        .map(|(live, pristine)| live ^ pristine)
        .collect();

    while changed.last() == Some(&0) {
        changed.pop();
    }

    let mut encoded = Vec::new();
    let mut position = 0;

    while position < changed.len() {
        if changed[position] != 0 {
            encoded.push(changed[position]);
            position += 1;
            continue;
        }

        let mut run = position;

        while run < changed.len() && changed[run] == 0 {
            run += 1;
        }

        let mut length = position;

        while length < run {
            encoded.push(0);
            encoded.push(((run - length).min(LONGEST_RUN) - 1) as u8);
            length += LONGEST_RUN;
        }

        position = run;
    }

    encoded
}

/// Undo CMem compression against the original story (Quetzal
/// §3.2). Fails if the data ends mid-run or decodes to more than
/// dynamic memory holds -- the two read errors of Quetzal §3.5.
fn decompress(encoded: &[u8], story: &Story, size: usize) -> Result<Vec<u8>, VoxamError> {
    let mut changed = Vec::with_capacity(size);
    let mut position = 0;

    while position < encoded.len() {
        let byte = encoded[position];

        if byte != 0 {
            changed.push(byte);
            position += 1;
            continue;
        }

        if position + 1 == encoded.len() {
            return Err(quetzal_error(
                "compressed memory ends with a zero byte and no run length (Quetzal \
                 §3.5)"
                    .into(),
            ));
        }

        changed.extend(std::iter::repeat_n(
            0,
            usize::from(encoded[position + 1]) + 1,
        ));
        position += 2;
    }

    if changed.len() > size {
        return Err(quetzal_error(format!(
            "compressed memory decodes to {} bytes, but dynamic memory holds only \
             {size} (Quetzal §3.5)",
            changed.len()
        )));
    }

    changed.resize(size, 0);

    Ok(changed
        .iter()
        .zip(&story.data()[..size])
        .map(|(live, pristine)| live ^ pristine)
        .collect())
}

/// Lay the call chain out as Stks data, oldest first (Quetzal §4).
/// The base frame becomes the dummy frame of Quetzal §4.11: every
/// field zero except its evaluation stack count.
fn encode_frames(frames: &[FrameSnapshot]) -> Result<Vec<u8>, VoxamError> {
    let mut encoded = Vec::new();

    for (index, frame) in frames.iter().enumerate() {
        if index == 0 {
            encoded.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        } else {
            if frame.argument_count > ARGUMENTS_LIMIT {
                return Err(quetzal_error(format!(
                    "a frame holding {} arguments does not fit the seven argument \
                     bits (Quetzal §4.3.4)",
                    frame.argument_count
                )));
            }

            let mut flags = frame.locals.len() as u8;
            let store = match frame.store_variable {
                Some(variable) => variable,
                None => {
                    flags |= DISCARD_FLAG;

                    0
                }
            };

            encoded.extend_from_slice(&address(frame.return_address)?);
            encoded.extend_from_slice(&[flags, store, ((1u16 << frame.argument_count) - 1) as u8]);
        }

        encoded.extend_from_slice(&(frame.stack.len() as u16).to_be_bytes());

        for word in frame.locals.iter().chain(&frame.stack) {
            encoded.extend_from_slice(&word.to_be_bytes());
        }
    }

    Ok(encoded)
}

/// Parse Stks data back into a call chain (Quetzal §4). Fails if a
/// frame is cut short, uses reserved flag bits, holds a
/// gap-riddled argument mask, or the required dummy frame is not
/// dummy (Quetzal §4.11.1).
fn decode_frames(data: &[u8]) -> Result<Vec<FrameSnapshot>, VoxamError> {
    let mut frames = Vec::new();
    let mut position = 0;

    while position < data.len() {
        if position + FRAME_HEADER_SIZE > data.len() {
            return Err(quetzal_error(
                "a stack frame is cut short mid-header (Quetzal §4.3)".into(),
            ));
        }

        let return_address = usize::from(data[position]) << 16
            | usize::from(data[position + 1]) << 8
            | usize::from(data[position + 2]);
        let (flags, store, mask) = (data[position + 3], data[position + 4], data[position + 5]);
        let stack_count = usize::from(u16::from_be_bytes([data[position + 6], data[position + 7]]));
        position += FRAME_HEADER_SIZE;

        if flags & FLAGS_RESERVED != 0 {
            return Err(quetzal_error(format!(
                "a frame's flags byte ${flags:02x} uses reserved bits: only 000pvvvv \
                 is defined (Quetzal §4.3.2)"
            )));
        }

        if mask & mask.wrapping_add(1) != 0 {
            return Err(quetzal_error(format!(
                "a frame's argument mask ${mask:02x} has gaps: arguments are supplied \
                 in order (Quetzal §4.3.4)"
            )));
        }

        let local_count = usize::from(flags & LOCALS_MASK);
        let words_size = (local_count + stack_count) * WORD_SIZE;

        if position + words_size > data.len() {
            return Err(quetzal_error(
                "a stack frame is cut short mid-words (Quetzal §4.3)".into(),
            ));
        }

        let words: Vec<u16> = data[position..position + words_size]
            .chunks(WORD_SIZE)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        position += words_size;

        let store_variable = if frames.is_empty() {
            if return_address != 0 || flags != 0 || store != 0 || mask != 0 {
                return Err(quetzal_error(
                    "the first frame must be the dummy: every field zero but its \
                     stack count (Quetzal §4.11.1)"
                        .into(),
                ));
            }

            None
        } else if flags & DISCARD_FLAG != 0 {
            None
        } else {
            Some(store)
        };

        frames.push(FrameSnapshot {
            return_address,
            store_variable,
            locals: words[..local_count].to_vec(),
            argument_count: mask.count_ones() as usize,
            stack: words[local_count..].to_vec(),
        });
    }

    if frames.is_empty() {
        return Err(quetzal_error(
            "the Stks chunk is empty: the dummy frame is always present (Quetzal \
             §4.11.2)"
                .into(),
        ));
    }

    Ok(frames)
}

/// Pull the three required chunks out of the FORM (Quetzal §7.18):
/// the IFhd payload, the memory chunk as an (id, payload) pair,
/// and the Stks payload.
type MemoryChunk = ([u8; 4], Vec<u8>);

fn split(data: &[u8]) -> Result<(Vec<u8>, MemoryChunk, Vec<u8>), VoxamError> {
    let (form_type, chunks) = parse_form(data).map_err(|error| match error {
        VoxamError::Iff(message) => quetzal_error(message),
        other => other,
    })?;

    if &form_type != SAVE_FORM {
        return Err(quetzal_error(format!(
            "the FORM type is {:?}, not the IFZS of a saved game (Quetzal §2.1)",
            String::from_utf8_lossy(&form_type)
        )));
    }

    let mut ifhd: Option<Vec<u8>> = None;
    let mut cmem: Option<Vec<u8>> = None;
    let mut umem: Option<Vec<u8>> = None;
    let mut stks: Option<Vec<u8>> = None;

    for piece in chunks {
        if !matches!(piece.chunk_id, IFHD_ID | CMEM_ID | UMEM_ID | STKS_ID) {
            continue;
        }

        if piece.chunk_id != IFHD_ID && ifhd.is_none() {
            return Err(quetzal_error(format!(
                "the {:?} chunk arrives before IFhd, which must come first (Quetzal \
                 §5.4)",
                String::from_utf8_lossy(&piece.chunk_id)
            )));
        }

        let slot = match piece.chunk_id {
            IFHD_ID => &mut ifhd,
            CMEM_ID => &mut cmem,
            UMEM_ID => &mut umem,
            _ => &mut stks,
        };

        if slot.is_some() {
            return Err(quetzal_error(format!(
                "the {:?} chunk appears twice (Quetzal §7.18)",
                String::from_utf8_lossy(&piece.chunk_id)
            )));
        }

        *slot = Some(piece.payload);
    }

    let Some(ifhd) = ifhd else {
        return Err(quetzal_error(
            "the required IFhd chunk is missing (Quetzal §7.18)".into(),
        ));
    };

    let Some(stks) = stks else {
        return Err(quetzal_error(
            "the required Stks chunk is missing (Quetzal §7.18)".into(),
        ));
    };

    let memory_chunk = match (cmem, umem) {
        (Some(_), Some(_)) => {
            return Err(quetzal_error(
                "CMem and UMem both appear: a save carries one or the other (Quetzal \
                 §7.18)"
                    .into(),
            ));
        }
        (Some(payload), None) => (CMEM_ID, payload),
        (None, Some(payload)) => (UMEM_ID, payload),
        (None, None) => {
            return Err(quetzal_error(
                "the required CMem or UMem chunk is missing (Quetzal §7.18)".into(),
            ));
        }
    };

    Ok((ifhd, memory_chunk, stks))
}

/// Match the save to the story and return its PC (Quetzal §5.3):
/// the refusal §6.1.2.1 of the Standard asks for.
fn check_identity(ifhd: &[u8], story: &Story) -> Result<usize, VoxamError> {
    if ifhd.len() < IFHD_LENGTH {
        return Err(quetzal_error(format!(
            "the IFhd chunk holds {} bytes, fewer than the {IFHD_LENGTH} its first \
             bytes always contain (Quetzal §5.5)",
            ifhd.len()
        )));
    }

    if ifhd[..IDENTITY_SIZE] != story_identity(story) {
        return Err(quetzal_error(
            "this save names a different game: its release, serial, and checksum do \
             not match the story being played (Quetzal §5.3, §6.1.2.1)"
                .into(),
        ));
    }

    Ok(usize::from(ifhd[IDENTITY_SIZE]) << 16
        | usize::from(ifhd[IDENTITY_SIZE + 1]) << 8
        | usize::from(ifhd[IDENTITY_SIZE + 2]))
}

/// Recover dynamic memory from CMem or UMem (Quetzal §3).
fn decode_memory(memory_chunk: &([u8; 4], Vec<u8>), story: &Story) -> Result<Vec<u8>, VoxamError> {
    let (chunk_id, payload) = memory_chunk;
    let size = usize::from(story.header().static_memory_base());

    if *chunk_id == UMEM_ID {
        if payload.len() != size {
            return Err(quetzal_error(format!(
                "a UMem dump must be exactly dynamic memory: {size} bytes, not {} \
                 (Quetzal §3.6)",
                payload.len()
            )));
        }

        return Ok(payload.clone());
    }

    decompress(payload, story, size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zmachine::testing::story_bytes;

    fn scene() -> (Story, Snapshot) {
        let mut data = story_bytes(3, 512, 0x1C0, 0x1C0);
        data[0x02..0x04].copy_from_slice(&88u16.to_be_bytes());

        let story = Story::new(data).unwrap();
        let mut dynamic = story.data()[..0x1C0].to_vec();
        dynamic[0x100] = 0x42;
        dynamic[0x101] = 0x43;

        let snapshot = Snapshot {
            dynamic_memory: dynamic,
            pc: 0x1234,
            frames: vec![
                FrameSnapshot {
                    return_address: 0,
                    store_variable: None,
                    locals: Vec::new(),
                    argument_count: 0,
                    stack: vec![7, 8],
                },
                FrameSnapshot {
                    return_address: 0x0456,
                    store_variable: Some(0x10),
                    locals: vec![1, 2, 3],
                    argument_count: 2,
                    stack: vec![0xBEEF],
                },
                FrameSnapshot {
                    return_address: 0x0789,
                    store_variable: None,
                    locals: vec![5],
                    argument_count: 1,
                    stack: Vec::new(),
                },
            ],
        };

        (story, snapshot)
    }

    #[test]
    fn a_snapshot_round_trips() {
        let (story, snapshot) = scene();
        let saved = write(&snapshot, &story).unwrap();
        let restored = read(&saved, &story).unwrap();

        assert_eq!(restored, snapshot);
    }

    #[test]
    fn compression_drops_the_unchanged_tail() {
        let (story, snapshot) = scene();
        let compressed = compress(&snapshot.dynamic_memory, &story);

        // Only the two changed bytes and the zero-runs before them
        // survive; the tail is assumed unchanged (Quetzal §3.4).
        assert!(compressed.ends_with(&[0x42, 0x43]));
        assert!(compressed.len() < 8);
    }

    #[test]
    fn a_umem_dump_reads_back_too() {
        let (story, snapshot) = scene();

        let form = write_form(
            SAVE_FORM,
            &[
                IffChunk {
                    chunk_id: IFHD_ID,
                    payload: encode_identity(snapshot.pc, &story).unwrap(),
                    offset: 0,
                },
                IffChunk {
                    chunk_id: UMEM_ID,
                    payload: snapshot.dynamic_memory.clone(),
                    offset: 0,
                },
                IffChunk {
                    chunk_id: STKS_ID,
                    payload: encode_frames(&snapshot.frames).unwrap(),
                    offset: 0,
                },
            ],
        );

        assert_eq!(read(&form, &story).unwrap(), snapshot);
    }

    #[test]
    fn a_save_from_another_game_is_refused() {
        let (story, snapshot) = scene();
        let saved = write(&snapshot, &story).unwrap();

        let mut other_bytes = story_bytes(3, 512, 0x1C0, 0x1C0);
        other_bytes[0x02..0x04].copy_from_slice(&99u16.to_be_bytes());
        let other = Story::new(other_bytes).unwrap();

        let error = read(&saved, &other).unwrap_err();
        assert!(error.to_string().contains("different game"));
    }

    #[test]
    fn required_chunks_are_policed() {
        let (story, _) = scene();

        let no_stks = write_form(
            SAVE_FORM,
            &[IffChunk {
                chunk_id: IFHD_ID,
                payload: encode_identity(0, &story).unwrap(),
                offset: 0,
            }],
        );

        assert!(read(&no_stks, &story).is_err());

        let wrong_form = write_form(b"IFRS", &[]);
        let error = read(&wrong_form, &story).unwrap_err();
        assert!(error.to_string().contains("IFZS"));
    }
}
