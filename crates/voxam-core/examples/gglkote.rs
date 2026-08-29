//! Speak the Glulx GlkOte protocol for one story on stdio.
//!
//! The certification twin of the reference CLI's --glkote path for
//! Glulx: the story loads with its resources -- a Blorb package by
//! suffix, or a like-named sidecar beside a bare .ulx -- and the
//! session serves stanza by stanza until the display hangs up.
//!
//! Usage: gglkote <story> [seed]

use std::io::BufReader;

use voxam_core::blorb::Blorb;
use voxam_core::glulx::glk::glkote::{opened, serve};
use voxam_core::glulx::story::Story;

const BLORB_SUFFIXES: [&str; 4] = ["blb", "blorb", "zblorb", "gblorb"];

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = std::path::PathBuf::from(arguments.next().expect("usage: gglkote <story> [seed]"));
    let seed: Option<u32> = arguments.next().map(|held| held.parse().expect("a seed"));

    let suffix = path
        .extension()
        .map(|held| held.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let bytes = std::fs::read(&path).expect("a readable story");

    let (story, blorb) = if BLORB_SUFFIXES.contains(&suffix.as_str()) {
        let blorb = Blorb::parse(&bytes).expect("a readable Blorb");
        let packaged = blorb.glulx().expect("a packaged Glulx story").to_vec();

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

    let (mut machine, face) = opened(story, blorb, seed).expect("a bootable story");
    let mut reader = BufReader::new(std::io::stdin());
    let mut writer = std::io::stdout();

    let clean = serve(&mut machine, &face, &mut reader, &mut writer);

    std::process::exit(if clean { 0 } else { 1 });
}
