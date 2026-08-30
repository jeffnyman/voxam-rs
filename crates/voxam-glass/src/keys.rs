//! Keystrokes, translated to the §3.8 input characters.
//!
//! Special keys arrive from crossterm with names; §3.8.2.2 and
//! §3.8.4 give the input-only ZSCII characters they mean. The
//! cursor keys travel as their §3.8.4 codepoints 129 to 132 --
//! characters no key actually types -- which the machine's input
//! seam passes through whole, so Beyond Zork's menus can hear
//! them. Anything unnamed passes through as the character it
//! already is; a key the table cannot spell -- a function key, a
//! resize -- is nothing usable.

use std::time::Duration;

use ratatui::crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, poll, read};

/// One keystroke's §3.8 character, or None for nothing usable --
/// an expired timeout, an unmapped key. A None timeout waits
/// forever for one read, though the read itself may still answer
/// nothing usable.
pub trait KeySource {
    fn key(&mut self, timeout: Option<Duration>) -> Option<char>;
}

/// One crossterm event's §3.8 character, or None for an event no
/// read can use. Key releases are events too on some terminals;
/// only presses and repeats type.
pub fn translated(event: &Event) -> Option<char> {
    let Event::Key(key) = event else {
        return None;
    };

    if key.kind == KeyEventKind::Release {
        return None;
    }

    match key.code {
        KeyCode::Enter => Some('\n'),
        KeyCode::Backspace | KeyCode::Delete => Some('\u{7f}'),
        KeyCode::Esc => Some('\u{1b}'),
        KeyCode::Up => Some('\u{81}'),
        KeyCode::Down => Some('\u{82}'),
        KeyCode::Left => Some('\u{83}'),
        KeyCode::Right => Some('\u{84}'),
        KeyCode::Char(character) => Some(character),
        _ => None,
    }
}

/// The live intake: crossterm's own event queue, translated.
///
/// The blocking read waits for one event; the timed read polls to
/// the deadline first. Either way one event is taken and
/// translated, and an unusable one -- like the reference's
/// unmapped escape sequence -- reports as nothing rather than
/// pretending; every caller already waits that out or lets the
/// timeout expire honestly.
#[derive(Default)]
pub struct EventKeys;

impl KeySource for EventKeys {
    fn key(&mut self, timeout: Option<Duration>) -> Option<char> {
        if let Some(patience) = timeout
            && !poll(patience).ok()?
        {
            return None;
        }

        let event = read().ok()?;

        // The reference's cbreak keeps SIGINT alive, so control-C
        // ends its session; raw mode ate the signal here, and the
        // intake restores the shell and dies the same death --
        // 130, the interrupted exit every shell recognises.
        if let Event::Key(key) = &event
            && key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            let _ = ratatui::crossterm::terminal::disable_raw_mode();
            println!();
            std::process::exit(130);
        }

        translated(&event)
    }
}

/// A scripted intake for the batteries: keys in order, with the
/// timeouts each read asked for kept on the record. None entries
/// are heartbeat expiries; a drained script answers expiries
/// forever, so a deadline read can run its clock out.
#[derive(Default)]
pub struct ScriptedKeys {
    /// The keystrokes still to come; a battery may refill them.
    pub keys: Vec<Option<char>>,
    /// Every timeout the painter asked with, in ask order.
    pub timeouts: Vec<Option<Duration>>,
}

impl ScriptedKeys {
    pub fn new(keys: Vec<Option<char>>) -> Self {
        Self {
            keys,
            timeouts: Vec::new(),
        }
    }

    /// The keystrokes of typed text, enter included when present.
    pub fn typed(text: &str) -> Self {
        Self::new(text.chars().map(Some).collect())
    }
}

impl KeySource for ScriptedKeys {
    fn key(&mut self, timeout: Option<Duration>) -> Option<char> {
        // The record is a test's ledger, not a black box: a
        // drained script inside a spinning wait would otherwise
        // grow it without bound.
        if self.timeouts.len() < 10_000 {
            self.timeouts.push(timeout);
        }

        if self.keys.is_empty() {
            return None;
        }

        self.keys.remove(0)
    }
}

/// A shared script: the battery keeps one handle to refill keys
/// and read the timeout record while the glass holds the other.
impl KeySource for std::rc::Rc<std::cell::RefCell<ScriptedKeys>> {
    fn key(&mut self, timeout: Option<Duration>) -> Option<char> {
        self.borrow_mut().key(timeout)
    }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
