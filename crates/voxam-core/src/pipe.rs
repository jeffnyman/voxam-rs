//! A byte pipe between two threads, for hosts that link the
//! interpreter instead of spawning it.
//!
//! The session facade serves a story over a `BufRead` and a
//! `Write` -- stdin and stdout for the CLI, a socket for the
//! browser face. A host that links the interpreter in-process has
//! no pipe from the operating system to hand it, so this module
//! spells one: a shared byte queue with a reader that blocks until
//! bytes arrive and a writer that wakes it, both ends hanging up
//! when dropped.
//!
//! It lives here, beside the facade rather than in any one face,
//! for the reason the facade itself does: every host that links
//! the interpreter needs exactly this, and here the standing gate
//! certifies it. Nothing in it is a display or filesystem opinion
//! -- it is the standard library's own threading, spelled for the
//! shape the serving loops already take.
//!
//! The hangups are what make a linked session end the way a
//! spawned one did: dropping the writer is the closing stdin that
//! ends the machine's conversation, and dropping the reader is the
//! dead child whose next write fails.

use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};
use std::sync::{Arc, Condvar, Mutex};

/// The bytes in flight, and whether either end has hung up.
#[derive(Default)]
struct Held {
    bytes: VecDeque<u8>,
    /// The writer is gone: what remains reads, then ends.
    written_out: bool,
    /// The reader is gone: further writes have nowhere to land.
    read_out: bool,
}

/// The queue itself, and the signal that stirs a waiting reader.
#[derive(Default)]
struct Shared {
    held: Mutex<Held>,
    stirred: Condvar,
}

/// The writing end of a pipe. Dropping it ends the reader's
/// stream, the way a closing stdin ends a child's.
pub struct Sender(Arc<Shared>);

/// The reading end of a pipe, buffered: `fill_buf` blocks until
/// bytes arrive or the writer hangs up.
pub struct Receiver {
    shared: Arc<Shared>,
    /// Bytes taken from the queue and not yet consumed -- a
    /// `BufRead` must hand back a borrowed slice, which a slice of
    /// the shared queue could never be.
    taken: Vec<u8>,
    at: usize,
}

/// Open a pipe: the writing end and the reading end.
pub fn pipe() -> (Sender, Receiver) {
    let shared = Arc::new(Shared::default());

    (
        Sender(Arc::clone(&shared)),
        Receiver {
            shared,
            taken: Vec::new(),
            at: 0,
        },
    )
}

impl Write for Sender {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut held = self.0.held.lock().expect("the pipe's lock");

        if held.read_out {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "the pipe's reader has hung up",
            ));
        }

        held.bytes.extend(bytes);
        drop(held);

        self.0.stirred.notify_all();

        Ok(bytes.len())
    }

    /// The bytes are in the queue the moment they are written;
    /// there is nothing held back to flush.
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        if let Ok(mut held) = self.0.held.lock() {
            held.written_out = true;
        }

        self.0.stirred.notify_all();
    }
}

impl BufRead for Receiver {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.at < self.taken.len() {
            return Ok(&self.taken[self.at..]);
        }

        let mut held = self.shared.held.lock().expect("the pipe's lock");

        // A writer still standing but with nothing to say is worth
        // waiting for; one that has hung up leaves only what it
        // already wrote, and then the end of the stream.
        while held.bytes.is_empty() && !held.written_out {
            held = self.shared.stirred.wait(held).expect("the pipe's lock");
        }

        self.taken = held.bytes.drain(..).collect();
        self.at = 0;

        Ok(&self.taken)
    }

    fn consume(&mut self, spent: usize) {
        self.at = (self.at + spent).min(self.taken.len());
    }
}

impl Read for Receiver {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        let held = self.fill_buf()?;
        let taken = held.len().min(into.len());

        into[..taken].copy_from_slice(&held[..taken]);
        self.consume(taken);

        Ok(taken)
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        if let Ok(mut held) = self.shared.held.lock() {
            held.read_out = true;
        }

        self.shared.stirred.notify_all();
    }
}
