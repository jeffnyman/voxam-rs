//! The gallery battery, mirroring the reference's `test_gallery.py`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use crate::flate::{crc32, deflated};
use crate::gallery::{Art, Gallery, Placard, Ratio, Resolution, Scaling};
use crate::png::{Pixel, SIGNATURE};

fn piece(name: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = (payload.len() as u32).to_be_bytes().to_vec();

    out.extend_from_slice(name);
    out.extend_from_slice(payload);
    out.extend_from_slice(&crc32(payload, crc32(name, 0)).to_be_bytes());

    out
}

/// A 2-by-1 indexed-colour PNG in the APal style.
fn indexed(colours: &[Pixel], alphas: &[u8], raw: &[u8]) -> Vec<u8> {
    let mut header = 2u32.to_be_bytes().to_vec();

    header.extend_from_slice(&1u32.to_be_bytes());
    header.extend_from_slice(&[8, 3, 0, 0, 0]);

    let palette: Vec<u8> = colours
        .iter()
        .flat_map(|&(red, green, blue)| [red, green, blue])
        .collect();
    let mut pieces = SIGNATURE.to_vec();

    pieces.extend_from_slice(&piece(b"IHDR", &header));
    pieces.extend_from_slice(&piece(b"PLTE", &palette));

    if !alphas.is_empty() {
        pieces.extend_from_slice(&piece(b"tRNS", alphas));
    }

    pieces.extend_from_slice(&piece(b"IDAT", &deflated(raw)));
    pieces.extend_from_slice(&piece(b"IEND", &[]));

    pieces
}

fn indexed_plain(colours: &[Pixel]) -> Vec<u8> {
    indexed(colours, &[], &[0x00, 0x00, 0x01])
}

/// The conftest's tiny_png: a 2-by-2 truecolour PNG, one bright
/// row over a black one.
fn tiny_png() -> Vec<u8> {
    let mut header = 2u32.to_be_bytes().to_vec();

    header.extend_from_slice(&2u32.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);

    let mut raw = vec![0u8, 10, 20, 30, 40, 50, 60, 0];

    raw.extend_from_slice(&[0; 6]);

    let mut pieces = SIGNATURE.to_vec();

    pieces.extend_from_slice(&piece(b"IHDR", &header));
    pieces.extend_from_slice(&piece(b"IDAT", &deflated(&raw)));
    pieces.extend_from_slice(&piece(b"IEND", &[]));

    pieces
}

fn hung(png: Vec<u8>) -> Gallery {
    let mut art = BTreeMap::new();

    art.insert(3, Art::Png(png));
    art.insert(
        7,
        Art::Placard(Placard {
            width: 314,
            height: 84,
        }),
    );

    Gallery::new(art, 27, None, HashSet::new(), HashMap::new())
}

fn plain_gallery(art: BTreeMap<u32, Art>, adaptive: &[u32]) -> Gallery {
    Gallery::new(
        art,
        0,
        None,
        adaptive.iter().copied().collect(),
        HashMap::new(),
    )
}

// Sizes answer without decoding a pixel: the PNG's own IHDR words
// and a placard's stored shape, height first as picture_data wants
// them (§15 picture_data).
#[test]
fn sizes_answer_height_first() {
    let gallery = hung(tiny_png());

    assert_eq!(gallery.size(3).expect("measures"), Some((2, 2)));
    assert_eq!(gallery.size(7).expect("measures"), Some((84, 314)));
    assert_eq!(gallery.size(99).expect("measures"), None);
    assert_eq!(gallery.count(), 2);
    assert_eq!(gallery.release, 27);
}

// Pixels decode on the first ask and are remembered: the second
// ask answers the same object. A placard has no pixels to give,
// and an absent number gives None.
#[test]
fn pictures_decode_lazily_and_are_remembered() {
    let mut gallery = hung(tiny_png());
    let first = gallery.picture(3).expect("decodes").expect("hangs");

    assert_eq!(first.rows[0], vec![(10, 20, 30), (40, 50, 60)]);

    let again = gallery.picture(3).expect("decodes").expect("hangs");

    assert!(Rc::ptr_eq(&first, &again));
    assert!(gallery.picture(7).expect("asks").is_none());
    assert!(gallery.picture(99).expect("asks").is_none());
}

// The scaling ratio follows the Blorb spec to the letter: the
// Elbow Room Factor is the tighter axis of screen over standard
// window, a listed picture's standard ratio multiplies it, and
// the minimum and maximum clamp the result. An unlisted picture
// -- or a gallery with no Reso chunk at all -- stays at 1: one
// image pixel per screen pixel (Blorb: The Resolution Chunk).
#[test]
fn the_scaling_ratio_follows_the_elbow_room() {
    let bare = hung(tiny_png());

    assert_eq!(bare.scale(3, 720, 432), Ratio::ONE);

    let mut scalings = BTreeMap::new();

    scalings.insert(
        3,
        Scaling {
            standard: Ratio::ONE,
            minimum: None,
            maximum: None,
        },
    );
    scalings.insert(
        5,
        Scaling {
            standard: Ratio::new(1, 2),
            minimum: None,
            maximum: None,
        },
    );
    scalings.insert(
        8,
        Scaling {
            standard: Ratio::ONE,
            minimum: Some(Ratio::new(3, 1)),
            maximum: None,
        },
    );
    scalings.insert(
        9,
        Scaling {
            standard: Ratio::ONE,
            minimum: None,
            maximum: Some(Ratio::new(2, 1)),
        },
    );

    let resolution = Resolution {
        width: 320,
        height: 200,
        scalings,
    };
    let gallery = Gallery::new(
        BTreeMap::new(),
        0,
        Some(resolution),
        HashSet::new(),
        HashMap::new(),
    );

    // ERF = min(720/320, 432/200) = 54/25: the height decides.
    assert_eq!(gallery.scale(3, 720, 432), Ratio::new(54, 25));
    assert_eq!(gallery.scale(5, 720, 432), Ratio::new(27, 25));
    assert_eq!(gallery.scale(8, 720, 432), Ratio::new(3, 1));
    assert_eq!(gallery.scale(9, 720, 432), Ratio::new(2, 1));
    assert_eq!(gallery.scale(99, 720, 432), Ratio::ONE);
}

// The adaptive-palette dance (Blorb: The Adaptive Palette Chunk):
// chrome plotted before any scene wears its own palette, quietly;
// a plotted scene becomes the Current Palette; the chrome then
// wears it, re-dressing whenever the scene changes; a shorter
// scene palette changes only the entries it brought; and a
// palette-less picture disturbs nothing (tiny_png is truecolour).
#[test]
fn adaptive_chrome_wears_the_scene_palette() {
    let mut art = BTreeMap::new();

    art.insert(1, Art::Png(indexed_plain(&[(10, 10, 10), (20, 20, 20)])));
    art.insert(2, Art::Png(indexed_plain(&[(30, 30, 30), (40, 40, 40)])));
    art.insert(3, Art::Png(tiny_png()));
    art.insert(
        4,
        Art::Png(indexed(&[(99, 99, 99)], &[], &[0x00, 0x00, 0x00])),
    );
    art.insert(7, Art::Png(indexed_plain(&[(1, 1, 1), (2, 2, 2)])));

    let mut gallery = plain_gallery(art, &[7]);

    assert_eq!(gallery.adaptive(), &[7].iter().copied().collect());
    assert_eq!(gallery.serial(), 0);

    let before = gallery.picture(7).expect("decodes").expect("hangs");

    assert_eq!(before.rows[0], vec![(1, 1, 1), (2, 2, 2)]);
    assert!(Rc::ptr_eq(
        &before,
        &gallery.picture(7).expect("decodes").expect("hangs")
    ));

    gallery.picture(1).expect("plots");

    assert_eq!(gallery.serial(), 1);

    gallery.picture(1).expect("plots");

    assert_eq!(gallery.serial(), 1);

    let dressed = gallery.picture(7).expect("decodes").expect("hangs");

    assert_eq!(dressed.rows[0], vec![(10, 10, 10), (20, 20, 20)]);
    assert!(Rc::ptr_eq(
        &dressed,
        &gallery.picture(7).expect("decodes").expect("hangs")
    ));

    gallery.picture(3).expect("plots");

    assert_eq!(gallery.serial(), 1);
    assert!(Rc::ptr_eq(
        &dressed,
        &gallery.picture(7).expect("decodes").expect("hangs")
    ));

    gallery.picture(2).expect("plots");

    let redressed = gallery.picture(7).expect("decodes").expect("hangs");

    assert_eq!(redressed.rows[0], vec![(30, 30, 30), (40, 40, 40)]);

    gallery.picture(4).expect("plots");

    let merged = gallery.picture(7).expect("decodes").expect("hangs");

    assert_eq!(merged.rows[0], vec![(99, 99, 99), (40, 40, 40)]);
}

// An adaptive picture's transparency is its own even while wearing
// the Current Palette: the scene recolours the chrome, but the
// holes stay holes (Blorb: The Adaptive Palette Chunk).
#[test]
fn adaptive_chrome_keeps_its_holes() {
    let mut art = BTreeMap::new();

    art.insert(1, Art::Png(indexed_plain(&[(10, 10, 10), (20, 20, 20)])));
    art.insert(
        7,
        Art::Png(indexed(&[(1, 1, 1), (2, 2, 2)], &[0], &[0x00, 0x00, 0x01])),
    );

    let mut gallery = plain_gallery(art, &[7]);

    gallery.picture(1).expect("plots");

    let dressed = gallery.picture(7).expect("decodes").expect("hangs");

    assert_eq!(dressed.rows[0], vec![(0, 0, 0), (20, 20, 20)]);
    assert_eq!(dressed.clear, Some(vec![vec![true, false]]));
}

// A BPal record pre-empts the dance (Bocfel: The Bocfel Adaptive
// Palette Chunk): with scene 1 holding the Current Palette, the
// chrome plots as its baked replacement, decoded plainly. Before
// any scene the APal rules answer as ever; a palette-less plot
// does not move the donor; and a scene with no record falls back
// to the live dance.
#[test]
fn baked_replacements_stand_in_for_the_chrome() {
    let mut art = BTreeMap::new();

    art.insert(1, Art::Png(indexed_plain(&[(10, 10, 10), (20, 20, 20)])));
    art.insert(2, Art::Png(indexed_plain(&[(30, 30, 30), (40, 40, 40)])));
    art.insert(3, Art::Png(tiny_png()));
    art.insert(7, Art::Png(indexed_plain(&[(1, 1, 1), (2, 2, 2)])));
    art.insert(1000, Art::Png(indexed_plain(&[(70, 70, 70), (80, 80, 80)])));

    let mut baked = HashMap::new();

    baked.insert((1, 7), 1000);

    let mut gallery = Gallery::new(art, 0, None, [7].iter().copied().collect(), baked);
    let before = gallery.picture(7).expect("decodes").expect("hangs");

    assert_eq!(before.rows[0], vec![(1, 1, 1), (2, 2, 2)]);

    gallery.picture(1).expect("plots");

    let baked = gallery.picture(7).expect("decodes").expect("hangs");

    assert_eq!(baked.rows[0], vec![(70, 70, 70), (80, 80, 80)]);
    assert!(Rc::ptr_eq(
        &baked,
        &gallery.picture(7).expect("decodes").expect("hangs")
    ));

    gallery.picture(3).expect("plots");

    assert!(Rc::ptr_eq(
        &baked,
        &gallery.picture(7).expect("decodes").expect("hangs")
    ));

    gallery.picture(2).expect("plots");

    let dressed = gallery.picture(7).expect("decodes").expect("hangs");

    assert_eq!(dressed.rows[0], vec![(30, 30, 30), (40, 40, 40)]);
}

// A BPal record naming a picture the Blorb does not hold is a lie
// heard loudly, never a silent mis-draw.
#[test]
fn a_baked_record_pointing_at_nothing_is_loud() {
    let mut art = BTreeMap::new();

    art.insert(1, Art::Png(indexed_plain(&[(10, 10, 10), (20, 20, 20)])));
    art.insert(7, Art::Png(indexed_plain(&[(1, 1, 1), (2, 2, 2)])));

    let mut baked = HashMap::new();

    baked.insert((1, 7), 1000);

    let mut gallery = Gallery::new(art, 0, None, [7].iter().copied().collect(), baked);

    gallery.picture(1).expect("plots");

    let complaint = gallery.picture(7).expect_err("refused").to_string();

    assert!(complaint.contains("names picture 1000"));
}

// An entry that does not open with the PNG signature and IHDR is
// refused loudly rather than measured wrongly -- whether the bytes
// are wrong or simply too few.
#[test]
fn malformed_art_is_refused() {
    let mut art = BTreeMap::new();

    art.insert(
        1,
        Art::Png(b"not a png, but comfortably past twenty-four bytes".to_vec()),
    );

    let wrong = plain_gallery(art, &[]);

    assert!(
        wrong
            .size(1)
            .expect_err("refused")
            .to_string()
            .contains("signature and IHDR")
    );

    let mut stub = BTreeMap::new();

    stub.insert(2, Art::Png(b"xx".to_vec()));

    assert!(
        plain_gallery(stub, &[])
            .size(2)
            .expect_err("refused")
            .to_string()
            .contains("signature and IHDR")
    );
}
