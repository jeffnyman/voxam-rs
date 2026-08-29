//! The Å-machine savefile: the AASV form written and revived.
//!
//! A saved game is IFF: form AASV holding HEAD, DATA, and REGS
//! (Aa-machine: Savefile). HEAD is the story's own header, copied
//! byte for byte so a savefile can never be revived into the wrong
//! story. DATA is the whole game state -- the initialized
//! registers, then the random access area, the auxiliary heap, and
//! the main heap, big-endian words all -- exclusive-orred against
//! the INIT chunk padded with the unused word, then run-length
//! encoded: a null byte followed by N-1 stands for a stretch of N
//! nulls. REGS carries the sixty-four general registers, the
//! special registers, and the open divs.
//!
//! The captured State these functions speak is the machine's own:
//! the same shape Machine's capture builds and its restore takes
//! back.

use crate::aamachine::story::Story;
use crate::errors::VoxamError;
use crate::iff::{chunk, parse_form};

pub const FORM_ID: [u8; 4] = *b"AASV";

// The unused-word stamp that pads the INIT chunk out to the full
// state (Aa-machine: Savefile).
const UNUSED: [u8; 2] = [0x3F, 0x3F];

// The REGS chunk's fixed bytes before the div list: sixty-four
// registers, two longs, eight words, two lone bytes, and the div
// count itself (Aa-machine: Savefile).
const REGS_FIXED: usize = 156;

// The longest null run one encoded pair can spell.
const RUN_TOP: usize = 256;

fn saves_error(message: String) -> VoxamError {
    VoxamError::AAMachine(message)
}

/// One captured game state: the initialized registers, the three
/// memory areas masked to their allocations, the general and
/// special registers, and the open divs (Aa-machine: Savefile).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    /// The INIT registers: object count, long-term base and top.
    pub counted: (u16, u16, u16),
    /// The random access area, unallocated words masked unused.
    pub ram: Vec<u16>,
    /// The auxiliary area, the gap masked unused.
    pub aux: Vec<u16>,
    /// The main heap, the gap masked unused.
    pub heap: Vec<u16>,
    /// The sixty-four general registers.
    pub regs: Vec<u16>,
    /// The landing address, continuation, and the four heap marks:
    /// top, env, cho, sim.
    pub flow: (u32, u32, u16, u16, u16, u16),
    /// The aux marks and output bytes: auxp, trl, sta, stc, cwl,
    /// spc.
    pub stacks: (u16, u16, u16, u16, u8, u8),
    /// The open div styles, oldest first.
    pub divs: Vec<u16>,
}

/// One captured state as a whole AASV savefile (Aa-machine: Savefile).
pub fn kept(story: &Story, state: &State) -> Vec<u8> {
    let mut told = worded(&[state.counted.0, state.counted.1, state.counted.2]);

    told.extend(worded(&state.ram));
    told.extend(worded(&state.aux));
    told.extend(worded(&state.heap));

    let base = grounded(story, told.len());
    let diff: Vec<u8> = told.iter().zip(&base).map(|(a, b)| a ^ b).collect();
    let (landing, cont, top, env, cho, sim) = state.flow;
    let (auxp, trl, sta, stc, cwl, spc) = state.stacks;
    let mut registers = worded(&state.regs);

    registers.extend_from_slice(&landing.to_be_bytes());
    registers.extend_from_slice(&cont.to_be_bytes());
    registers.extend(worded(&[top, env, cho, sim, auxp, trl, sta, stc]));
    registers.push(cwl);
    registers.push(spc);
    registers.extend_from_slice(&(state.divs.len() as u16).to_be_bytes());
    registers.extend(worded(&state.divs));

    let mut body = FORM_ID.to_vec();

    body.extend(chunk(b"HEAD", &headed(story)));
    body.extend(chunk(b"DATA", &shrunk(&diff)));
    body.extend(chunk(b"REGS", &registers));

    chunk(b"FORM", &body)
}

/// A savefile's captured state, verified against its story.
///
/// Fails for a form that is not AASV, a HEAD that does not match
/// the story's own, or DATA or REGS that cannot hold the story's
/// whole state; and for a FORM that cannot be walked at all.
pub fn revived(story: &Story, data: &[u8]) -> Result<State, VoxamError> {
    let (form, chunks) = parse_form(data)?;

    if form != FORM_ID {
        return Err(saves_error(format!(
            "a saved game is FORM AASV, not FORM {} (Aa-machine: Savefile)",
            form.iter()
                .map(|&byte| if byte < 0x80 {
                    byte as char
                } else {
                    '\u{fffd}'
                })
                .collect::<String>()
        )));
    }

    let held = |name: &[u8; 4]| chunks.iter().find(|piece| piece.chunk_id == *name);

    for name in [b"HEAD", b"DATA", b"REGS"] {
        if held(name).is_none() {
            return Err(saves_error(format!(
                "the savefile is missing its {} chunk (Aa-machine: Savefile)",
                name.iter().map(|&byte| byte as char).collect::<String>()
            )));
        }
    }

    let head = held(b"HEAD").expect("checked present");

    if head.payload != headed(story) {
        return Err(saves_error(
            "the savefile's HEAD does not match this story's -- it belongs to \
             another game or another release (Aa-machine: Savefile)"
                .into(),
        ));
    }

    let words = 3
        + usize::from(story.ram_size)
        + usize::from(story.aux_size)
        + usize::from(story.heap_size);
    let diff = grown(&held(b"DATA").expect("checked present").payload)?;

    if diff.len() != words * 2 {
        return Err(saves_error(format!(
            "the savefile's DATA unpacks to {} bytes, but this story's state \
             is {} (Aa-machine: Savefile)",
            diff.len(),
            words * 2
        )));
    }

    let base = grounded(story, diff.len());
    let told: Vec<u8> = diff.iter().zip(&base).map(|(a, b)| a ^ b).collect();
    let values: Vec<u16> = told
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_be_bytes(pair))
        .collect();
    let ram_end = 3 + usize::from(story.ram_size);
    let aux_end = ram_end + usize::from(story.aux_size);
    let (regs, flow, stacks, divs) = registered(&held(b"REGS").expect("checked present").payload)?;

    Ok(State {
        counted: (values[0], values[1], values[2]),
        ram: values[3..ram_end].to_vec(),
        aux: values[ram_end..aux_end].to_vec(),
        heap: values[aux_end..].to_vec(),
        regs,
        flow,
        stacks,
        divs,
    })
}

type Registered = (
    Vec<u16>,
    (u32, u32, u16, u16, u16, u16),
    (u16, u16, u16, u16, u8, u8),
    Vec<u16>,
);

/// The REGS chunk's registers and divs (Aa-machine: Savefile).
///
/// Fails for a chunk too short for its own claims.
pub(crate) fn registered(payload: &[u8]) -> Result<Registered, VoxamError> {
    if payload.len() < REGS_FIXED {
        return Err(saves_error(
            "the savefile's REGS chunk is too short (Aa-machine: Savefile)".into(),
        ));
    }

    let word = |at: usize| u16::from_be_bytes([payload[at], payload[at + 1]]);
    let long = |at: usize| {
        u32::from_be_bytes([
            payload[at],
            payload[at + 1],
            payload[at + 2],
            payload[at + 3],
        ])
    };
    let regs: Vec<u16> = (0..64).map(|seat| word(seat * 2)).collect();
    let landing = long(128);
    let cont = long(132);
    let marks: Vec<u16> = (0..8).map(|seat| word(136 + seat * 2)).collect();
    let cwl = payload[152];
    let spc = payload[153];
    let counted = usize::from(word(154));

    if 156 + counted * 2 > payload.len() {
        return Err(saves_error(format!(
            "the savefile claims {counted} open divs, past the REGS chunk's \
             end (Aa-machine: Savefile)"
        )));
    }

    let divs: Vec<u16> = (0..counted).map(|seat| word(156 + seat * 2)).collect();

    Ok((
        regs,
        (landing, cont, marks[0], marks[1], marks[2], marks[3]),
        (marks[4], marks[5], marks[6], marks[7], cwl, spc),
        divs,
    ))
}

/// The story's own HEAD payload, the savefile's identity check.
fn headed(story: &Story) -> Vec<u8> {
    story.summed(b"HEAD").payload.clone()
}

/// Words as big-endian bytes (Aa-machine: Savefile).
fn worded(values: &[u16]) -> Vec<u8> {
    let mut told = Vec::with_capacity(values.len() * 2);

    for value in values {
        told.extend_from_slice(&value.to_be_bytes());
    }

    told
}

/// The INIT chunk padded with the unused word to a length.
pub(crate) fn grounded(story: &Story, length: usize) -> Vec<u8> {
    let mut base = story.summed(b"INIT").payload.clone();

    while base.len() < length {
        base.extend_from_slice(&UNUSED);
    }

    base.truncate(length);

    base
}

/// Run-length encode: N nulls become a null and N-1.
pub(crate) fn shrunk(data: &[u8]) -> Vec<u8> {
    let mut told = Vec::new();
    let mut at = 0;

    while at < data.len() {
        if data[at] != 0 {
            told.push(data[at]);
            at += 1;
        } else {
            let mut run = 1;

            while run < RUN_TOP && at + run < data.len() && data[at + run] == 0 {
                run += 1;
            }

            told.push(0);
            told.push((run - 1) as u8);
            at += run;
        }
    }

    told
}

/// Run-length decode, the encoder's exact inverse.
///
/// Fails for a stream ending inside a null run.
pub(crate) fn grown(data: &[u8]) -> Result<Vec<u8>, VoxamError> {
    let mut told = Vec::new();
    let mut at = 0;

    while at < data.len() {
        if data[at] != 0 {
            told.push(data[at]);
            at += 1;
        } else {
            if at + 1 >= data.len() {
                return Err(saves_error(
                    "the savefile's DATA ends inside a null run (Aa-machine: Savefile)".into(),
                ));
            }

            told.extend(std::iter::repeat_n(0u8, usize::from(data[at + 1]) + 1));
            at += 2;
        }
    }

    Ok(told)
}

#[cfg(test)]
mod tests {
    use super::*;

    // DATA ending inside a null run is refused rather than
    // guessed.
    #[test]
    fn a_torn_null_run_is_refused() {
        let told = grown(&[0x01, 0x00]).expect_err("a torn run").to_string();

        assert!(told.contains("inside a null run"), "{told}");
    }

    // The run-length coding is its own inverse, the longest runs
    // split at the 256-null seam.
    #[test]
    fn the_run_length_coding_inverts() {
        let mut data = vec![0x07];

        data.extend(vec![0u8; 700]);
        data.extend_from_slice(&[0x09, 0x00]);

        assert_eq!(grown(&shrunk(&data)).unwrap(), data);
    }

    // A REGS chunk shorter than its fixed registers is refused,
    // and one claiming divs past its end likewise.
    #[test]
    fn a_short_or_overclaiming_regs_is_refused() {
        let told = registered(&[0u8; 10]).expect_err("too short").to_string();

        assert!(told.contains("REGS chunk is too short"), "{told}");

        let mut overclaiming = vec![0u8; 154];

        overclaiming.extend_from_slice(&9u16.to_be_bytes());

        let told = registered(&overclaiming)
            .expect_err("an overclaim")
            .to_string();

        assert!(told.contains("claims 9 open divs"), "{told}");
    }
}
