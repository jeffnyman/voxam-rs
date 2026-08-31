//! One story recognized and served, whichever machine owns it.
//!
//! The CLI's `--glkote` arm, the browser face, and the desktop
//! shell all begin the same way: recognize the story, load the
//! machine that owns it, and speak the GlkOte protocol over a
//! reader and a writer. This module is that beginning, held once.
//! Bytes in, never paths: the filesystem stays with the callers,
//! so the same facade serves the wasm face when its era arrives.
//!
//! The recognition order is the reference CLI's own (`_play`):
//! a Blorb-suffixed name must carry a packaged story -- Glulx
//! answering first by its GLUL Exec resource, then Z-code by
//! ZCOD, then the honest refusal -- while any other name answers
//! by its opening bytes: Glulx magic, the Å-machine's FORM AAVM,
//! and the Z-Machine last, whose loader speaks its own refusal
//! for bytes no machine recognizes.

use std::cell::RefCell;
use std::io::{BufRead, Write};
use std::rc::Rc;

use crate::aamachine::glkote::{GlkOteFrontend as AaFrontend, Verdict as AaVerdict, WireVoice};
use crate::aamachine::machine::{Machine as AaMachine, Wait};
use crate::aamachine::story::Story as AaStory;
use crate::blorb::{Blorb, PNG_ID};
use crate::errors::VoxamError;
use crate::format::{StoryFormat, sniff};
use crate::glkote::json::{Object, Value};
use crate::glulx::glk::glkote::{
    Accepted, GlkOteFrontend as GlulxFrontend, opened as glulx_opened,
};
use crate::glulx::glk::resources::Resources;
use crate::glulx::machine::Machine as GlulxMachine;
use crate::glulx::story::Story as GlulxStory;
use crate::zmachine::glkote::{Session as ZWire, Verdict as ZVerdict, fronted as z_fronted};
use crate::zmachine::machine::Identity;
use crate::zmachine::story::Story as ZStory;

/// The suffixes that promise a Blorb container: a name wearing one
/// must carry a packaged story, and a bare story looks beside
/// itself for a sidecar wearing one -- the reference loaders' own
/// roster.
pub const BLORB_SUFFIXES: [&str; 4] = ["blb", "blorb", "zblorb", "gblorb"];

/// Whether a story's name promises a Blorb container.
fn blorbish(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, suffix)| BLORB_SUFFIXES.contains(&suffix.to_lowercase().as_str()))
}

/// A story opened for serving: the owning machine's loaded story
/// and whatever resources arrived with it. The Å-machine carries
/// its resources inside the story file itself, so it rides alone.
pub enum Opening {
    /// A Z-Machine story and any resources found with it.
    Z { story: ZStory, blorb: Option<Blorb> },
    /// A Glulx story and any resources found with it.
    Glulx {
        story: GlulxStory,
        blorb: Option<Blorb>,
    },
    /// An Å-machine story, resources aboard.
    Aa { story: AaStory },
}

impl Opening {
    /// Recognize and load a story from its own bytes.
    ///
    /// The bytes decide the container question and the name is
    /// only a hint: the reference loaders go by suffix because a
    /// path always has one, but a host that never touched a
    /// filesystem has only a title to offer -- the wasm face
    /// passes the story's display name -- and a Blorb announces
    /// itself in its first bytes regardless (`FORM`/`IFRS`). A
    /// name wearing a Blorb suffix is still believed, so a
    /// container the sniffer somehow missed is opened as the
    /// player asked.
    ///
    /// The sidecar is whatever like-named resource file the
    /// caller found beside a bare story, ignored by the machines
    /// that carry their own.
    pub fn of(name: &str, bytes: Vec<u8>, sidecar: Option<Vec<u8>>) -> Result<Self, VoxamError> {
        let sidecar = match sidecar {
            Some(held) => Some(Blorb::parse(&held)?),
            None => None,
        };

        if blorbish(name) || matches!(sniff(&bytes), Some(StoryFormat::Blorb)) {
            let blorb = Blorb::parse(&bytes)?;

            if let Some(packaged) = blorb.glulx() {
                let story = GlulxStory::new(packaged.to_vec())?;

                return Ok(Self::Glulx {
                    story,
                    blorb: Some(blorb),
                });
            }

            let Some(packaged) = blorb.story() else {
                return Err(VoxamError::Blorb(format!(
                    "{name} packages no Z-code story to run"
                )));
            };

            let story = ZStory::new(packaged.to_vec())?;

            return Ok(Self::Z {
                story,
                blorb: Some(blorb),
            });
        }

        match sniff(&bytes) {
            Some(StoryFormat::Glulx) => Ok(Self::Glulx {
                story: GlulxStory::new(bytes)?,
                blorb: sidecar,
            }),
            Some(StoryFormat::AaMachine) => Ok(Self::Aa {
                story: AaStory::new(&bytes)?,
            }),
            // Everything else belongs to the Z-Machine loader,
            // whose own refusal names bytes no machine recognizes.
            _ => Ok(Self::Z {
                story: ZStory::new(bytes)?,
                blorb: sidecar,
            }),
        }
    }

    /// Serve the opened story over the GlkOte protocol.
    ///
    /// Ok carries each machine's own serving verdict: true is a
    /// session that ended cleanly -- the story quit, or the
    /// display hung up. Err is a failure before the wire opened,
    /// which the caller must speak in its own voice, no protocol
    /// standing yet to carry an error stanza. The identity is the
    /// Z-Machine's §11.1.3 claim; the other machines have no
    /// header to wear it and ignore it.
    pub fn serve(
        self,
        reader: &mut dyn BufRead,
        writer: &mut dyn Write,
        seed: Option<u32>,
        identity: Identity,
    ) -> Result<bool, VoxamError> {
        match self {
            Self::Glulx { story, blorb } => {
                let (mut machine, face) = crate::glulx::glk::glkote::opened(story, blorb, seed)?;

                Ok(crate::glulx::glk::glkote::serve(
                    &mut machine,
                    &face,
                    reader,
                    writer,
                ))
            }
            Self::Aa { story } => Ok(crate::aamachine::glkote::serve(story, reader, writer, seed)),
            Self::Z { story, blorb } => {
                let resources = Resources::new(blorb);
                let frontend = crate::zmachine::glkote::fronted(story.version(), Some(resources))?;

                Ok(crate::zmachine::glkote::serve_claimed(
                    story, frontend, reader, writer, seed, identity,
                ))
            }
        }
    }
}

/// Which machine a sitting is played on, and for the Z-Machine
/// which version -- what a face needs to wear the right mark
/// without knowing anything else about the story.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Played {
    Z { version: u8 },
    Glulx,
    Aa,
}

/// One story's sitting: init to exit, one event at a time.
///
/// The serving loop above blocks on a reader, which suits stdio
/// and a socket and suits a browser not at all -- a page has no
/// blocking read to offer. This is the same conversation turned
/// the other way out: hand it one event, take one stanza back,
/// and the host decides when the next event comes. It is what the
/// browser face has always spoken and what a linked wasm face
/// needs, so the two share it rather than each keeping a copy.
///
/// Every init event -- the page's first breath, and every reload
/// after -- builds a fresh frontend, library, and machine from the
/// parsed story; the resources rebuild from the kept Blorb, their
/// image cache being pure memoization. A sitting whose machine
/// faulted stays faulted, answering the same error until an init
/// starts it over.
pub struct Sitting {
    resources: RefCell<Resources>,
    blorb: Option<Blorb>,
    seed: Option<u32>,
    identity: Identity,
    fault: Option<Object>,
    kind: Kind,
}

#[allow(clippy::large_enum_variant)] // three machines, one seat each
enum Kind {
    Glulx {
        story: GlulxStory,
        live: Option<(GlulxMachine, Rc<RefCell<GlulxFrontend>>)>,
    },
    Z {
        story: ZStory,
        live: Option<ZWire>,
    },
    Aa {
        story: AaStory,
        live: Option<(AaFrontend, AaMachine<WireVoice>)>,
    },
}

/// The protocol's own error stanza, as an answer rather than a
/// line written: a turn always answers something.
fn error_stanza(message: &str) -> Object {
    let mut stanza = Object::new();

    stanza.set("type", "error");
    stanza.set("message", message);

    stanza
}

/// The answer to a cycle that asked for nothing. Every inbound
/// stanza is owed a response, so one that asks for nothing is
/// answered with the pass rather than silence, which would starve
/// a lockstep display (GlkOte: Output: Updating the Display).
fn pass_stanza() -> Object {
    let mut stanza = Object::new();

    stanza.set("type", "pass");

    stanza
}

/// The answer to an event before any init has spoken.
fn unopened() -> Object {
    error_stanza("voxam: the conversation opens with an init event")
}

impl Opening {
    /// Take the opened story as a sitting: one event in, one
    /// stanza out, the host holding the clock.
    ///
    /// The identity is the Z-Machine's §11.1.3 claim; the other
    /// machines have no header to wear it and ignore it.
    pub fn sitting(self, seed: Option<u32>, identity: Identity) -> Sitting {
        let (blorb, kind) = match self {
            Self::Glulx { story, blorb } => (blorb, Kind::Glulx { story, live: None }),
            Self::Z { story, blorb } => (blorb, Kind::Z { story, live: None }),
            Self::Aa { story } => (None, Kind::Aa { story, live: None }),
        };

        Sitting {
            resources: RefCell::new(Resources::new(blorb.clone())),
            blorb,
            seed,
            identity,
            fault: None,
            kind,
        }
    }
}

impl Sitting {
    /// Which machine holds this story, for a face that wants to
    /// wear its mark.
    pub fn played(&self) -> Played {
        match &self.kind {
            Kind::Glulx { .. } => Played::Glulx,
            Kind::Aa { .. } => Played::Aa,
            Kind::Z { story, .. } => Played::Z {
                version: story.version(),
            },
        }
    }

    /// One event in, one stanza out: the burst model's turn.
    ///
    /// An init rebuilds the sitting; anything else lands on the
    /// machine standing suspended. A fault answers as the
    /// protocol's own error stanza and keeps answering so until
    /// the next init.
    pub fn answer(&mut self, stanza: &Object) -> Object {
        if stanza.get("type").and_then(Value::as_str) == Some("init") {
            self.fault = None;

            return match self.reborn(stanza) {
                Ok(update) => update,
                Err(error) => self.faulted(&error),
            };
        }

        if let Some(fault) = &self.fault {
            return fault.clone();
        }

        match self.delivered(stanza) {
            Ok(update) => update,
            Err(error) => self.faulted(&error),
        }
    }

    /// One Blorb picture by number, with the type it travels as.
    pub fn picture(&self, number: u32) -> Option<(&'static str, Vec<u8>)> {
        let mut resources = self.resources.borrow_mut();
        let found = resources.image(number)?;
        let kind = if found.kind == PNG_ID {
            "image/png"
        } else {
            "image/jpeg"
        };

        Some((kind, found.data.clone()))
    }

    fn faulted(&mut self, error: &VoxamError) -> Object {
        let stanza = error_stanza(&format!("voxam: {error}"));

        self.fault = Some(stanza.clone());

        stanza
    }

    /// Start the story over, fresh objects from the kept story.
    fn reborn(&mut self, stanza: &Object) -> Result<Object, VoxamError> {
        match &mut self.kind {
            Kind::Glulx { story, live } => {
                let (machine, face) = glulx_opened(story.clone(), self.blorb.clone(), self.seed)?;

                face.borrow_mut().begin(stanza)?;
                *live = Some((machine, face));

                let Some((machine, face)) = live else {
                    unreachable!("just installed");
                };

                turned_glulx(machine, face)
            }
            Kind::Z { story, live } => {
                let mut frontend =
                    z_fronted(story.version(), Some(Resources::new(self.blorb.clone())))?;

                frontend.begin(stanza)?;

                let mut session =
                    ZWire::open_claimed(story.clone(), frontend, self.seed, self.identity)?;
                let update = turned_z(&mut session)?;

                *live = Some(session);

                Ok(update)
            }
            Kind::Aa { story, live } => {
                let mut frontend = AaFrontend::new(story);
                let mut voice = WireVoice::new(story)?;

                frontend.begin(&mut voice, stanza)?;

                let mut machine = AaMachine::new(story.clone(), voice, self.seed)?;

                frontend.waiting = Some(machine.run(None)?);

                let exit = frontend.waiting == Some(Wait::Quit);
                let update = frontend.render(&mut machine.voice, exit)?;

                *live = Some((frontend, machine));

                Ok(update)
            }
        }
    }

    /// Deliver one event to the suspended machine and run on.
    fn delivered(&mut self, stanza: &Object) -> Result<Object, VoxamError> {
        match &mut self.kind {
            Kind::Glulx { live, .. } => {
                let Some((machine, face)) = live else {
                    return Ok(unopened());
                };

                let verdict = {
                    let (glk, memory) = attached(machine)?;

                    face.borrow_mut().accept(glk, memory, stanza)?
                };

                match verdict {
                    Accepted::Event(event) => {
                        machine.deliver_event(event)?;

                        turned_glulx(machine, face)
                    }
                    Accepted::File(name) => {
                        // The stanza itself completed the wait: a
                        // file answer stores through the parked
                        // call.
                        machine.deliver_file(name.as_deref())?;

                        turned_glulx(machine, face)
                    }
                    Accepted::Nothing => {
                        let cleared = machine.glk_mut().is_none_or(|glk| glk.waiting.is_none());

                        if cleared {
                            return turned_glulx(machine, face);
                        }

                        Ok(pass_stanza())
                    }
                }
            }
            Kind::Z { live, .. } => {
                let Some(session) = live else {
                    return Ok(unopened());
                };

                match session.accept(stanza)? {
                    ZVerdict::Advance => turned_z(session),
                    ZVerdict::Stand => session.render(false),
                    ZVerdict::Pass => Ok(pass_stanza()),
                }
            }
            Kind::Aa { live, .. } => {
                let Some((frontend, machine)) = live else {
                    return Ok(unopened());
                };

                match frontend.accept(machine, stanza)? {
                    AaVerdict::Advance => {
                        let exit = frontend.waiting == Some(Wait::Quit);

                        frontend.render(&mut machine.voice, exit)
                    }
                    AaVerdict::Stand => frontend.render(&mut machine.voice, false),
                    AaVerdict::Pass => Ok(pass_stanza()),
                }
            }
        }
    }
}

/// The machine's library and memory, both in hand.
fn attached(
    machine: &mut GlulxMachine,
) -> Result<
    (
        &mut crate::glulx::glk::api::Glk,
        &mut crate::glulx::memory::Memory,
    ),
    VoxamError,
> {
    machine
        .glk_and_memory_mut()
        .ok_or_else(|| VoxamError::GlkOte("the display is not attached to a library".into()))
}

/// Run the Glulx machine to its next wait and render the update.
fn turned_glulx(
    machine: &mut GlulxMachine,
    face: &Rc<RefCell<GlulxFrontend>>,
) -> Result<Object, VoxamError> {
    machine.run(None)?;

    let running = machine.running();
    let (glk, memory) = attached(machine)?;

    face.borrow_mut().render(glk, memory, !running)
}

/// Run the Z machine to its next wait and render the update.
fn turned_z(session: &mut ZWire) -> Result<Object, VoxamError> {
    session.machine().run()?;

    let running = session.machine().running();

    session.render(!running)
}
