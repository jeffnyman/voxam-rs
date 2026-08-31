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

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use voxam_core::glkote::json::{Object, Value, dumps, loads};
use voxam_core::session::{Played, Sitting};

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

/// The request surface, socket-free and testable whole.
///
/// Every route answers as (status, content type, payload); the
/// handler shell below only carries those onto the wire.
pub struct Face {
    /// The sitting every event lands on.
    pub session: Sitting,
    /// The mark the browser tab wears: each machine's own window
    /// icon, exactly as the reference title bars wear them.
    pub icon: String,
    /// The page title, the story's own name when one is known.
    pub caption: String,
}

impl Face {
    /// Front one session, under the story's own name.
    pub fn new(session: Sitting, caption: Option<&str>) -> Self {
        let icon = match session.played() {
            Played::Z { version } => format!("z{version}.ico"),
            // The third machine borrows the Glulx mark until it
            // earns its own -- a named road, not an oversight.
            Played::Glulx | Played::Aa => "glulx.ico".to_string(),
        };

        Self {
            session,
            icon,
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
                    icon(&self.icon).to_vec(),
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
            .and_then(|number| self.session.picture(number));

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
