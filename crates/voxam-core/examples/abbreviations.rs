//! Print every abbreviation a story defines, one per line, as
//! space-separated hexadecimal text units -- the certify sweep's
//! escaping-proof rendering. The Python reference prints the same
//! form; the two must agree line for line.

use voxam_core::zmachine::memory::Memory;
use voxam_core::zmachine::story::Story;
use voxam_core::zmachine::zscii::decode_units;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: abbreviations <story>");
    let story = Story::new(std::fs::read(&path).expect("readable story")).expect("a story file");
    let memory = Memory::new(&story).expect("a coherent memory map");

    let header = memory.header();
    let version = header.version();
    let table = usize::from(header.abbreviations_table_address());

    // Version 1 has no abbreviations; Version 2 has one bank of 32,
    // Version 3 up three banks of 96 (§3.3); a zero table address
    // means none are defined.
    let count = match version {
        1 => 0,
        2 => 32,
        _ => 96,
    };

    for entry_number in 0..count {
        if table == 0 {
            break;
        }

        let line = memory
            .fetch_word(table + 2 * entry_number)
            .and_then(|entry| decode_units(&memory, 2 * usize::from(entry)))
            .map(|(units, _)| {
                units
                    .iter()
                    .map(|unit| format!("{unit:04X}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            });

        match line {
            Ok(units) => println!("{entry_number}: {units}"),
            Err(error) => println!("{entry_number}: ERROR {error}"),
        }
    }
}
