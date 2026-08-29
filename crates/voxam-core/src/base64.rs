//! Standard base64, hand-rolled the way the JSON dialect was.
//!
//! The wire's data: urls carry pictures and sounds whole, and the
//! encoding is RFC 4648's standard alphabet with padding -- what
//! Python's `base64.b64encode` writes, byte for byte. The port
//! keeps its two-dependency diet by spelling the sixty-four
//! characters here rather than importing them.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const PAD: u8 = b'=';

/// Encode bytes in the standard alphabet, padded.
pub fn b64(data: &[u8]) -> String {
    let mut held = Vec::with_capacity(data.len().div_ceil(3) * 4);

    for triple in data.chunks(3) {
        let group = (u32::from(triple[0]) << 16)
            | (u32::from(*triple.get(1).unwrap_or(&0)) << 8)
            | u32::from(*triple.get(2).unwrap_or(&0));

        held.push(ALPHABET[(group >> 18) as usize & 0x3F]);
        held.push(ALPHABET[(group >> 12) as usize & 0x3F]);
        held.push(if triple.len() > 1 {
            ALPHABET[(group >> 6) as usize & 0x3F]
        } else {
            PAD
        });
        held.push(if triple.len() > 2 {
            ALPHABET[group as usize & 0x3F]
        } else {
            PAD
        });
    }

    String::from_utf8(held).expect("the alphabet is ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 4648's own test vectors, which b64encode matches.
    #[test]
    fn the_rfc_vectors_encode() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foob"), "Zm9vYg==");
        assert_eq!(b64(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn every_byte_value_survives() {
        let all: Vec<u8> = (0..=255).collect();

        assert_eq!(b64(&all[..3]), "AAEC");
        assert_eq!(b64(&all[252..]), "/P3+/w==");
    }
}
