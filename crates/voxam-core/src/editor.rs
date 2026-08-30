//! The interpreter's line editor for painted input (§15 read).
//!
//! Line editing belongs to the interpreter, not the machine: §15
//! read hands whole lines to the game, and how the player composes
//! one -- moving the cursor within it, correcting the middle,
//! recalling an earlier command -- is interpreter courtesy, the
//! same courtesy the classic interpreters offered. The editor here
//! is pure state: a buffer, an insertion point, and a session
//! history, with each keystroke a small transition, so the whole
//! vocabulary is testable without a terminal. The painted
//! frontends translate transitions onto their glass; recordings
//! and replays never meet the editor, because only the submitted
//! line reaches the machine.
//!
//! The cursor keys do double duty on the Z-Machine: §3.8.4 defines
//! them as input characters so a game like Beyond Zork can hear
//! them in read_char menus. Single-keystroke reads still pass them
//! through whole -- the editor lives only inside line input, where
//! today no key can reach the game before the line is done. When
//! §10.7 terminating characters arrive, a game that names the
//! cursor keys as terminators will take precedence over the
//! editor's use of them.

// The editing vocabulary, in §3.8 input characters: both classic
// delete bytes rub out, the cursor keys move and recall, and escape
// with the remaining §3.8.4 input-only codes means nothing to a
// line and is waited out.
pub const RUB_OUT_KEYS: [char; 2] = ['\u{7f}', '\u{8}'];
pub const CURSOR_UP: char = '\u{81}';
pub const CURSOR_DOWN: char = '\u{82}';
pub const CURSOR_LEFT: char = '\u{83}';
pub const CURSOR_RIGHT: char = '\u{84}';
pub const INPUT_ONLY_FIRST: char = '\u{81}';
pub const INPUT_ONLY_LAST: char = '\u{9a}';
pub const ESCAPE: char = '\u{1b}';
pub const NEWLINE: char = '\n';
// A bare carriage return IS the return key -- ZSCII 13 (§3.8.2.5)
// -- on a terminal that hands it over without naming it.
pub const CARRIAGE_RETURN: char = '\r';

/// A session keeps this many submitted lines for recall.
pub const HISTORY_LIMIT: usize = 100;

/// A key source may answer this instead of a key to say a timed
/// wait expired (§15 read): the loop hands back None with the
/// composed line intact, so the read can resume after the game's
/// interrupt has run. NUL can never arrive as real typing.
pub const EXPIRED: char = '\0';

/// What the editor needs from a screen model to echo edits.
pub trait LineCanvas {
    /// Print text at the cursor.
    fn write(&mut self, text: &str);

    /// Move the cursor left without erasing; answer cells moved.
    fn retreat(&mut self, cells: usize) -> usize;
}

impl LineCanvas for crate::screen::ScreenModel {
    fn write(&mut self, text: &str) {
        crate::screen::ScreenModel::write(self, text);
    }

    fn retreat(&mut self, cells: usize) -> usize {
        crate::screen::ScreenModel::retreat(self, cells)
    }
}

/// A line being composed, with the session's history behind it.
///
/// One editor lives per frontend, so the history spans the whole
/// session: every submitted line joins it, and the cursor-up key
/// walks back through it the way every shell since has. Recalling
/// preserves the interrupted draft -- cursor-down past the newest
/// history line restores it.
#[derive(Default)]
pub struct LineEditor {
    history: Vec<String>,
    buffer: Vec<char>,
    cursor: usize,
    recall: Option<usize>,
    draft: String,
}

impl LineEditor {
    /// Start with an empty line and an empty session history.
    pub fn new() -> Self {
        Self::default()
    }

    /// The line as composed so far.
    pub fn text(&self) -> String {
        self.buffer.iter().collect()
    }

    /// The insertion point, in characters from the line's start.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Start composing a fresh, empty line.
    pub fn begin(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.recall = None;
        self.draft = String::new();
    }

    /// Type one character at the insertion point.
    pub fn insert(&mut self, character: char) {
        self.buffer.insert(self.cursor, character);
        self.cursor += 1;
    }

    /// Delete the character before the insertion point.
    ///
    /// Answers whether anything was deleted; at the line's start
    /// there is nothing left of the line to rub.
    pub fn rub_out(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }

        self.cursor -= 1;
        self.buffer.remove(self.cursor);

        true
    }

    /// Move the insertion point one character left.
    pub fn left(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }

        self.cursor -= 1;

        true
    }

    /// Move the insertion point one character right.
    pub fn right(&mut self) -> bool {
        if self.cursor == self.buffer.len() {
            return false;
        }

        self.cursor += 1;

        true
    }

    /// Recall the previous history line, saving the draft first.
    ///
    /// Answers whether the line changed; with no history, or at
    /// the oldest line already, there is nothing earlier.
    pub fn earlier(&mut self) -> bool {
        match self.recall {
            None => {
                if self.history.is_empty() {
                    return false;
                }

                self.draft = self.text();
                self.recall = Some(self.history.len() - 1);
            }
            Some(recall) if recall > 0 => {
                self.recall = Some(recall - 1);
            }
            Some(_) => return false,
        }

        let recalled = &self.history[self.recall.expect("a recall was just set")];

        self.buffer = recalled.chars().collect();
        self.cursor = self.buffer.len();

        true
    }

    /// Walk forward through history, back to the saved draft.
    ///
    /// Answers whether the line changed; without a recall in
    /// progress there is nothing later to return to.
    pub fn later(&mut self) -> bool {
        let Some(recall) = self.recall else {
            return false;
        };

        let recall = recall + 1;

        if recall == self.history.len() {
            self.recall = None;
            self.buffer = self.draft.chars().collect();
        } else {
            self.recall = Some(recall);
            self.buffer = self.history[recall].chars().collect();
        }

        self.cursor = self.buffer.len();

        true
    }

    /// Finish the line: record it in history and reset.
    ///
    /// An empty line never joins the history, and a line matching
    /// the newest entry joins only once -- pressing cursor-up
    /// after repeating a command should not walk through the
    /// repetitions.
    pub fn submit(&mut self) -> String {
        let line = self.text();

        if !line.is_empty() && self.history.last() != Some(&line) {
            self.history.push(line.clone());

            if self.history.len() > HISTORY_LIMIT {
                self.history.remove(0);
            }
        }

        self.begin();

        line
    }

    /// The editing keys, each naming its transition. Every
    /// transition answers whether the line changed -- the loop
    /// redraws only when one did; a key that is not an edit
    /// answers None.
    fn edited(&mut self, key: char) -> Option<bool> {
        match key {
            key if RUB_OUT_KEYS.contains(&key) => Some(self.rub_out()),
            CURSOR_UP => Some(self.earlier()),
            CURSOR_DOWN => Some(self.later()),
            CURSOR_LEFT => Some(self.left()),
            CURSOR_RIGHT => Some(self.right()),
            _ => None,
        }
    }
}

/// Retreat to the line's start and repaint it whole: the new text,
/// a blanked remnant where the line shrank, and the cursor walked
/// back to its place. The glass ledger -- cells painted, cursor
/// cells from the line's start -- updates alongside.
fn redrawn<C: LineCanvas + ?Sized>(
    editor: &LineEditor,
    canvas: &mut C,
    repaint: &mut dyn FnMut(&mut C),
    painted: &mut usize,
    at: &mut usize,
) {
    canvas.retreat(*at);

    let text = editor.text();
    let count = text.chars().count();

    canvas.write(&text);

    if *painted > count {
        let remnant = *painted - count;

        canvas.write(&" ".repeat(remnant));
        canvas.retreat(remnant);
    }

    canvas.retreat(count - editor.cursor());

    *painted = count;
    *at = editor.cursor();

    repaint(canvas);
}

/// Run one line read through the editor, echoing via the canvas.
///
/// The shared loop behind both painted frontends: keys arrive raw
/// from the frontend's own source, the editor transitions, and any
/// visible change is redrawn through the canvas -- so nothing but
/// the frontend ever writes to its glass. Appending at the line's
/// end takes the fast path of a single write; every other edit
/// repaints the line whole from its start. The canvas's retreat
/// stops at the left edge, as its rub_out always has, so on the
/// rare line that wrapped only the final row redraws -- the
/// returned line is right regardless, because the buffer, not the
/// glass, is what the game receives.
///
/// A key source that answers EXPIRED ends the call with None: the
/// composed line stays in the editor and on the glass, and a later
/// call with fresh=false resumes it exactly where it stood -- how
/// a timed read survives its interrupts (§15 read).
///
/// The repaint takes the canvas back -- the reference's bound
/// method captured its frontend whole; here the canvas is lent
/// for the whole read, so any repainting of it must borrow it
/// through the loop's own hand.
pub fn read_line_edited<C: LineCanvas + ?Sized>(
    editor: &mut LineEditor,
    canvas: &mut C,
    key_source: &mut dyn FnMut() -> Option<char>,
    repaint: &mut dyn FnMut(&mut C),
    fresh: bool,
) -> Option<String> {
    // Cells on the glass since the line began, and the canvas
    // cursor in cells from the line's start.
    let (mut painted, mut at) = if fresh {
        editor.begin();

        (0, 0)
    } else {
        (editor.text().chars().count(), editor.cursor())
    };

    loop {
        let key = match key_source() {
            Some(EXPIRED) => return None,
            None | Some(ESCAPE) => continue,
            Some(key) => key,
        };

        if key == NEWLINE || key == CARRIAGE_RETURN {
            canvas.write("\n");
            repaint(canvas);

            return Some(editor.submit());
        }

        if let Some(changed) = editor.edited(key) {
            if changed {
                redrawn(editor, canvas, repaint, &mut painted, &mut at);
            }
        } else if key < ' ' || (INPUT_ONLY_FIRST..=INPUT_ONLY_LAST).contains(&key) {
            // The §3.8.4 input-only codes beyond the editing keys,
            // and every raw control character -- the tab chief
            // among them -- mean nothing to a line: no ZSCII code
            // to submit (§3.8), no glyph to echo, so they are
            // waited out rather than inserted to crash at submit.
            continue;
        } else {
            let appending = editor.cursor() == editor.buffer.len();

            editor.insert(key);

            if appending {
                canvas.write(&key.to_string());
                painted += 1;
                at += 1;
                repaint(canvas);
            } else {
                redrawn(editor, canvas, repaint, &mut painted, &mut at);
            }
        }
    }
}

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
