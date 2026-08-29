//! The dynamic allocation heap (Glulx: Memory Allocation Heap).
//!
//! Allocated blocks live above ENDMEM. The first malloc activates
//! the heap: the current end of memory becomes the heap's start
//! address, and the map grows from there. Freeing the last block
//! deactivates it and shrinks memory back to where it began, at
//! which point setmemsize becomes legal again.
//!
//! The block list covers the heap completely and in address order
//! -- the first block starts at the heap's start, each one ends
//! where the next begins, and the last ends at endmem. Free blocks
//! are part of that list rather than a separate free-list, which
//! is why coalescing is something the allocator does as it
//! searches.
//!
//! The bookkeeping lives here, not in the memory map, so a game
//! writing outside its blocks cannot corrupt it -- the spec says
//! the interpreter may keep it "in a private data structure", and
//! Voxam does exactly that. Writing anywhere in the heap range
//! stays legal. Following the port's usual arrangement, the heap
//! does not hold the memory map: the methods that grow or shrink
//! it take the map as an argument.

use crate::errors::VoxamError;
use crate::glulx::memory::Memory;

/// Memory grows in 256-byte units, like every Glulx boundary.
const BOUNDARY: u32 = 0x100;

fn memory_error(message: String) -> VoxamError {
    VoxamError::GlulxMemory(message)
}

fn save_error(message: &str) -> VoxamError {
    VoxamError::GlulxSave(message.into())
}

/// One span of the heap, allocated or free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    /// Where the span begins.
    pub address: u32,
    /// How many bytes it covers.
    pub length: u32,
    /// Whether the span is unclaimed.
    pub free: bool,
}

/// The allocation heap for one machine.
#[derive(Debug, Default)]
pub struct Heap {
    /// The heap's start address; zero means inactive.
    pub start: u32,
    /// Every span, allocated and free, in address order.
    pub blocks: Vec<Block>,
    /// How many blocks are currently allocated.
    pub alloc_count: u32,
}

impl Heap {
    /// A heap standing over no map yet, inactive.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether any block is extant -- the heap owns the map.
    pub fn active(&self) -> bool {
        self.start != 0
    }

    /// Deactivate the heap and give its memory back.
    ///
    /// Freeing the last block lands here, and so does restart --
    /// the heap does not survive one (Glulx: Memory Allocation
    /// Heap).
    pub fn clear(&mut self, memory: &mut Memory) -> Result<(), VoxamError> {
        self.blocks.clear();

        if self.start != 0 {
            memory.set_size(self.start)?;
        }

        self.start = 0;
        self.alloc_count = 0;

        Ok(())
    }

    /// Claim a span; the address comes back, or 0 on failure.
    ///
    /// Allocation is never guaranteed: a refusal is an answer, not
    /// an error (Glulx: Memory Allocation Heap). A zero-length
    /// request, which no answer could name, is refused loudly.
    pub fn alloc(&mut self, memory: &mut Memory, length: u32) -> Result<u32, VoxamError> {
        if length == 0 {
            return Err(memory_error(
                "a heap allocation must ask for at least one byte".into(),
            ));
        }

        let index = match self.find_free(length) {
            Some(index) => index,
            None => match self.extend(memory, length) {
                Some(index) => index,
                None => return Ok(0),
            },
        };

        let block = self.blocks[index];

        if block.length > length {
            // Split, leaving the remainder free and the list still
            // in address order.
            self.blocks.insert(
                index + 1,
                Block {
                    address: block.address + length,
                    length: block.length - length,
                    free: true,
                },
            );

            self.blocks[index].length = length;
        }

        self.blocks[index].free = false;
        self.alloc_count += 1;

        Ok(self.blocks[index].address)
    }

    /// First-fit search, coalescing free neighbors on the way.
    ///
    /// Merging happens during the search rather than eagerly, as
    /// the reference glulxe has it: a run of free blocks is only
    /// joined up when something actually needs the space.
    fn find_free(&mut self, length: u32) -> Option<usize> {
        let mut index = 0;

        while index < self.blocks.len() {
            let block = self.blocks[index];

            if block.free && block.length >= length {
                return Some(index);
            }

            if !block.free {
                index += 1;

                continue;
            }

            let following = self.blocks.get(index + 1).copied();

            match following {
                Some(next) if next.free => {
                    // Free, too small, and followed by free space:
                    // merge and retry at the same position rather
                    // than advancing.
                    self.blocks[index].length += next.length;
                    self.blocks.remove(index + 1);
                }
                _ => index += 1,
            }
        }

        None
    }

    /// Grow the map; the new free block's index comes back.
    ///
    /// The heap doubles, or grows by the requested length, or by
    /// one boundary -- whichever is largest -- rounded up to the
    /// 256-byte grain. A map the address space cannot hold refuses
    /// the same way a map the machine cannot hold does.
    fn extend(&mut self, memory: &mut Memory, length: u32) -> Option<usize> {
        let old_endmem = memory.endmem();
        let held = if self.start != 0 {
            old_endmem - self.start
        } else {
            0
        };
        let extension = held.max(length).max(BOUNDARY).checked_add(BOUNDARY - 1)? & !(BOUNDARY - 1);

        // Allocation is never guaranteed (Glulx: Memory Allocation
        // Heap).
        let size = old_endmem.checked_add(extension)?;
        memory.set_size(size).ok()?;

        if self.start == 0 {
            self.start = old_endmem;
        }

        match self.blocks.last_mut() {
            Some(last) if last.free => last.length += extension,
            _ => self.blocks.push(Block {
                address: old_endmem,
                length: extension,
                free: true,
            }),
        }

        Some(self.blocks.len() - 1)
    }

    /// Release the block at an address, which must be extant.
    ///
    /// Freeing the last block deactivates the heap and hands the
    /// memory back (Glulx: Memory Allocation Heap). Refused for an
    /// address that names no allocated block.
    pub fn free(&mut self, memory: &mut Memory, address: u32) -> Result<(), VoxamError> {
        let found = self
            .blocks
            .iter_mut()
            .find(|block| block.address == address && !block.free);

        let Some(block) = found else {
            return Err(memory_error(format!(
                "no allocated heap block begins at {address:#x}"
            )));
        };

        block.free = true;
        self.alloc_count -= 1;

        if self.alloc_count == 0 {
            self.clear(memory)?;
        }

        Ok(())
    }

    /// The heap as the save format's MAll words.
    ///
    /// The layout is start, count, then address and length for
    /// each extant block (Glulx: Memory Allocation Heap); an
    /// inactive heap summarizes as nothing at all, and its chunk
    /// is omitted.
    pub fn summary(&self) -> Vec<u32> {
        if !self.active() {
            return Vec::new();
        }

        let mut values = vec![self.start, self.alloc_count];

        for block in &self.blocks {
            if !block.free {
                values.push(block.address);
                values.push(block.length);
            }
        }

        values
    }

    /// Rebuild the heap from a summary's words.
    ///
    /// Memory must already be the size it was when the summary was
    /// taken -- restoring the map is the caller's job -- and the
    /// free blocks are reconstructed from the gaps between extant
    /// ones, out to endmem. Refused when the heap is already
    /// active, the summary's pairs are cut short, or its blocks
    /// are out of address order.
    pub fn apply_summary(&mut self, memory: &Memory, values: &[u32]) -> Result<(), VoxamError> {
        if self.active() {
            return Err(save_error("a heap summary cannot land on an active heap"));
        }

        if values.is_empty() || values[..2.min(values.len())] == [0, 0] {
            return Ok(());
        }

        let extant = &values[2.min(values.len())..];

        if !extant.len().is_multiple_of(2) {
            return Err(save_error(
                "the save file's heap summary is cut short mid-block",
            ));
        }

        let addresses: Vec<u32> = extant.iter().step_by(2).copied().collect();

        if addresses.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(save_error(
                "the save file's heap blocks are out of address order",
            ));
        }

        self.start = values[0];
        self.alloc_count = values[1];
        self.blocks = Vec::new();

        let mut position = 0;
        let mut cursor = self.start;
        let endmem = memory.endmem();

        while position < extant.len() || cursor < endmem {
            if position >= extant.len() {
                // Trailing free space, out to the end of the map.
                self.blocks.push(Block {
                    address: cursor,
                    length: endmem - cursor,
                    free: true,
                });

                break;
            }

            let (address, length) = (extant[position], extant[position + 1]);

            if cursor < address {
                // A gap before the next extant block is free space.
                self.blocks.push(Block {
                    address: cursor,
                    length: address - cursor,
                    free: true,
                });

                cursor = address;

                continue;
            }

            self.blocks.push(Block {
                address,
                length,
                free: false,
            });

            position += 2;
            cursor = address + length;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glulx::story::Story;

    const BOOT_END: u32 = 0x300;

    /// A memory whose map ends at the tests' BOOT_END, standing in
    /// for the reference suite's booted machine.
    fn booted() -> Memory {
        let mut data = vec![0u8; 256];
        data[..4].copy_from_slice(b"Glul");
        data[4..8].copy_from_slice(&0x0003_0102u32.to_be_bytes());
        data[8..12].copy_from_slice(&256u32.to_be_bytes());
        data[12..16].copy_from_slice(&256u32.to_be_bytes());
        data[16..20].copy_from_slice(&BOOT_END.to_be_bytes());
        data[20..24].copy_from_slice(&256u32.to_be_bytes());

        Memory::new(&Story::new(data).unwrap())
    }

    // The first malloc activates the heap at the old end of memory
    // and grows the map; freeing the last block hands everything
    // back.
    #[test]
    fn the_heap_activates_and_retires() {
        let mut memory = booted();
        let mut heap = Heap::new();

        assert!(!heap.active());

        let first = heap.alloc(&mut memory, 0x40).unwrap();

        assert_eq!(first, BOOT_END);
        assert_eq!(heap.start, BOOT_END);
        assert_eq!(memory.endmem(), BOOT_END + 0x100);

        heap.free(&mut memory, first).unwrap();

        assert!(!heap.active());
        assert_eq!(memory.endmem(), BOOT_END);
    }

    // Allocation splits free space, reuses freed blocks first-fit,
    // and coalesces adjacent free spans only when something needs
    // the room.
    #[test]
    fn blocks_split_reuse_and_coalesce() {
        let mut memory = booted();
        let mut heap = Heap::new();

        let first = heap.alloc(&mut memory, 0x40).unwrap();
        let second = heap.alloc(&mut memory, 0x40).unwrap();
        let third = heap.alloc(&mut memory, 0x40).unwrap();

        assert_eq!((second, third), (BOOT_END + 0x40, BOOT_END + 0x80));

        // Freeing the first two leaves two small free spans; a
        // request too big for either merges them on the way past.
        heap.free(&mut memory, first).unwrap();
        heap.free(&mut memory, second).unwrap();

        let merged = heap.alloc(&mut memory, 0x60).unwrap();

        assert_eq!(merged, first);

        // The remainder of the merged span is free again and taken
        // by a fit-sized request.
        assert_eq!(heap.alloc(&mut memory, 0x20).unwrap(), first + 0x60);
        assert_eq!(heap.alloc(&mut memory, 0x40).unwrap(), BOOT_END + 0xC0);

        // A free span walled in by allocated neighbors cannot
        // merge; the request extends the map past the allocated
        // tail instead.
        heap.free(&mut memory, third).unwrap();

        assert_eq!(heap.alloc(&mut memory, 0x50).unwrap(), BOOT_END + 0x100);

        // And a free tail with nothing after it merges with the
        // extension when the map grows again.
        heap.free(&mut memory, BOOT_END + 0x100).unwrap();

        assert_eq!(heap.alloc(&mut memory, 0x200).unwrap(), BOOT_END + 0x100);
    }

    // Growing doubles the heap once one exists, and a grown
    // request merges into a trailing free block rather than
    // fragmenting.
    #[test]
    fn the_heap_doubles_as_it_grows() {
        let mut memory = booted();
        let mut heap = Heap::new();

        let first = heap.alloc(&mut memory, 0x100).unwrap();

        assert_eq!(memory.endmem(), BOOT_END + 0x100);

        // The heap is full; the next request doubles it.
        let second = heap.alloc(&mut memory, 0x100).unwrap();

        assert_eq!(second, BOOT_END + 0x100);
        assert_eq!(memory.endmem(), BOOT_END + 0x200);

        // A big request from a full heap extends by the request,
        // rounded to the boundary, and lands after the extant
        // blocks.
        let third = heap.alloc(&mut memory, 0x210).unwrap();

        assert_eq!(third, BOOT_END + 0x200);
        assert_eq!(memory.endmem(), BOOT_END + 0x500);

        // Free the tail block, then overask: the extension merges
        // into the trailing free span.
        heap.free(&mut memory, third).unwrap();

        let fourth = heap.alloc(&mut memory, 0x800).unwrap();

        assert_eq!(fourth, BOOT_END + 0x200);

        heap.free(&mut memory, fourth).unwrap();
        heap.free(&mut memory, first).unwrap();
        heap.free(&mut memory, second).unwrap();

        assert!(!heap.active());
    }

    // The refusals: a zero-length request is an error, an
    // impossible extension is a spoken zero, and freeing what was
    // never allocated is an error whether it is unknown or already
    // free.
    #[test]
    fn the_heap_refuses_loudly_or_softly() {
        let mut memory = booted();
        let mut heap = Heap::new();

        let error = heap.alloc(&mut memory, 0).unwrap_err();
        assert!(error.to_string().contains("at least one byte"));

        let error = heap.free(&mut memory, 0x9999).unwrap_err();
        assert!(error.to_string().contains("no allocated heap block"));
        assert_eq!(
            error.to_string(),
            "no allocated heap block begins at 0x9999"
        );

        let first = heap.alloc(&mut memory, 0x40).unwrap();

        heap.alloc(&mut memory, 0x40).unwrap();
        heap.free(&mut memory, first).unwrap();

        let error = heap.free(&mut memory, first).unwrap_err();
        assert!(error.to_string().contains("no allocated heap block"));

        // Allocation is never guaranteed: a map the address space
        // cannot hold answers zero, not a fault.
        assert_eq!(heap.alloc(&mut memory, 0xFFFF_FF00).unwrap(), 0);
    }

    // A summary names the extant blocks; applying one rebuilds
    // them with the gaps and the tail reconstructed as free space.
    #[test]
    fn summaries_rebuild_the_heap() {
        let mut memory = booted();
        let mut heap = Heap::new();

        assert!(heap.summary().is_empty());

        let first = heap.alloc(&mut memory, 0x40).unwrap();
        let second = heap.alloc(&mut memory, 0x30).unwrap();
        let third = heap.alloc(&mut memory, 0x20).unwrap();

        heap.free(&mut memory, second).unwrap();

        let words = heap.summary();

        assert_eq!(words, [BOOT_END, 2, first, 0x40, third, 0x20]);

        // Rebuild on a fresh twin whose memory is already the
        // right size, and the gap and tail come back free.
        let mut twin_memory = booted();
        let mut twin = Heap::new();

        twin_memory.set_size(memory.endmem()).unwrap();
        twin.apply_summary(&twin_memory, &words).unwrap();

        assert_eq!(
            twin.blocks,
            [
                Block {
                    address: first,
                    length: 0x40,
                    free: false
                },
                Block {
                    address: first + 0x40,
                    length: 0x30,
                    free: true
                },
                Block {
                    address: third,
                    length: 0x20,
                    free: false
                },
                Block {
                    address: third + 0x20,
                    length: 0x70,
                    free: true
                },
            ]
        );
        assert_eq!(twin.summary(), words);

        // A summary whose last block runs flush to the end of
        // memory rebuilds with no trailing free span at all.
        let mut flush_memory = booted();
        let mut flush = Heap::new();

        flush_memory.set_size(0x400).unwrap();
        flush
            .apply_summary(&flush_memory, &[BOOT_END, 1, BOOT_END, 0x100])
            .unwrap();

        assert_eq!(
            flush.blocks,
            [Block {
                address: BOOT_END,
                length: 0x100,
                free: false
            }]
        );

        // The empty forms apply as nothing at all.
        let bare_memory = booted();
        let mut bare = Heap::new();

        bare.apply_summary(&bare_memory, &[]).unwrap();
        bare.apply_summary(&bare_memory, &[0, 0]).unwrap();

        assert!(!bare.active());
    }

    // The summaries that cannot be applied: onto an active heap,
    // cut short mid-block, or with blocks out of address order.
    #[test]
    fn wrong_summaries_are_refused() {
        let mut memory = booted();
        let mut heap = Heap::new();

        heap.alloc(&mut memory, 0x40).unwrap();

        let error = heap
            .apply_summary(&memory, &[0x300, 1, 0x300, 0x40])
            .unwrap_err();
        assert!(error.to_string().contains("active heap"));

        let fresh_memory = booted();
        let mut fresh = Heap::new();

        let error = fresh
            .apply_summary(&fresh_memory, &[0x300, 2, 0x300])
            .unwrap_err();
        assert!(error.to_string().contains("cut short"));

        let error = fresh
            .apply_summary(&fresh_memory, &[0x300, 2, 0x340, 0x10, 0x300, 0x10])
            .unwrap_err();
        assert!(error.to_string().contains("out of address order"));
    }
}
