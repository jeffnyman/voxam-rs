//! Blorb resource files: the package stories and their art travel
//! in (Blorb 2.0.4).
//!
//! A Blorb is an IFF FORM of type IFRS whose RIdx chunk indexes
//! every resource by usage and number, each entry pointing at a
//! chunk by its file offset. This port carries the walkable index,
//! the packaged story, the identity check, and the census; the
//! gallery, palettes, and sounds arrive with the faces that draw
//! and play them.

use std::collections::HashSet;

use crate::errors::VoxamError;
use crate::iff::{IffChunk, parse_form};
use crate::zmachine::quetzal::story_identity;
use crate::zmachine::story::Story;

/// The FORM type of a resource file (Blorb: Introduction).
const RESOURCE_FORM: [u8; 4] = *b"IFRS";

/// The resource index: a count, then 12-byte entries of usage,
/// number, and chunk offset (Blorb: Resource Index Chunk).
const INDEX_ID: [u8; 4] = *b"RIdx";
const COUNT_SIZE: usize = 4;
const ENTRY_SIZE: usize = 12;

/// The §11 usages, and the executable seat's fixed number.
pub const USAGE_PICTURE: [u8; 4] = *b"Pict";
pub const USAGE_SOUND: [u8; 4] = *b"Snd ";
pub const USAGE_DATA: [u8; 4] = *b"Data";
pub const USAGE_EXEC: [u8; 4] = *b"Exec";

/// The PNG picture chunk type (Blorb: Picture Resource Chunks).
pub const PNG_ID: [u8; 4] = *b"PNG ";
const ZCODE_ID: [u8; 4] = *b"ZCOD";
const GLULX_ID: [u8; 4] = *b"GLUL";
const EXEC_NUMBER: u32 = 0;

/// The chunk naming the story these resources belong to (Blorb:
/// Game Identifier Chunk) -- the same ten bytes a Quetzal save
/// uses.
const IDENTITY_ID: [u8; 4] = *b"IFhd";
const IDENTITY_SIZE: usize = 10;

/// Optional chunks: the frontispiece names a picture resource to
/// stand as cover art (Blorb: Frontispiece Chunk), and the IFmd
/// chunk carries the story's iFiction record as XML bytes (Blorb:
/// Metadata).
const FRONTISPIECE_ID: [u8; 4] = *b"Fspc";
const FRONTISPIECE_SIZE: usize = 4;
const IFICTION_ID: [u8; 4] = *b"IFmd";

/// From Version 5 the repeat count rides sound_effect's operand;
/// Version 3 uses the Loop chunk instead: eight-byte entries
/// pairing a sound number with a repeat flag -- zero repeats until
/// stopped -- and an absent entry means once (Blorb: The Looping
/// Chunk).
const LOOPING_ID: [u8; 4] = *b"Loop";
const LOOP_ENTRY_SIZE: usize = 8;
const PLAY_ONCE: u32 = 1;

fn blorb_error(message: String) -> VoxamError {
    VoxamError::Blorb(message)
}

/// One indexed resource: its usage, number, and chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub usage: [u8; 4],
    pub number: u32,
    pub chunk: IffChunk,
}

/// A parsed resource file: the index made walkable.
#[derive(Debug)]
pub struct Blorb {
    /// Every indexed resource, in index order.
    pub resources: Vec<Resource>,
    /// The IFhd payload naming the story these resources belong
    /// to, or None without one.
    pub identity: Option<Vec<u8>>,
    /// The picture number offered as cover art, or None (Blorb:
    /// Frontispiece Chunk).
    pub frontispiece: Option<u32>,
    /// The sounds a Version 3 game plays on repeat until stopped,
    /// by number (Blorb: The Looping Chunk); empty without a Loop
    /// chunk, and ignored from Version 5 on, where the operand
    /// carries the count.
    pub loops: HashSet<u32>,
    /// The IFmd chunk's iFiction record, XML bytes as they
    /// arrived, or None (Blorb: Metadata).
    pub ifiction: Option<Vec<u8>>,
}

impl Blorb {
    /// Parse Blorb bytes into an indexed resource set.
    ///
    /// Fails if the bytes are not an IFRS FORM, the index is
    /// missing, doubled, or malformed, or an entry points at no
    /// chunk.
    pub fn parse(data: &[u8]) -> Result<Self, VoxamError> {
        let (form_type, chunks) = parse_form(data).map_err(|error| match error {
            VoxamError::Iff(message) => blorb_error(message),
            other => other,
        })?;

        if form_type != RESOURCE_FORM {
            return Err(blorb_error(format!(
                "the FORM type is {:?}, not the IFRS of a resource file (Blorb: \
                 Introduction)",
                String::from_utf8_lossy(&form_type)
            )));
        }

        let indexes: Vec<&IffChunk> = chunks
            .iter()
            .filter(|piece| piece.chunk_id == INDEX_ID)
            .collect();

        if indexes.len() != 1 {
            return Err(blorb_error(format!(
                "a Blorb carries exactly one RIdx resource index; this one has {} \
                 (Blorb: Resource Index Chunk)",
                indexes.len()
            )));
        }

        let resources = entries(&indexes[0].payload, &chunks)?;
        let identity = chunks
            .iter()
            .find(|piece| piece.chunk_id == IDENTITY_ID)
            .map(|piece| piece.payload.clone());
        let frontispiece = frontispiece(&chunks)?;
        let loops = loops(&chunks)?;
        let ifiction = chunks
            .iter()
            .find(|piece| piece.chunk_id == IFICTION_ID)
            .map(|piece| piece.payload.clone());

        Ok(Self {
            resources,
            identity,
            frontispiece,
            loops,
            ifiction,
        })
    }

    /// The resource a game asks for by usage and number.
    pub fn resource(&self, usage: [u8; 4], number: u32) -> Option<&Resource> {
        self.resources
            .iter()
            .find(|piece| piece.usage == usage && piece.number == number)
    }

    /// The packaged Z-code story, when the Blorb carries one: the
    /// Exec resource numbered 0 in the ZCOD executable format
    /// (Blorb: Code Resource Chunks).
    pub fn story(&self) -> Option<&[u8]> {
        let executable = self.resource(USAGE_EXEC, EXEC_NUMBER)?;

        if executable.chunk.chunk_id != ZCODE_ID {
            return None;
        }

        Some(&executable.chunk.payload)
    }

    /// The packaged Glulx story: the same Exec seat, in the GLUL
    /// executable format instead.
    pub fn glulx(&self) -> Option<&[u8]> {
        let executable = self.resource(USAGE_EXEC, EXEC_NUMBER)?;

        if executable.chunk.chunk_id != GLULX_ID {
            return None;
        }

        Some(&executable.chunk.payload)
    }

    /// Whether the resources name this story (Blorb: Game
    /// Identifier Chunk). A Blorb without an identity matches
    /// anything: the check is optional, and absence is not
    /// disagreement.
    pub fn matches(&self, story: &Story) -> bool {
        match &self.identity {
            None => true,
            Some(identity) => {
                identity.len() >= IDENTITY_SIZE
                    && identity[..IDENTITY_SIZE] == story_identity(story)
            }
        }
    }

    /// A one-line census for the session banner.
    pub fn described(&self) -> String {
        let pictures = self
            .resources
            .iter()
            .filter(|piece| piece.usage == USAGE_PICTURE)
            .count();
        let sounds = self
            .resources
            .iter()
            .filter(|piece| piece.usage == USAGE_SOUND)
            .count();

        let mut parts = Vec::new();

        if pictures > 0 {
            parts.push(format!(
                "{pictures} picture{}",
                if pictures != 1 { "s" } else { "" }
            ));
        }

        if sounds > 0 {
            parts.push(format!(
                "{sounds} sound{}",
                if sounds != 1 { "s" } else { "" }
            ));
        }

        if self.story().is_some() || self.glulx().is_some() {
            parts.push("a packaged story".to_string());
        }

        if parts.is_empty() {
            "no resources".to_string()
        } else {
            parts.join(", ")
        }
    }
}

/// The cover picture's number, from at most one Fspc chunk. Fails
/// for a doubled Fspc, or one without its four number bytes
/// (Blorb: Frontispiece Chunk).
fn frontispiece(chunks: &[IffChunk]) -> Result<Option<u32>, VoxamError> {
    let found: Vec<&IffChunk> = chunks
        .iter()
        .filter(|piece| piece.chunk_id == FRONTISPIECE_ID)
        .collect();

    if found.is_empty() {
        return Ok(None);
    }

    if found.len() > 1 {
        return Err(blorb_error(format!(
            "{} Fspc chunks appear, but there may not be more than one (Blorb: \
             Frontispiece Chunk)",
            found.len()
        )));
    }

    if found[0].payload.len() != FRONTISPIECE_SIZE {
        return Err(blorb_error(
            "the Fspc chunk does not hold its four picture-number bytes".into(),
        ));
    }

    Ok(Some(u32::from_be_bytes(
        found[0].payload[..4].try_into().expect("four bytes"),
    )))
}

/// The repeat-forever sound numbers, from at most one Loop chunk.
///
/// A flag of zero repeats the sound until it is stopped; any other
/// flag, or no entry at all, plays it once (Blorb: The Looping
/// Chunk). Fails for a doubled Loop chunk, or one whose length is
/// not a whole number of eight-byte entries.
fn loops(chunks: &[IffChunk]) -> Result<HashSet<u32>, VoxamError> {
    let found: Vec<&IffChunk> = chunks
        .iter()
        .filter(|piece| piece.chunk_id == LOOPING_ID)
        .collect();

    if found.is_empty() {
        return Ok(HashSet::new());
    }

    if found.len() > 1 {
        return Err(blorb_error(format!(
            "{} Loop chunks appear, but there may not be more than one (Blorb: The \
             Looping Chunk)",
            found.len()
        )));
    }

    let payload = &found[0].payload;

    if !payload.len().is_multiple_of(LOOP_ENTRY_SIZE) {
        return Err(blorb_error(format!(
            "a Loop chunk is eight-byte entries, but this one holds {} bytes \
             (Blorb: The Looping Chunk)",
            payload.len()
        )));
    }

    let (entries, _) = payload.as_chunks::<LOOP_ENTRY_SIZE>();

    Ok(entries
        .iter()
        .filter(|entry| {
            u32::from_be_bytes(entry[4..8].try_into().expect("four bytes")) != PLAY_ONCE
        })
        .map(|entry| u32::from_be_bytes(entry[..4].try_into().expect("four bytes")))
        .collect())
}

/// Decode the index entries and resolve their chunks. Fails if the
/// count disagrees with the payload size, or an entry's offset
/// points at no chunk.
fn entries(payload: &[u8], chunks: &[IffChunk]) -> Result<Vec<Resource>, VoxamError> {
    if payload.len() < COUNT_SIZE {
        return Err(blorb_error(
            "the RIdx chunk is too short to hold its own count".into(),
        ));
    }

    let count = u32::from_be_bytes(payload[..4].try_into().expect("four bytes")) as usize;

    if payload.len() != COUNT_SIZE + count * ENTRY_SIZE {
        return Err(blorb_error(format!(
            "the RIdx count of {count} needs {} bytes, but the chunk holds {} \
             (Blorb: Resource Index Chunk)",
            COUNT_SIZE + count * ENTRY_SIZE,
            payload.len()
        )));
    }

    let mut resources = Vec::with_capacity(count);

    for index in 0..count {
        let start = COUNT_SIZE + index * ENTRY_SIZE;
        let usage: [u8; 4] = payload[start..start + 4].try_into().expect("four bytes");
        let number = u32::from_be_bytes(payload[start + 4..start + 8].try_into().expect("four"));
        let offset =
            u32::from_be_bytes(payload[start + 8..start + 12].try_into().expect("four")) as usize;

        let Some(chunk) = chunks.iter().find(|piece| piece.offset == offset) else {
            return Err(blorb_error(format!(
                "the {:?} {number} entry points at offset {offset}, where no chunk \
                 begins (Blorb: Resource Index Chunk)",
                String::from_utf8_lossy(&usage)
            )));
        };

        resources.push(Resource {
            usage,
            number,
            chunk: chunk.clone(),
        });
    }

    Ok(resources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iff::write_form;
    use crate::zmachine::testing::story_bytes;

    /// Assemble a minimal Blorb: an index, a packaged story, and a
    /// picture, offsets computed the way a writer lays them out.
    fn scene() -> (Vec<u8>, Vec<u8>) {
        let packaged = story_bytes(5, 128, 64, 64);

        // The RIdx payload sits first; its entries point at the
        // chunks that follow it. Offsets: FORM header (8) + type
        // (4) + RIdx header (8) + payload (4 + 2*12 = 28).
        let exec_offset = 12 + 8 + 28;
        let pict_offset = exec_offset + 8 + packaged.len() + packaged.len() % 2;

        let mut index = 2u32.to_be_bytes().to_vec();
        index.extend_from_slice(b"Exec");
        index.extend_from_slice(&0u32.to_be_bytes());
        index.extend_from_slice(&(exec_offset as u32).to_be_bytes());
        index.extend_from_slice(b"Pict");
        index.extend_from_slice(&1u32.to_be_bytes());
        index.extend_from_slice(&(pict_offset as u32).to_be_bytes());

        let form = write_form(
            b"IFRS",
            &[
                IffChunk {
                    chunk_id: INDEX_ID,
                    payload: index,
                    offset: 0,
                },
                IffChunk {
                    chunk_id: ZCODE_ID,
                    payload: packaged.clone(),
                    offset: 0,
                },
                IffChunk {
                    chunk_id: *b"PNG ",
                    payload: vec![1, 2, 3],
                    offset: 0,
                },
            ],
        );

        (form, packaged)
    }

    #[test]
    fn unwraps_the_packaged_story() {
        let (form, packaged) = scene();
        let blorb = Blorb::parse(&form).unwrap();

        assert_eq!(blorb.story().unwrap(), packaged);
        assert!(blorb.glulx().is_none());
    }

    #[test]
    fn the_census_counts_by_usage() {
        let (form, _) = scene();
        let blorb = Blorb::parse(&form).unwrap();

        assert_eq!(blorb.described(), "1 picture, a packaged story");
    }

    #[test]
    fn a_blorb_without_identity_matches_anything() {
        let (form, _) = scene();
        let blorb = Blorb::parse(&form).unwrap();
        let story = Story::new(story_bytes(3, 128, 64, 64)).unwrap();

        assert!(blorb.matches(&story));
    }

    /// An empty index plus whatever optional chunks a test hangs.
    fn bare_blorb(extra: &[IffChunk]) -> Vec<u8> {
        let mut chunks = vec![IffChunk {
            chunk_id: INDEX_ID,
            payload: 0u32.to_be_bytes().to_vec(),
            offset: 0,
        }];

        chunks.extend_from_slice(extra);

        write_form(b"IFRS", &chunks)
    }

    fn optional(chunk_id: [u8; 4], payload: Vec<u8>) -> IffChunk {
        IffChunk {
            chunk_id,
            payload,
            offset: 0,
        }
    }

    // The frontispiece names a cover picture; doubling it is
    // refused (Blorb: Frontispiece Chunk).
    #[test]
    fn the_frontispiece_is_read_and_policed() {
        let named = bare_blorb(&[optional(*b"Fspc", 2u32.to_be_bytes().to_vec())]);

        assert_eq!(Blorb::parse(&named).unwrap().frontispiece, Some(2));
        assert_eq!(Blorb::parse(&bare_blorb(&[])).unwrap().frontispiece, None);

        let doubled = bare_blorb(&[
            optional(*b"Fspc", 1u32.to_be_bytes().to_vec()),
            optional(*b"Fspc", 2u32.to_be_bytes().to_vec()),
        ]);

        assert!(
            Blorb::parse(&doubled)
                .unwrap_err()
                .to_string()
                .contains("more than one")
        );

        let stunted = bare_blorb(&[optional(*b"Fspc", vec![1])]);

        assert!(
            Blorb::parse(&stunted)
                .unwrap_err()
                .to_string()
                .contains("four picture-number bytes")
        );
    }

    // A Loop chunk marks which Version 3 sounds repeat until
    // stopped: flag zero loops, anything else -- like an absent
    // entry -- plays once (Blorb: The Looping Chunk).
    #[test]
    fn the_loop_chunk_names_the_repeating_sounds() {
        let mut entries = Vec::new();

        entries.extend(4u32.to_be_bytes());
        entries.extend(0u32.to_be_bytes());
        entries.extend(7u32.to_be_bytes());
        entries.extend(1u32.to_be_bytes());

        let blorb = Blorb::parse(&bare_blorb(&[optional(*b"Loop", entries)])).unwrap();

        assert_eq!(blorb.loops, HashSet::from([4]));
        assert!(Blorb::parse(&bare_blorb(&[])).unwrap().loops.is_empty());
    }

    // Doubled or ragged Loop chunks are refused.
    #[test]
    fn malformed_loop_chunks_are_refused() {
        let doubled = bare_blorb(&[
            optional(*b"Loop", Vec::new()),
            optional(*b"Loop", Vec::new()),
        ]);

        assert!(
            Blorb::parse(&doubled)
                .unwrap_err()
                .to_string()
                .contains("Loop chunks appear")
        );

        let ragged = bare_blorb(&[optional(*b"Loop", vec![0; 7])]);

        assert!(
            Blorb::parse(&ragged)
                .unwrap_err()
                .to_string()
                .contains("eight-byte entries")
        );
    }

    // The IFmd chunk's iFiction record rides whole, exactly as it
    // arrived (Blorb: Metadata).
    #[test]
    fn the_ifiction_record_rides_whole() {
        let record = b"<ifindex><story></story></ifindex>".to_vec();
        let held = bare_blorb(&[optional(*b"IFmd", record.clone())]);

        assert_eq!(Blorb::parse(&held).unwrap().ifiction, Some(record));
        assert_eq!(Blorb::parse(&bare_blorb(&[])).unwrap().ifiction, None);
    }

    #[test]
    fn refuses_a_missing_index() {
        let form = write_form(b"IFRS", &[]);
        let error = Blorb::parse(&form).unwrap_err();

        assert!(error.to_string().contains("RIdx"));
    }

    #[test]
    fn refuses_the_wrong_form() {
        let form = write_form(b"IFZS", &[]);

        assert!(Blorb::parse(&form).is_err());
    }
}
