//! The desktop shell's core: the interpreter linked in, speaking
//! GlkOte over a pipe instead of a process boundary.
//!
//! The webview wears the display; the session facade in
//! `voxam-core` owns the story. This core opens a pair of
//! in-memory pipes, serves the session on a thread of its own,
//! pumps the lines it writes to the page as events, and writes the
//! page's events back down the other pipe. The contract the child
//! process once kept is now kept by this module directly: nothing
//! is said until the init stanza arrives, every line is a stanza,
//! pre-wire refusals travel as bare `voxam: ...` text, and a
//! session ends 0 on game over or hangup, 2 on a fault.
//!
//! Milestone 7 swapped the subprocess out from under this file
//! without the page noticing: the events, their shapes, and the
//! session-id filtering are exactly as the spawned arrangement
//! left them, so `shell.js` did not change a line. What went away
//! with the child: finding an interpreter beside the shell, the
//! missing-interpreter refusal, the console-window suppression,
//! and the `--babel` subprocess a title bar used to cost.

pub mod map;
pub mod sidecar;

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, Wry};
use voxam_core::pipe;
use voxam_core::session::Opening;
use voxam_core::zmachine::machine::Identity;

use crate::map::Map;
use crate::sidecar::Bearings;

/// The exit codes the CLI spoke, kept for the page's ended bar: 0
/// for a session that ended cleanly, 2 for one that faulted.
const EXIT_OK: i32 = 0;
const EXIT_UNUSABLE: i32 = 2;

/// The screen's share the window opens at, as the pygame glass
/// takes it: 0.85 of the desktop, centered.
const SHARE: f64 = 0.85;

/// The §11.1.3 platforms the Story menu offers: the names Infocom
/// used, the words the menu ids are keyed by, and the numbers the
/// header claims. Glulx and Å-machine stories have no such header
/// and ignore the claim, so it rides every session unconditionally.
const PLATFORMS: [(&str, &str, u8); 11] = [
    ("DECSystem-20", "dec-20", 1),
    ("Apple IIe", "apple-iie", 2),
    ("Macintosh", "macintosh", 3),
    ("Amiga", "amiga", 4),
    ("Atari ST", "atari-st", 5),
    ("IBM PC", "ibm-pc", 6),
    ("Commodore 128", "commodore-128", 7),
    ("Commodore 64", "commodore-64", 8),
    ("Apple IIc", "apple-iic", 9),
    ("Apple IIgs", "apple-iigs", 10),
    ("Tandy Color", "tandy-color", 11),
];

/// The identity the next machine boots with (§11.1.3-4): the
/// claimed platform and the legendary Tandy bit. IBM PC to begin,
/// since that is the number voxam claims on its own.
#[derive(Clone)]
struct Claim {
    interpreter: String,
    tandy: bool,
}

impl Default for Claim {
    fn default() -> Self {
        Self {
            interpreter: "ibm-pc".to_string(),
            tandy: false,
        }
    }
}

impl Claim {
    /// The claim as the machine wears it (§11.1.3-4).
    fn identity(&self) -> Identity {
        Identity {
            interpreter: PLATFORMS
                .iter()
                .find(|(_, named, _)| *named == self.interpreter)
                .map(|(_, _, number)| *number),
            tandy: self.tandy,
        }
    }
}

/// One Display menu group: its shown title, its settings word,
/// and its (label, value) options.
type DisplayGroup = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// The Display menu's roster: each group a radio row, each value
/// the word the page's own dresser understands.
const DISPLAY_GROUPS: [DisplayGroup; 4] = [
    (
        "Type",
        "face",
        &[("Serif", "serif"), ("Sans", "sans"), ("Typewriter", "mono")],
    ),
    (
        "Size",
        "size",
        &[
            ("12", "12"),
            ("15", "15"),
            ("18", "18"),
            ("21", "21"),
            ("24", "24"),
        ],
    ),
    (
        "Ink",
        "theme",
        &[("Paper", "paper"), ("Sepia", "sepia"), ("Dark", "dark")],
    ),
    (
        "Measure",
        "measure",
        &[
            ("Narrow", "narrow"),
            ("Standard", "standard"),
            ("Wide", "wide"),
            ("Full", "full"),
        ],
    ),
];

/// How the page dresses itself: the face and size of the story's
/// type, the ink it is set in, and the measure of its column.
/// Persisted as display.json in the app's own config dir, so the
/// menu's checkmarks tell the truth at every startup.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Display {
    face: String,
    size: u32,
    theme: String,
    measure: String,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            face: "serif".to_string(),
            size: 15,
            theme: "paper".to_string(),
            measure: "standard".to_string(),
        }
    }
}

impl Display {
    /// The chosen value of one menu group, for the checkmarks.
    fn value(&self, group: &str) -> String {
        match group {
            "face" => self.face.clone(),
            "size" => self.size.to_string(),
            "theme" => self.theme.clone(),
            _ => self.measure.clone(),
        }
    }
}

/// Which side panes stand open. Persisted as panes.json beside
/// the display settings, so the window opens as it was left.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct Panes {
    map: bool,
}

fn panes_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join("panes.json"))
}

fn load_panes(app: &AppHandle) -> Panes {
    panes_path(app)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|held| serde_json::from_str(&held).ok())
        .unwrap_or_default()
}

fn save_panes(app: &AppHandle, panes: &Panes) {
    let Some(path) = panes_path(app) else {
        return;
    };

    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    if let Ok(held) = serde_json::to_string_pretty(panes) {
        let _ = std::fs::write(path, held);
    }
}

/// Where the open-story dialog starts: a folder the player pinned
/// by hand, or -- with nothing pinned -- wherever the last story
/// was opened from, so a save to some other corner of the disk
/// never drags the story picker after it. Persisted as home.json
/// beside the display settings.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct Home {
    pinned: Option<PathBuf>,
    followed: Option<PathBuf>,
}

fn home_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join("home.json"))
}

fn load_home(app: &AppHandle) -> Home {
    home_path(app)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|held| serde_json::from_str(&held).ok())
        .unwrap_or_default()
}

fn save_home(app: &AppHandle, home: &Home) {
    let Some(path) = home_path(app) else {
        return;
    };

    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    if let Ok(held) = serde_json::to_string_pretty(home) {
        let _ = std::fs::write(path, held);
    }
}

fn display_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join("display.json"))
}

fn load_display(app: &AppHandle) -> Display {
    display_path(app)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|held| serde_json::from_str(&held).ok())
        .unwrap_or_default()
}

/// Best effort: a display preference that cannot be kept is not
/// worth refusing the change over.
fn save_display(app: &AppHandle, display: &Display) {
    let Some(path) = display_path(app) else {
        return;
    };

    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    if let Ok(held) = serde_json::to_string_pretty(display) {
        let _ = std::fs::write(path, held);
    }
}

/// The menus' own check items, kept so a choice can dress its
/// whole radio row and the toggle from the state's word.
struct Chrome {
    interpreters: Vec<(String, CheckMenuItem<Wry>)>,
    tandy: CheckMenuItem<Wry>,
    displays: Vec<(String, CheckMenuItem<Wry>)>,
    following: CheckMenuItem<Wry>,
    mapped: CheckMenuItem<Wry>,
}

/// One running story: the writing end of the pipe it listens on.
///
/// The reading end belongs to the pump thread, so send_stanza
/// never contends with it. Dropping this sender is the hangup that
/// ends the session, exactly as closing a child's stdin was.
struct Session {
    id: u64,
    to_session: pipe::Sender,
}

#[derive(Default)]
struct Shell {
    session: Mutex<Option<Session>>,
    story: Mutex<Option<PathBuf>>,
    claim: Mutex<Claim>,
    display: Mutex<Display>,
    home: Mutex<Home>,
    minted: AtomicU64,
    /// The last bearings the wire reported: where the player
    /// stands, and how they got there. The deluxe panes read it;
    /// an ungranted or silent session leaves it as it was.
    bearings: Mutex<Option<Bearings>>,
    /// The map of the story being played, grown from those
    /// bearings and kept under the story's IFID.
    map: Mutex<Map>,
    /// That IFID: the Treaty's own name for the story, which is
    /// what a map is filed under. A story the treaty cannot name
    /// keeps its map only for the session.
    ifid: Mutex<Option<String>>,
    /// Which side panes stand open.
    panes: Mutex<Panes>,
}

/// Remember the chosen story and wear its name on the title bar.
/// The story's own folder becomes the followed home, so the next
/// open starts among stories no matter where a save wandered.
#[tauri::command]
async fn set_story(app: AppHandle, state: State<'_, Shell>, path: String) -> Result<(), String> {
    let chosen = PathBuf::from(&path);
    let name = titled(&chosen);

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_title(&format!("{name} \u{2014} Voxam"));
    }

    if let Some(parent) = chosen.parent() {
        let mut home = state.home.lock().unwrap();

        home.followed = Some(parent.to_path_buf());

        save_home(&app, &home);
    }

    *state.story.lock().unwrap() = Some(chosen);

    Ok(())
}

/// Where the story picker opens: the pinned folder if one was
/// chosen, else wherever the last story came from.
#[tauri::command]
async fn story_home(state: State<'_, Shell>) -> Result<Option<String>, String> {
    let home = state.home.lock().unwrap();

    Ok(home
        .pinned
        .as_ref()
        .or(home.followed.as_ref())
        .map(|path| path.to_string_lossy().into_owned()))
}

/// Pin the stories folder, or unpin it to follow the last story
/// again; the menu's checkmark tells whichever is true.
#[tauri::command]
async fn set_home(
    app: AppHandle,
    state: State<'_, Shell>,
    path: Option<String>,
) -> Result<(), String> {
    let mut home = state.home.lock().unwrap();

    home.pinned = path.map(PathBuf::from);

    save_home(&app, &home);

    let _ = app
        .state::<Chrome>()
        .following
        .set_checked(home.pinned.is_none());

    Ok(())
}

/// The story's name under the Treaty of Babel, read in-process
/// where a `--babel` subprocess used to answer -- the same two
/// roads that report's Title line takes: a Blorb's iFiction record
/// names its story, and anything else is looked up in the Infocom
/// catalog by IFID. The filename's stem stands in for the
/// nameless, and every failure along the way falls back to it: a
/// title bar is a courtesy, never a gate.
fn titled(story: &Path) -> String {
    let stem = story
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Voxam".to_string());

    let Ok(bytes) = std::fs::read(story) else {
        return stem;
    };

    let blorbed = story
        .extension()
        .map(|held| held.to_string_lossy().to_lowercase())
        .is_some_and(|suffix| voxam_core::session::BLORB_SUFFIXES.contains(&suffix.as_str()));

    if blorbed {
        let Ok(blorb) = voxam_core::blorb::Blorb::parse(&bytes) else {
            return stem;
        };
        let record = blorb
            .ifiction
            .as_deref()
            .and_then(voxam_core::babel::ifiction);

        if let Some(title) = record.as_ref().and_then(|held| held.title.clone()) {
            if !title.trim().is_empty() {
                return title.trim().to_string();
            }
        }

        // A record that names no title still names an IFID, and
        // the packaged story answers when it does not.
        let identity = record.and_then(|held| held.ifid).or_else(|| {
            blorb
                .glulx()
                .or_else(|| blorb.story())
                .and_then(voxam_core::babel::ifid)
        });

        return identity
            .and_then(|held| voxam_core::infocom::title(&held).map(str::to_string))
            .unwrap_or(stem);
    }

    voxam_core::babel::ifid(&bytes)
        .and_then(|held| voxam_core::infocom::title(&held).map(str::to_string))
        .unwrap_or(stem)
}

/// The story the shell holds, surviving the page's reloads.
#[tauri::command]
async fn current_story(state: State<'_, Shell>) -> Result<Option<String>, String> {
    Ok(state
        .story
        .lock()
        .unwrap()
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned()))
}

/// Serve the held story on a thread of its own and start the pump.
///
/// Returns the minted session id; every event this session emits
/// carries it, so a reloaded page can ignore a dead session's
/// last words (the stale-ended race).
///
/// The story crosses to the serving thread as bytes and is opened
/// over there: a built session is a thicket of `Rc` handles that
/// could never cross a thread boundary, while bytes and pipe ends
/// cross freely. That is why the facade takes bytes and not a
/// path -- the linked host's whole arrangement rests on it.
#[tauri::command]
async fn start_session(app: AppHandle, state: State<'_, Shell>) -> Result<u64, String> {
    let story = state
        .story
        .lock()
        .unwrap()
        .clone()
        .ok_or("no story has been chosen")?;

    let name = story
        .file_name()
        .map(|held| held.to_string_lossy().into_owned())
        .unwrap_or_default();
    let bytes = std::fs::read(&story).map_err(|fault| format!("voxam: {fault}"))?;
    let sidecar = sidecar_of(&story);

    // The Story menu's claim joins the boot (§11.1.3-4); the
    // machines without a header for it ignore it.
    let identity = state.claim.lock().unwrap().identity();

    let mut held = state.session.lock().unwrap();

    // A replaced session hangs up when its sender drops. One that
    // is standing at a read ends on the spot; one spinning inside
    // the machine plays on unheard until the shell exits, which is
    // the linked arrangement's one honest cost -- see the departure
    // note in PORT.md.
    held.take();

    // A new story starts nowhere: the last one's bearings are not
    // this one's, and a pane must never draw them as such.
    state.bearings.lock().unwrap().take();

    // The story's map comes back from the last time it was
    // played, under the Treaty's own name for it. A story the
    // treaty cannot name keeps its map only for this session.
    let ifid = identified(&name, &bytes);

    *state.map.lock().unwrap() = ifid
        .as_deref()
        .map(|held| load_map(&app, held))
        .unwrap_or_default();
    *state.ifid.lock().unwrap() = ifid;

    let id = state.minted.fetch_add(1, Ordering::SeqCst) + 1;

    let (to_session, from_host) = pipe::pipe();
    let (to_host, from_session) = pipe::pipe();

    *held = Some(Session { id, to_session });
    drop(held);

    // The serving thread's verdict, left where the pump can read it
    // once the writing end drops: the store happens first, so an
    // EOF always finds the code that explains it.
    let verdict = Arc::new(AtomicI32::new(EXIT_OK));
    let told = Arc::clone(&verdict);
    let faults = app.clone();

    std::thread::spawn(move || {
        let mut reader = from_host;
        let mut writer = to_host;

        // A panic inside the machines is this shell's own crash,
        // and the page's error pane is where a crash belongs --
        // the stderr drain's duty, kept now that there is no
        // stderr to drain.
        let played = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Opening::of(&name, bytes, sidecar)
                .and_then(|opening| opening.serve(&mut reader, &mut writer, None, identity))
        }));

        let code = match played {
            Ok(Ok(true)) => EXIT_OK,
            Ok(Ok(false)) => EXIT_UNUSABLE,
            Ok(Err(error)) => {
                // A pre-wire refusal, spoken in voxam's own words:
                // the story would not load, or its face would not
                // stand up, and no protocol yet exists to carry it.
                let _ = faults.emit(
                    "fault",
                    json!({"id": id, "kind": "refusal", "text": format!("voxam: {error}")}),
                );

                EXIT_UNUSABLE
            }
            Err(panic) => {
                let told = panic
                    .downcast_ref::<&str>()
                    .map(|held| (*held).to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "the interpreter panicked".to_string());

                let _ = faults.emit("fault", json!({"id": id, "kind": "crash", "text": told}));

                EXIT_UNUSABLE
            }
        };

        told.store(code, Ordering::SeqCst);

        // The pump's end of the story: dropping the writer is what
        // EOFs it, and the code above is already in place to read.
        drop(writer);
    });

    let pump = app.clone();

    std::thread::spawn(move || {
        let mut lines = from_session;
        let mut line = String::new();

        loop {
            line.clear();

            match lines.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }

            let text = line.trim_end();

            if text.is_empty() {
                continue;
            }

            // A line that is not JSON is a pre-wire refusal, spoken
            // in voxam's own words; it travels as a fault verbatim.
            match serde_json::from_str::<Value>(text) {
                Ok(stanza) => {
                    // The sidecar is read here, before the page
                    // ever sees the stanza: the deluxe features'
                    // intelligence lives in this Rust, and the
                    // webview is left to wear the display alone.
                    if let Some(bearings) = Bearings::of(&stanza) {
                        let state = pump.state::<Shell>();

                        *state.bearings.lock().unwrap() = Some(bearings.clone());

                        // The map grows here, and is written only
                        // when it actually grew: a session spent
                        // examining the scenery rewrites nothing.
                        let mut map = state.map.lock().unwrap();
                        let before = (map.rooms.len(), map.edges.len(), map.here, map.unreliable);

                        map.observe(&bearings);

                        let after = (map.rooms.len(), map.edges.len(), map.here, map.unreliable);

                        if before != after {
                            let held = map.clone();

                            drop(map);

                            if let Some(ifid) = state.ifid.lock().unwrap().as_deref() {
                                save_map(&pump, ifid, &held);
                            }

                            // The pane redraws only when there is
                            // something new to draw.
                            let _ = pump.emit("map", held);
                        }
                    }

                    let _ = pump.emit("stanza", json!({"id": id, "stanza": stanza}));
                }
                Err(_) => {
                    let _ = pump.emit("fault", json!({"id": id, "kind": "refusal", "text": text}));
                }
            }
        }

        // EOF: if this session is still the current one, retire it
        // and report how it ended; a replaced session dies silently.
        let state = pump.state::<Shell>();
        let mut held = state.session.lock().unwrap();

        if held.as_ref().map(|session| session.id) == Some(id) {
            held.take();
            drop(held);

            let _ = pump.emit(
                "ended",
                json!({"id": id, "code": verdict.load(Ordering::SeqCst)}),
            );
        }
    });

    Ok(id)
}

/// A bare story's like-named Blorb, when one lies beside it. A
/// story that is its own container needs none: the facade unwraps
/// what it carries.
fn sidecar_of(story: &Path) -> Option<Vec<u8>> {
    let suffix = story
        .extension()
        .map(|held| held.to_string_lossy().to_lowercase());

    if suffix.is_some_and(|held| voxam_core::session::BLORB_SUFFIXES.contains(&held.as_str())) {
        return None;
    }

    voxam_core::session::BLORB_SUFFIXES
        .iter()
        .map(|held| story.with_extension(held))
        .find(|beside| beside.exists())
        .and_then(|beside| std::fs::read(beside).ok())
}

/// The story's IFID: the Treaty of Babel's own name for it, and
/// what a map is filed under. A Blorb's iFiction record answers
/// first, then the packaged or loose story's own bytes (Babel:
/// The IFID for a blorbed story file).
fn identified(name: &str, bytes: &[u8]) -> Option<String> {
    let blorbed = name.rsplit_once('.').is_some_and(|(_, suffix)| {
        voxam_core::session::BLORB_SUFFIXES.contains(&suffix.to_lowercase().as_str())
    });

    if !blorbed {
        return voxam_core::babel::ifid(bytes);
    }

    let blorb = voxam_core::blorb::Blorb::parse(bytes).ok()?;
    let record = blorb
        .ifiction
        .as_deref()
        .and_then(voxam_core::babel::ifiction);

    record.and_then(|held| held.ifid).or_else(|| {
        blorb
            .glulx()
            .or_else(|| blorb.story())
            .and_then(voxam_core::babel::ifid)
    })
}

/// Where a story's map is kept: one file per IFID, beside the
/// display settings.
fn map_path(app: &AppHandle, ifid: &str) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join("maps").join(format!("{ifid}.json")))
}

fn load_map(app: &AppHandle, ifid: &str) -> Map {
    map_path(app, ifid)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|held| serde_json::from_str(&held).ok())
        .unwrap_or_default()
}

/// Best effort, as the other settings are: a map that cannot be
/// kept is not worth interrupting a story over.
fn save_map(app: &AppHandle, ifid: &str, map: &Map) {
    let Some(path) = map_path(app, ifid) else {
        return;
    };

    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    if let Ok(held) = serde_json::to_string(map) {
        let _ = std::fs::write(path, held);
    }
}

/// The map of the story being played, as far as it is walked.
#[tauri::command]
async fn walked_map(state: State<'_, Shell>) -> Result<Map, String> {
    Ok(state.map.lock().unwrap().clone())
}

/// Which side panes stand open, asked at every load.
#[tauri::command]
async fn open_panes(state: State<'_, Shell>) -> Result<Panes, String> {
    Ok(state.panes.lock().unwrap().clone())
}

/// Where the player stands, as the wire last reported it.
///
/// The panes ask this; a session that grants no sidecar, or one
/// that has not yet said anything, answers with nothing at all.
#[tauri::command]
async fn bearings(state: State<'_, Shell>) -> Result<Option<Bearings>, String> {
    Ok(state.bearings.lock().unwrap().clone())
}

/// The settings the page dresses in, asked at every load.
#[tauri::command]
async fn display_settings(state: State<'_, Shell>) -> Result<Display, String> {
    Ok(state.display.lock().unwrap().clone())
}

/// One GlkOte event down the pipe, on its own line, flushed.
#[tauri::command]
async fn send_stanza(state: State<'_, Shell>, line: String) -> Result<(), String> {
    let mut held = state.session.lock().unwrap();

    let session = held.as_mut().ok_or("no session is running")?;

    writeln!(session.to_session, "{line}")
        .and_then(|()| session.to_session.flush())
        .map_err(|fault| format!("the pipe failed: {fault}"))
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Shell::default())
        .setup(|app| {
            let handle = app.handle();

            let open = MenuItem::with_id(
                handle,
                "open",
                "Open Story\u{2026}",
                true,
                Some("CmdOrCtrl+O"),
            )?;
            let restart =
                MenuItem::with_id(handle, "restart", "Restart Story", true, None::<&str>)?;

            // Where the story picker opens: pin a folder, or
            // follow the last story -- the persisted home read
            // first, so the checkmark tells the truth at startup.
            let settled = load_home(handle);

            *app.state::<Shell>().home.lock().unwrap() = settled.clone();

            let pin =
                MenuItem::with_id(handle, "home", "Choose Folder\u{2026}", true, None::<&str>)?;
            let following = CheckMenuItem::with_id(
                handle,
                "follow",
                "Follow the Last Story",
                true,
                settled.pinned.is_none(),
                None::<&str>,
            )?;
            let homes = Submenu::with_items(handle, "Stories Home", true, &[&pin, &following])?;

            let quit = PredefinedMenuItem::quit(handle, Some("Exit"))?;
            let file =
                Submenu::with_items(handle, "File", true, &[&open, &restart, &homes, &quit])?;

            // The Story menu: the §11.1.3 platform claim as a
            // radio row -- IBM PC checked first, the number voxam
            // claims on its own -- and the Tandy bit as a toggle.
            let claimed = app.state::<Shell>().claim.lock().unwrap().clone();
            let mut interpreters = Vec::new();

            for (shown, named, _) in PLATFORMS {
                interpreters.push((
                    named.to_string(),
                    CheckMenuItem::with_id(
                        handle,
                        format!("claim:{named}"),
                        shown,
                        true,
                        named == claimed.interpreter,
                        None::<&str>,
                    )?,
                ));
            }

            let row: Vec<&dyn IsMenuItem<Wry>> = interpreters
                .iter()
                .map(|(_, item)| item as &dyn IsMenuItem<Wry>)
                .collect();
            let platforms = Submenu::with_items(handle, "Interpreter", true, &row)?;
            let tandy = CheckMenuItem::with_id(
                handle,
                "tandy",
                "Tandy Header Bit",
                true,
                false,
                None::<&str>,
            )?;
            let story = Submenu::with_items(handle, "Story", true, &[&platforms, &tandy])?;

            // The Display menu: the persisted dress read first, so
            // every checkmark tells the truth at startup.
            let dressed = load_display(handle);

            *app.state::<Shell>().display.lock().unwrap() = dressed.clone();

            let mut displays = Vec::new();
            let mut groups = Vec::new();

            for (shown, group, options) in DISPLAY_GROUPS {
                let chosen = dressed.value(group);
                let start = displays.len();

                for (label, value) in options {
                    displays.push((
                        format!("{group}:{value}"),
                        CheckMenuItem::with_id(
                            handle,
                            format!("{group}:{value}"),
                            *label,
                            true,
                            *value == chosen,
                            None::<&str>,
                        )?,
                    ));
                }

                let row: Vec<&dyn IsMenuItem<Wry>> = displays[start..]
                    .iter()
                    .map(|(_, item)| item as &dyn IsMenuItem<Wry>)
                    .collect();

                groups.push(Submenu::with_items(handle, shown, true, &row)?);
            }

            let rows: Vec<&dyn IsMenuItem<Wry>> = groups
                .iter()
                .map(|group| group as &dyn IsMenuItem<Wry>)
                .collect();
            let display = Submenu::with_items(handle, "Display", true, &rows)?;

            // The View menu: the side panes, each a toggle, opened
            // as they were left. The map draws itself from the
            // sidecar the shell already reads.
            let standing = load_panes(handle);

            *app.state::<Shell>().panes.lock().unwrap() = standing.clone();

            let mapped = CheckMenuItem::with_id(
                handle,
                "pane:map",
                "Map",
                true,
                standing.map,
                Some("CmdOrCtrl+M"),
            )?;
            let view = Submenu::with_items(handle, "View", true, &[&mapped])?;
            let menu = Menu::with_items(handle, &[&file, &story, &display, &view])?;

            app.set_menu(menu)?;
            app.manage(Chrome {
                interpreters,
                tandy,
                displays,
                following,
                mapped,
            });

            // The menu only signals; the page owns the flow, since
            // choosing and restarting both end in its reload. A
            // changed claim restarts the open story on the spot:
            // the identity is the booting machine's (§11.1.3), so
            // the checkmark never outruns the header.
            app.on_menu_event(|app, event| match event.id().as_ref() {
                "open" => {
                    let _ = app.emit("menu-open", ());
                }
                "restart" => {
                    let _ = app.emit("menu-restart", ());
                }
                "home" => {
                    let _ = app.emit("menu-home", ());
                }
                "follow" => {
                    // The page owns no flow here: unpin directly,
                    // and set_home rights the checkmark.
                    let _ = app.emit("menu-follow", ());
                }
                "pane:map" => {
                    // The pane opens and closes live: no restart
                    // is owed, and the page re-measures itself
                    // once the story's column has moved over.
                    let shell = app.state::<Shell>();
                    let mut panes = shell.panes.lock().unwrap();

                    panes.map = !panes.map;

                    let standing = panes.clone();

                    drop(panes);
                    save_panes(app, &standing);

                    let _ = app.state::<Chrome>().mapped.set_checked(standing.map);
                    let _ = app.emit("panes", standing);
                }
                "tandy" => {
                    let shell = app.state::<Shell>();
                    let mut claim = shell.claim.lock().unwrap();

                    claim.tandy = !claim.tandy;

                    let _ = app.state::<Chrome>().tandy.set_checked(claim.tandy);
                    drop(claim);

                    let _ = app.emit("menu-restart", ());
                }
                chose if chose.starts_with("claim:") => {
                    let wanted = chose["claim:".len()..].to_string();

                    app.state::<Shell>().claim.lock().unwrap().interpreter = wanted.clone();

                    for (value, item) in &app.state::<Chrome>().interpreters {
                        let _ = item.set_checked(*value == wanted);
                    }

                    let _ = app.emit("menu-restart", ());
                }
                chose if chose.contains(':') => {
                    // A Display choice: dress the state, keep it,
                    // fix the group's checkmarks, and tell the
                    // page -- which re-dresses live, no restart.
                    let (group, value) = chose.split_once(':').expect("the arm found a colon");
                    let shell = app.state::<Shell>();
                    let mut display = shell.display.lock().unwrap();

                    match group {
                        "face" => display.face = value.to_string(),
                        "size" => display.size = value.parse().unwrap_or(15),
                        "theme" => display.theme = value.to_string(),
                        _ => display.measure = value.to_string(),
                    }

                    let settings = display.clone();

                    drop(display);
                    save_display(app, &settings);

                    let fellows = format!("{group}:");

                    for (id, item) in &app.state::<Chrome>().displays {
                        if id.starts_with(&fellows) {
                            let _ = item.set_checked(id == chose);
                        }
                    }

                    let _ = app.emit("display", settings);
                }
                _ => {}
            });

            // The window opens as the pygame glass does: a share
            // of the screen, centered -- placed while hidden, then
            // shown, so it never flashes at the fallback size. The
            // show is unconditional: a screen that cannot be asked
            // still gets the config's own size.
            if let Some(window) = app.get_webview_window("main") {
                if let Ok(Some(monitor)) = window.primary_monitor() {
                    let size = monitor.size();

                    let _ = window.set_size(tauri::PhysicalSize::new(
                        (f64::from(size.width) * SHARE) as u32,
                        (f64::from(size.height) * SHARE) as u32,
                    ));
                    let _ = window.center();
                }

                let _ = window.show();
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            set_story,
            current_story,
            start_session,
            send_stanza,
            display_settings,
            story_home,
            set_home,
            bearings,
            walked_map,
            open_panes
        ])
        .build(tauri::generate_context!())
        .expect("the shell could not be built")
        .run(|app, event| {
            // Exit fires exactly once on every way out; the session
            // is hung up rather than left listening. The serving
            // thread is never joined: a machine spinning inside a
            // story would hold the shell open forever, and the
            // process is going away regardless.
            if let RunEvent::Exit = event {
                drop(app.state::<Shell>().session.lock().unwrap().take());
            }
        });
}
