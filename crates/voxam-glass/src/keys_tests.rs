//! The keystroke translation, held to §3.8's input characters.

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::*;

fn keyed(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

// Named special keys translate to their §3.8.2.2 input
// characters: enter, delete, and escape all have ZSCII meanings.
#[test]
fn special_keys_translate() {
    assert_eq!(translated(&keyed(KeyCode::Enter)), Some('\n'));
    assert_eq!(translated(&keyed(KeyCode::Backspace)), Some('\u{7f}'));
    assert_eq!(translated(&keyed(KeyCode::Delete)), Some('\u{7f}'));
    assert_eq!(translated(&keyed(KeyCode::Esc)), Some('\u{1b}'));
}

// The cursor keys translate to their §3.8.4 codepoints 129 to
// 132, which the machine's input seam passes through whole -- how
// Beyond Zork's menus hear an arrow.
#[test]
fn cursor_keys_translate() {
    assert_eq!(translated(&keyed(KeyCode::Up)), Some('\u{81}'));
    assert_eq!(translated(&keyed(KeyCode::Down)), Some('\u{82}'));
    assert_eq!(translated(&keyed(KeyCode::Left)), Some('\u{83}'));
    assert_eq!(translated(&keyed(KeyCode::Right)), Some('\u{84}'));
}

// A plain character passes through as itself.
#[test]
fn plain_characters_pass_through() {
    assert_eq!(translated(&keyed(KeyCode::Char('n'))), Some('n'));
}

// A key the table cannot spell -- a function key the story
// cannot hear -- is nothing usable, and so is an event that is
// no key at all.
#[test]
fn unmapped_events_are_nothing_usable() {
    assert_eq!(translated(&keyed(KeyCode::F(5))), None);
    assert_eq!(translated(&Event::Resize(80, 24)), None);
}

// Key releases are events too on some terminals; only presses
// and repeats type.
#[test]
fn releases_do_not_type() {
    let mut release = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty());

    release.kind = KeyEventKind::Release;

    assert_eq!(translated(&Event::Key(release)), None);
}

// The scripted intake answers its keys in order, keeps the asked
// timeouts on the record, and answers expiries forever once
// drained.
#[test]
fn the_scripted_intake_keeps_the_record() {
    let mut script = ScriptedKeys::typed("a");

    assert_eq!(script.key(None), Some('a'));
    assert_eq!(
        script.key(Some(std::time::Duration::from_millis(100))),
        None
    );
    assert_eq!(
        script.timeouts,
        vec![None, Some(std::time::Duration::from_millis(100))]
    );
}
