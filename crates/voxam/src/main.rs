//! The `voxam` command. Three faces so far: point it at a
//! Z-Machine story and it plays on the plain stream; ask for
//! `--header` and it reads the story's manifest (§11.1); any
//! other format is named by its magic.

mod accept;
mod glance;

use std::io::BufRead;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use voxam_core::format::{StoryFormat, sniff};
use voxam_core::frontend::plain;
use voxam_core::zmachine::machine::{Identity, Machine, RunState};
use voxam_core::zmachine::story::Story;

/// The exit codes the Python CLI speaks: 0 for a served request,
/// 2 for one that could not be used.
const EXIT_OK: u8 = 0;
const EXIT_UNUSABLE: u8 = 2;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    match arguments.as_slice() {
        [flag, story] if flag == "--header" => header_report(story),
        [flag, script] if flag == "--accept" => accepted_session(script),
        [story] if !story.starts_with("--") => play(story, None),
        [seed_flag, seed, story] if seed_flag == "--seed" => match seed.parse::<u32>() {
            Ok(seed) => play(story, Some(seed)),
            Err(_) => {
                eprintln!("voxam: --seed takes a number, not {seed:?}");
                ExitCode::from(EXIT_UNUSABLE)
            }
        },
        _ => {
            eprintln!("usage: voxam [--header] [--accept script] [--seed N] <story-file>");
            ExitCode::from(EXIT_UNUSABLE)
        }
    }
}

/// Replay an acceptance script: the recorded commands type
/// themselves, each echoed through the stream, and the session
/// ends when they run out, as at end of input.
fn accepted_session(script_path: &str) -> ExitCode {
    println!("\nVoxam Interpreter for Z-Machine and Glulx Stories\n");

    let script = match accept::AcceptanceScript::parse(Path::new(script_path)) {
        Ok(script) => script,
        Err(error) => {
            println!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    let bytes = match std::fs::read(&script.game) {
        Ok(bytes) => bytes,
        Err(error) => {
            println!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    let session = Story::new(bytes).and_then(|story| {
        let header = story.header();

        println!(
            "Running {}: release {}, serial {} (z{})\n",
            script
                .game
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            header.release(),
            header.serial_number(),
            header.version()
        );

        // The refusal watch reads the replayed conversation: the
        // response to a command is everything the story prints
        // before the next command is typed.
        let seen = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
        // Saved games live beside the story: zork1.z3 saves to
        // zork1.sav, where every other interpreter can find them.
        let saves = voxam_core::saves::FileSaveSlot {
            path: script.game.with_extension("sav"),
        };
        let mut machine = Machine::new(
            story,
            Box::new(watched_stream(seen.clone())),
            script.seed,
            Identity::default(),
            Some(Box::new(saves)),
        )?;
        let mut commands = script.commands.iter();
        let mut awaiting: Option<&(String, usize)> = None;

        let judge = |awaiting: &Option<&(String, usize)>| {
            if let Some((command, line)) = awaiting
                && let Some(offense) = accept::refusal_in(&seen.borrow())
            {
                println!(
                    "voxam: line {line}: {} looks refused: {}",
                    accept::shown(command),
                    offense.trim()
                );
            }

            seen.borrow_mut().clear();
        };

        loop {
            match machine.run()? {
                RunState::Halted => {
                    judge(&awaiting);
                    println!();
                    return Ok(true);
                }
                RunState::Waiting => match commands.next() {
                    Some(entry) => {
                        judge(&awaiting);
                        awaiting = Some(entry);

                        // The replay's echo: the transcript shows
                        // what was entered at each prompt, since no
                        // fingers ever typed it there.
                        println!("{}", accept::echoed(&entry.0));
                        let _ = std::io::stdout().flush();
                        machine.deliver_line(&entry.0, 0)?;
                    }
                    None => {
                        judge(&awaiting);
                        return Ok(false);
                    }
                },
            }
        }
    });

    match session {
        Ok(halted) => {
            if !halted {
                println!("\nvoxam: end of input");
            }

            ExitCode::from(EXIT_OK)
        }
        Err(error) => {
            println!("\nvoxam: {error}");
            ExitCode::from(EXIT_UNUSABLE)
        }
    }
}

/// Play a Z-Machine story on the plain stream: text flows out,
/// lines flow in, and end of input ends the session.
fn play(path: &str, seed: Option<u32>) -> ExitCode {
    println!("\nVoxam Interpreter for Z-Machine and Glulx Stories\n");

    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            println!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    match sniff(&bytes) {
        Some(StoryFormat::ZCode { .. }) => {}
        Some(other) => {
            let name = match other {
                StoryFormat::Glulx => "Glulx",
                StoryFormat::Blorb => "a Blorb resource file",
                StoryFormat::AaMachine => "an \u{c5}-machine story",
                StoryFormat::ZCode { .. } => unreachable!(),
            };
            println!(
                "voxam: only Z-Machine stories play yet, and {} is {name}",
                basename(path)
            );
            return ExitCode::from(EXIT_UNUSABLE);
        }
        None => {
            println!(
                "voxam: {} is not a story file Voxam recognizes",
                basename(path)
            );
            return ExitCode::from(EXIT_UNUSABLE);
        }
    }

    let session = Story::new(bytes)
        .and_then(|story| {
            let saves = voxam_core::saves::FileSaveSlot {
                path: Path::new(path).with_extension("sav"),
            };

            Machine::new(
                story,
                Box::new(plain()),
                seed,
                Identity::default(),
                Some(Box::new(saves)),
            )
        })
        .and_then(|mut machine| {
            let stdin = std::io::stdin();
            let mut lines = stdin.lock().lines();

            loop {
                match machine.run()? {
                    RunState::Halted => {
                        println!();
                        return Ok(());
                    }
                    RunState::Waiting => match lines.next() {
                        Some(Ok(line)) => machine.deliver_line(&line, 0)?,
                        // End of input ends the session, as the
                        // acceptance contract asks.
                        _ => {
                            println!(
                                "
voxam: end of input"
                            );
                            return Ok(());
                        }
                    },
                }
            }
        });

    match session {
        Ok(()) => ExitCode::from(EXIT_OK),
        Err(error) => {
            println!("voxam: {error}");
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

/// The plain stream with the refusal watch's ear: everything
/// prints onward and is kept for judging the response.
fn watched_stream(
    seen: std::rc::Rc<std::cell::RefCell<String>>,
) -> voxam_core::frontend::StreamFrontend<impl FnMut(&str)> {
    voxam_core::frontend::StreamFrontend::new(move |text: &str| {
        print!("{text}");
        let _ = std::io::stdout().flush();
        seen.borrow_mut().push_str(text);
    })
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}
