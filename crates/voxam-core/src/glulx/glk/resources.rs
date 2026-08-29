//! Glk's view of the Blorb resources: pictures, sounds, data
//! files.
//!
//! voxam's blorb module reads the container; this decides what its
//! contents mean to Glk -- the pictures glk_image_draw names (Glk:
//! Graphics), the sounds a channel plays (Glk: Sound Resources),
//! and the data chunks a resource stream opens over (Glk: Resource
//! Streams). The split matters because the interpreter needs the
//! same container to find the executable chunk before any of this
//! exists.
//!
//! Image sizes are read out of the picture bytes here rather than
//! asked of the display, because glk_image_get_info must answer
//! even when nothing can be drawn -- a game may lay out a window
//! from the dimensions and then discover it has no graphics (Glk:
//! Testing for Graphics Capabilities). The reference's data-url
//! renderings (pictured, audible) belong to the wire displays and
//! arrive with that era.

use std::collections::HashMap;

use crate::blorb::{Blorb, USAGE_DATA, USAGE_PICTURE, USAGE_SOUND};
use crate::iff::chunk;

const FORM: [u8; 4] = *b"FORM";
const TEXT: [u8; 4] = *b"TEXT";

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const PNG_HEADER_END: usize = 24;
const IHDR_WIDTH_AT: usize = 16;
const IHDR_HEIGHT_AT: usize = 20;

const JPEG_SIGNATURE: &[u8] = b"\xff\xd8";
const MARKER: u8 = 0xFF;
const STANDALONE_LOW: u8 = 0xD0;
const STANDALONE_HIGH: u8 = 0xD7;
const START_OF_IMAGE: u8 = 0xD8;
const TEMPORARY: u8 = 0x01;
const SOF_NEED: usize = 9;
const SOF_HEIGHT_AT: usize = 5;
const SOF_WIDTH_AT: usize = 7;

/// The start-of-frame markers carry the image dimensions. C4, C8,
/// and CC sit in the SOF numbering but are not SOFs: they are the
/// Huffman table, JPEG extension, and arithmetic coding markers.
fn is_sof(marker: u8) -> bool {
    (0xC0..0xD0).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC)
}

/// One picture resource, measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    /// The number the game asks for it by.
    pub number: u32,
    /// The Blorb chunk type: PNG followed by a space, or JPEG
    /// (Blorb: Picture Resource Chunks).
    pub kind: [u8; 4],
    /// The picture bytes, ready for a display to decode.
    pub data: Vec<u8>,
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
}

fn word_at(data: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn short_at(data: &[u8], at: usize) -> u32 {
    u32::from(u16::from_be_bytes([data[at], data[at + 1]]))
}

/// Read the pixel dimensions of a PNG or a JPEG.
///
/// PNG requires the IHDR chunk to come first, so width and height
/// sit at fixed offsets; a JPEG hides them in a start-of-frame
/// segment that has to be walked to.
pub fn image_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.starts_with(PNG_SIGNATURE) && data.len() >= PNG_HEADER_END {
        return Some((word_at(data, IHDR_WIDTH_AT), word_at(data, IHDR_HEIGHT_AT)));
    }

    if data.starts_with(JPEG_SIGNATURE) {
        return jpeg_size(data);
    }

    None
}

/// Walk JPEG segments until a start-of-frame marker turns up.
fn jpeg_size(data: &[u8]) -> Option<(u32, u32)> {
    let mut position = 2;
    let end = data.len();

    while position + 4 <= end {
        if data[position] != MARKER {
            return None;
        }

        let marker = data[position + 1];

        if marker == START_OF_IMAGE
            || marker == TEMPORARY
            || (STANDALONE_LOW..=STANDALONE_HIGH).contains(&marker)
        {
            // Standalone markers carry no length word.
            position += 2;

            continue;
        }

        let length = short_at(data, position + 2) as usize;

        if is_sof(marker) {
            if position + SOF_NEED > end {
                return None;
            }

            return Some((
                short_at(data, position + SOF_WIDTH_AT),
                short_at(data, position + SOF_HEIGHT_AT),
            ));
        }

        position += 2 + length;
    }

    None
}

/// The pictures, sounds, and data available to a game.
///
/// An instance with no Blorb behind it answers "nothing here" to
/// everything, which is the right answer for a bare .ulx story.
#[derive(Debug, Default)]
pub struct Resources {
    /// The container, or None without one.
    pub blorb: Option<Blorb>,
    images: HashMap<u32, Option<ImageInfo>>,
}

impl Resources {
    /// Stand in front of a container.
    pub fn new(blorb: Option<Blorb>) -> Self {
        Self {
            blorb,
            images: HashMap::new(),
        }
    }

    /// Look up a picture, measuring it on first use.
    ///
    /// A picture whose dimensions cannot be read answers None: a
    /// size glk_image_get_info cannot report is a picture the game
    /// cannot lay out (Glk: Graphics).
    pub fn image(&mut self, number: u32) -> Option<&ImageInfo> {
        if !self.images.contains_key(&number) {
            let found = self
                .blorb
                .as_ref()
                .and_then(|blorb| blorb.resource(USAGE_PICTURE, number))
                .and_then(|found| {
                    let data = &found.chunk.payload;

                    image_size(data).map(|(width, height)| ImageInfo {
                        number,
                        kind: found.chunk.chunk_id,
                        data: data.clone(),
                        width,
                        height,
                    })
                });

            self.images.insert(number, found);
        }

        self.images[&number].as_ref()
    }

    /// Return a sound resource's bytes, or None if absent.
    ///
    /// AIFF sounds are stored as FORM chunks, and an AIFF file
    /// *is* that FORM -- header included (Blorb: Sound Resource
    /// Chunks). Handing an audio decoder the body alone would give
    /// it a file starting at "AIFF" with no container.
    pub fn sound(&self, number: u32) -> Option<Vec<u8>> {
        let found = self
            .blorb
            .as_ref()
            .and_then(|blorb| blorb.resource(USAGE_SOUND, number))?;

        Some(contents(&found.chunk.chunk_id, &found.chunk.payload))
    }

    /// Return a data resource as (bytes, is_text).
    ///
    /// Blorb marks a data chunk TEXT or BINA (Blorb: Data Resource
    /// Chunks); the distinction only matters when the resource is
    /// opened as a Unicode stream, where text means UTF-8 and
    /// binary means four-byte words (Glk: Resource Streams).
    pub fn data(&self, number: u32) -> Option<(Vec<u8>, bool)> {
        let found = self
            .blorb
            .as_ref()
            .and_then(|blorb| blorb.resource(USAGE_DATA, number))?;

        if found.chunk.chunk_id == FORM {
            return Some((contents(&FORM, &found.chunk.payload), false));
        }

        Some((found.chunk.payload.clone(), found.chunk.chunk_id == TEXT))
    }
}

/// A chunk's bytes: the whole thing for a FORM, the body else.
///
/// A FORM resource is a complete nested IFF file -- an AIFF sound,
/// or a data container -- so its header belongs to the contents.
/// Everything else (PNG, JPEG, TEXT, BINA) is raw payload.
fn contents(chunk_id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    if *chunk_id == FORM {
        return chunk(chunk_id, payload);
    }

    payload.to_vec()
}
