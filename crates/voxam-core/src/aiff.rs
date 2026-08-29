//! Reading AIFF sounds with nothing but this crate.
//!
//! Blorb resource files carry their sampled sounds as AIFF FORMs
//! (Blorb: Sound Resource Chunks), and playing them starts with
//! reading them -- done here by hand on top of the shared IFF
//! walker, following the Audio Interchange File Format 1.3
//! specification (Apple, 1989).
//!
//! The scope is the census of every sound in the vendored resource
//! files: 32 AIFF FORMs, all mono 8-bit samples, holding a COMM
//! chunk and an SSND chunk -- three of them with MARK and INST
//! sampler loops alongside, which are skipped the way AIFF tells
//! readers to skip chunks they have no use for (a Blorb sound's
//! looping comes from the sound_effect operand, not the
//! instrument). Compressed AIFF-C never appears there and is
//! refused with its name given.

use crate::errors::VoxamError;
use crate::iff::{IffChunk, parse_form};

// The FORM types: plain AIFF is the one Blorb sounds use, and
// AIFF-C adds compression codecs no vendored sound needs.
const SOUND_FORM: [u8; 4] = *b"AIFF";
const COMPRESSED_FORM: [u8; 4] = *b"AIFC";

// The Common Chunk: channels, sample frames, sample size in bits,
// and the sample rate as an 80-bit extended float -- exactly 18
// bytes, appearing exactly once (AIFF: Common Chunk).
const COMMON_ID: [u8; 4] = *b"COMM";
const COMMON_SIZE: usize = 18;
const FIELDS_SIZE: usize = 8;
const MIN_SAMPLE_SIZE: i16 = 1;
const MAX_SAMPLE_SIZE: i16 = 32;

// The Sound Data Chunk: an offset, a block size the offset already
// accounts for, then the sample frames (AIFF: Sound Data Chunk).
// It may only be omitted when there are no sample frames at all.
const SOUND_DATA_ID: [u8; 4] = *b"SSND";
const SOUND_DATA_HEADER_SIZE: usize = 8;

// The 80-bit extended float holding the sample rate: a sign bit,
// a 15-bit biased exponent, and a 64-bit mantissa whose integer
// bit is explicit (AIFF: Common Chunk).
const SIGN_BIT: u16 = 0x8000;
const EXPONENT_MASK: u16 = 0x7FFF;
const EXTENDED_BIAS: i32 = 16383;
const MANTISSA_SHIFT: i32 = 63;

pub(crate) const BITS_PER_BYTE: u32 = 8;

fn aiff_error(message: String) -> VoxamError {
    VoxamError::Aiff(message)
}

/// A decoded sound: its shape and its raw sample frames.
#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    /// How many interleaved channels each frame holds.
    pub channels: u16,
    /// Bits per sample point, 1 to 32.
    pub sample_size: u16,
    /// Sample frames per second.
    pub sample_rate: f64,
    /// How many sample frames the sound holds.
    pub frames: u32,
    /// The frames as stored: signed two's-complement sample
    /// points, each left-justified in as many whole bytes as its
    /// bits need (AIFF: Sound Data Chunk).
    pub samples: Vec<u8>,
}

impl Sound {
    /// The playing time in seconds.
    pub fn duration(&self) -> f64 {
        f64::from(self.frames) / self.sample_rate
    }
}

/// Decode AIFF bytes into a sound.
///
/// Fails if the bytes are not an AIFF FORM, are compressed AIFF-C,
/// or are internally inconsistent.
pub fn decode(data: &[u8]) -> Result<Sound, VoxamError> {
    let (form_type, chunks) = parse_form(data).map_err(|error| match error {
        VoxamError::Iff(message) => aiff_error(message),
        other => other,
    })?;

    if form_type == COMPRESSED_FORM {
        return Err(aiff_error(
            "this sound is compressed AIFF-C, whose codecs are outside the plain \
             AIFF every Blorb sound uses"
                .into(),
        ));
    }

    if form_type != SOUND_FORM {
        return Err(aiff_error(format!(
            "the FORM type is {:?}, not the AIFF of a sound",
            String::from_utf8_lossy(&form_type)
        )));
    }

    let common: Vec<&IffChunk> = chunks
        .iter()
        .filter(|piece| piece.chunk_id == COMMON_ID)
        .collect();

    if common.len() != 1 {
        return Err(aiff_error(format!(
            "an AIFF holds exactly one COMM chunk; this one has {} (AIFF: Common \
             Chunk)",
            common.len()
        )));
    }

    let (channels, frames, sample_size, sample_rate) = decode_common(&common[0].payload)?;
    let samples = extract_samples(&chunks, frames, channels, sample_size)?;

    Ok(Sound {
        channels,
        sample_size,
        sample_rate,
        frames,
        samples,
    })
}

/// Decode the COMM chunk's four fields. Fails if the chunk is not
/// its fixed 18 bytes, or a field's value is outside what AIFF
/// allows.
fn decode_common(payload: &[u8]) -> Result<(u16, u32, u16, f64), VoxamError> {
    if payload.len() != COMMON_SIZE {
        return Err(aiff_error(format!(
            "a COMM chunk is exactly {COMMON_SIZE} bytes, but this one holds {} \
             (AIFF: Common Chunk)",
            payload.len()
        )));
    }

    let channels = i16::from_be_bytes([payload[0], payload[1]]);
    let frames = u32::from_be_bytes([payload[2], payload[3], payload[4], payload[5]]);
    let sample_size = i16::from_be_bytes([payload[6], payload[7]]);

    if channels < 1 {
        return Err(aiff_error(format!(
            "a sound needs at least one channel, not {channels}"
        )));
    }

    if !(MIN_SAMPLE_SIZE..=MAX_SAMPLE_SIZE).contains(&sample_size) {
        return Err(aiff_error(format!(
            "a sample point is {MIN_SAMPLE_SIZE} to {MAX_SAMPLE_SIZE} bits, not \
             {sample_size} (AIFF: Common Chunk)"
        )));
    }

    let rate = decode_rate(&payload[FIELDS_SIZE..])?;

    Ok((channels as u16, frames, sample_size as u16, rate))
}

/// Decode the sample rate's 80-bit extended float. Fails if the
/// value is not a positive finite number a sound could play at.
fn decode_rate(raw: &[u8]) -> Result<f64, VoxamError> {
    let sign_exponent = u16::from_be_bytes([raw[0], raw[1]]);
    let mantissa = u64::from_be_bytes(raw[2..10].try_into().expect("eight bytes"));
    let exponent = sign_exponent & EXPONENT_MASK;
    let complaint = "the sample rate must be a positive finite number";

    if sign_exponent & SIGN_BIT != 0 || exponent == EXPONENT_MASK {
        return Err(aiff_error(complaint.into()));
    }

    // mantissa * 2^(exponent - bias - 63), the reference's ldexp;
    // an overflow lands at infinity and fails the finite check the
    // way Python's OverflowError raises.
    let rate = (mantissa as f64)
        * f64::exp2(f64::from(
            i32::from(exponent) - EXTENDED_BIAS - MANTISSA_SHIFT,
        ));

    if !rate.is_finite() || rate <= 0.0 {
        return Err(aiff_error(complaint.into()));
    }

    Ok(rate)
}

/// Extract the sample frames from at most one SSND chunk. Fails
/// for a doubled SSND, one missing while frames remain to store,
/// one shorter than its own header, or one holding fewer bytes
/// than the frames need.
fn extract_samples(
    chunks: &[IffChunk],
    frames: u32,
    channels: u16,
    sample_size: u16,
) -> Result<Vec<u8>, VoxamError> {
    let sound_data: Vec<&IffChunk> = chunks
        .iter()
        .filter(|piece| piece.chunk_id == SOUND_DATA_ID)
        .collect();

    if sound_data.len() > 1 {
        return Err(aiff_error(format!(
            "an AIFF holds at most one SSND chunk; this one has {} (AIFF: Sound \
             Data Chunk)",
            sound_data.len()
        )));
    }

    let Some(found) = sound_data.first() else {
        if frames != 0 {
            return Err(aiff_error(format!(
                "{frames} sample frames are promised, but no SSND chunk holds them \
                 (AIFF: Sound Data Chunk)"
            )));
        }

        return Ok(Vec::new());
    };

    let payload = &found.payload;

    if payload.len() < SOUND_DATA_HEADER_SIZE {
        return Err(aiff_error(format!(
            "an SSND chunk starts with {SOUND_DATA_HEADER_SIZE} bytes of offset and \
             block size, but this one holds only {} (AIFF: Sound Data Chunk)",
            payload.len()
        )));
    }

    let offset = u32::from_be_bytes(payload[..4].try_into().expect("four bytes")) as usize;
    let width = (u32::from(sample_size)).div_ceil(BITS_PER_BYTE) as usize;
    let needed = frames as usize * usize::from(channels) * width;
    let start = (SOUND_DATA_HEADER_SIZE + offset).min(payload.len());
    let region = &payload[start..];

    if region.len() < needed {
        return Err(aiff_error(format!(
            "{frames} frames of {channels} channel(s) at {width} byte(s) each need \
             {needed} bytes, but the SSND chunk offers {} (AIFF: Sound Data Chunk)",
            region.len()
        )));
    }

    Ok(region[..needed].to_vec())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::iff::write_form;

    /// The 80-bit extended spelling of a rate, frexp's way.
    pub(crate) fn extended(value: f64) -> Vec<u8> {
        // frexp: value = mantissa * 2^exponent with 0.5 <= m < 1.
        let exponent = value.log2().floor() as i32 + 1;
        let mantissa = value * f64::exp2(f64::from(-exponent));
        let mut held = ((exponent - 1 + 16383) as u16).to_be_bytes().to_vec();

        held.extend(((mantissa * f64::exp2(64.0)) as u64).to_be_bytes());

        held
    }

    pub(crate) fn comm(channels: i16, frames: u32, bits: i16, rate: f64) -> IffChunk {
        comm_raw(channels, frames, bits, &extended(rate))
    }

    fn comm_raw(channels: i16, frames: u32, bits: i16, rate_bytes: &[u8]) -> IffChunk {
        let mut payload = channels.to_be_bytes().to_vec();

        payload.extend(frames.to_be_bytes());
        payload.extend(bits.to_be_bytes());
        payload.extend(rate_bytes);

        piece(*b"COMM", payload)
    }

    pub(crate) fn ssnd(samples: &[u8], offset: u32) -> IffChunk {
        let mut payload = offset.to_be_bytes().to_vec();

        payload.extend(0u32.to_be_bytes());
        payload.extend(std::iter::repeat_n(0xEEu8, offset as usize));
        payload.extend(samples);

        piece(*b"SSND", payload)
    }

    fn piece(chunk_id: [u8; 4], payload: Vec<u8>) -> IffChunk {
        IffChunk {
            chunk_id,
            payload,
            offset: 0,
        }
    }

    pub(crate) fn sound_bytes(chunks: &[IffChunk], form_type: &[u8; 4]) -> Vec<u8> {
        write_form(form_type, chunks)
    }

    // The Infocom shape -- mono 8-bit COMM and SSND, nothing else
    // -- decodes to its frames exactly, and a whole second of
    // frames at the sample rate is a duration of one.
    #[test]
    fn the_infocom_shape_decodes() {
        let raw: Vec<u8> = (0u8..150).collect::<Vec<u8>>().repeat(147);
        let sound = decode(&sound_bytes(
            &[comm(1, 22050, 8, 22050.0), ssnd(&raw, 0)],
            b"AIFF",
        ))
        .unwrap();

        assert_eq!(sound.channels, 1);
        assert_eq!(sound.sample_size, 8);
        assert_eq!(sound.sample_rate, 22050.0);
        assert_eq!(sound.frames, 22050);
        assert_eq!(sound.samples, raw);
        assert_eq!(sound.duration(), 1.0);
    }

    // The 80-bit extended float carries fractional rates whole --
    // the Lurking Horror sounds play at rates like 9676.2, not
    // round numbers (AIFF: Common Chunk).
    #[test]
    fn a_fractional_sample_rate_survives() {
        let sound = decode(&sound_bytes(
            &[comm(1, 2, 8, 11025.5), ssnd(&[1, 2], 0)],
            b"AIFF",
        ))
        .unwrap();

        assert_eq!(sound.sample_rate, 11025.5);
    }

    // MARK and INST sampler loops ride along in some sounds; a
    // reader skips chunks it has no use for, as AIFF instructs.
    #[test]
    fn sampler_loop_chunks_are_skipped() {
        let sound = decode(&sound_bytes(
            &[
                comm(1, 3, 8, 22050.0),
                piece(*b"MARK", vec![0, 0]),
                piece(*b"INST", vec![0; 20]),
                ssnd(&[1, 2, 3], 0),
            ],
            b"AIFF",
        ))
        .unwrap();

        assert_eq!(sound.samples, vec![1, 2, 3]);
    }

    // The SSND offset pushes the first frame past alignment
    // padding, and block padding after the last frame is not
    // sample data (AIFF: Sound Data Chunk).
    #[test]
    fn the_ssnd_offset_and_block_padding_are_stepped_around() {
        let sound = decode(&sound_bytes(
            &[
                comm(1, 2, 8, 22050.0),
                ssnd(&[0x0A, 0x0B, 0xEE, 0xEE, 0xEE], 4),
            ],
            b"AIFF",
        ))
        .unwrap();

        assert_eq!(sound.samples, vec![0x0A, 0x0B]);
    }

    // A sound promising no frames may omit its SSND chunk entirely
    // (AIFF: Sound Data Chunk).
    #[test]
    fn a_frameless_sound_needs_no_ssnd() {
        let sound = decode(&sound_bytes(&[comm(1, 0, 8, 22050.0)], b"AIFF")).unwrap();

        assert!(sound.samples.is_empty());
        assert_eq!(sound.duration(), 0.0);
    }

    // A sample point takes as many whole bytes as its bits need:
    // 16 bits is two, and so is 12 (AIFF: Sound Data Chunk).
    #[test]
    fn wide_and_packed_sample_points_take_whole_bytes() {
        let stereo = decode(&sound_bytes(
            &[comm(2, 2, 16, 22050.0), ssnd(&[0; 8], 0)],
            b"AIFF",
        ))
        .unwrap();
        let packed = decode(&sound_bytes(
            &[comm(1, 3, 12, 22050.0), ssnd(&[0; 6], 0)],
            b"AIFF",
        ))
        .unwrap();

        assert_eq!(stereo.samples.len(), 8);
        assert_eq!(packed.samples.len(), 6);
    }

    fn refused(data: &[u8], complaint: &str) {
        let error = decode(data).unwrap_err().to_string();

        assert!(
            error.contains(complaint),
            "expected {complaint:?} in {error:?}"
        );
    }

    #[test]
    fn unusable_sounds_are_refused() {
        let rate_packed = |sign_exponent: u16, mantissa: u64| {
            let mut held = sign_exponent.to_be_bytes().to_vec();

            held.extend(mantissa.to_be_bytes());

            held
        };

        refused(b"RIFF but not a FORM", "not an IFF file");
        refused(&sound_bytes(&[comm(1, 0, 8, 22050.0)], b"AIFC"), "AIFF-C");
        refused(
            &sound_bytes(&[comm(1, 0, 8, 22050.0)], b"IFZS"),
            "not the AIFF",
        );
        refused(&sound_bytes(&[ssnd(b"", 0)], b"AIFF"), "exactly one COMM");
        refused(
            &sound_bytes(&[comm(1, 0, 8, 22050.0), comm(1, 0, 8, 22050.0)], b"AIFF"),
            "exactly one COMM",
        );
        refused(
            &sound_bytes(&[piece(*b"COMM", vec![0; 17])], b"AIFF"),
            "exactly 18 bytes",
        );
        refused(
            &sound_bytes(&[comm(0, 0, 8, 22050.0)], b"AIFF"),
            "at least one channel",
        );
        refused(&sound_bytes(&[comm(1, 0, 0, 22050.0)], b"AIFF"), "1 to 32");
        refused(&sound_bytes(&[comm(1, 0, 33, 22050.0)], b"AIFF"), "1 to 32");
        refused(
            &sound_bytes(
                &[comm_raw(1, 0, 8, &rate_packed(0x8000 | 16397, 1 << 63))],
                b"AIFF",
            ),
            "positive finite",
        );
        refused(
            &sound_bytes(&[comm_raw(1, 0, 8, &rate_packed(0x7FFF, 0))], b"AIFF"),
            "positive finite",
        );
        refused(
            &sound_bytes(&[comm_raw(1, 0, 8, &[0; 10])], b"AIFF"),
            "positive finite",
        );
        refused(
            &sound_bytes(&[comm_raw(1, 0, 8, &rate_packed(0x7FFE, 1 << 63))], b"AIFF"),
            "positive finite",
        );
        refused(
            &sound_bytes(
                &[comm(1, 0, 8, 22050.0), ssnd(b"", 0), ssnd(b"", 0)],
                b"AIFF",
            ),
            "at most one SSND",
        );
        refused(
            &sound_bytes(&[comm(1, 5, 8, 22050.0)], b"AIFF"),
            "no SSND chunk holds them",
        );
        refused(
            &sound_bytes(
                &[comm(1, 0, 8, 22050.0), piece(*b"SSND", vec![0; 4])],
                b"AIFF",
            ),
            "offset and block size",
        );
        refused(
            &sound_bytes(&[comm(1, 4, 8, 22050.0), ssnd(&[1, 2], 0)], b"AIFF"),
            "offers 2",
        );
    }
}
