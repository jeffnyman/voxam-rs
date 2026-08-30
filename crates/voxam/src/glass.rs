//! The glass session: a Z-Machine story on the painted terminal.
//!
//! The reference's blocking machine reads at its frontend
//! mid-instruction; this machine always suspends (the suspension
//! departure), so the serving loop here is the blocking path
//! turned inside out: run until a read stands waiting, take the
//! keystrokes or the line at the glass, and deliver. §15's timed
//! reads tick their interrupts on the wall clock between
//! deliveries -- the reference's _ticked_line and _timed_keystroke
//! drills, spoken through deliver_tick -- with the §15 redisplay
//! courtesy when an interrupt printed and the input erased when
//! one terminates the read.

use std::cell::RefCell;
use std::io::{IsTerminal, Stdout, Write};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use voxam_core::blorb::{Blorb, PNG_ID};
use voxam_core::errors::VoxamError;
use voxam_core::glulx::glk::api::Glk;
use voxam_core::glulx::glk::resources::Resources;
use voxam_core::glulx::machine::Machine as GlulxMachine;
use voxam_core::glulx::story::Story as GlulxStory;
use voxam_core::png::{Picture, decode};
use voxam_core::zmachine::machine::{Identity, Machine, RunState, Waiting, Wants};
use voxam_core::zmachine::story::Story;
use voxam_glass::glk::{EventCodes, GlkGlass};
use voxam_glass::keys::EventKeys;
use voxam_glass::painter::{Glass, PaintedHalf, ScreenFrontend};
use voxam_glass::ratatui::Terminal;
use voxam_glass::ratatui::backend::CrosstermBackend;
use voxam_glass::ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use voxam_glass::ratatui::layout::Position;

/// The session's shared handle on the painted face. Generic over
/// the backend so the drill battery can serve a whole story onto
/// a TestBackend grid.
type Face<B> = Rc<RefCell<ScreenFrontend<B>>>;

/// Whether the glass can stand at all: a real terminal on stdout.
/// A pipe or a captured stream keeps the plain stream, which is
/// always there.
pub fn glassable() -> bool {
    std::io::stdout().is_terminal()
}

fn glass_error(message: String) -> VoxamError {
    VoxamError::Glass(message)
}

/// Raw mode, held for the session and released on every road out
/// -- the reference's per-read cbreak, worn for the whole session
/// the way a ratatui glass keeps its terminal.
struct Raw;

impl Raw {
    fn held() -> Result<Self, VoxamError> {
        enable_raw_mode()
            .map_err(|error| glass_error(format!("the terminal refused raw mode: {error}")))?;

        Ok(Self)
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// The Blorb's cover picture, when there is one Voxam can draw.
///
/// Cover art is a courtesy, never a gate: a cover Voxam cannot
/// draw -- Zork 1's JPEG, an exotic PNG -- earns a note and the
/// story plays on. The notes print before the glass takes the
/// screen.
fn covered(blorb: Option<&Blorb>) -> Option<Picture> {
    let cover = blorb?.cover()?;

    if cover.chunk.chunk_id != PNG_ID {
        let kind = String::from_utf8_lossy(&cover.chunk.chunk_id)
            .trim()
            .to_string();

        println!("voxam: the cover picture is {kind}, which Voxam cannot draw\n");

        return None;
    }

    match decode(&cover.chunk.payload, None) {
        Ok(picture) => Some(picture),
        Err(error) => {
            println!("voxam: the cover picture cannot be drawn: {error}\n");

            None
        }
    }
}

/// Play one loaded Z-Machine story at the painted terminal.
///
/// Infocom's own interpreters opened this way: the cover art when
/// the Blorb brought one, a keypress, and the story on a clean
/// glass. On the way out the cursor retires to the bottom row, so
/// the shell's next prompt lands under the story rather than
/// somewhere mid-screen where the cursor was parked.
pub fn session(
    story: Story,
    blorb: Option<&Blorb>,
    seed: Option<u32>,
    path: &Path,
) -> Result<(), VoxamError> {
    let version = story.header().version();
    let cover = covered(blorb);
    let raw = Raw::held()?;
    let terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))
        .map_err(|error| glass_error(format!("the glass cannot stand: {error}")))?;
    let glass = Rc::new(RefCell::new(Glass::new(
        terminal,
        Box::new(EventKeys),
        Box::new(|| {
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(b"\x07");
            let _ = stdout.flush();
        }),
    )));
    let face: Face<CrosstermBackend<Stdout>> =
        Rc::new(RefCell::new(ScreenFrontend::new(version, glass)));
    let saves = voxam_core::saves::FileSaveSlot {
        path: path.with_extension("sav"),
    };
    let mut machine = Machine::new(
        story,
        Box::new(PaintedHalf(Rc::clone(&face))),
        seed,
        Identity::default(),
        Some(Box::new(saves)),
    )?;

    if let Some(picture) = cover {
        face.borrow_mut().show_frontispiece(&picture);
    }

    // The story deserves a clean glass: anything the shell left on
    // screen would otherwise show through every row the game has
    // not yet painted.
    face.borrow_mut().clear();

    let outcome = served(&mut machine, &face);

    // The shell's next prompt belongs under the story.
    retire(&face);
    drop(raw);
    println!();

    outcome
}

/// Run the machine to its end, serving every read at the glass.
fn served<B: voxam_glass::ratatui::backend::Backend + 'static>(
    machine: &mut Machine,
    face: &Face<B>,
) -> Result<(), VoxamError> {
    loop {
        let state = machine.run()?;

        surfaced(face)?;

        match state {
            RunState::Halted => return Ok(()),
            RunState::Waiting => {
                attended(machine, face)?;
                surfaced(face)?;
            }
        }
    }
}

/// Surface the first fault either half noted: a Frontend call
/// cannot return what its trait did not promise, so the model's
/// refusals and the terminal's mishaps wait here for the loop.
fn surfaced<B: voxam_glass::ratatui::backend::Backend>(face: &Face<B>) -> Result<(), VoxamError>
where
    B::Error: std::fmt::Display,
{
    if let Some(fault) = face.borrow_mut().fault.take() {
        return Err(fault);
    }

    let glass = Rc::clone(&face.borrow().glass);
    let held = glass.borrow_mut().fault.take();

    if let Some(fault) = held {
        return Err(glass_error(format!("the glass failed: {fault}")));
    }

    Ok(())
}

/// Serve the read that stands waiting, §15's clocks included.
fn attended<B: voxam_glass::ratatui::backend::Backend + 'static>(
    machine: &mut Machine,
    face: &Face<B>,
) -> Result<(), VoxamError> {
    let (wants, time, routine) = match machine.waiting() {
        Some(Waiting::Read(reading)) => (reading.wants, reading.time, reading.routine),
        _ => {
            return Err(glass_error(
                "the glass face keeps its saves on the slot beside the story, \
                 so only a read can stand waiting"
                    .to_string(),
            ));
        }
    };
    // §15 speaks in tenths of a second, on the wall clock.
    let interval = Duration::from_millis(u64::from(time) * 100);
    let timed = time != 0 && routine != 0;

    match wants {
        // The live half of a §15 timed line read: each expiry
        // fires the interrupt; a terminating one erases the input
        // (the machine has already stored 0 and moved on), and one
        // that printed earns the input line redisplayed -- Jigsaw
        // prints each chapter's epigraph from exactly such a
        // routine.
        Wants::Line if timed => loop {
            let line = face.borrow_mut().read_line_until(interval);

            match line {
                Some(line) => {
                    machine.deliver_line(&line, 0)?;

                    return Ok(());
                }
                None => {
                    face.borrow_mut().begin_input();

                    let before = face.borrow().prints;

                    machine.deliver_tick()?;

                    if machine.waiting().is_none() {
                        face.borrow_mut().abandon_input();

                        return Ok(());
                    }

                    if face.borrow().prints != before {
                        face.borrow_mut().resume_input();
                    }
                }
            }
        },
        Wants::Line => {
            let line = face.borrow_mut().read_line();

            machine.deliver_line(&line, 0)
        }
        // A timed keystroke ticks the same clock; a key ZSCII
        // cannot spell is a key the story cannot hear (§3.8), and
        // the wait stands for the next one.
        Wants::Key if timed => loop {
            let key = face.borrow_mut().read_key(Some(interval));

            match key {
                Some(key) => {
                    if machine.deliver_key(key)? {
                        return Ok(());
                    }
                }
                None => {
                    machine.deliver_tick()?;

                    if machine.waiting().is_none() {
                        return Ok(());
                    }
                }
            }
        },
        Wants::Key => loop {
            let key = face
                .borrow_mut()
                .read_key(None)
                .expect("an untimed key read waits for a real key");

            if machine.deliver_key(key)? {
                return Ok(());
            }
        },
    }
}

/// Play one loaded Glulx story at the painted terminal.
///
/// The Glk library owns the glass whole -- its frontend box, the
/// blocking arrangement glkterm uses -- so the session's shares
/// are the bookends: the raw-mode dance, a clean glass before the
/// story, and the cursor retired under it after. A story that
/// ends with quit rather than glk_exit never asked for a last
/// flush; whatever its windows still hold is shown on the way
/// out.
pub fn glulx_session(
    name: &str,
    story: GlulxStory,
    blorb: Option<Blorb>,
    seed: Option<u32>,
) -> Result<(), VoxamError> {
    // The checksum verdict is printed but does not gate the run:
    // the verify opcode exists so a story can judge itself.
    let verdict = if story.verify() {
        "checksum verified"
    } else {
        "CHECKSUM MISMATCH"
    };

    println!("Running {name}: Glulx {}, {verdict}\n", story.version());

    let raw = Raw::held()?;
    let terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))
        .map_err(|error| glass_error(format!("the glass cannot stand: {error}")))?;
    let mut painted = GlkGlass::new(terminal, Box::new(EventCodes));

    // The story deserves a clean glass: anything the shell left on
    // screen would otherwise show through every row the game has
    // not yet painted.
    painted.clear();

    let mut library = Glk::new(Box::new(painted));

    library.resources = Resources::new(blorb);

    let session = GlulxMachine::new(story, seed).and_then(|mut machine| {
        machine.install_glk(library);
        machine.run(None)?;

        if let Some(glk) = machine.glk_mut() {
            let root = glk.root;

            glk.frontend.flush(&mut glk.windows, root);
        }

        Ok(())
    });

    // The shell's next prompt belongs under the story; the glass
    // is boxed away inside the library, so the parking goes
    // through the terminal's own door.
    let (_, rows) = voxam_glass::ratatui::crossterm::terminal::size().unwrap_or((80, 24));
    let _ = voxam_glass::ratatui::crossterm::execute!(
        std::io::stdout(),
        voxam_glass::ratatui::crossterm::cursor::MoveTo(0, rows.saturating_sub(1)),
        voxam_glass::ratatui::crossterm::cursor::Show
    );

    drop(raw);
    println!();

    session
}

/// Park the cursor on the bottom row on the way out.
fn retire<B: voxam_glass::ratatui::backend::Backend>(face: &Face<B>) {
    let face = face.borrow();
    let mut glass = face.glass.borrow_mut();
    let bottom = glass.lines.saturating_sub(1) as u16;
    let terminal = glass.terminal_mut();
    let _ = terminal.set_cursor_position(Position::new(0, bottom));
    let _ = terminal.show_cursor();
}

#[cfg(test)]
#[path = "glass_tests.rs"]
mod tests;
