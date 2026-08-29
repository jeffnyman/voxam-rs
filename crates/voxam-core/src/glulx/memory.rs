//! Glulx main memory: the ROM/RAM map (Glulx: The Memory Map).
//!
//! Addresses 0 to RAMSTART are ROM -- the header included -- and
//! writing there is illegal; RAM runs from RAMSTART to ENDMEM. The
//! game file stores only the bytes up to EXTSTART, and everything
//! above starts zeroed; once execution begins there is no
//! difference between the memory below and above that line. Unlike
//! the stack, memory has no alignment rule: a four-byte read at an
//! odd address is legal Glulx.
//!
//! The bounds checks are unconditional, as in the reference: the
//! vendored glulxe hides its checks behind a compile-time switch;
//! here they are the law.

use crate::errors::VoxamError;
use crate::glulx::story::Story;

/// Boundaries sit on 256-byte seats (Glulx: The Header).
const BOUNDARY: u32 = 256;

fn memory_error(message: String) -> VoxamError {
    VoxamError::GlulxMemory(message)
}

/// The one message every out-of-map access carries.
fn out_of_range(address: u32) -> VoxamError {
    memory_error(format!(
        "the address ${address:x} is outside the memory map (Glulx: The Memory Map)"
    ))
}

/// Why a write was refused: ROM below RAMSTART, or off the map.
fn refused_write(address: u32, ramstart: u32) -> VoxamError {
    if address < ramstart {
        return memory_error(format!(
            "the address ${address:x} is in ROM, which ends at ${ramstart:x}: it is \
             illegal to write there (Glulx: The Memory Map)"
        ));
    }

    out_of_range(address)
}

/// The live memory map: ROM held sacred, RAM growable.
pub struct Memory {
    image: Vec<u8>,
    ramstart: u32,
    boot_endmem: u32,
    protect_start: u32,
    protect_end: u32,
    data: Vec<u8>,
    endmem: u32,
}

impl Memory {
    /// Lay the stored image into a map grown to ENDMEM. The story
    /// already held the header to its promises, so none of that is
    /// re-litigated here.
    pub fn new(story: &Story) -> Self {
        let mut memory = Self {
            image: story.data().to_vec(),
            ramstart: story.ramstart(),
            boot_endmem: story.endmem(),
            protect_start: 0,
            protect_end: 0,
            data: Vec::new(),
            endmem: 0,
        };

        memory.reset();

        memory
    }

    /// The first writable address (Glulx: The Memory Map).
    pub fn ramstart(&self) -> u32 {
        self.ramstart
    }

    /// The current end of the memory map.
    pub fn endmem(&self) -> u32 {
        self.endmem
    }

    /// The raw backing store, for the instruction decoder only:
    /// everything else goes through the accessors, and the decoder
    /// must keep the guarantee they provide.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Read one byte anywhere in the map.
    pub fn read_byte(&self, address: u32) -> Result<u8, VoxamError> {
        if address >= self.endmem {
            return Err(out_of_range(address));
        }

        Ok(self.data[address as usize])
    }

    /// Read a big-endian 16-bit short, any alignment.
    pub fn read_short(&self, address: u32) -> Result<u16, VoxamError> {
        if self.endmem < 2 || address > self.endmem - 2 {
            return Err(out_of_range(address));
        }

        let at = address as usize;

        Ok(u16::from_be_bytes([self.data[at], self.data[at + 1]]))
    }

    /// Read a big-endian 32-bit word, any alignment.
    pub fn read_word(&self, address: u32) -> Result<u32, VoxamError> {
        if self.endmem < 4 || address > self.endmem - 4 {
            return Err(out_of_range(address));
        }

        let at = address as usize;

        Ok(u32::from_be_bytes([
            self.data[at],
            self.data[at + 1],
            self.data[at + 2],
            self.data[at + 3],
        ]))
    }

    /// Read at an operand's width: 1, 2, or 4 bytes.
    pub fn read(&self, address: u32, width: u32) -> Result<u32, VoxamError> {
        match width {
            4 => self.read_word(address),
            1 => Ok(u32::from(self.read_byte(address)?)),
            _ => Ok(u32::from(self.read_short(address)?)),
        }
    }

    /// Read a run of bytes; an empty run needs no address at all.
    pub fn read_run(&self, address: u32, count: u32) -> Result<Vec<u8>, VoxamError> {
        if count == 0 {
            return Ok(Vec::new());
        }

        self.require_readable(address, count)?;

        let at = address as usize;

        Ok(self.data[at..at + count as usize].to_vec())
    }

    /// Write one byte into RAM.
    pub fn write_byte(&mut self, address: u32, value: u8) -> Result<(), VoxamError> {
        if address < self.ramstart || address >= self.endmem {
            return Err(refused_write(address, self.ramstart));
        }

        self.data[address as usize] = value;

        Ok(())
    }

    /// Write a big-endian short into RAM.
    pub fn write_short(&mut self, address: u32, value: u16) -> Result<(), VoxamError> {
        if address < self.ramstart || address > self.endmem.saturating_sub(2) {
            return Err(refused_write(address, self.ramstart));
        }

        let at = address as usize;
        self.data[at..at + 2].copy_from_slice(&value.to_be_bytes());

        Ok(())
    }

    /// Write a big-endian word into RAM.
    pub fn write_word(&mut self, address: u32, value: u32) -> Result<(), VoxamError> {
        if address < self.ramstart || address > self.endmem.saturating_sub(4) {
            return Err(refused_write(address, self.ramstart));
        }

        let at = address as usize;
        self.data[at..at + 4].copy_from_slice(&value.to_be_bytes());

        Ok(())
    }

    /// Write at an operand's width, the value masked to it.
    pub fn write(&mut self, address: u32, width: u32, value: u32) -> Result<(), VoxamError> {
        match width {
            4 => self.write_word(address, value),
            1 => self.write_byte(address, (value & 0xFF) as u8),
            _ => self.write_short(address, (value & 0xFFFF) as u16),
        }
    }

    /// Write a run of bytes into RAM; an empty run writes nowhere.
    pub fn write_run(&mut self, address: u32, data: &[u8]) -> Result<(), VoxamError> {
        if data.is_empty() {
            return Ok(());
        }

        self.require_writable(address, data.len() as u32)?;

        let at = address as usize;
        self.data[at..at + data.len()].copy_from_slice(data);

        Ok(())
    }

    /// Set a run of RAM bytes to one value -- mzero's work.
    pub fn fill(&mut self, address: u32, count: u32, value: u8) -> Result<(), VoxamError> {
        if count == 0 {
            return Ok(());
        }

        self.require_writable(address, count)?;

        let at = address as usize;
        self.data[at..at + count as usize].fill(value);

        Ok(())
    }

    /// Copy a run within memory -- mcopy's work. Overlap is
    /// handled correctly: the source is read out whole before a
    /// byte lands.
    pub fn copy(&mut self, destination: u32, source: u32, count: u32) -> Result<(), VoxamError> {
        if count == 0 {
            return Ok(());
        }

        self.require_readable(source, count)?;
        self.require_writable(destination, count)?;

        let held = self.data[source as usize..(source + count) as usize].to_vec();
        self.data[destination as usize..(destination + count) as usize].copy_from_slice(&held);

        Ok(())
    }

    /// Resize the memory map -- setmemsize's work: growth is
    /// zero-filled and shrinkage discards, but the map never
    /// shrinks below its boot ENDMEM, and every size sits on the
    /// 256-byte boundary (Glulx: Game State).
    pub fn set_size(&mut self, size: u32) -> Result<(), VoxamError> {
        if !size.is_multiple_of(BOUNDARY) {
            return Err(memory_error(format!(
                "a memory size of {size} is not a multiple of {BOUNDARY} (Glulx: \
                 Game State)"
            )));
        }

        if size < self.boot_endmem {
            return Err(memory_error(format!(
                "memory cannot shrink to {size}, below the {} it booted with (Glulx: \
                 Game State)",
                self.boot_endmem
            )));
        }

        self.data.resize(size as usize, 0);
        self.endmem = size;

        Ok(())
    }

    /// Mark the range restart and restore leave alone -- protect's
    /// work. One range exists at a time, a zero length turns
    /// protection off, and the range is deliberately not part of
    /// saved state (Glulx: Game State).
    pub fn set_protection(&mut self, start: u32, length: u32) {
        if length == 0 {
            self.protect_start = 0;
            self.protect_end = 0;
        } else {
            self.protect_start = start;
            self.protect_end = start.saturating_add(length);
        }
    }

    /// What the game file held over a span; zeroes past its end
    /// (Glulx: The Save-Game Format).
    pub fn original_run(&self, address: u32, count: u32) -> Vec<u8> {
        let start = (address as usize).min(self.image.len());
        let end = (address as usize + count as usize).min(self.image.len());

        let mut run = self.image[start..end].to_vec();
        run.resize(count as usize, 0);

        run
    }

    /// Lay restored RAM in from RAMSTART, sparing protection: the
    /// protected range is "silently unaffected" by a restore
    /// (Glulx: Game State). Skipping the writes is the right model
    /// because the restore may have resized memory underneath the
    /// range.
    pub fn overwrite_ram(&mut self, contents: &[u8]) {
        let start = self.ramstart as usize;
        let end = start + contents.len();
        let low = (self.protect_start as usize).max(start);
        let high = (self.protect_end as usize).min(end);

        if high <= low {
            self.data[start..end].copy_from_slice(contents);

            return;
        }

        if low > start {
            self.data[start..low].copy_from_slice(&contents[..low - start]);
        }

        if high < end {
            self.data[high..end].copy_from_slice(&contents[high - start..]);
        }
    }

    /// Restore the boot image whole -- restart's work. The
    /// protected range is "silently unaffected" (Glulx: Game
    /// State), surviving even above EXTSTART, and the map returns
    /// to its boot size.
    pub fn reset(&mut self) {
        let saved = self.protected_copy();

        self.data = vec![0; self.boot_endmem as usize];
        self.endmem = self.boot_endmem;
        self.data[..self.image.len()].copy_from_slice(&self.image);

        if let Some((start, held)) = saved {
            let start = start as usize;
            let end = (start + held.len()).min(self.endmem as usize);

            if end > start {
                self.data[start..end].copy_from_slice(&held[..end - start]);
            }
        }
    }

    /// The protected range's live bytes, None when none is set.
    fn protected_copy(&self) -> Option<(u32, Vec<u8>)> {
        let start = self.protect_start;
        let end = self.protect_end.min(self.endmem);

        if end <= start {
            return None;
        }

        Some((start, self.data[start as usize..end as usize].to_vec()))
    }

    /// Hold a run to the map (Glulx: The Memory Map).
    fn require_readable(&self, address: u32, count: u32) -> Result<(), VoxamError> {
        if u64::from(address) + u64::from(count) > u64::from(self.endmem) {
            return Err(out_of_range(address));
        }

        Ok(())
    }

    /// Hold a run to RAM (Glulx: The Memory Map).
    fn require_writable(&self, address: u32, count: u32) -> Result<(), VoxamError> {
        if address < self.ramstart || u64::from(address) + u64::from(count) > u64::from(self.endmem)
        {
            return Err(refused_write(address, self.ramstart));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 512-byte map over the story module's honest 256-byte ROM
    /// image: RAM runs 256 to 512, starting zeroed.
    fn memory() -> Memory {
        let mut data = vec![0u8; 256];
        data[..4].copy_from_slice(b"Glul");
        data[4..8].copy_from_slice(&0x0003_0102u32.to_be_bytes());
        data[8..12].copy_from_slice(&256u32.to_be_bytes());
        data[12..16].copy_from_slice(&256u32.to_be_bytes());
        data[16..20].copy_from_slice(&512u32.to_be_bytes());
        data[20..24].copy_from_slice(&256u32.to_be_bytes());
        data[100..104].copy_from_slice(&0xDEAD_BEEFu32.to_be_bytes());

        Memory::new(&Story::new(data).unwrap())
    }

    #[test]
    fn the_image_lays_in_and_ram_starts_zeroed() {
        let memory = memory();

        assert_eq!(memory.read_word(100).unwrap(), 0xDEAD_BEEF);
        assert_eq!(memory.read_word(300).unwrap(), 0);
        assert_eq!(memory.endmem(), 512);
    }

    #[test]
    fn reads_have_no_alignment_rule() {
        let memory = memory();

        assert_eq!(memory.read_word(101).unwrap(), 0xADBE_EF00);
        assert_eq!(memory.read_short(103).unwrap(), 0xEF00);
    }

    #[test]
    fn rom_refuses_writes_and_the_map_bounds_all() {
        let mut memory = memory();

        let error = memory.write_byte(10, 1).unwrap_err();
        assert!(error.to_string().contains("ROM"));

        assert!(memory.write_word(300, 0x12345678).is_ok());
        assert_eq!(memory.read_word(300).unwrap(), 0x12345678);

        assert!(memory.read_byte(512).is_err());
        assert!(memory.read_word(509).is_err());
        assert!(memory.write_word(509, 0).is_err());
    }

    #[test]
    fn copies_survive_overlap() {
        let mut memory = memory();
        memory.write_run(300, &[1, 2, 3, 4]).unwrap();

        memory.copy(302, 300, 4).unwrap();

        assert_eq!(memory.read_run(300, 6).unwrap(), [1, 2, 1, 2, 3, 4]);
    }

    #[test]
    fn resizing_respects_the_boundary_and_the_floor() {
        let mut memory = memory();

        memory.set_size(1024).unwrap();
        assert_eq!(memory.endmem(), 1024);
        assert_eq!(memory.read_word(1000).unwrap(), 0);

        memory.set_size(512).unwrap();
        assert!(memory.read_byte(512).is_err());

        assert!(memory.set_size(300).is_err());
        assert!(memory.set_size(256).is_err());
    }

    #[test]
    fn reset_restores_the_boot_image_and_size() {
        let mut memory = memory();
        memory.write_word(300, 0x1111_2222).unwrap();
        memory.set_size(1024).unwrap();

        memory.reset();

        assert_eq!(memory.endmem(), 512);
        assert_eq!(memory.read_word(300).unwrap(), 0);
        assert_eq!(memory.read_word(100).unwrap(), 0xDEAD_BEEF);
    }

    #[test]
    fn protection_survives_reset_and_restore() {
        let mut memory = memory();
        memory.write_word(300, 0x5555_6666).unwrap();
        memory.set_protection(300, 4);

        memory.reset();
        assert_eq!(memory.read_word(300).unwrap(), 0x5555_6666);

        // A restore lays RAM in around the range.
        let mut restored = vec![0xAAu8; 256];
        restored[48] = 0xBB; // address 304
        memory.overwrite_ram(&restored);

        assert_eq!(memory.read_word(300).unwrap(), 0x5555_6666);
        assert_eq!(memory.read_byte(304).unwrap(), 0xBB);
        assert_eq!(memory.read_byte(299).unwrap(), 0xAA);
    }

    #[test]
    fn the_original_run_extends_with_zeroes() {
        let memory = memory();

        let run = memory.original_run(252, 8);
        assert_eq!(run.len(), 8);
        assert_eq!(&run[4..], &[0, 0, 0, 0]);

        assert_eq!(memory.original_run(1000, 4), [0, 0, 0, 0]);
    }
}
