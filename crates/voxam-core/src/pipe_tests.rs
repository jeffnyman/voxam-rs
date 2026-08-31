//! The linked host's pipe: bytes crossing threads, and the two
//! hangups that end a session the way a closed process did.

use std::io::{BufRead, Read, Write};

use crate::pipe::pipe;

#[test]
fn carries_bytes_from_one_end_to_the_other() {
    let (mut sender, mut receiver) = pipe();

    sender.write_all(b"a stanza\n").expect("a written line");

    let mut held = String::new();

    receiver.read_line(&mut held).expect("a read line");

    assert_eq!(held, "a stanza\n");
}

#[test]
fn a_reader_waits_for_a_writer_on_another_thread() {
    let (mut sender, mut receiver) = pipe();

    let writing = std::thread::spawn(move || {
        // The reader is already blocked in fill_buf by the time
        // this lands; the wake is what the drill is proving.
        std::thread::sleep(std::time::Duration::from_millis(50));
        sender
            .write_all(b"late but heard\n")
            .expect("a written line");
    });

    let mut held = String::new();

    receiver.read_line(&mut held).expect("a read line");
    writing.join().expect("the writing thread");

    assert_eq!(held, "late but heard\n");
}

#[test]
fn a_dropped_writer_ends_the_stream() {
    let (sender, mut receiver) = pipe();

    drop(sender);

    let mut held = String::new();

    assert_eq!(receiver.read_to_string(&mut held).expect("a read"), 0);
    assert!(held.is_empty());
}

#[test]
fn what_a_writer_said_before_hanging_up_still_reads() {
    let (mut sender, mut receiver) = pipe();

    sender.write_all(b"one\ntwo\n").expect("written lines");
    drop(sender);

    let mut held = String::new();

    receiver.read_to_string(&mut held).expect("a read");

    assert_eq!(held, "one\ntwo\n");
}

#[test]
fn a_writer_blocked_on_a_hangup_wakes_to_the_end() {
    let (sender, mut receiver) = pipe();

    let hanging = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        drop(sender);
    });

    // The read is already waiting: the hangup, not a byte, is what
    // releases it.
    let mut held = String::new();

    assert_eq!(receiver.read_to_string(&mut held).expect("a read"), 0);
    hanging.join().expect("the hanging thread");
}

#[test]
fn a_dropped_reader_breaks_the_writers_pipe() {
    let (mut sender, receiver) = pipe();

    drop(receiver);

    let refused = sender.write_all(b"into the void").expect_err("a refusal");

    assert_eq!(refused.kind(), std::io::ErrorKind::BrokenPipe);
}

#[test]
fn a_partial_read_leaves_the_rest_behind() {
    let (mut sender, mut receiver) = pipe();

    sender.write_all(b"abcdef").expect("a written run");

    let mut first = [0u8; 3];

    receiver.read_exact(&mut first).expect("a partial read");

    assert_eq!(&first, b"abc");

    let mut rest = String::new();

    drop(sender);
    receiver.read_to_string(&mut rest).expect("the remainder");

    assert_eq!(rest, "def");
}

#[test]
fn lines_written_in_pieces_still_arrive_whole() {
    let (mut sender, mut receiver) = pipe();

    let writing = std::thread::spawn(move || {
        for piece in ["a ", "line ", "in ", "pieces\n"] {
            sender.write_all(piece.as_bytes()).expect("a written piece");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    });

    let mut held = String::new();

    receiver.read_line(&mut held).expect("a read line");
    writing.join().expect("the writing thread");

    assert_eq!(held, "a line in pieces\n");
}
