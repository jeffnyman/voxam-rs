//! Walk an Å-machine story through the plain voice, the reference
//! Node frontend's own drill, and print the whole telling raw.
//! The certify sweep diffs the output byte for byte against the
//! reference's vendored gold transcripts, which are the community
//! fork's own engine at seed 1234.
//!
//! Usage: aawalk <story> [script] [--saves]
//!
//! With --saves the voice declares savefile support (keeping no
//! files), the variant whose gold carries the SAVEFILE feature
//! lines; that walk takes no script.

use std::io::Write;

use voxam_core::aamachine::machine::{Machine, walked};
use voxam_core::aamachine::output::{PlainVoice, Voice};
use voxam_core::aamachine::story::Story;

fn main() {
    let mut paths: Vec<String> = Vec::new();
    let mut saves = false;

    for arg in std::env::args().skip(1) {
        if arg == "--saves" {
            saves = true;
        } else {
            paths.push(arg);
        }
    }

    let story_path = paths
        .first()
        .expect("usage: aawalk <story> [script] [--saves]");
    let data = std::fs::read(story_path).expect("readable story");
    let story = Story::new(&data).expect("a story file");
    let script = paths
        .get(1)
        .map(|path| {
            String::from_utf8(std::fs::read(path).expect("readable script"))
                .expect("a UTF-8 script")
        })
        .unwrap_or_default();

    let told = if saves {
        let voice = PlainVoice::new(&story).expect("a coherent LOOK").keeping();
        let mut machine = Machine::new(story, voice, Some(1234)).expect("a runnable story");

        machine.run(None).expect("a clean run");
        machine.voice.line();

        machine.voice.told().to_string()
    } else {
        walked(story, &script, Some(1234)).expect("a clean walk")
    };

    std::io::stdout()
        .write_all(told.as_bytes())
        .expect("stdout stands open");
}
