//! The `voxam` command: for now, the smallest honest slice --
//! point it at a story file and it names the format, by the
//! file's own magic rather than its suffix.

use std::process::ExitCode;

use voxam_core::format::{StoryFormat, sniff};

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: voxam <story-file>");
        return ExitCode::FAILURE;
    };

    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("voxam: {path}: {err}");
            return ExitCode::FAILURE;
        }
    };

    match sniff(&bytes) {
        Some(StoryFormat::ZCode { version }) => println!("Z-code, version {version}"),
        Some(StoryFormat::Glulx) => println!("Glulx"),
        Some(StoryFormat::Blorb) => println!("Blorb resource file"),
        Some(StoryFormat::AaMachine) => println!("Å-machine story"),
        None => {
            eprintln!("voxam: {path}: not a story file Voxam recognizes");
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}
