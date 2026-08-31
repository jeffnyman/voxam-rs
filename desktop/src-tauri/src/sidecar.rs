//! The `voxam:` block, read off the wire.
//!
//! The interpreter's whole contribution to the deluxe features is
//! a dumb factual feed riding each update stanza: where the player
//! is, what they scored, how many turns have passed, the command
//! that moved them, and one bit saying this update does not follow
//! causally from the last (PORT: What the sidecar carries). Every
//! ounce of graph, layout, and rendering intelligence lives on
//! this side of the wire.
//!
//! The shell reads the block in its own pump, before the stanza
//! reaches the page, because the map's intelligence lives in this
//! Rust and not in the webview. An ungranted session carries no
//! block at all, so absence is ordinary and never an error.
//!
//! Every field is optional on purpose. The Z-Machine's location
//! global is guaranteed only through Version 3 and conventional
//! after; Glulx has no fixed location global at all; and a story
//! that keeps no score reports none. The reader is honest about
//! all of it rather than pretending zeros, so a mapper can tell
//! "nowhere" from "room zero".

use serde::Serialize;
use serde_json::Value;

/// Where the player stands: the object number the machine keeps,
/// and the name it prints for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Location {
    pub object: i64,
    pub name: String,
}

/// One update's bearings, as the interpreter reported them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Bearings {
    pub location: Option<Location>,
    pub score: Option<i64>,
    pub turns: Option<i64>,
    /// The line the wire handed the machine -- scripted input
    /// included, which beats any memory of what was typed.
    pub command: Option<String>,
    /// This update does not follow from the last command: an undo,
    /// restore, restart, or death intervened. A mapper that heeds
    /// it never draws an edge across time travel.
    pub discontinuity: bool,
}

impl Bearings {
    /// The bearings a stanza carries, or None when it carries no
    /// sidecar -- an ungranted session, or a cycle that reported
    /// nothing.
    pub fn of(stanza: &Value) -> Option<Self> {
        let block = stanza.get("voxam")?.as_object()?;

        let location =
            block
                .get("location")
                .and_then(Value::as_object)
                .and_then(|held| -> Option<Location> {
                    Some(Location {
                        object: held.get("object")?.as_i64()?,
                        name: held.get("name")?.as_str()?.to_string(),
                    })
                });

        Some(Self {
            location,
            score: block.get("score").and_then(Value::as_i64),
            turns: block.get("turns").and_then(Value::as_i64),
            command: block
                .get("command")
                .and_then(Value::as_str)
                .map(str::to_string),
            discontinuity: block
                .get("discontinuity")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The blocks below were taken from live Zork I sessions over
    /// the linked host's own pipes, not written by hand: the
    /// opening cycle, a step west, and the restart whose answer
    /// carries the discontinuity bit.
    fn stanza(voxam: &str) -> Value {
        serde_json::from_str(&format!(r#"{{"type":"update","gen":1,"voxam":{voxam}}}"#))
            .expect("a wire stanza")
    }

    #[test]
    fn reads_the_opening_bearings() {
        let held = Bearings::of(&stanza(
            r#"{"location":{"object":180,"name":"West of House"},"score":0,"turns":0}"#,
        ))
        .expect("a sidecar block");

        assert_eq!(
            held.location,
            Some(Location {
                object: 180,
                name: "West of House".to_string()
            })
        );
        assert_eq!(held.score, Some(0));
        assert_eq!(held.turns, Some(0));
        assert_eq!(held.command, None);
        assert!(!held.discontinuity);
    }

    #[test]
    fn reads_the_command_that_moved_the_player() {
        let held = Bearings::of(&stanza(
            r#"{"location":{"object":78,"name":"Forest"},"score":0,"turns":1,"command":"west"}"#,
        ))
        .expect("a sidecar block");

        assert_eq!(held.command.as_deref(), Some("west"));
        assert_eq!(held.location.expect("a location").object, 78);
        assert_eq!(held.turns, Some(1));
    }

    #[test]
    fn reads_the_discontinuity_bit() {
        let held = Bearings::of(&stanza(
            r#"{"location":{"object":180,"name":"West of House"},"score":0,"turns":0,"command":"y","discontinuity":true}"#,
        ))
        .expect("a sidecar block");

        assert!(held.discontinuity);
        assert_eq!(held.command.as_deref(), Some("y"));
    }

    #[test]
    fn an_ungranted_session_carries_no_block() {
        let plain: Value = serde_json::from_str(r#"{"type":"update","gen":1}"#).expect("a stanza");

        assert_eq!(Bearings::of(&plain), None);
    }

    #[test]
    fn an_empty_block_is_still_a_reading() {
        let held = Bearings::of(&stanza("{}")).expect("a sidecar block");

        assert_eq!(held, Bearings::default());
    }

    // A machine with no location global reports none, and the
    // reader says so rather than inventing room zero.
    #[test]
    fn a_locationless_machine_reports_no_location() {
        let held = Bearings::of(&stanza(r#"{"score":10,"turns":3}"#)).expect("a sidecar block");

        assert_eq!(held.location, None);
        assert_eq!(held.score, Some(10));
    }

    // A half-written location is no location: both halves or
    // neither, so a mapper never keys a room by a missing name.
    #[test]
    fn a_half_written_location_is_refused() {
        let held =
            Bearings::of(&stanza(r#"{"location":{"object":180}}"#)).expect("a sidecar block");

        assert_eq!(held.location, None);
    }
}
