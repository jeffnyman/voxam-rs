//! Speak the Z GlkOte protocol for one story on stdio.
//!
//! The certification twin of the reference CLI's --glkote path:
//! the story loads with its resources -- a Blorb package by
//! suffix, or a like-named sidecar beside a bare story -- and the
//! session serves stanza by stanza until the display hangs up.
//!
//! Usage: zglkote <story> [seed]

use std::io::BufReader;

use voxam_core::blorb::Blorb;
use voxam_core::glulx::glk::resources::Resources;
use voxam_core::zmachine::glkote::{fronted, serve};
use voxam_core::zmachine::story::Story;

const BLORB_SUFFIXES: [&str; 4] = ["blb", "blorb", "zblorb", "gblorb"];

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = std::path::PathBuf::from(arguments.next().expect("usage: zglkote <story> [seed]"));
    let seed: Option<u32> = arguments.next().map(|held| held.parse().expect("a seed"));

    let suffix = path
        .extension()
        .map(|held| held.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let bytes = std::fs::read(&path).expect("a readable story");

    let (story, blorb) = if BLORB_SUFFIXES.contains(&suffix.as_str()) {
        let blorb = Blorb::parse(&bytes).expect("a readable Blorb");
        let packaged = blorb.story().expect("a packaged Z-code story").to_vec();

        (Story::new(packaged).expect("a valid story"), Some(blorb))
    } else {
        let story = Story::new(bytes).expect("a valid story");
        let sidecar = BLORB_SUFFIXES.iter().find_map(|suffix| {
            let beside = path.with_extension(suffix);

            beside
                .exists()
                .then(|| std::fs::read(&beside).expect("a readable sidecar"))
                .map(|bytes| Blorb::parse(&bytes).expect("a readable Blorb"))
        });

        (story, sidecar)
    };

    let frontend =
        fronted(story.version(), Some(Resources::new(blorb))).expect("a servable version");
    let mut reader = BufReader::new(std::io::stdin());
    let mut writer = std::io::stdout();

    let clean = serve(story, frontend, &mut reader, &mut writer, seed);

    std::process::exit(if clean { 0 } else { 1 });
}
