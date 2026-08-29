//! Glk's object model: windows, streams, filerefs, sound channels.
//!
//! The four opaque classes a game can hold references to (Glk:
//! Opaque Objects) are declared here, along with the constants
//! that describe them. Behavior that reaches across objects --
//! opening windows, dispatching events, a window stream's handoff
//! to its window -- belongs to the api era; this module is the
//! model those functions will operate on.
//!
//! Objects carry no dispatch-layer identity. The 32-bit ids Glulx
//! sees are the bridge era's business. Within the library, objects
//! name each other by the internal keys of the maps they live in
//! -- the port's arena arrangement -- and the operations that walk
//! the window tree take the map as an argument, the way state
//! views do throughout the port. Character buffers are coordinates
//! into VM memory rather than live views, with the memory passed
//! to every operation that touches one: the reference's tests hand
//! in plain lists, but a real interpreter only ever hands in
//! memory, and the port's model says so in its types.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};

use crate::errors::VoxamError;
use crate::glulx::memory::Memory;

/// What a non-Unicode stream substitutes for a character it cannot
/// hold: '?', the placeholder the spec names (Glk: Output).
pub const UNPRINTABLE: u32 = 0x3F;

pub const MAX_UNICODE: u32 = 0x10FFFF;

pub const NEWLINE: u32 = 0x0A;

// The surrogate block: reserved for UTF-16 pairs, so the values
// are not independently encodable characters.
const SURROGATE_FIRST: u32 = 0xD800;
const SURROGATE_LAST: u32 = 0xDFFF;

// One past the last character a byte stream can hold.
const BYTE_LIMIT: u32 = 0x100;

// The UTF-8 lead-byte thresholds: below the first is ASCII, and
// each of the others starts a sequence of that many bytes.
const ASCII_LIMIT: u8 = 0x80;
const LEAD_TWO: u8 = 0xC0;
const LEAD_THREE: u8 = 0xE0;
const LEAD_FOUR: u8 = 0xF0;

/// A window's bounding box: (left, top, right, bottom), in display
/// units.
pub type Box4 = (i64, i64, i64, i64);

/// The map every live window sits in, keyed by internal id.
pub type WindowMap = HashMap<u32, Window>;

fn glk_error(message: String) -> VoxamError {
    VoxamError::GlulxGlk(message)
}

/// Render a Glulx character value as text.
///
/// Glulx characters are arbitrary 32-bit values, so a game can
/// print something that is not a Unicode code point at all --
/// glulxercise does exactly that. Anything outside the Unicode
/// range, and the surrogate block (which is not independently
/// encodable), becomes '?' (Glk: Output).
pub fn to_char(value: u32) -> char {
    if value > MAX_UNICODE || (SURROGATE_FIRST..=SURROGATE_LAST).contains(&value) {
        return '?';
    }

    char::from_u32(value).unwrap_or('?')
}

// -- the constant families --------------------------------------------------

/// The window types (Glk: The Types of Windows). ALL is not a type
/// a window can have: it is the wildcard the gestalt selectors
/// accept when asking about every type at once.
pub mod window_type {
    pub const ALL: u32 = 0;
    pub const PAIR: u32 = 1;
    pub const BLANK: u32 = 2;
    pub const TEXT_BUFFER: u32 = 3;
    pub const TEXT_GRID: u32 = 4;
    pub const GRAPHICS: u32 = 5;
}

/// The split-method bits window_open takes: masked bitfields (Glk:
/// Window Opening, Closing, and Constraints). BORDER shares the
/// value zero with LEFT on purpose.
pub mod window_method {
    pub const LEFT: u32 = 0x00;
    pub const RIGHT: u32 = 0x01;
    pub const ABOVE: u32 = 0x02;
    pub const BELOW: u32 = 0x03;
    pub const DIR_MASK: u32 = 0x0F;

    pub const FIXED: u32 = 0x10;
    pub const PROPORTIONAL: u32 = 0x20;
    pub const DIVISION_MASK: u32 = 0xF0;

    pub const BORDER: u32 = 0x000;
    pub const NO_BORDER: u32 = 0x100;
    pub const BORDER_MASK: u32 = 0x100;
}

/// The event types glk_select can report (Glk: Events).
pub mod event_type {
    pub const NONE: u32 = 0;
    pub const TIMER: u32 = 1;
    pub const CHAR_INPUT: u32 = 2;
    pub const LINE_INPUT: u32 = 3;
    pub const MOUSE_INPUT: u32 = 4;
    pub const ARRANGE: u32 = 5;
    pub const REDRAW: u32 = 6;
    pub const SOUND_NOTIFY: u32 = 7;
    pub const HYPERLINK: u32 = 8;
    pub const VOLUME_NOTIFY: u32 = 9;
}

/// The eleven text styles (Glk: Styles).
pub mod style {
    pub const NORMAL: u32 = 0;
    pub const EMPHASIZED: u32 = 1;
    pub const PREFORMATTED: u32 = 2;
    pub const HEADER: u32 = 3;
    pub const SUBHEADER: u32 = 4;
    pub const ALERT: u32 = 5;
    pub const NOTE: u32 = 6;
    pub const BLOCK_QUOTE: u32 = 7;
    pub const INPUT: u32 = 8;
    pub const USER1: u32 = 9;
    pub const USER2: u32 = 10;
    pub const NUMSTYLES: u32 = 11;
}

/// Where a stream seek measures from (Glk: Stream Positions).
pub mod seek_mode {
    pub const START: u32 = 0;
    pub const CURRENT: u32 = 1;
    pub const END: u32 = 2;
}

/// How a stream is opened (Glk: File Streams).
pub mod file_mode {
    pub const WRITE: u32 = 0x01;
    pub const READ: u32 = 0x02;
    pub const READ_WRITE: u32 = 0x03;
    pub const WRITE_APPEND: u32 = 0x05;
}

/// What a file is for (Glk: The Types of File References). The
/// usage is a masked field, and BINARY_MODE shares the value zero
/// with DATA.
pub mod file_usage {
    pub const DATA: u32 = 0x00;
    pub const SAVED_GAME: u32 = 0x01;
    pub const TRANSCRIPT: u32 = 0x02;
    pub const INPUT_RECORD: u32 = 0x03;
    pub const TYPE_MASK: u32 = 0x0F;

    pub const BINARY_MODE: u32 = 0x000;
    pub const TEXT_MODE: u32 = 0x100;
}

/// The special keys of character input (Glk: Character Input).
///
/// The function keys are not contiguous with END: glk.h leaves
/// 0xFFFFFFF2 through 0xFFFFFFF0 unassigned. MAXVAL is glk.h's own
/// bookkeeping -- the last keycode is 0x100000000 minus this.
pub mod key_code {
    pub const UNKNOWN: u32 = 0xFFFFFFFF;
    pub const LEFT: u32 = 0xFFFFFFFE;
    pub const RIGHT: u32 = 0xFFFFFFFD;
    pub const UP: u32 = 0xFFFFFFFC;
    pub const DOWN: u32 = 0xFFFFFFFB;
    pub const RETURN: u32 = 0xFFFFFFFA;
    pub const DELETE: u32 = 0xFFFFFFF9;
    pub const ESCAPE: u32 = 0xFFFFFFF8;
    pub const TAB: u32 = 0xFFFFFFF7;
    pub const PAGE_UP: u32 = 0xFFFFFFF6;
    pub const PAGE_DOWN: u32 = 0xFFFFFFF5;
    pub const HOME: u32 = 0xFFFFFFF4;
    pub const END: u32 = 0xFFFFFFF3;
    pub const FUNC1: u32 = 0xFFFFFFEF;
    pub const FUNC2: u32 = 0xFFFFFFEE;
    pub const FUNC3: u32 = 0xFFFFFFED;
    pub const FUNC4: u32 = 0xFFFFFFEC;
    pub const FUNC5: u32 = 0xFFFFFFEB;
    pub const FUNC6: u32 = 0xFFFFFFEA;
    pub const FUNC7: u32 = 0xFFFFFFE9;
    pub const FUNC8: u32 = 0xFFFFFFE8;
    pub const FUNC9: u32 = 0xFFFFFFE7;
    pub const FUNC10: u32 = 0xFFFFFFE6;
    pub const FUNC11: u32 = 0xFFFFFFE5;
    pub const FUNC12: u32 = 0xFFFFFFE4;
    pub const MAXVAL: u32 = 28;
}

// -- buffers ----------------------------------------------------------------

/// A character array in VM memory: coordinates, not a copy.
///
/// Holding coordinates and indexing lazily means a retained array
/// -- one Glk keeps after the call returns, such as a pending line
/// request's buffer -- stays valid across a setmemsize that would
/// invalidate a snapshot (the reference's MemArray reasoning,
/// carried whole).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemArray {
    /// Where the array begins.
    pub address: u32,
    /// The element count the call named.
    pub count: u32,
    /// Bytes per element: 1 for the char types, 4 otherwise.
    pub width: u32,
}

impl MemArray {
    /// Read one element.
    pub fn get(&self, memory: &Memory, index: u32) -> Result<u32, VoxamError> {
        memory.read(
            self.address.wrapping_add(index.wrapping_mul(self.width)),
            self.width,
        )
    }

    /// Write one element; the memory layer masks to width.
    pub fn set(&self, memory: &mut Memory, index: u32, value: u32) -> Result<(), VoxamError> {
        memory.write(
            self.address.wrapping_add(index.wrapping_mul(self.width)),
            self.width,
            value,
        )
    }
}

// -- streams ----------------------------------------------------------------

/// The bytes behind a file stream: a real file, or an in-memory
/// cursor -- what a resource stream reads, and what the tests
/// inspect.
#[derive(Debug)]
pub enum FileHandle {
    Real(std::fs::File),
    Bytes(std::io::Cursor<Vec<u8>>),
}

impl FileHandle {
    fn write_bytes(&mut self, data: &[u8]) -> Result<(), VoxamError> {
        let outcome = match self {
            Self::Real(file) => file.write_all(data),
            Self::Bytes(cursor) => cursor.write_all(data),
        };

        outcome.map_err(|error| glk_error(format!("a file stream write failed: {error}")))
    }

    /// Read up to buf's length; the count actually read comes
    /// back.
    fn read_bytes(&mut self, buf: &mut [u8]) -> Result<usize, VoxamError> {
        let mut filled = 0;

        while filled < buf.len() {
            let count = match self {
                Self::Real(file) => file.read(&mut buf[filled..]),
                Self::Bytes(cursor) => cursor.read(&mut buf[filled..]),
            }
            .map_err(|error| glk_error(format!("a file stream read failed: {error}")))?;

            if count == 0 {
                break;
            }

            filled += count;
        }

        Ok(filled)
    }

    fn seek(&mut self, from: SeekFrom) -> Result<u64, VoxamError> {
        let outcome = match self {
            Self::Real(file) => file.seek(from),
            Self::Bytes(cursor) => cursor.seek(from),
        };

        outcome.map_err(|error| glk_error(format!("a file stream seek failed: {error}")))
    }

    fn tell(&mut self) -> Result<u64, VoxamError> {
        self.seek(SeekFrom::Current(0))
    }

    fn flush(&mut self) {
        if let Self::Real(file) = self {
            let _ = file.flush();
        }
    }
}

/// How a stream holds its characters.
#[derive(Debug)]
pub enum StreamKind {
    /// A window's output stream, never readable (Glk: Window
    /// Streams); the window's internal id. Emitted characters are
    /// handed back to the api layer, which owns the window map.
    Window(u32),
    /// A stream over an array in the game's memory (Glk: Memory
    /// Streams). A null buffer is legal: the stream then discards
    /// writes but still counts them, which is how a game measures
    /// output length.
    Memory {
        buf: Option<MemArray>,
        position: u32,
    },
    /// A stream over a file, or over resource bytes (Glk: File
    /// Streams, Glk: Resource Streams). A byte stream holds one
    /// Latin-1 byte per character in either mode; a Unicode stream
    /// holds four-byte big-endian words in binary mode and UTF-8,
    /// with no byte-order mark, in text mode.
    File {
        handle: FileHandle,
        utf8: bool,
        width: u32,
    },
}

/// A stream: a sink, a source, or both (Glk: Streams).
#[derive(Debug)]
pub struct Stream {
    /// The 32-bit value the game filed the stream under (Glk:
    /// Rocks).
    pub rock: u32,
    /// Whether the stream has been destroyed.
    pub disposed: bool,
    /// Whether characters can be read from it.
    pub readable: bool,
    /// Whether characters can be written to it.
    pub writable: bool,
    /// Whether it holds full words; a byte stream substitutes '?'
    /// for anything above 0xFF (Glk: Output).
    pub unicode: bool,
    /// Characters read so far.
    pub readcount: u32,
    /// Characters written so far, discards included.
    pub writecount: u32,
    /// The link value written output belongs to; zero means "not a
    /// link" (Glk: Creating Hyperlinks).
    pub hyperlink: u32,
    /// How the characters are held.
    pub kind: StreamKind,
}

fn mode_readable(fmode: u32) -> bool {
    matches!(fmode, file_mode::READ | file_mode::READ_WRITE)
}

fn mode_writable(fmode: u32) -> bool {
    matches!(
        fmode,
        file_mode::WRITE | file_mode::READ_WRITE | file_mode::WRITE_APPEND
    )
}

impl Stream {
    /// A window's output stream: always writable, always Unicode.
    /// The byte-stream rule is about how a stream *stores*
    /// characters, which for a window is the display's affair;
    /// glkote's glkapi.js sets the same flag on the same object.
    pub fn window(window: u32) -> Self {
        Self {
            rock: 0,
            disposed: false,
            readable: false,
            writable: true,
            unicode: true,
            readcount: 0,
            writecount: 0,
            hyperlink: 0,
            kind: StreamKind::Window(window),
        }
    }

    /// A stream over game memory, in a file mode's directions.
    pub fn memory(buf: Option<MemArray>, fmode: u32, rock: u32, unicode: bool) -> Self {
        Self {
            rock,
            disposed: false,
            readable: mode_readable(fmode),
            writable: mode_writable(fmode),
            unicode,
            readcount: 0,
            writecount: 0,
            hyperlink: 0,
            kind: StreamKind::Memory { buf, position: 0 },
        }
    }

    /// A stream over an open file handle, in a file mode's
    /// directions.
    pub fn file(handle: FileHandle, fmode: u32, rock: u32, unicode: bool, text_mode: bool) -> Self {
        Self {
            rock,
            disposed: false,
            readable: mode_readable(fmode),
            writable: mode_writable(fmode),
            unicode,
            readcount: 0,
            writecount: 0,
            hyperlink: 0,
            kind: StreamKind::File {
                handle,
                utf8: unicode && text_mode,
                width: if unicode && !text_mode { 4 } else { 1 },
            },
        }
    }

    /// Write one character, counting it even if it goes nowhere.
    ///
    /// The write count reported at close must include characters a
    /// stream discards -- "it will count the number of characters
    /// written into the stream, not the number that fit" (Glk:
    /// Memory Streams) -- so it is incremented before any capacity
    /// check. A window stream hands the character back for the api
    /// layer to place: Some means "deliver this to my window".
    pub fn put_char(
        &mut self,
        memory: &mut Memory,
        character: u32,
    ) -> Result<Option<u32>, VoxamError> {
        if !self.writable {
            return Ok(None);
        }

        let character = if !self.unicode && character >= BYTE_LIMIT {
            UNPRINTABLE
        } else {
            character
        };

        self.writecount = self.writecount.wrapping_add(1);

        match &mut self.kind {
            StreamKind::Window(_) => return Ok(Some(character)),
            StreamKind::Memory { buf, position } => {
                // Store within the buffer; advance past its end
                // regardless: the position advancing past the end
                // is what lets a game discover how much output it
                // would have produced.
                if let Some(array) = buf
                    && *position < array.count
                {
                    array.set(memory, *position, character)?;
                }

                *position = position.wrapping_add(1);
            }
            StreamKind::File {
                handle,
                utf8,
                width,
            } => {
                if *utf8 {
                    let mut encoded = [0u8; 4];

                    handle.write_bytes(to_char(character).encode_utf8(&mut encoded).as_bytes())?;
                } else if *width == 1 {
                    handle.write_bytes(&[character as u8])?;
                } else {
                    handle.write_bytes(&character.to_be_bytes())?;
                }
            }
        }

        Ok(None)
    }

    /// Read one character, or -1 at end of stream.
    pub fn get_char(&mut self, memory: &Memory) -> Result<i64, VoxamError> {
        if !self.readable {
            return Ok(-1);
        }

        let value = match &mut self.kind {
            StreamKind::Window(_) => -1,
            StreamKind::Memory { buf, position } => match buf {
                Some(array) if *position < array.count => {
                    let value = array.get(memory, *position)?;
                    *position += 1;

                    i64::from(value)
                }
                _ => -1,
            },
            StreamKind::File {
                handle,
                utf8,
                width,
            } => {
                if *utf8 {
                    read_utf8(handle)?
                } else {
                    let mut data = [0u8; 4];
                    let wanted = *width as usize;
                    let got = handle.read_bytes(&mut data[..wanted])?;

                    if got < wanted {
                        -1
                    } else {
                        i64::from(
                            data[..wanted]
                                .iter()
                                .fold(0u32, |value, byte| value << 8 | u32::from(*byte)),
                        )
                    }
                }
            }
        };

        if value >= 0 {
            self.readcount = self.readcount.wrapping_add(1);
        }

        Ok(value)
    }

    /// Fill a buffer; return how many characters were read. No
    /// terminal null is placed (Glk: How To Read).
    pub fn get_buffer(&mut self, memory: &mut Memory, target: MemArray) -> Result<u32, VoxamError> {
        let mut count = 0;

        for index in 0..target.count {
            let value = self.get_char(memory)?;

            if value < 0 {
                break;
            }

            target.set(memory, index, value as u32)?;
            count = index + 1;
        }

        Ok(count)
    }

    /// Read up to a newline, null-terminating; return the length.
    ///
    /// At most one less than the buffer's capacity is stored, the
    /// newline is kept if one is read, and the result is always
    /// terminated -- the terminal null not counted (Glk: How To
    /// Read).
    pub fn get_line(&mut self, memory: &mut Memory, target: MemArray) -> Result<u32, VoxamError> {
        if target.count == 0 {
            return Ok(0);
        }

        let mut count = 0;

        while count < target.count - 1 {
            let value = self.get_char(memory)?;

            if value < 0 {
                break;
            }

            target.set(memory, count, value as u32)?;
            count += 1;

            if value == i64::from(NEWLINE) {
                break;
            }
        }

        target.set(memory, count, 0)?;

        Ok(count)
    }

    /// The stream's mark; zero where seeking is meaningless.
    pub fn get_position(&mut self) -> Result<u32, VoxamError> {
        match &mut self.kind {
            StreamKind::Window(_) => Ok(0),
            StreamKind::Memory { position, .. } => Ok(*position),
            StreamKind::File { handle, .. } => Ok(handle.tell()? as u32),
        }
    }

    /// Move the mark. Window streams have no position at all (Glk:
    /// Stream Positions), so there it is ignored; a memory stream
    /// clamps to its buffer; an unknown mode on a file measures
    /// from the start.
    pub fn set_position(&mut self, position: i64, mode: u32) -> Result<(), VoxamError> {
        match &mut self.kind {
            StreamKind::Window(_) => Ok(()),
            StreamKind::Memory {
                buf,
                position: mark,
            } => {
                let capacity = i64::from(buf.map_or(0, |array| array.count));
                let sought = match mode {
                    seek_mode::CURRENT => position + i64::from(*mark),
                    seek_mode::END => position + capacity,
                    _ => position,
                };

                *mark = sought.clamp(0, capacity) as u32;

                Ok(())
            }
            StreamKind::File { handle, .. } => {
                let from = match mode {
                    seek_mode::CURRENT => SeekFrom::Current(position),
                    seek_mode::END => SeekFrom::End(position),
                    _ => SeekFrom::Start(position.max(0) as u64),
                };

                handle.seek(from)?;

                Ok(())
            }
        }
    }

    /// Close, answering stream_result_t (Glk: Closing Streams). A
    /// file flushes; its handle is released when the stream leaves
    /// the live list.
    pub fn close(&mut self) -> (u32, u32) {
        self.disposed = true;

        if let StreamKind::File { handle, .. } = &mut self.kind {
            handle.flush();
        }

        (self.readcount, self.writecount)
    }
}

/// Decode one UTF-8 sequence, one byte at a time.
///
/// The length is read off the leading byte rather than decoding
/// the whole file, because a stream may be positioned anywhere and
/// the caller wants exactly one character. Damaged UTF-8 -- a
/// stray continuation byte, or a sequence the file ends in the
/// middle of -- reads as '?' rather than faulting.
fn read_utf8(handle: &mut FileHandle) -> Result<i64, VoxamError> {
    let mut first = [0u8; 1];

    if handle.read_bytes(&mut first)? == 0 {
        return Ok(-1);
    }

    let lead = first[0];

    if lead < ASCII_LIMIT {
        return Ok(i64::from(lead));
    }

    let extra = if lead >= LEAD_FOUR {
        3
    } else if lead >= LEAD_THREE {
        2
    } else if lead >= LEAD_TWO {
        1
    } else {
        // A stray continuation byte.
        return Ok(i64::from(UNPRINTABLE));
    };

    let mut sequence = [0u8; 4];
    sequence[0] = lead;

    let got = handle.read_bytes(&mut sequence[1..1 + extra])?;

    match std::str::from_utf8(&sequence[..1 + got]) {
        Ok(text) => match text.chars().next() {
            Some(character) => Ok(i64::from(u32::from(character))),
            None => Ok(i64::from(UNPRINTABLE)),
        },
        Err(_) => Ok(i64::from(UNPRINTABLE)),
    }
}

// -- windows ----------------------------------------------------------------

/// A pending line request on a window (Glk: Line Input Events).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineRequest {
    /// The buffer the line lands in, or None.
    pub buf: Option<MemArray>,
    /// How many characters of it are pre-filled.
    pub initlen: u32,
    /// Whether the buffer holds words rather than bytes.
    pub unicode: bool,
    /// Whether the finished line is echoed to the window (Glk:
    /// Line Input Events, via set_echo_line_event).
    pub echo: bool,
    /// The special keys that may end the line.
    pub terminators: Vec<u32>,
}

impl LineRequest {
    /// Record what the request asked for.
    pub fn new(buf: Option<MemArray>, initlen: u32, unicode: bool) -> Self {
        Self {
            buf,
            initlen,
            unicode,
            echo: true,
            terminators: Vec::new(),
        }
    }

    /// The buffer's length; zero for the null buffer.
    pub fn capacity(&self) -> u32 {
        self.buf.map_or(0, |array| array.count)
    }
}

/// What a text window costs in the display's own layout unit.
///
/// The window tree is arranged in display units. A terminal's unit
/// *is* the character cell, so its metrics are 1x1 and every
/// measurement is the same number either way. The cell is a float
/// because a display may measure one -- GlkOte says so outright --
/// and the margins are what a window spends on padding and
/// borders, over and above its characters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub width: f64,
    pub height: f64,
    pub margin_x: f64,
    pub margin_y: f64,
}

impl Metrics {
    pub const fn new(width: f64, height: f64, margin_x: f64, margin_y: f64) -> Self {
        Self {
            width,
            height,
            margin_x,
            margin_y,
        }
    }
}

/// The metrics of a display whose unit is already the character.
pub const CHARACTER_CELL: Metrics = Metrics::new(1.0, 1.0, 0.0, 0.0);

/// How many characters fit an extent, margin taken out.
///
/// Rounded down, for the same reason the other direction rounds
/// up: a window claiming a column it does not have room for spills
/// over its own edge.
fn cells(extent: i64, cell: f64, margin: f64) -> i64 {
    if cell > 0.0 {
        (((extent as f64 - margin) / cell).trunc() as i64).max(0)
    } else {
        0
    }
}

/// A span of window text sharing one style and link value, a
/// picture set into the flow, or a flow break (Glk: Graphics in
/// Text Buffer Windows).
#[derive(Debug, Clone, PartialEq)]
pub enum Flow {
    Run {
        style: u32,
        hyperlink: u32,
        text: String,
    },
    Placed(Placed),
    Break,
}

/// One picture set into a buffer's text flow: the Pict's number,
/// the picture whole as a data: url, the size the draw asked for,
/// the §imagealign value naming how the text meets it, and the
/// link value it was drawn under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub image: u32,
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub alignment: u32,
    pub hyperlink: u32,
}

/// The contents a text buffer window accumulates until a display
/// drains them.
#[derive(Debug, Default)]
pub struct BufferData {
    /// Runs, pictures, and breaks, oldest first.
    pub content: Vec<Flow>,
}

/// A character grid with a cursor (Glk: Text Grid Windows). The
/// characters, their styles, and their link values are held as
/// parallel row lists, resized whenever the layout hands over a
/// new box.
#[derive(Debug, Default)]
pub struct GridData {
    pub lines: Vec<Vec<char>>,
    pub styles: Vec<Vec<u32>>,
    pub links: Vec<Vec<u32>>,
    pub cursor_x: i64,
    pub cursor_y: i64,
}

/// A grid of pixels (Glk: Graphics Windows). The pixels themselves
/// live in the display; the model holds the box and the requests.
#[derive(Debug, Default)]
pub struct GraphicsData {
    /// Raised by a rearrange that changed a real box: the canvas
    /// cleared, and the game is owed a redraw event (Glk: Window
    /// Events).
    pub moved: bool,
}

/// An internal node: a split of two (Glk: Window Arrangement).
#[derive(Debug)]
pub struct PairData {
    /// The window on the split's unconstrained side -- the
    /// original window, until a re-arrangement flips the direction
    /// and swaps the children.
    pub child1: u32,
    /// The window on the side the direction names, which carries
    /// the size constraint -- the split-off window, at first.
    pub child2: u32,
    /// The window the split's size is *measured* against. Only the
    /// measurement: the constraint sits on child2's side wherever
    /// the key lives (Glk: Changing Window Constraints).
    pub key: u32,
    /// The split's size, in the key window's units, or as a
    /// percentage for a proportional split.
    pub size: u32,
    /// The box the constrained side received, kept for displays
    /// that draw borders.
    pub sized_box: Box4,
    pub direction: u32,
    pub division: u32,
    pub has_border: bool,
    pub vertical: bool,
    pub backward: bool,
}

impl PairData {
    /// Join two windows under a split method.
    pub fn new(child1: u32, child2: u32, key: u32, method: u32, size: u32) -> Self {
        let mut pair = Self {
            child1,
            child2,
            key,
            size,
            sized_box: (0, 0, 0, 0),
            direction: 0,
            division: 0,
            has_border: true,
            vertical: false,
            backward: false,
        };

        pair.set_method(method);

        pair
    }

    /// Unpack a method word into the split's parts.
    pub fn set_method(&mut self, method: u32) {
        self.direction = method & window_method::DIR_MASK;
        self.division = method & window_method::DIVISION_MASK;
        self.has_border = (method & window_method::BORDER_MASK) == window_method::BORDER;
        self.vertical = matches!(self.direction, window_method::LEFT | window_method::RIGHT);
        self.backward = matches!(self.direction, window_method::LEFT | window_method::ABOVE);
    }

    /// The split's parts recomposed into a method word.
    pub fn method(&self) -> u32 {
        let border = if self.has_border {
            window_method::BORDER
        } else {
            window_method::NO_BORDER
        };

        self.direction | self.division | border
    }
}

/// How a window holds its contents.
#[derive(Debug)]
pub enum WindowKind {
    /// A window that is always blank (Glk: Blank Windows).
    Blank,
    /// A scrolling text window (Glk: Text Buffer Windows).
    Buffer(BufferData),
    /// A character grid with a cursor (Glk: Text Grid Windows).
    Grid(GridData),
    /// A grid of pixels (Glk: Graphics Windows).
    Graphics(GraphicsData),
    /// An internal node: a split of two (Glk: Window Arrangement).
    Pair(PairData),
}

/// A window, of whatever kind (Glk: The Types of Windows).
#[derive(Debug)]
pub struct Window {
    /// The 32-bit value the game filed the window under (Glk:
    /// Rocks).
    pub rock: u32,
    /// Whether the window has been destroyed.
    pub disposed: bool,
    /// The pair window this hangs under, or None at the root.
    pub parent: Option<u32>,
    /// The internal id of the window's own output stream; wired by
    /// the api layer when the window opens.
    pub stream: u32,
    /// A stream that receives a copy of the window's output, or
    /// None (Glk: Echo Streams).
    pub echo_stream: Option<u32>,
    /// The style new output is written in.
    pub style: u32,
    /// The pending line request, or None.
    pub line_request: Option<LineRequest>,
    /// Whether character input is requested.
    pub char_request: bool,
    /// Whether the character request wants words.
    pub char_unicode: bool,
    /// Whether a hyperlink click is requested.
    pub hyperlink_request: bool,
    /// Whether a mouse click is requested.
    pub mouse_request: bool,
    /// The display's cell measurements for this window.
    pub metrics: Metrics,
    /// Set by clear, cleared by a display once it redraws.
    pub pending_clear: bool,
    /// (left, top, right, bottom), in display units.
    pub bbox: Box4,
    /// How the contents are held.
    pub kind: WindowKind,
}

impl Window {
    /// Open unattached, with nothing requested. A graphics window
    /// opens asking to be cleared: a fresh canvas is background
    /// (Glk: Graphics Windows).
    pub fn new(kind: WindowKind, rock: u32) -> Self {
        let pending_clear = matches!(kind, WindowKind::Graphics(_));

        Self {
            rock,
            disposed: false,
            parent: None,
            stream: 0,
            echo_stream: None,
            style: style::NORMAL,
            line_request: None,
            char_request: false,
            char_unicode: false,
            hyperlink_request: false,
            mouse_request: false,
            metrics: CHARACTER_CELL,
            pending_clear,
            bbox: (0, 0, 0, 0),
            kind,
        }
    }

    /// The window's type number (Glk: The Types of Windows).
    pub fn wintype(&self) -> u32 {
        match self.kind {
            WindowKind::Blank => window_type::BLANK,
            WindowKind::Buffer(_) => window_type::TEXT_BUFFER,
            WindowKind::Grid(_) => window_type::TEXT_GRID,
            WindowKind::Graphics(_) => window_type::GRAPHICS,
            WindowKind::Pair(_) => window_type::PAIR,
        }
    }

    /// The window's width in its own measurement system: cells for
    /// a text window, pixels for a canvas -- and zero for a blank
    /// or a pair, which have no measurement system at all (Glk:
    /// Blank Windows, Glk: Changing Window Constraints).
    pub fn width(&self) -> i64 {
        match self.kind {
            WindowKind::Blank | WindowKind::Pair(_) => 0,
            WindowKind::Graphics(_) => (self.bbox.2 - self.bbox.0).max(0),
            WindowKind::Buffer(_) | WindowKind::Grid(_) => cells(
                self.bbox.2 - self.bbox.0,
                self.metrics.width,
                self.metrics.margin_x,
            ),
        }
    }

    /// The window's height in its own measurement system.
    pub fn height(&self) -> i64 {
        match self.kind {
            WindowKind::Blank | WindowKind::Pair(_) => 0,
            WindowKind::Graphics(_) => (self.bbox.3 - self.bbox.1).max(0),
            WindowKind::Buffer(_) | WindowKind::Grid(_) => cells(
                self.bbox.3 - self.bbox.1,
                self.metrics.height,
                self.metrics.margin_y,
            ),
        }
    }

    /// Display units needed for a size in this window's units.
    ///
    /// A fixed split is expressed in the key window's measurement
    /// system (Glk: Window Opening, Closing, and Constraints):
    /// characters plus margin for a text window -- rounded up, or
    /// a window a fraction of a pixel short would push its last
    /// line past its own border -- and the size itself elsewhere.
    pub fn extent(&self, size: u32, vertical: bool) -> i64 {
        match self.kind {
            WindowKind::Buffer(_) | WindowKind::Grid(_) => {
                let cell = if vertical {
                    self.metrics.width
                } else {
                    self.metrics.height
                };
                let margin = if vertical {
                    self.metrics.margin_x
                } else {
                    self.metrics.margin_y
                };

                (f64::from(size) * cell + margin).ceil() as i64
            }
            _ => i64::from(size),
        }
    }

    /// Hold a character from this window's stream, written in the
    /// window's current style under a link value. The copy to any
    /// echo stream is the api layer's share, since it crosses
    /// objects.
    pub fn put_char(&mut self, character: u32, hyperlink: u32) {
        match &mut self.kind {
            WindowKind::Buffer(data) => {
                // Append to the last run, or start a new one: a
                // run continues only while both the style and the
                // link value hold -- and only across text, since a
                // placed picture or a flow break ends the run it
                // follows.
                let glyph = to_char(character);

                match data.content.last_mut() {
                    Some(Flow::Run {
                        style: run_style,
                        hyperlink: run_link,
                        text,
                    }) if *run_style == self.style && *run_link == hyperlink => {
                        text.push(glyph);
                    }
                    _ => data.content.push(Flow::Run {
                        style: self.style,
                        hyperlink,
                        text: glyph.to_string(),
                    }),
                }
            }
            WindowKind::Grid(data) => {
                // Write at the cursor and advance (Glk: Text Grid
                // Windows): a newline moves to the start of the
                // next row and prints nothing, the right edge
                // wraps, and anything landing outside the grid is
                // dropped.
                let width = cells(
                    self.bbox.2 - self.bbox.0,
                    self.metrics.width,
                    self.metrics.margin_x,
                );
                let height = cells(
                    self.bbox.3 - self.bbox.1,
                    self.metrics.height,
                    self.metrics.margin_y,
                );

                if character == NEWLINE {
                    data.cursor_x = 0;
                    data.cursor_y += 1;

                    return;
                }

                if data.cursor_x >= width {
                    data.cursor_x = 0;
                    data.cursor_y += 1;
                }

                if (0..height).contains(&data.cursor_y) && (0..width).contains(&data.cursor_x) {
                    let (row, column) = (data.cursor_y as usize, data.cursor_x as usize);

                    data.lines[row][column] = to_char(character);
                    data.styles[row][column] = self.style;
                    data.links[row][column] = hyperlink;
                }

                data.cursor_x += 1;
            }
            // The blank window supports no output; a canvas and a
            // pair take none; the discard still counted upstream.
            _ => {}
        }
    }

    /// Erase the window's contents. What a display holds -- a
    /// buffer's scrollback, a canvas's pixels -- can only be asked
    /// for, so the flag is raised for it; what the model holds is
    /// erased here.
    pub fn clear(&mut self) {
        self.pending_clear = true;

        match &mut self.kind {
            WindowKind::Buffer(data) => data.content.clear(),
            WindowKind::Grid(data) => {
                for row in &mut data.lines {
                    row.fill(' ');
                }

                data.cursor_x = 0;
                data.cursor_y = 0;
            }
            _ => {}
        }
    }

    /// The accumulated text of a buffer window, styles and
    /// pictures flattened away; empty elsewhere.
    pub fn text(&self) -> String {
        match &self.kind {
            WindowKind::Buffer(data) => data
                .content
                .iter()
                .filter_map(|flow| match flow {
                    Flow::Run { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect(),
            _ => String::new(),
        }
    }

    /// Return a buffer window's accumulated text and reset, for a
    /// display to render.
    pub fn take_text(&mut self) -> String {
        let out = self.text();

        if let WindowKind::Buffer(data) = &mut self.kind {
            data.content.clear();
        }

        out
    }

    /// Return a buffer window's accumulated flow and reset,
    /// keeping styles -- the same drain as take_text, for a
    /// display that renders styles rather than flattening them.
    pub fn take_content(&mut self) -> Vec<Flow> {
        match &mut self.kind {
            WindowKind::Buffer(data) => std::mem::take(&mut data.content),
            _ => Vec::new(),
        }
    }

    /// A grid window's rows, one string each; empty elsewhere.
    pub fn rows(&self) -> Vec<String> {
        match &self.kind {
            WindowKind::Grid(data) => data.lines.iter().map(|row| row.iter().collect()).collect(),
            _ => Vec::new(),
        }
    }

    /// Put a grid's cursor where the game asks. Past-the-edge
    /// positions are legal: output there falls into the void until
    /// the cursor comes back inside.
    pub fn move_cursor(&mut self, x: i64, y: i64) {
        if let WindowKind::Grid(data) = &mut self.kind {
            data.cursor_x = x;
            data.cursor_y = y;
        }
    }

    /// Set a picture into a buffer's flow, after everything
    /// written so far.
    pub fn put_placed(&mut self, placed: Placed) {
        if let WindowKind::Buffer(data) = &mut self.kind {
            data.content.push(Flow::Placed(placed));
        }
    }

    /// Set a flow break into a buffer's flow (Glk: Graphics in
    /// Text Buffer Windows).
    pub fn put_break(&mut self) {
        if let WindowKind::Buffer(data) = &mut self.kind {
            data.content.push(Flow::Break);
        }
    }

    /// Take a new bounding box, resizing what the box measures: a
    /// grid regrows its rows, a canvas whose real box changed
    /// loses its pixels and owes a redraw.
    fn take_box(&mut self, bbox: Box4) {
        // The spec allows a resized window's contents to be thrown
        // away so long as the game hears a redraw event (Glk:
        // Window Events). A fresh window whose old box was empty
        // owes no such event: it opens as background and the game
        // knows it -- measured against the old box, before the new
        // one lands.
        if matches!(self.kind, WindowKind::Graphics(_)) && bbox != self.bbox {
            let had_pixels = self.width() > 0 && self.height() > 0;

            self.pending_clear = true;

            if let WindowKind::Graphics(data) = &mut self.kind {
                data.moved = data.moved || had_pixels;
            }
        }

        self.bbox = bbox;

        if let WindowKind::Grid(_) = self.kind {
            let width = self.width();
            let height = self.height();

            if let WindowKind::Grid(data) = &mut self.kind {
                resize_grid(data, width, height);
            }
        }
    }
}

/// Grow or trim a grid's rows, keeping what still fits; the cursor
/// is clamped into the new bounds.
fn resize_grid(data: &mut GridData, width: i64, height: i64) {
    let (width, height) = (width.max(0) as usize, height.max(0) as usize);

    let stretch = |rows: &mut Vec<Vec<char>>, blank: char| {
        rows.resize(height, Vec::new());

        for row in rows.iter_mut() {
            row.resize(width, blank);
        }
    };

    stretch(&mut data.lines, ' ');

    data.styles.resize(height, Vec::new());
    data.links.resize(height, Vec::new());

    for row in &mut data.styles {
        row.resize(width, style::NORMAL);
    }

    for row in &mut data.links {
        row.resize(width, 0);
    }

    data.cursor_x = data.cursor_x.min(width as i64);
    data.cursor_y = data.cursor_y.min(height as i64);
}

/// Lay a window and its subtree out over a bounding box.
///
/// The reference's Window.rearrange, walking the arena instead of
/// an object graph. A pair splits the box between its children:
/// the box is in display units, a proportional split is a
/// percentage, and a fixed one is expressed in the *key window's*
/// measurement system (Glk: Window Opening, Closing, and
/// Constraints). The direction decides the sides outright: child2
/// sits on the named side and takes the split's size, however deep
/// the key window has since been buried.
pub fn rearrange(windows: &mut WindowMap, id: u32, bbox: Box4) {
    let Some(window) = windows.get_mut(&id) else {
        return;
    };

    window.take_box(bbox);

    let WindowKind::Pair(pair) = &window.kind else {
        return;
    };

    let (child1, child2, key) = (pair.child1, pair.child2, pair.key);
    let (division, size, vertical, backward) =
        (pair.division, pair.size, pair.vertical, pair.backward);

    let (left, top, right, bottom) = bbox;
    let extent = if vertical { right - left } else { bottom - top };

    let split = if division == window_method::PROPORTIONAL {
        // Python's floor division, which f64 or plain / would get
        // wrong for negative extents.
        (extent * i64::from(size)).div_euclid(100)
    } else {
        windows
            .get(&key)
            .map_or(i64::from(size), |held| held.extent(size, vertical))
    };

    let split = split.clamp(0, extent.max(0));

    // How much of the extent the first box gets; the second box
    // takes the rest.
    let first = if backward { split } else { extent - split };

    let (box1, box2) = if vertical {
        let middle = left + first;

        ((left, top, middle, bottom), (middle, top, right, bottom))
    } else {
        let middle = top + first;

        ((left, top, right, middle), (left, middle, right, bottom))
    };

    let (sized_box, other_box) = if backward { (box1, box2) } else { (box2, box1) };

    if let Some(window) = windows.get_mut(&id)
        && let WindowKind::Pair(pair) = &mut window.kind
    {
        pair.sized_box = sized_box;
    }

    rearrange(windows, child2, sized_box);
    rearrange(windows, child1, other_box);
}

/// A window and all its descendants, by id.
pub fn subtree(windows: &WindowMap, id: u32) -> Vec<u32> {
    let mut found = vec![id];

    if let Some(window) = windows.get(&id)
        && let WindowKind::Pair(pair) = &window.kind
    {
        found.extend(subtree(windows, pair.child1));
        found.extend(subtree(windows, pair.child2));
    }

    found
}

// -- other opaque classes ---------------------------------------------------

/// A reference to a file (Glk: File References).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    /// The 32-bit value the game filed the reference under.
    pub rock: u32,
    /// Whether the reference has been destroyed.
    pub disposed: bool,
    /// The path the reference names.
    pub filename: String,
    /// What the file is for, masked to the type bits.
    pub usage: u32,
    /// Whether the file opens in text mode.
    pub text_mode: bool,
    /// Whether the file dies with the reference.
    pub temporary: bool,
}

impl FileRef {
    /// Record what the file is and how it is meant to open.
    pub fn new(filename: String, usage: u32, rock: u32, temporary: bool) -> Self {
        Self {
            rock,
            disposed: false,
            filename,
            usage: usage & file_usage::TYPE_MASK,
            text_mode: usage & file_usage::TEXT_MODE != 0,
            temporary,
        }
    }
}

/// A sound channel (Glk: Sound).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundChannel {
    /// The 32-bit value the game filed the channel under.
    pub rock: u32,
    /// Whether the channel has been destroyed.
    pub disposed: bool,
    /// As a fraction of 0x10000, which is full volume (Glk: Other
    /// Sound Channel Functions).
    pub volume: u32,
    /// The resource number playing, or 0 for silence.
    pub sound: u32,
    /// How many plays were asked for.
    pub repeats: u32,
    /// The nonzero value a finished play reports with.
    pub notify: u32,
    /// Whether the channel is paused.
    pub paused: bool,
}

impl SoundChannel {
    /// Open silent, at the volume asked for.
    pub fn new(volume: u32, rock: u32) -> Self {
        Self {
            rock,
            disposed: false,
            volume,
            sound: 0,
            repeats: 0,
            notify: 0,
            paused: false,
        }
    }
}

// -- events -----------------------------------------------------------------

/// One Glk event: the four fields of event_t (Glk: Events).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    /// The event type -- event_t calls this field "type", which
    /// Rust spells better as something else.
    pub kind: u32,
    /// The internal id of the window the event belongs to, or
    /// None.
    pub window: Option<u32>,
    /// The first value; meaning depends on the type.
    pub val1: u32,
    /// The second value.
    pub val2: u32,
}

impl Event {
    /// Build an event.
    pub fn new(kind: u32, window: Option<u32>, val1: u32, val2: u32) -> Self {
        Self {
            kind,
            window,
            val1,
            val2,
        }
    }

    /// The event that means "nothing happened".
    pub fn none() -> Self {
        Self::new(event_type::NONE, None, 0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glulx::story::Story;

    /// A memory whose RAM holds the tests' buffers.
    fn ram() -> Memory {
        let mut data = vec![0u8; 256];
        data[..4].copy_from_slice(b"Glul");
        data[4..8].copy_from_slice(&0x0003_0102u32.to_be_bytes());
        data[8..12].copy_from_slice(&256u32.to_be_bytes());
        data[12..16].copy_from_slice(&256u32.to_be_bytes());
        data[16..20].copy_from_slice(&0x300u32.to_be_bytes());
        data[20..24].copy_from_slice(&256u32.to_be_bytes());

        Memory::new(&Story::new(data).unwrap())
    }

    fn bytes_at(address: u32, count: u32) -> MemArray {
        MemArray {
            address,
            count,
            width: 1,
        }
    }

    fn words_at(address: u32, count: u32) -> MemArray {
        MemArray {
            address,
            count,
            width: 4,
        }
    }

    fn grid(width: i64, height: i64) -> Window {
        let mut window = Window::new(WindowKind::Grid(GridData::default()), 0);

        window.take_box((0, 0, width, height));

        window
    }

    fn put_all(stream: &mut Stream, memory: &mut Memory, values: impl IntoIterator<Item = u32>) {
        for value in values {
            stream.put_char(memory, value).unwrap();
        }
    }

    // A Glulx character is an arbitrary 32-bit value, so a game
    // can print something that is not a code point at all --
    // glulxercise does. Anything past the Unicode range, and the
    // surrogate block, renders as '?' (Glk: Output).
    #[test]
    fn unprintable_values_render_as_question_marks() {
        assert_eq!(to_char(0x41), 'A');
        assert_eq!(to_char(0x110000), '?');
        assert_eq!(to_char(0xD800), '?');
    }

    // A stream only moves characters in its declared directions: a
    // write to an unwritable stream is not even counted, and a
    // read from an unreadable one is end-of-stream.
    #[test]
    fn streams_enforce_their_directions() {
        let mut memory = ram();
        let mut silent = Stream::memory(Some(bytes_at(0x180, 2)), file_mode::READ, 0, false);
        let mut deaf = Stream::window(1);

        silent.put_char(&mut memory, 0x41).unwrap();
        deaf.put_char(&mut memory, 0x41).unwrap();

        assert_eq!(silent.writecount, 0);
        assert_eq!(deaf.writecount, 1);
        assert_eq!(deaf.get_char(&memory).unwrap(), -1);

        // Seeking is meaningless on a window stream; the mark just
        // answers 0.
        deaf.set_position(5, seek_mode::START).unwrap();

        assert_eq!(deaf.get_position().unwrap(), 0);
        assert_eq!(deaf.close(), (0, 1));
        assert!(deaf.disposed);
    }

    // A byte stream substitutes '?' for anything it cannot hold; a
    // Unicode stream holds the full word (Glk: Output).
    #[test]
    fn byte_streams_substitute_what_they_cannot_hold() {
        let mut memory = ram();
        let mut narrow = Stream::memory(Some(bytes_at(0x180, 2)), file_mode::WRITE, 0, false);
        let mut wide = Stream::memory(Some(words_at(0x190, 2)), file_mode::WRITE, 0, true);

        put_all(&mut narrow, &mut memory, [0x41, 0x2603]);
        put_all(&mut wide, &mut memory, [0x41, 0x2603]);

        assert_eq!(memory.read_run(0x180, 2).unwrap(), [0x41, 0x3F]);
        assert_eq!(memory.read_word(0x190).unwrap(), 0x41);
        assert_eq!(memory.read_word(0x194).unwrap(), 0x2603);
    }

    // "It will count the number of characters written into the
    // stream, not the number that fit in the buffer" (Glk: Memory
    // Streams) -- and a null buffer is the legal extreme,
    // discarding everything while counting it, which is how a game
    // measures output length.
    #[test]
    fn write_counts_include_what_overflowed() {
        let mut memory = ram();
        let mut short = Stream::memory(Some(bytes_at(0x180, 1)), file_mode::WRITE, 0, false);
        let mut null = Stream::memory(None, file_mode::WRITE, 0, false);

        put_all(&mut short, &mut memory, [0x61, 0x62, 0x63]);
        put_all(&mut null, &mut memory, [0x61, 0x62, 0x63]);

        assert_eq!(memory.read_byte(0x180).unwrap(), 0x61);
        assert_eq!(short.writecount, 3);
        assert_eq!(short.get_position().unwrap(), 3);
        assert_eq!(null.writecount, 3);

        // And a null buffer reads as instant end-of-stream.
        let mut hollow = Stream::memory(None, file_mode::READ, 0, false);

        assert_eq!(hollow.get_char(&memory).unwrap(), -1);
    }

    // get_buffer fills until the buffer or the stream runs out, no
    // terminal null placed (Glk: How To Read).
    #[test]
    fn get_buffer_fills_until_something_runs_out() {
        let mut memory = ram();

        memory.write_run(0x180, &[0x61, 0x62, 0x63]).unwrap();
        memory.write_run(0x1A0, &[9, 9, 9, 9, 9]).unwrap();

        let mut source = Stream::memory(Some(bytes_at(0x180, 3)), file_mode::READ, 0, false);

        assert_eq!(
            source.get_buffer(&mut memory, bytes_at(0x1A0, 5)).unwrap(),
            3
        );
        assert_eq!(memory.read_run(0x1A0, 5).unwrap(), [0x61, 0x62, 0x63, 9, 9]);
        assert_eq!(source.readcount, 3);

        memory.write_run(0x190, &[0x64, 0x65]).unwrap();

        let mut refill = Stream::memory(Some(bytes_at(0x190, 2)), file_mode::READ, 0, false);

        assert_eq!(
            refill.get_buffer(&mut memory, bytes_at(0x1B0, 2)).unwrap(),
            2
        );
        assert_eq!(memory.read_run(0x1B0, 2).unwrap(), [0x64, 0x65]);
    }

    // get_line reads until len-1 characters or a newline, keeps
    // the newline, and always null-terminates, the null not
    // counted (Glk: How To Read).
    #[test]
    fn get_line_keeps_the_newline_and_terminates() {
        let mut memory = ram();

        memory.write_run(0x180, &[0x61, 0x62, 0x0A, 0x63]).unwrap();
        memory.write_run(0x1A0, &[9, 9, 9, 9]).unwrap();

        let mut source = Stream::memory(Some(bytes_at(0x180, 4)), file_mode::READ, 0, false);

        assert_eq!(source.get_line(&mut memory, bytes_at(0x1A0, 4)).unwrap(), 3);
        assert_eq!(memory.read_run(0x1A0, 4).unwrap(), [0x61, 0x62, 0x0A, 0]);

        // The stream ending mid-line terminates what was read.
        memory.write_run(0x1B0, &[9, 9, 9, 9]).unwrap();

        assert_eq!(source.get_line(&mut memory, bytes_at(0x1B0, 4)).unwrap(), 1);
        assert_eq!(memory.read_run(0x1B0, 4).unwrap(), [0x63, 0, 9, 9]);

        // A full buffer stops one short to leave room for the
        // null.
        memory.write_run(0x1C0, &[9, 9]).unwrap();
        memory.write_run(0x190, &[0x64, 0x65, 0x66]).unwrap();

        let mut long = Stream::memory(Some(bytes_at(0x190, 3)), file_mode::READ, 0, false);

        assert_eq!(long.get_line(&mut memory, bytes_at(0x1C0, 2)).unwrap(), 1);
        assert_eq!(memory.read_run(0x1C0, 2).unwrap(), [0x64, 0]);

        // A zero-capacity buffer reads nothing at all.
        assert_eq!(long.get_line(&mut memory, bytes_at(0x1D0, 0)).unwrap(), 0);
    }

    // The mark seeks from the start, the current position, or the
    // end, and clamps to the buffer either way (Glk: Stream
    // Positions).
    #[test]
    fn memory_streams_seek_and_clamp() {
        let mut memory = ram();

        memory.write_run(0x180, &[0x61, 0x62, 0x63, 0x64]).unwrap();

        let mut stream = Stream::memory(Some(bytes_at(0x180, 4)), file_mode::READ_WRITE, 0, false);

        stream.set_position(2, seek_mode::START).unwrap();

        assert_eq!(stream.get_char(&memory).unwrap(), 0x63);

        stream.set_position(-2, seek_mode::CURRENT).unwrap();

        assert_eq!(stream.get_char(&memory).unwrap(), 0x62);

        stream.set_position(-1, seek_mode::END).unwrap();

        assert_eq!(stream.get_char(&memory).unwrap(), 0x64);
        assert_eq!(stream.get_char(&memory).unwrap(), -1);

        stream.set_position(-10, seek_mode::START).unwrap();

        assert_eq!(stream.get_position().unwrap(), 0);

        stream.set_position(10, seek_mode::START).unwrap();

        assert_eq!(stream.get_position().unwrap(), 4);

        // WriteAppend is a writable mode for the stream flags,
        // even though opening a memory stream with it is the api's
        // to refuse.
        let appender = Stream::memory(Some(bytes_at(0x190, 1)), file_mode::WRITE_APPEND, 0, false);

        assert!(appender.writable);
        assert!(!appender.readable);
    }

    // A window's stream is never readable and hands back what it
    // is given, for the api layer to land in the window (Glk:
    // Window Streams).
    #[test]
    fn window_streams_hand_characters_back() {
        let mut memory = ram();
        let mut stream = Stream::window(7);

        assert!(!stream.readable);
        assert!(stream.unicode);
        assert_eq!(stream.put_char(&mut memory, 0x61).unwrap(), Some(0x61));
        assert_eq!(stream.writecount, 1);
    }

    // A line request records what was asked; the null buffer has
    // no capacity.
    #[test]
    fn line_requests_hold_what_was_asked() {
        let mut request = LineRequest::new(Some(bytes_at(0x180, 3)), 1, true);
        request.echo = false;

        let hollow = LineRequest::new(None, 0, false);

        assert_eq!(request.capacity(), 3);
        assert_eq!(request.initlen, 1);
        assert!(request.unicode);
        assert!(!request.echo);
        assert!(request.terminators.is_empty());
        assert_eq!(hollow.capacity(), 0);
        assert!(hollow.echo);
    }

    // A text window's size is its box divided by the display's
    // cell, margins off the top, rounded down -- claiming a column
    // that does not fit would spill over the window's own edge.
    #[test]
    fn text_windows_measure_in_cells() {
        let mut window = Window::new(WindowKind::Buffer(BufferData::default()), 0);

        window.metrics = Metrics::new(10.0, 16.0, 4.0, 2.0);

        window.take_box((0, 0, 104, 66));

        assert_eq!(window.width(), 10);
        assert_eq!(window.height(), 4);

        // A zero cell -- a display that has not measured --
        // answers no room at all rather than dividing by it.
        window.metrics = Metrics::new(0.0, 0.0, 0.0, 0.0);

        assert_eq!(window.width(), 0);

        // A box smaller than the margin clamps to zero, not
        // negative.
        window.metrics = Metrics::new(10.0, 16.0, 200.0, 2.0);

        assert_eq!(window.width(), 0);
    }

    // The reverse conversion rounds up: a fixed split a fraction
    // of a pixel short would push its last line past its own
    // border. A canvas's units are already display units, so it
    // converts by doing nothing.
    #[test]
    fn extents_round_up_for_the_split() {
        let mut window = Window::new(WindowKind::Grid(GridData::default()), 0);

        window.metrics = Metrics::new(10.4, 16.0, 4.0, 2.0);

        assert_eq!(window.extent(3, true), 36);
        assert_eq!(window.extent(2, false), 34);

        let mut pixels = Window::new(WindowKind::Graphics(GraphicsData::default()), 0);

        pixels.take_box((10, 10, 74, 58));

        assert_eq!(pixels.extent(50, true), 50);
        assert_eq!(pixels.width(), 64);
        assert_eq!(pixels.height(), 48);
    }

    // A fresh canvas asks to be cleared -- its background starts
    // white -- and owes no redraw, since it opens as background
    // and the game knows it. A canvas whose real box changed lost
    // its pixels: it asks to be cleared again and owes the game a
    // redraw event (Glk: Window Events).
    #[test]
    fn a_canvas_clears_on_open_and_on_loss() {
        let mut window = Window::new(WindowKind::Graphics(GraphicsData::default()), 0);

        let moved = |window: &Window| match &window.kind {
            WindowKind::Graphics(data) => data.moved,
            _ => unreachable!(),
        };

        assert!(window.pending_clear);
        assert!(!moved(&window));

        window.take_box((0, 0, 64, 48));

        assert!(!moved(&window));

        window.pending_clear = false;
        window.take_box((0, 0, 64, 48));

        assert!(!window.pending_clear);
        assert!(!moved(&window));

        window.take_box((10, 10, 74, 58));

        assert!(window.pending_clear);
        assert!(moved(&window));
    }

    // A degenerate box never reports a negative size.
    #[test]
    fn boxes_clamp_at_nothing() {
        let mut window = Window::new(WindowKind::Graphics(GraphicsData::default()), 0);

        window.take_box((10, 10, 4, 4));

        assert_eq!(window.width(), 0);
        assert_eq!(window.height(), 0);
    }

    // "A blank window has no size; glk_window_get_size() will
    // return (0,0)" (Glk: Blank Windows) -- and a pair window is a
    // split, not a place. The box stays, because a display draws
    // borders from it; zero is only what the game is told.
    #[test]
    fn sizeless_windows_answer_zero_with_a_real_box() {
        let mut blank = Window::new(WindowKind::Blank, 0);

        blank.take_box((0, 0, 80, 24));

        assert_eq!(blank.width(), 0);
        assert_eq!(blank.height(), 0);
        assert_eq!(blank.bbox, (0, 0, 80, 24));
        assert_eq!(blank.wintype(), window_type::BLANK);

        // The base clear has nothing to erase; it can only raise
        // the flag for a display to act on.
        blank.clear();

        assert!(blank.pending_clear);
    }

    // Buffer text accumulates as runs: a run continues only while
    // both the style and the link value hold (Glk: Creating
    // Hyperlinks).
    #[test]
    fn buffer_runs_split_on_style_and_link() {
        let mut window = Window::new(WindowKind::Buffer(BufferData::default()), 0);

        window.put_char(0x61, 0);
        window.put_char(0x62, 0);

        window.style = style::EMPHASIZED;

        window.put_char(0x63, 0);

        window.style = style::NORMAL;

        window.put_char(0x64, 7);

        let content = match &window.kind {
            WindowKind::Buffer(data) => data.content.clone(),
            _ => unreachable!(),
        };

        assert_eq!(
            content,
            [
                Flow::Run {
                    style: style::NORMAL,
                    hyperlink: 0,
                    text: "ab".into()
                },
                Flow::Run {
                    style: style::EMPHASIZED,
                    hyperlink: 0,
                    text: "c".into()
                },
                Flow::Run {
                    style: style::NORMAL,
                    hyperlink: 7,
                    text: "d".into()
                },
            ]
        );
        assert_eq!(window.text(), "abcd");

        // The drains hand everything over exactly once.
        assert_eq!(window.take_content().len(), 3);
        assert_eq!(window.text(), "");

        window.put_char(0x65, 0);

        assert_eq!(window.take_text(), "e");
        assert_eq!(window.text(), "");

        window.put_char(0x66, 0);

        window.clear();

        assert_eq!(window.text(), "");
        assert!(window.pending_clear);
    }

    // A placed picture or a flow break ends the run it follows:
    // text after one starts a fresh run even in the same dress,
    // the flattening drains skip past them, and take_content hands
    // the whole flow over in order (Glk: Graphics in Text Buffer
    // Windows).
    #[test]
    fn the_flow_carries_placed_pictures_and_breaks() {
        let mut window = Window::new(WindowKind::Buffer(BufferData::default()), 0);
        let placed = Placed {
            image: 3,
            url: "data:".into(),
            width: 4,
            height: 5,
            alignment: 1,
            hyperlink: 0,
        };

        window.put_char(0x61, 0);
        window.put_char(0x62, 0);
        window.put_placed(placed.clone());
        window.put_char(0x63, 0);
        window.put_char(0x64, 0);
        window.put_break();
        window.put_char(0x65, 0);
        window.put_char(0x66, 0);

        assert_eq!(window.text(), "abcdef");

        let flow = window.take_content();

        assert_eq!(flow.len(), 5);
        assert_eq!(
            flow[0],
            Flow::Run {
                style: style::NORMAL,
                hyperlink: 0,
                text: "ab".into()
            }
        );
        assert_eq!(flow[1], Flow::Placed(placed));
        assert!(matches!(flow[3], Flow::Break));
    }

    // The grid writes at the cursor and advances, wraps at the
    // right edge, treats newline as a cursor drop that prints
    // nothing, and drops what lands outside entirely (Glk: Text
    // Grid Windows).
    #[test]
    fn grids_write_wrap_and_drop() {
        let mut window = grid(3, 2);

        for character in [0x61, 0x62, 0x63, 0x64] {
            window.put_char(character, 0);
        }

        assert_eq!(window.rows(), ["abc", "d  "]);

        window.move_cursor(0, 1);
        window.put_char(0x0A, 0);

        for character in "lost".bytes() {
            window.put_char(u32::from(character), 0);
        }

        assert_eq!(window.rows(), ["abc", "d  "]);

        // A negative cursor is equally out of the grid.
        window.move_cursor(-2, 0);
        window.put_char(0x7A, 0);

        assert_eq!(window.rows(), ["abc", "d  "]);
    }

    // Each grid cell keeps the style and link it was written
    // under.
    #[test]
    fn grids_keep_styles_and_links_per_cell() {
        let mut window = grid(2, 1);

        window.style = style::HEADER;

        window.put_char(0x61, 5);

        let data = match &window.kind {
            WindowKind::Grid(data) => data,
            _ => unreachable!(),
        };

        assert_eq!(data.styles[0], [style::HEADER, style::NORMAL]);
        assert_eq!(data.links[0], [5, 0]);
    }

    // Rearranging a grid keeps what still fits; the cursor is
    // clamped into the new bounds.
    #[test]
    fn grids_resize_keeping_what_fits() {
        let mut window = grid(4, 2);

        for character in "abcdef".bytes() {
            window.put_char(u32::from(character), 0);
        }

        window.take_box((0, 0, 6, 3));

        assert_eq!(window.rows(), ["abcd  ", "ef    ", "      "]);

        window.move_cursor(5, 2);

        window.take_box((0, 0, 2, 1));

        assert_eq!(window.rows(), ["ab"]);

        let (x, y) = match &window.kind {
            WindowKind::Grid(data) => (data.cursor_x, data.cursor_y),
            _ => unreachable!(),
        };

        assert_eq!((x, y), (2, 1));

        window.clear();

        assert_eq!(window.rows(), ["  "]);
    }

    // A pair window unpacks its method word and can recompose it.
    #[test]
    fn pair_methods_unpack_and_recompose() {
        let mut pair = PairData::new(1, 2, 2, window_method::LEFT | window_method::FIXED, 10);

        assert!(pair.vertical);
        assert!(pair.backward);
        assert!(pair.has_border);
        assert_eq!(pair.method(), window_method::LEFT | window_method::FIXED);

        pair.set_method(
            window_method::BELOW | window_method::PROPORTIONAL | window_method::NO_BORDER,
        );

        assert!(!pair.vertical);
        assert!(!pair.backward);
        assert!(!pair.has_border);
        assert_eq!(
            pair.method(),
            window_method::BELOW | window_method::PROPORTIONAL | window_method::NO_BORDER
        );
    }

    /// Stand a two-window tree in a map: the original at 1, the
    /// split-off at 2, the pair at 3.
    fn tree(
        method: u32,
        size: u32,
        original: WindowKind,
        added: WindowKind,
        key: u32,
    ) -> WindowMap {
        let mut windows = WindowMap::new();

        windows.insert(1, Window::new(original, 0));
        windows.insert(2, Window::new(added, 0));

        let mut pair = Window::new(WindowKind::Pair(PairData::new(1, 2, key, method, size)), 0);

        pair.parent = None;

        windows.insert(3, pair);

        windows
    }

    // A proportional split takes its percentage of the extent; the
    // split-off child sits on the named side, the original takes
    // the rest.
    #[test]
    fn proportional_splits_divide_by_percentage() {
        let mut windows = tree(
            window_method::ABOVE | window_method::PROPORTIONAL,
            25,
            WindowKind::Buffer(BufferData::default()),
            WindowKind::Grid(GridData::default()),
            2,
        );

        rearrange(&mut windows, 3, (0, 0, 80, 24));

        assert_eq!(windows[&2].bbox, (0, 0, 80, 6));
        assert_eq!(windows[&1].bbox, (0, 6, 80, 24));
        assert_eq!(windows[&3].width(), 0);

        match &windows[&3].kind {
            WindowKind::Pair(pair) => assert_eq!(pair.sized_box, (0, 0, 80, 6)),
            _ => unreachable!(),
        }
    }

    // A fixed split is expressed in the key window's measurement
    // system (Glk: Window Opening, Closing, and Constraints): the
    // key converts characters to display units, and the split
    // lands below when the direction says so.
    #[test]
    fn fixed_splits_measure_by_the_key_window() {
        let mut windows = tree(
            window_method::BELOW | window_method::FIXED,
            3,
            WindowKind::Buffer(BufferData::default()),
            WindowKind::Grid(GridData::default()),
            2,
        );

        rearrange(&mut windows, 3, (0, 0, 80, 24));

        assert_eq!(windows[&2].bbox, (0, 21, 80, 24));
        assert_eq!(windows[&1].bbox, (0, 0, 80, 21));

        // A split larger than the box clamps to the box.
        if let WindowKind::Pair(pair) = &mut windows.get_mut(&3).unwrap().kind {
            pair.size = 100;
        }

        rearrange(&mut windows, 3, (0, 0, 80, 24));

        assert_eq!(windows[&2].bbox, (0, 0, 80, 24));
        assert_eq!(windows[&1].bbox, (0, 0, 80, 0));
    }

    // The key window only supplies the measurement system: the
    // sized side is always the one the direction names -- child2's
    // side -- even when the key lives on the other side, which the
    // spec's own worked example does on purpose (Glk: Changing
    // Window Constraints).
    #[test]
    fn the_key_only_measures() {
        let mut windows = tree(
            window_method::LEFT | window_method::FIXED,
            5,
            WindowKind::Grid(GridData::default()),
            WindowKind::Buffer(BufferData::default()),
            1,
        );

        rearrange(&mut windows, 3, (0, 0, 80, 24));

        assert_eq!(windows[&2].bbox, (0, 0, 5, 24));
        assert_eq!(windows[&1].bbox, (5, 0, 80, 24));
    }

    // The subtree walk names a window and every descendant.
    #[test]
    fn subtrees_collect_descendants() {
        let windows = tree(
            window_method::LEFT | window_method::FIXED,
            5,
            WindowKind::Grid(GridData::default()),
            WindowKind::Buffer(BufferData::default()),
            1,
        );

        assert_eq!(subtree(&windows, 3), [3, 1, 2]);
        assert_eq!(subtree(&windows, 1), [1]);
    }

    // A fileref records what the file is for, keeping only the
    // type bits of the usage word (Glk: The Types of File
    // References); the rock is a 32-bit value, and a channel opens
    // silent at the volume asked for.
    #[test]
    fn filerefs_and_channels_record_their_making() {
        let saved = FileRef::new(
            "story.glksave".into(),
            file_usage::SAVED_GAME | file_usage::TEXT_MODE,
            0,
            false,
        );
        let scratch = FileRef::new("notes.glkdata".into(), file_usage::DATA, 0, true);

        assert_eq!(saved.usage, file_usage::SAVED_GAME);
        assert!(saved.text_mode);
        assert!(!saved.temporary);
        assert!(!scratch.text_mode);
        assert!(scratch.temporary);

        let channel = SoundChannel::new(0x10000, 0x2345_6789);

        assert_eq!(channel.rock, 0x2345_6789);
        assert_eq!(channel.volume, 0x10000);
        assert_eq!(channel.sound, 0);
        assert_eq!(channel.repeats, 0);
        assert_eq!(channel.notify, 0);
        assert!(!channel.paused);
        assert!(!channel.disposed);
    }

    // A byte file stream holds one Latin-1 byte per character in
    // either mode (Glk: File Streams); what a byte cannot hold was
    // already substituted upstream.
    #[test]
    fn byte_file_streams_hold_latin_1() {
        let mut memory = ram();
        let handle = FileHandle::Bytes(std::io::Cursor::new(Vec::new()));
        let mut stream = Stream::file(handle, file_mode::READ_WRITE, 0, false, false);

        put_all(&mut stream, &mut memory, [0x61, 0x62, 0x2603]);

        if let StreamKind::File {
            handle: FileHandle::Bytes(cursor),
            ..
        } = &stream.kind
        {
            assert_eq!(cursor.get_ref(), b"ab?");
        } else {
            unreachable!();
        }

        stream.set_position(0, seek_mode::START).unwrap();

        assert_eq!(stream.get_char(&memory).unwrap(), 0x61);

        stream.set_position(-1, seek_mode::END).unwrap();

        assert_eq!(stream.get_char(&memory).unwrap(), 0x3F);
        assert_eq!(stream.get_char(&memory).unwrap(), -1);

        stream.set_position(1, seek_mode::START).unwrap();
        stream.set_position(1, seek_mode::CURRENT).unwrap();

        assert_eq!(stream.get_position().unwrap(), 2);

        // An unknown seek mode measures from the start.
        stream.set_position(0, 9).unwrap();

        assert_eq!(stream.get_position().unwrap(), 0);
    }

    // A Unicode file stream in binary mode is four-byte big-endian
    // words (Glk: File Streams).
    #[test]
    fn unicode_binary_file_streams_hold_words() {
        let mut memory = ram();
        let handle = FileHandle::Bytes(std::io::Cursor::new(Vec::new()));
        let mut stream = Stream::file(handle, file_mode::READ_WRITE, 0, true, false);

        stream.put_char(&mut memory, 0x1F600).unwrap();

        if let StreamKind::File {
            handle: FileHandle::Bytes(cursor),
            ..
        } = &stream.kind
        {
            assert_eq!(cursor.get_ref(), &[0x00, 0x01, 0xF6, 0x00]);
        } else {
            unreachable!();
        }

        stream.set_position(0, seek_mode::START).unwrap();

        assert_eq!(stream.get_char(&memory).unwrap(), 0x1F600);
        assert_eq!(stream.get_char(&memory).unwrap(), -1);
    }

    // A Unicode file stream in text mode is UTF-8 with no
    // byte-order mark (Glk: File Streams) -- which is what makes
    // an ASCII file byte-identical to one written through the byte
    // functions.
    #[test]
    fn unicode_text_file_streams_hold_utf8() {
        let mut memory = ram();
        let handle = FileHandle::Bytes(std::io::Cursor::new(Vec::new()));
        let mut stream = Stream::file(handle, file_mode::READ_WRITE, 0, true, true);

        put_all(&mut stream, &mut memory, [0x41, 0xE9, 0x2603, 0x1F600]);

        if let StreamKind::File {
            handle: FileHandle::Bytes(cursor),
            ..
        } = &stream.kind
        {
            assert_eq!(cursor.get_ref(), b"A\xc3\xa9\xe2\x98\x83\xf0\x9f\x98\x80");
        } else {
            unreachable!();
        }

        stream.set_position(0, seek_mode::START).unwrap();

        assert_eq!(stream.get_char(&memory).unwrap(), 0x41);
        assert_eq!(stream.get_char(&memory).unwrap(), 0xE9);
        assert_eq!(stream.get_char(&memory).unwrap(), 0x2603);
        assert_eq!(stream.get_char(&memory).unwrap(), 0x1F600);
        assert_eq!(stream.get_char(&memory).unwrap(), -1);
    }

    // Damaged UTF-8 -- a stray continuation byte, or a sequence
    // the file ends in the middle of -- reads as '?' rather than
    // faulting: a position is anywhere a game seeks, so a
    // mid-sequence start must be survivable.
    #[test]
    fn damaged_utf8_reads_as_question_marks() {
        let memory = ram();
        let stray = FileHandle::Bytes(std::io::Cursor::new(b"\x83A".to_vec()));
        let mut stream = Stream::file(stray, file_mode::READ, 0, true, true);

        assert_eq!(stream.get_char(&memory).unwrap(), 0x3F);
        assert_eq!(stream.get_char(&memory).unwrap(), 0x41);

        let cut = FileHandle::Bytes(std::io::Cursor::new(b"\xe2\x98".to_vec()));
        let mut short = Stream::file(cut, file_mode::READ, 0, true, true);

        assert_eq!(short.get_char(&memory).unwrap(), 0x3F);
    }

    // An event defaults to "nothing happened" (Glk: Events).
    #[test]
    fn events_default_to_nothing_happened() {
        let quiet = Event::none();
        let typed = Event::new(event_type::CHAR_INPUT, Some(4), 0x61, 0);

        assert_eq!(
            (quiet.kind, quiet.window, quiet.val1, quiet.val2),
            (0, None, 0, 0)
        );
        assert_eq!(
            (typed.kind, typed.window, typed.val1, typed.val2),
            (event_type::CHAR_INPUT, Some(4), 0x61, 0)
        );
    }
}
