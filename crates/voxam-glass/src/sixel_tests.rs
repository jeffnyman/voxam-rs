//! The sixel encoder, held to the reference battery's sequences.

use voxam_core::png::Picture;

use super::*;

const RED: (u8, u8, u8) = (255, 0, 0);
const BLUE: (u8, u8, u8) = (0, 0, 255);

fn pictured(width: u32, height: u32, rows: Vec<Vec<(u8, u8, u8)>>) -> Picture {
    Picture {
        width,
        height,
        rows,
        clear: None,
        alpha: None,
    }
}

// A one-colour picture encodes as one palette register in
// percent, raster attributes carrying the pixel size, and a
// full-height run.
#[test]
fn a_flat_picture_encodes_one_register() {
    let picture = pictured(4, 2, vec![vec![RED; 4]; 2]);
    let sequence = encode(&picture, 1);

    assert!(sequence.starts_with("\u{1b}Pq\"1;1;4;2"));
    assert!(sequence.contains("#0;2;100;0;0"));
    // Two rows of four pixels: mask 0b000011 -> '?' + 3 = 'B'.
    assert!(sequence.contains("!4B"));
    assert!(sequence.ends_with("-\u{1b}\\"));
}

// Two colours share a band: the second pass returns to the left
// edge with $ before overprinting its own pixels.
#[test]
fn band_colours_take_turns_from_the_left() {
    let picture = pictured(2, 1, vec![vec![RED, BLUE]]);
    let sequence = encode(&picture, 1);

    assert!(sequence.contains("#0;2;0;0;100"));
    assert!(sequence.contains("#1;2;100;0;0"));
    assert!(sequence.contains('$'));
    // Blue holds register 0 and paints the second column; red the
    // first.
    assert!(sequence.contains("#0?@"));
    assert!(sequence.contains("#1@?"));
}

// Integer scaling multiplies pixels in both directions: a single
// pixel at scale 3 is a 3x3 block.
#[test]
fn scaling_magnifies_whole_pixels() {
    let picture = pictured(1, 1, vec![vec![RED]]);
    let sequence = encode(&picture, 3);

    assert!(sequence.contains("\"1;1;3;3"));
    // Three rows high -> mask 0b111 -> '?' + 7 = 'F', three wide.
    assert!(sequence.contains("FFF"));
}

// A picture richer than sixel's 256 registers posterizes down to
// a workable palette instead of refusing.
#[test]
fn rich_pictures_posterize_into_the_palette() {
    let rows: Vec<Vec<(u8, u8, u8)>> = (0u16..255)
        .step_by(16)
        .map(|red| (0u8..32).map(|green| (red as u8, green, 0)).collect())
        .collect();
    let picture = pictured(32, 16, rows);
    let sequence = encode(&picture, 1);

    assert!(sequence.starts_with("\u{1b}Pq"));
    assert!(sequence.matches(";2;").count() <= 256);
}

// The magnification fits the floors: a big glass magnifies a
// small cover, and a cover too large even unmagnified stays at
// native size for the terminal to clip.
#[test]
fn the_scale_fits_the_glass() {
    let small = pictured(4, 2, vec![vec![RED; 4]; 2]);

    assert_eq!(pixel_scale(&small, 640, 384), 160);

    let vast = pictured(10_000, 10_000, vec![vec![RED; 1]; 1]);

    assert_eq!(pixel_scale(&vast, 640, 384), 1);
}
