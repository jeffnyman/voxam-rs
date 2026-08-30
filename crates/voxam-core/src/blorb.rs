//! Blorb resource files: the package stories and their art travel
//! in (Blorb 2.0.4).
//!
//! A Blorb is an IFF FORM of type IFRS whose RIdx chunk indexes
//! every resource by usage and number, each entry pointing at a
//! chunk by its file offset. This port carries the walkable index,
//! the packaged story, the identity check, the census, and the
//! Version 6 art's whole apparatus: the gallery, the Reso scaling
//! instructions, and the adaptive-palette chunks.

use std::collections::{HashMap, HashSet};

use crate::errors::VoxamError;
use crate::gallery::{Art, Gallery, Placard, Ratio, Resolution, Scaling};
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

/// Pictures arrive as PNG or JPEG chunks, with Rect placeholders
/// among the Version 6 art (Blorb: Picture Resource Chunks); PNG is
/// the one Voxam can draw. A Rect carries two four-byte words --
/// width, then height -- and no pixels at all.
pub const PNG_ID: [u8; 4] = *b"PNG ";
pub const RECT_ID: [u8; 4] = *b"Rect";
const RECT_SIZE: usize = 8;

/// The resource file's release number, a two-byte word the
/// picture_data census reports (Blorb: Release Number Chunk).
const RELEASE_ID: [u8; 4] = *b"RelN";
const RELEASE_SIZE: usize = 2;

/// The resolution chunk: six words of standard, minimum, and
/// maximum window sizes, then 28-byte entries of a picture number
/// and its three scaling ratios (Blorb: The Resolution Chunk).
const RESOLUTION_ID: [u8; 4] = *b"Reso";
const RESOLUTION_HEADER: usize = 24;
const RESOLUTION_ENTRY: usize = 28;
const WORD_SIZE: usize = 4;

/// The adaptive palette chunk: four-byte picture numbers naming the
/// legacy Infocom chrome that wears the palette of whatever scene
/// was plotted last (Blorb: The Adaptive Palette Chunk).
const ADAPTIVE_ID: [u8; 4] = *b"APal";

/// The baked-palette chunk, Bocfel's extension to the adaptive
/// dance: 12-byte records of a scene picture, an adaptive picture,
/// and the replacement picture the packager pre-dressed in that
/// scene's palette -- plotted in the adaptive picture's stead
/// (Bocfel: The Bocfel Adaptive Palette Chunk).
const BAKED_ID: [u8; 4] = *b"BPal";
const BAKED_ENTRY_SIZE: usize = 12;

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
#[derive(Debug, Clone)]
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
    /// The resource file's release number, 0 without a RelN chunk
    /// (Blorb: Release Number Chunk).
    pub release: u16,
    /// The Reso chunk's scaling instructions, or None without one
    /// -- every picture non-scalable (Blorb: The Resolution
    /// Chunk).
    pub resolution: Option<Resolution>,
    /// The pictures whose palettes adapt to the scene plotted
    /// before them; empty without an APal chunk (Blorb: The
    /// Adaptive Palette Chunk).
    pub adaptive: HashSet<u32>,
    /// Each (scene, adaptive) pair's pre-dressed replacement
    /// picture; empty without a BPal chunk (Bocfel: The Bocfel
    /// Adaptive Palette Chunk).
    pub baked: HashMap<(u32, u32), u32>,
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
            release: release(&chunks)?,
            resolution: resolution(&chunks)?,
            adaptive: adaptive(&chunks)?,
            baked: baked(&chunks)?,
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

    /// The picture to show before play, when one presents itself.
    ///
    /// The Fspc chunk names it outright (Blorb: Frontispiece
    /// Chunk). Failing that, a resource file carrying exactly one
    /// picture offers that picture -- Beyond Zork ships its splash
    /// so -- while the big Version 6 art sets, hundreds of scene
    /// pictures with no Fspc, offer nothing rather than a guess.
    pub fn cover(&self) -> Option<&Resource> {
        if let Some(number) = self.frontispiece {
            return self.resource(USAGE_PICTURE, number);
        }

        let mut pictures = self
            .resources
            .iter()
            .filter(|piece| piece.usage == USAGE_PICTURE);
        let lone = pictures.next();

        if pictures.next().is_none() {
            lone
        } else {
            None
        }
    }

    /// The Version 6 art as a gallery: sizes eager, pixels lazy.
    ///
    /// PNG pictures and Rect placeholders make the census; a JPEG
    /// -- no Infocom Version 6 set carries one -- is left out,
    /// because a picture Voxam cannot draw is not "available" in
    /// picture_data's sense (§15). Fails if a Rect does not hold
    /// its eight width-and-height bytes.
    pub fn gallery(&self) -> Result<Gallery, VoxamError> {
        let mut art = std::collections::BTreeMap::new();

        for piece in &self.resources {
            if piece.usage != USAGE_PICTURE {
                continue;
            }

            if piece.chunk.chunk_id == PNG_ID {
                art.insert(piece.number, Art::Png(piece.chunk.payload.clone()));
            } else if piece.chunk.chunk_id == RECT_ID {
                art.insert(piece.number, Art::Placard(placard(piece)?));
            }
        }

        Ok(Gallery::new(
            art,
            self.release,
            self.resolution.clone(),
            self.adaptive.clone(),
            self.baked.clone(),
        ))
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
/// A Rect resource's size, made a placard; the payload must be
/// the eight bytes of a width word and a height word (Blorb:
/// Picture Resource Chunks).
fn placard(piece: &Resource) -> Result<Placard, VoxamError> {
    let payload = &piece.chunk.payload;

    if payload.len() != RECT_SIZE {
        return Err(blorb_error(format!(
            "picture {} is a Rect of {} bytes, not the eight of a width and height \
             (Blorb: Picture Resource Chunks)",
            piece.number,
            payload.len()
        )));
    }

    Ok(Placard {
        width: worded(payload, 0),
        height: worded(payload, WORD_SIZE),
    })
}

/// One big-endian word at a byte offset the caller has sized for.
fn worded(payload: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([
        payload[at],
        payload[at + 1],
        payload[at + 2],
        payload[at + 3],
    ])
}

/// The release number, from at most one RelN chunk; a doubled
/// RelN, or one that is not a two-byte word, is refused (Blorb:
/// Release Number Chunk).
fn release(chunks: &[IffChunk]) -> Result<u16, VoxamError> {
    let found: Vec<&IffChunk> = chunks
        .iter()
        .filter(|piece| piece.chunk_id == RELEASE_ID)
        .collect();

    let Some(first) = found.first() else {
        return Ok(0);
    };

    if found.len() > 1 {
        return Err(blorb_error(format!(
            "{} RelN chunks appear, but there may not be more than one (Blorb: \
             Release Number Chunk)",
            found.len()
        )));
    }

    if first.payload.len() != RELEASE_SIZE {
        return Err(blorb_error(
            "the RelN chunk does not hold its two release-number bytes".to_string(),
        ));
    }

    Ok(u16::from_be_bytes([first.payload[0], first.payload[1]]))
}

/// The adaptive picture numbers, from at most one APal chunk; a
/// doubled APal, or one whose length is not a whole number of
/// four-byte entries, is refused (Blorb: The Adaptive Palette
/// Chunk).
fn adaptive(chunks: &[IffChunk]) -> Result<HashSet<u32>, VoxamError> {
    let found: Vec<&IffChunk> = chunks
        .iter()
        .filter(|piece| piece.chunk_id == ADAPTIVE_ID)
        .collect();

    let Some(first) = found.first() else {
        return Ok(HashSet::new());
    };

    if found.len() > 1 {
        return Err(blorb_error(format!(
            "{} APal chunks appear, but there may not be more than one (Blorb: The \
             Adaptive Palette Chunk)",
            found.len()
        )));
    }

    let payload = &first.payload;

    if !payload.len().is_multiple_of(WORD_SIZE) {
        return Err(blorb_error(format!(
            "an APal chunk is four-byte picture numbers, but this one holds {} bytes \
             (Blorb: The Adaptive Palette Chunk)",
            payload.len()
        )));
    }

    Ok((0..payload.len())
        .step_by(WORD_SIZE)
        .map(|start| worded(payload, start))
        .collect())
}

/// The pre-dressed replacements, from at most one BPal chunk.
///
/// Each 12-byte record maps a scene picture and an adaptive
/// picture to the replacement whose palette the packager already
/// applied (Bocfel: The Bocfel Adaptive Palette Chunk). A doubled
/// BPal, or one whose length is not a whole number of records, is
/// refused.
fn baked(chunks: &[IffChunk]) -> Result<HashMap<(u32, u32), u32>, VoxamError> {
    let found: Vec<&IffChunk> = chunks
        .iter()
        .filter(|piece| piece.chunk_id == BAKED_ID)
        .collect();

    let Some(first) = found.first() else {
        return Ok(HashMap::new());
    };

    if found.len() > 1 {
        return Err(blorb_error(format!(
            "{} BPal chunks appear, but there may not be more than one (Bocfel: The \
             Bocfel Adaptive Palette Chunk)",
            found.len()
        )));
    }

    let payload = &first.payload;

    if !payload.len().is_multiple_of(BAKED_ENTRY_SIZE) {
        return Err(blorb_error(format!(
            "a BPal chunk is 12-byte records, but this one holds {} bytes (Bocfel: \
             The Bocfel Adaptive Palette Chunk)",
            payload.len()
        )));
    }

    let mut records = HashMap::new();

    for start in (0..payload.len()).step_by(BAKED_ENTRY_SIZE) {
        let scene = worded(payload, start);
        let adaptive = worded(payload, start + WORD_SIZE);
        let replacement = worded(payload, start + 2 * WORD_SIZE);

        records.insert((scene, adaptive), replacement);
    }

    Ok(records)
}

/// The scaling instructions, from at most one Reso chunk. Refused:
/// a doubled Reso; one whose length is not the six-word header
/// plus whole 28-byte entries; a zero standard window dimension;
/// or a half-zero ratio fraction, which the spec calls illegal
/// (Blorb: The Resolution Chunk).
fn resolution(chunks: &[IffChunk]) -> Result<Option<Resolution>, VoxamError> {
    let found: Vec<&IffChunk> = chunks
        .iter()
        .filter(|piece| piece.chunk_id == RESOLUTION_ID)
        .collect();

    let Some(first) = found.first() else {
        return Ok(None);
    };

    if found.len() > 1 {
        return Err(blorb_error(format!(
            "{} Reso chunks appear, but there may not be more than one (Blorb: The \
             Resolution Chunk)",
            found.len()
        )));
    }

    let payload = &first.payload;

    if payload.len() < RESOLUTION_HEADER
        || !(payload.len() - RESOLUTION_HEADER).is_multiple_of(RESOLUTION_ENTRY)
    {
        return Err(blorb_error(format!(
            "a Reso chunk is a 24-byte header and 28-byte entries, but this one holds \
             {} bytes (Blorb: The Resolution Chunk)",
            payload.len()
        )));
    }

    let width = worded(payload, 0);
    let height = worded(payload, WORD_SIZE);

    if width == 0 || height == 0 {
        return Err(blorb_error(format!(
            "the Reso standard window is {width} by {height}, but px and py must be \
             non-zero (Blorb: The Resolution Chunk)"
        )));
    }

    let mut scalings = std::collections::BTreeMap::new();

    for start in (RESOLUTION_HEADER..payload.len()).step_by(RESOLUTION_ENTRY) {
        let words: Vec<u32> = (0..RESOLUTION_ENTRY / WORD_SIZE)
            .map(|seat| worded(payload, start + seat * WORD_SIZE))
            .collect();
        let number = words[0];

        scalings.insert(
            number,
            Scaling {
                standard: standard_ratio(number, words[1], words[2])?,
                minimum: limit_ratio(number, words[3], words[4])?,
                maximum: limit_ratio(number, words[5], words[6])?,
            },
        );
    }

    Ok(Some(Resolution {
        width,
        height,
        scalings,
    }))
}

/// A picture's standard ratio, which has no zero form: only the
/// minimum and maximum ratios may be zero, and only whole (Blorb:
/// The Resolution Chunk).
fn standard_ratio(number: u32, numerator: u32, denominator: u32) -> Result<Ratio, VoxamError> {
    if denominator == 0 {
        return Err(blorb_error(format!(
            "picture {number}'s standard ratio divides by zero (Blorb: The Resolution \
             Chunk)"
        )));
    }

    Ok(Ratio::new(i64::from(numerator), i64::from(denominator)))
}

/// A minimum or maximum ratio; zero-over-zero means no limit, and
/// only half the fraction zero is what the spec calls illegal
/// (Blorb: The Resolution Chunk).
fn limit_ratio(number: u32, numerator: u32, denominator: u32) -> Result<Option<Ratio>, VoxamError> {
    if numerator == 0 && denominator == 0 {
        return Ok(None);
    }

    if numerator == 0 || denominator == 0 {
        return Err(blorb_error(format!(
            "picture {number} has a half-zero ratio of {numerator}/{denominator}, \
             which is illegal (Blorb: The Resolution Chunk)"
        )));
    }

    Ok(Some(Ratio::new(
        i64::from(numerator),
        i64::from(denominator),
    )))
}

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

    /// One entry for `built`: usage, number, chunk id, payload.
    type Hung = ([u8; 4], u32, [u8; 4], Vec<u8>);

    /// Assemble a Blorb whose index points at real chunk offsets,
    /// the reference battery's `build_blorb`.
    fn built(entries: &[Hung], extra: &[IffChunk]) -> Vec<u8> {
        let mut index = (entries.len() as u32).to_be_bytes().to_vec();
        let mut position = 12 + 8 + 4 + entries.len() * 12;
        let mut pieces: Vec<IffChunk> = Vec::new();

        for (usage, number, chunk_id, payload) in entries {
            index.extend_from_slice(usage);
            index.extend_from_slice(&number.to_be_bytes());
            index.extend_from_slice(&(position as u32).to_be_bytes());

            position += 8 + payload.len() + payload.len() % 2;

            pieces.push(IffChunk {
                chunk_id: *chunk_id,
                payload: payload.clone(),
                offset: 0,
            });
        }

        let mut chunks = vec![IffChunk {
            chunk_id: INDEX_ID,
            payload: index,
            offset: 0,
        }];

        chunks.extend(pieces);
        chunks.extend_from_slice(extra);

        write_form(b"IFRS", &chunks)
    }

    fn reso_chunk(entries: &[u8], px: u32, py: u32) -> IffChunk {
        let mut header = Vec::new();

        for word in [px, py, 0, 0, 0, 0] {
            header.extend_from_slice(&word.to_be_bytes());
        }

        header.extend_from_slice(entries);

        optional(*b"Reso", header)
    }

    fn reso_entry(words: [u32; 7]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    fn parted(data: &[u8]) -> String {
        Blorb::parse(data).unwrap_err().to_string()
    }

    // The gallery hangs the drawable art: PNG bytes by number, Rect
    // placeholders as placards -- width word then height word --
    // and a JPEG left out, since a picture Voxam cannot draw is not
    // "available" in picture_data's sense (§15). The RelN release
    // number rides along for the census.
    #[test]
    fn the_gallery_hangs_drawable_art() {
        let mut rect = 314u32.to_be_bytes().to_vec();

        rect.extend_from_slice(&84u32.to_be_bytes());

        let data = built(
            &[
                (*b"Pict", 1, *b"PNG ", b"png-bytes".to_vec()),
                (*b"Pict", 2, *b"Rect", rect),
                (*b"Pict", 3, *b"JPEG", b"jpeg-bytes".to_vec()),
                (*b"Snd ", 4, *b"FORM", b"AIFFnoise".to_vec()),
            ],
            &[optional(*b"RelN", 27u16.to_be_bytes().to_vec())],
        );
        let blorb = Blorb::parse(&data).unwrap();

        assert_eq!(blorb.release, 27);

        let gallery = blorb.gallery().expect("hangs");

        assert_eq!(gallery.count(), 2);
        assert_eq!(gallery.size(2).expect("measures"), Some((84, 314)));
        assert_eq!(gallery.release, 27);
    }

    // A Blorb without a RelN releases 0; doubled or short RelN
    // chunks are refused, as is a Rect without its eight
    // width-and-height bytes (Blorb: Release Number Chunk, Picture
    // Resource Chunks).
    #[test]
    fn release_and_rect_chunks_are_policed() {
        assert_eq!(Blorb::parse(&built(&[], &[])).unwrap().release, 0);

        let release = optional(*b"RelN", 27u16.to_be_bytes().to_vec());

        assert!(parted(&built(&[], &[release.clone(), release.clone()])).contains("more than one"));
        assert!(
            parted(&built(&[], &[optional(*b"RelN", vec![0x1B])]))
                .contains("two release-number bytes")
        );

        let stubby = built(&[(*b"Pict", 5, *b"Rect", vec![0, 0])], &[]);

        assert!(
            Blorb::parse(&stubby)
                .unwrap()
                .gallery()
                .expect_err("refused")
                .to_string()
                .contains("Rect of 2 bytes")
        );
    }

    // The Reso chunk's scaling instructions reach the gallery: on a
    // 640-by-400 screen against a 320-by-200 standard window the
    // Elbow Room Factor is 2, multiplied by each listed picture's
    // standard ratio and clamped by its limits; unlisted pictures
    // stay at 1 (Blorb: The Resolution Chunk).
    #[test]
    fn the_resolution_chunk_reaches_the_gallery() {
        let mut entries = reso_entry([1, 2, 1, 0, 0, 0, 0]);

        entries.extend(reso_entry([2, 1, 1, 3, 1, 0, 0]));
        entries.extend(reso_entry([3, 10, 1, 0, 0, 3, 1]));

        let data = built(
            &[(*b"Pict", 1, *b"PNG ", b"png-bytes".to_vec())],
            &[reso_chunk(&entries, 320, 200)],
        );
        let gallery = Blorb::parse(&data).unwrap().gallery().expect("hangs");

        assert_eq!(gallery.scale(1, 640, 400), Ratio::new(4, 1));
        assert_eq!(gallery.scale(2, 640, 400), Ratio::new(3, 1));
        assert_eq!(gallery.scale(3, 640, 400), Ratio::new(3, 1));
        assert_eq!(gallery.scale(9, 640, 400), Ratio::ONE);
    }

    // The APal chunk names the adaptive pictures -- Infocom's
    // chrome, which wears the palette of the scene plotted before
    // it -- and is policed for doubling and ragged lengths (Blorb:
    // The Adaptive Palette Chunk).
    #[test]
    fn the_adaptive_chunk_is_read_and_policed() {
        let mut numbers = 54u32.to_be_bytes().to_vec();

        numbers.extend_from_slice(&170u32.to_be_bytes());

        let held = optional(*b"APal", numbers);
        let blorb = Blorb::parse(&built(&[], std::slice::from_ref(&held))).unwrap();

        assert_eq!(blorb.adaptive, HashSet::from([54, 170]));
        assert!(Blorb::parse(&built(&[], &[])).unwrap().adaptive.is_empty());
        assert!(parted(&built(&[], &[held.clone(), held.clone()])).contains("more than one"));
        assert!(
            parted(&built(&[], &[optional(*b"APal", vec![0, 1])]))
                .contains("four-byte picture numbers")
        );
    }

    // The BPal chunk maps each (scene, adaptive) pair to the
    // replacement picture the packager pre-dressed in that scene's
    // palette, and is policed for doubling and ragged lengths
    // (Bocfel: The Bocfel Adaptive Palette Chunk).
    #[test]
    fn the_baked_chunk_is_read_and_policed() {
        let record = |scene: u32, adaptive: u32, replacement: u32| -> Vec<u8> {
            [scene, adaptive, replacement]
                .iter()
                .flat_map(|word| word.to_be_bytes())
                .collect()
        };

        let mut records = record(1, 9, 1000);

        records.extend(record(2, 9, 1001));

        let held = optional(*b"BPal", records);
        let blorb = Blorb::parse(&built(&[], std::slice::from_ref(&held))).unwrap();

        assert_eq!(blorb.baked, HashMap::from([((1, 9), 1000), ((2, 9), 1001)]));
        assert!(Blorb::parse(&built(&[], &[])).unwrap().baked.is_empty());
        assert!(parted(&built(&[], &[held.clone(), held.clone()])).contains("more than one"));
        assert!(parted(&built(&[], &[optional(*b"BPal", vec![0, 1])])).contains("12-byte records"));
    }

    // Reso chunks are policed: doubled chunks, ragged lengths, a
    // zero standard window, a zero standard denominator, and a
    // half-zero limit fraction are each refused; a Blorb without
    // one simply has no scaling (Blorb: The Resolution Chunk).
    #[test]
    fn resolution_chunks_are_policed() {
        assert!(Blorb::parse(&built(&[], &[])).unwrap().resolution.is_none());

        let plain = reso_chunk(&[], 320, 200);

        assert!(parted(&built(&[], &[plain.clone(), plain.clone()])).contains("more than one"));
        assert!(parted(&built(&[], &[optional(*b"Reso", vec![0; 10])])).contains("24-byte header"));
        assert!(parted(&built(&[], &[optional(*b"Reso", vec![0; 30])])).contains("24-byte header"));
        assert!(parted(&built(&[], &[reso_chunk(&[], 0, 200)])).contains("must be non-zero"));
        assert!(
            parted(&built(
                &[],
                &[reso_chunk(&reso_entry([1, 1, 0, 0, 0, 0, 0]), 320, 200)]
            ))
            .contains("divides by zero")
        );
        assert!(
            parted(&built(
                &[],
                &[reso_chunk(&reso_entry([1, 1, 1, 1, 0, 0, 0]), 320, 200)]
            ))
            .contains("half-zero")
        );
    }

    // The cover is the Fspc picture when one is named; failing
    // that, a resource file carrying exactly one picture offers
    // that picture -- Beyond Zork ships its splash so -- while
    // bigger art sets offer nothing rather than a guess (Blorb:
    // Frontispiece Chunk).
    #[test]
    fn the_cover_is_the_frontispiece_or_the_lone_picture() {
        let payload = |resource: Option<&Resource>| -> Vec<u8> {
            resource
                .map(|held| held.chunk.payload.clone())
                .unwrap_or_default()
        };

        let named = Blorb::parse(&built(
            &[
                (*b"Pict", 1, *b"PNG ", b"one".to_vec()),
                (*b"Pict", 2, *b"PNG ", b"two".to_vec()),
            ],
            &[optional(*b"Fspc", 2u32.to_be_bytes().to_vec())],
        ))
        .unwrap();

        assert_eq!(payload(named.cover()), b"two".to_vec());

        let lone = Blorb::parse(&built(&[(*b"Pict", 5, *b"PNG ", b"solo".to_vec())], &[])).unwrap();

        assert_eq!(payload(lone.cover()), b"solo".to_vec());

        let crowd = Blorb::parse(&built(
            &[
                (*b"Pict", 1, *b"PNG ", b"one".to_vec()),
                (*b"Pict", 2, *b"PNG ", b"two".to_vec()),
            ],
            &[],
        ))
        .unwrap();

        assert!(crowd.cover().is_none());
        assert!(Blorb::parse(&built(&[], &[])).unwrap().cover().is_none());
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
