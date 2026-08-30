//! The PNG battery, mirroring the reference's `test_png.py`.

use crate::errors::VoxamError;
use crate::flate::{crc32, deflated};
use crate::png::{IEND, IHDR, Picture, Pixel, SIGNATURE, decode, encoded, palette};

const BLACK: Pixel = (0, 0, 0);
const WHITE: Pixel = (255, 255, 255);

fn chunk(name: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = (payload.len() as u32).to_be_bytes().to_vec();

    out.extend_from_slice(name);
    out.extend_from_slice(payload);

    let summed = crc32(payload, crc32(name, 0));

    out.extend_from_slice(&summed.to_be_bytes());

    out
}

/// The battery's picture press, the keyword-argument builder the
/// reference's `picture_bytes` helper spells with defaults.
struct Press {
    width: u32,
    height: u32,
    depth: u8,
    colour_type: u8,
    raw: Vec<u8>,
    palette: Vec<u8>,
    alphas: Vec<u8>,
    interlace: u8,
    idat_pieces: usize,
    ended: bool,
}

impl Press {
    fn new(width: u32, height: u32, depth: u8, colour_type: u8, raw: &[u8]) -> Self {
        Self {
            width,
            height,
            depth,
            colour_type,
            raw: raw.to_vec(),
            palette: Vec::new(),
            alphas: Vec::new(),
            interlace: 0,
            idat_pieces: 1,
            ended: true,
        }
    }

    fn palette(mut self, held: &[u8]) -> Self {
        self.palette = held.to_vec();
        self
    }

    fn alphas(mut self, held: &[u8]) -> Self {
        self.alphas = held.to_vec();
        self
    }

    fn interlace(mut self, held: u8) -> Self {
        self.interlace = held;
        self
    }

    fn idat_pieces(mut self, held: usize) -> Self {
        self.idat_pieces = held;
        self
    }

    fn ended(mut self, held: bool) -> Self {
        self.ended = held;
        self
    }

    fn built(self) -> Vec<u8> {
        let mut header = self.width.to_be_bytes().to_vec();

        header.extend_from_slice(&self.height.to_be_bytes());
        header.extend_from_slice(&[self.depth, self.colour_type, 0, 0, self.interlace]);

        let compressed = deflated(&self.raw);
        let mut pieces = SIGNATURE.to_vec();

        pieces.extend_from_slice(&chunk(&IHDR, &header));
        pieces.extend_from_slice(&chunk(b"gAMA", &[0x00, 0x01, 0x86, 0xA0]));

        if !self.palette.is_empty() {
            pieces.extend_from_slice(&chunk(b"PLTE", &self.palette));
        }

        if !self.alphas.is_empty() {
            pieces.extend_from_slice(&chunk(b"tRNS", &self.alphas));
        }

        let split = (compressed.len() / self.idat_pieces).max(1);

        for piece in compressed.chunks(split) {
            pieces.extend_from_slice(&chunk(b"IDAT", piece));
        }

        if self.ended {
            pieces.extend_from_slice(&chunk(&IEND, &[]));
        }

        pieces
    }
}

fn pressed(width: u32, height: u32, depth: u8, colour_type: u8, raw: &[u8]) -> Vec<u8> {
    Press::new(width, height, depth, colour_type, raw).built()
}

fn complaint(result: Result<Picture, VoxamError>) -> String {
    result.expect_err("refused").to_string()
}

// A truecolour picture with unfiltered scanlines decodes to its
// pixels exactly.
#[test]
fn truecolour_pixels_decode_exactly() {
    let mut raw = vec![0, 255, 0, 0, 0, 255, 0];

    raw.extend_from_slice(&[0, 0, 0, 255, 255, 255, 255]);

    let picture = decode(&pressed(2, 2, 8, 2, &raw), None).expect("decodes");

    assert_eq!(picture.width, 2);
    assert_eq!(picture.height, 2);
    assert_eq!(picture.rows[0], vec![(255, 0, 0), (0, 255, 0)]);
    assert_eq!(picture.rows[1], vec![(0, 0, 255), WHITE]);
}

// The Sub filter adds the byte one pixel to the left back in
// (PNG 9.2).
#[test]
fn the_sub_filter_reconstructs_from_the_left() {
    let raw = [1, 10, 20, 30, 5, 5, 5];
    let picture = decode(&pressed(2, 1, 8, 2, &raw), None).expect("decodes");

    assert_eq!(picture.rows[0], vec![(10, 20, 30), (15, 25, 35)]);
}

// The Up filter adds the byte directly above back in; above the
// first line sits an imaginary row of zeros (PNG 9.2).
#[test]
fn the_up_filter_reconstructs_from_above() {
    let raw = [2, 10, 20, 30, 2, 1, 2, 3];
    let picture = decode(&pressed(1, 2, 8, 2, &raw), None).expect("decodes");

    assert_eq!(picture.rows[0], vec![(10, 20, 30)]);
    assert_eq!(picture.rows[1], vec![(11, 22, 33)]);
}

// The Average filter adds back the mean of left and above, floored
// (PNG 9.2).
#[test]
fn the_average_filter_reconstructs_from_the_mean() {
    let raw = [0, 10, 20, 30, 40, 50, 60, 3, 5, 5, 5, 5, 5, 5];
    let picture = decode(&pressed(2, 2, 8, 2, &raw), None).expect("decodes");

    assert_eq!(picture.rows[1][0], (10, 15, 20));
    assert_eq!(picture.rows[1][1], (30, 37, 45));
}

// Paeth's predictor picks whichever of left, above, and corner lies
// nearest its guess; this line makes each of the three win once
// (PNG 9.4).
#[test]
fn the_paeth_filter_tries_all_three_neighbours() {
    let raw = [0, 1, 2, 3, 4, 4, 252, 7];
    let picture = decode(&pressed(3, 2, 8, 0, &raw), None).expect("decodes");

    assert_eq!(picture.rows[1], vec![(5, 5, 5), (1, 1, 1), (9, 9, 9)]);
}

// A truecolour picture with a translucent pixel keeps its straight
// source colors and carries the alpha channel whole, the clear
// flags still marking the fully transparent -- a display that can
// blend does the composing itself.
#[test]
fn partial_alpha_travels_straight() {
    let raw = [0, 100, 150, 200, 128, 10, 20, 30, 0, 40, 50, 60, 255];
    let picture = decode(&pressed(3, 1, 8, 6, &raw), None).expect("decodes");

    assert_eq!(
        picture.rows[0],
        vec![(100, 150, 200), (10, 20, 30), (40, 50, 60)]
    );
    assert_eq!(picture.alpha, Some(vec![vec![128, 0, 255]]));
    assert_eq!(picture.clear, Some(vec![vec![false, true, false]]));
}

// With only full opacity and full transparency aboard, the alpha
// channel is dropped and the picture decodes exactly as it always
// has: composed rows, and the clear flags saying everything.
#[test]
fn binary_alpha_stays_composed() {
    let raw = [0, 100, 150, 200, 255, 10, 20, 30, 0];
    let picture = decode(&pressed(2, 1, 8, 6, &raw), None).expect("decodes");

    assert_eq!(picture.alpha, None);
    assert_eq!(picture.rows[0], vec![(100, 150, 200), BLACK]);
    assert_eq!(picture.clear, Some(vec![vec![false, true]]));
}

// Grey-with-alpha and palette pictures carry partial alpha the
// same way: straight greys, straight palette entries, and the
// opacities aboard beside them.
#[test]
fn grey_and_palette_partial_alpha() {
    let raw = [0, 200, 77, 100, 255];
    let grey = decode(&pressed(2, 1, 8, 4, &raw), None).expect("decodes");

    assert_eq!(grey.rows[0], vec![(200, 200, 200), (100, 100, 100)]);
    assert_eq!(grey.alpha, Some(vec![vec![77, 255]]));

    let plotted = decode(
        &Press::new(2, 1, 8, 3, &[0, 0, 1])
            .palette(&[200, 0, 0, 9, 9, 9])
            .alphas(&[255, 128])
            .built(),
        None,
    )
    .expect("decodes");

    assert_eq!(plotted.rows[0], vec![(200, 0, 0), (9, 9, 9)]);
    assert_eq!(plotted.alpha, Some(vec![vec![255, 128]]));
    assert_eq!(plotted.clear, Some(vec![vec![false, false]]));
}

// A palette picture at bit depth 4 -- Beyond Zork's own shape --
// unpacks two indices per byte, and its data may arrive split
// across several IDAT chunks.
#[test]
fn palette_nibbles_decode_across_split_idats() {
    let held = [255, 0, 0, 0, 255, 0, 0, 0, 255];
    let raw = [0, 0x01, 0x20, 0, 0x21, 0x00];
    let picture = decode(
        &Press::new(3, 2, 4, 3, &raw)
            .palette(&held)
            .idat_pieces(3)
            .built(),
        None,
    )
    .expect("decodes");

    assert_eq!(picture.rows[0], vec![(255, 0, 0), (0, 255, 0), (0, 0, 255)]);
    assert_eq!(picture.rows[1], vec![(0, 0, 255), (0, 255, 0), (255, 0, 0)]);
}

// Bit depth 1 packs eight indices per byte, and a picture without
// an IEND chunk simply runs out of chunks.
#[test]
fn single_bit_palettes_decode() {
    let held = [0, 0, 0, 255, 255, 255];
    let raw = [0, 0b1011_0000];
    let picture = decode(
        &Press::new(4, 1, 1, 3, &raw)
            .palette(&held)
            .ended(false)
            .built(),
        None,
    )
    .expect("decodes");

    assert_eq!(picture.rows[0], vec![WHITE, BLACK, WHITE, WHITE]);
}

// Greyscale values scale up to the full 0-255 range: at depth 2,
// the four levels are 0, 85, 170, and 255.
#[test]
fn greyscale_depths_scale_to_full_range() {
    let raw = [0, 0b0001_1011];
    let picture = decode(&pressed(4, 1, 2, 0, &raw), None).expect("decodes");

    assert_eq!(
        picture.rows[0],
        vec![BLACK, (85, 85, 85), (170, 170, 170), WHITE]
    );
}

// A half-transparent pixel keeps its straight orange and carries
// its opacity: composing is the display's business now, and the
// clear flags still mark the fully see-through.
#[test]
fn partial_alpha_keeps_straight_colors() {
    let raw = [0, 200, 100, 50, 128, 255, 255, 255, 0];
    let picture = decode(&pressed(2, 1, 8, 6, &raw), None).expect("decodes");

    assert_eq!(picture.rows[0], vec![(200, 100, 50), WHITE]);
    assert_eq!(picture.alpha, Some(vec![vec![128, 0]]));
    assert_eq!(picture.clear, Some(vec![vec![false, true]]));
}

// Greyscale with alpha composes the same way.
#[test]
fn grey_alpha_composes_over_black() {
    let raw = [0, 100, 255, 200, 0];
    let picture = decode(&pressed(2, 1, 8, 4, &raw), None).expect("decodes");

    assert_eq!(picture.rows[0], vec![(100, 100, 100), BLACK]);
    assert_eq!(picture.clear, Some(vec![vec![false, true]]));
}

// A tRNS chunk gives palette entries alphas; entries beyond its
// end stay opaque, and a partial entry rides the alpha channel
// with its straight palette color.
#[test]
fn palette_transparency_defaults_opaque() {
    let held = [200, 100, 50, 10, 20, 30];
    let picture = decode(
        &Press::new(2, 1, 8, 3, &[0, 0, 1])
            .palette(&held)
            .alphas(&[128])
            .built(),
        None,
    )
    .expect("decodes");

    assert_eq!(picture.rows[0], vec![(200, 100, 50), (10, 20, 30)]);
    assert_eq!(picture.alpha, Some(vec![vec![128, 255]]));
    assert_eq!(picture.clear, Some(vec![vec![false, false]]));
}

// Only a zero alpha marks a pixel clear -- Version 6 chrome layers
// with fully see-through holes, and only full transparency matters
// there (Blorb: Picture Resource Chunks). A picture with no alpha
// at all carries no clear grid.
#[test]
fn fully_transparent_pixels_are_marked_clear() {
    let held = [200, 100, 50, 10, 20, 30];
    let picture = decode(
        &Press::new(2, 1, 8, 3, &[0, 0, 1])
            .palette(&held)
            .alphas(&[255, 0])
            .built(),
        None,
    )
    .expect("decodes");

    assert_eq!(picture.rows[0], vec![(200, 100, 50), BLACK]);
    assert_eq!(picture.clear, Some(vec![vec![false, true]]));

    let opaque = decode(&pressed(1, 1, 8, 2, &[0, 1, 2, 3]), None).expect("decodes");

    assert_eq!(opaque.clear, None);
}

// The palette reader hands back a PNG's own PLTE -- what a plotted
// scene carries into the Current Palette -- a palette-less picture
// answers empty, and non-PNG bytes are refused (Blorb: The
// Adaptive Palette Chunk).
#[test]
fn the_palette_reader_answers_the_plte() {
    let data = Press::new(2, 1, 8, 3, &[0, 0, 1])
        .palette(&[1, 2, 3, 4, 5, 6])
        .built();

    assert_eq!(palette(&data).expect("reads"), vec![(1, 2, 3), (4, 5, 6)]);
    assert!(
        palette(&pressed(1, 1, 8, 2, &[0; 4]))
            .expect("reads")
            .is_empty()
    );
    assert!(
        palette(b"GIF89a nope")
            .expect_err("refused")
            .to_string()
            .contains("PNG signature")
    );
}

// An adapted palette overrides the file's own at plot time, while
// transparency stays the file's (Blorb: The Adaptive Palette
// Chunk).
#[test]
fn an_adapted_palette_redresses_the_pixels() {
    let data = Press::new(2, 1, 8, 3, &[0, 0, 1])
        .palette(&[9; 6])
        .alphas(&[255, 0])
        .built();
    let picture = decode(&data, Some(&[(10, 11, 12), (20, 21, 22)])).expect("decodes");

    assert_eq!(picture.rows[0], vec![(10, 11, 12), BLACK]);
    assert_eq!(picture.clear, Some(vec![vec![false, true]]));
}

// What cannot be a supported picture is refused with the reason
// given: foreign bytes, interlacing, the pairings outside the
// census, an empty image, a missing palette, a missing or
// malformed header, and a chunk cut short.
#[test]
fn unusable_pictures_are_refused() {
    assert!(complaint(decode(b"GIF89a not a png", None)).contains("PNG signature"));
    assert!(
        complaint(decode(
            &Press::new(1, 1, 8, 2, &[0; 4]).interlace(1).built(),
            None
        ))
        .contains("interlaced")
    );
    assert!(
        complaint(decode(&pressed(1, 1, 16, 2, &[0; 7]), None)).contains("not a supported pairing")
    );
    assert!(
        complaint(decode(&pressed(1, 1, 8, 5, &[0; 4]), None)).contains("not a supported pairing")
    );
    assert!(complaint(decode(&pressed(0, 1, 8, 2, &[]), None)).contains("no pixels"));
    assert!(complaint(decode(&pressed(1, 1, 8, 3, &[0; 2]), None)).contains("without its PLTE"));

    let mut ended = SIGNATURE.to_vec();

    ended.extend_from_slice(&chunk(&IEND, &[]));

    assert!(complaint(decode(&ended, None)).contains("no IHDR"));

    let mut cut = SIGNATURE.to_vec();

    cut.extend_from_slice(b"\x00\x00\x00\x0dIHDR\x00");

    assert!(complaint(decode(&cut, None)).contains("cut short"));

    let mut malformed = SIGNATURE.to_vec();

    malformed.extend_from_slice(&chunk(&IHDR, &[0x00, 0x01]));

    assert!(complaint(decode(&malformed, None)).contains("malformed"));
}

// Image data that does not inflate, inflates to the wrong size, or
// names an undefined filter is refused with the reason given.
#[test]
fn broken_image_data_is_refused() {
    let mut header = 1u32.to_be_bytes().to_vec();

    header.extend_from_slice(&1u32.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);

    let mut garbage = SIGNATURE.to_vec();

    garbage.extend_from_slice(&chunk(&IHDR, &header));
    garbage.extend_from_slice(&chunk(b"IDAT", b"not-deflated"));
    garbage.extend_from_slice(&chunk(&IEND, &[]));

    assert!(complaint(decode(&garbage, None)).contains("does not inflate"));
    assert!(complaint(decode(&pressed(1, 1, 8, 2, &[0; 9]), None)).contains("bytes of scanlines"));
    assert!(complaint(decode(&pressed(1, 1, 8, 2, &[5, 0, 0, 0]), None)).contains("filter type 5"));
}

// A pixel pointing beyond the palette is corrupt, not black.
#[test]
fn palette_overruns_are_refused() {
    let data = Press::new(1, 1, 8, 3, &[0, 7]).palette(&[1, 2, 3]).built();

    assert!(complaint(decode(&data, None)).contains("beyond the 1-entry palette"));
}

// The encoder is decode's write-side twin: plain truecolour rides
// opaque, clear flags travel as zero alpha, and partial alpha
// rides whole -- so what the wire carries decodes back to the very
// pixels the gallery plotted. A fully-clear pixel's colour is the
// one thing that does not survive, composed over black as decode
// always composes what cannot show.
#[test]
fn encoded_pictures_round_trip() {
    let plain = Picture {
        width: 2,
        height: 1,
        rows: vec![vec![(1, 2, 3), (4, 5, 6)]],
        clear: None,
        alpha: None,
    };
    let back = decode(&encoded(&plain), None).expect("decodes");

    assert_eq!((back.width, back.height), (2, 1));
    assert_eq!(back.rows, plain.rows);
    assert_eq!(back.clear, None);
    assert_eq!(back.alpha, None);

    let holed = Picture {
        width: 2,
        height: 1,
        rows: vec![vec![(9, 9, 9), (4, 5, 6)]],
        clear: Some(vec![vec![true, false]]),
        alpha: None,
    };
    let hollow = decode(&encoded(&holed), None).expect("decodes");

    assert_eq!(hollow.clear, Some(vec![vec![true, false]]));
    assert_eq!(hollow.rows[0][1], (4, 5, 6));

    let misty = Picture {
        width: 1,
        height: 1,
        rows: vec![vec![(10, 20, 30)]],
        clear: None,
        alpha: Some(vec![vec![128]]),
    };
    let misted = decode(&encoded(&misty), None).expect("decodes");

    assert_eq!(misted.alpha, Some(vec![vec![128]]));
    assert_eq!(misted.rows[0][0], (10, 20, 30));
}

// The encoder's bytes are the reference's, vector for vector: the
// patched png.py's encoded() produced these three files -- plain,
// holed, and misty -- and the port must spell them identically,
// because they ride the certified wire as data: urls.
#[test]
fn encoded_matches_the_reference_vectors() {
    let unhexed = |text: &str| -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|held| u8::from_str_radix(&text[held..held + 2], 16).expect("hex"))
            .collect()
    };
    let plain = Picture {
        width: 2,
        height: 1,
        rows: vec![vec![(1, 2, 3), (4, 5, 6)]],
        clear: None,
        alpha: None,
    };

    assert_eq!(
        encoded(&plain),
        unhexed(concat!(
            "89504e470d0a1a0a0000000d49484452000000020000000108020000007b40",
            "e8dd0000000f494441547801636064626661650300003f0016738177810000",
            "000049454e44ae426082"
        ))
    );

    let holed = Picture {
        width: 2,
        height: 1,
        rows: vec![vec![(9, 9, 9), (4, 5, 6)]],
        clear: Some(vec![vec![true, false]]),
        alpha: None,
    };

    assert_eq!(
        encoded(&holed),
        unhexed(concat!(
            "89504e470d0a1a0a0000000d4948445200000002000000010806000000f422",
            "7f8a0000001149444154780163e0e4e464606165fb0f0001f0012a9e28c690",
            "0000000049454e44ae426082"
        ))
    );

    let misty = Picture {
        width: 1,
        height: 1,
        rows: vec![vec![(10, 20, 30)]],
        clear: None,
        alpha: Some(vec![vec![128]]),
    };

    assert_eq!(
        encoded(&misty),
        unhexed(concat!(
            "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15",
            "c4890000000d49444154780163e012916b0000012500bd1c36baec00000000",
            "49454e44ae426082"
        ))
    );
}
