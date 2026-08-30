//! The §8 screen model's battery, mirrored from the reference.

use super::*;
use crate::frontend::GRAPHICS_FONT;

const WIDTH: usize = 20;
const HEIGHT: usize = 6;

fn small(version: u8) -> ScreenModel {
    ScreenModel::new(WIDTH, HEIGHT, version)
}

fn refused(result: Result<(), VoxamError>, wants: &str) {
    let error = result.expect_err("the screen should refuse").to_string();

    assert!(error.contains(wants), "{error}");
}

// The start-of-game screen is cleared with the cursor at the
// bottom left through Version 4, so text scrolls upward as the
// game gets under way (§8.6.3, §8.7.3.3).
#[test]
fn versions_through_4_start_at_the_bottom() {
    let mut screen = small(4);

    screen.write("hello");

    assert_eq!(screen.row_text(HEIGHT), "hello");
}

// From Version 5 the start-of-game cursor sits at the top left
// (§8.7.3.3).
#[test]
fn version_5_starts_at_the_top() {
    let mut screen = small(5);

    screen.write("hello");

    assert_eq!(screen.row_text(1), "hello");
}

// While buffering is on, a word that would overrun the margin
// wraps whole onto the next line (§15 buffer_mode).
#[test]
fn buffered_text_wraps_at_word_boundaries() {
    let mut screen = small(5);

    screen.write("a mellifluous parsing");

    assert_eq!(screen.row_text(1), "a mellifluous");
    assert_eq!(screen.row_text(2), "parsing");
}

// A space that would wrap becomes the line break itself: the next
// line never opens with the gap.
#[test]
fn a_margin_space_becomes_the_break() {
    let mut screen = small(5);

    screen.write(&format!("{} bb", "a".repeat(WIDTH)));

    assert_eq!(screen.row_text(1), "a".repeat(WIDTH));
    assert_eq!(screen.row_text(2), "bb");
}

// A word too long for any line has no whole line to wait for, so
// it character-wraps.
#[test]
fn an_overlong_word_character_wraps() {
    let mut screen = small(5);

    screen.write(&"x".repeat(WIDTH + 3));

    assert_eq!(screen.row_text(1), "x".repeat(WIDTH));
    assert_eq!(screen.row_text(2), "xxx");
}

// With buffering off, text breaks wherever the margin falls (§15
// buffer_mode).
#[test]
fn unbuffered_text_breaks_at_the_margin() {
    let mut screen = small(5);

    screen.set_buffering(false);
    screen.write("abcdefghij klmnopqrstuv");

    assert_eq!(screen.row_text(1), "abcdefghij klmnopqrs");
    assert_eq!(screen.row_text(2), "tuv");
}

// When text reaches the bottom of the lower window it scrolls
// upward, and the upper region stays put (§8.6.2, §8.7.3.1).
#[test]
fn the_lower_window_scrolls_and_the_upper_does_not() {
    let mut screen = small(5);

    screen.split_window(1).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.write("STATUS");
    screen.set_window(LOWER).unwrap();

    for number in 1..8 {
        screen.write(&format!("line {number}\n"));
    }

    assert_eq!(screen.row_text(1), "STATUS");
    assert_eq!(screen.row_text(2), "line 3");
    assert_eq!(screen.row_text(HEIGHT), "line 7");
}

// The fresh line a scroll exposes is blank without reverse video,
// even while the text style is Reverse (§8.7.3.1).
#[test]
fn scrolling_never_reverses_the_fresh_line() {
    let mut screen = small(5);

    screen.set_style(REVERSE);

    for number in 1..8 {
        screen.write(&format!("row {number}\n"));
    }

    screen.write("row 8");

    assert_eq!(screen.cell(HEIGHT, 10).style, ROMAN);
}

// Styles dress the characters printed after them; Roman clears
// the combination (§8.7.1, §15 set_text_style).
#[test]
fn styles_combine_and_roman_clears() {
    let mut screen = small(5);

    screen.set_style(BOLD);
    screen.set_style(ITALIC);
    screen.write("ab");
    screen.set_style(ROMAN);
    screen.write("c");

    assert_eq!(screen.cell(1, 1).style, BOLD | ITALIC);
    assert_eq!(screen.cell(1, 3).style, ROMAN);
}

// Changing style mid-word is legal, so each pending character
// keeps the style it was printed in (§8.7.1.2).
#[test]
fn styles_may_change_mid_word() {
    let mut screen = small(5);

    screen.write("pa");
    screen.set_style(BOLD);
    screen.write("rser");

    assert_eq!(screen.cell(1, 2).style, ROMAN);
    assert_eq!(screen.cell(1, 3).style, BOLD);
}

// The line editor's rubout retreats the cursor one cell and
// blanks it, and stops at the left edge rather than chewing into
// an earlier row (§15 read).
#[test]
fn rub_out_erases_the_last_typed_character() {
    let mut screen = small(5);

    screen.write("ab");
    screen.rub_out();

    assert_eq!(screen.row_text(1), "a");
    assert_eq!(screen.cursor(), (1, 2));

    screen.rub_out();
    screen.rub_out();

    assert_eq!(screen.row_text(1), "");
    assert_eq!(screen.cursor(), (1, 1));
}

// Rubout follows the selected window: upper-window typing is
// edited in place, and at the window's left edge there is nothing
// to rub.
#[test]
fn rub_out_works_in_the_upper_window() {
    let mut screen = small(5);

    screen.split_window(2).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.set_cursor(1, 3).unwrap();
    screen.write("x");
    screen.rub_out();

    assert_eq!(screen.row_text(1), "");
    assert_eq!(screen.cursor(), (1, 3));

    screen.set_cursor(1, 1).unwrap();
    screen.rub_out();

    assert_eq!(screen.cursor(), (1, 1));
}

// A hung more callback fires after a screenful of lower-window
// lines -- the window's height less the prompt's own line -- and
// the count starts over after each pause (§8.8.3.2.6's courtesy,
// offered on the two-window screen).
#[test]
fn more_fires_at_a_screenful() {
    use std::cell::Cell as Counter;
    use std::rc::Rc;

    let pauses = Rc::new(Counter::new(0));
    let seen = pauses.clone();

    // The callback crosses no threads; the counter makes the
    // FnMut Send-free sharing explicit.
    struct Held(Rc<Counter<usize>>);

    let held = Held(seen);
    let mut screen = small(5);

    screen.more = Some(Box::new(move |_model| held.0.set(held.0.get() + 1)));

    screen.write("\n\n\n\n");

    assert_eq!(pauses.get(), 0);

    screen.write("\n");

    assert_eq!(pauses.get(), 1);

    screen.write("\n\n\n\n\n");

    assert_eq!(pauses.get(), 2);
}

// Input rests the budget, and erasing the lower window refills
// it: read text and erased text alike cannot be unread.
#[test]
fn rest_and_erase_refill_the_more_budget() {
    use std::cell::Cell as Counter;
    use std::rc::Rc;

    let pauses = Rc::new(Counter::new(0));
    let seen = pauses.clone();
    let mut screen = small(5);

    screen.more = Some(Box::new(move |_model| seen.set(seen.get() + 1)));

    screen.write("\n\n\n\n");
    screen.rest();
    screen.write("\n\n\n\n");

    assert_eq!(pauses.get(), 0);

    screen.erase_window(0).unwrap();
    screen.write("\n\n\n\n");

    assert_eq!(pauses.get(), 0);
}

// The upper window neither scrolls nor counts, and a split
// narrows the page to the lower window that remains.
#[test]
fn upper_window_feeds_no_more_budget() {
    use std::cell::Cell as Counter;
    use std::rc::Rc;

    let pauses = Rc::new(Counter::new(0));
    let seen = pauses.clone();
    let mut screen = small(5);

    screen.more = Some(Box::new(move |_model| seen.set(seen.get() + 1)));

    screen.split_window(2).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.write("\n\n\n\n\n\n\n\n");

    assert_eq!(pauses.get(), 0);

    screen.set_window(0).unwrap();
    screen.write("\n\n\n");

    assert_eq!(pauses.get(), 1);
}

// The line editor's cursor motion retreats without erasing: the
// text stays painted, the motion clamps at the left edge, and the
// cells actually moved come back (§15 read).
#[test]
fn retreat_moves_the_cursor_without_erasing() {
    let mut screen = small(5);

    screen.write("ab");

    assert_eq!(screen.retreat(1), 1);
    assert_eq!(screen.row_text(1), "ab");
    assert_eq!(screen.cursor(), (1, 2));
    assert_eq!(screen.retreat(5), 1);
    assert_eq!(screen.cursor(), (1, 1));
}

// Retreat follows the selected window, clamped at the upper
// window's left edge just the same.
#[test]
fn retreat_works_in_the_upper_window() {
    let mut screen = small(5);

    screen.split_window(2).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.set_cursor(1, 3).unwrap();

    assert_eq!(screen.retreat(9), 2);
    assert_eq!(screen.cursor(), (1, 1));
}

// A §15 rectangle in the upper window spreads right and down from
// the cursor: each row returns to the starting column, so a map
// can sit beside a story box without erasing its left edge --
// which is precisely how Beyond Zork stamps its map (§15
// print_table).
#[test]
fn upper_rectangles_keep_their_left_edge() {
    let mut screen = small(5);

    screen.split_window(4).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.set_cursor(2, 5).unwrap();
    screen.write_rectangle(&["ab".to_string(), "cd".to_string()]);

    assert_eq!(screen.row_text(2), "    ab");
    assert_eq!(screen.row_text(3), "    cd");
}

// A rectangle taller than the upper window presses its last rows
// onto the bottom line, as upper-window newlines do (§8.7.2).
#[test]
fn tall_rectangles_press_on_the_window_bottom() {
    let mut screen = small(5);

    screen.split_window(2).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.set_cursor(1, 1).unwrap();
    screen.write_rectangle(&["a".to_string(), "b".to_string(), "c".to_string()]);

    assert_eq!(screen.row_text(1), "a");
    assert_eq!(screen.row_text(2), "c");
}

// In the lower window, where §15 leaves heights past 1 undefined,
// the rows are ordinary stacked lines.
#[test]
fn lower_rectangles_stack_as_lines() {
    let mut screen = small(5);

    screen.write_rectangle(&["ab".to_string(), "cd".to_string()]);

    assert_eq!(screen.row_text(1), "ab");
    assert_eq!(screen.row_text(2), "cd");
}

// Cells remember the font they were printed in, and changing font
// mid-word is as legal as changing style there (§8.1.2,
// §8.1.3.1); drawing §16's shapes from that record is the
// painter's business.
#[test]
fn cells_wear_the_current_font() {
    let mut screen = small(5);

    screen.write("ma");
    screen.set_font(GRAPHICS_FONT);
    screen.write("p!");
    screen.set_font(NORMAL_FONT);
    screen.write("x");

    assert_eq!(screen.cell(1, 2).font, NORMAL_FONT);
    assert_eq!(screen.cell(1, 3).font, GRAPHICS_FONT);
    assert_eq!(screen.cell(1, 5).font, NORMAL_FONT);
}

// Colour code 0 keeps the colour already current, on either side
// of the pair (§8.3.1).
#[test]
fn colour_zero_keeps_the_current_colour() {
    let mut screen = small(5);

    screen.set_colour(3, 6);
    screen.set_colour(0, 4);
    screen.set_colour(5, 0);
    screen.write("x");

    assert_eq!(screen.cell(1, 1).foreground, 5);
    assert_eq!(screen.cell(1, 1).background, 4);
    assert_eq!(screen.background(), 4);
}

// The model reports its own dimensions, which the painter and the
// header both consult (§8.4).
#[test]
fn the_screen_knows_its_dimensions() {
    let screen = small(3);

    assert_eq!(screen.columns(), WIDTH);
    assert_eq!(screen.lines(), HEIGHT);
}

// Versions 1 and 2 are teletypes: their screens can only be
// printed to, and the window opcodes have nothing to talk to
// (§8.5.1).
#[test]
fn teletype_versions_refuse_windows() {
    let mut screen = small(1);

    refused(screen.split_window(2), "§8.5.1");
    refused(screen.set_window(UPPER), "§8.5.1");
}

// Selecting the upper window homes its cursor to the top left
// every time (§8.6.1, §8.7.2), and printing there overlays the
// screen without disturbing the lower window's cursor.
#[test]
fn selecting_the_upper_window_homes_its_cursor() {
    let mut screen = small(5);

    screen.split_window(2).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.set_cursor(2, 5).unwrap();
    screen.set_window(LOWER).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.write("TOP");

    assert_eq!(screen.row_text(1), "TOP");
}

// In Version 3 the upper window hangs below the interpreter's
// status line, so its first row is the screen's second
// (§8.6.1.1).
#[test]
fn the_version_3_upper_window_sits_below_the_status_line() {
    let mut screen = small(3);

    screen.split_window(1).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.write("BELOW");

    assert_eq!(screen.row_text(1), "");
    assert_eq!(screen.row_text(2), "BELOW");
}

// A Version 3 split clears the freshly split upper window
// (§8.6.1.1.2); from Version 4 the screen's appearance is left
// alone (§8.6.1).
#[test]
fn version_3_splits_clear_and_version_5_splits_do_not() {
    let mut torn = small(3);

    torn.write("\n\nold text here\n\n\n");
    torn.split_window(2).unwrap();

    assert_eq!(torn.row_text(2), "");

    let mut kept = small(5);

    kept.write("old text here");
    kept.split_window(2).unwrap();

    assert_eq!(kept.row_text(1), "old text here");
}

// A split that would swallow the lower window's cursor pushes it
// down to the line just below the new upper window (§8.7.2.2).
#[test]
fn a_split_cannot_swallow_the_lower_cursor() {
    let mut screen = small(5);

    screen.write("top line");
    screen.split_window(3).unwrap();
    screen.write("pushed");

    assert_eq!(screen.row_text(4), "pushed");
}

// A split made while the upper window is selected keeps its
// cursor when still inside the new size, homing it otherwise
// (§8.7.2.1.1).
#[test]
fn a_split_over_the_selected_upper_window_keeps_or_homes() {
    let mut screen = small(5);

    screen.split_window(3).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.set_cursor(3, 4).unwrap();
    screen.split_window(4).unwrap();

    assert_eq!(screen.get_cursor(), (3, 4));

    screen.split_window(2).unwrap();

    assert_eq!(screen.get_cursor(), (1, 1));
}

// The upper window may take the whole screen -- Z-Tornado plays
// its entire game that way -- but not more than exists, and never
// a negative height (§8.7.2.1).
#[test]
fn a_full_height_split_is_legal_and_larger_is_not() {
    let mut screen = small(5);

    screen.split_window(HEIGHT as i32).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.set_cursor(HEIGHT as u16, 1).unwrap();
    screen.write("floor");

    assert_eq!(screen.row_text(HEIGHT), "floor");

    refused(screen.split_window(HEIGHT as i32 + 1), "§8.7.2.1");
    refused(screen.split_window(-1), "§8.7.2.1");
}

// Only windows 0 and 1 exist before Version 6 (§8.7.2).
#[test]
fn unknown_windows_cannot_be_selected() {
    let mut screen = small(5);

    refused(screen.set_window(3), "§8.7.2");
}

// set_cursor speaks (row, column) with (1,1) at the window's top
// left. §8.7.2.3.1 calls a move outside the upper window illegal,
// but the settlement is Frotz's silent tolerance out to the
// screen's edge -- Solitaire Poker splits 20 rows and deals from
// row 21 -- while a move past the physical screen stays loud, and
// a column past the width was never tolerated by anyone.
#[test]
fn the_upper_cursor_tolerates_overreach_to_the_screen() {
    let mut screen = small(5);

    screen.split_window(2).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.set_cursor(2, 3).unwrap();
    screen.write("X");

    assert_eq!(screen.row_text(2), "  X");

    screen.set_cursor(3, 1).unwrap();
    screen.write("Y");

    assert_eq!(screen.row_text(3), "Y");

    refused(screen.set_cursor(HEIGHT as u16 + 1, 1), "§8.7.2.3.1");
    refused(screen.set_cursor(1, WIDTH as u16 + 1), "§8.7.2.3.1");
}

// The opcode has no effect when the lower window is selected --
// the spec's own sentence, so the quiet is conforming
// (§8.7.2.3.1).
#[test]
fn set_cursor_in_the_lower_window_does_nothing() {
    let mut screen = small(5);

    screen.split_window(2).unwrap();
    screen.set_cursor(2, 2).unwrap();
    screen.write("stays put");

    assert_eq!(screen.row_text(3), "stays put");
}

// get_cursor reports the upper window's cursor whichever window
// is selected (§8.7.2.3.2).
#[test]
fn get_cursor_speaks_for_the_upper_window() {
    let mut screen = small(5);

    screen.split_window(2).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.set_cursor(2, 7).unwrap();
    screen.set_window(LOWER).unwrap();
    screen.write("elsewhere");

    assert_eq!(screen.get_cursor(), (2, 7));
}

// Printing on the bottom right of the upper window is legal, the
// cursor staying put as §8.7.3.1's author suggests; a newline at
// the window's bottom line has nowhere further to go.
#[test]
fn the_upper_window_never_scrolls() {
    let mut screen = small(5);

    screen.split_window(2).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.set_cursor(2, WIDTH as u16 - 1).unwrap();
    screen.write("abc\nz");

    let row = screen.row_text(2);

    assert_eq!(&row[row.len() - 2..], "ac");
    assert_eq!(&row[..1], "z");
}

// Erasing window -1 unsplits the screen, clears the lot, selects
// the lower window, and homes its cursor by version: bottom left
// in Version 4, top left from Version 5 (§8.7.3.3).
#[test]
fn erase_minus_one_resets_the_screen() {
    let mut late = small(5);

    late.split_window(2).unwrap();
    late.set_window(UPPER).unwrap();
    late.write("gone");
    late.erase_window(-1).unwrap();
    late.write("fresh");

    assert_eq!(late.split(), 0);
    assert_eq!(late.selected(), LOWER);
    assert_eq!(late.row_text(1), "fresh");

    let mut middle = small(4);

    middle.erase_window(-1).unwrap();
    middle.write("low");

    assert_eq!(middle.row_text(HEIGHT), "low");
}

// Erasing window -2 clears the screen but keeps the split and the
// cursors exactly as they were (§15 erase_window).
#[test]
fn erase_minus_two_keeps_the_split() {
    let mut screen = small(5);

    screen.split_window(2).unwrap();
    screen.write("about to vanish");
    screen.erase_window(-2).unwrap();

    assert_eq!(screen.split(), 2);
    assert_eq!(screen.rendered().trim(), "");
}

// Erasing a single window blanks only its own rows; from Version
// 5 the erased window's cursor homes to its top left
// (§8.7.3.2.1).
#[test]
fn erasing_one_window_spares_the_other() {
    let mut screen = small(5);

    screen.split_window(1).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.write("KEPT");
    screen.set_window(LOWER).unwrap();
    screen.write("dust");
    screen.erase_window(i32::from(LOWER)).unwrap();
    screen.write("swept");

    assert_eq!(screen.row_text(1), "KEPT");
    assert_eq!(screen.row_text(2), "swept");

    screen.erase_window(i32::from(UPPER)).unwrap();

    assert_eq!(screen.row_text(1), "");
    assert_eq!(screen.row_text(2), "swept");
}

// In Version 4 erasing the lower window homes its cursor to the
// bottom left, where its cursor always lives (§8.7.3.2.1).
#[test]
fn version_4_lower_erasure_homes_to_the_bottom() {
    let mut screen = small(4);

    screen.write("one\ntwo");
    screen.erase_window(i32::from(LOWER)).unwrap();
    screen.write("floor");

    assert_eq!(screen.row_text(HEIGHT), "floor");
}

// Only real windows can be erased (§15 erase_window).
#[test]
fn unknown_windows_cannot_be_erased() {
    let mut screen = small(5);

    refused(screen.erase_window(9), "erase_window");
}

// erase_line clears from the cursor to the right edge in the
// selected window (§8.7.3.4).
#[test]
fn erase_line_clears_to_the_right_edge() {
    let mut screen = small(5);

    screen.split_window(1).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.write("wiped almost all");
    screen.set_cursor(1, 6).unwrap();
    screen.erase_line();

    assert_eq!(screen.row_text(1), "wiped");

    screen.set_window(LOWER).unwrap();
    screen.write("keep tail");
    screen.erase_line();

    assert_eq!(screen.row_text(2), "keep tail");
}

// Erased blanks wear the current background without reverse
// video, even while the text style is Reverse (§8.7.3.2).
#[test]
fn erasure_is_never_reversed() {
    let mut screen = small(5);

    screen.set_style(REVERSE);
    screen.set_colour(2, 4);
    screen.write("vivid");
    screen.erase_window(i32::from(LOWER)).unwrap();

    let blank = screen.cell(1, 1);

    assert_eq!(blank.style, ROMAN);
    assert_eq!(blank.background, 4);
}

// The Version 3 status line: location on the left, score and
// turns on the right, the whole row in reverse video (§8.2).
#[test]
fn the_status_line_shows_score_and_moves() {
    let mut screen = ScreenModel::new(40, HEIGHT, 3);

    screen
        .show_status(&Status {
            location: "West of House".to_string(),
            score: 35,
            turns: 110,
            time_game: false,
        })
        .unwrap();

    assert!(screen.row_text(1).contains("West of House"));
    assert!(screen.row_text(1).contains("Score: 35  Moves: 110"));
    assert_eq!(screen.cell(1, 2).style, REVERSE);
}

// A time game's status line shows an hours:minutes clock instead
// (§8.2.3.2).
#[test]
fn the_status_line_tells_the_time() {
    let mut screen = ScreenModel::new(40, HEIGHT, 3);

    screen
        .show_status(&Status {
            location: "Bedroom".to_string(),
            score: 2,
            turns: 7,
            time_game: true,
        })
        .unwrap();

    assert!(screen.row_text(1).contains("Time: 2:07"));
}

// A location too long for its room breaks with an ellipsis, as
// §8.2.2.2's author suggests.
#[test]
fn a_long_location_gains_an_ellipsis() {
    let mut screen = small(3);

    screen
        .show_status(&Status {
            location: "The Halls of the Dead King".to_string(),
            score: 0,
            turns: 0,
            time_game: false,
        })
        .unwrap();

    assert!(screen.row_text(1).contains("..."));
}

// From Version 4 the game paints its own status area and the
// interpreter's line is over (§8.2).
#[test]
fn later_versions_have_no_interpreter_status_line() {
    let mut screen = small(4);

    refused(
        screen.show_status(&Status {
            location: "Anywhere".to_string(),
            score: 0,
            turns: 0,
            time_game: false,
        }),
        "§8.2",
    );
}

// The cursor property speaks screen coordinates for whichever
// window is selected.
#[test]
fn the_cursor_property_follows_the_selection() {
    let mut screen = small(5);

    screen.split_window(2).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.set_cursor(2, 4).unwrap();

    assert_eq!(screen.cursor(), (2, 4));

    screen.set_window(LOWER).unwrap();
    screen.write("abc");

    assert_eq!(screen.cursor(), (3, 4));
}

// Inspection flushes pending buffered text, so the grid always
// shows what a player would see.
#[test]
fn inspection_flushes_the_pending_word() {
    let mut screen = small(5);

    screen.write("half");

    assert_eq!(screen.row_text(1), "half");
}

// A golden grid: a miniature licence-form screen assembled from
// splits, cursor moves, and overlays -- the certification style
// the Bureaucracy recording taught us to want.
#[test]
fn a_form_renders_as_a_golden_grid() {
    let mut screen = ScreenModel::new(24, 8, 4);

    screen.split_window(4).unwrap();
    screen.set_window(UPPER).unwrap();
    screen.set_cursor(1, 5).unwrap();
    screen.write("LICENCE FORM");
    screen.set_cursor(3, 1).unwrap();
    screen.write("Name:");
    screen.set_cursor(3, 7).unwrap();
    screen.write("NYMAN");
    screen.set_window(LOWER).unwrap();
    screen.write("Thank you, Ms Nyman.");

    let expected = [
        "    LICENCE FORM",
        "",
        "Name: NYMAN",
        "",
        "",
        "",
        "",
        "Thank you, Ms Nyman.",
    ]
    .join("\n");

    assert_eq!(screen.rendered(), expected);
}

// The damage sweep names exactly the touched rows, in order, and
// clears its own slate.
#[test]
fn the_sweep_names_the_damaged_rows() {
    let mut screen = small(5);

    screen.write("a");

    assert_eq!(screen.sweep(), vec![1]);
    assert!(screen.sweep().is_empty());
}
