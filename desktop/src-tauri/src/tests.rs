//! The shell's own small drills: the settings a window's shape
//! depends on, held to shapes a window can actually wear.

use super::*;

#[test]
fn a_layout_left_alone_is_kept_as_it_was() {
    let held = Panes {
        map: true,
        notes: true,
        width: 420,
        split: 0.35,
    }
    .held();

    assert_eq!(held.width, 420);
    assert!((held.split - 0.35).abs() < f64::EPSILON);
    assert!(held.map && held.notes);
}

#[test]
fn the_default_layout_is_already_wearable() {
    let held = Panes::default();
    let same = held.clone().held();

    assert_eq!(held.width, same.width);
    assert!((held.split - same.split).abs() < f64::EPSILON);
}

// A settings file edited by hand, or left by some other version,
// never gets to wedge the window into a shape with no way back.
#[test]
fn a_pane_too_narrow_to_read_is_widened() {
    assert_eq!(
        Panes {
            width: 12,
            ..Panes::default()
        }
        .held()
        .width,
        PANE_NARROWEST
    );
}

#[test]
fn a_pane_that_would_swallow_the_window_is_narrowed() {
    assert_eq!(
        Panes {
            width: 99_999,
            ..Panes::default()
        }
        .held()
        .width,
        PANE_WIDEST
    );
}

#[test]
fn a_division_past_either_end_is_brought_back() {
    assert!(
        (Panes {
            split: -3.0,
            ..Panes::default()
        }
        .held()
        .split
            - SPLIT_LEAST)
            .abs()
            < f64::EPSILON
    );
    assert!(
        (Panes {
            split: 40.0,
            ..Panes::default()
        }
        .held()
        .split
            - SPLIT_MOST)
            .abs()
            < f64::EPSILON
    );
}

// JSON has no NaN, but a number that arrives as one from anywhere
// else would clamp to itself and leave the panes undrawable.
#[test]
fn a_division_that_is_not_a_number_falls_back() {
    for held in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let settled = Panes {
            split: held,
            ..Panes::default()
        }
        .held();

        assert!(
            (settled.split - PANE_SPLIT).abs() < f64::EPSILON,
            "{held} should have fallen back"
        );
    }
}

// The file the shell writes must be one it can read again.
#[test]
fn a_layout_survives_being_written_and_read_back() {
    let held = Panes {
        map: true,
        notes: false,
        width: 505,
        split: 0.62,
    };
    let written = serde_json::to_string(&held).expect("a written layout");
    let read: Panes = serde_json::from_str(&written).expect("a read layout");

    assert_eq!(read.width, 505);
    assert!((read.split - 0.62).abs() < f64::EPSILON);
    assert!(read.map && !read.notes);
}

// A file from before the panes could be dragged carries neither
// width nor division, and must still open.
#[test]
fn a_layout_from_an_older_shell_still_opens() {
    let read: Panes = serde_json::from_str(r#"{"map":true}"#).expect("a read layout");

    assert!(read.map);
    assert!(!read.notes);
    assert_eq!(read.width, PANE_WIDTH);
    assert!((read.split - PANE_SPLIT).abs() < f64::EPSILON);
}
