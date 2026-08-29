//! Tiny checksummed Glulx images for tests, mirroring the
//! reference suite's conftest builder: ROM to $100, stored RAM to
//! $200, the map to $300, a $100 stack, the start function at $48,
//! and the header naming a decoding table at $54.

const RAMSTART: u32 = 0x100;
const EXTSTART: u32 = 0x200;
const ENDMEM: u32 = 0x300;
const STACK: u32 = 0x100;
const START_FUNCTION: u32 = 0x48;
const DECODING_TABLE: u32 = 0x54;

/// An image with the given code seated at the start function.
pub(crate) fn image(code: &[u8]) -> Vec<u8> {
    build(code, None)
}

/// The same image with the checksum forced -- for verify tests.
pub(crate) fn image_with_checksum(code: &[u8], checksum: u32) -> Vec<u8> {
    build(code, Some(checksum))
}

fn build(code: &[u8], checksum: Option<u32>) -> Vec<u8> {
    let mut data = vec![0u8; EXTSTART as usize];

    data[0..4].copy_from_slice(b"Glul");
    data[4..8].copy_from_slice(&0x0003_0102u32.to_be_bytes());
    data[8..12].copy_from_slice(&RAMSTART.to_be_bytes());
    data[12..16].copy_from_slice(&EXTSTART.to_be_bytes());
    data[16..20].copy_from_slice(&ENDMEM.to_be_bytes());
    data[20..24].copy_from_slice(&STACK.to_be_bytes());
    data[24..28].copy_from_slice(&START_FUNCTION.to_be_bytes());
    data[28..32].copy_from_slice(&DECODING_TABLE.to_be_bytes());

    let seat = START_FUNCTION as usize;
    data[seat..seat + code.len()].copy_from_slice(code);

    let checksum = checksum.unwrap_or_else(|| {
        (0..data.len()).step_by(4).fold(0u32, |total, at| {
            total.wrapping_add(u32::from_be_bytes([
                data[at],
                data[at + 1],
                data[at + 2],
                data[at + 3],
            ]))
        })
    });

    data[32..36].copy_from_slice(&checksum.to_be_bytes());

    data
}
