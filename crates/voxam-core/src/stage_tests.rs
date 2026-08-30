//! The stage battery, mirroring the reference's `test_stage.py`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::frontend::Status;
use crate::screen::{BLANK, BOLD, Cell, REVERSE, ROMAN};
use crate::stage::{FillPaint, Paint, ShiftPaint, StageModel, TextPaint};

/// The pauses a test's [MORE] callback collects.
type Pauses = Rc<RefCell<Vec<(i32, i32, i32, i32)>>>;

/// A readable geometry: 20 by 10 cells of 10-by-10-unit type, so a
/// screen of 200 by 100 units and every position a round number.
fn staged() -> StageModel {
    StageModel::new(20, 10, 10, 10)
}

fn lettered(character: char) -> Cell {
    Cell { character, ..BLANK }
}

// The §8.8.3.3 boot stage: window 0 fills the screen, wraps whole
// words at its right edge, and scrolls its own text.
#[test]
fn window_0_boots_full_wrapping_and_scrolling() {
    let mut stage = staged();

    assert_eq!(stage.selected(), 0);

    stage.write("a stretch of words that wraps at the twentieth column");

    assert_eq!(stage.row_text(1), "a stretch of words");
    assert_eq!(stage.row_text(2), "that wraps at the");
    assert_eq!(stage.row_text(3), "twentieth column");
}

// Text past the bottom scrolls window 0's rectangle: the scroll is
// owed at the last line and paid when the next text arrives, so
// the final line stays at the window's foot.
#[test]
fn window_0_scrolls_at_its_bottom() {
    let mut stage = staged();
    let lines: Vec<String> = (1..12).map(|number| number.to_string()).collect();

    stage.write(&lines.join("\n"));

    assert_eq!(stage.row_text(1), "2");
    assert_eq!(stage.row_text(10), "11");

    stage.write("\n12");

    assert_eq!(stage.row_text(1), "3");
    assert_eq!(stage.row_text(10), "12");

    // Consecutive blank lines at the bottom each earn their own
    // scroll: an owed one is paid before the next is owed.
    stage.write("\n\n13");

    assert_eq!(stage.row_text(10), "13");
    assert_eq!(stage.row_text(9), "");
    assert_eq!(stage.row_text(8), "12");
}

// A placed window takes its text at its own position: the cursor
// is window-relative units, the printing lands on the grid, and
// get_cursor answers in the same units printing advanced it to.
#[test]
fn placed_windows_take_text_at_their_position() {
    let mut stage = staged();

    stage.place_window(3, 21, 51, 30, 80).expect("places");
    stage.set_window(3).expect("selects");
    stage.set_cursor(11, 21);
    stage.write("boxed");

    assert_eq!(stage.row_text(4), "       boxed");
    assert_eq!(stage.get_cursor(), (11, 71));
    assert_eq!(stage.cell(4, 8).character, 'b');
}

// §8.3.1's "pixel under the cursor" needs the cursor as a screen
// position: screen_cursor folds the window's own origin in, where
// get_cursor stays window-relative (§8.7.2.3.2).
#[test]
fn the_screen_cursor_speaks_absolute_units() {
    let mut stage = staged();

    stage.place_window(3, 21, 51, 30, 80).expect("places");
    stage.set_window(3).expect("selects");
    stage.set_cursor(11, 21);

    assert_eq!(stage.get_cursor(), (11, 21));
    assert_eq!(stage.screen_cursor(), (31, 71));
}

// A window with wrapping off prints to its right margin, parks the
// cursor there, and ignores the rest (§8.8.3.1.1); a newline in a
// non-scrolling window pins at its bottom line.
#[test]
fn unwrapped_windows_pin_at_their_margin() {
    let mut stage = staged();

    stage.place_window(2, 11, 11, 20, 50).expect("places");
    stage.set_window(2).expect("selects");
    stage.write("overflowing text");
    stage.write(" more");

    assert_eq!(stage.row_text(2), " overf");

    stage.write("\n\ndown");

    assert_eq!(stage.row_text(3), " down");
}

// With buffering off, wrapping breaks after the last character
// that fits (§8.8.3.1.2.2), and a space at the margin becomes the
// line break itself.
#[test]
fn unbuffered_wrapping_breaks_by_character() {
    let mut stage = staged();

    stage.set_buffering(false);
    stage.write("abcdefghijklmnopqrstuvwx");

    assert_eq!(stage.row_text(1), "abcdefghijklmnopqrst");
    assert_eq!(stage.row_text(2), "uvwx");

    stage.set_buffering(true);
    stage.write(&format!("yz{}end word", " ".repeat(17)));

    assert_eq!(stage.row_text(2), "uvwxyz");
    assert_eq!(stage.row_text(3), "  end word");
}

// A word too long for any line simply character-wraps: there is no
// whole line it could have waited for.
#[test]
fn giant_words_wrap_by_character() {
    let mut stage = staged();

    stage.write(&format!("{} tail", "a".repeat(25)));

    assert_eq!(stage.row_text(1), "a".repeat(20));
    assert_eq!(stage.row_text(2), "aaaaa tail");
}

// split_window tiles windows 1 and 0 vertically in units
// (§8.8.4.1): window 1 takes the top, window 0 the rest, and each
// cursor keeps its absolute screen position -- homing only when
// that position falls outside its window.
#[test]
fn split_tiles_the_two_windows() {
    let mut stage = staged();

    stage.write("\n\n\nfour");
    stage.split_window(20);
    stage.write(" deep");

    assert_eq!(stage.row_text(4), "four deep");

    stage.set_window(1).expect("selects");
    stage.write("top");

    assert_eq!(stage.row_text(1), "top");

    stage.set_window(0).expect("selects");
    stage.set_cursor(1, 1);
    stage.write("below");

    assert_eq!(stage.row_text(3), "below");

    // A split to the full screen leaves window 0 no rows at all:
    // its cursor falls outside and homes, and its text goes
    // nowhere, quietly.
    stage.split_window(100);
    stage.set_window(0).expect("selects");
    stage.write("homed");

    assert_eq!(stage.get_cursor(), (1, 1));
}

// erase_window fills a window's own rectangle with its background,
// homes its cursor, and answers the cell rectangle it touched; -2
// erases the whole screen and moves nothing.
#[test]
fn erasures_fill_their_rectangles() {
    let mut stage = staged();

    stage.write("story text everywhere");
    stage.place_window(4, 11, 11, 20, 40).expect("places");
    stage.set_window(4).expect("selects");
    stage.write("gone");

    let rectangle = stage.erase_window(4).expect("erases");

    // Only the window's own rectangle blanks: "everywhere" on row
    // 2 loses exactly the cells the window covered.
    assert_eq!(rectangle, (2, 2, 2, 4));
    assert_eq!(stage.row_text(2), "e    where");
    assert_eq!(stage.row_text(1), "story text");
    assert_eq!(stage.get_cursor(), (1, 1));

    assert_eq!(stage.erase_window(-2).expect("erases"), (1, 1, 10, 20));
    assert_eq!(stage.row_text(1), "");
    assert_eq!(stage.selected(), 4);
    assert!(
        stage
            .erase_window(9)
            .expect_err("refused")
            .to_string()
            .contains("not one of the eight")
    );
}

// Erasing -1 clears the whole screen to window 0's background,
// re-tiles a split back to nothing (§8.8.4.2), and selects window
// 0 with its cursor homed (§8.8.5.3.1).
#[test]
fn erasing_minus_one_unsplits_and_selects_zero() {
    let mut stage = staged();

    stage.split_window(30);
    stage.set_window(1).expect("selects");
    stage.write("chrome");

    assert_eq!(stage.erase_window(-1).expect("erases"), (1, 1, 10, 20));
    assert_eq!(stage.selected(), 0);
    assert_eq!(stage.rendered().trim(), "");

    stage.write("fresh");

    assert_eq!(stage.row_text(1), "fresh");
}

// erase_line blanks from the cursor to the window's right edge; a
// cursor already past the window's rows erases nothing.
#[test]
fn erase_line_stops_at_the_window_edge() {
    let mut stage = staged();

    stage.place_window(5, 11, 11, 20, 60).expect("places");
    stage.set_window(5).expect("selects");
    stage.write("wiped!");
    stage.set_cursor(1, 31);
    stage.erase_line(None);

    assert_eq!(stage.row_text(2), " wip");

    stage.set_cursor(31, 1);
    stage.erase_line(None);

    assert_eq!(stage.row_text(2), " wip");

    // A cursor already at the right margin has nothing to erase.
    stage.set_cursor(1, 61);
    stage.erase_line(None);

    assert_eq!(stage.row_text(2), " wip");
}

// The pixel form erases an exact width rightward (§8.8.5.2): the
// fill carries the width, the grid blanks only the cells the span
// fully covers, and an over-long reach clips at the right margin.
#[test]
fn erase_line_takes_a_pixel_width() {
    let mut stage = staged();

    stage.place_window(5, 11, 11, 20, 60).expect("places");
    stage.set_window(5).expect("selects");
    stage.write("wiped!");
    stage.set_cursor(1, 11);
    stage.paints();
    stage.erase_line(Some(25));

    assert_eq!(stage.row_text(2), " w  ed!");
    assert_eq!(
        stage.paints(),
        vec![Paint::Fill(FillPaint {
            line: 11,
            column: 21,
            height: 10,
            width: 25,
            background: 1,
        })]
    );

    stage.set_cursor(1, 41);
    stage.erase_line(Some(9999));

    assert_eq!(stage.row_text(2), " w  e");
    assert_eq!(
        stage.paints(),
        vec![Paint::Fill(FillPaint {
            line: 11,
            column: 51,
            height: 10,
            width: 20,
            background: 1,
        })]
    );
}

// rub_out retreats one cell and blanks it, and at the window's
// left edge there is nothing left to rub.
#[test]
fn rub_out_retreats_one_cell() {
    let mut stage = staged();

    stage.write("hi");
    stage.rub_out();

    assert_eq!(stage.row_text(1), "h");
    assert_eq!(stage.get_cursor(), (1, 11));

    stage.rub_out();
    stage.rub_out();

    assert_eq!(stage.row_text(1), "");
    assert_eq!(stage.get_cursor(), (1, 1));
}

// The line editor's cursor motion retreats without erasing: the
// text stays painted, the motion clamps at the window's left edge,
// and the cells actually moved come back (§15 read).
#[test]
fn retreat_moves_the_cursor_without_erasing() {
    let mut stage = staged();

    stage.write("hi");

    assert_eq!(stage.retreat(1), 1);
    assert_eq!(stage.row_text(1), "hi");
    assert_eq!(stage.get_cursor(), (1, 11));
    assert_eq!(stage.retreat(5), 1);
    assert_eq!(stage.get_cursor(), (1, 1));
}

// A §15 rectangle prints right and down from the cursor without
// wrapping, each row at the starting column, pressing onto the
// window's bottom line when too tall.
#[test]
fn rectangles_stamp_down_from_the_cursor() {
    let mut stage = staged();

    stage.place_window(6, 21, 31, 30, 50).expect("places");
    stage.set_window(6).expect("selects");
    stage.set_cursor(1, 11);
    stage.write_rectangle(&["ab", "cd", "ef", "gh"]);

    assert_eq!(stage.row_text(3), "    ab");
    assert_eq!(stage.row_text(4), "    cd");
    assert_eq!(stage.row_text(5), "    gh");
}

// Style, colour, and font dress each window separately
// (§8.8.3.2.3): a selection change swaps the whole dress, roman
// clears the styles, and the background answers for the selected
// window.
#[test]
fn each_window_wears_its_own_dress() {
    let mut stage = staged();

    stage.set_style(REVERSE);
    stage.set_style(BOLD);
    stage.set_colour(3, 4);
    stage.set_font(3);
    stage.write("a");

    let dressed = stage.cell(1, 1);

    assert_eq!(dressed.style, REVERSE | BOLD);
    assert_eq!(dressed.foreground, 3);
    assert_eq!(dressed.font, 3);
    assert_eq!(stage.background(), 4);

    stage.place_window(1, 1, 1, 10, 200).expect("places");
    stage.set_window(1).expect("selects");

    assert_eq!(stage.background(), 1);

    stage.set_style(ROMAN);
    stage.write("b");

    assert_eq!(stage.cell(1, 1).style, ROMAN);

    stage.set_window(0).expect("selects");
    stage.set_style(ROMAN);
    stage.set_colour(0, 0);

    assert_eq!(stage.background(), 4);
}

// A window that was never placed has no cells: its text goes
// nowhere, quietly, and a window hanging past the screen edge
// clips instead of crashing.
#[test]
fn sizeless_and_overhanging_windows_clip() {
    let mut stage = staged();

    stage.set_window(7).expect("selects");
    stage.write("nowhere\n");
    stage.write_rectangle(&["x"]);

    assert_eq!(stage.rendered().trim(), "");

    stage.place_window(7, 91, 191, 40, 40).expect("places");
    stage.set_window(7).expect("selects");
    stage.write("edge");

    assert_eq!(stage.row_text(10), format!("{}e", " ".repeat(19)));
}

// The stage refuses a §8.2 status line -- a Version 6 game draws
// its own -- and polices window numbers loudly.
#[test]
fn the_stage_refuses_status_and_strange_windows() {
    let mut stage = staged();
    let status = Status {
        location: "Nowhere".to_string(),
        score: 0,
        turns: 0,
        time_game: false,
    };

    assert!(
        stage
            .show_status(&status)
            .expect_err("refused")
            .to_string()
            .contains("draws its own status")
    );
    assert!(
        stage
            .set_window(8)
            .expect_err("refused")
            .to_string()
            .contains("not one of the eight")
    );
}

// Margins bound the wrapping text (§8.8.3.2.1): a newline returns
// to the left margin, words wrap at the right margin -- here 30
// and 50 units leave text columns 4 to 15 -- and erase_line
// reaches only to the right margin (§8.8.5.2).
#[test]
fn margins_bound_the_wrapping_text() {
    let mut stage = staged();

    stage.set_margins(0, 30, 50).expect("sets");
    stage.write("\nabc def ghi");

    assert_eq!(stage.row_text(2), "   abc def ghi");

    stage.write(" jklmn");

    assert_eq!(stage.row_text(3), "   jklmn");

    stage.set_cursor(11, 111);
    stage.erase_line(None);

    assert_eq!(stage.row_text(2), "   abc def");

    // Loosening the margins around a cursor already inside them
    // moves nothing (§8.8.3.2.2.2).
    stage.set_margins(0, 20, 20).expect("sets");

    assert_eq!(stage.get_cursor(), (11, 111));
}

// Changing margins nudges a cursor they would strand to the left
// margin (§8.8.3.2.2.2); margins that leave no room at all swallow
// the text quietly.
#[test]
fn margins_nudge_a_stranded_cursor() {
    let mut stage = staged();

    stage.write("edge");
    stage.set_margins(0, 60, 60).expect("sets");

    assert_eq!(stage.get_cursor(), (1, 61));

    stage.set_margins(0, 110, 110).expect("sets");
    stage.write("gone");

    assert_eq!(stage.row_text(1), "edge");
}

// scroll_window shifts a window's own rectangle by whole cell
// rows: positive up, negative down, exposed rows blanked, and a
// fraction of a cell row scrolls nothing (§8.8.3.6).
#[test]
fn scroll_window_shifts_the_rectangle() {
    let mut stage = staged();

    stage.write("one\ntwo\nthree");
    stage.scroll_window(0, 10).expect("scrolls");

    assert_eq!(stage.row_text(1), "two");
    assert_eq!(stage.row_text(2), "three");
    assert_eq!(stage.row_text(3), "");

    stage.scroll_window(0, -10).expect("scrolls");

    assert_eq!(stage.row_text(1), "");
    assert_eq!(stage.row_text(2), "two");

    stage.scroll_window(0, 5).expect("scrolls");

    assert_eq!(stage.row_text(2), "two");
}

// The stage narrates its painting in units, for a glass whose
// pixels are the retained screen: text lands at the window's true
// position, erasures fill true rectangles, scrolls shift them with
// the exposed strip filled, and draining clears the slate.
#[test]
fn paints_narrate_in_units() {
    let mut stage = staged();

    stage.place_window(3, 21, 35, 30, 80).expect("places");
    stage.set_window(3).expect("selects");
    stage.write("ab");

    let paints = stage.paints();

    assert_eq!(
        paints[0],
        Paint::Text(TextPaint {
            line: 21,
            column: 35,
            cell: lettered('a'),
        })
    );
    assert_eq!(
        paints[1],
        Paint::Text(TextPaint {
            line: 21,
            column: 45,
            cell: lettered('b'),
        })
    );
    assert!(stage.paints().is_empty());

    stage.erase_window(3).expect("erases");

    assert_eq!(
        stage.paints(),
        vec![Paint::Fill(FillPaint {
            line: 21,
            column: 35,
            height: 30,
            width: 80,
            background: 1,
        })]
    );

    stage.scroll_window(3, 10).expect("scrolls");

    assert_eq!(
        stage.paints(),
        vec![
            Paint::Shift(ShiftPaint {
                line: 21,
                column: 35,
                height: 30,
                width: 80,
                rise: 10,
            }),
            Paint::Fill(FillPaint {
                line: 41,
                column: 35,
                height: 10,
                width: 80,
                background: 1,
            }),
        ]
    );

    stage.scroll_window(3, -10).expect("scrolls");

    assert_eq!(
        stage.paints(),
        vec![
            Paint::Shift(ShiftPaint {
                line: 21,
                column: 35,
                height: 30,
                width: 80,
                rise: -10,
            }),
            Paint::Fill(FillPaint {
                line: 21,
                column: 35,
                height: 10,
                width: 80,
                background: 1,
            }),
        ]
    );

    stage.set_cursor(1, 21);
    stage.erase_line(None);

    assert_eq!(
        stage.paints(),
        vec![Paint::Fill(FillPaint {
            line: 21,
            column: 55,
            height: 10,
            width: 60,
            background: 1,
        })]
    );

    stage.write("x");
    stage.paints();
    stage.rub_out();

    assert_eq!(
        stage.paints(),
        vec![Paint::Fill(FillPaint {
            line: 21,
            column: 55,
            height: 10,
            width: 10,
            background: 1,
        })]
    );

    stage.erase_window(-2).expect("erases");

    assert_eq!(
        stage.paints(),
        vec![Paint::Fill(FillPaint {
            line: 1,
            column: 1,
            height: 100,
            width: 200,
            background: 1,
        })]
    );
}

// A scrolling window that feeds a screenful of lines since the
// player's last rest asks for a [MORE] pause at its bottom line;
// resting refills the budget, -999 never pauses, and windows that
// do not scroll never count (§8.8.3.2.6).
#[test]
fn a_screenful_earns_the_more_pause() {
    let mut stage = staged();
    let pauses: Pauses = Rc::new(RefCell::new(Vec::new()));
    let held = pauses.clone();

    stage.more = Some(Box::new(move |line, column, ink, paper| {
        held.borrow_mut().push((line, column, ink, paper));
    }));

    stage.set_colour(3, 4);

    let lines: Vec<String> = (1..11).map(|number| number.to_string()).collect();

    stage.write(&lines.join("\n"));

    // The pause carries the window's own colour codes, so the
    // frontend can dress the prompt and its erasure correctly.
    assert_eq!(*pauses.borrow(), vec![(91, 1, 3, 4)]);

    stage.rest();
    stage.write(&"\n".repeat(9));

    assert_eq!(pauses.borrow().len(), 2);

    stage.set_line_count(0, -999).expect("sets");
    stage.rest();
    stage.write(&"\n".repeat(30));

    assert_eq!(pauses.borrow().len(), 2);

    stage.set_line_count(0, 8).expect("sets");
    stage.write("\n");

    assert_eq!(pauses.borrow().len(), 3);

    stage.place_window(2, 11, 11, 40, 60).expect("places");
    stage.set_window(2).expect("selects");
    stage.write(&"\n".repeat(30));

    assert_eq!(pauses.borrow().len(), 3);
}

// An erase refills the [MORE] budget: erased text cannot be
// unread. Shogun feeds its credits into a tall window 0, then
// shrinks and erases it for the title menu -- a stale count would
// pause the menu on its very first line (§8.8.3.2.6). An explicit
// never-pause survives an erase, and the full-screen erases refill
// every window's budget.
#[test]
fn an_erase_refills_the_more_budget() {
    let mut stage = staged();
    let pauses: Pauses = Rc::new(RefCell::new(Vec::new()));
    let held = pauses.clone();

    stage.more = Some(Box::new(move |line, column, ink, paper| {
        held.borrow_mut().push((line, column, ink, paper));
    }));

    let lines: Vec<String> = (1..8).map(|number| number.to_string()).collect();

    stage.write(&lines.join("\n"));
    stage.place_window(0, 61, 1, 40, 200).expect("places");
    stage.erase_window(0).expect("erases");
    stage.write("menu\n");

    assert!(pauses.borrow().is_empty());

    stage.set_line_count(0, -999).expect("sets");
    stage.erase_window(0).expect("erases");
    stage.write(&"\n".repeat(30));

    assert!(pauses.borrow().is_empty());

    stage.set_line_count(0, 0).expect("sets");
    stage.write("\n\n");
    stage.erase_window(-1).expect("erases");
    stage.write("\n\n");
    stage.erase_window(-2).expect("erases");
    stage.write("\n\n");

    assert!(pauses.borrow().is_empty());
}

// The damage sweep names changed rows once and clears its slate;
// a cursor sent below units 1 clamps to the window's origin; the
// stage reports its own cell dimensions.
#[test]
fn sweeps_and_cursor_clamps() {
    let mut stage = staged();

    stage.write("row one\nrow two");

    assert_eq!(stage.sweep(), vec![1, 2]);
    assert!(stage.sweep().is_empty());

    stage.set_cursor(0, 0);

    assert_eq!(stage.get_cursor(), (1, 1));
    assert_eq!(stage.columns(), 20);
    assert_eq!(stage.lines(), 10);
}

// Erasing -1 on a stage never split leaves the tiling alone: there
// is nothing to unsplit (§8.8.4.2).
#[test]
fn erasing_minus_one_without_a_split_retiles_nothing() {
    let mut stage = staged();

    stage.write("words");

    assert_eq!(stage.erase_window(-1).expect("erases"), (1, 1, 10, 20));
    assert_eq!(stage.rendered().trim(), "");
}

// A window's own text-flow scroll sweeps only the region between
// its margins: the margins' art stays anchored, which is how
// Shogun keeps its ship beside fifty scrolled lines while §15's
// explicit scroll_window still takes the whole rectangle
// (§8.8.3.2.1).
#[test]
fn flow_scrolls_leave_the_margins_anchored() {
    let mut stage = staged();

    stage.set_margins(0, 20, 60).expect("sets");

    let lines: Vec<String> = (1..12).map(|number| number.to_string()).collect();

    stage.write(&lines.join("\n"));
    stage.paints();
    stage.write("\n12");

    let paints = stage.paints();
    let shifts: Vec<&Paint> = paints
        .iter()
        .filter(|paint| matches!(paint, Paint::Shift(_)))
        .collect();
    let exposed: Vec<&Paint> = paints
        .iter()
        .filter(|paint| matches!(paint, Paint::Fill(_)))
        .collect();

    assert_eq!(
        shifts,
        vec![&Paint::Shift(ShiftPaint {
            line: 1,
            column: 21,
            height: 100,
            width: 120,
            rise: 10,
        })]
    );
    assert!(exposed.contains(&&Paint::Fill(FillPaint {
        line: 91,
        column: 21,
        height: 10,
        width: 120,
        background: 1,
    })));
}
