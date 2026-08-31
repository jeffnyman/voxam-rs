//! The linked host's arrangement, headless: one story served
//! in-process over pipes of our own making.
//!
//! The desktop shell links the interpreter rather than spawning
//! it, and this is that same wiring with the webview taken away --
//! the story opened on a thread of its own, the conversation
//! travelling `voxam-core::pipe` in both directions, stanzas in on
//! stdin and out on stdout. It takes the same arguments the
//! `zglkote` subject takes and must answer identically: the
//! linked sweep drives both through one driver and diffs them, so
//! a pipe that reorders, drops, or re-frames a byte is caught
//! against the transport the reference itself certified.
//!
//! Usage: linked <story> [seed]

use std::io::{BufRead, Write};

use voxam_core::pipe;
use voxam_core::session::{BLORB_SUFFIXES, Opening};
use voxam_core::zmachine::machine::Identity;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = std::path::PathBuf::from(arguments.next().expect("usage: linked <story> [seed]"));
    let seed: Option<u32> = arguments.next().map(|held| held.parse().expect("a seed"));

    let name = path
        .file_name()
        .map(|held| held.to_string_lossy().into_owned())
        .unwrap_or_default();
    let bytes = std::fs::read(&path).expect("a readable story");

    // A bare story's like-named Blorb, as every host finds it.
    let blorbed = path
        .extension()
        .map(|held| held.to_string_lossy().to_lowercase())
        .is_some_and(|held| BLORB_SUFFIXES.contains(&held.as_str()));
    let sidecar = (!blorbed)
        .then(|| {
            BLORB_SUFFIXES
                .iter()
                .map(|held| path.with_extension(held))
                .find(|beside| beside.exists())
                .map(|beside| std::fs::read(beside).expect("a readable sidecar"))
        })
        .flatten();

    let (mut to_session, from_host) = pipe::pipe();
    let (to_host, from_session) = pipe::pipe();

    // The story crosses as bytes and is opened over there: a built
    // session is a thicket of Rc handles that could never cross a
    // thread boundary, which is why the facade takes bytes.
    let serving = std::thread::spawn(move || {
        let mut reader = from_host;
        let mut writer = to_host;

        let played = Opening::of(&name, bytes, sidecar)
            .and_then(|opening| opening.serve(&mut reader, &mut writer, seed, Identity::default()));

        match played {
            Ok(true) => 0,
            Ok(false) => 1,
            Err(error) => {
                // A pre-wire refusal, in voxam's own words, on the
                // stream the shell shows as a fault.
                let _ = writeln!(writer, "voxam: {error}");

                1
            }
        }
    });

    // The host's own pump, line for line, exactly as the shell
    // hands each stanza to the page.
    let pumping = std::thread::spawn(move || {
        let mut lines = from_session;
        let mut line = String::new();

        loop {
            line.clear();

            match lines.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }

            print!("{line}");
            let _ = std::io::stdout().flush();
        }
    });

    // The host's stanzas travel on a thread of their own so the
    // session's ending is never waiting behind a blocked read:
    // a story that quits ends the process at once, exactly as the
    // stdio subject does, rather than sitting on stdin until the
    // player hangs up. Closing stdin still ends a session -- the
    // sender drops with the thread, which is the hangup.
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };

            if writeln!(to_session, "{line}").is_err() {
                break;
            }
        }
    });

    let code = serving.join().unwrap_or(1);

    // The writer dropped with the serving thread, so the pump has
    // an end to reach; joining it is what flushes the last stanza.
    let _ = pumping.join();

    std::process::exit(code);
}
