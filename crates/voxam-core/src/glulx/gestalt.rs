//! Gestalt selectors: what this interpreter can do (Glulx:
//! Gestalt).
//!
//! The reference glulxe answers most of these from compile-time
//! switches; the Python reference answers them from Capabilities,
//! a runtime value that tracks which eras exist yet. The port
//! keeps that arrangement -- each False was never a design
//! decision but a statement that the supporting era had not
//! arrived -- so the two implementations answer identically at
//! every point along the ladder. `answer` takes the machine's
//! relevant pieces as arguments rather than the machine itself,
//! the port's usual arrangement for state views.

/// The Glulx specification version implemented: 3.1.3, packed as
/// the header packs it (Glulx: The Header).
pub const GLULX_VERSION: u32 = 0x0003_0103;

const MAJOR_SHIFT: u32 = 16;
const MINOR_SHIFT: u32 = 8;

/// The selector numbers the gestalt opcode answers.
pub mod selector {
    pub const GLULX_VERSION: u32 = 0;
    pub const TERP_VERSION: u32 = 1;
    pub const RESIZE_MEM: u32 = 2;
    pub const UNDO: u32 = 3;
    pub const IO_SYSTEM: u32 = 4;
    pub const UNICODE: u32 = 5;
    pub const MEM_COPY: u32 = 6;
    pub const MALLOC: u32 = 7;
    pub const MALLOC_HEAP: u32 = 8;
    pub const ACCELERATION: u32 = 9;
    pub const ACCEL_FUNC: u32 = 10;
    pub const FLOAT: u32 = 11;
    pub const EXT_UNDO: u32 = 12;
    pub const DOUBLE: u32 = 13;
}

/// The io systems the IO_SYSTEM selector is asked about.
pub const IOSYS_NULL: u32 = 0;
pub const IOSYS_FILTER: u32 = 1;
pub const IOSYS_GLK: u32 = 2;

/// What this build of the machine can currently do.
///
/// `glk` says whether a Glk library is installed on this machine:
/// the bridge answers the glk opcode and iosys mode 2 works.
/// Everything else names the era that built it -- setmemsize
/// (`resize_mem`), mzero and mcopy (`mem_copy`), the wide strings
/// (`unicode`), saveundo and restoreundo (`undo`), hasundo and
/// discardundo (`ext_undo`), malloc and mfree (`malloc`),
/// accelfunc and accelparam (`acceleration`), and the float and
/// double opcodes (`floats`, `doubles`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub resize_mem: bool,
    pub mem_copy: bool,
    pub unicode: bool,
    pub undo: bool,
    pub ext_undo: bool,
    pub malloc: bool,
    pub acceleration: bool,
    pub floats: bool,
    pub doubles: bool,
    pub glk: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            resize_mem: true,
            mem_copy: true,
            unicode: true,
            undo: true,
            ext_undo: true,
            malloc: true,
            acceleration: true,
            floats: true,
            doubles: true,
            glk: false,
        }
    }
}

/// Voxam's own version, packed the way the header packs one.
///
/// Read from the crate metadata so the answer can never drift from
/// Cargo.toml: release 1.2.3 answers 0x00010203.
pub fn terp_version() -> u32 {
    let mut parts = env!("CARGO_PKG_VERSION")
        .split('.')
        .map(|part| part.parse::<u32>().expect("a dotted numeric crate version"));

    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0);

    (major << MAJOR_SHIFT) | (minor << MINOR_SHIFT) | patch
}

/// One gestalt query, answered honestly.
///
/// `heap_start` is the allocation heap's start address, zero with
/// no blocks extant; `accel_available` holds the function numbers
/// this interpreter can replace. Unknown selectors answer zero
/// rather than erring: that is how a program written against a
/// future spec probes an older interpreter (Glulx: Gestalt).
pub fn answer(
    capabilities: &Capabilities,
    heap_start: u32,
    accel_available: &[u32],
    selector: u32,
    argument: u32,
) -> u32 {
    match selector {
        selector::GLULX_VERSION => GLULX_VERSION,
        selector::TERP_VERSION => terp_version(),
        selector::RESIZE_MEM => u32::from(capabilities.resize_mem),
        selector::UNDO => u32::from(capabilities.undo),

        // The null and filter systems always work; Glk is its own
        // era's promise to keep.
        selector::IO_SYSTEM => match argument {
            IOSYS_NULL | IOSYS_FILTER => 1,
            IOSYS_GLK => u32::from(capabilities.glk),
            _ => 0,
        },

        selector::UNICODE => u32::from(capabilities.unicode),
        selector::MEM_COPY => u32::from(capabilities.mem_copy),
        selector::MALLOC => u32::from(capabilities.malloc),

        // The heap's start address, or zero with no blocks extant
        // (Glulx: Gestalt).
        selector::MALLOC_HEAP => heap_start,

        selector::ACCELERATION => u32::from(capabilities.acceleration),

        // Per function: which numbers this interpreter can replace
        // (Glulx: Gestalt).
        selector::ACCEL_FUNC => u32::from(accel_available.contains(&argument)),

        selector::FLOAT => u32::from(capabilities.floats),
        selector::EXT_UNDO => u32::from(capabilities.ext_undo),
        selector::DOUBLE => u32::from(capabilities.doubles),

        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The function numbers the accelerator will implement: 1
    // through 13, per the reference's roster.
    fn accel() -> Vec<u32> {
        (1..=13).collect()
    }

    // The interpreter's own version packs from the crate metadata,
    // so the gestalt answer can never drift from Cargo.toml.
    #[test]
    fn the_terp_version_packs_from_the_crate() {
        let mut parts = env!("CARGO_PKG_VERSION")
            .split('.')
            .map(|part| part.parse::<u32>().unwrap());

        let major = parts.next().unwrap();
        let minor = parts.next().unwrap();
        let patch = parts.next().unwrap();

        assert_eq!(terp_version(), (major << 16) | (minor << 8) | patch);
    }

    // Every selector answers for this build: the spec version, the
    // eras already carried at 1, and the io systems that exist --
    // with Glk honestly not among them yet. Unknown selectors
    // answer zero, which is how future programs probe old
    // interpreters.
    #[test]
    fn every_selector_answers_for_this_build() {
        let capabilities = Capabilities::default();
        let available = accel();
        let asked =
            |selector: u32, argument: u32| answer(&capabilities, 0, &available, selector, argument);

        let answers = [
            (0, GLULX_VERSION),
            (1, terp_version()),
            (2, 1),
            (3, 1),
            (5, 1),
            (6, 1),
            (7, 1),
            (8, 0),
            (9, 1),
            (10, 0),
            (11, 1),
            (12, 1),
            (13, 1),
            (99, 0),
        ];

        for (selector, expected) in answers {
            assert_eq!(asked(selector, 0), expected);
        }

        assert_eq!(asked(4, 0), 1);
        assert_eq!(asked(4, 1), 1);
        assert_eq!(asked(4, 2), 0);
        assert_eq!(asked(4, 9), 0);
        assert_eq!(asked(10, 1), 1);
        assert_eq!(asked(10, 13), 1);
        assert_eq!(asked(10, 14), 0);
    }

    // The Glk era flips its flag, and a heap with blocks extant
    // answers its start address.
    #[test]
    fn the_later_eras_answer_when_they_arrive() {
        let with_glk = Capabilities {
            glk: true,
            ..Default::default()
        };

        assert_eq!(answer(&with_glk, 0, &accel(), 4, 2), 1);
        assert_eq!(answer(&with_glk, 0x8000, &accel(), 8, 0), 0x8000);
    }
}
