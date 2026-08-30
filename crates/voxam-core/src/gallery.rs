//! The picture gallery: a Blorb's Version 6 art, sized and served.
//!
//! Version 6 games treat their pictures as data before decoration:
//! picture_data reads dimensions to lay out windows, and only then
//! is anything drawn (§15 picture_data). So the gallery answers
//! sizes cheaply -- a PNG's own IHDR words, a Rect placeholder's
//! eight bytes -- and decodes pixels lazily, one picture at a time
//! as draw_picture first asks, which keeps a two-thousand-picture
//! Zork Zero from paying its whole decode bill at boot. Rect
//! entries are the Blorb format's invisible pictures: real sizes
//! games measure and position by, with nothing to draw (Blorb:
//! Picture Resource Chunks). The Reso chunk's scaling instructions
//! ride along too: on a screen roomier than the art's standard
//! window, scalable pictures grow by the Elbow Room Factor, and
//! picture_data must report the grown size, because games lay out
//! their whole stage from those words (Blorb: The Resolution
//! Chunk).

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use crate::errors::VoxamError;
use crate::png::{IHDR, Picture, Pixel, SIGNATURE, decode, palette};

/// The Current Palette an adaptive-palette Blorb keeps: sixteen
/// entries, of which the spec deems indices 2 to 15 significant
/// (Blorb: The Adaptive Palette Chunk).
const PALETTE_SIZE: usize = 16;

// The IHDR chunk opens every PNG at a fixed seat: the eight-byte
// signature, the chunk length and name, then the width and height
// words (PNG: 5.6 Chunk ordering, 11.2.1 IHDR).
const IHDR_NAME_AT: usize = 12;
const WIDTH_AT: usize = 16;
const HEIGHT_AT: usize = 20;
const HEADER_END: usize = 24;

/// The reference's `Fraction`, sized to the gallery's needs: an
/// exact ratio, reduced with the sign on the numerator, so the
/// reported and drawn sizes can never drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ratio {
    numerator: i64,
    denominator: i64,
}

impl Ratio {
    /// The unscaled ratio: one image pixel per screen pixel.
    pub const ONE: Self = Self {
        numerator: 1,
        denominator: 1,
    };

    /// An exact numerator over a non-zero denominator, reduced.
    pub fn new(numerator: i64, denominator: i64) -> Self {
        assert!(denominator != 0, "a ratio's denominator cannot be zero");

        let shared = gcd(numerator.unsigned_abs(), denominator.unsigned_abs()).max(1) as i64;
        let sign = if denominator < 0 { -1 } else { 1 };

        Self {
            numerator: sign * numerator / shared,
            denominator: sign * denominator / shared,
        }
    }

    /// The exact product, reduced.
    #[must_use]
    pub fn times(self, other: Self) -> Self {
        // Cross-reduce first so the products stay well inside i64
        // for any pair of reduced ratios built from 32-bit words.
        let left = Self::new(self.numerator, other.denominator);
        let right = Self::new(other.numerator, self.denominator);

        Self::new(
            left.numerator * right.numerator,
            left.denominator * right.denominator,
        )
    }

    pub fn numerator(&self) -> i64 {
        self.numerator
    }

    pub fn denominator(&self) -> i64 {
        self.denominator
    }
}

impl Ord for Ratio {
    fn cmp(&self, other: &Self) -> Ordering {
        let left = i128::from(self.numerator) * i128::from(other.denominator);
        let right = i128::from(other.numerator) * i128::from(self.denominator);

        left.cmp(&right)
    }
}

impl PartialOrd for Ratio {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The greatest common divisor, Euclid's way.
fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        (left, right) = (right, left % right);
    }

    left
}

/// A Rect placeholder: a picture-shaped size with no pixels --
/// the width and height in pixels games lay out by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placard {
    pub width: u32,
    pub height: u32,
}

/// One scalable picture's ratios (Blorb: The Resolution Chunk):
/// the standard ratio the Elbow Room Factor multiplies, the floor
/// the result never drops below, and the ceiling it never rises
/// above -- None for no limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scaling {
    pub standard: Ratio,
    pub minimum: Option<Ratio>,
    pub maximum: Option<Ratio>,
}

/// The Reso chunk: a standard window and its scalable art.
///
/// `width` and `height` are the standard window in pixels -- the
/// screen the author drew for. `scalings` holds each scalable
/// picture's ratios by number; a picture with no entry is not
/// scalable at all: one image pixel per screen pixel, whatever the
/// room (Blorb: The Resolution Chunk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
    pub scalings: BTreeMap<u32, Scaling>,
}

/// One hung entry: a PNG's file bytes, a Placard where the Blorb
/// held a Rect, or an already-decoded Picture -- how the pre-Blorb
/// MG1/EG1 files hang their art, palettes long since applied.
#[derive(Debug, Clone)]
pub enum Art {
    Png(Vec<u8>),
    Placard(Placard),
    Picture(Rc<Picture>),
}

/// A Blorb's drawable art, by number: sizes eager, pixels lazy.
#[derive(Debug)]
pub struct Gallery {
    art: BTreeMap<u32, Art>,
    decoded: HashMap<u32, Rc<Picture>>,
    /// The release number of the picture file, which the
    /// picture_data census reports (§15 picture_data).
    pub release: u16,
    resolution: Option<Resolution>,
    adaptive: HashSet<u32>,
    baked: HashMap<(u32, u32), u32>,
    donor: Option<u32>,
    current: Option<Vec<Pixel>>,
    adapted: HashMap<u32, (Vec<Pixel>, Rc<Picture>)>,
    serial: u32,
}

impl Gallery {
    /// Hang the art: entries by picture number, the picture file's
    /// release, the Reso chunk's scaling instructions (None means
    /// every picture is non-scalable), the pictures that wear the
    /// Current Palette instead of their own -- Infocom's chrome,
    /// which recolours to match whatever scene was plotted last
    /// (Blorb: The Adaptive Palette Chunk) -- and the pre-applied
    /// replacements: each (scene, adaptive) pair's stand-in
    /// picture, its palette baked in by the packager (Bocfel: The
    /// Bocfel Adaptive Palette Chunk).
    pub fn new(
        art: BTreeMap<u32, Art>,
        release: u16,
        resolution: Option<Resolution>,
        adaptive: HashSet<u32>,
        baked: HashMap<(u32, u32), u32>,
    ) -> Self {
        Self {
            art,
            decoded: HashMap::new(),
            release,
            resolution,
            adaptive,
            baked,
            donor: None,
            current: None,
            adapted: HashMap::new(),
            serial: 0,
        }
    }

    /// The pictures that wear the Current Palette when plotted.
    pub fn adaptive(&self) -> &HashSet<u32> {
        &self.adaptive
    }

    /// How many times the Current Palette has actually changed.
    ///
    /// A frontend watches this across a plot: when a scene changes
    /// the palette, the chrome already on screen must be
    /// re-dressed -- Infocom's interpreters recoloured it through
    /// the hardware palette without replotting (Blorb: The
    /// Adaptive Palette Chunk).
    pub fn serial(&self) -> u32 {
        self.serial
    }

    /// How many pictures hang here, placards included.
    pub fn count(&self) -> usize {
        self.art.len()
    }

    /// Every hung picture number, ascending -- the census the
    /// reference walks straight off its art mapping.
    pub fn numbers(&self) -> impl Iterator<Item = u32> {
        self.art.keys().copied()
    }

    /// A picture's height and width in pixels, None for none.
    ///
    /// The order is picture_data's: height first (§15). A PNG
    /// answers from its IHDR words without decoding a pixel; an
    /// entry whose opening bytes are not the signature and IHDR
    /// the format requires is refused.
    pub fn size(&self, number: u32) -> Result<Option<(u32, u32)>, VoxamError> {
        match self.art.get(&number) {
            None => Ok(None),
            Some(Art::Placard(placard)) => Ok(Some((placard.height, placard.width))),
            Some(Art::Picture(picture)) => Ok(Some((picture.height, picture.width))),
            Some(Art::Png(entry)) => measured(entry).map(Some),
        }
    }

    /// A picture's scaling ratio on a screen of this size.
    ///
    /// The Elbow Room Factor is how many times the standard window
    /// fits the screen, the tighter axis deciding; a listed
    /// picture's standard ratio multiplies it, clamped between its
    /// minimum and maximum. A picture with no entry -- or a Blorb
    /// with no Reso chunk -- is not scalable and stays at 1
    /// (Blorb: The Resolution Chunk). The ratio is exact, so the
    /// reported and drawn sizes can never drift apart.
    pub fn scale(&self, number: u32, screen_width: u32, screen_height: u32) -> Ratio {
        let Some(resolution) = &self.resolution else {
            return Ratio::ONE;
        };
        let Some(scaling) = resolution.scalings.get(&number) else {
            return Ratio::ONE;
        };

        let room = Ratio::new(i64::from(screen_width), i64::from(resolution.width)).min(
            Ratio::new(i64::from(screen_height), i64::from(resolution.height)),
        );
        let ratio = room.times(scaling.standard);

        if let Some(minimum) = scaling.minimum
            && ratio < minimum
        {
            return minimum;
        }

        if let Some(maximum) = scaling.maximum
            && ratio > maximum
        {
            return maximum;
        }

        ratio
    }

    /// A picture's decoded pixels, None for a placard or none.
    ///
    /// This is the plotting seam, so the adaptive-palette dance
    /// happens here: a non-adaptive picture carries its own
    /// palette into the Current Palette as it is plotted, and an
    /// adaptive one is plotted wearing the Current Palette instead
    /// of its own (Blorb: The Adaptive Palette Chunk). Decoding is
    /// remembered -- the cache picture_table only ever hints at
    /// (§15) -- and an adaptive picture re-decodes whenever the
    /// Current Palette has changed beneath it.
    pub fn picture(&mut self, number: u32) -> Result<Option<Rc<Picture>>, VoxamError> {
        match self.art.get(&number) {
            None | Some(Art::Placard(_)) => return Ok(None),
            Some(Art::Picture(held)) => {
                // Pre-decoded art -- a picture file's -- carries
                // its palette applied and joins no adaptive dance.
                return Ok(Some(held.clone()));
            }
            Some(Art::Png(_)) => {}
        }

        if !self.adaptive.is_empty() {
            if self.adaptive.contains(&number) {
                let baked = self
                    .donor
                    .and_then(|donor| self.baked.get(&(donor, number)).copied());

                if let Some(replacement) = baked {
                    return self.baked_picture(replacement).map(Some);
                }

                return self.adapted_picture(number).map(Some);
            }

            self.absorb(number)?;
        }

        self.plainly_decoded(number).map(Some)
    }

    /// The remembered plain decode of a hung PNG entry.
    fn plainly_decoded(&mut self, number: u32) -> Result<Rc<Picture>, VoxamError> {
        if !self.decoded.contains_key(&number) {
            let picture = Rc::new(decode(self.hung(number), None)?);

            self.decoded.insert(number, picture);
        }

        Ok(self.decoded[&number].clone())
    }

    /// The PNG bytes the caller already matched; the art map never
    /// changes after hanging, so the entry is still there.
    fn hung(&self, number: u32) -> &[u8] {
        match self.art.get(&number) {
            Some(Art::Png(entry)) => entry,
            _ => unreachable!("the caller matched this entry as PNG art"),
        }
    }

    /// Carry a plotted picture's palette into the Current Palette.
    ///
    /// Only as many entries as the picture brought are changed
    /// (Blorb: The Adaptive Palette Chunk); a palette-less picture
    /// changes nothing -- not even the remembered donor, the
    /// picture number the baked replacements are looked up under
    /// (Bocfel: The Bocfel Adaptive Palette Chunk).
    fn absorb(&mut self, number: u32) -> Result<(), VoxamError> {
        let own = palette(self.hung(number))?;
        let own = &own[..own.len().min(PALETTE_SIZE)];

        if own.is_empty() {
            return Ok(());
        }

        self.donor = Some(number);

        let mut merged = self
            .current
            .clone()
            .unwrap_or_else(|| vec![(0, 0, 0); PALETTE_SIZE]);

        merged[..own.len()].copy_from_slice(own);

        if self.current.as_ref() != Some(&merged) {
            self.current = Some(merged);
            self.serial += 1;
        }

        Ok(())
    }

    /// A pre-applied replacement, standing in for adaptive art.
    ///
    /// The packager already dressed this picture in the scene's
    /// palette (Bocfel: The Bocfel Adaptive Palette Chunk), so it
    /// decodes plainly. Its size matches the picture it replaces
    /// by that chunk's own rule, so every measurement still
    /// answers from the original. A BPal record pointing at
    /// nothing is a lie worth hearing about.
    fn baked_picture(&mut self, replacement: u32) -> Result<Rc<Picture>, VoxamError> {
        match self.art.get(&replacement) {
            Some(Art::Png(_)) => {}
            _ => {
                return Err(VoxamError::Blorb(format!(
                    "a BPal record names picture {replacement} as a baked replacement, \
                     but the Blorb holds no such picture (Bocfel: The Bocfel Adaptive \
                     Palette Chunk)"
                )));
            }
        }

        self.plainly_decoded(replacement)
    }

    /// An adaptive picture, plotted in the Current Palette.
    ///
    /// Before any scene has set a palette the spec calls the
    /// result undefined; the picture's own palette is the quiet
    /// answer. The cache is keyed to the palette it was decoded
    /// under, so a scene change re-dresses the chrome.
    fn adapted_picture(&mut self, number: u32) -> Result<Rc<Picture>, VoxamError> {
        let Some(current) = self.current.clone() else {
            return self.plainly_decoded(number);
        };

        let stale = self
            .adapted
            .get(&number)
            .is_none_or(|(worn, _)| *worn != current);

        if stale {
            let picture = Rc::new(decode(self.hung(number), Some(&current))?);

            self.adapted.insert(number, (current, picture));
        }

        Ok(self.adapted[&number].1.clone())
    }
}

/// A PNG's height and width, read straight off its IHDR; bytes
/// that do not open with the signature and IHDR chunk every PNG
/// must lead with are refused.
fn measured(data: &[u8]) -> Result<(u32, u32), VoxamError> {
    if data.len() < HEADER_END
        || !data.starts_with(&SIGNATURE)
        || data[IHDR_NAME_AT..WIDTH_AT] != IHDR
    {
        return Err(VoxamError::Png(
            "a gallery picture does not open with a PNG signature and IHDR".to_string(),
        ));
    }

    let height = u32::from_be_bytes([
        data[HEIGHT_AT],
        data[HEIGHT_AT + 1],
        data[HEIGHT_AT + 2],
        data[HEIGHT_AT + 3],
    ]);
    let width = u32::from_be_bytes([
        data[WIDTH_AT],
        data[WIDTH_AT + 1],
        data[WIDTH_AT + 2],
        data[WIDTH_AT + 3],
    ]);

    Ok((height, width))
}
