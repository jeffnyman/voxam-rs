//! The `voxam` command. Point it at a Z-Machine or Glulx story
//! and it plays on the plain stream; an Å-machine story plays at
//! the terminal, the third machine's own face; ask for `--header`
//! and it reads the story's manifest (§11.1); any other format is
//! named by its magic.

mod accept;
mod glance;
mod glass;
mod web;

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
use voxam_core::session::{BLORB_SUFFIXES, Opening};
use voxam_core::zmachine::machine::{Identity, Machine, RunState};
use voxam_core::zmachine::story::Story;

/// The exit codes the Python CLI speaks: 0 for a served request,
/// 2 for one that could not be used.
const EXIT_OK: u8 = 0;
const EXIT_UNUSABLE: u8 = 2;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    if arguments
        .iter()
        .any(|held| held == "--glkote" || held == "--web")
    {
        return wired(&arguments);
    }

    match arguments.as_slice() {
        [flag, story] if flag == "--header" => header_report(story),
        [flag, story] if flag == "--babel" => babel_report(story),
        [flag, script] if flag == "--accept" => accepted_session(script),
        _ => plays(&arguments),
    }
}

/// Parse a play request: the story, its seed, and whether the
/// plain stream was asked for by name. The painted terminal is
/// the default face at a real terminal; `--plain` keeps the
/// stream, which is always there.
fn plays(arguments: &[String]) -> ExitCode {
    let mut plain = false;
    let mut pixels = false;
    let mut seed: Option<u32> = None;
    let mut story: Option<String> = None;
    let mut identity = Identity::default();
    let mut walker = arguments.iter();

    while let Some(held) = walker.next() {
        match held.as_str() {
            "--plain" => plain = true,
            "--pixels" => pixels = true,
            "--interpreter" => {
                let Some(name) = walker.next() else {
                    eprintln!("voxam: --interpreter takes a platform name or number");
                    return ExitCode::from(EXIT_UNUSABLE);
                };

                match interpreter_number(name) {
                    Some(number) => identity.interpreter = Some(number),
                    None => {
                        eprintln!(
                            "voxam: unknown interpreter {name:?}; use a number or one                              of the §11.1.3 platform names"
                        );
                        return ExitCode::from(EXIT_UNUSABLE);
                    }
                }
            }
            "--tandy" => identity.tandy = true,
            "--seed" => {
                let Some(value) = walker.next().and_then(|told| told.parse().ok()) else {
                    eprintln!("voxam: --seed takes a number");
                    return ExitCode::from(EXIT_UNUSABLE);
                };

                seed = Some(value);
            }
            told if !told.starts_with("--") && story.is_none() => {
                story = Some(told.to_string());
            }
            _ => return usage(),
        }
    }

    let Some(story) = story else {
        return usage();
    };

    if pixels && plain {
        eprintln!("voxam: --pixels asks for the glass, which --plain declines; pick one");
        return ExitCode::from(EXIT_UNUSABLE);
    }

    play(&story, seed, !plain, pixels, identity)
}

/// Print the flag surface and refuse.
fn usage() -> ExitCode {
    eprintln!(
        "usage: voxam [--header] [--babel] [--accept script] [--seed N] \
         [--plain] [--pixels] [--interpreter NAME] [--tandy] [--glkote] \
         [--web [--port N]] <story-file>"
    );
    ExitCode::from(EXIT_UNUSABLE)
}

/// The default port `--web` listens on.
const WEB_PORT: u16 = 8080;

/// Serve one story over the wire: `--glkote` on stdio, or `--web`
/// over HTTP with the vendored GlkOte display.
fn wired(arguments: &[String]) -> ExitCode {
    let mut glkote = false;
    let mut web = false;
    let mut port: u16 = WEB_PORT;
    let mut port_given = false;
    let mut seed: Option<u32> = None;
    let mut story: Option<String> = None;
    let mut identity = Identity::default();
    let mut walker = arguments.iter();

    while let Some(held) = walker.next() {
        match held.as_str() {
            "--glkote" => glkote = true,
            "--web" => web = true,
            "--interpreter" => {
                let Some(name) = walker.next() else {
                    eprintln!("voxam: --interpreter takes a platform name or number");
                    return ExitCode::from(EXIT_UNUSABLE);
                };

                match interpreter_number(name) {
                    Some(number) => identity.interpreter = Some(number),
                    None => {
                        eprintln!(
                            "voxam: unknown interpreter {name:?}; use a number or one \
                             of the §11.1.3 platform names"
                        );
                        return ExitCode::from(EXIT_UNUSABLE);
                    }
                }
            }
            "--tandy" => identity.tandy = true,
            "--port" => {
                port_given = true;

                let Some(value) = walker.next().and_then(|told| told.parse().ok()) else {
                    eprintln!("voxam: --port takes a number");
                    return ExitCode::from(EXIT_UNUSABLE);
                };

                port = value;
            }
            "--seed" => {
                let Some(value) = walker.next().and_then(|told| told.parse().ok()) else {
                    eprintln!("voxam: --seed takes a number");
                    return ExitCode::from(EXIT_UNUSABLE);
                };

                seed = Some(value);
            }
            told if !told.starts_with("--") && story.is_none() => {
                story = Some(told.to_string());
            }
            told => {
                eprintln!("voxam: {told} does not belong to --glkote or --web");
                return ExitCode::from(EXIT_UNUSABLE);
            }
        }
    }

    if glkote && web {
        eprintln!("voxam: --glkote and --web are two different faces; choose one");
        return ExitCode::from(EXIT_UNUSABLE);
    }

    if port_given && !web {
        eprintln!("voxam: --port belongs to --web");
        return ExitCode::from(EXIT_UNUSABLE);
    }

    let Some(story) = story else {
        eprintln!("usage: voxam [--seed N] --glkote <story-file>, or --web [--port N]");
        return ExitCode::from(EXIT_UNUSABLE);
    };

    if glkote {
        serve_glkote(&story, seed, identity)
    } else {
        serve_webbed(&story, seed, port)
    }
}

/// The §11.1.3 interpreter numbers, by the names Infocom used --
/// or any bare number a player claims directly.
fn interpreter_number(name: &str) -> Option<u8> {
    match name.to_lowercase().as_str() {
        "dec-20" => Some(1),
        "apple-iie" => Some(2),
        "macintosh" => Some(3),
        "amiga" => Some(4),
        "atari-st" => Some(5),
        "ibm-pc" => Some(6),
        "commodore-128" => Some(7),
        "commodore-64" => Some(8),
        "apple-iic" => Some(9),
        "apple-iigs" => Some(10),
        "tandy-color" => Some(11),
        told => told.parse().ok(),
    }
}

/// The wire's pieces of one story path: its name, its bytes, and
/// any like-named sidecar Blorb's bytes.
type WirePieces = (String, Vec<u8>, Option<Vec<u8>>);

/// Gather a story's wire pieces -- the filesystem's whole share of
/// the session facade's work.
fn wire_pieces(path: &Path) -> Result<WirePieces, String> {
    let name = basename(&path.to_string_lossy());
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;

    // A Blorb-suffixed path is its own container; only a bare
    // story looks beside itself.
    let suffix = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase());

    if let Some(suffix) = &suffix
        && BLORB_SUFFIXES.contains(&suffix.as_str())
    {
        return Ok((name, bytes, None));
    }

    for sidecar_suffix in BLORB_SUFFIXES {
        let sidecar = path.with_extension(sidecar_suffix);

        if sidecar.exists() {
            let held = std::fs::read(&sidecar).map_err(|error| error.to_string())?;

            return Ok((name, bytes, Some(held)));
        }
    }

    Ok((name, bytes, None))
}

/// Speak the GlkOte protocol for one story on stdin and stdout.
///
/// Nothing else may print there -- no banner, no verdict -- so the
/// display's own error stanza is the only voice a failure has, and
/// pre-wire refusals speak as bare voxam: text, the shell
/// contract's own word for them. The recognition and the serving
/// both live in the core's session facade; this arm owns only the
/// filesystem and the exit code.
fn serve_glkote(path: &str, seed: Option<u32>, identity: Identity) -> ExitCode {
    let opening = wire_pieces(Path::new(path)).and_then(|(name, bytes, sidecar)| {
        Opening::of(&name, bytes, sidecar).map_err(|error| error.to_string())
    });
    let opening = match opening {
        Ok(opening) => opening,
        Err(error) => {
            println!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    let mut reader = std::io::BufReader::new(std::io::stdin());
    let mut writer = std::io::stdout();

    match opening.serve(&mut reader, &mut writer, seed, identity) {
        Ok(clean) => ExitCode::from(if clean { EXIT_OK } else { EXIT_UNUSABLE }),
        Err(error) => {
            println!("voxam: {error}");
            ExitCode::from(EXIT_UNUSABLE)
        }
    }
}

/// Serve one story to the browser, under its own name.
fn serve_webbed(path: &str, seed: Option<u32>, port: u16) -> ExitCode {
    let opening = wire_pieces(Path::new(path)).and_then(|(name, bytes, sidecar)| {
        Opening::of(&name, bytes, sidecar).map_err(|error| error.to_string())
    });
    let opening = match opening {
        Ok(opening) => opening,
        Err(error) => {
            eprintln!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    let (session, caption) = match opening {
        Opening::Glulx { story, blorb } => {
            let caption = titled(&blorb);

            (web::Session::glulx(story, blorb, seed), caption)
        }
        Opening::Aa { story } => (web::Session::aamachine(story, None, seed), None),
        Opening::Z { story, blorb } => {
            let caption = titled(&blorb);

            (web::Session::z(story, blorb, seed), caption)
        }
    };
    let mut face = web::Face::new(session, caption.as_deref());
    let listener = match web::webbed(port) {
        Ok(listener) => listener,
        Err(error) => {
            // The port would not bind, most likely: say so plainly.
            println!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    web::serve_web(&mut face, &listener);

    ExitCode::from(EXIT_OK)
}

/// Serve `--babel`: the story's identity and its bibliography.
///
/// The treaty speaks all three machines: a blorb's iFiction
/// record answers first, then the packaged or loose story's own
/// bytes (Babel: The IFID for a blorbed story file) -- and the
/// record's bibliography rides along when it has any. A
/// metadata-only blorb still refuses: a blorb with no story "is
/// not itself a work of IF".
fn babel_report(path: &str) -> ExitCode {
    let path = Path::new(path);
    let name = path
        .file_name()
        .map(|held| held.to_string_lossy().into_owned())
        .unwrap_or_default();
    let suffix = path
        .extension()
        .map(|held| held.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let blorbed = BLORB_SUFFIXES.contains(&suffix.as_str());

    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            println!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
    };

    let (data, record) = if blorbed {
        let blorb = match voxam_core::blorb::Blorb::parse(&bytes) {
            Ok(blorb) => blorb,
            Err(error) => {
                println!("voxam: {error}");
                return ExitCode::from(EXIT_UNUSABLE);
            }
        };
        let record = blorb.ifiction.as_deref().and_then(|held| {
            let parsed = voxam_core::babel::ifiction(held);

            if parsed.is_none() {
                println!("voxam: the iFiction record cannot be read; the story answers instead");
            }

            parsed
        });
        let packaged = blorb
            .glulx()
            .map(<[u8]>::to_vec)
            .or_else(|| blorb.story().map(<[u8]>::to_vec));

        (packaged, record)
    } else {
        (Some(bytes), None)
    };

    let Some(data) = data else {
        println!(
            "voxam: {name} packages no story, and a blorb without one is not itself \
             a work of IF"
        );
        return ExitCode::from(EXIT_UNUSABLE);
    };

    let identity = record
        .as_ref()
        .and_then(|held| held.ifid.clone())
        .or_else(|| voxam_core::babel::ifid(&data));

    let Some(identity) = identity else {
        println!("voxam: {name} is neither Z-code nor Glulx");
        return ExitCode::from(EXIT_UNUSABLE);
    };

    println!("{name}\n");
    println!("IFID: {identity}");

    let named = record
        .as_ref()
        .and_then(|held| held.title.clone())
        .or_else(|| voxam_core::infocom::title(&identity).map(str::to_string));

    if let Some(named) = named {
        println!("Title: {named}");
    }

    if let Some(author) = record.as_ref().and_then(|held| held.author.as_ref()) {
        println!("Author: {author}");
    }

    if let Some(headline) = record.as_ref().and_then(|held| held.headline.as_ref()) {
        println!("Headline: {headline}");
    }

    ExitCode::from(EXIT_OK)
}

/// The caption a session deserves, when the game is known.
///
/// A Blorb's iFiction record names its story, and the story plays
/// under that name: the treaty's first interpreter guideline
/// (Babel: Guidelines for interpreters and browsers). The
/// reference also consults the Infocom catalog by IFID, which
/// waits on the Babel identities milestone; anything unknown is
/// quietly no caption -- a title bar is a courtesy, never a gate.
fn titled(blorb: &Option<voxam_core::blorb::Blorb>) -> Option<String> {
    let record = blorb
        .as_ref()
        .and_then(|held| held.ifiction.as_deref())
        .and_then(voxam_core::babel::ifiction)?;

    record.title.map(|title| format!("{title} — Voxam"))
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
fn aamachine_session(path: &str, seed: Option<u32>, screen: bool) -> ExitCode {
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
    let dressed = screen && std::io::stdout().is_terminal();
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

/// Play a story: the painted terminal by default at a real
/// terminal, the plain stream behind `--plain` or a pipe -- text
/// flows out, lines flow in, and end of input ends the session.
fn play(path: &str, seed: Option<u32>, screen: bool, pixels: bool, identity: Identity) -> ExitCode {
    println!("\nVoxam Interpreter for Z-Machine and Glulx Stories\n");

    if aamachine_story(Path::new(path)) {
        return aamachine_session(path, seed, screen);
    }

    match load_glulx(Path::new(path)) {
        Err(error) => {
            println!("voxam: {error}");
            return ExitCode::from(EXIT_UNUSABLE);
        }
        Ok(Some((story, blorb))) => {
            // The painted terminal is the default face at a real
            // terminal; `--plain` or a pipe keeps the stream.
            if screen && glass::glassable() {
                return match glass::glulx_session(&basename(path), story, blorb, seed) {
                    Ok(()) => ExitCode::from(EXIT_OK),
                    Err(error) => {
                        println!("voxam: {error}");
                        ExitCode::from(EXIT_UNUSABLE)
                    }
                };
            }

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

    // The painted terminal is the default face at a real terminal
    // -- the reference's own choice -- and `--plain` or a pipe
    // keeps the stream, which is always there.
    if screen && glass::glassable() {
        return match glass::session(
            loaded,
            blorb.as_ref(),
            seed,
            Path::new(path),
            pixels,
            identity,
        ) {
            Ok(()) => ExitCode::from(EXIT_OK),
            Err(error) => {
                println!("voxam: {error}");
                ExitCode::from(EXIT_UNUSABLE)
            }
        };
    }

    let session = Ok(loaded)
        .and_then(|story: Story| {
            let saves = voxam_core::saves::FileSaveSlot {
                path: Path::new(path).with_extension("sav"),
            };

            Machine::new(
                story,
                Box::new(plain()),
                seed,
                identity,
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
