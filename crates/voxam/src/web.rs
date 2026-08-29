//! The browser face: GlkOte served over HTTP, one turn per POST.
//!
//! The same seams --glkote speaks over stdio, spoken over the wire
//! the display library itself was designed for: "the Game.accept
//! call is a single HTTP request -- and the data structure is a
//! single HTTP response" (GlkOte: The Application's Life Story).
//! The server is hand-rolled on the standard library's listener,
//! single-threaded on purpose -- one story, one session, every
//! request in its turn -- and the page it serves carries the
//! vendored GlkOte display, compiled into the binary the way the
//! window icons are.
//!
//! A browser reload sends a fresh init, and a fresh init rebuilds
//! the whole session from the already-parsed story: reloading the
//! page restarts the game, which is exactly what a reload should
//! mean.
//!
//! One departure from the reference, recorded: the reference
//! catches Ctrl+C for a clean zero exit; here the process ends the
//! platform's own way, the server having nothing to flush.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::rc::Rc;

use voxam_core::aamachine::glkote::{
    GlkOteFrontend as AaFrontend, Verdict as AaVerdict, WireVoice,
};
use voxam_core::aamachine::machine::{Machine as AaMachine, Wait};
use voxam_core::aamachine::story::Story as AaStory;
use voxam_core::blorb::{Blorb, PNG_ID};
use voxam_core::errors::VoxamError;
use voxam_core::glkote::json::{Object, Value, dumps, loads};
use voxam_core::glulx::glk::glkote::{
    Accepted, GlkOteFrontend as GlulxFrontend, opened as glulx_opened,
};
use voxam_core::glulx::glk::resources::Resources;
use voxam_core::glulx::machine::Machine as GlulxMachine;
use voxam_core::glulx::story::Story as GlulxStory;
use voxam_core::zmachine::glkote::{Session as ZWire, Verdict as ZVerdict, fronted as z_fronted};
use voxam_core::zmachine::story::Story as ZStory;

const HTTP_OK: u16 = 200;
const HTTP_NOT_FOUND: u16 = 404;

const PICT_ROAD: &str = "/pict/";

/// What each shipped page file is, on the wire -- the license
/// rides in the binary's source tree but is nobody's fetch.
const PAGES: [(&str, &str, &[u8]); 6] = [
    (
        "index.html",
        "text/html; charset=utf-8",
        include_bytes!("../pages/index.html"),
    ),
    (
        "glkote.css",
        "text/css",
        include_bytes!("../pages/glkote.css"),
    ),
    (
        "glkote.js",
        "text/javascript",
        include_bytes!("../pages/glkote.js"),
    ),
    (
        "voxam-audio.js",
        "text/javascript",
        include_bytes!("../pages/voxam-audio.js"),
    ),
    (
        "jquery-1.12.4.min.js",
        "text/javascript",
        include_bytes!("../pages/jquery-1.12.4.min.js"),
    ),
    (
        "waiting.gif",
        "image/gif",
        include_bytes!("../pages/waiting.gif"),
    ),
];

/// The window icons, the same files the reference's title bars
/// wear.
const ICONS: [(&str, &[u8]); 9] = [
    ("glulx.ico", include_bytes!("../icons/glulx.ico")),
    ("z1.ico", include_bytes!("../icons/z1.ico")),
    ("z2.ico", include_bytes!("../icons/z2.ico")),
    ("z3.ico", include_bytes!("../icons/z3.ico")),
    ("z4.ico", include_bytes!("../icons/z4.ico")),
    ("z5.ico", include_bytes!("../icons/z5.ico")),
    ("z6.ico", include_bytes!("../icons/z6.ico")),
    ("z7.ico", include_bytes!("../icons/z7.ico")),
    ("z8.ico", include_bytes!("../icons/z8.ico")),
];

fn page(name: &str) -> Option<(&'static str, &'static [u8])> {
    PAGES
        .iter()
        .find(|(held, _, _)| *held == name)
        .map(|(_, kind, bytes)| (*kind, *bytes))
}

fn icon(name: &str) -> &'static [u8] {
    ICONS
        .iter()
        .find(|(held, _)| *held == name)
        .map_or(&[], |(_, bytes)| *bytes)
}

fn error_stanza(message: &str) -> Object {
    let mut stanza = Object::new();

    stanza.set("type", "error");
    stanza.set("message", message);

    stanza
}

fn pass_stanza() -> Object {
    let mut stanza = Object::new();

    stanza.set("type", "pass");

    stanza
}

/// The answer to an event before any init has spoken.
fn unopened() -> Object {
    error_stanza("voxam: the conversation opens with an init event")
}

/// One story's life behind the server, init to exit.
///
/// Every init event -- the page's first breath, and every reload
/// after -- builds a fresh frontend, library, and machine from the
/// parsed story; the resources rebuild from the kept Blorb, their
/// image cache being pure memoization. A session whose machine
/// faulted stays faulted, answering the same error until a reload
/// starts over.
pub struct Session {
    /// The mark the browser tab wears: each machine's own window
    /// icon, exactly as the reference title bars wear them.
    pub icon: String,
    resources: RefCell<Resources>,
    blorb: Option<Blorb>,
    seed: Option<u32>,
    fault: Option<Object>,
    kind: Kind,
}

#[allow(clippy::large_enum_variant)] // three machines, one seat each
enum Kind {
    Glulx {
        story: GlulxStory,
        live: Option<(GlulxMachine, Rc<RefCell<GlulxFrontend>>)>,
    },
    Z {
        story: ZStory,
        live: Option<ZWire>,
    },
    Aa {
        story: AaStory,
        live: Option<(AaFrontend, AaMachine<WireVoice>)>,
    },
}

impl Session {
    /// A Glulx story behind the server, over the Glk library.
    pub fn glulx(story: GlulxStory, blorb: Option<Blorb>, seed: Option<u32>) -> Self {
        Self {
            icon: "glulx.ico".to_string(),
            resources: RefCell::new(Resources::new(blorb.clone())),
            blorb,
            seed,
            fault: None,
            kind: Kind::Glulx { story, live: None },
        }
    }

    /// A Z-Machine story behind the server, over the screen model.
    pub fn z(story: ZStory, blorb: Option<Blorb>, seed: Option<u32>) -> Self {
        Self {
            icon: format!("z{}.ico", story.version()),
            resources: RefCell::new(Resources::new(blorb.clone())),
            blorb,
            seed,
            fault: None,
            kind: Kind::Z { story, live: None },
        }
    }

    /// An Å-machine story behind the server, over the wire face.
    ///
    /// The third machine borrows the Glulx mark until it earns its
    /// own -- a named road, not an oversight.
    pub fn aamachine(story: AaStory, blorb: Option<Blorb>, seed: Option<u32>) -> Self {
        Self {
            icon: "glulx.ico".to_string(),
            resources: RefCell::new(Resources::new(blorb.clone())),
            blorb,
            seed,
            fault: None,
            kind: Kind::Aa { story, live: None },
        }
    }

    /// One event in, one stanza out: the burst model's turn.
    ///
    /// An init rebuilds the session; anything else lands on the
    /// machine standing suspended. A fault answers as the
    /// protocol's own error stanza and keeps answering so until
    /// the next init.
    pub fn answer(&mut self, stanza: &Object) -> Object {
        if stanza.get("type").and_then(Value::as_str) == Some("init") {
            self.fault = None;

            return match self.reborn(stanza) {
                Ok(update) => update,
                Err(error) => self.faulted(&error),
            };
        }

        if let Some(fault) = &self.fault {
            return fault.clone();
        }

        match self.delivered(stanza) {
            Ok(update) => update,
            Err(error) => self.faulted(&error),
        }
    }

    fn faulted(&mut self, error: &VoxamError) -> Object {
        let stanza = error_stanza(&format!("voxam: {error}"));

        self.fault = Some(stanza.clone());

        stanza
    }

    /// One Blorb picture by number, with its content type.
    pub fn pict(&self, number: u32) -> Option<(&'static str, Vec<u8>)> {
        let mut resources = self.resources.borrow_mut();
        let found = resources.image(number)?;
        let kind = if found.kind == PNG_ID {
            "image/png"
        } else {
            "image/jpeg"
        };

        Some((kind, found.data.clone()))
    }

    /// Start the story over, fresh objects from the kept story.
    fn reborn(&mut self, stanza: &Object) -> Result<Object, VoxamError> {
        match &mut self.kind {
            Kind::Glulx { story, live } => {
                let (machine, face) = glulx_opened(story.clone(), self.blorb.clone(), self.seed)?;

                face.borrow_mut().begin(stanza)?;
                *live = Some((machine, face));

                let Some((machine, face)) = live else {
                    unreachable!("just installed");
                };

                turned_glulx(machine, face)
            }
            Kind::Z { story, live } => {
                let mut frontend =
                    z_fronted(story.version(), Some(Resources::new(self.blorb.clone())))?;

                frontend.begin(stanza)?;

                let mut session = ZWire::open(story.clone(), frontend, self.seed)?;
                let update = turned_z(&mut session)?;

                *live = Some(session);

                Ok(update)
            }
            Kind::Aa { story, live } => {
                let mut frontend = AaFrontend::new(story);
                let mut voice = WireVoice::new(story)?;

                frontend.begin(&mut voice, stanza)?;

                let mut machine = AaMachine::new(story.clone(), voice, self.seed)?;

                frontend.waiting = Some(machine.run(None)?);

                let exit = frontend.waiting == Some(Wait::Quit);
                let update = frontend.render(&mut machine.voice, exit)?;

                *live = Some((frontend, machine));

                Ok(update)
            }
        }
    }

    /// Deliver one event to the suspended machine and run on.
    fn delivered(&mut self, stanza: &Object) -> Result<Object, VoxamError> {
        match &mut self.kind {
            Kind::Glulx { live, .. } => {
                let Some((machine, face)) = live else {
                    return Ok(unopened());
                };

                let verdict = {
                    let (glk, memory) = attached(machine)?;

                    face.borrow_mut().accept(glk, memory, stanza)?
                };

                match verdict {
                    Accepted::Event(event) => {
                        machine.deliver_event(event)?;

                        turned_glulx(machine, face)
                    }
                    Accepted::File(name) => {
                        // The stanza itself completed the wait: a
                        // file answer stores through the parked
                        // call.
                        machine.deliver_file(name.as_deref())?;

                        turned_glulx(machine, face)
                    }
                    Accepted::Nothing => {
                        let cleared = machine.glk_mut().is_none_or(|glk| glk.waiting.is_none());

                        if cleared {
                            return turned_glulx(machine, face);
                        }

                        Ok(pass_stanza())
                    }
                }
            }
            Kind::Z { live, .. } => {
                let Some(session) = live else {
                    return Ok(unopened());
                };

                match session.accept(stanza)? {
                    ZVerdict::Advance => turned_z(session),
                    ZVerdict::Stand => session.render(false),
                    ZVerdict::Pass => Ok(pass_stanza()),
                }
            }
            Kind::Aa { live, .. } => {
                let Some((frontend, machine)) = live else {
                    return Ok(unopened());
                };

                match frontend.accept(machine, stanza)? {
                    AaVerdict::Advance => {
                        let exit = frontend.waiting == Some(Wait::Quit);

                        frontend.render(&mut machine.voice, exit)
                    }
                    AaVerdict::Stand => frontend.render(&mut machine.voice, false),
                    AaVerdict::Pass => Ok(pass_stanza()),
                }
            }
        }
    }
}

/// The machine's library and memory, both in hand.
fn attached(
    machine: &mut GlulxMachine,
) -> Result<
    (
        &mut voxam_core::glulx::glk::api::Glk,
        &mut voxam_core::glulx::memory::Memory,
    ),
    VoxamError,
> {
    machine
        .glk_and_memory_mut()
        .ok_or_else(|| VoxamError::GlkOte("the display is not attached to a library".into()))
}

/// Run the Glulx machine to its next wait and render the update.
fn turned_glulx(
    machine: &mut GlulxMachine,
    face: &Rc<RefCell<GlulxFrontend>>,
) -> Result<Object, VoxamError> {
    machine.run(None)?;

    let running = machine.running();
    let (glk, memory) = attached(machine)?;

    face.borrow_mut().render(glk, memory, !running)
}

/// Run the Z machine to its next wait and render the update.
fn turned_z(session: &mut ZWire) -> Result<Object, VoxamError> {
    session.machine().run()?;

    let running = session.machine().running();

    session.render(!running)
}

/// The request surface, socket-free and testable whole.
///
/// Every route answers as (status, content type, payload); the
/// handler shell below only carries those onto the wire.
pub struct Face {
    /// The session every event lands on.
    pub session: Session,
    /// The page title, the story's own name when one is known.
    pub caption: String,
}

impl Face {
    /// Front one session, under the story's own name.
    pub fn new(session: Session, caption: Option<&str>) -> Self {
        Self {
            session,
            caption: caption.unwrap_or("Voxam").to_string(),
        }
    }

    /// Answer one request, whatever road it asks for.
    pub fn respond(&mut self, method: &str, path: &str, body: &[u8]) -> (u16, String, Vec<u8>) {
        if method == "POST" && path == "/event" {
            return self.event(body);
        }

        if method == "GET" {
            if path == "/" {
                return self.index();
            }

            if path == "/favicon.ico" {
                return (
                    HTTP_OK,
                    "image/x-icon".to_string(),
                    icon(&self.session.icon).to_vec(),
                );
            }

            let name = path.trim_start_matches('/');

            if name != "index.html"
                && let Some((kind, bytes)) = page(name)
            {
                return (HTTP_OK, kind.to_string(), bytes.to_vec());
            }

            if let Some(tail) = path.strip_prefix(PICT_ROAD) {
                return self.pict(tail);
            }
        }

        (
            HTTP_NOT_FOUND,
            "text/plain".to_string(),
            b"voxam: no such road".to_vec(),
        )
    }

    /// The page itself, wearing the story's name.
    fn index(&self) -> (u16, String, Vec<u8>) {
        let (kind, bytes) = page("index.html").expect("the page ships");
        let told = String::from_utf8_lossy(bytes).replace("VOXAM_TITLE", &self.caption);

        (HTTP_OK, kind.to_string(), told.into_bytes())
    }

    /// One Blorb picture by number; a placeholder is no picture.
    fn pict(&mut self, tail: &str) -> (u16, String, Vec<u8>) {
        let found = tail
            .parse::<u32>()
            .ok()
            .filter(|_| tail.chars().all(|held| held.is_ascii_digit()))
            .and_then(|number| self.session.pict(number));

        match found {
            Some((kind, bytes)) => (HTTP_OK, kind.to_string(), bytes),
            None => (
                HTTP_NOT_FOUND,
                "text/plain".to_string(),
                b"voxam: no such picture".to_vec(),
            ),
        }
    }

    /// One turn: the event in the body, the update in the answer.
    ///
    /// Even what is not JSON answers 200 with the protocol's error
    /// stanza -- the display renders that far better than a bare
    /// status ever would.
    fn event(&mut self, body: &[u8]) -> (u16, String, Vec<u8>) {
        let answered = match std::str::from_utf8(body)
            .map_err(|error| error.to_string())
            .and_then(|text| loads(text).map_err(|error| error.to_string()))
        {
            Err(error) => error_stanza(&format!("voxam: not JSON: {error}")),
            Ok(Value::Object(stanza)) => self.session.answer(&stanza),
            Ok(_) => error_stanza("voxam: a stanza is a JSON object"),
        };

        (
            HTTP_OK,
            "application/json".to_string(),
            dumps(&Value::Object(answered)).into_bytes(),
        )
    }
}

/// A listener for one Face, bound to localhost and ready to run.
pub fn webbed(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", port))
}

/// Serve one session until the player is done.
///
/// The player stops the server with Ctrl+C, which ends the process
/// the platform's way -- the reference catches it for a zero exit,
/// a courtesy the departure note above records.
pub fn serve_web(face: &mut Face, listener: &TcpListener) {
    let port = listener
        .local_addr()
        .map(|held| held.port())
        .unwrap_or_default();

    println!(
        "voxam: serving {} at http://127.0.0.1:{port} (Ctrl+C to stop)",
        face.caption
    );

    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            continue;
        };

        let _ = handled(face, stream);
    }
}

/// One request off the socket, one response back on.
fn handled(face: &mut Face, stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream);
    let mut request = String::new();

    reader.read_line(&mut request)?;

    let mut pieces = request.split_whitespace();
    let method = pieces.next().unwrap_or_default().to_string();
    let path = pieces.next().unwrap_or_default().to_string();
    let mut length = 0usize;

    loop {
        let mut header = String::new();

        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }

        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }

    let mut body = vec![0u8; length];

    reader.read_exact(&mut body)?;

    let (status, kind, payload) = face.respond(&method, &path, &body);
    let reason = if status == HTTP_OK { "OK" } else { "Not Found" };
    let mut stream = reader.into_inner();

    // Without an explicit answer the browser caches assets
    // heuristically, and a tab can keep serving last week's
    // display against this week's server -- a mismatch that reads
    // as mystery breakage, never as staleness.
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        payload.len()
    )?;
    stream.write_all(&payload)?;
    stream.flush()
}

#[cfg(test)]
#[path = "web_tests.rs"]
mod tests;
