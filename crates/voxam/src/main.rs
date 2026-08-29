//! The `voxam` command. Point it at a Z-Machine or Glulx story
//! and it plays on the plain stream; an Å-machine story plays at
//! the terminal, the third machine's own face; ask for `--header`
//! and it reads the story's manifest (§11.1); any other format is
//! named by its magic.

mod accept;
mod glance;

use std::io::BufRead;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use voxam_core::aamachine::story::Story as AAMachineStory;
use voxam_core::aamachine::terminal::played as aamachine_played;
use voxam_core::format::{StoryFormat, sniff};
use voxam_core::frontend::plain;
use voxam_core::glulx::glk::api::Glk;
use voxam_core::glulx::glk::resources::Resources;
use voxam_core::glulx::glk::stdio::StdioFrontend;
use voxam_core::glulx::machine::Machine as GlulxMachine;
use voxam_core::glulx::story::Story as GlulxStory;
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

    // The session instruments the other machines carry are
    // refused by name rather than half-working: the acceptance
    // driver is the third machine's later road.
    if aamachine_story(&script.game) {
        println!(
            "voxam: the Å-machine plays live for now -- the acceptance \
             driver and the tracer are later roads"
        );
        return ExitCode::from(EXIT_UNUSABLE);
    }

    match load_glulx(&script.game) {
        Err(error) => {
            println!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
        Ok(Some((story, blorb))) => return glulx_replay(&script, story, blorb),
        Ok(None) => {}
    }

    let (loaded, blorb) = match load_story(&script.game) {
        Ok(loaded) => loaded,
        Err(error) => {
            println!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    let session = (|story: Story| -> Result<bool, voxam_core::errors::VoxamError> {
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

        present_resources(&blorb, &story);

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
    })(loaded);

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

/// Whether the file opens as an Å-machine story's FORM AAVM.
fn aamachine_story(path: &Path) -> bool {
    std::fs::read(path)
        .is_ok_and(|data| data.len() >= 12 && &data[..4] == b"FORM" && &data[8..12] == b"AAVM")
}

/// Run one Å-machine story at the terminal, the reference
/// frontends' own shape, certified against their transcripts.
///
/// The dress waits on the honesty gate -- only a real terminal is
/// ever dressed -- and the width takes the shell's COLUMNS word
/// for it, the classic 80 otherwise: the painted terminal is the
/// milestone that learns to measure for itself.
fn aamachine_session(path: &str, seed: Option<u32>) -> ExitCode {
    use std::io::IsTerminal;

    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(error) => {
            println!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };
    let story = match AAMachineStory::new(&data) {
        Ok(story) => story,
        Err(error) => {
            println!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    let width = std::env::var("COLUMNS")
        .ok()
        .and_then(|columns| columns.parse::<i64>().ok())
        .unwrap_or(80);
    let dressed = std::io::stdout().is_terminal();
    // The line source and the filename prompt each lock stdin
    // only for their own read, so neither starves the other.
    let source = Box::new(|| {
        let mut line = String::new();

        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line),
        }
    });
    let asked = Box::new(|prompt: &str| {
        let mut out = std::io::stdout();
        let _ = out.write_all(prompt.as_bytes());
        let _ = out.flush();

        let mut answer = String::new();
        let _ = std::io::stdin().read_line(&mut answer);

        answer.trim_end_matches(['\r', '\n']).to_string()
    });

    match aamachine_played(
        story,
        seed,
        source,
        Box::new(std::io::stdout()),
        asked,
        width,
        dressed,
    ) {
        Ok(()) => ExitCode::from(EXIT_OK),
        Err(error) => {
            println!("voxam: {error}");
            ExitCode::from(EXIT_UNUSABLE)
        }
    }
}

/// Play a Z-Machine story on the plain stream: text flows out,
/// lines flow in, and end of input ends the session.
fn play(path: &str, seed: Option<u32>) -> ExitCode {
    println!("\nVoxam Interpreter for Z-Machine and Glulx Stories\n");

    if aamachine_story(Path::new(path)) {
        return aamachine_session(path, seed);
    }

    match load_glulx(Path::new(path)) {
        Err(error) => {
            println!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
        Ok(Some((story, blorb))) => {
            let stdin = std::io::stdin();
            let mut lines = stdin.lock().lines();

            return glulx_session(
                &basename(path),
                story,
                blorb,
                seed,
                Box::new(move || lines.next().and_then(Result::ok)),
                None,
                || {},
            );
        }
        Ok(None) => {}
    }

    let (loaded, blorb) = match load_story(Path::new(path)) {
        Ok(loaded) => loaded,
        Err(error) => {
            println!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    {
        let header = loaded.header();

        println!(
            "Running {}: release {}, serial {} (z{})\n",
            basename(path),
            header.release(),
            header.serial_number(),
            header.version()
        );
    }

    present_resources(&blorb, &loaded);

    let session = Ok(loaded)
        .and_then(|story: Story| {
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
        Some(StoryFormat::Blorb) | Some(StoryFormat::ZCode { .. }) | None => {}
    }

    if let Some(StoryFormat::Blorb) = sniff(&bytes)
        && let Ok(blorb) = voxam_core::blorb::Blorb::parse(&bytes)
        && blorb.glulx().is_some()
    {
        println!("voxam: --header reads Z-Machine stories, and {name} is Glulx");
        return ExitCode::from(EXIT_UNUSABLE);
    }

    let story = match load_story(Path::new(path)) {
        Ok((story, _)) => story,
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

/// Load a story as Glulx if that is what it is: a bare Glulx file
/// (with any like-named Blorb sidecar), or a Blorb packaging a
/// Glulx story. Ok(None) means the path is not Glulx at all and
/// the Z-Machine loader should have it.
fn load_glulx(
    path: &Path,
) -> Result<Option<(GlulxStory, Option<voxam_core::blorb::Blorb>)>, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;

    let suffix = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase());

    if let Some(suffix) = &suffix
        && BLORB_SUFFIXES.contains(&suffix.as_str())
    {
        let blorb = voxam_core::blorb::Blorb::parse(&bytes).map_err(|error| error.to_string())?;

        let Some(packaged) = blorb.glulx() else {
            return Ok(None);
        };

        let story = GlulxStory::new(packaged.to_vec()).map_err(|error| error.to_string())?;

        return Ok(Some((story, Some(blorb))));
    }

    if !matches!(sniff(&bytes), Some(StoryFormat::Glulx)) {
        return Ok(None);
    }

    let story = GlulxStory::new(bytes).map_err(|error| error.to_string())?;

    for sidecar_suffix in BLORB_SUFFIXES {
        let sidecar = path.with_extension(sidecar_suffix);

        if sidecar.exists() {
            let bytes = std::fs::read(&sidecar).map_err(|error| error.to_string())?;
            let blorb =
                voxam_core::blorb::Blorb::parse(&bytes).map_err(|error| error.to_string())?;

            return Ok(Some((story, Some(blorb))));
        }
    }

    Ok(Some((story, None)))
}

/// Run a Glulx story over the stdio display: the banner, the
/// session, a final flush for whatever quit left unshown, and the
/// closing blank line -- the reference CLI's exact shape.
fn glulx_session(
    name: &str,
    story: GlulxStory,
    blorb: Option<voxam_core::blorb::Blorb>,
    seed: Option<u32>,
    source: Box<dyn FnMut() -> Option<String>>,
    witness: Option<voxam_core::glulx::glk::stdio::Witness>,
    finish: impl FnOnce(),
) -> ExitCode {
    // The checksum verdict is printed but does not gate the run:
    // the verify opcode exists so a story can judge itself.
    let verdict = if story.verify() {
        "checksum verified"
    } else {
        "CHECKSUM MISMATCH"
    };

    println!("Running {name}: Glulx {}, {verdict}\n", story.version());

    let frontend = StdioFrontend::new(
        Box::new(|text: &str| {
            print!("{text}");
            let _ = std::io::stdout().flush();
        }),
        source,
        witness,
    );
    let mut library = Glk::new(Box::new(frontend));

    library.resources = Resources::new(blorb);

    let session = GlulxMachine::new(story, seed).and_then(|mut machine| {
        machine.install_glk(library);
        machine.run(None)?;

        // A story that ends with quit rather than glk_exit never
        // asked for a last flush; whatever its windows still hold
        // is shown on the way out.
        if let Some(glk) = machine.glk_mut() {
            let root = glk.root;

            glk.frontend.flush(&mut glk.windows, root);
        }

        Ok(())
    });

    finish();

    match session {
        Ok(()) => {
            println!();
            ExitCode::from(EXIT_OK)
        }
        Err(error) => {
            println!("\nvoxam: {error}");
            ExitCode::from(EXIT_UNUSABLE)
        }
    }
}

/// Replay an acceptance script on the Glulx machine: the recorded
/// commands type themselves through the stdio display's own input
/// seam, the refusal watch listens at its witness seam, and the
/// session ends quietly when the script runs out.
fn glulx_replay(
    script: &accept::AcceptanceScript,
    story: GlulxStory,
    blorb: Option<voxam_core::blorb::Blorb>,
) -> ExitCode {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
    let awaiting: std::rc::Rc<std::cell::RefCell<Option<(String, usize)>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));

    let judge: std::rc::Rc<dyn Fn()> = {
        let seen = seen.clone();
        let awaiting = awaiting.clone();

        std::rc::Rc::new(move || {
            if let Some((command, line)) = awaiting.borrow().as_ref()
                && let Some(offense) = accept::refusal_in(&seen.borrow())
            {
                println!(
                    "voxam: line {line}: {} looks refused: {}",
                    accept::shown(command),
                    offense.trim()
                );
            }

            seen.borrow_mut().clear();
        })
    };

    let source = {
        let commands = script.commands.clone();
        let awaiting = awaiting.clone();
        let judge = judge.clone();
        let mut position = 0;

        Box::new(move || {
            // The response to a command is everything the story
            // printed before the next command is typed; judging
            // happens here, at the moment of the next ask.
            judge();

            let entry = commands.get(position)?;

            position += 1;
            *awaiting.borrow_mut() = Some(entry.clone());

            // The replay's echo: the transcript shows what was
            // entered at each prompt, since no fingers ever typed
            // it there.
            println!("{}", accept::echoed(&entry.0));
            let _ = std::io::stdout().flush();

            Some(entry.0.clone())
        })
    };

    let witness = {
        let seen = seen.clone();

        Box::new(move |text: &str| {
            seen.borrow_mut().push_str(text);
        })
    };

    glulx_session(
        &basename(&script.game.to_string_lossy()),
        story,
        blorb,
        script.seed,
        source,
        Some(witness),
        move || judge(),
    )
}

/// The suffixes a Blorb wears (Blorb: Introduction).
const BLORB_SUFFIXES: [&str; 4] = ["blb", "blorb", "zblorb", "gblorb"];

/// Load a story and whatever resources belong to it: a path with
/// a Blorb suffix must carry a packaged story; any other path
/// loads as a story file, with a like-named Blorb beside the
/// story found on its own.
fn load_story(path: &Path) -> Result<(Story, Option<voxam_core::blorb::Blorb>), String> {
    let suffix = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase());

    if let Some(suffix) = &suffix
        && BLORB_SUFFIXES.contains(&suffix.as_str())
    {
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        let blorb = voxam_core::blorb::Blorb::parse(&bytes).map_err(|error| error.to_string())?;

        let Some(packaged) = blorb.story() else {
            return Err(format!(
                "{} packages no Z-code story to run",
                basename(&path.to_string_lossy())
            ));
        };

        let story = Story::new(packaged.to_vec()).map_err(|error| error.to_string())?;

        return Ok((story, Some(blorb)));
    }

    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let story = Story::new(bytes).map_err(|error| error.to_string())?;

    for sidecar_suffix in BLORB_SUFFIXES {
        let sidecar = path.with_extension(sidecar_suffix);

        if sidecar.exists() {
            let bytes = std::fs::read(&sidecar).map_err(|error| error.to_string())?;
            let blorb =
                voxam_core::blorb::Blorb::parse(&bytes).map_err(|error| error.to_string())?;

            return Ok((story, Some(blorb)));
        }
    }

    Ok((story, None))
}

/// Announce a Blorb at the banner (Blorb: Game Identifier Chunk):
/// the census, and a warning that plays on when the resources
/// name a different story.
fn present_resources(blorb: &Option<voxam_core::blorb::Blorb>, story: &Story) {
    if let Some(blorb) = blorb {
        println!(
            "Resources: {}
",
            blorb.described()
        );

        if !blorb.matches(story) {
            println!(
                "voxam: the resource file names a different story
"
            );
        }
    }
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}
