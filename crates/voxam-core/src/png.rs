//! Reading and writing PNG pictures with the crate's own hands.
//!
//! Blorb resource files carry their pictures as PNG (Blorb: Picture
//! Resource Chunks), and a cover picture is worth showing before
//! play -- but Voxam's core stays dependency-free, so the decoding
//! is done here by hand: chunk walking, inflation through the flate
//! module, scanline unfiltering, and pixel extraction, following
//! the PNG specification (ISO/IEC 15948). The encoder spells its
//! own deflate stream for the same reason the reference does:
//! zlib's compressed bytes vary by the library behind it -- madler
//! zlib and zlib-ng disagree -- and the wire these pictures ride is
//! certified byte for byte, so the encoded form must be the same on
//! every build (RFC 1951).
//!
//! The scope is the census of every picture in the vendored Infocom
//! resource files: palette images at bit depths 1 to 8, truecolour,
//! greyscale, and the alpha-bearing forms, none interlaced. Adam7
//! interlacing and 16-bit depths never appear there and are refused
//! with their names given.

use crate::errors::VoxamError;
use crate::flate::{crc32, deflated, inflated};

pub const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

// Chunk layout: a four-byte length, a four-byte name, the payload,
// and a CRC the reader has no reason to distrust.
const LENGTH_SIZE: usize = 4;
const NAME_SIZE: usize = 4;
const CRC_SIZE: usize = 4;

pub const IHDR: [u8; 4] = *b"IHDR";
pub const PLTE: [u8; 4] = *b"PLTE";
pub const TRNS: [u8; 4] = *b"tRNS";
pub const IDAT: [u8; 4] = *b"IDAT";
pub const IEND: [u8; 4] = *b"IEND";

// The colour types and their channel counts: greyscale, truecolour,
// palette indices, greyscale with alpha, truecolour with alpha.
const GREYSCALE: u8 = 0;
const TRUECOLOUR: u8 = 2;
const PALETTE: u8 = 3;
const GREY_ALPHA: u8 = 4;
const TRUE_ALPHA: u8 = 6;

// The bit depths each colour type allows here: the packed depths
// belong to greyscale and palette images, everything else is one
// full byte per channel.
const PACKED_DEPTHS: [u8; 4] = [1, 2, 4, 8];
const BYTE_DEPTH: u8 = 8;

// Scanline filter types (PNG 9.2): each line names how its bytes
// were predicted from the pixels to its left and above.
const FILTER_NONE: u8 = 0;
const FILTER_SUB: u8 = 1;
const FILTER_UP: u8 = 2;
const FILTER_AVERAGE: u8 = 3;
const FILTER_PAETH: u8 = 4;

const OPAQUE: u8 = 255;
const FULL_SCALE: u32 = 255;

/// One decoded pixel's channels.
pub type Pixel = (u8, u8, u8);

/// A decoded picture: rows of (red, green, blue) pixels.
///
/// With no alpha aboard the rows are composed over black -- the
/// terminal a cover picture is shown on; with alpha carried they
/// are the straight source colors, for a display that can truly
/// blend. `clear` marks which pixels are fully transparent, one
/// row of flags per row of pixels -- or None for a picture with no
/// transparency at all: Version 6 art layers its chrome with
/// see-through holes, and only full transparency matters there
/// (Blorb: Picture Resource Chunks). `alpha` carries per-pixel
/// opacity -- or None when no pixel is partially see-through, in
/// which case the clear flags already say everything transparency
/// has to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rows: Vec<Vec<Pixel>>,
    pub clear: Option<Vec<Vec<bool>>>,
    pub alpha: Option<Vec<Vec<u8>>>,
}

/// The walked ingredients: the header words, the palette, the tRNS
/// alphas, and the concatenated image data.
struct Walked {
    width: u32,
    height: u32,
    depth: u8,
    colour_type: u8,
    palette: Vec<Pixel>,
    alphas: Vec<u8>,
    compressed: Vec<u8>,
}

fn png_error(message: String) -> VoxamError {
    VoxamError::Png(message)
}

/// Decode PNG bytes into rows of RGB pixels.
///
/// `adapted` is a palette to plot with instead of the file's own
/// PLTE -- how an adaptive-palette picture wears the Current
/// Palette (Blorb: The Adaptive Palette Chunk). Transparency still
/// comes from the file's own tRNS.
pub fn decode(data: &[u8], adapted: Option<&[Pixel]>) -> Result<Picture, VoxamError> {
    if !data.starts_with(&SIGNATURE) {
        return Err(png_error(
            "the bytes do not begin with the PNG signature".to_string(),
        ));
    }

    let mut walked = walk(data)?;

    if let Some(adapted) = adapted {
        walked.palette = adapted.to_vec();
    }

    let width = walked.width as usize;
    let height = walked.height as usize;
    let bits = channels(walked.colour_type) * usize::from(walked.depth);
    let stride = (width * bits).div_ceil(8);
    let bytes_back = (bits / 8).max(1);

    let inflated = inflated(&walked.compressed)
        .map_err(|reason| png_error(format!("the image data does not inflate: {reason}")))?;

    if inflated.len() != height * (stride + 1) {
        return Err(png_error(format!(
            "a {}x{} image needs {} bytes of scanlines, but {} inflated",
            walked.width,
            walked.height,
            height * (stride + 1),
            inflated.len()
        )));
    }

    let lines = unfiltered(&inflated, height, stride, bytes_back)?;
    let translucent = translucent(walked.colour_type, &walked.alphas);
    let mut alpha = translucent.then(|| {
        lines
            .iter()
            .map(|line| {
                alpha_row(
                    line,
                    width,
                    walked.depth,
                    walked.colour_type,
                    &walked.alphas,
                )
            })
            .collect::<Vec<_>>()
    });

    if let Some(held) = &alpha
        && !held
            .iter()
            .flatten()
            .any(|&value| value > 0 && value < OPAQUE)
    {
        // Nothing is partially see-through: the clear flags say it
        // all, and the rows stay composed over black as ever, so a
        // picture of holes and solids decodes exactly as it always
        // has.
        alpha = None;
    }

    let rows = lines
        .iter()
        .map(|line| {
            if alpha.is_none() {
                pixels(
                    line,
                    width,
                    walked.depth,
                    walked.colour_type,
                    &walked.palette,
                    &walked.alphas,
                )
            } else {
                straight_pixels(
                    line,
                    width,
                    walked.depth,
                    walked.colour_type,
                    &walked.palette,
                )
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let clear = translucent.then(|| {
        lines
            .iter()
            .map(|line| {
                clear_row(
                    line,
                    width,
                    walked.depth,
                    walked.colour_type,
                    &walked.alphas,
                )
            })
            .collect::<Vec<_>>()
    });

    Ok(Picture {
        width: walked.width,
        height: walked.height,
        rows,
        clear,
        alpha,
    })
}

/// Encode a Picture back into PNG bytes, its palette long applied.
///
/// The write-side twin of decode, for art whose true colours only
/// exist after the adaptive-palette dance: a display handed an
/// adaptive stub's own bytes would paint the placeholder palette,
/// so the plotted pixels travel instead (Blorb: The Adaptive
/// Palette Chunk). Truecolour when every pixel is opaque,
/// truecolour with alpha when any is not; every scanline rides
/// unfiltered ahead of one hand-spelled zlib stream, so the bytes
/// never vary by build (PNG 9.2, 11.2.4 IDAT; RFC 1951).
pub fn encoded(picture: &Picture) -> Vec<u8> {
    let translucent = picture.clear.is_some() || picture.alpha.is_some();
    let colour_type = if translucent { TRUE_ALPHA } else { TRUECOLOUR };
    let mut lines: Vec<u8> = Vec::new();

    for row in 0..picture.height as usize {
        lines.push(FILTER_NONE);

        for column in 0..picture.width as usize {
            let (red, green, blue) = picture.rows[row][column];

            lines.extend_from_slice(&[red, green, blue]);

            if translucent {
                lines.push(opacity(picture, row, column));
            }
        }
    }

    // Width, height, one byte per channel, the colour type, and
    // the format's sole compression, filter, and interlace methods
    // -- all zero (PNG 11.2.1 IHDR).
    let mut header = Vec::with_capacity(13);

    header.extend_from_slice(&picture.width.to_be_bytes());
    header.extend_from_slice(&picture.height.to_be_bytes());
    header.extend_from_slice(&[BYTE_DEPTH, colour_type, 0, 0, 0]);

    let mut out = SIGNATURE.to_vec();

    out.extend_from_slice(&chunked(IHDR, &header));
    out.extend_from_slice(&chunked(IDAT, &deflated(&lines)));
    out.extend_from_slice(&chunked(IEND, &[]));

    out
}

/// One pixel's alpha: clear flags rule, alpha values refine.
fn opacity(picture: &Picture, row: usize, column: usize) -> u8 {
    if let Some(clear) = &picture.clear
        && clear[row][column]
    {
        return 0;
    }

    if let Some(alpha) = &picture.alpha {
        return alpha[row][column];
    }

    OPAQUE
}

/// One PNG chunk: length, name, payload, and its CRC (PNG 5.3).
fn chunked(name: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(LENGTH_SIZE + NAME_SIZE + payload.len() + CRC_SIZE);

    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&name);
    out.extend_from_slice(payload);

    let mut summed = crc32(&name, 0);

    summed = crc32(payload, summed);

    out.extend_from_slice(&summed.to_be_bytes());

    out
}

/// A PNG's own PLTE entries, empty for a palette-less picture.
///
/// What a plotted non-adaptive picture carries into the Current
/// Palette (Blorb: The Adaptive Palette Chunk).
pub fn palette(data: &[u8]) -> Result<Vec<Pixel>, VoxamError> {
    if !data.starts_with(&SIGNATURE) {
        return Err(png_error(
            "the bytes do not begin with the PNG signature".to_string(),
        ));
    }

    Ok(walk(data)?.palette)
}

/// Walk the chunks: header, palette, transparency, image data.
///
/// Chunks outside the reader's business -- gamma, text -- pass
/// unread, as the specification instructs for ancillary chunks.
fn walk(data: &[u8]) -> Result<Walked, VoxamError> {
    let mut header: Option<(u32, u32, u8, u8)> = None;
    let mut palette: Vec<Pixel> = Vec::new();
    let mut alphas: Vec<u8> = Vec::new();
    let mut compressed: Vec<u8> = Vec::new();
    let mut position = SIGNATURE.len();

    while position + LENGTH_SIZE + NAME_SIZE <= data.len() {
        let length = u32::from_be_bytes([
            data[position],
            data[position + 1],
            data[position + 2],
            data[position + 3],
        ]) as usize;
        let name = [
            data[position + LENGTH_SIZE],
            data[position + LENGTH_SIZE + 1],
            data[position + LENGTH_SIZE + 2],
            data[position + LENGTH_SIZE + 3],
        ];
        let start = position + LENGTH_SIZE + NAME_SIZE;
        let payload = &data[start.min(data.len())..(start + length).min(data.len())];

        position = start + length + CRC_SIZE;

        if payload.len() < length {
            return Err(png_error(format!(
                "the {} chunk is cut short",
                name.iter()
                    .map(|&held| char::from(held))
                    .collect::<String>()
            )));
        }

        if name == IHDR {
            header = Some(read_header(payload)?);
        } else if name == PLTE {
            palette = payload
                .as_chunks::<3>()
                .0
                .iter()
                .map(|triple| (triple[0], triple[1], triple[2]))
                .collect();
        } else if name == TRNS {
            alphas = payload.to_vec();
        } else if name == IDAT {
            compressed.extend_from_slice(payload);
        } else if name == IEND {
            break;
        }
    }

    let Some((width, height, depth, colour_type)) = header else {
        return Err(png_error(
            "the picture has no IHDR header chunk".to_string(),
        ));
    };

    if colour_type == PALETTE && palette.is_empty() {
        return Err(png_error(
            "a palette picture arrived without its PLTE chunk".to_string(),
        ));
    }

    Ok(Walked {
        width,
        height,
        depth,
        colour_type,
        palette,
        alphas,
        compressed,
    })
}

/// Read IHDR, refusing what the census says never appears.
fn read_header(payload: &[u8]) -> Result<(u32, u32, u8, u8), VoxamError> {
    if payload.len() != 13 {
        return Err(png_error(format!(
            "the IHDR chunk is malformed: {} bytes stand where thirteen belong",
            payload.len()
        )));
    }

    let width = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let height = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
    let depth = payload[8];
    let colour_type = payload[9];
    let interlace = payload[12];

    if interlace != 0 {
        return Err(png_error(
            "Adam7 interlaced pictures are not supported".to_string(),
        ));
    }

    if width == 0 || height == 0 {
        return Err(png_error("the picture has no pixels".to_string()));
    }

    let supported = if colour_type == GREYSCALE || colour_type == PALETTE {
        PACKED_DEPTHS.contains(&depth)
    } else {
        depth == BYTE_DEPTH
            && (colour_type == TRUECOLOUR || colour_type == GREY_ALPHA || colour_type == TRUE_ALPHA)
    };

    if !supported {
        return Err(png_error(format!(
            "colour type {colour_type} at bit depth {depth} is not a supported pairing"
        )));
    }

    Ok((width, height, depth, colour_type))
}

/// A colour type's channel count; the header already refused any
/// type outside the five the format defines.
fn channels(colour_type: u8) -> usize {
    match colour_type {
        TRUECOLOUR => 3,
        GREY_ALPHA => 2,
        TRUE_ALPHA => 4,
        _ => 1,
    }
}

/// Undo the scanline filters (PNG 9.2).
///
/// Each line opens with a filter byte naming how its bytes were
/// predicted -- from the byte one pixel left, the byte above, their
/// average, or Paeth's choice among them -- and reconstruction adds
/// the prediction back, line by line.
fn unfiltered(
    data: &[u8],
    height: usize,
    stride: usize,
    back: usize,
) -> Result<Vec<Vec<u8>>, VoxamError> {
    let mut lines: Vec<Vec<u8>> = Vec::with_capacity(height);
    let mut previous = vec![0u8; stride];
    let mut position = 0;

    for _ in 0..height {
        let filter_type = data[position];
        let mut line = data[position + 1..position + 1 + stride].to_vec();

        position += 1 + stride;

        match filter_type {
            FILTER_SUB => {
                for index in back..stride {
                    line[index] = line[index].wrapping_add(line[index - back]);
                }
            }
            FILTER_UP => {
                for index in 0..stride {
                    line[index] = line[index].wrapping_add(previous[index]);
                }
            }
            FILTER_AVERAGE => {
                for index in 0..stride {
                    let left = if index >= back {
                        u32::from(line[index - back])
                    } else {
                        0
                    };
                    let mean = ((left + u32::from(previous[index])) / 2) as u8;

                    line[index] = line[index].wrapping_add(mean);
                }
            }
            FILTER_PAETH => {
                for index in 0..stride {
                    let left = if index >= back { line[index - back] } else { 0 };
                    let above = previous[index];
                    let corner = if index >= back {
                        previous[index - back]
                    } else {
                        0
                    };

                    line[index] = line[index].wrapping_add(paeth(left, above, corner));
                }
            }
            FILTER_NONE => {}
            _ => {
                return Err(png_error(format!(
                    "scanline filter type {filter_type} is not defined"
                )));
            }
        }

        previous.clone_from(&line);

        lines.push(line);
    }

    Ok(lines)
}

/// Paeth's predictor: whichever neighbour is nearest the guess.
fn paeth(left: u8, above: u8, corner: u8) -> u8 {
    let guess = i32::from(left) + i32::from(above) - i32::from(corner);
    let to_left = (guess - i32::from(left)).abs();
    let to_above = (guess - i32::from(above)).abs();
    let to_corner = (guess - i32::from(corner)).abs();

    if to_left <= to_above && to_left <= to_corner {
        return left;
    }

    if to_above <= to_corner {
        return above;
    }

    corner
}

/// Turn one unfiltered scanline into RGB triples.
fn pixels(
    line: &[u8],
    width: usize,
    depth: u8,
    colour_type: u8,
    palette: &[Pixel],
    alphas: &[u8],
) -> Result<Vec<Pixel>, VoxamError> {
    if colour_type == TRUECOLOUR {
        return Ok((0..width)
            .map(|index| (line[3 * index], line[3 * index + 1], line[3 * index + 2]))
            .collect());
    }

    if colour_type == TRUE_ALPHA {
        return Ok((0..width)
            .map(|index| {
                over_black(
                    [line[4 * index], line[4 * index + 1], line[4 * index + 2]],
                    line[4 * index + 3],
                )
            })
            .collect());
    }

    if colour_type == GREY_ALPHA {
        return Ok((0..width)
            .map(|index| {
                let grey = line[2 * index];

                over_black([grey, grey, grey], line[2 * index + 1])
            })
            .collect());
    }

    let values = unpacked(line, width, depth);

    if colour_type == GREYSCALE {
        let full = (1u32 << depth) - 1;

        return Ok(values
            .iter()
            .map(|&value| {
                let scaled = (u32::from(value) * FULL_SCALE / full) as u8;

                (scaled, scaled, scaled)
            })
            .collect());
    }

    values
        .iter()
        .map(|&value| from_palette(usize::from(value), palette, alphas))
        .collect()
}

/// Read width values of depth bits each, most significant first.
fn unpacked(line: &[u8], width: usize, depth: u8) -> Vec<u8> {
    if depth == BYTE_DEPTH {
        return line[..width].to_vec();
    }

    let depth = usize::from(depth);
    let per_byte = 8 / depth;
    let mask = (1u8 << depth) - 1;

    (0..width)
        .map(|index| (line[index / per_byte] >> (8 - depth * (index % per_byte + 1))) & mask)
        .collect()
}

/// One palette entry, composed over black where tRNS says so.
fn from_palette(index: usize, palette: &[Pixel], alphas: &[u8]) -> Result<Pixel, VoxamError> {
    let Some(&(red, green, blue)) = palette.get(index) else {
        return Err(png_error(format!(
            "pixel index {index} points beyond the {}-entry palette",
            palette.len()
        )));
    };

    let alpha = alphas.get(index).copied().unwrap_or(OPAQUE);

    Ok(over_black([red, green, blue], alpha))
}

/// Compose one pixel over black, the screen a cover shows on.
fn over_black(channels: [u8; 3], alpha: u8) -> Pixel {
    let composed = |value: u8| (u32::from(value) * u32::from(alpha) / u32::from(OPAQUE)) as u8;

    (
        composed(channels[0]),
        composed(channels[1]),
        composed(channels[2]),
    )
}

/// One scanline's source colors, uncomposed.
///
/// Only the alpha-bearing color types arrive here: a picture with
/// partial alpha keeps its straight colors, and a display that can
/// truly blend does the composing itself.
fn straight_pixels(
    line: &[u8],
    width: usize,
    depth: u8,
    colour_type: u8,
    palette: &[Pixel],
) -> Result<Vec<Pixel>, VoxamError> {
    if colour_type == TRUE_ALPHA {
        return Ok((0..width)
            .map(|index| (line[4 * index], line[4 * index + 1], line[4 * index + 2]))
            .collect());
    }

    if colour_type == GREY_ALPHA {
        return Ok((0..width)
            .map(|index| {
                let grey = line[2 * index];

                (grey, grey, grey)
            })
            .collect());
    }

    // Composing over black at full opacity is the identity, so the
    // palette path reuses from_palette for its bounds check alone.
    unpacked(line, width, depth)
        .iter()
        .map(|&value| from_palette(usize::from(value), palette, &[]))
        .collect()
}

/// One scanline's opacity, pixel by pixel.
fn alpha_row(line: &[u8], width: usize, depth: u8, colour_type: u8, alphas: &[u8]) -> Vec<u8> {
    if colour_type == TRUE_ALPHA {
        return (0..width).map(|index| line[4 * index + 3]).collect();
    }

    if colour_type == GREY_ALPHA {
        return (0..width).map(|index| line[2 * index + 1]).collect();
    }

    unpacked(line, width, depth)
        .iter()
        .map(|&index| alphas.get(usize::from(index)).copied().unwrap_or(OPAQUE))
        .collect()
}

/// Whether this picture can hold transparency at all.
fn translucent(colour_type: u8, alphas: &[u8]) -> bool {
    colour_type == TRUE_ALPHA
        || colour_type == GREY_ALPHA
        || (colour_type == PALETTE && !alphas.is_empty())
}

/// One scanline's fully-transparent flags, pixel by pixel.
fn clear_row(line: &[u8], width: usize, depth: u8, colour_type: u8, alphas: &[u8]) -> Vec<bool> {
    if colour_type == TRUE_ALPHA {
        return (0..width).map(|index| line[4 * index + 3] == 0).collect();
    }

    if colour_type == GREY_ALPHA {
        return (0..width).map(|index| line[2 * index + 1] == 0).collect();
    }

    unpacked(line, width, depth)
        .iter()
        .map(|&index| alphas.get(usize::from(index)) == Some(&0))
        .collect()
}
