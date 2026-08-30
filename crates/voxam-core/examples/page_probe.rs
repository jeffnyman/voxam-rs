//! The Page oracle's four drills, for a one-shot diff against the
//! reference's own stanza spellings.

use voxam_core::glkote::json::{Object, Value, dumps};
use voxam_core::glkote::{LineSpec, Page, Run, TextRun, WindowSpec};

const BOX: (i64, i64, i64, i64) = (0, 0, 640, 400);
const TOP: (i64, i64, i64, i64) = (0, 0, 640, 30);

fn told(stanza: &Object) {
    println!("{}", dumps(&Value::Object(stanza.clone())));
}

fn main() {
    let mut page = Page::new();

    page.window(1, "buffer", 0, BOX, WindowSpec::default())
        .unwrap();
    told(&page.update(false, false, None).unwrap());

    page.window(1, "buffer", 0, BOX, WindowSpec::default())
        .unwrap();
    page.buffer(
        1,
        &[
            Run::text("normal", 3, "l\u{e5}nk\n"),
            Run::Flowbreak,
            Run::text("header", 0, "below"),
        ],
        false,
    )
    .unwrap();
    page.line_input(
        1,
        80,
        LineSpec {
            initial: "go".to_string(),
            terminators: vec!["escape".to_string(), "func5".to_string()],
            ..LineSpec::default()
        },
    )
    .unwrap();
    told(&page.update(false, false, None).unwrap());

    page.window(1, "buffer", 0, BOX, WindowSpec::default())
        .unwrap();
    page.line_input(
        1,
        80,
        LineSpec {
            initial: "go".to_string(),
            terminators: vec!["escape".to_string(), "func5".to_string()],
            ..LineSpec::default()
        },
    )
    .unwrap();
    page.typed(std::collections::HashMap::from([(1, "go nor".to_string())]));
    page.buffer(1, &[Run::text("normal", 0, "clock\n")], false)
        .unwrap();
    told(&page.update(false, false, None).unwrap());

    let mut grid = Page::new();

    grid.window(
        1,
        "grid",
        0,
        TOP,
        WindowSpec {
            gridsize: Some((80, 2)),
            ..WindowSpec::default()
        },
    )
    .unwrap();
    grid.window(
        2,
        "graphics",
        0,
        BOX,
        WindowSpec {
            graphsize: Some((320, 200)),
            scaled: true,
            ..WindowSpec::default()
        },
    )
    .unwrap();
    grid.grid(1, &[vec![TextRun::new("normal", 0, "Score 10   ")], vec![]])
        .unwrap();
    grid.draw(
        2,
        vec![Object::from([
            ("special", Value::from("fill")),
            ("x", Value::from(0i64)),
            ("y", Value::from(0i64)),
            ("width", Value::from(8i64)),
            ("height", Value::from(8i64)),
        ])],
    )
    .unwrap();
    grid.char_input(1, Some((3, 0)), false, false).unwrap();
    grid.line_input(
        2,
        40,
        LineSpec {
            cursor: Some((8, 184)),
            cell: Some((8, 8)),
            ink: Some("#c0ffee".to_string()),
            ..LineSpec::default()
        },
    )
    .unwrap();
    grid.timer(100, false);
    grid.prompt("write", "save").unwrap();
    told(&grid.update(false, false, None).unwrap());
}
