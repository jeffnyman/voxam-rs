//! Encoding pictures as DEC sixel graphics.
//!
//! Sixel is the terminal's own pixel protocol: an escape sequence
//! carrying a palette and columns of six vertical pixels at a
//! time, drawn at the cursor in real pixels rather than character
//! cells. Windows Terminal speaks it from release 1.22, and
//! through it a cover picture shows at its true resolution -- the
//! same image a graphical interpreter would draw.
//!
//! The encoder is pure like the PNG reader it feeds on: sixel
//! palettes hold at most 256 colours, which suits Infocom's
//! palette art natively; richer pictures posterize down first.

use std::collections::BTreeMap;

use voxam_core::png::Picture;

/// A palette of sixel registers and the picture's rows spelled as
/// register numbers.
type Indexed = (Vec<(u8, u8, u8)>, Vec<Vec<usize>>);

pub const ENTER: &str = "\u{1b}Pq";
pub const LEAVE: &str = "\u{1b}\\";

// Sixel data characters carry six vertical pixels as a bitmask on
// top of an offset; ! introduces a run length, $ returns to the
// left edge for the band's next colour, - moves down one band.
const OFFSET: u32 = 0x3F;
const BAND: usize = 6;
const RUN_WORTHWHILE: usize = 3;

// A sixel palette holds registers 0 to 255, defined in
// percentages; pictures with more distinct colours posterize each
// channel to six levels first, which no cover in the vendored art
// needs.
const PALETTE_LIMIT: usize = 256;
const POSTERIZE_STEP: u8 = 51;
const PERCENT: u32 = 100;
const FULL: u32 = 255;

// Sizing sixel pixels against a glass measured only in cells: no
// terminal cell is narrower than 8 pixels or shorter than 16 on a
// modern display, so scaling against these floors magnifies a
// cover as far as it can certainly fit when the terminal will not
// say more.
pub const CELL_WIDTH_FLOOR: usize = 8;
pub const CELL_HEIGHT_FLOOR: usize = 16;

/// Encode a picture as a sixel sequence, integer-scaled up:
/// sixel pixels are screen pixels, so a small original is
/// enlarged to be seen at all. The answer is the complete escape
/// sequence, enter to leave.
pub fn encode(picture: &Picture, scale: usize) -> String {
    let (palette, indices) = indexed(picture);
    let width = picture.width as usize * scale;
    let height = picture.height as usize * scale;
    let mut pieces = vec![ENTER.to_string(), format!("\"1;1;{width};{height}")];

    for (register, (red, green, blue)) in palette.iter().enumerate() {
        pieces.push(format!(
            "#{register};2;{};{};{}",
            u32::from(*red) * PERCENT / FULL,
            u32::from(*green) * PERCENT / FULL,
            u32::from(*blue) * PERCENT / FULL
        ));
    }

    let mut band_top = 0;

    while band_top < height {
        let rows: Vec<&Vec<usize>> = (0..BAND.min(height - band_top))
            .map(|drop| &indices[(band_top + drop) / scale])
            .collect();
        let mut present: Vec<usize> = rows
            .iter()
            .flat_map(|row| row.iter().copied())
            .collect::<std::collections::BTreeSet<usize>>()
            .into_iter()
            .collect();

        present.sort_unstable();

        for (position, register) in present.iter().enumerate() {
            if position > 0 {
                pieces.push("$".to_string());
            }

            pieces.push(format!("#{register}"));
            pieces.push(band_run(&rows, *register, width, scale));
        }

        pieces.push("-".to_string());
        band_top += BAND;
    }

    pieces.push(LEAVE.to_string());

    pieces.concat()
}

/// One colour's run-length-encoded pass across a six-row band.
fn band_run(rows: &[&Vec<usize>], register: usize, width: usize, scale: usize) -> String {
    let mut pieces = String::new();
    let mut running: Option<char> = None;
    let mut length = 0;

    for column in 0..width {
        let mut mask = 0;

        for (drop, row) in rows.iter().enumerate() {
            if row[column / scale] == register {
                mask |= 1 << drop;
            }
        }

        let character = char::from_u32(OFFSET + mask).expect("a sixel data character");

        if running == Some(character) {
            length += 1;

            continue;
        }

        if let Some(held) = running {
            pieces.push_str(&run(held, length));
        }

        running = Some(character);
        length = 1;
    }

    if let Some(held) = running {
        pieces.push_str(&run(held, length));
    }

    pieces
}

/// A run of one sixel character, counted when that is shorter.
fn run(character: char, length: usize) -> String {
    if length > RUN_WORTHWHILE {
        return format!("!{length}{character}");
    }

    character.to_string().repeat(length)
}

/// The picture as a palette and rows of register numbers.
///
/// A picture with more distinct colours than sixel's 256
/// registers posterizes each channel to six levels first -- a
/// loss no cover in the vendored art ever pays, their palettes
/// being small.
fn indexed(picture: &Picture) -> Indexed {
    let posterized: Vec<Vec<(u8, u8, u8)>>;
    let distinct: std::collections::BTreeSet<(u8, u8, u8)> =
        picture.rows.iter().flatten().copied().collect();
    let rows: &Vec<Vec<(u8, u8, u8)>> = if distinct.len() > PALETTE_LIMIT {
        posterized = picture
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|pixel| {
                        (
                            pixel.0 / POSTERIZE_STEP * POSTERIZE_STEP,
                            pixel.1 / POSTERIZE_STEP * POSTERIZE_STEP,
                            pixel.2 / POSTERIZE_STEP * POSTERIZE_STEP,
                        )
                    })
                    .collect()
            })
            .collect();

        &posterized
    } else {
        &picture.rows
    };
    let palette: Vec<(u8, u8, u8)> = rows
        .iter()
        .flatten()
        .copied()
        .collect::<std::collections::BTreeSet<(u8, u8, u8)>>()
        .into_iter()
        .collect();
    let registers: BTreeMap<(u8, u8, u8), usize> = palette
        .iter()
        .enumerate()
        .map(|(register, colour)| (*colour, register))
        .collect();
    let indices = rows
        .iter()
        .map(|row| row.iter().map(|pixel| registers[pixel]).collect())
        .collect();

    (palette, indices)
}

/// The whole-number magnification a sixel cover certainly fits.
///
/// The bounds are the glass's pixel dimensions -- the cell floors
/// times the cells, since the terminal will not say more. A
/// picture too large even unmagnified draws at native size and
/// lets the terminal clip its edge.
pub fn pixel_scale(picture: &Picture, width_pixels: usize, height_pixels: usize) -> usize {
    let width_bound = width_pixels / (picture.width as usize).max(1);
    let height_bound = height_pixels / (picture.height as usize).max(1);

    width_bound.min(height_bound).max(1)
}

#[cfg(test)]
#[path = "sixel_tests.rs"]
mod tests;
