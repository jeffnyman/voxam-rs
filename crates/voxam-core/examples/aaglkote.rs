//! Speak the Å-machine GlkOte protocol for one story on stdio --
//! the certification twin of the reference's serving.
//!
//! Usage: aaglkote <story.aastory> [seed]

use std::io::BufReader;

use voxam_core::aamachine::glkote::serve;
use voxam_core::aamachine::story::Story;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("usage: aaglkote <story> [seed]");
    let seed: Option<u32> = arguments.next().map(|held| held.parse().expect("a seed"));
    let bytes = std::fs::read(&path).expect("a readable story");
    let story = Story::new(&bytes).expect("a valid story");

    let mut reader = BufReader::new(std::io::stdin());
    let mut writer = std::io::stdout();

    let clean = serve(story, &mut reader, &mut writer, seed);

    std::process::exit(if clean { 0 } else { 1 });
}
