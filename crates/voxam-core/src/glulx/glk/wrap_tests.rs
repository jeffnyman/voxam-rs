//! The wrapper: styled wrapping, the pager, and the scrollback.

use super::*;

/// One accumulated window, spoken to in plain text for brevity.
fn wrapped(width: usize, texts: &[&str]) -> Wrapper<u32> {
    let mut wrapper = Wrapper::new(width);

    wrapper.add(texts.iter().map(|text| (0, (*text).to_string())));

    wrapper
}

fn shown(wrapper: &mut Wrapper<u32>) -> Vec<String> {
    wrapper.lines().iter().map(|line| plain(line)).collect()
}

fn seg(key: u32, text: &str) -> Segment<u32> {
    (key, text.to_string())
}

// Lines break at spaces, and the space at the break costs nothing:
// it is dropped rather than carried to the next line.
#[test]
fn wrap_breaks_at_spaces() {
    assert_eq!(
        wrap("the deep magic word", 9),
        ["the deep", "magic", "word"]
    );
}

// The break may fall on the character just past the line, since a
// space there is about to be dropped anyway.
#[test]
fn wrap_breaks_on_the_space_past_the_line() {
    assert_eq!(wrap("xyzzy plugh", 5), ["xyzzy", "plugh"]);
}

// A word wider than the whole line is cut rather than left to
// overflow into a neighbouring window.
#[test]
fn wrap_cuts_a_word_wider_than_the_line() {
    assert_eq!(wrap("overincredulous", 6), ["overin", "credul", "ous"]);
}

// Newlines are consumed as hard breaks, and a blank line in the
// text stays a blank line on the display.
#[test]
fn wrap_honours_newlines() {
    assert_eq!(wrap("above\n\nbelow", 20), ["above", "", "below"]);
}

// A width below one is treated as one: no window is thin enough
// to hold no characters at all.
#[test]
fn wrap_clamps_the_width() {
    assert_eq!(wrap("ab", 0), ["a", "b"]);
}

// Breaking a line cuts the segments that make it up, so each
// piece keeps the style it arrived wearing.
#[test]
fn wrap_segments_keeps_the_styles() {
    let lines = wrap_segments(&[seg(1, "bold text "), seg(2, "and italic")], 10);

    assert_eq!(
        lines,
        vec![vec![seg(1, "bold text")], vec![seg(2, "and italic")]]
    );
}

// An empty paragraph is still one display line, or a blank line
// in the text would vanish from the layout.
#[test]
fn wrap_segments_of_nothing_is_one_empty_line() {
    assert_eq!(wrap_segments::<u32>(&[], 10), vec![Vec::new()]);
}

// A segment that straddles a blank line contributes nothing to
// it: the empty slice is skipped rather than kept as an empty
// piece.
#[test]
fn wrap_segments_leaves_blank_lines_empty() {
    let lines = wrap_segments(&[seg(0, "up\n\ndown")], 10);

    assert_eq!(
        lines,
        vec![vec![seg(0, "up")], Vec::new(), vec![seg(0, "down")]]
    );
}

// plain flattens a styled line back to its text.
#[test]
fn plain_strips_the_styling() {
    assert_eq!(plain(&[seg(1, "xy"), seg(2, "zzy")]), "xyzzy");
}

// Output arriving in pieces continues the open paragraph, and
// same-styled pieces fuse into one segment.
#[test]
fn the_wrapper_folds_pieces_into_the_open_paragraph() {
    let mut wrapper = wrapped(20, &["You are in a ", "maze", " of twisty passages"]);

    assert_eq!(
        shown(&mut wrapper),
        vec!["You are in a maze of", "twisty passages"]
    );
    assert_eq!(wrapper.lines()[0].len(), 1);
}

// Differently styled pieces stay separate segments, and empty
// pieces vanish without starting anything.
#[test]
fn the_wrapper_keeps_styles_apart() {
    let mut wrapper: Wrapper<u32> = Wrapper::new(30);

    wrapper.add([
        seg(0, "a "),
        seg(0, ""),
        seg(1, "magic"),
        seg(0, " word\n"),
        seg(0, ""),
    ]);

    assert_eq!(
        wrapper.lines()[0],
        vec![seg(0, "a "), seg(1, "magic"), seg(0, " word")]
    );
}

// A newline completes the paragraph; what follows opens the next.
#[test]
fn a_newline_breaks_the_paragraph() {
    let mut wrapper = wrapped(20, &["West of House\n", "You are standing"]);

    assert_eq!(
        shown(&mut wrapper),
        vec!["West of House", "You are standing"]
    );
}

// The preview shows the display lines as if the runs had been
// added, without adding them: the typed line takes part in the
// layout before the game has accepted it.
#[test]
fn the_preview_does_not_commit() {
    let mut wrapper = wrapped(20, &["What now?\n", "> "]);

    let preview = wrapper.preview(&[seg(8, "go north")]);

    assert_eq!(
        plain(preview.last().expect("a previewed line")),
        "> go north"
    );
    assert_eq!(shown(&mut wrapper), vec!["What now?", "> "]);
}

// Previewing nothing is just the lines as they stand.
#[test]
fn an_empty_preview_is_the_lines() {
    let mut wrapper = wrapped(20, &["steady"]);
    let lines = wrapper.lines();

    assert_eq!(wrapper.preview(&[]), lines);
}

// When everything unseen fits in the window, the view is the
// newest windowful and the player is considered to have read it.
#[test]
fn a_view_that_fits_advances_seen() {
    let mut wrapper = wrapped(20, &["one\ntwo\nthree"]);

    let view = wrapper.view(5);
    let texts: Vec<String> = view.lines.iter().map(|line| plain(line)).collect();

    assert_eq!(texts, vec!["one", "two", "three"]);
    assert_eq!(view.start, 0);
    assert!(!view.more);
    assert_eq!(wrapper.seen, 3);
}

// More text than a windowful holds the view at the first page,
// repaint after repaint, until the player advances -- which is
// what makes the pause a pause.
#[test]
fn a_full_window_waits_to_be_read() {
    let text: Vec<String> = (0..9).map(|index| index.to_string()).collect();
    let mut wrapper = wrapped(10, &[&text.join("\n")]);

    let first = wrapper.view(4);
    let texts: Vec<String> = first.lines.iter().map(|line| plain(line)).collect();

    assert!(first.more);
    assert_eq!(texts, vec!["0", "1", "2"]);
    assert_eq!(wrapper.view(4), first);

    wrapper.advance(4);

    let second = wrapper.view(4);

    assert!(second.more);
    assert_eq!(second.start, 2);

    wrapper.advance(4);

    let last = wrapper.view(4);
    let texts: Vec<String> = last.lines.iter().map(|line| plain(line)).collect();

    assert!(!last.more);
    assert!(texts.contains(&"8".to_string()));
}

// The view of a window with no rows is nothing at all.
#[test]
fn a_flat_window_shows_nothing() {
    let mut wrapper = wrapped(10, &["words"]);

    assert!(wrapper.view(0).lines.is_empty());
}

// In a one-line window the page and the overlap are both a single
// line; the advance still moves, or the prompt would never clear.
#[test]
fn a_tiny_window_still_turns_its_page() {
    let mut wrapper = wrapped(10, &["a\nb\nc"]);

    assert!(wrapper.view(1).more);

    wrapper.advance(1);

    assert_eq!(wrapper.seen, 1);
}

// Catching up declares everything read, however much is waiting.
#[test]
fn catching_up_reads_everything() {
    let letters: Vec<String> = "abcdefgh".chars().map(String::from).collect();
    let mut wrapper = wrapped(10, &[&letters.join("\n")]);

    assert!(wrapper.view(3).more);

    wrapper.catch_up();

    assert!(!wrapper.view(3).more);
}

// A resize recomputes the display lines from the original
// paragraphs, so no break point loses its space twice.
#[test]
fn a_resize_rewraps_from_the_paragraphs() {
    let mut wrapper = wrapped(20, &["hello wide world"]);

    assert_eq!(shown(&mut wrapper), vec!["hello wide world"]);

    wrapper.resize(10);

    assert_eq!(shown(&mut wrapper), vec!["hello wide", "world"]);

    wrapper.resize(10);

    assert_eq!(shown(&mut wrapper), vec!["hello wide", "world"]);
}

// A cleared window has no past.
#[test]
fn clearing_forgets_everything() {
    let mut wrapper = wrapped(10, &["gone\n"]);

    wrapper.view(3);
    wrapper.clear();

    assert_eq!(shown(&mut wrapper), vec![""]);
    assert_eq!(wrapper.seen, 0);
}

// Past the scrollback limit the oldest paragraphs are dropped in
// a batch, and the display lines are recomputed from what
// remains.
#[test]
fn the_scrollback_is_bounded() {
    let mut wrapper: Wrapper<u32> = Wrapper::new(20);

    for index in 0..(SCROLLBACK + 202) {
        wrapper.add([seg(0, &format!("turn {index}\n"))]);
    }

    let lines = shown(&mut wrapper);

    assert!(lines.len() <= SCROLLBACK + 3);
    assert_eq!(lines[0], "turn 200");
    assert_eq!(lines[lines.len() - 2], format!("turn {}", SCROLLBACK + 201));
}
