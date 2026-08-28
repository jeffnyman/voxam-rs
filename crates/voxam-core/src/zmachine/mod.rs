//! The Z-Machine: Infocom's 1979 virtual machine, implemented to
//! the Z-Machine Standard 1.1.

pub mod header;
pub mod memory;
pub mod rng;
pub mod story;

/// Synthetic story files for the module tests: the smallest bytes
/// that pass validation, shaped one knob at a time.
#[cfg(test)]
pub(crate) mod testing {
    /// A story image with the given version and region boundaries,
    /// a serial of "851218", and zeroes elsewhere.
    pub(crate) fn story_bytes(
        version: u8,
        length: usize,
        static_base: u16,
        high_base: u16,
    ) -> Vec<u8> {
        let mut data = vec![0u8; length];
        data[0] = version;
        data[0x04..0x06].copy_from_slice(&high_base.to_be_bytes());
        data[0x0E..0x10].copy_from_slice(&static_base.to_be_bytes());
        data[0x12..0x18].copy_from_slice(b"851218");

        data
    }
}
