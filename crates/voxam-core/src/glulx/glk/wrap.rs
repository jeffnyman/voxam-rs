//! Word wrapping for text buffer windows.
//!
//! Buffer windows wrap: the game emits a stream of styled
//! characters and the display decides where the lines break (Glk:
//! Text Buffer Windows). Only a display knows its width, so
//! wrapping belongs on the display side -- but every painted
//! display needs the same thing, so it lives here rather than
//! being written twice.
//!
//! Two things make this more than a call to a wrapping library.
//! Text arrives in pieces: a window hands over whatever
//! accumulated since the last flush, which may stop mid-word, so
//! the wrapper keeps the unfinished paragraph and folds the next
//! piece into it. And text is styled: breaking a line has to cut
//! the *segments* that make it up, not a flat string, or the
//! emphasis moves -- so the breaks are found in the plain text
//! and the segments sliced to match.
//!
//! The wrapper also keeps the paragraphs it has been given, which
//! is what makes a resize exact: display lines are recomputed
//! from the original text rather than re-broken from lines that
//! already lost their spaces at the break points.
//!
//! Positions and widths count characters, so the slicing runs on
//! character vectors rather than byte strings -- the reference's
//! Python indexing, spelled for UTF-8's sake.

/// A run of text sharing one appearance: (key, text).
///
/// The key is whatever the display wants to distinguish runs by,
/// and is only ever compared for equality here -- a Glk style
/// number for a test, a (style, link) pair for a painted display.
pub type Segment<K> = (K, String);

/// What a window should be showing at this moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View<K> {
    /// The display lines to show, oldest first.
    pub lines: Vec<Vec<Segment<K>>>,
    /// The index in the wrapper's lines that these begin at, for
    /// anything anchored to a line number.
    pub start: usize,
    /// Whether text is waiting that this view could not fit --
    /// what a pause prompt announces.
    pub more: bool,
}

/// How many completed paragraphs to remember. Past this the
/// oldest are dropped -- a terminal cannot scroll back to them
/// anyway, and a long game would otherwise accumulate its entire
/// transcript.
pub const SCROLLBACK: usize = 2000;
const TRIM: usize = 200;

/// Index ranges of text, one per display line.
///
/// Newlines are consumed, as is the space at each break. A word
/// wider than the line is cut rather than left to overflow.
fn spans(text: &[char], width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut spans = Vec::new();
    let mut position = 0;
    let length = text.len();

    loop {
        let newline = text[position..]
            .iter()
            .position(|&held| held == '\n')
            .map(|found| position + found);
        let end = newline.unwrap_or(length);
        let mut start = position;

        while end - start > width {
            let limit = start + width;
            // The break may fall on the character just past the
            // line, since a space there costs nothing to drop.
            let point = text[start..(limit + 1).min(end)]
                .iter()
                .rposition(|&held| held == ' ')
                .map(|found| start + found);

            match point {
                Some(point) if point > start => {
                    spans.push((start, point));
                    start = point + 1;
                }
                _ => {
                    spans.push((start, limit));
                    start = limit;
                }
            }
        }

        spans.push((start, end));

        match newline {
            None => return spans,
            Some(found) => position = found + 1,
        }
    }
}

/// Break text into lines of at most width characters.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let characters: Vec<char> = text.chars().collect();

    spans(&characters, width)
        .into_iter()
        .map(|(start, end)| characters[start..end].iter().collect())
        .collect()
}

/// Break one styled paragraph into styled display lines.
pub fn wrap_segments<K: Clone + PartialEq>(
    segments: &[Segment<K>],
    width: usize,
) -> Vec<Vec<Segment<K>>> {
    if segments.is_empty() {
        return vec![Vec::new()];
    }

    let pieces: Vec<Vec<char>> = segments
        .iter()
        .map(|(_, chunk)| chunk.chars().collect())
        .collect();
    let text: Vec<char> = pieces.iter().flatten().copied().collect();
    let mut starts = Vec::with_capacity(pieces.len());
    let mut position = 0;

    for piece in &pieces {
        starts.push(position);
        position += piece.len();
    }

    let mut lines = Vec::new();

    for (begin, finish) in spans(&text, width) {
        let mut line = Vec::new();

        for (index, (key, _)) in segments.iter().enumerate() {
            let at = starts[index];
            let to = at + pieces[index].len();

            if to <= begin || at >= finish {
                continue;
            }

            let piece: String = pieces[index][begin.max(at) - at..finish.min(to) - at]
                .iter()
                .collect();

            if !piece.is_empty() {
                line.push((key.clone(), piece));
            }
        }

        lines.push(line);
    }

    lines
}

/// The text of a display line, without its styling.
pub fn plain<K>(line: &[Segment<K>]) -> String {
    line.iter().map(|(_, chunk)| chunk.as_str()).collect()
}

/// Accumulates one window's styled output and wraps it to a width.
///
/// Keeps every paragraph, because a full-screen display repaints
/// from scratch and cannot re-ask the window for text it has
/// already drained.
pub struct Wrapper<K> {
    /// The width lines are currently wrapped to.
    pub width: usize,
    // Completed paragraphs, and the one still being written.
    history: Vec<Vec<Segment<K>>>,
    current: Vec<Segment<K>>,
    // Wrapped forms of each, recomputed only when they change.
    done: Option<Vec<Vec<Segment<K>>>>,
    tail: Option<Vec<Vec<Segment<K>>>>,
    /// How many display lines the player has been shown.
    /// Everything before this has had its turn on screen; text
    /// past it that will not fit in one windowful is what a pause
    /// prompt is for -- see view.
    pub seen: usize,
}

impl<K: Clone + PartialEq> Wrapper<K> {
    /// Start empty, wrapping to the given width.
    pub fn new(width: usize) -> Self {
        Self {
            width: width.max(1),
            history: Vec::new(),
            current: Vec::new(),
            done: Some(Vec::new()),
            tail: None,
            seen: 0,
        }
    }

    /// Fold more styled output in, continuing the open paragraph.
    pub fn add<I: IntoIterator<Item = Segment<K>>>(&mut self, runs: I) {
        for (key, text) in runs {
            if text.is_empty() {
                continue;
            }

            let mut pieces = text.split('\n');

            if let Some(first) = pieces.next() {
                self.extend(&key, first);
            }

            for piece in pieces {
                self.break_paragraph();
                self.extend(&key, piece);
            }
        }

        self.tail = None;
    }

    fn extend(&mut self, key: &K, text: &str) {
        if text.is_empty() {
            return;
        }

        if let Some((held, chunk)) = self.current.last_mut()
            && held == key
        {
            chunk.push_str(text);
        } else {
            self.current.push((key.clone(), text.to_string()));
        }
    }

    fn break_paragraph(&mut self) {
        let paragraph = std::mem::take(&mut self.current);

        if let Some(done) = self.done.as_mut() {
            done.extend(wrap_segments(&paragraph, self.width));
        }

        self.history.push(paragraph);

        if self.history.len() > SCROLLBACK + TRIM {
            // Trimmed in batches, so this is not paid on every
            // line.
            self.history.drain(..TRIM);
            self.done = None;
        }
    }

    /// Every display line, oldest first.
    pub fn lines(&mut self) -> Vec<Vec<Segment<K>>> {
        if self.done.is_none() {
            self.done = Some(
                self.history
                    .iter()
                    .flat_map(|paragraph| wrap_segments(paragraph, self.width))
                    .collect(),
            );
        }

        if self.tail.is_none() {
            self.tail = Some(wrap_segments(&self.current, self.width));
        }

        let mut lines = self.done.clone().expect("just ensured");

        lines.extend(self.tail.clone().expect("just ensured"));

        lines
    }

    /// The display lines as if runs had been added, without
    /// adding.
    ///
    /// A display draws the line the player is typing this way: it
    /// is part of the layout, but it is not part of the window's
    /// contents until the game accepts it.
    pub fn preview(&mut self, runs: &[Segment<K>]) -> Vec<Vec<Segment<K>>> {
        if runs.is_empty() {
            return self.lines();
        }

        let lines = self.lines();
        let tail = self.tail.as_ref().map_or(1, Vec::len).max(1);
        let mut previewed: Vec<Vec<Segment<K>>> = lines[..lines.len() - tail].to_vec();
        let mut composed = self.current.clone();

        composed.extend(runs.iter().cloned());
        previewed.extend(wrap_segments(&composed, self.width));

        previewed
    }

    // A window shows a windowful. If the game prints more than
    // that between two chances for the player to read, the excess
    // would scroll past unread, so the display stops and waits.
    // The model is glkterm's lastseenline: seen is the high-water
    // mark of what has been shown, and text beyond it that will
    // not fit is what holds things up.

    /// What to show now, where it starts, and whether more waits.
    ///
    /// Calling this *is* the display showing them, so it advances
    /// seen -- but only when there is nothing left waiting. While
    /// there is, the view stays put until advance is called,
    /// which is what makes the pause a pause.
    pub fn view(&mut self, height: usize) -> View<K> {
        let lines = self.lines();

        if height == 0 {
            return View {
                lines: Vec::new(),
                start: 0,
                more: false,
            };
        }

        if lines.len() - self.seen.min(lines.len()) <= height {
            // Everything unseen fits: show the newest windowful,
            // and the player has now had the lot. Idempotent,
            // which matters -- a repaint happens on every
            // keystroke.
            self.seen = lines.len();

            let start = lines.len().saturating_sub(height);

            return View {
                lines: lines[start..].to_vec(),
                start,
                more: false,
            };
        }

        let start = self.page_start();
        let end = (start + self.page(height)).min(lines.len());

        View {
            lines: lines[start..end].to_vec(),
            start,
            more: true,
        }
    }

    /// The player has read a page; move on to the next.
    ///
    /// Always at least one line further on. In a window one or
    /// two lines tall the page and the overlap are both a single
    /// line, and without this the pair cancel out and the prompt
    /// never clears.
    pub fn advance(&mut self, height: usize) {
        let page = self.page_start() + self.page(height);

        self.seen = self.lines().len().min((self.seen + 1).max(page));
    }

    /// Treat everything as read, however much of it there is.
    ///
    /// For the moments when pausing would be wrong: a file
    /// prompt, or a window the game has just cleared.
    pub fn catch_up(&mut self) {
        self.seen = self.lines().len();
    }

    /// Lines of text per page: the window, less the prompt's line.
    fn page(&self, height: usize) -> usize {
        height.saturating_sub(1).max(1)
    }

    fn page_start(&self) -> usize {
        // One line of overlap, so the page break does not read as
        // a gap.
        self.seen.saturating_sub(1)
    }

    /// Re-wrap everything for a new width.
    pub fn resize(&mut self, width: usize) {
        let width = width.max(1);

        if width == self.width {
            return;
        }

        self.width = width;
        self.done = None;
        self.tail = None;
    }

    /// Forget everything, as a cleared window has.
    pub fn clear(&mut self) {
        self.history.clear();
        self.current.clear();
        self.done = Some(Vec::new());
        self.tail = None;
        self.seen = 0;
    }
}

#[cfg(test)]
#[path = "wrap_tests.rs"]
mod tests;
