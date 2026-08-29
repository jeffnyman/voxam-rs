//! Writing WAVE sounds with nothing but this crate.
//!
//! The wire's own sound container: a browser's audio engine
//! decodes WAVE everywhere, where AIFF is a gamble -- so a Blorb's
//! sampled sounds travel the protocol re-wrapped, their sample
//! points intact. The writing follows the canonical PCM WAVE
//! layout (RIFF: WAVE Audio File Format): one fmt chunk, one data
//! chunk, nothing else.

use crate::aiff::{BITS_PER_BYTE, Sound};

// The canonical PCM header: RIFF size counts everything after its
// own eight bytes, and the fmt chunk is the fixed sixteen of
// uncompressed PCM (format tag 1).
const PCM_FORMAT: u16 = 1;
const FMT_SIZE: u32 = 16;
const RIFF_TAIL: u32 = 36;

// WAVE stores 8-bit sample points unsigned, midpoint 0x80; wider
// points stay two's complement and turn little-endian.
const UNSIGNED_OFFSET: u8 = 0x80;

/// An AIFF-decoded sound re-wrapped as a complete WAVE file.
///
/// Sample points keep their values: 8-bit points move to WAVE's
/// unsigned convention, wider ones swap byte order, and both
/// formats left-justify a point in its whole bytes, so nothing is
/// rescaled. A fractional sample rate -- Lurking Horror plays at
/// values like 9676.2 -- rounds to the whole hertz the format
/// stores, and the listener's audio host resamples, exactly as the
/// speaker's does (§9 remarks; AIFF: Common Chunk).
pub fn riff(sound: &Sound) -> Vec<u8> {
    let width = u32::from(sound.sample_size).div_ceil(BITS_PER_BYTE);
    let data = little(&sound.samples, width as usize);
    let rate = (sound.sample_rate.round() as u32).max(1);
    let block = u32::from(sound.channels) * width;

    let mut held = Vec::with_capacity(44 + data.len());

    held.extend(b"RIFF");
    held.extend((RIFF_TAIL + data.len() as u32).to_le_bytes());
    held.extend(b"WAVE");
    held.extend(b"fmt ");
    held.extend(FMT_SIZE.to_le_bytes());
    held.extend(PCM_FORMAT.to_le_bytes());
    held.extend(sound.channels.to_le_bytes());
    held.extend(rate.to_le_bytes());
    held.extend((rate * block).to_le_bytes());
    held.extend((block as u16).to_le_bytes());
    held.extend(((width * BITS_PER_BYTE) as u16).to_le_bytes());
    held.extend(b"data");
    held.extend((data.len() as u32).to_le_bytes());
    held.extend(data);

    held
}

/// Big-endian signed sample points as WAVE stores them.
fn little(samples: &[u8], width: usize) -> Vec<u8> {
    if width == 1 {
        return samples
            .iter()
            .map(|point| point ^ UNSIGNED_OFFSET)
            .collect();
    }

    let mut turned = samples.to_vec();

    for point in turned.chunks_exact_mut(width) {
        point.reverse();
    }

    turned
}

#[cfg(test)]
mod tests {
    use super::*;

    // An 8-bit mono sound wraps into the canonical 44-byte header
    // -- PCM format, sizes counted, block align one byte -- with
    // its sample points moved to WAVE's unsigned convention,
    // values intact, and its fractional sample rate rounded to the
    // whole hertz the format stores.
    #[test]
    fn eight_bit_points_turn_unsigned() {
        let sound = Sound {
            channels: 1,
            sample_size: 8,
            sample_rate: 9676.2,
            frames: 4,
            samples: vec![0x00, 0x7F, 0x80, 0xFF],
        };

        let held = riff(&sound);

        assert_eq!(&held[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(held[4..8].try_into().unwrap()), 40);
        assert_eq!(&held[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes(held[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(held[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(held[24..28].try_into().unwrap()), 9676);
        assert_eq!(u32::from_le_bytes(held[28..32].try_into().unwrap()), 9676);
        assert_eq!(u16::from_le_bytes(held[32..34].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(held[34..36].try_into().unwrap()), 8);
        assert_eq!(&held[36..40], b"data");
        assert_eq!(u32::from_le_bytes(held[40..44].try_into().unwrap()), 4);
        assert_eq!(&held[44..], &[0x80, 0xFF, 0x00, 0x7F]);
    }

    // Wider sample points keep two's complement and swap byte
    // order, point by point, with the block align counting every
    // interleaved channel and the byte rate following from it.
    #[test]
    fn wider_points_swap_to_little_endian() {
        let sound = Sound {
            channels: 2,
            sample_size: 16,
            sample_rate: 8000.0,
            frames: 1,
            samples: vec![0x12, 0x34, 0xAB, 0xCD],
        };

        let held = riff(&sound);

        assert_eq!(u16::from_le_bytes(held[22..24].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(held[24..28].try_into().unwrap()), 8000);
        assert_eq!(u32::from_le_bytes(held[28..32].try_into().unwrap()), 32000);
        assert_eq!(u16::from_le_bytes(held[32..34].try_into().unwrap()), 4);
        assert_eq!(u16::from_le_bytes(held[34..36].try_into().unwrap()), 16);
        assert_eq!(&held[44..], &[0x34, 0x12, 0xCD, 0xAB]);
    }
}
