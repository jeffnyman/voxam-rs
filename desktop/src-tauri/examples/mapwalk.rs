//! The automapper's magnifying glass: a real walk, drawn as text.
//!
//! The map's own battery drills its rules; this instrument judges
//! its *layout*, which no assertion can. It reads a session's
//! update stanzas on stdin -- the wire sweeps' driver prints
//! exactly that, and grants the sidecar under VOXAM_SIDECAR -- and
//! prints the map the walk built: a census, the passages, and the
//! grid as a player would see it drawn.
//!
//! Pointed at the corpus's own recordings, it is how a layout
//! heuristic is judged against real play rather than invented
//! walks:
//!
//!     cd ../../..                     # the repository root
//!     VOXAM_SIDECAR=1 PYTHONUTF8=1 uv run --directory ../voxam python \
//!         certify/zglkote_drive.py ../voxam/acceptance/<name>.accept /tmp/saves \
//!         -- target/debug/examples/linked <story> <seed> \
//!         | cargo run --manifest-path desktop/src-tauri/Cargo.toml --example mapwalk

use std::collections::HashMap;
use std::io::BufRead;

use voxam_desktop_lib::map::{Map, Step};
use voxam_desktop_lib::sidecar::Bearings;

fn main() {
    let mut map = Map::default();
    let mut updates = 0;
    let mut observed = 0;

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let text = line.trim();

        if text.is_empty() {
            continue;
        }

        let Ok(stanza) = serde_json::from_str::<serde_json::Value>(text) else {
            continue;
        };

        updates += 1;

        if let Some(bearings) = Bearings::of(&stanza) {
            observed += 1;
            map.observe(&bearings);
        }
    }

    // The map itself, for the pane's own drawing to be fed by.
    if std::env::args().any(|held| held == "--json") {
        println!("{}", serde_json::to_string(&map).expect("a written map"));

        return;
    }

    println!("updates: {updates}, sidecar blocks: {observed}");

    if map.unreliable {
        println!("this story does not report where the player is; no map was drawn");
        return;
    }

    println!("rooms: {}, passages: {}", map.rooms.len(), map.edges.len());

    if map.rooms.is_empty() {
        println!("\n(nothing walked -- was the sidecar granted?)");
        return;
    }

    let ways = map.edges.iter().fold(HashMap::new(), |mut held, edge| {
        *held.entry(named(edge.step)).or_insert(0) += 1;
        held
    });
    let mut ways: Vec<_> = ways.into_iter().collect();

    ways.sort_by_key(|(name, _)| *name);

    println!(
        "passages by kind: {}",
        ways.iter()
            .map(|(name, count)| format!("{name} {count}"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    // How tightly the walk packed: a heuristic that spirals badly
    // shows here as a grid far larger than its room count.
    let left = map.rooms.values().map(|held| held.x).min().expect("a room");
    let right = map.rooms.values().map(|held| held.x).max().expect("a room");
    let top = map.rooms.values().map(|held| held.y).min().expect("a room");
    let bottom = map.rooms.values().map(|held| held.y).max().expect("a room");
    let cells = ((right - left + 1) as i64) * ((bottom - top + 1) as i64);

    println!(
        "grid: {} by {} = {cells} cells for {} rooms ({}% filled)",
        right - left + 1,
        bottom - top + 1,
        map.rooms.len(),
        (map.rooms.len() as i64 * 100) / cells.max(1)
    );

    println!("\nthe map, one cell per room:\n");

    let placed: HashMap<(i32, i32), &str> = map
        .rooms
        .values()
        .map(|held| ((held.x, held.y), held.name.as_str()))
        .collect();

    for y in top..=bottom {
        let mut row = String::new();

        for x in left..=right {
            row.push(match placed.get(&(x, y)) {
                Some(_) if map.here == here_at(&map, x, y) => '@',
                Some(_) => '#',
                None => '.',
            });
        }

        println!("  {row}");
    }

    println!("\nrooms, in reading order:\n");

    let mut rooms: Vec<_> = map.rooms.values().collect();

    rooms.sort_by_key(|held| (held.y, held.x));

    for room in rooms {
        println!("  ({:>3},{:>3})  {}", room.x, room.y, room.name);
    }
}

/// The room standing at a cell, when one does.
fn here_at(map: &Map, x: i32, y: i32) -> Option<i64> {
    map.rooms
        .values()
        .find(|held| held.x == x && held.y == y)
        .map(|held| held.object)
}

fn named(step: Step) -> &'static str {
    match step {
        Step::Compass(_) => "compass",
        Step::Up => "up",
        Step::Down => "down",
        Step::In => "in",
        Step::Out => "out",
        Step::Other => "unnamed",
    }
}
