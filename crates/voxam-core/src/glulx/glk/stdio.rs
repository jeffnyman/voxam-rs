//! A stdio display, in the manner of cheapglk.
//!
//! Text buffer windows stream to the output as their contents
//! accumulate. Text grid windows are drawn as a block whenever
//! they change, which is what an Inform status line amounts to on
//! a terminal that cannot address the cursor. Input is a line off
//! the input source.
//!
//! No cursor control, no styles, no partial redraw: this is the
//! minimum viable Glk, the display glulxercise needs and a piped
//! session can drive. Everything richer belongs to the painted
//! displays.
//!
//! Two reshapings from the reference, both seams rather than
//! behavior: output is a write callback and input always a source
//! callback (the CLI wires stdout and stdin in; a test wires
//! whatever it likes), and the click and link sources wait for a
//! recording that clicks. The terminal is never measured -- the
//! size is the classic 80x24 unless the host chooses one, which is
//! also what the reference answers in a piped session.

use std::collections::HashMap;

use crate::glulx::glk::frontend::{Asked, Frontend};

/// The witness seam: receives every run of buffer text as it
/// renders, which is where the refusal watch listens.
pub type Witness = Box<dyn FnMut(&str)>;
use crate::glulx::glk::objects::{Window, WindowKind, WindowMap, file_mode, key_code};

const DEFAULT_SIZE: (i64, i64) = (80, 24);

/// The status-line divider never grows past a sensible width,
/// even on a very wide terminal.
const DIVIDER_LIMIT: i64 = 60;

/// The acceptance grammar's key tokens replay as the Z-Machine's
/// input characters -- the §3.8.4 cursor codes and the §3.8.2.6
/// escape. Here those characters become the Glk keycodes they mean
/// (Glk: Character Input), so one recorded `<up>` presses up on
/// either machine.
fn token_keycode(character: char) -> Option<u32> {
    match character {
        '\u{81}' => Some(key_code::UP),
        '\u{82}' => Some(key_code::DOWN),
        '\u{83}' => Some(key_code::LEFT),
        '\u{84}' => Some(key_code::RIGHT),
        '\u{1b}' => Some(key_code::ESCAPE),
        _ => None,
    }
}

/// A display over two callbacks: an output writer and an input
/// source. A source answering None means end of input -- the
/// session is over, not broken.
pub struct StdioFrontend {
    output: Box<dyn FnMut(&str)>,
    source: Box<dyn FnMut() -> Option<String>>,
    witness: Option<Witness>,
    size: Option<(i64, i64)>,
    // Grids are redrawn only when they change, by window key.
    grids: HashMap<u32, Vec<String>>,
}

impl StdioFrontend {
    /// Stand over an output writer and an input source.
    ///
    /// A witness receives every run of buffer text as it renders,
    /// which is where the refusal watch listens.
    pub fn new(
        output: Box<dyn FnMut(&str)>,
        source: Box<dyn FnMut() -> Option<String>>,
        witness: Option<Witness>,
    ) -> Self {
        Self {
            output,
            source,
            witness,
            size: None,
            grids: HashMap::new(),
        }
    }

    /// Choose a display size other than the classic 80x24.
    pub fn sized(mut self, size: (i64, i64)) -> Self {
        self.size = Some(size);

        self
    }

    /// Walk the tree in visual order, drawing what shows.
    ///
    /// Visual order rather than tree order, so a status line split
    /// off above its buffer prints above it.
    fn render(&mut self, windows: &mut WindowMap, key: u32) {
        let Some(window) = windows.get(&key) else {
            return;
        };

        match &window.kind {
            WindowKind::Pair(pair) => {
                let mut children = [pair.child1, pair.child2];

                children.sort_by_key(|held| {
                    windows
                        .get(held)
                        .map_or((0, 0), |child| (child.bbox.1, child.bbox.0))
                });

                for child in children {
                    self.render(windows, child);
                }
            }
            WindowKind::Grid(_) => self.render_grid(windows, key),
            WindowKind::Buffer(_) => {
                // take_text drains the window, so each run of
                // output prints exactly once however often we are
                // flushed.
                let text = windows
                    .get_mut(&key)
                    .map_or_else(String::new, Window::take_text);

                if !text.is_empty() {
                    (self.output)(&text);

                    if let Some(witness) = &mut self.witness {
                        witness(&text);
                    }
                }
            }
            _ => {}
        }
    }

    /// Draw a grid as a block, only when its contents moved.
    fn render_grid(&mut self, windows: &WindowMap, key: u32) {
        let Some(window) = windows.get(&key) else {
            return;
        };

        let rows: Vec<String> = window
            .rows()
            .iter()
            .map(|row| row.trim_end().to_string())
            .collect();

        if rows.iter().all(String::is_empty) {
            return;
        }

        if self.grids.get(&key) == Some(&rows) {
            return;
        }

        self.grids.insert(key, rows.clone());

        for row in rows {
            (self.output)(&row);
            (self.output)("\n");
        }

        let divider = "-".repeat(self.size().0.clamp(0, DIVIDER_LIMIT) as usize);

        (self.output)(&divider);
        (self.output)("\n");
    }
}

impl Frontend for StdioFrontend {
    /// A terminal echoes as the player types, so Glk does not echo
    /// the line into the window as well.
    fn echoes_input(&self) -> bool {
        true
    }

    /// The chosen measure, or the classic 80x24.
    fn size(&self) -> (i64, i64) {
        self.size.unwrap_or(DEFAULT_SIZE)
    }

    /// Render what changed and push it out.
    fn flush(&mut self, windows: &mut WindowMap, root: Option<u32>) {
        if let Some(root) = root {
            self.render(windows, root);
        }
    }

    /// A line off the input, cut to what the buffer holds.
    fn read_line(
        &mut self,
        _windows: &mut WindowMap,
        _window: u32,
        maxlen: u32,
    ) -> Asked<(String, u32)> {
        match (self.source)() {
            Some(line) => Asked::Answer((line.chars().take(maxlen as usize).collect(), 0)),
            None => Asked::End,
        }
    }

    /// One keystroke: the first character of a line.
    ///
    /// The input is line-buffered, so this is the same compromise
    /// cheapglk makes -- and a bare Return reads as the Return
    /// keycode, which is what "press any key" prompts expect. A
    /// replayed key token arrives as its input character and
    /// leaves as the Glk keycode it means.
    fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
        match (self.source)() {
            None => Asked::End,
            Some(line) => match line.chars().next() {
                None => Asked::Answer(key_code::RETURN),
                Some(first) => {
                    Asked::Answer(token_keycode(first).unwrap_or_else(|| u32::from(first)))
                }
            },
        }
    }

    /// Ask for a filename in the stream; empty cancels, and so
    /// does the end of input.
    fn prompt_file(&mut self, _usage: u32, fmode: u32) -> Option<String> {
        let verb = if fmode == file_mode::READ {
            "Load from"
        } else {
            "Save to"
        };

        (self.output)(&format!("{verb} which file? "));

        let name = (self.source)()?;
        let name = name.trim();

        if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::glulx::glk::api::Glk;
    use crate::glulx::glk::objects::window_type;

    fn scripted(lines: &[&str]) -> (StdioFrontend, Rc<RefCell<String>>) {
        let written = Rc::new(RefCell::new(String::new()));
        let sink = written.clone();
        let mut feed: Vec<String> = lines.iter().map(|line| line.to_string()).collect();

        let frontend = StdioFrontend::new(
            Box::new(move |text| sink.borrow_mut().push_str(text)),
            Box::new(move || {
                if feed.is_empty() {
                    None
                } else {
                    Some(feed.remove(0))
                }
            }),
            None,
        );

        (frontend, written)
    }

    // A buffer streams once per drain; a grid draws as a block
    // with its divider, and only when it changes.
    #[test]
    fn buffers_stream_and_grids_block() {
        let (frontend, written) = scripted(&[]);
        let mut library = Glk::new(Box::new(frontend.sized((40, 12))));

        let base = library
            .glk_window_open(None, 0, 0, window_type::TEXT_BUFFER, 0)
            .unwrap()
            .unwrap();
        let grid = library
            .glk_window_open(
                Some(base),
                crate::glulx::glk::objects::window_method::ABOVE
                    | crate::glulx::glk::objects::window_method::FIXED,
                1,
                window_type::TEXT_GRID,
                0,
            )
            .unwrap()
            .unwrap();

        for character in "Score: 10".chars() {
            library
                .windows
                .get_mut(&grid)
                .unwrap()
                .put_char(u32::from(character), 0);
        }

        for character in "Hello.\n".chars() {
            library
                .windows
                .get_mut(&base)
                .unwrap()
                .put_char(u32::from(character), 0);
        }

        let root = library.root;

        library.frontend.flush(&mut library.windows, root);
        library.frontend.flush(&mut library.windows, root);

        let held = written.borrow().clone();

        // The status line above, the divider, the prose below --
        // and neither printed twice.
        assert_eq!(
            held,
            "Score: 10\n----------------------------------------\nHello.\n"
        );
    }

    // A line is cut to the buffer's capacity; a key token becomes
    // its Glk keycode; a bare Return reads as Return; the end of
    // input ends the session.
    #[test]
    fn input_translates_the_grammar() {
        let (mut frontend, _) = scripted(&["northwest", "\u{81}rest", "", "x"]);
        let mut windows = WindowMap::new();

        assert_eq!(
            frontend.read_line(&mut windows, 1, 5),
            Asked::Answer(("north".into(), 0))
        );
        assert_eq!(
            frontend.read_char(&mut windows, 1),
            Asked::Answer(key_code::UP)
        );
        assert_eq!(
            frontend.read_char(&mut windows, 1),
            Asked::Answer(key_code::RETURN)
        );
        assert_eq!(
            frontend.read_char(&mut windows, 1),
            Asked::Answer(u32::from(b'x'))
        );
        assert_eq!(frontend.read_line(&mut windows, 1, 8), Asked::End);
        assert_eq!(frontend.read_char(&mut windows, 1), Asked::End);
    }

    // The file prompt speaks its verb and takes the next line;
    // empty cancels.
    #[test]
    fn the_file_prompt_asks_in_the_stream() {
        let (mut frontend, written) = scripted(&["saga", "  "]);

        assert_eq!(
            frontend.prompt_file(0, file_mode::WRITE),
            Some("saga".into())
        );
        assert_eq!(frontend.prompt_file(0, file_mode::READ), None);
        assert_eq!(frontend.prompt_file(0, file_mode::READ), None);

        let held = written.borrow().clone();

        assert!(held.starts_with("Save to which file? "));
        assert!(held.contains("Load from which file? "));
    }
}
