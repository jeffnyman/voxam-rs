//! Print a story's object table: every object's family relations,
//! raw attribute bytes, decoded short name, and property walk. The
//! Python reference prints the same form; the two must agree line
//! for line.
//!
//! The table stores no object count, so the census uses the classic
//! heuristic the inspection tools use: entries end where the lowest
//! property table begins.

use voxam_core::zmachine::memory::Memory;
use voxam_core::zmachine::objects::ObjectTable;
use voxam_core::zmachine::story::Story;
use voxam_core::zmachine::zscii::decode_units;

fn main() {
    let path = std::env::args().nth(1).expect("usage: objects <story>");
    let story = Story::new(std::fs::read(&path).expect("readable story")).expect("a story file");
    let memory = Memory::new(&story).expect("a coherent memory map");
    let table = ObjectTable::new(&memory);

    let version = memory.header().version();
    let v3 = version <= 3;
    let base = usize::from(memory.header().object_table_address());
    let entries = base + 2 * if v3 { 31 } else { 63 };
    let entry_size = if v3 { 9 } else { 14 };
    let attribute_bytes = if v3 { 4 } else { 6 };
    let max_object: u16 = if v3 { 255 } else { 65535 };

    // The census: walk objects until an entry would overlap the
    // lowest property table seen.
    let mut limit: Option<usize> = None;
    let mut count: u16 = 0;

    for obj in 1..=max_object {
        let offset = entries + usize::from(obj - 1) * entry_size;

        if limit.is_some_and(|limit| offset + entry_size > limit) {
            break;
        }

        let Ok(name_address) = table.short_name_address(&memory, obj) else {
            break;
        };
        let property_table = name_address - 1;

        if property_table > 0 {
            limit = Some(limit.map_or(property_table, |seen| seen.min(property_table)));
        }

        count = obj;
    }

    println!("base={base:04X} count={count}");

    for obj in 1..=count {
        let line = describe(&memory, &table, obj, entries, entry_size, attribute_bytes);

        match line {
            Ok(text) => println!("{obj}: {text}"),
            Err(error) => println!("{obj}: ERROR {error}"),
        }
    }
}

fn describe(
    memory: &Memory,
    table: &ObjectTable,
    obj: u16,
    entries: usize,
    entry_size: usize,
    attribute_bytes: usize,
) -> Result<String, voxam_core::errors::VoxamError> {
    let parent = table.parent(memory, obj)?;
    let sibling = table.sibling(memory, obj)?;
    let child = table.child(memory, obj)?;

    let entry = entries + usize::from(obj - 1) * entry_size;
    let mut attributes = String::new();
    for offset in 0..attribute_bytes {
        attributes.push_str(&format!("{:02X}", memory.read_byte(entry + offset)?));
    }

    let name_address = table.short_name_address(memory, obj)?;
    let name_words = memory.read_byte(name_address - 1)?;
    let name = if name_words == 0 {
        "-".to_string()
    } else {
        let (units, _) = decode_units(memory, name_address)?;
        units
            .iter()
            .map(|unit| format!("{unit:04X}"))
            .collect::<Vec<_>>()
            .join(" ")
    };

    // One linear pass down the property list (§12.4), capped at 64
    // entries: no legitimate list exceeds 63 properties, so the cap
    // only truncates walks through junk -- which a corrupt story
    // can offer -- identically on both sides.
    let v3 = memory.header().version() <= 3;
    let mut properties = String::new();
    let mut address = name_address + 2 * usize::from(name_words);

    for step in 0..=64 {
        if step == 64 {
            properties.push_str(" ...");
            break;
        }

        let first = memory.read_byte(address)?;

        if first == 0 {
            break;
        }

        let (number, length, data) = if v3 {
            (first & 0x1F, usize::from(first / 32) + 1, address + 1)
        } else if first & 0x80 != 0 {
            let length = usize::from(memory.read_byte(address + 1)? & 0x3F);
            (
                first & 0x3F,
                if length == 0 { 64 } else { length },
                address + 2,
            )
        } else {
            let length = if first & 0x40 != 0 { 2 } else { 1 };
            (first & 0x3F, length, address + 1)
        };

        let mut bytes = String::new();
        for offset in 0..length {
            bytes.push_str(&format!("{:02X}", memory.read_byte(data + offset)?));
        }

        properties.push_str(&format!(" {number}:{bytes}"));
        address = data + length;
    }

    Ok(format!(
        "p={parent} s={sibling} c={child} a={attributes} n=[{name}] props{properties}"
    ))
}
