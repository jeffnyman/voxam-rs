//! The ported half of the stage sweep: the same ten drills as
//! `certify/stage_reference.py`, printed in the same canonical
//! spelling, to be diffed line for line.

use std::cell::RefCell;
use std::rc::Rc;

use voxam_core::frontend::Status;
use voxam_core::screen::{BOLD, REVERSE};
use voxam_core::stage::{Paint, StageModel};

type Pauses = Rc<RefCell<Vec<(i32, i32, i32, i32)>>>;

fn staged() -> StageModel {
    StageModel::new(20, 10, 10, 10)
}

fn spelled(paint: &Paint) -> String {
    match paint {
        Paint::Text(held) => {
            let cell = held.cell;

            format!(
                "text {} {} {} {} {} {} {}",
                held.line,
                held.column,
                cell.character,
                cell.style,
                cell.foreground,
                cell.background,
                cell.font
            )
        }
        Paint::Fill(held) => format!(
            "fill {} {} {} {} {}",
            held.line, held.column, held.height, held.width, held.background
        ),
        Paint::Shift(held) => format!(
            "shift {} {} {} {} {}",
            held.line, held.column, held.height, held.width, held.rise
        ),
    }
}

fn told(stage: &mut StageModel, pauses: &Pauses) {
    for row in 1..=stage.lines() {
        println!("|{}|", stage.row_text(row));
    }

    let (line, column) = stage.get_cursor();
    let (screen_line, screen_column) = stage.screen_cursor();

    println!(
        "cursor {line} {column} screen {screen_line} {screen_column} selected {} ink {} paper {}",
        stage.selected(),
        stage.foreground(),
        stage.background()
    );

    let swept: Vec<String> = stage.sweep().iter().map(usize::to_string).collect();

    if swept.is_empty() {
        println!("sweep");
    } else {
        println!("sweep {}", swept.join(" "));
    }

    for paint in stage.paints() {
        println!("{}", spelled(&paint));
    }

    for (line, column, ink, paper) in pauses.borrow().iter() {
        println!("more {line} {column} {ink} {paper}");
    }

    pauses.borrow_mut().clear();
}

fn hung(stage: &mut StageModel) -> Pauses {
    let pauses: Pauses = Rc::new(RefCell::new(Vec::new()));
    let held = pauses.clone();

    stage.more = Some(Box::new(move |line, column, ink, paper| {
        held.borrow_mut().push((line, column, ink, paper));
    }));

    pauses
}

fn quiet() -> Pauses {
    Rc::new(RefCell::new(Vec::new()))
}

fn counted(first: i32, last: i32) -> String {
    (first..last)
        .map(|number| number.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(clippy::too_many_lines)] // one drill per stanza, as the oracle spells them
fn main() {
    println!("drill boot-wrap");

    let mut stage = staged();

    stage.write("a stretch of words that wraps at the twentieth column");
    told(&mut stage, &quiet());

    println!("drill scroll-at-bottom");

    let mut stage = staged();

    stage.write(&counted(1, 14));
    stage.write("\n\n14");
    told(&mut stage, &quiet());

    println!("drill dressed-window");

    let mut stage = staged();

    stage.place_window(3, 21, 51, 30, 80).expect("places");
    stage.set_window(3).expect("selects");
    stage.set_style(REVERSE);
    stage.set_style(BOLD);
    stage.set_colour(3, 4);
    stage.set_font(4);
    stage.set_cursor(11, 21);
    stage.write("boxed words that run long enough to wrap twice inside");
    told(&mut stage, &quiet());

    println!("drill split-dance");

    let mut stage = staged();

    stage.write("\n\n\nfour");
    stage.split_window(20);
    stage.write(" deep");
    stage.set_window(1).expect("selects");
    stage.write("top of the strip");
    stage.set_window(0).expect("selects");
    stage.set_cursor(1, 1);
    stage.write("below");
    stage.split_window(45);
    stage.write("x");
    stage.split_window(100);
    stage.write("homed");
    told(&mut stage, &quiet());

    println!("drill margins");

    let mut stage = staged();

    stage.set_margins(0, 30, 50).expect("sets");
    stage.write(&counted(1, 12));
    stage.write("\n12");
    stage.erase_line(Some(25));
    stage.set_cursor(11, 111);
    stage.erase_line(None);
    stage.set_margins(0, 110, 110).expect("sets");
    stage.write("gone");
    told(&mut stage, &quiet());

    println!("drill erases");

    let mut stage = staged();

    stage.write("story text everywhere");
    stage.place_window(4, 11, 11, 20, 40).expect("places");
    stage.set_window(4).expect("selects");
    stage.write("gone");

    let (a, b, c, d) = stage.erase_window(4).expect("erases");

    println!("erased {a} {b} {c} {d}");

    let (a, b, c, d) = stage.erase_window(-2).expect("erases");

    println!("erased {a} {b} {c} {d}");
    stage.split_window(30);

    let (a, b, c, d) = stage.erase_window(-1).expect("erases");

    println!("erased {a} {b} {c} {d}");
    told(&mut stage, &quiet());

    println!("drill scroll-window");

    let mut stage = staged();

    stage.write("one\ntwo\nthree");
    stage.scroll_window(0, 10).expect("scrolls");
    stage.scroll_window(0, -10).expect("scrolls");
    stage.scroll_window(0, 25).expect("scrolls");
    told(&mut stage, &quiet());

    println!("drill editing");

    let mut stage = staged();

    stage.set_buffering(false);
    stage.write("abcdefghijklmnopqrstuvwx");
    stage.set_buffering(true);
    stage.write(&format!("yz{}end word", " ".repeat(17)));
    stage.rub_out();
    println!("retreated {}", stage.retreat(3));
    stage.set_cursor(1, 171);
    stage.write_rectangle(&["ab", "cd", "ef", "gh"]);
    told(&mut stage, &quiet());

    println!("drill more-budget");

    let mut stage = staged();
    let pauses = hung(&mut stage);

    stage.set_colour(3, 4);
    stage.write(&counted(1, 11));
    stage.rest();
    stage.write(&"\n".repeat(9));
    stage.set_line_count(0, -999).expect("sets");
    stage.write(&"\n".repeat(30));
    stage.set_line_count(0, 8).expect("sets");
    stage.write("\n");
    stage.place_window(0, 61, 1, 40, 200).expect("places");
    stage.erase_window(0).expect("erases");
    stage.write("menu\n");
    told(&mut stage, &pauses);

    println!("drill odd-metrics");

    let mut stage = StageModel::new(17, 7, 7, 9);

    stage.place_window(5, 8, 13, 40, 50).expect("places");
    stage.set_window(5).expect("selects");
    stage.set_cursor(5, 11);
    stage.write("odd metrics land where");
    stage.place_window(7, 60, 110, 90, 90).expect("places");
    stage.set_window(7).expect("selects");
    stage.write("edge overhang test");
    stage.erase_line(Some(23));
    told(&mut stage, &quiet());

    println!("drill refusals");

    let mut stage = staged();
    let status = Status {
        location: "Nowhere".to_string(),
        score: 0,
        turns: 0,
        time_game: false,
    };

    if let Err(error) = stage.erase_window(9) {
        println!("refused: {error}");
    }

    if let Err(error) = stage.set_window(8) {
        println!("refused: {error}");
    }

    if let Err(error) = stage.show_status(&status) {
        println!("refused: {error}");
    }
}
