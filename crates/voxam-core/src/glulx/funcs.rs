//! Function entry: headers read, frames built, arguments seated.
//!
//! A function opens with a type byte -- C0 for stack arguments, C1
//! for local arguments -- and a locals-format list, and its code
//! begins just past that (Glulx: Functions). Entering one builds a
//! call frame and seats the arguments as the type directs: a C0
//! function finds them pushed on its value stack, last argument
//! first with the count on top, while a C1 function finds them
//! written into its locals in order, extras dropped silently and
//! unfilled locals left zero (Glulx: Calling and Returning).
//!
//! The call stub is deliberately *not* pushed here. Whether one is
//! needed, and what its DestType says, depends on the opcode --
//! call pushes one, tailcall pointedly does not -- so the stub
//! stays the caller's business.

use crate::errors::VoxamError;
use crate::glulx::memory::Memory;
use crate::glulx::stack::{LocalsFormat, Stack};

/// The two function types (Glulx: Functions): C0 takes its
/// arguments on the stack, C1 in its locals.
pub const STACK_ARGUMENTS: u8 = 0xC0;
pub const LOCAL_ARGUMENTS: u8 = 0xC1;

/// C2 through DF are reserved for function types yet to be defined
/// (Glulx: Functions). The spec distinguishes them from plain
/// non-functions, and so does the reference glulxe, because the
/// difference tells an author whether an address is wrong or
/// merely too new for the interpreter.
const RESERVED_FIRST: u8 = 0xC2;
const RESERVED_LAST: u8 = 0xDF;

/// The sign bit of an unsigned 32-bit argument count: a "negative"
/// count is a count gone wrong, not a big one.
const COUNT_SIGN_BIT: u32 = 0x8000_0000;

const WORD_WIDTH: u32 = 4;

fn function_error(message: String) -> VoxamError {
    VoxamError::GlulxFunction(message)
}

/// A decoded function header (Glulx: Functions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionHeader {
    /// STACK_ARGUMENTS or LOCAL_ARGUMENTS.
    pub functype: u8,
    /// The declared locals, in order.
    pub locals_format: Vec<LocalsFormat>,
    /// The first instruction, just past the header.
    pub code_addr: u32,
}

/// Read the type byte and locals-format list at an address.
///
/// Refused for a type byte that is no function -- or one reserved
/// for a future kind of function, named as such -- or a local type
/// the format bytes cannot mean (Glulx: Functions), and for a
/// header running off the map.
pub fn read_function_header(memory: &Memory, addr: u32) -> Result<FunctionHeader, VoxamError> {
    let functype = memory.read_byte(addr)?;

    if functype != STACK_ARGUMENTS && functype != LOCAL_ARGUMENTS {
        let message = if (RESERVED_FIRST..=RESERVED_LAST).contains(&functype) {
            format!(
                "the address ${addr:x} holds type ${functype:x}, a function of a \
                 kind reserved for the future (Glulx: Functions)"
            )
        } else {
            format!(
                "the address ${addr:x} holds type ${functype:x}, which is not a \
                 function at all (Glulx: Functions)"
            )
        };

        return Err(function_error(message));
    }

    let mut at = addr.wrapping_add(1);
    let mut entries = Vec::new();

    loop {
        let size = memory.read_byte(at)?;
        let count = memory.read_byte(at.wrapping_add(1))?;
        at = at.wrapping_add(2);

        if size == 0 {
            break;
        }

        if !matches!(size, 1 | 2 | 4) {
            return Err(function_error(format!(
                "the function header at ${:x} declares a local type of {size}, not \
                 1, 2, or 4 (Glulx: Functions)",
                at.wrapping_sub(2)
            )));
        }

        entries.push(LocalsFormat { size, count });
    }

    Ok(FunctionHeader {
        functype,
        locals_format: entries,
        code_addr: at,
    })
}

/// Enter the function at an address; the new PC comes back.
///
/// The arguments arrive in call order -- args[0] first -- and are
/// seated as the function's type directs (Glulx: Calling and
/// Returning). Refused for an address that is no function or a
/// frame the stack cannot hold.
pub fn push_call_frame(
    memory: &Memory,
    stack: &mut Stack,
    funcaddr: u32,
    args: &[u32],
) -> Result<u32, VoxamError> {
    let header = read_function_header(memory, funcaddr)?;

    stack.push_frame(&header.locals_format)?;

    if header.functype == STACK_ARGUMENTS {
        push_stack_arguments(stack, args)?;
    } else {
        write_local_arguments(stack, &header.locals_format, args)?;
    }

    Ok(header.code_addr)
}

/// Collect a call's arguments, from the stack or from memory.
///
/// With addr zero the arguments come off the stack, first argument
/// topmost -- how callf's kin leave them. Otherwise they read as a
/// word array at addr, which is what the accelerated functions
/// will need, the address arithmetic wrapping at 32 bits like all
/// address arithmetic. Refused for a stack with fewer values than
/// asked, and for a count with its sign bit set -- a count gone
/// wrong, not a big one.
pub fn pop_arguments(
    stack: &mut Stack,
    count: u32,
    memory: &Memory,
    addr: u32,
) -> Result<Vec<u32>, VoxamError> {
    if count & COUNT_SIGN_BIT != 0 {
        return Err(function_error(format!(
            "an argument count of {count} has its sign bit set"
        )));
    }

    if addr == 0 {
        return (0..count).map(|_| stack.pop()).collect();
    }

    (0..count)
        .map(|index| memory.read_word(addr.wrapping_add(WORD_WIDTH.wrapping_mul(index))))
        .collect()
}

/// Seat a C0 function's arguments: backwards, then the count.
///
/// The last argument pushes first, so the first ends up topmost
/// with the count above it (Glulx: Functions).
fn push_stack_arguments(stack: &mut Stack, args: &[u32]) -> Result<(), VoxamError> {
    for value in args.iter().rev() {
        stack.push(*value)?;
    }

    stack.push(args.len() as u32)
}

/// Seat a C1 function's arguments into its locals, in order.
///
/// Extra arguments drop silently and unfilled locals stay zero,
/// both per (Glulx: Functions). A value written into an 8- or
/// 16-bit local truncates -- a deprecated arrangement, but still a
/// legal one.
fn write_local_arguments(
    stack: &mut Stack,
    locals_format: &[LocalsFormat],
    args: &[u32],
) -> Result<(), VoxamError> {
    let mut index = 0;
    let mut offset: u32 = 0;

    for entry in locals_format {
        if index >= args.len() {
            break;
        }

        // Each run starts at its own natural alignment, exactly as
        // the frame laid it down.
        let size = u32::from(entry.size);
        offset += (size - offset % size) % size;

        for _ in 0..entry.count {
            if index >= args.len() {
                return Ok(());
            }

            stack.set_local(offset, args[index], size)?;
            offset += size;
            index += 1;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glulx::story::Story;

    const FUNC: u32 = 0x140;

    /// A memory with a function header planted at $140, and a
    /// stack: the story module's honest 256-byte ROM with RAM
    /// running 256 to 512.
    fn rig(header: &[u8]) -> (Memory, Stack) {
        let mut data = vec![0u8; 256];
        data[..4].copy_from_slice(b"Glul");
        data[4..8].copy_from_slice(&0x0003_0102u32.to_be_bytes());
        data[8..12].copy_from_slice(&256u32.to_be_bytes());
        data[12..16].copy_from_slice(&256u32.to_be_bytes());
        data[16..20].copy_from_slice(&512u32.to_be_bytes());
        data[20..24].copy_from_slice(&256u32.to_be_bytes());

        let mut memory = Memory::new(&Story::new(data).unwrap());

        memory.write_run(FUNC, header).unwrap();

        (memory, Stack::new(0x200).unwrap())
    }

    // A header is a type byte and a zero-terminated locals-format
    // list, with the code starting just past it.
    #[test]
    fn a_function_header_reads_whole() {
        let (memory, _) = rig(&[0xC1, 0x04, 0x02, 0x01, 0x03, 0x00, 0x00]);

        assert_eq!(
            read_function_header(&memory, FUNC).unwrap(),
            FunctionHeader {
                functype: 0xC1,
                locals_format: vec![
                    LocalsFormat { size: 4, count: 2 },
                    LocalsFormat { size: 1, count: 3 }
                ],
                code_addr: FUNC + 7,
            }
        );

        let (bare, _) = rig(&[0xC0, 0x00, 0x00]);

        assert_eq!(
            read_function_header(&bare, FUNC).unwrap().code_addr,
            FUNC + 3
        );
    }

    // The type taxonomy matters: C2 through DF are functions of a
    // kind reserved for the future, and everything else is no
    // function at all -- the difference tells an author whether an
    // address is wrong or merely too new.
    #[test]
    fn the_type_taxonomy_names_the_failure() {
        let (reserved, _) = rig(&[0xC5]);
        let error = read_function_header(&reserved, FUNC).unwrap_err();

        assert!(error.to_string().contains("reserved for the future"));
        assert_eq!(
            error.to_string(),
            "the address $140 holds type $c5, a function of a kind reserved for \
             the future (Glulx: Functions)"
        );

        for wrong in [0x00, 0xE0] {
            let (other, _) = rig(&[wrong]);
            let error = read_function_header(&other, FUNC).unwrap_err();

            assert!(error.to_string().contains("not a function at all"));
        }

        let (illegal, _) = rig(&[0xC1, 0x03, 0x01, 0x00, 0x00]);
        let error = read_function_header(&illegal, FUNC).unwrap_err();

        assert!(error.to_string().contains("local type of 3"));
    }

    // A C0 function finds its arguments on the value stack: pushed
    // backwards, so the first argument sits topmost with the count
    // above it.
    #[test]
    fn a_c0_function_takes_arguments_on_the_stack() {
        let (memory, mut stack) = rig(&[0xC0, 0x00, 0x00]);

        let pc = push_call_frame(&memory, &mut stack, FUNC, &[7, 8, 9]).unwrap();

        assert_eq!(pc, FUNC + 3);
        assert_eq!(stack.pop().unwrap(), 3);
        assert_eq!(stack.pop().unwrap(), 7);
        assert_eq!(stack.pop().unwrap(), 8);
        assert_eq!(stack.pop().unwrap(), 9);
    }

    // A C1 function finds its arguments written into its locals in
    // order: values truncate to narrow locals -- deprecated but
    // legal -- extras drop silently, unfilled locals stay zero,
    // and each run seats at its own natural alignment.
    #[test]
    fn a_c1_function_takes_arguments_in_its_locals() {
        let (memory, mut stack) = rig(&[0xC1, 0x04, 0x02, 0x01, 0x03, 0x00, 0x00]);

        push_call_frame(
            &memory,
            &mut stack,
            FUNC,
            &[0x1122_3344, 0x55, 0x1FF, 0xAA, 0xBB, 0xCC, 0xDD],
        )
        .unwrap();

        assert_eq!(stack.get_local(0, 4).unwrap(), 0x1122_3344);
        assert_eq!(stack.get_local(4, 4).unwrap(), 0x55);
        assert_eq!(stack.get_local(8, 1).unwrap(), 0xFF);
        assert_eq!(stack.get_local(9, 1).unwrap(), 0xAA);
        assert_eq!(stack.get_local(10, 1).unwrap(), 0xBB);
        assert_eq!(stack.count(), 0);

        let (sparse, mut thin) = rig(&[0xC1, 0x04, 0x02, 0x00, 0x00]);

        push_call_frame(&sparse, &mut thin, FUNC, &[1]).unwrap();

        assert_eq!(thin.get_local(0, 4).unwrap(), 1);
        assert_eq!(thin.get_local(4, 4).unwrap(), 0);

        let (padded, mut aligned) = rig(&[0xC1, 0x01, 0x01, 0x04, 0x01, 0x00, 0x00]);

        push_call_frame(&padded, &mut aligned, FUNC, &[0x11, 0x22]).unwrap();

        assert_eq!(aligned.get_local(0, 1).unwrap(), 0x11);
        assert_eq!(aligned.get_local(4, 4).unwrap(), 0x22);

        let (skipped, mut hollow) = rig(&[0xC1, 0x04, 0x01, 0x01, 0x02, 0x00, 0x00]);

        push_call_frame(&skipped, &mut hollow, FUNC, &[9]).unwrap();

        assert_eq!(hollow.get_local(4, 1).unwrap(), 0);
    }

    // Arguments collect from the stack -- first argument topmost
    // -- or from a word array in memory, for the accelerated
    // functions to come; a count with its sign bit set is a count
    // gone wrong.
    #[test]
    fn arguments_collect_from_stack_or_memory() {
        let (mut memory, mut stack) = rig(&[]);

        stack.push(1).unwrap();
        stack.push(2).unwrap();
        stack.push(3).unwrap();

        assert_eq!(pop_arguments(&mut stack, 3, &memory, 0).unwrap(), [3, 2, 1]);
        assert!(pop_arguments(&mut stack, 0, &memory, 0).unwrap().is_empty());

        memory.write_word(0x180, 41).unwrap();
        memory.write_word(0x184, 42).unwrap();

        assert_eq!(
            pop_arguments(&mut stack, 2, &memory, 0x180).unwrap(),
            [41, 42]
        );

        let error = pop_arguments(&mut stack, 0x8000_0001, &memory, 0).unwrap_err();
        assert!(error.to_string().contains("sign bit"));
    }
}
