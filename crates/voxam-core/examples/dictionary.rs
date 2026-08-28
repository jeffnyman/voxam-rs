//! Print a story's dictionary: the table's shape, every entry
//! decoded to hexadecimal text units, and a sampled round trip --
//! every thirteenth entry's decoded word looked back up. The
//! Python reference prints the same form; the two must agree line
//! for line.

use voxam_core::zmachine::dictionary::Dictionary;
use voxam_core::zmachine::memory::Memory;
use voxam_core::zmachine::story::Story;
use voxam_core::zmachine::zscii::{decode_units, units_to_string};

const LOOKUP_SAMPLE_STRIDE: usize = 13;

fn main() {
    let path = std::env::args().nth(1).expect("usage: dictionary <story>");
    let story = Story::new(std::fs::read(&path).expect("readable story")).expect("a story file");
    let memory = Memory::new(&story).expect("a coherent memory map");

    let base = usize::from(memory.header().dictionary_address());
    let separator_count = usize::from(memory.read_byte(base).expect("separator count"));
    let entry_length = usize::from(
        memory
            .read_byte(base + 1 + separator_count)
            .expect("entry length"),
    );
    let entries = base + 4 + separator_count;

    let dictionary = match Dictionary::new(&memory, None) {
        Ok(dictionary) => dictionary,
        Err(error) => {
            println!("DICTIONARY ERROR {error}");
            return;
        }
    };

    println!(
        "base={base:04X} separators={separator_count} length={entry_length} count={}",
        dictionary.entry_count()
    );

    for index in 0..dictionary.entry_count() {
        let address = entries + index * entry_length;

        match decode_units(&memory, address) {
            Ok((units, _)) => {
                let hex: Vec<String> = units.iter().map(|unit| format!("{unit:04X}")).collect();
                println!("{index}: {hex}", hex = hex.join(" "));

                if index % LOOKUP_SAMPLE_STRIDE == 0 {
                    match dictionary.lookup(&units_to_string(&units)) {
                        Ok(found) => println!("  lookup -> {found:04X}"),
                        Err(error) => println!("  lookup -> ERROR {error}"),
                    }
                }
            }
            Err(error) => println!("{index}: ERROR {error}"),
        }
    }
}
