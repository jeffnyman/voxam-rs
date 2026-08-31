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

use std::io::{BufRead, Write};

use crate::aamachine::story::Story as AaStory;
use crate::blorb::Blorb;
use crate::errors::VoxamError;
use crate::format::{StoryFormat, sniff};
use crate::glulx::glk::resources::Resources;
use crate::glulx::story::Story as GlulxStory;
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
    /// The name decides the container question, as the reference
    /// loaders decide it by suffix; the sidecar is whatever
    /// like-named resource file the caller found beside a bare
    /// story, ignored by the machines that carry their own.
    pub fn of(name: &str, bytes: Vec<u8>, sidecar: Option<Vec<u8>>) -> Result<Self, VoxamError> {
        let sidecar = match sidecar {
            Some(held) => Some(Blorb::parse(&held)?),
            None => None,
        };

        if blorbish(name) {
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
