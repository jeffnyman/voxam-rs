//! The automapper's brain: rooms walked, edges taken, cells laid.
//!
//! This is the first work in the project with no reference to port
//! from -- the Python implementation never had a mapper -- so the
//! design is written down here rather than inherited.
//!
//! The interpreter's contribution is the sidecar's dumb factual
//! feed (PORT: What the sidecar carries): where the player stands,
//! the command that moved them, and a bit saying this update does
//! not follow causally from the last. Everything else is this
//! module's own reasoning, which is exactly why it lives in the
//! shell and not in `voxam-core`: reading a typed command for its
//! compass word is an English-only, typed-input-only heuristic,
//! fine as a face's choice and poisonous as a core assumption.
//!
//! What the map is:
//!
//! - **Rooms are keyed by location object.** The machine's own
//!   number identifies a room across visits, so the same room is
//!   never drawn twice and a renamed one (a dark room lit) keeps
//!   its place under its newest name.
//! - **Edges are directed.** One-way passages are ordinary in
//!   interactive fiction, so walking north from A to B says
//!   nothing about what walking south from B does; the reciprocal
//!   edge is drawn only when it is actually walked.
//! - **A placed room never moves.** The map is watched live while
//!   it grows, and a layout that reshuffles under the player's eye
//!   is worse than one with a crooked corridor.
//! - **Up, down, in, and out are marked edges on one plane**
//!   rather than floors to page between: which floor a room
//!   belongs to is often unanswerable (a cave descending
//!   diagonally), and the marked edge never has to answer it.
//! - **Only what was walked is drawn.** The sidecar carries no
//!   exit list, and inferring untaken exits would mean reading
//!   room descriptions as English prose -- the heuristic the
//!   boundary forbids.
//! - **A discontinuity draws nothing.** Undo, restore, restart,
//!   and death all move the player without a passage between, and
//!   the interpreter says so, which spares the map every
//!   transcript-grepping guess earlier automappers needed.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::sidecar::Bearings;

/// How far a placement search will walk before giving up on the
/// ideal line and spiralling instead.
const ALONG_LIMIT: i32 = 32;

/// How far a spiral will ring outward before a room is left
/// unplaced. A map this dense is beyond drawing anyway.
const SPIRAL_LIMIT: i32 = 64;

/// How many direction commands may lead nowhere before the map
/// stops believing the story's reported location. A player can
/// walk into a wall; nobody walks into a dozen in a row.
const STUCK_LIMIT: u32 = 12;

/// The eight compass directions, the only steps with a place on
/// the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compass {
    North,
    Northeast,
    East,
    Southeast,
    South,
    Southwest,
    West,
    Northwest,
}

impl Compass {
    /// The cell this direction steps to, in a grid whose y grows
    /// downward as a screen's does.
    fn delta(self) -> (i32, i32) {
        match self {
            Self::North => (0, -1),
            Self::Northeast => (1, -1),
            Self::East => (1, 0),
            Self::Southeast => (1, 1),
            Self::South => (0, 1),
            Self::Southwest => (-1, 1),
            Self::West => (-1, 0),
            Self::Northwest => (-1, -1),
        }
    }
}

/// One move between rooms, as the map understands it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "kind", content = "way")]
pub enum Step {
    /// A compass move, which places its room on the grid.
    Compass(Compass),
    Up,
    Down,
    In,
    Out,
    /// A move the map cannot name: a magic word, a vehicle, a
    /// plot's own doing. Drawn, since the player did travel it,
    /// but never laid out by direction.
    Other,
}

impl Step {
    /// The grid step this move makes, if it makes one.
    fn delta(self) -> Option<(i32, i32)> {
        match self {
            Self::Compass(held) => Some(held.delta()),
            _ => None,
        }
    }
}

/// The movement verbs a command may wear before its direction.
const VERBS: [&str; 4] = ["go ", "walk ", "run ", "head "];

/// What one delivered line did to the player's whereabouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Walked {
    /// One move, drawn as a passage between the two rooms.
    Once(Step),
    /// Several moves on one line -- the house parsers all accept
    /// `d. s. e` -- which the wire reports as a single update at
    /// the end of the run. The rooms passed through were never
    /// seen, so no passage is drawn: an edge from the first room
    /// to the last would claim an adjacency that does not exist.
    /// When every leg was a compass word the destination can
    /// still be *placed* by their summed vector, which is a far
    /// better guess than a bare spiral.
    Chain(Option<(i32, i32)>),
}

/// Split a delivered line into the commands it actually holds.
///
/// The house parsers separate commands with a full stop, a comma,
/// or the word "then"; a trailing separator leaves no command
/// behind it.
fn legs(command: &str) -> Vec<String> {
    command
        .replace(" then ", ".")
        .replace(',', ".")
        .split('.')
        .map(|held| held.trim().to_string())
        .filter(|held| !held.is_empty())
        .collect()
}

/// Whether every command on a line was a direction.
///
/// This is the evidence a stale location is weighed on, and it
/// counts a chain as readily as a single word: the corpus's own
/// Adventure recording walks almost entirely in runs like
/// `w. s. s. s`, and a run of four directions that moves the
/// player nowhere says what one direction says, four times over.
fn all_directions(command: &str) -> bool {
    let legs = legs(command);

    !legs.is_empty() && legs.iter().all(|held| step_of(held).is_some())
}

/// Read one delivered line for what it did.
pub fn walked(command: &str) -> Walked {
    let legs = legs(command);

    // A line with nothing on it still moved the player: the story
    // itself carried them, and that is a passage worth drawing.
    let [only] = legs.as_slice() else {
        if legs.is_empty() {
            return Walked::Once(Step::Other);
        }

        let mut summed = (0, 0);

        for leg in &legs {
            // One leg that is not a compass word and the whole
            // sum is worthless: a vertical or unnamed move has no
            // vector to add.
            let Some(Step::Compass(held)) = step_of(leg) else {
                return Walked::Chain(None);
            };

            let (dx, dy) = held.delta();

            summed = (summed.0 + dx, summed.1 + dy);
        }

        return Walked::Chain(Some(summed));
    };

    Walked::Once(step_of(only).unwrap_or(Step::Other))
}

/// Read a typed command for its compass word.
///
/// English-only and typed-input-only by design, and the map is
/// honest about the limit: a command it cannot name is still a
/// move (`Step::Other`) when the player's whereabouts changed.
/// The abbreviations come first because the recordings say they
/// dominate -- of nearly two thousand movements in the corpus's
/// transcripts, over half are single letters.
pub fn step_of(command: &str) -> Option<Step> {
    let lowered = command.trim().to_lowercase();
    let held = lowered.trim_matches(|held: char| !held.is_alphanumeric() && held != ' ');

    // A leading movement verb is stripped and the rest read as the
    // direction: "go north" is north, and so is "walk n".
    let held = VERBS
        .iter()
        .find_map(|verb| held.strip_prefix(verb))
        .unwrap_or(held)
        .trim();

    let step = match held {
        "n" | "north" => Step::Compass(Compass::North),
        "ne" | "northeast" => Step::Compass(Compass::Northeast),
        "e" | "east" => Step::Compass(Compass::East),
        "se" | "southeast" => Step::Compass(Compass::Southeast),
        "s" | "south" => Step::Compass(Compass::South),
        "sw" | "southwest" => Step::Compass(Compass::Southwest),
        "w" | "west" => Step::Compass(Compass::West),
        "nw" | "northwest" => Step::Compass(Compass::Northwest),
        // Shipboard directions, which the house parsers take
        // wherever a game has a vessel to walk about in -- the
        // Heart of Gold's decks are half of Hitchhiker's own
        // recording. Laying fore as north is a convention, not a
        // claim about the compass: what matters is that it is the
        // *same* convention every time, so a ship's plan comes out
        // square instead of scattered.
        "fore" | "forward" => Step::Compass(Compass::North),
        "aft" => Step::Compass(Compass::South),
        "port" => Step::Compass(Compass::West),
        "sb" | "starboard" => Step::Compass(Compass::East),
        "u" | "up" => Step::Up,
        "d" | "down" => Step::Down,
        "in" | "enter" => Step::In,
        "out" | "exit" | "leave" => Step::Out,
        _ => return None,
    };

    Some(step)
}

/// One room the player has stood in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Room {
    /// The machine's own object number: the room's identity.
    pub object: i64,
    /// The name last printed for it. A dark room that is lit
    /// prints a different name for the same place, and the newest
    /// telling is the truer one.
    pub name: String,
    pub x: i32,
    pub y: i32,
}

/// One passage the player has walked, in the direction walked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Edge {
    pub from: i64,
    pub to: i64,
    pub step: Step,
}

// Step is a plain enum of unit variants; deriving Hash for Edge
// needs it, and the derive above cannot reach through serde's
// attributes to say so.
impl std::hash::Hash for Step {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);

        if let Self::Compass(held) = self {
            std::mem::discriminant(held).hash(state);
        }
    }
}

/// The map of one story, as walked.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Map {
    pub rooms: HashMap<i64, Room>,
    pub edges: Vec<Edge>,
    /// Where the player stands now, when the map knows.
    pub here: Option<i64>,
    /// This story's reported location is not to be believed.
    ///
    /// The location global is guaranteed only through Version 3
    /// and merely conventional after (§8), and some later stories
    /// keep nothing useful there: Adventure's Version 5 build
    /// reports one unchanging object with a garbled name however
    /// far the player walks. A map cannot tell a wrong number from
    /// a right one by looking at it, but it can notice that
    /// direction after direction moves the player nowhere -- and
    /// saying so plainly beats drawing a single fictional room.
    #[serde(default)]
    pub unreliable: bool,
    /// Consecutive direction commands that changed nothing.
    #[serde(default)]
    stuck: u32,
    /// Which room holds which cell, so a placement never lands on
    /// a room already drawn.
    #[serde(default)]
    taken: HashMap<String, i64>,
}

impl Map {
    /// Take one update's bearings and grow the map by whatever it
    /// honestly says.
    ///
    /// A location the machine cannot report leaves the map alone
    /// entirely -- Glulx has no location global, and a mapper that
    /// invented one would draw fiction.
    pub fn observe(&mut self, bearings: &Bearings) {
        let Some(seen) = &bearings.location else {
            return;
        };

        let from = self.here;
        let moved = from.is_some_and(|held| held != seen.object);

        // A story whose location never answers: count the
        // directions that led nowhere, and stop believing it once
        // no plausible game could have walked into that many
        // walls in a row.
        if moved || from.is_none() {
            self.stuck = 0;
        } else if bearings.command.as_deref().is_some_and(all_directions) {
            self.stuck += 1;

            if self.stuck >= STUCK_LIMIT {
                self.unreliable = true;
            }
        }

        if self.unreliable {
            self.here = Some(seen.object);

            return;
        }

        // What the player travelled, when they travelled at all:
        // a move the interpreter did not disown.
        let travel = (moved && !bearings.discontinuity)
            .then(|| walked(bearings.command.as_deref().unwrap_or_default()));

        // Where the new room belongs, when it is new: along the
        // step it was reached by, along a whole chain's summed
        // vector, else wherever there is room.
        if !self.rooms.contains_key(&seen.object) {
            let anchor = from
                .and_then(|held| self.rooms.get(&held))
                .map(|held| (held.x, held.y));
            let vector = match travel {
                Some(Walked::Once(step)) => step.delta(),
                Some(Walked::Chain(summed)) => summed.filter(|held| *held != (0, 0)),
                None => None,
            };
            let at = match (anchor, vector) {
                (Some((x, y)), Some((dx, dy))) => self.free_cell((x + dx, y + dy), Some((dx, dy))),
                (Some(held), None) => self.free_cell(held, None),
                (None, _) => self.free_cell((0, 0), None),
            };

            self.rooms.insert(
                seen.object,
                Room {
                    object: seen.object,
                    name: seen.name.clone(),
                    x: at.0,
                    y: at.1,
                },
            );
            self.taken.insert(celled(at), seen.object);
        } else if let Some(held) = self.rooms.get_mut(&seen.object) {
            // The room stays where it was drawn; only its name
            // follows the story.
            held.name = seen.name.clone();
        }

        // Only a single move earns a passage. A chain's rooms were
        // never seen, so an edge across it would draw an adjacency
        // that does not exist.
        if let (Some(from), Some(Walked::Once(step))) = (from, travel) {
            let edge = Edge {
                from,
                to: seen.object,
                step,
            };

            if !self.edges.contains(&edge) {
                self.edges.push(edge);
            }
        }

        self.here = Some(seen.object);
    }

    /// A cell no room holds: the ideal one if it is free, else
    /// further along the same line, else spiralling outward.
    ///
    /// Walking the line first is what keeps a corridor straight
    /// when a map doubles back on itself: the fourth room of a
    /// northward run belongs north of the third, not beside it.
    fn free_cell(&self, ideal: (i32, i32), along: Option<(i32, i32)>) -> (i32, i32) {
        if !self.taken.contains_key(&celled(ideal)) {
            return ideal;
        }

        if let Some((dx, dy)) = along {
            let mut at = ideal;

            for _ in 0..ALONG_LIMIT {
                at = (at.0 + dx, at.1 + dy);

                if !self.taken.contains_key(&celled(at)) {
                    return at;
                }
            }
        }

        for ring in 1..=SPIRAL_LIMIT {
            for dy in -ring..=ring {
                for dx in -ring..=ring {
                    if dx.abs() != ring && dy.abs() != ring {
                        continue;
                    }

                    let at = (ideal.0 + dx, ideal.1 + dy);

                    if !self.taken.contains_key(&celled(at)) {
                        return at;
                    }
                }
            }
        }

        ideal
    }
}

/// A cell's key. JSON objects take string keys, and the map is
/// persisted as JSON, so the pair is spelled rather than tupled.
fn celled((x, y): (i32, i32)) -> String {
    format!("{x},{y}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::Location;

    fn at(object: i64, name: &str, command: Option<&str>) -> Bearings {
        Bearings {
            location: Some(Location {
                object,
                name: name.to_string(),
            }),
            score: None,
            turns: None,
            command: command.map(str::to_string),
            discontinuity: false,
        }
    }

    fn cell(map: &Map, object: i64) -> (i32, i32) {
        let held = map.rooms.get(&object).expect("a drawn room");

        (held.x, held.y)
    }

    #[test]
    fn reads_the_abbreviations_the_corpus_actually_types() {
        assert_eq!(step_of("n"), Some(Step::Compass(Compass::North)));
        assert_eq!(step_of("SW"), Some(Step::Compass(Compass::Southwest)));
        assert_eq!(step_of("  east  "), Some(Step::Compass(Compass::East)));
        assert_eq!(step_of("go west"), Some(Step::Compass(Compass::West)));
        assert_eq!(step_of("walk n"), Some(Step::Compass(Compass::North)));
        assert_eq!(step_of("u"), Some(Step::Up));
        assert_eq!(step_of("down"), Some(Step::Down));
        assert_eq!(step_of("enter"), Some(Step::In));
        assert_eq!(step_of("exit"), Some(Step::Out));
    }

    #[test]
    fn a_command_that_is_not_a_direction_is_not_read_as_one() {
        assert_eq!(step_of("take lamp"), None);
        assert_eq!(step_of("northern exposure"), None);
        assert_eq!(step_of(""), None);
    }

    #[test]
    fn the_first_room_stands_at_the_origin() {
        let mut map = Map::default();

        map.observe(&at(180, "West of House", None));

        assert_eq!(cell(&map, 180), (0, 0));
        assert_eq!(map.here, Some(180));
        assert!(map.edges.is_empty());
    }

    #[test]
    fn a_walk_west_lays_the_room_west_and_draws_the_passage() {
        let mut map = Map::default();

        map.observe(&at(180, "West of House", None));
        map.observe(&at(78, "Forest", Some("west")));

        assert_eq!(cell(&map, 78), (-1, 0));
        assert_eq!(
            map.edges,
            vec![Edge {
                from: 180,
                to: 78,
                step: Step::Compass(Compass::West)
            }]
        );
    }

    #[test]
    fn a_command_that_moves_nowhere_draws_no_passage() {
        let mut map = Map::default();

        map.observe(&at(180, "West of House", None));
        map.observe(&at(180, "West of House", Some("look")));

        assert!(map.edges.is_empty());
        assert_eq!(map.rooms.len(), 1);
    }

    #[test]
    fn a_room_revisited_keeps_its_cell() {
        let mut map = Map::default();

        map.observe(&at(180, "West of House", None));
        map.observe(&at(78, "Forest", Some("west")));
        map.observe(&at(180, "West of House", Some("east")));

        assert_eq!(cell(&map, 180), (0, 0));
        assert_eq!(map.rooms.len(), 2);
    }

    // One-way passages are ordinary: walking north from A to B
    // says nothing about what walking south from B does, so the
    // reciprocal edge is drawn only when it is walked.
    #[test]
    fn passages_are_directed_and_never_assumed_reciprocal() {
        let mut map = Map::default();

        map.observe(&at(1, "Ledge", None));
        map.observe(&at(2, "Pit", Some("n")));
        map.observe(&at(3, "Tunnel", Some("s")));

        assert_eq!(map.edges.len(), 2);
        assert_eq!(map.edges[1].from, 2);
        assert_eq!(map.edges[1].to, 3);
        assert!(!map.edges.iter().any(|held| held.from == 2 && held.to == 1));
    }

    #[test]
    fn a_passage_walked_twice_is_drawn_once() {
        let mut map = Map::default();

        map.observe(&at(1, "Ledge", None));
        map.observe(&at(2, "Pit", Some("n")));
        map.observe(&at(1, "Ledge", Some("s")));
        map.observe(&at(2, "Pit", Some("n")));

        assert_eq!(map.edges.len(), 2);
    }

    // Undo, restore, restart, and death move the player with no
    // passage between: the map follows, and draws nothing.
    #[test]
    fn a_discontinuity_moves_the_player_but_draws_no_passage() {
        let mut map = Map::default();

        map.observe(&at(1, "Ledge", None));
        map.observe(&at(2, "Pit", Some("n")));

        let mut restored = at(1, "Ledge", Some("restore"));

        restored.discontinuity = true;
        map.observe(&restored);

        assert_eq!(map.here, Some(1));
        assert_eq!(map.edges.len(), 1);
    }

    #[test]
    fn a_vertical_move_is_a_marked_passage_beside_its_room() {
        let mut map = Map::default();

        map.observe(&at(1, "Kitchen", None));
        map.observe(&at(2, "Cellar", Some("down")));

        assert_eq!(map.edges[0].step, Step::Down);
        assert_ne!(cell(&map, 2), (0, 0));
    }

    // The house parsers all take several commands on one line,
    // and the corpus's own recordings lean on it heavily. The
    // wire reports one update at the end of the run, so the rooms
    // between were never seen: the map draws no passage across
    // them rather than claiming the first and last are adjacent.
    #[test]
    fn a_chain_of_commands_draws_no_passage() {
        let mut map = Map::default();

        map.observe(&at(1, "Kitchen", None));
        map.observe(&at(2, "Clearing", Some("d. s. e")));

        assert_eq!(map.rooms.len(), 2);
        assert!(map.edges.is_empty());
        assert_eq!(map.here, Some(2));
    }

    // A chain walked entirely in compass words still says where
    // its destination lies, even though it says nothing about the
    // path: the summed vector places the room far better than a
    // spiral would.
    #[test]
    fn an_all_compass_chain_places_by_its_summed_vector() {
        let mut map = Map::default();

        map.observe(&at(1, "Start", None));
        map.observe(&at(2, "Far Field", Some("e. e. n. n")));

        assert_eq!(cell(&map, 2), (2, -2));
        assert!(map.edges.is_empty());
    }

    #[test]
    fn a_chain_with_a_vertical_leg_keeps_its_counsel() {
        assert_eq!(walked("d. s. e"), Walked::Chain(None));
        assert_eq!(walked("e. e. se. e"), Walked::Chain(Some((4, 1))));
        assert_eq!(walked("n, n, u"), Walked::Chain(None));
        assert_eq!(walked("n then e"), Walked::Chain(Some((1, -1))));
    }

    // A trailing separator leaves no command behind it, so a line
    // that is really one move is still read as one.
    #[test]
    fn a_trailing_stop_does_not_make_a_chain() {
        assert_eq!(walked("u."), Walked::Once(Step::Up));
        assert_eq!(
            walked("north."),
            Walked::Once(Step::Compass(Compass::North))
        );
    }

    // A move with no command at all -- the story itself carrying
    // the player -- is a passage, and an honestly unnamed one.
    #[test]
    fn a_move_with_no_command_is_an_unnamed_passage() {
        let mut map = Map::default();

        map.observe(&at(1, "Cell", None));
        map.observe(&at(2, "Corridor", None));

        assert_eq!(map.edges[0].step, Step::Other);
    }

    #[test]
    fn a_move_the_map_cannot_name_is_still_a_passage() {
        let mut map = Map::default();

        map.observe(&at(1, "Temple", None));
        map.observe(&at(2, "Altar", Some("pray")));

        assert_eq!(map.edges[0].step, Step::Other);
    }

    // A run of rooms in one direction stays a straight corridor
    // even when it walks back over ground already drawn.
    #[test]
    fn a_collision_walks_further_along_the_same_line() {
        let mut map = Map::default();

        map.observe(&at(1, "One", None));
        map.observe(&at(2, "Two", Some("e")));
        // Back west, then east again into a *different* room: the
        // ideal cell is taken by Two, so Three goes further east.
        map.observe(&at(1, "One", Some("w")));
        map.observe(&at(3, "Three", Some("e")));

        assert_eq!(cell(&map, 2), (1, 0));
        assert_eq!(cell(&map, 3), (2, 0));
    }

    #[test]
    fn a_room_renamed_keeps_its_place_under_the_newer_name() {
        let mut map = Map::default();

        map.observe(&at(1, "Dark Room", None));
        map.observe(&at(2, "Hall", Some("n")));
        map.observe(&at(1, "Lit Room", Some("s")));

        assert_eq!(cell(&map, 1), (0, 0));
        assert_eq!(map.rooms[&1].name, "Lit Room");
    }

    // A machine with no location global -- Glulx has none -- draws
    // nothing at all rather than inventing a room.
    #[test]
    fn bearings_without_a_location_draw_nothing() {
        let mut map = Map::default();

        map.observe(&Bearings::default());

        assert!(map.rooms.is_empty());
        assert_eq!(map.here, None);
    }

    // The Heart of Gold's decks: shipboard words are directions
    // wherever a game gives the player a vessel, and laying them
    // on the compass is what makes a ship's plan come out square.
    #[test]
    fn shipboard_directions_are_directions() {
        assert_eq!(step_of("fore"), Some(Step::Compass(Compass::North)));
        assert_eq!(step_of("aft"), Some(Step::Compass(Compass::South)));
        assert_eq!(step_of("port"), Some(Step::Compass(Compass::West)));
        assert_eq!(step_of("sb"), Some(Step::Compass(Compass::East)));
        assert_eq!(step_of("starboard"), Some(Step::Compass(Compass::East)));
    }

    // Adventure's Version 5 build keeps nothing useful in the
    // location global and reports one unchanging object however
    // far the player walks. The map notices and says so, rather
    // than drawing a single fictional room forever.
    #[test]
    fn a_story_whose_location_never_answers_is_disbelieved() {
        let mut map = Map::default();

        map.observe(&at(2, "Ob.ect", None));

        for _ in 0..STUCK_LIMIT {
            map.observe(&at(2, "Ob.ect", Some("e")));
        }

        assert!(map.unreliable);
    }

    // And the same story walked in chains, which is how the
    // corpus's Adventure recording actually travels.
    #[test]
    fn a_stale_location_is_caught_through_chains_too() {
        let mut map = Map::default();

        map.observe(&at(2, "Ob.ect", None));

        for _ in 0..STUCK_LIMIT {
            map.observe(&at(2, "Ob.ect", Some("w. s. s. s")));
        }

        assert!(map.unreliable);
    }

    // Walking into the occasional wall is ordinary play and must
    // never cost the map its faith.
    #[test]
    fn a_few_walls_walked_into_are_forgiven() {
        let mut map = Map::default();

        map.observe(&at(1, "Clearing", None));

        for _ in 0..(STUCK_LIMIT - 1) {
            map.observe(&at(1, "Clearing", Some("n")));
        }

        map.observe(&at(2, "Forest", Some("s")));

        for _ in 0..(STUCK_LIMIT - 1) {
            map.observe(&at(2, "Forest", Some("n")));
        }

        assert!(!map.unreliable);
        assert_eq!(map.rooms.len(), 2);
    }

    // Commands that are not directions say nothing either way: a
    // long stretch of examining and taking must not be read as a
    // story that will not report itself.
    #[test]
    fn idling_is_not_evidence_against_a_story() {
        let mut map = Map::default();

        map.observe(&at(1, "Attic", None));

        for _ in 0..(STUCK_LIMIT * 2) {
            map.observe(&at(1, "Attic", Some("examine the rope")));
        }

        assert!(!map.unreliable);
    }

    // The pane's contract. The drawing reads these very names off
    // the JSON -- `step.kind`, the lowercase way, `here`, a room's
    // `x` and `y` -- so the shape is pinned here rather than
    // discovered by a pane that silently draws nothing.
    #[test]
    fn the_wire_the_pane_draws_from_keeps_its_shape() {
        let mut map = Map::default();

        map.observe(&at(1, "Attic", None));
        map.observe(&at(2, "Cellar", Some("down")));
        map.observe(&at(3, "Lawn", Some("west")));

        let held = serde_json::to_value(&map).expect("a written map");

        assert_eq!(held["rooms"]["1"]["name"], "Attic");
        assert_eq!(held["rooms"]["1"]["x"], 0);
        assert_eq!(held["rooms"]["1"]["y"], 0);
        assert_eq!(held["here"], 3);
        assert_eq!(held["unreliable"], false);

        assert_eq!(held["edges"][0]["from"], 1);
        assert_eq!(held["edges"][0]["to"], 2);
        assert_eq!(held["edges"][0]["step"]["kind"], "down");

        assert_eq!(held["edges"][1]["step"]["kind"], "compass");
        assert_eq!(held["edges"][1]["step"]["way"], "west");
    }

    #[test]
    fn a_map_survives_being_written_and_read_back() {
        let mut map = Map::default();

        map.observe(&at(180, "West of House", None));
        map.observe(&at(78, "Forest", Some("west")));

        let held = serde_json::to_string(&map).expect("a written map");
        let read: Map = serde_json::from_str(&held).expect("a read map");

        assert_eq!(read.rooms.len(), 2);
        assert_eq!(read.edges, map.edges);
        assert_eq!(read.here, Some(78));

        // The cell ledger survives too, so a later room never
        // lands on one already drawn.
        let mut walked = read;

        walked.observe(&at(180, "West of House", Some("east")));
        walked.observe(&at(99, "Clearing", Some("west")));

        assert_ne!(cell(&walked, 99), (-1, 0));
    }
}
