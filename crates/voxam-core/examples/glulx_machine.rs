//! The machine-era parity half: what the Glulx machine does with
//! no Glk library installed, printed for certify to diff against
//! the reference oracle.
//!
//! Two acts: first the byte-exact save of a known mutated state --
//! CMem compression, MAll heap chunk, Stks stub and all -- then
//! every .ulx in the given corpus booted bare and run until it
//! quits or halts, the step count and the halt spoken.

use std::path::PathBuf;

use voxam_core::glulx::machine::Machine;
use voxam_core::glulx::serial;
use voxam_core::glulx::stack::dest_type;
use voxam_core::glulx::story::Story;

const IDLE: &[u8] = &[0xC0, 0x00, 0x00, 0x81, 0x20];

const LIMIT: u64 = 200_000;

/// The oracle's tiny image: ROM to $100, stored RAM to $200, the
/// map to $300, the idle main at $48.
fn image() -> Vec<u8> {
    let mut data = vec![0u8; 0x200];

    data[0..4].copy_from_slice(b"Glul");
    data[4..8].copy_from_slice(&0x0003_0102u32.to_be_bytes());
    data[8..12].copy_from_slice(&0x100u32.to_be_bytes());
    data[12..16].copy_from_slice(&0x200u32.to_be_bytes());
    data[16..20].copy_from_slice(&0x300u32.to_be_bytes());
    data[20..24].copy_from_slice(&0x100u32.to_be_bytes());
    data[24..28].copy_from_slice(&0x48u32.to_be_bytes());
    data[28..32].copy_from_slice(&0x54u32.to_be_bytes());
    data[0x48..0x48 + IDLE.len()].copy_from_slice(IDLE);

    let checksum = (0..data.len()).step_by(4).fold(0u32, |total, at| {
        total.wrapping_add(u32::from_be_bytes([
            data[at],
            data[at + 1],
            data[at + 2],
            data[at + 3],
        ]))
    });

    data[32..36].copy_from_slice(&checksum.to_be_bytes());

    data
}

fn save_vector() {
    let mut machine = Machine::new(Story::new(image()).unwrap(), None).unwrap();

    machine.memory.write_byte(0x150, 0x42).unwrap();
    machine.memory.set_size(0x400).unwrap();
    machine.memory.write_byte(0x350, 0x77).unwrap();
    machine.heap.alloc(&mut machine.memory, 0x40).unwrap();
    machine.heap.alloc(&mut machine.memory, 0x30).unwrap();
    machine.stack.push(123).unwrap();
    machine
        .stack
        .push_stub(dest_type::MEMORY, 0x140, 0x1234)
        .unwrap();

    let saved = serial::serialize(&machine).unwrap();
    let hex: String = saved.iter().map(|byte| format!("{byte:02x}")).collect();

    println!("save: {hex}");
}

fn bare_runs(corpus: &PathBuf) {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(corpus)
        .expect("a readable corpus directory")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "ulx"))
        .collect();

    paths.sort();

    for path in paths {
        let name = path.file_name().unwrap().to_string_lossy();
        let data = std::fs::read(&path).expect("a readable story");

        let story = match Story::new(data) {
            Ok(story) => story,
            Err(error) => {
                println!("{name}: boot refused: {error}");

                continue;
            }
        };

        let mut machine = match Machine::new(story, None) {
            Ok(machine) => machine,
            Err(error) => {
                println!("{name}: boot refused: {error}");

                continue;
            }
        };

        let mut steps: u64 = 0;
        let mut spoken = false;

        while machine.running() {
            if steps >= LIMIT {
                println!("{name}: still running after {steps} steps");

                spoken = true;

                break;
            }

            if let Err(error) = machine.step() {
                println!("{name}: {steps} steps, halted: {error}");

                spoken = true;

                break;
            }

            steps += 1;
        }

        if !spoken {
            println!("{name}: quit after {steps} steps");
        }
    }
}

fn main() {
    save_vector();

    if let Some(corpus) = std::env::args().nth(1).map(PathBuf::from) {
        bare_runs(&corpus);
    }
}
