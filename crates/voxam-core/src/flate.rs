//! Inflate, deflate, and their checksums, spelled by hand
//! (RFC 1950, RFC 1951).
//!
//! The reference leans on Python's stdlib `zlib` for the PNG art
//! the Blorb corpus carries; Rust's stdlib carries no such thing,
//! so this module is the port's own zlib corner. The inflate is a
//! full RFC 1951 reader -- stored, fixed, and dynamic blocks --
//! because the corpus art was compressed by whatever tool packaged
//! it. The deflate is the reference's own `_deflated` (png.py),
//! ported move for move: one final block under the fixed Huffman
//! codes, matches found greedily at the last place the next three
//! bytes stood. The reference retired `zlib.compress` because its
//! bytes vary by the library behind it -- madler zlib and zlib-ng
//! disagree -- and the wire these streams ride is certified byte
//! for byte.

use std::collections::HashMap;

/// The reference's `zlib.crc32`, chained: the standard CRC-32 over
/// the polynomial IFF's world shares with zlib and PNG.
pub fn crc32(data: &[u8], running: u32) -> u32 {
    let mut crc = !running;

    for &byte in data {
        crc ^= u32::from(byte);

        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }

    !crc
}

/// The reference's `zlib.adler32`: the checksum a zlib stream ends
/// with (RFC 1950 8.2).
pub fn adler32(data: &[u8]) -> u32 {
    let mut low: u32 = 1;
    let mut high: u32 = 0;

    for &byte in data {
        low = (low + u32::from(byte)) % 65_521;
        high = (high + low) % 65_521;
    }

    (high << 16) | low
}

// The deflate stream `deflated` writes: matches no shorter than
// three bytes and no longer than the format allows, found no
// further back than the window reaches (RFC 1951 3.2.3).
const WINDOW: usize = 32_768;
const LEAST_MATCH: usize = 3;
const MOST_MATCH: usize = 258;
const END_OF_BLOCK: u16 = 256;

// Each length symbol's first length and its extra bits, then each
// distance symbol's first distance and its extra bits (RFC 1951
// 3.2.5).
const LENGTH_STARTS: [usize; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRAS: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_STARTS: [usize; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12_289, 16_385, 24_577,
];
const DISTANCE_EXTRAS: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Deflate's bitstream (RFC 1951 3.1.1).
///
/// Data elements ride least significant bit first; Huffman codes
/// ride most significant bit first, so `code` reverses them on the
/// way in.
struct Writer {
    bytes: Vec<u8>,
    held: u32,
    count: u32,
}

impl Writer {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            held: 0,
            count: 0,
        }
    }

    /// Write `width` bits of `value`, least significant first.
    fn bits(&mut self, value: u32, width: u32) {
        self.held |= value << self.count;
        self.count += width;

        while self.count >= 8 {
            self.bytes.push((self.held & 0xFF) as u8);
            self.held >>= 8;
            self.count -= 8;
        }
    }

    /// Write one Huffman code, most significant bit first.
    fn code(&mut self, value: u32, width: u32) {
        let mut value = value;
        let mut told = 0;

        for _ in 0..width {
            told = (told << 1) | (value & 1);
            value >>= 1;
        }

        self.bits(told, width);
    }

    /// The stream, its last partial byte padded with zeros.
    fn flushed(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.bytes.push((self.held & 0xFF) as u8);
        }

        self.bytes
    }
}

/// A zlib stream whose bytes are the same on every build.
///
/// The reference's `_deflated` (png.py), move for move: the zlib
/// dress (RFC 1950) around one final deflate block under the fixed
/// Huffman codes, matches found greedily at the last place the
/// next three bytes stood (RFC 1951 3.2.6).
pub fn deflated(data: &[u8]) -> Vec<u8> {
    let mut writer = Writer::new();

    writer.bits(1, 1);
    writer.bits(1, 2);

    let mut table: HashMap<[u8; 3], usize> = HashMap::new();
    let mut position = 0;

    while position < data.len() {
        let (mut length, start) = matched(data, position, &table);

        if length > 0 {
            length_coded(&mut writer, length);
            distance_coded(&mut writer, position - start);
        } else {
            length = 1;

            symbol(&mut writer, u16::from(data[position]));
        }

        remembered(data, position, length, &mut table);

        position += length;
    }

    symbol(&mut writer, END_OF_BLOCK);

    let mut out = vec![0x78, 0x01];

    out.extend_from_slice(&writer.flushed());
    out.extend_from_slice(&adler32(data).to_be_bytes());

    out
}

/// The longest match at the last place these three bytes stood.
///
/// Zero for none: the tail too short to hold a match, bytes never
/// seen, or a stand beyond the window's reach. A match may run
/// into itself -- distance one, length many, is how a run spells
/// itself (RFC 1951 3.2.3).
fn matched(data: &[u8], position: usize, table: &HashMap<[u8; 3], usize>) -> (usize, usize) {
    if position + LEAST_MATCH > data.len() {
        return (0, 0);
    }

    let key = [data[position], data[position + 1], data[position + 2]];
    let Some(&prior) = table.get(&key) else {
        return (0, 0);
    };

    if position - prior > WINDOW {
        return (0, 0);
    }

    let most = MOST_MATCH.min(data.len() - position);
    let mut length = LEAST_MATCH;

    while length < most && data[prior + length] == data[position + length] {
        length += 1;
    }

    (length, prior)
}

/// Each covered position becomes its three bytes' last stand.
fn remembered(data: &[u8], position: usize, length: usize, table: &mut HashMap<[u8; 3], usize>) {
    for held in position..position + length {
        if held + LEAST_MATCH <= data.len() {
            table.insert([data[held], data[held + 1], data[held + 2]], held);
        }
    }
}

/// One literal-or-length symbol, fixed codes (RFC 1951 3.2.6).
fn symbol(writer: &mut Writer, symbol: u16) {
    let held = u32::from(symbol);

    match symbol {
        0..=143 => writer.code(0x30 + held, 8),
        144..=255 => writer.code(0x190 + held - 144, 9),
        256..=279 => writer.code(held - 256, 7),
        _ => writer.code(0xC0 + held - 280, 8),
    }
}

/// A match length: its symbol, then its extra bits.
fn length_coded(writer: &mut Writer, length: usize) {
    let mut told = LENGTH_STARTS.len() - 1;

    while LENGTH_STARTS[told] > length {
        told -= 1;
    }

    symbol(writer, 257 + told as u16);

    if LENGTH_EXTRAS[told] > 0 {
        writer.bits((length - LENGTH_STARTS[told]) as u32, LENGTH_EXTRAS[told]);
    }
}

/// A match distance: its five-bit code, then its extra bits.
fn distance_coded(writer: &mut Writer, distance: usize) {
    let mut told = DISTANCE_STARTS.len() - 1;

    while DISTANCE_STARTS[told] > distance {
        told -= 1;
    }

    writer.code(told as u32, 5);

    if DISTANCE_EXTRAS[told] > 0 {
        writer.bits(
            (distance - DISTANCE_STARTS[told]) as u32,
            DISTANCE_EXTRAS[told],
        );
    }
}

/// Inflate's bitstream: bits arrive least significant first, and
/// between calls at most seven ride in `held`, so the reader's
/// byte position never runs ahead of what was consumed.
struct Reader<'a> {
    data: &'a [u8],
    position: usize,
    held: u32,
    count: u32,
}

impl Reader<'_> {
    fn bits(&mut self, width: u32) -> Result<u32, String> {
        while self.count < width {
            let Some(&byte) = self.data.get(self.position) else {
                return Err("the stream ends mid-block".to_string());
            };

            self.position += 1;
            self.held |= u32::from(byte) << self.count;
            self.count += 8;
        }

        let value = self.held & ((1 << width) - 1);

        self.held >>= width;
        self.count -= width;

        Ok(value)
    }

    /// Drop the partial byte a stored block's header must not
    /// straddle (RFC 1951 3.2.4).
    fn aligned(&mut self) {
        self.held = 0;
        self.count = 0;
    }
}

/// One canonical Huffman code, built from its symbols' lengths and
/// decoded a bit at a time (RFC 1951 3.2.2).
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn built(lengths: &[u16]) -> Result<Self, String> {
        let mut counts = [0u16; 16];

        for &length in lengths {
            counts[usize::from(length)] += 1;
        }

        counts[0] = 0;

        let mut left = 1i32;

        for &count in &counts[1..] {
            left = (left << 1) - i32::from(count);

            if left < 0 {
                return Err("the code lengths oversubscribe a Huffman table".to_string());
            }
        }

        let mut offsets = [0usize; 16];

        for length in 1..15 {
            offsets[length + 1] = offsets[length] + usize::from(counts[length]);
        }

        let mut symbols = vec![0u16; lengths.len()];

        for (held, &length) in lengths.iter().enumerate() {
            if length > 0 {
                symbols[offsets[usize::from(length)]] = held as u16;
                offsets[usize::from(length)] += 1;
            }
        }

        Ok(Self { counts, symbols })
    }

    fn decoded(&self, reader: &mut Reader) -> Result<u16, String> {
        let mut code: u32 = 0;
        let mut first: u32 = 0;
        let mut index: usize = 0;

        for length in 1..16 {
            code |= reader.bits(1)?;

            let count = u32::from(self.counts[length]);

            if code < first + count {
                return Ok(self.symbols[index + (code - first) as usize]);
            }

            index += count as usize;
            first = (first + count) << 1;
            code <<= 1;
        }

        Err("a Huffman code matches no symbol".to_string())
    }
}

/// The reference's `zlib.decompress`: a zlib stream inflated whole,
/// its dress and checksum verified (RFC 1950; RFC 1951).
///
/// The refusal reasons are this port's own words -- the reference
/// hears whatever its zlib build says -- so callers fold them into
/// their own error sentences.
pub fn inflated(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 6 {
        return Err("the stream ends before its zlib dress is whole".to_string());
    }

    if data[0] & 0x0F != 8 {
        return Err("the stream does not name the deflate method".to_string());
    }

    if (u32::from(data[0]) * 256 + u32::from(data[1])) % 31 != 0 {
        return Err("the stream's header check fails".to_string());
    }

    if data[1] & 0x20 != 0 {
        return Err("the stream asks for a preset dictionary".to_string());
    }

    let mut reader = Reader {
        data,
        position: 2,
        held: 0,
        count: 0,
    };
    let mut out: Vec<u8> = Vec::new();

    loop {
        let last = reader.bits(1)?;
        let kind = reader.bits(2)?;

        match kind {
            0 => stored_block(&mut reader, &mut out)?,
            1 => {
                let (literals, distances) = fixed_tables();

                inflated_block(&mut reader, &mut out, &literals, &distances)?;
            }
            2 => {
                let (literals, distances) = dynamic_tables(&mut reader)?;

                inflated_block(&mut reader, &mut out, &literals, &distances)?;
            }
            _ => return Err("the reserved block type appeared".to_string()),
        }

        if last == 1 {
            break;
        }
    }

    let Some(told) = data
        .get(reader.position..reader.position + 4)
        .map(|held| u32::from_be_bytes([held[0], held[1], held[2], held[3]]))
    else {
        return Err("the stream ends before its checksum".to_string());
    };

    if told != adler32(&out) {
        return Err("the checksum does not match the inflated bytes".to_string());
    }

    Ok(out)
}

/// A stored block: aligned, length-checked, and copied whole
/// (RFC 1951 3.2.4).
fn stored_block(reader: &mut Reader, out: &mut Vec<u8>) -> Result<(), String> {
    reader.aligned();

    let length = reader.bits(16)?;
    let check = reader.bits(16)?;

    if length ^ 0xFFFF != check {
        return Err("a stored block's length check fails".to_string());
    }

    for _ in 0..length {
        out.push(reader.bits(8)? as u8);
    }

    Ok(())
}

/// The fixed literal and distance codes (RFC 1951 3.2.6).
fn fixed_tables() -> (Huffman, Huffman) {
    let mut lengths = [8u16; 288];

    lengths[144..256].fill(9);
    lengths[256..280].fill(7);

    let literals = Huffman::built(&lengths).expect("the fixed literal code is well formed");
    let distances = Huffman::built(&[5u16; 30]).expect("the fixed distance code is well formed");

    (literals, distances)
}

/// A dynamic block's two codes, read through the code-length code
/// (RFC 1951 3.2.7).
fn dynamic_tables(reader: &mut Reader) -> Result<(Huffman, Huffman), String> {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];

    let literals = reader.bits(5)? as usize + 257;
    let distances = reader.bits(5)? as usize + 1;
    let classes = reader.bits(4)? as usize + 4;

    if literals > 286 || distances > 30 {
        return Err("the dynamic header claims more codes than exist".to_string());
    }

    let mut class_lengths = [0u16; 19];

    for &seat in &ORDER[..classes] {
        class_lengths[seat] = reader.bits(3)? as u16;
    }

    let class_code = Huffman::built(&class_lengths)?;
    let mut lengths: Vec<u16> = Vec::with_capacity(literals + distances);

    while lengths.len() < literals + distances {
        let told = class_code.decoded(reader)?;

        match told {
            0..=15 => lengths.push(told),
            16 => {
                let Some(&previous) = lengths.last() else {
                    return Err("a code-length repeat has nothing to repeat".to_string());
                };
                let times = reader.bits(2)? as usize + 3;

                lengths.extend(std::iter::repeat_n(previous, times));
            }
            17 => {
                let times = reader.bits(3)? as usize + 3;

                lengths.extend(std::iter::repeat_n(0, times));
            }
            _ => {
                let times = reader.bits(7)? as usize + 11;

                lengths.extend(std::iter::repeat_n(0, times));
            }
        }
    }

    if lengths.len() > literals + distances {
        return Err("the code lengths overrun their count".to_string());
    }

    Ok((
        Huffman::built(&lengths[..literals])?,
        Huffman::built(&lengths[literals..])?,
    ))
}

/// One block's symbols: literals pushed, matches copied back over
/// themselves where they overlap (RFC 1951 3.2.3).
fn inflated_block(
    reader: &mut Reader,
    out: &mut Vec<u8>,
    literals: &Huffman,
    distances: &Huffman,
) -> Result<(), String> {
    loop {
        let told = literals.decoded(reader)?;

        if told < 256 {
            out.push(told as u8);

            continue;
        }

        if told == END_OF_BLOCK {
            return Ok(());
        }

        let seat = usize::from(told) - 257;

        if seat >= LENGTH_STARTS.len() {
            return Err("a length symbol past the defined range appeared".to_string());
        }

        let length = LENGTH_STARTS[seat] + reader.bits(LENGTH_EXTRAS[seat])? as usize;
        let coded = usize::from(distances.decoded(reader)?);

        if coded >= DISTANCE_STARTS.len() {
            return Err("a distance symbol past the defined range appeared".to_string());
        }

        let distance = DISTANCE_STARTS[coded] + reader.bits(DISTANCE_EXTRAS[coded])? as usize;

        if distance > out.len() {
            return Err("a match reaches back before the stream began".to_string());
        }

        for _ in 0..length {
            out.push(out[out.len() - distance]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhexed(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|held| u8::from_str_radix(&text[held..held + 2], 16).expect("hex"))
            .collect()
    }

    // The checksums speak zlib's: the CRC check value for
    // "123456789" is the classic CBF43926, and Adler-32 opens at
    // one (RFC 1950 8.2).
    #[test]
    fn the_checksums_speak_zlib() {
        assert_eq!(crc32(b"", 0), 0);
        assert_eq!(crc32(b"123456789", 0), 0xCBF4_3926);
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"123456789"), 0x091E_01DE);
    }

    // The deflate's bytes are the reference's own, vector for
    // vector: `_deflated` in the patched png.py produced these,
    // and the port must spell them identically -- an empty stream,
    // bare literals, a self-overlapping run, matched prose, and a
    // spread of every byte value.
    #[test]
    fn deflated_matches_the_reference_vectors() {
        assert_eq!(deflated(b""), unhexed("7801030000000001"));
        assert_eq!(deflated(b"abc"), unhexed("78014b4c4a0600024d0127"));
        assert_eq!(deflated(&[0u8; 300]), unhexed("7801631805440300012c0001"));

        // Six distinct nine-bit literals close the stream exactly
        // on a byte boundary: 3 + 8 + 9 * 6 + 7 bits, no padding.
        assert_eq!(
            deflated(&[0, 200, 201, 202, 203, 204, 205]),
            unhexed("7801633871f2d4e9336701109204c0")
        );

        let lorem = b"the quick brown fox jumps over the lazy dog, \
the quick brown fox jumps over the lazy dog.";

        assert_eq!(
            deflated(lorem),
            unhexed(concat!(
                "78012bc94855282ccd4cce56482aca2fcf5348cbaf50c82acd2d2856c82f",
                "4b2d520049e72456552aa4e4a7eb8079442ad60300bf15206d"
            ))
        );

        let spread: Vec<u8> = (0..=255u8).cycle().take(768).collect();

        assert_eq!(
            deflated(&spread),
            unhexed(concat!(
                "78016360646266616563e7e0e4e2e6e1e5e31710141216111513979094929691",
                "95935750545256515553d7d0d4d2d6d1d5d33730343236313533b7b0b4b2b6b1",
                "b5b37770747276717573f7f0f4f2f6f1f5f30f080c0a0e090d0b8f888c8a8e89",
                "8d8b4f484c4a4e494d4bcfc8cccacec9cdcb2f282c2a2e292d2bafa8acaaaea9",
                "adab6f686c6a6e696d6befe8eceaeee9edeb9f3071d2e42953a74d9f3173d6ec",
                "3973e7cd5fb070d1e2254b972d5fb172d5ea356bd7addfb071d3e62d5bb76ddf",
                "b173d7ee3d7bf7ed3f70f0d0e123478f1d3f71f2d4e93367cf9dbf70f1d2e52b",
                "57af5dbf71f3d6ed3b77efdd7ff0f0d1e3274f9f3d7ff1f2d5eb376fdfbdfff0",
                "f1d3e72f5fbf7dfff1f3d7ef3f7ffffd1ff5ffc8f63f00a0627e90"
            ))
        );
    }

    // Whatever deflated spells, inflated reads back whole: empty,
    // short, runs, window-crossing repeats, and every byte value.
    #[test]
    fn deflate_and_inflate_round_trip() {
        let cases: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"a".to_vec(),
            b"abc".to_vec(),
            vec![0u8; 300],
            (0..=255u8).cycle().take(768).collect(),
            {
                // A 997-byte phrase repeated far past the window,
                // so late matches must reach 32K back and self-
                // overlapping runs appear along the way.
                let phrase: Vec<u8> = (0..997u32).map(|held| (held % 251) as u8).collect();
                let mut held = Vec::new();

                for _ in 0..40 {
                    held.extend_from_slice(&phrase);
                }

                held.extend(std::iter::repeat_n(7u8, 1000));
                held
            },
        ];

        for data in cases {
            assert_eq!(inflated(&deflated(&data)).expect("inflates"), data);
        }
    }

    // Streams other tools compressed inflate too -- every vector
    // generated from the reference environment, never guessed. A
    // 256-byte phrase zlib-ng spelled with the fixed codes, the
    // same phrase at level zero as one stored block, and a longer
    // text zlib-ng spelled with a dynamic-Huffman block -- whose
    // inflated content the stream's own verified Adler-32 vouches
    // for.
    #[test]
    fn foreign_streams_inflate() {
        let sample: Vec<u8> = (b"a season of mists and mellow fruitfulness "
            .iter()
            .copied()
            .cycle()
            .take(256))
        .collect();

        let zng = unhexed(concat!(
            "789c4b54284e4d2ccecf53c84f53c8cd2c2e295648cc4b51c84dcdc9c92f5748",
            "2b2acd2c492bcdc94b2d2e56481c962a01faae5fc2"
        ));

        assert_eq!(
            inflated(&zng).expect("the fixed-code stream inflates"),
            sample
        );

        let mut stored = vec![0x78u8, 0x01, 0x01, 0x00, 0x01, 0xFF, 0xFE];

        stored.extend_from_slice(&sample);
        stored.extend_from_slice(&adler32(&sample).to_be_bytes());

        assert_eq!(
            inflated(&stored).expect("the stored stream inflates"),
            sample
        );

        let dynamic = unhexed(concat!(
            "789c75976b6ee3300c84afe2aba55b6f1bc07680b8eea23dfdc27c48dfc8",
            "cc9f248e248a8f9921fdf1389eefd37eacebfc9c3e6fbff3920f6fcffbbaceef",
            "d39f7959f6e9c3f6f9a76cdb3fe76599d679591effa6bfcfe3fe15277c979f78",
            "9be73d76faa26fdce7dbfed862cf7e6cfab71bf0fdcd99e5b6ae3f6e6fbdef5f",
            "79fd634f176d29b6f9a2fd13967db71d9490fd465bf00f8f27af8d471c6756be",
            "efdb9c41d88eb01da7fc8ab46899383268b3e5ff31176d9705e606d297ccbbef",
            "cf488fcddd70bfe466b36321fb11d621033fafc9e5b09fb73337098d96feb868",
            "48574bdbb925023d4d85b3f65baeec750a4b51f54c8385d6fc8b6777c0ec0a46",
            "629f573ced459ac63abb732ca61fb39ba57084be1a25a2cd1d0f84b96b0167be",
            "3c992dd9518c00cfa609146eb959520fdc6278127e6718c0eb673cb575cdf54b",
            "48c062818866cd4dda3373ca7f2482580ed29cf7a308b988a4841f85fe24c044",
            "8d2019572a7ac1c2b27098a5237b9063f04b11019cf79caaac6a3652ed227055",
            "bb0ad08463d004e99010e0df85a09a3686996e077d5169c0ab713132c172ba1d",
            "5bd5a8209a14ebf82a2c75d52d8425ff72f5eb6ad85d254da4e1d40a2aeecb01",
            "778d17895343d5f48b9d515ca6b4998f1ad628ca9d23a1f49065c2778c1c9a3b",
            "ae976467ba847cd071576112486326c4f959b55e36fb6069d3487a89b6594a3b",
            "051de10f0c1f25364430390649954efab288c51d5ac630838cb50e3f8a919be7",
            "e7606478645f8988e99a68a17f8280a32805beac49485ed1ac8ad9ad48ab8874",
            "471f6418a38e9297502786740ae89ec2376a0507c106a356695f064efa2174da",
            "abfac511344c4e2c44c88bd98e04837ec39f8b24e894a56c6fa0a52471a00966",
            "61551ac96900d7ca6e7fa0225f661d1960b2d50e1840277da19568fa0414b951",
            "ce96188f0bb25353c24f8cea9db8ea670943cca37573ae07619dabeb579a8ec8",
            "c13010ce1e8bf9038184667601e9ef18d530a9284681005daf079a1e209a3659",
            "33ee1f62d1174b10533d6403d3f4ca6b0d71dd47b0ac146f049d75bba81e6d24",
            "e3386657831bd823b278f10afa45c12e9adff83e8b11133f055a82f06b9b5444",
            "8b6e6bec7847d3d77e0572573bb2d4ed5e8781216fe3f765a672cffe03c8e932",
            "fd"
        ));
        let told = inflated(&dynamic).expect("the dynamic stream inflates");

        assert_eq!(told.len(), 4241);
        assert!(told.starts_with(b"gourd summer hazel summer brimmed cells"));
    }

    // What cannot be a zlib stream is refused with the reason
    // given: a missing dress, a wrong method, a failed header
    // check, a preset dictionary, the reserved block type, a
    // truncated block, a wrong checksum, and a stored block whose
    // length check fails.
    #[test]
    fn broken_streams_are_refused() {
        let complaint = |data: &[u8]| inflated(data).expect_err("refused");

        assert!(complaint(b"x").contains("dress"));
        assert!(complaint(&[0x77, 0x01, 0, 0, 0, 0]).contains("deflate method"));
        assert!(complaint(&[0x78, 0x02, 0, 0, 0, 0]).contains("header check"));
        assert!(complaint(&[0x78, 0x20, 0, 0, 0, 0]).contains("preset dictionary"));
        assert!(complaint(&[0x78, 0x01, 0x07, 0, 0, 0]).contains("reserved block type"));
        // A stored block promising sixteen bytes with only three
        // aboard runs out mid-block.
        assert!(
            complaint(&[0x78, 0x01, 0x01, 0x10, 0x00, 0xEF, 0xFF, 1, 2, 3]).contains("mid-block")
        );

        let mut wrong = deflated(b"abc");
        let last = wrong.len() - 1;

        wrong[last] ^= 1;

        assert!(complaint(&wrong).contains("checksum does not match"));

        // A stored block claiming one byte with a length check
        // that does not complement it.
        let lying = [0x78, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x41, 0, 0, 0, 0];

        assert!(complaint(&lying).contains("length check"));
    }
}
