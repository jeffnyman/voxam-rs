//! IFF containers: the FORM that saves and resources both live in.
//!
//! Quetzal saved games (FORM type IFZS) and Blorb resource files
//! (FORM type IFRS) are both simple IFF files: one outer FORM
//! chunk holding a form type and a sequence of typed chunks, each
//! padded to an even length. This module is the container alone --
//! what a FORM is, how a chunk is framed, where the pad bytes go
//! -- and knows nothing about what any chunk means. The structural
//! rules are cited by their Quetzal §8 numbers.

use crate::errors::VoxamError;

/// FORM is the single outer chunk of any simple IFF file, its
/// payload beginning with a four-byte form type (Quetzal §8.5).
const FORM_ID: &[u8; 4] = b"FORM";
const TYPE_SIZE: usize = 4;

/// A chunk is an ID, a 32-bit big-endian length, and that many
/// bytes of data (Quetzal §8.3, §8.4); odd data gains a pad byte
/// the length does not count (Quetzal §8.4.1).
const CHUNK_HEADER_SIZE: usize = 8;

/// One typed chunk: a four-byte ID and its payload (Quetzal §8.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IffChunk {
    /// The four-byte chunk type.
    pub chunk_id: [u8; 4],
    /// The chunk's data, pad byte excluded.
    pub payload: Vec<u8>,
    /// Where the chunk's header begins in its file -- the address
    /// a Blorb resource index speaks. Zero on chunks built by hand.
    pub offset: usize,
}

fn iff_error(message: String) -> VoxamError {
    VoxamError::Iff(message)
}

/// Frame a payload as an IFF chunk, padding odd data (Quetzal
/// §8.4.1): ID, big-endian length, data, and a pad byte after odd
/// data.
pub fn chunk(chunk_id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(CHUNK_HEADER_SIZE + payload.len() + 1);
    framed.extend_from_slice(chunk_id);
    framed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    framed.extend_from_slice(payload);

    if !payload.len().is_multiple_of(2) {
        framed.push(0);
    }

    framed
}

/// Assemble a FORM from its type and chunks (Quetzal §8.5).
pub fn write_form(form_type: &[u8; 4], chunks: &[IffChunk]) -> Vec<u8> {
    let mut body = form_type.to_vec();

    for piece in chunks {
        body.extend(chunk(&piece.chunk_id, &piece.payload));
    }

    chunk(FORM_ID, &body)
}

/// Open a FORM and walk every chunk inside it, in order.
///
/// Every chunk is collected, known or not: what a chunk means is
/// the caller's business. Fails if the bytes are not a FORM, the
/// FORM claims more than the file holds, or a chunk is truncated.
pub fn parse_form(data: &[u8]) -> Result<([u8; 4], Vec<IffChunk>), VoxamError> {
    if data.len() < CHUNK_HEADER_SIZE + TYPE_SIZE || &data[..4] != FORM_ID {
        return Err(iff_error(
            "not an IFF file: no FORM chunk to open it (Quetzal §8.5)".into(),
        ));
    }

    let length = u32::from_be_bytes(data[4..8].try_into().expect("four bytes")) as usize;

    if CHUNK_HEADER_SIZE + length > data.len() {
        return Err(iff_error(format!(
            "the FORM chunk claims {length} bytes, but the file has only {} after \
             its header (Quetzal §8.3.5)",
            data.len() - CHUNK_HEADER_SIZE
        )));
    }

    let form_type: [u8; 4] = data[8..12].try_into().expect("four bytes");
    let mut found = Vec::new();
    let mut position = CHUNK_HEADER_SIZE + TYPE_SIZE;
    let end = CHUNK_HEADER_SIZE + length;

    while position < end {
        if position + CHUNK_HEADER_SIZE > end {
            return Err(iff_error(
                "a chunk is cut short mid-header (Quetzal §8.3.1)".into(),
            ));
        }

        let header_offset = position;
        let chunk_id: [u8; 4] = data[position..position + 4].try_into().expect("four bytes");
        let size =
            u32::from_be_bytes(data[position + 4..position + 8].try_into().expect("four")) as usize;
        position += CHUNK_HEADER_SIZE;

        if position + size > end {
            return Err(iff_error(format!(
                "the {} chunk claims {size} bytes, but the FORM ends before them \
                 (Quetzal §8.4)",
                String::from_utf8_lossy(&chunk_id)
            )));
        }

        found.push(IffChunk {
            chunk_id,
            payload: data[position..position + size].to_vec(),
            offset: header_offset,
        });
        position += size + size % 2;
    }

    Ok((form_type, found))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_and_pads_chunks() {
        assert_eq!(chunk(b"TEST", b"ab"), b"TEST\x00\x00\x00\x02ab");
        assert_eq!(chunk(b"TEST", b"abc"), b"TEST\x00\x00\x00\x03abc\x00");
    }

    #[test]
    fn a_form_round_trips() {
        let pieces = vec![
            IffChunk {
                chunk_id: *b"AAAA",
                payload: b"one".to_vec(),
                offset: 0,
            },
            IffChunk {
                chunk_id: *b"BBBB",
                payload: b"pair".to_vec(),
                offset: 0,
            },
        ];

        let form = write_form(b"IFZS", &pieces);
        let (form_type, walked) = parse_form(&form).unwrap();

        assert_eq!(&form_type, b"IFZS");
        assert_eq!(walked.len(), 2);
        assert_eq!(walked[0].payload, b"one");
        assert_eq!(walked[1].chunk_id, *b"BBBB");

        // The odd chunk gained its pad, so the next header sits on
        // an even offset (Quetzal §8.4.1).
        assert_eq!(walked[1].offset % 2, 0);
    }

    #[test]
    fn refuses_what_is_not_a_form() {
        assert!(parse_form(b"JUNK").is_err());
        assert!(parse_form(b"FORM\x00\x00\x00\xFFIFZS").is_err());
    }
}
