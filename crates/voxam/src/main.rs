//! The `voxam` command. Two faces so far: point it at a story
//! file and it names the format by the file's own magic, or ask
//! for `--header` and it reads a Z-Machine story's manifest
//! (§11.1) and reports it, rendered identically to the Python
//! implementation.

mod glance;

use std::path::Path;
use std::process::ExitCode;

use voxam_core::format::{StoryFormat, sniff};
use voxam_core::zmachine::story::Story;

/// The exit codes the Python CLI speaks: 0 for a served request,
/// 2 for one that could not be used.
const EXIT_OK: u8 = 0;
const EXIT_UNUSABLE: u8 = 2;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    match arguments.as_slice() {
        [flag, story] if flag == "--header" => header_report(story),
        [story] if story != "--header" => name_format(story),
        _ => {
            eprintln!("usage: voxam [--header] <story-file>");
            ExitCode::from(EXIT_UNUSABLE)
        }
    }
}

/// Serve `--header`: the story's own manifest, reported (§11.1).
fn header_report(path: &str) -> ExitCode {
    // The greeting the Python CLI prints for every face but the
    // wire, whose stdout carries stanzas and nothing else.
    println!("\nVoxam Interpreter for Z-Machine and Glulx Stories\n");

    let name = basename(path);

    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            println!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    match sniff(&bytes) {
        Some(StoryFormat::Glulx) => {
            println!("voxam: --header reads Z-Machine stories, and {name} is Glulx");
            return ExitCode::from(EXIT_UNUSABLE);
        }
        Some(StoryFormat::AaMachine) => {
            println!(
                "voxam: --header reads Z-Machine stories, and {name} is an \u{c5}-machine story"
            );
            return ExitCode::from(EXIT_UNUSABLE);
        }
        Some(StoryFormat::Blorb) => {
            println!(
                "voxam: Blorb-packaged stories await the blorb module; point --header at a bare story"
            );
            return ExitCode::from(EXIT_UNUSABLE);
        }
        Some(StoryFormat::ZCode { .. }) | None => {}
    }

    let story = match Story::new(bytes) {
        Ok(story) => story,
        Err(error) => {
            println!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    println!("{name}\n");
    println!("{}", glance::report(&story));

    ExitCode::from(EXIT_OK)
}

/// The default face for now: name a story file's format.
fn name_format(path: &str) -> ExitCode {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("voxam: {path}: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    match sniff(&bytes) {
        Some(StoryFormat::ZCode { version }) => println!("Z-code, version {version}"),
        Some(StoryFormat::Glulx) => println!("Glulx"),
        Some(StoryFormat::Blorb) => println!("Blorb resource file"),
        Some(StoryFormat::AaMachine) => println!("\u{c5}-machine story"),
        None => {
            eprintln!("voxam: {path}: not a story file Voxam recognizes");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    }

    ExitCode::from(EXIT_OK)
}

/// The file's own name, as the Python CLI prints it.
fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}
