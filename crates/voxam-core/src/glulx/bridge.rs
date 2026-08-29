//! The VM/Glk seam: argument marshalling and the object registry.
//!
//! This is the only module that reads both sides: Glulx sees
//! opaque Glk objects as 32-bit ids and passes references as
//! addresses, while the library sees arena keys and fills holders.
//! The glk opcode hands over a selector and a list of raw words;
//! everything here is about turning those into a library call and
//! writing the answers back into VM memory or onto the stack, by
//! the rules the spec spells out under the glk opcode (Glulx:
//! Miscellaneous).
//!
//! Where the reference defers work as closures, the port defers it
//! as data: a write-back is an argument index, a sink, and the
//! signature item that knows how to encode it, and a suspended
//! call parks its argument holders whole. The library's disposal
//! callback is likewise drained as a report list after each call.
//! The call itself is one match over every selector -- the
//! reference's getattr, spelled out.

use std::collections::HashMap;

use crate::errors::VoxamError;
use crate::glulx::glk::api::{Glk, Held, Outcome, RefSlot, Stop, StructSlot, Waiting};
use crate::glulx::glk::dispatch::{self, CLASS_FILEREF, CLASS_WINDOW, Item, Signature};
use crate::glulx::glk::objects::{Event, MemArray};
use crate::glulx::memory::Memory;
use crate::glulx::operand::{self, StoreTarget};
use crate::glulx::stack::Stack;

/// A reference argument of -1 means "read from or write to the
/// stack" -- a feature of the Glk invocation mechanism alone, not
/// of Glulx addressing (Glulx: Miscellaneous).
pub const STACK_REF: u32 = 0xFFFF_FFFF;

const WORD: u32 = 4;

// The type bytes of the string objects a Glk call may name: the
// unencoded forms, and only those (Glulx: Miscellaneous).
const UNENCODED: u8 = 0xE0;
const UNENCODED_UNICODE: u8 = 0xE2;

const MAX_UNICODE: u32 = 0x10FFFF;

fn glk_error(message: String) -> VoxamError {
    VoxamError::GlulxGlk(message)
}

/// Map a library outcome that cannot legitimately suspend or end.
pub(crate) fn plain<T>(outcome: Outcome<T>) -> Result<T, VoxamError> {
    match outcome {
        Ok(value) => Ok(value),
        Err(Stop::Fault(error)) => Err(error),
        Err(Stop::End) => Err(glk_error("the session ended inside a plain call".into())),
    }
}

/// Two-way mapping between Glk arena keys and the ids Glulx sees.
///
/// The reference glkop.c keeps a hash table per class and seeds
/// each with a randomized offset, so that games cannot come to
/// depend on particular id values. Voxam assigns ids sequentially
/// instead: reproducible ids make transcript-diffing against a
/// reference interpreter possible, which is the best correctness
/// test available, and nothing in the spec requires randomness.
///
/// Ids are unique across classes and never reused, but lookups are
/// still class-checked, so passing a stream id where a window is
/// expected reads as the null object rather than as the wrong
/// object. Minting stays lazy -- an object earns its id when it
/// first crosses to the VM -- exactly as in the reference, so id
/// sequences in transcripts diff identically.
#[derive(Debug, Default)]
pub struct Registry {
    by_id: [HashMap<u32, u32>; 4],
    by_key: HashMap<(u32, u32), u32>,
    next: u32,
}

impl Registry {
    /// Open empty, with the id counter at one.
    pub fn new() -> Self {
        Self {
            by_id: Default::default(),
            by_key: HashMap::new(),
            next: 1,
        }
    }

    /// The object's id, minted if it is new; the null object is 0.
    pub fn register(&mut self, glk_class: u32, key: Option<u32>) -> u32 {
        let Some(key) = key else {
            return 0;
        };

        if let Some(existing) = self.by_key.get(&(glk_class, key)) {
            return *existing;
        }

        let ident = self.next;

        self.next += 1;
        self.by_key.insert((glk_class, key), ident);
        self.by_id[glk_class as usize].insert(ident, key);

        ident
    }

    /// The arena key an id names within a class, or None.
    pub fn lookup(&self, glk_class: u32, ident: u32) -> Option<u32> {
        if ident == 0 {
            return None;
        }

        self.by_id[glk_class as usize].get(&ident).copied()
    }

    /// Drop a destroyed object, so its id stops resolving.
    pub fn forget(&mut self, glk_class: u32, key: u32) {
        if let Some(ident) = self.by_key.remove(&(glk_class, key)) {
            self.by_id[glk_class as usize].remove(&ident);
        }
    }
}

/// One marshalled argument, in the shapes the call match
/// destructures.
#[derive(Debug)]
pub(crate) enum GlkArg {
    Value(u32),
    Str(String),
    Obj(Option<u32>),
    ObjList(Vec<Option<u32>>),
    Array(Option<MemArray>),
    Ref(Option<RefSlot>),
    Struct(Option<StructSlot>),
}

/// Where a deferred write lands.
#[derive(Debug, Clone, Copy)]
enum Sink {
    Memory(u32),
    Stack,
}

/// One deferred write: the argument holding the answer, the sink,
/// and the item that knows its encoding.
#[derive(Debug)]
struct Out {
    index: usize,
    sink: Sink,
    item: Item,
}

/// The deferred tail of a suspended call.
#[derive(Debug)]
enum Parked {
    /// A suspended select: the event seat and the deferred writes,
    /// held whole until the host delivers the event.
    Select { args: Vec<GlkArg>, outs: Vec<Out> },
    /// A suspended file prompt: the opcode's store, parked by the
    /// machine once the call unwinds.
    Prompt { store: Option<StoreTarget> },
}

/// What one glk opcode call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Performed {
    /// The call completed; the value the opcode stores.
    Value(u32),
    /// A select recorded its wait: the value still stores, and
    /// then the machine stands down until the host delivers the
    /// event.
    Suspended(u32),
    /// A file prompt stands mid-flight: nothing stores until the
    /// name arrives through deliver_file.
    Prompted,
    /// glk_exit, or input no display can ever answer: the session
    /// ends the way quit ends it.
    Ended,
}

/// Dispatches glk opcode calls into the Glk library.
pub struct Bridge {
    /// The Glk library the calls land on.
    pub library: Glk,
    /// The id mapping for the opaque classes.
    pub registry: Registry,
    parked: Option<Parked>,
}

impl Bridge {
    /// Join a library to the machine whose calls it will take.
    pub fn new(library: Glk) -> Self {
        Self {
            library,
            registry: Registry::new(),
            parked: None,
        }
    }

    /// Run one Glk call; what the opcode should do comes back.
    ///
    /// Stack output references push here, after the call but
    /// before the opcode's own store -- the order the spec fixes
    /// (Glulx: Miscellaneous).
    pub fn perform(
        &mut self,
        memory: &mut Memory,
        stack: &mut Stack,
        selector: u32,
        raw: &[u32],
    ) -> Result<Performed, VoxamError> {
        let Some(signature) = dispatch::lookup(selector) else {
            // A game asking for a selector this Glk lacks expects
            // a library from the future; that should be loud.
            return Err(glk_error(format!(
                "the glk opcode asked for unknown function {selector:#06x}"
            )));
        };

        // A suspended machine executes nothing until its event is
        // delivered; a call arriving anyway means a host ran on
        // past the suspension. Refused before any argument
        // marshalling, so the stack stays whole.
        if self.library.waiting.is_some() {
            return Err(glk_error(format!(
                "{} called while the machine stands suspended",
                signature.glk_name()
            )));
        }

        if raw.len() as u32 != signature.word_count() {
            return Err(glk_error(format!(
                "{} takes {} argument words, but {} arrived",
                signature.glk_name(),
                signature.word_count(),
                raw.len()
            )));
        }

        let (mut args, outs) = self.unmarshal(memory, stack, signature, raw)?;

        let ret = match call(&mut self.library, memory, selector, &mut args) {
            Ok(ret) => ret,
            Err(Stop::End) => {
                self.drain_disposals();

                return Ok(Performed::Ended);
            }
            Err(Stop::Fault(error)) => {
                self.drain_disposals();

                return Err(error);
            }
        };

        self.drain_disposals();

        match self.library.waiting {
            Some(Waiting::Select) => {
                // The call suspended: whatever must travel back
                // into memory waits with it, so the game's own
                // sentinel survives until the event arrives.
                let value = self.encode_ret(signature, ret);

                self.parked = Some(Parked::Select { args, outs });

                Ok(Performed::Suspended(value))
            }
            Some(Waiting::Prompt { .. }) => {
                // The call itself stands mid-flight: the store is
                // owed to the player's answer, and the machine
                // parks it here once the opcode unwinds.
                self.parked = Some(Parked::Prompt { store: None });

                Ok(Performed::Prompted)
            }
            None => {
                self.run_writebacks(memory, stack, &args, &outs)?;

                Ok(Performed::Value(self.encode_ret(signature, ret)))
            }
        }
    }

    /// Park the opcode's store for a suspended file prompt.
    pub fn park_prompt_store(&mut self, target: StoreTarget) {
        if let Some(Parked::Prompt { store }) = &mut self.parked {
            *store = Some(target);
        }
    }

    /// Complete a suspended select: the event fills the parked
    /// seat and the deferred writes run, so the answer lands in VM
    /// memory exactly where the game will look when it steps on.
    pub fn deliver_event(
        &mut self,
        memory: &mut Memory,
        stack: &mut Stack,
        event: Event,
    ) -> Result<(), VoxamError> {
        let event = plain(self.library.deliver_event(event))?;

        let Some(Parked::Select { mut args, outs }) = self.parked.take() else {
            // The library stood suspended but nothing was parked:
            // the select never came through a glk opcode.
            return Err(glk_error(
                "the select stands outside any glk call, with nothing deferred".into(),
            ));
        };

        for arg in &mut args {
            if let GlkArg::Struct(Some(slot)) = arg {
                slot.0 = vec![
                    Held::Word(event.kind),
                    Held::Obj(CLASS_WINDOW, event.window),
                    Held::Word(event.val1),
                    Held::Word(event.val2),
                ];
            }
        }

        self.run_writebacks(memory, stack, &args, &outs)?;
        self.drain_disposals();

        Ok(())
    }

    /// Complete a suspended file prompt: the name mints the
    /// reference, and the parked store speaks its id.
    pub fn deliver_file(
        &mut self,
        memory: &mut Memory,
        stack: &mut Stack,
        name: Option<&str>,
    ) -> Result<(), VoxamError> {
        if !matches!(self.library.waiting, Some(Waiting::Prompt { .. })) {
            return Err(glk_error(
                "a file name arrived with no prompt suspended to receive it".into(),
            ));
        }

        let Some(Parked::Prompt {
            store: Some(target),
        }) = self.parked
        else {
            return Err(glk_error(
                "the file prompt stands outside any glk call, with no store owed".into(),
            ));
        };

        let fileref = plain(self.library.deliver_file(name))?;
        let ident = self.registry.register(CLASS_FILEREF, fileref);

        self.parked = None;

        operand::store(memory, stack, target, ident, WORD)?;
        self.drain_disposals();

        Ok(())
    }

    /// Whether a select or a file prompt stands suspended.
    pub fn suspended(&self) -> bool {
        self.library.waiting.is_some()
    }

    fn drain_disposals(&mut self) {
        for (glk_class, key) in self.library.take_disposals() {
            self.registry.forget(glk_class, key);
        }
    }

    /// Turn raw words into call arguments, left to right, noting
    /// the deferred writes.
    fn unmarshal(
        &mut self,
        memory: &Memory,
        stack: &mut Stack,
        signature: &Signature,
        raw: &[u32],
    ) -> Result<(Vec<GlkArg>, Vec<Out>), VoxamError> {
        let mut args = Vec::with_capacity(signature.args.len());
        let mut outs = Vec::new();
        let mut position = 0;

        for item in signature.args {
            let index = args.len();

            if item.array {
                let (address, count) = (raw[position], raw[position + 1]);

                position += 2;

                if address == 0 {
                    require_nullable(item)?;

                    args.push(if item.is_opaque() {
                        GlkArg::ObjList(Vec::new())
                    } else {
                        GlkArg::Array(None)
                    });

                    continue;
                }

                if let Some(glk_class) = item.opaque_class() {
                    // An array of object ids -- only ever passed
                    // in, so a snapshot is equivalent to a live
                    // view.
                    let mut found = Vec::with_capacity(count as usize);

                    for at in 0..count {
                        let ident = memory.read_word(address.wrapping_add(WORD * at))?;

                        found.push(self.registry.lookup(glk_class, ident));
                    }

                    args.push(GlkArg::ObjList(found));
                } else {
                    // Writes through the coordinates land straight
                    // in memory, so even an out-array needs no
                    // write-back step.
                    args.push(GlkArg::Array(Some(MemArray {
                        address,
                        count,
                        width: item.element_size(),
                    })));
                }

                continue;
            }

            if item.is_reference() {
                let address = raw[position];

                position += 1;

                if address == 0 {
                    require_nullable(item)?;

                    args.push(if item.is_struct() {
                        GlkArg::Struct(None)
                    } else {
                        GlkArg::Ref(None)
                    });

                    continue;
                }

                let sink = if address == STACK_REF {
                    Sink::Stack
                } else {
                    Sink::Memory(address)
                };

                if item.is_struct() {
                    let mut slot = StructSlot::new(item.fields.len());

                    if item.passes_in() {
                        for (at, field) in item.fields.iter().enumerate() {
                            let value = match sink {
                                // The value need not be aligned,
                                // but is big-endian (Glulx:
                                // Miscellaneous).
                                Sink::Memory(address) => {
                                    memory.read_word(address.wrapping_add(WORD * at as u32))?
                                }
                                // An input reference pops
                                // first-topmost, so a struct's
                                // first field is the topmost value
                                // (Glulx: Miscellaneous).
                                Sink::Stack => stack.pop()?,
                            };

                            slot.0[at] = self.decode_value(field, value);
                        }
                    }

                    args.push(GlkArg::Struct(Some(slot)));
                } else {
                    let mut slot = RefSlot::default();

                    if item.passes_in() {
                        let value = match sink {
                            Sink::Memory(address) => memory.read_word(address)?,
                            Sink::Stack => stack.pop()?,
                        };

                        slot.0 = self.decode_value(item, value);
                    }

                    args.push(GlkArg::Ref(Some(slot)));
                }

                if item.passes_out() {
                    outs.push(Out {
                        index,
                        sink,
                        item: *item,
                    });
                }

                continue;
            }

            let value = raw[position];

            position += 1;

            if item.is_string() {
                args.push(GlkArg::Str(
                    if item.code == Some(crate::glulx::glk::dispatch::Code::UString) {
                        read_unicode_string(memory, value)?
                    } else {
                        read_string(memory, value)?
                    },
                ));
            } else if let Some(glk_class) = item.opaque_class() {
                args.push(GlkArg::Obj(self.registry.lookup(glk_class, value)));
            } else {
                args.push(GlkArg::Value(value));
            }
        }

        Ok((args, outs))
    }

    /// A word into a holder value: an object, or itself.
    fn decode_value(&self, item: &Item, raw: u32) -> Held {
        match item.opaque_class() {
            Some(glk_class) => Held::Obj(glk_class, self.registry.lookup(glk_class, raw)),
            None => Held::Word(raw),
        }
    }

    /// A holder value back into a 32-bit word, minting ids.
    fn encode(&mut self, item: &Item, held: Held) -> u32 {
        match (item.opaque_class(), held) {
            (Some(glk_class), Held::Obj(_, key)) => self.registry.register(glk_class, key),
            (Some(_), Held::Word(_)) => 0,
            (None, held) => held.word(),
        }
    }

    /// The result as a word; a void call stores zero (Glulx:
    /// Miscellaneous).
    fn encode_ret(&mut self, signature: &Signature, ret: Ret) -> u32 {
        match (signature.result.as_ref(), ret) {
            (None, _) => 0,
            (Some(item), Ret::Obj(glk_class, key)) => {
                debug_assert_eq!(item.opaque_class(), Some(glk_class));

                self.registry.register(glk_class, key)
            }
            (Some(_), Ret::Word(value)) => value,
            (Some(_), Ret::Signed(value)) => value as u32,
            (Some(_), Ret::None) => 0,
        }
    }

    /// Run the deferred writes: holders out to their sinks, in
    /// argument order.
    fn run_writebacks(
        &mut self,
        memory: &mut Memory,
        stack: &mut Stack,
        args: &[GlkArg],
        outs: &[Out],
    ) -> Result<(), VoxamError> {
        for out in outs {
            match &args[out.index] {
                GlkArg::Struct(Some(slot)) => {
                    for (at, field) in out.item.fields.iter().enumerate() {
                        let value = self.encode(field, slot.0[at]);

                        match out.sink {
                            Sink::Memory(address) => {
                                memory.write_word(address.wrapping_add(WORD * at as u32), value)?;
                            }
                            // An output reference pushes
                            // last-topmost: fields in order leave
                            // the last one on top (Glulx:
                            // Miscellaneous).
                            Sink::Stack => stack.push(value)?,
                        }
                    }
                }
                GlkArg::Ref(Some(slot)) => {
                    let value = self.encode(&out.item, slot.0);

                    match out.sink {
                        Sink::Memory(address) => memory.write_word(address, value)?,
                        Sink::Stack => stack.push(value)?,
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }
}

/// What a call answered, before encoding.
#[derive(Debug, Clone, Copy)]
enum Ret {
    None,
    Word(u32),
    Signed(i64),
    Obj(u32, Option<u32>),
}

fn require_nullable(item: &Item) -> Result<(), VoxamError> {
    if item.nonnull {
        return Err(glk_error(
            "a null address arrived where the Glk call requires one".into(),
        ));
    }

    Ok(())
}

/// Read an unencoded (E0) string object.
///
/// A string argument is the address of a *string object*, not of a
/// bare byte array -- the type byte comes first and the text ends
/// at a zero byte (Glulx: Miscellaneous).
fn read_string(memory: &Memory, address: u32) -> Result<String, VoxamError> {
    let kind = memory.read_byte(address)?;

    if kind != UNENCODED {
        return Err(glk_error(format!(
            "the Glk string argument at {address:#x} is not an E0 string object \
             (found {kind:#04x})"
        )));
    }

    let mut at = address.wrapping_add(1);
    let mut out = String::new();

    loop {
        let byte = memory.read_byte(at)?;

        if byte == 0 {
            return Ok(out);
        }

        out.push(char::from_u32(u32::from(byte)).unwrap_or('?'));
        at = at.wrapping_add(1);
    }
}

/// Read an unencoded Unicode (E2) string object.
///
/// An E2 object is a type byte and three padding bytes, so the
/// characters start four bytes in (Glulx: String Encoding).
fn read_unicode_string(memory: &Memory, address: u32) -> Result<String, VoxamError> {
    let kind = memory.read_byte(address)?;

    if kind != UNENCODED_UNICODE {
        return Err(glk_error(format!(
            "the Glk Unicode string argument at {address:#x} is not an E2 string \
             object (found {kind:#04x})"
        )));
    }

    let mut at = address.wrapping_add(WORD);
    let mut out = String::new();

    loop {
        let value = memory.read_word(at)?;

        if value == 0 {
            return Ok(out);
        }

        // A Glulx string may hold values that are not code points
        // at all; they render as the placeholder.
        out.push(if value <= MAX_UNICODE {
            char::from_u32(value).unwrap_or('?')
        } else {
            '?'
        });
        at = at.wrapping_add(WORD);
    }
}

// -- the call table ---------------------------------------------------------

fn signed_of(value: u32) -> i64 {
    i64::from(value as i32)
}

/// Route one selector to its library function, destructuring the
/// marshalled arguments the signature promised. The shapes cannot
/// mismatch -- the dispatch table built the arguments -- so an
/// unmatched arm is an internal fault, not a game's.
#[allow(clippy::too_many_lines)] // one arm per Glk function
fn call(glk: &mut Glk, memory: &mut Memory, selector: u32, args: &mut [GlkArg]) -> Outcome<Ret> {
    use GlkArg as A;

    let ret = match (selector, args) {
        (0x0001, []) => {
            glk.glk_exit()?;

            Ret::None
        }
        (0x0003, []) => {
            glk.glk_tick();

            Ret::None
        }
        (0x0004, [A::Value(sel), A::Value(val)]) => Ret::Word(glk.glk_gestalt(*sel, *val)),
        (0x0005, [A::Value(sel), A::Value(val), A::Array(arr)]) => {
            Ret::Word(glk.glk_gestalt_ext(memory, *sel, *val, *arr)?)
        }

        (0x0020, [A::Obj(win), A::Ref(rock)]) => {
            Ret::Obj(CLASS_WINDOW, glk.glk_window_iterate(*win, rock.as_mut()))
        }
        (0x0021, [A::Obj(win)]) => Ret::Word(glk.glk_window_get_rock(*win)),
        (0x0022, []) => Ret::Obj(CLASS_WINDOW, glk.glk_window_get_root()),
        (
            0x0023,
            [
                A::Obj(split),
                A::Value(method),
                A::Value(size),
                A::Value(wtype),
                A::Value(rock),
            ],
        ) => Ret::Obj(
            CLASS_WINDOW,
            glk.glk_window_open(*split, *method, *size, *wtype, *rock)?,
        ),
        (0x0024, [A::Obj(win), A::Struct(result)]) => {
            glk.glk_window_close(*win, result.as_mut())?;

            Ret::None
        }
        (0x0025, [A::Obj(win), A::Ref(width), A::Ref(height)]) => {
            glk.glk_window_get_size(*win, width.as_mut(), height.as_mut());

            Ret::None
        }
        (0x0026, [A::Obj(win), A::Value(method), A::Value(size), A::Obj(key)]) => {
            glk.glk_window_set_arrangement(*win, *method, *size, *key)?;

            Ret::None
        }
        (0x0027, [A::Obj(win), A::Ref(method), A::Ref(size), A::Ref(key)]) => {
            glk.glk_window_get_arrangement(*win, method.as_mut(), size.as_mut(), key.as_mut())?;

            Ret::None
        }
        (0x0028, [A::Obj(win)]) => Ret::Word(glk.glk_window_get_type(*win)),
        (0x0029, [A::Obj(win)]) => Ret::Obj(CLASS_WINDOW, glk.glk_window_get_parent(*win)),
        (0x002A, [A::Obj(win)]) => {
            glk.glk_window_clear(*win);

            Ret::None
        }
        (0x002B, [A::Obj(win), A::Value(x), A::Value(y)]) => {
            glk.glk_window_move_cursor(*win, i64::from(*x), i64::from(*y))?;

            Ret::None
        }
        (0x002C, [A::Obj(win)]) => {
            Ret::Obj(dispatch::CLASS_STREAM, glk.glk_window_get_stream(*win))
        }
        (0x002D, [A::Obj(win), A::Obj(stream)]) => {
            glk.glk_window_set_echo_stream(*win, *stream);

            Ret::None
        }
        (0x002E, [A::Obj(win)]) => {
            Ret::Obj(dispatch::CLASS_STREAM, glk.glk_window_get_echo_stream(*win))
        }
        (0x002F, [A::Obj(win)]) => {
            glk.glk_set_window(*win);

            Ret::None
        }
        (0x0030, [A::Obj(win)]) => Ret::Obj(CLASS_WINDOW, glk.glk_window_get_sibling(*win)),

        (0x0040, [A::Obj(stream), A::Ref(rock)]) => Ret::Obj(
            dispatch::CLASS_STREAM,
            glk.glk_stream_iterate(*stream, rock.as_mut()),
        ),
        (0x0041, [A::Obj(stream)]) => Ret::Word(glk.glk_stream_get_rock(*stream)),
        (0x0042, [A::Obj(fileref), A::Value(fmode), A::Value(rock)]) => Ret::Obj(
            dispatch::CLASS_STREAM,
            glk.glk_stream_open_file(*fileref, *fmode, *rock)?,
        ),
        (0x0043, [A::Array(buf), A::Value(fmode), A::Value(rock)]) => Ret::Obj(
            dispatch::CLASS_STREAM,
            Some(glk.glk_stream_open_memory(*buf, *fmode, *rock)?),
        ),
        (0x0044, [A::Obj(stream), A::Struct(result)]) => {
            glk.glk_stream_close(*stream, result.as_mut())?;

            Ret::None
        }
        (0x0045, [A::Obj(stream), A::Value(position), A::Value(mode)]) => {
            glk.glk_stream_set_position(*stream, signed_of(*position), *mode)?;

            Ret::None
        }
        (0x0046, [A::Obj(stream)]) => Ret::Word(glk.glk_stream_get_position(*stream)?),
        (0x0047, [A::Obj(stream)]) => {
            glk.glk_stream_set_current(*stream);

            Ret::None
        }
        (0x0048, []) => Ret::Obj(dispatch::CLASS_STREAM, glk.glk_stream_get_current()),
        (0x0049, [A::Value(filenum), A::Value(rock)]) => Ret::Obj(
            dispatch::CLASS_STREAM,
            glk.glk_stream_open_resource(*filenum, *rock),
        ),

        (0x0060, [A::Value(usage), A::Value(rock)]) => Ret::Obj(
            CLASS_FILEREF,
            Some(glk.glk_fileref_create_temp(*usage, *rock)?),
        ),
        (0x0061, [A::Value(usage), A::Str(name), A::Value(rock)]) => Ret::Obj(
            CLASS_FILEREF,
            Some(glk.glk_fileref_create_by_name(*usage, name, *rock)),
        ),
        (0x0062, [A::Value(usage), A::Value(fmode), A::Value(rock)]) => Ret::Obj(
            CLASS_FILEREF,
            glk.glk_fileref_create_by_prompt(*usage, *fmode, *rock),
        ),
        (0x0063, [A::Obj(fileref)]) => {
            glk.glk_fileref_destroy(*fileref);

            Ret::None
        }
        (0x0064, [A::Obj(fileref), A::Ref(rock)]) => Ret::Obj(
            CLASS_FILEREF,
            glk.glk_fileref_iterate(*fileref, rock.as_mut()),
        ),
        (0x0065, [A::Obj(fileref)]) => Ret::Word(glk.glk_fileref_get_rock(*fileref)),
        (0x0066, [A::Obj(fileref)]) => {
            glk.glk_fileref_delete_file(*fileref);

            Ret::None
        }
        (0x0067, [A::Obj(fileref)]) => Ret::Word(glk.glk_fileref_does_file_exist(*fileref)),
        (0x0068, [A::Value(usage), A::Obj(fileref), A::Value(rock)]) => Ret::Obj(
            CLASS_FILEREF,
            Some(glk.glk_fileref_create_from_fileref(*usage, *fileref, *rock)?),
        ),

        (0x0080, [A::Value(ch)]) => {
            glk.glk_put_char(memory, *ch)?;

            Ret::None
        }
        (0x0081, [A::Obj(stream), A::Value(ch)]) => {
            glk.glk_put_char_stream(memory, *stream, *ch)?;

            Ret::None
        }
        (0x0082, [A::Str(text)]) => {
            let text = std::mem::take(text);

            glk.glk_put_string(memory, &text)?;

            Ret::None
        }
        (0x0083, [A::Obj(stream), A::Str(text)]) => {
            let text = std::mem::take(text);

            glk.glk_put_string_stream(memory, *stream, &text)?;

            Ret::None
        }
        (0x0084, [A::Array(buf)]) => {
            glk.glk_put_buffer(memory, *buf)?;

            Ret::None
        }
        (0x0085, [A::Obj(stream), A::Array(buf)]) => {
            glk.glk_put_buffer_stream(memory, *stream, *buf)?;

            Ret::None
        }
        (0x0086, [A::Value(style)]) => {
            glk.glk_set_style(*style);

            Ret::None
        }
        (0x0087, [A::Obj(stream), A::Value(style)]) => {
            glk.glk_set_style_stream(*stream, *style);

            Ret::None
        }

        (0x0090, [A::Obj(stream)]) => Ret::Signed(glk.glk_get_char_stream(memory, *stream)?),
        (0x0091, [A::Obj(stream), A::Array(buf)]) => {
            Ret::Word(glk.glk_get_line_stream(memory, *stream, *buf)?)
        }
        (0x0092, [A::Obj(stream), A::Array(buf)]) => {
            Ret::Word(glk.glk_get_buffer_stream(memory, *stream, *buf)?)
        }

        (0x00A0, [A::Value(ch)]) => Ret::Word(glk.glk_char_to_lower(*ch)),
        (0x00A1, [A::Value(ch)]) => Ret::Word(glk.glk_char_to_upper(*ch)),

        (
            0x00B0,
            [
                A::Value(wtype),
                A::Value(styl),
                A::Value(hint),
                A::Value(value),
            ],
        ) => {
            glk.glk_stylehint_set(*wtype, *styl, *hint, *value);

            Ret::None
        }
        (0x00B1, [A::Value(wtype), A::Value(styl), A::Value(hint)]) => {
            glk.glk_stylehint_clear(*wtype, *styl, *hint);

            Ret::None
        }
        (0x00B2, [A::Obj(win), A::Value(one), A::Value(two)]) => {
            Ret::Word(glk.glk_style_distinguish(*win, *one, *two))
        }
        (0x00B3, [A::Obj(win), A::Value(styl), A::Value(hint), A::Ref(result)]) => {
            Ret::Word(glk.glk_style_measure(*win, *styl, *hint, result.as_mut()))
        }

        (0x00C0, [A::Struct(event)]) => {
            glk.glk_select(memory, event.as_mut().expect("select's struct is nonnull"))?;

            Ret::None
        }
        (0x00C1, [A::Struct(event)]) => {
            glk.glk_select_poll(event.as_mut().expect("select_poll's struct is nonnull"));

            Ret::None
        }
        (0x00D0, [A::Obj(win), A::Array(buf), A::Value(initlen)]) => {
            glk.glk_request_line_event(*win, *buf, *initlen)?;

            Ret::None
        }
        (0x00D1, [A::Obj(win), A::Struct(event)]) => {
            glk.glk_cancel_line_event(*win, event.as_mut());

            Ret::None
        }
        (0x00D2, [A::Obj(win)]) => {
            glk.glk_request_char_event(*win)?;

            Ret::None
        }
        (0x00D3, [A::Obj(win)]) => {
            glk.glk_cancel_char_event(*win);

            Ret::None
        }
        (0x00D4, [A::Obj(win)]) => {
            glk.glk_request_mouse_event(*win);

            Ret::None
        }
        (0x00D5, [A::Obj(win)]) => {
            glk.glk_cancel_mouse_event(*win);

            Ret::None
        }
        (0x00D6, [A::Value(millisecs)]) => {
            glk.glk_request_timer_events(*millisecs);

            Ret::None
        }

        (0x00E0, [A::Value(image), A::Ref(width), A::Ref(height)]) => {
            Ret::Word(glk.glk_image_get_info(*image, width.as_mut(), height.as_mut()))
        }
        (0x00E1, [A::Obj(win), A::Value(image), A::Value(val1), A::Value(val2)]) => {
            Ret::Word(glk.glk_image_draw(*win, *image, signed_of(*val1), signed_of(*val2)))
        }
        (
            0x00E2,
            [
                A::Obj(win),
                A::Value(image),
                A::Value(val1),
                A::Value(val2),
                A::Value(width),
                A::Value(height),
            ],
        ) => Ret::Word(glk.glk_image_draw_scaled(
            *win,
            *image,
            signed_of(*val1),
            signed_of(*val2),
            *width,
            *height,
        )),
        (0x00E8, [A::Obj(win)]) => {
            glk.glk_window_flow_break(*win);

            Ret::None
        }
        (
            0x00E9,
            [
                A::Obj(win),
                A::Value(left),
                A::Value(top),
                A::Value(width),
                A::Value(height),
            ],
        ) => {
            glk.glk_window_erase_rect(*win, signed_of(*left), signed_of(*top), *width, *height);

            Ret::None
        }
        (
            0x00EA,
            [
                A::Obj(win),
                A::Value(color),
                A::Value(left),
                A::Value(top),
                A::Value(width),
                A::Value(height),
            ],
        ) => {
            glk.glk_window_fill_rect(
                *win,
                *color,
                signed_of(*left),
                signed_of(*top),
                *width,
                *height,
            );

            Ret::None
        }
        (0x00EB, [A::Obj(win), A::Value(color)]) => {
            glk.glk_window_set_background_color(*win, *color);

            Ret::None
        }
        (
            0x00EC,
            [
                A::Obj(win),
                A::Value(image),
                A::Value(val1),
                A::Value(val2),
                A::Value(width),
                A::Value(height),
                A::Value(rule),
                A::Value(maxwidth),
            ],
        ) => Ret::Word(glk.glk_image_draw_scaled_ext(
            *win,
            *image,
            signed_of(*val1),
            signed_of(*val2),
            *width,
            *height,
            *rule,
            *maxwidth,
        )),

        (0x00F0, [A::Obj(channel), A::Ref(rock)]) => Ret::Obj(
            dispatch::CLASS_SCHANNEL,
            glk.glk_schannel_iterate(*channel, rock.as_mut()),
        ),
        (0x00F1, [A::Obj(channel)]) => Ret::Word(glk.glk_schannel_get_rock(*channel)),
        (0x00F2, [A::Value(rock)]) => {
            Ret::Obj(dispatch::CLASS_SCHANNEL, glk.glk_schannel_create(*rock))
        }
        (0x00F3, [A::Obj(channel)]) => {
            glk.glk_schannel_destroy(*channel);

            Ret::None
        }
        (0x00F4, [A::Value(rock), A::Value(volume)]) => Ret::Obj(
            dispatch::CLASS_SCHANNEL,
            glk.glk_schannel_create_ext(*rock, *volume),
        ),
        (0x00F7, [A::ObjList(channels), A::Array(sounds), A::Value(notify)]) => {
            let channels = std::mem::take(channels);

            Ret::Word(glk.glk_schannel_play_multi(memory, &channels, *sounds, *notify)?)
        }
        (0x00F8, [A::Obj(channel), A::Value(sound)]) => {
            Ret::Word(glk.glk_schannel_play(*channel, *sound))
        }
        (
            0x00F9,
            [
                A::Obj(channel),
                A::Value(sound),
                A::Value(repeats),
                A::Value(notify),
            ],
        ) => Ret::Word(glk.glk_schannel_play_ext(*channel, *sound, *repeats, *notify)),
        (0x00FA, [A::Obj(channel)]) => {
            glk.glk_schannel_stop(*channel);

            Ret::None
        }
        (0x00FB, [A::Obj(channel), A::Value(volume)]) => {
            glk.glk_schannel_set_volume(*channel, *volume);

            Ret::None
        }
        (0x00FC, [A::Value(sound), A::Value(flag)]) => {
            glk.glk_sound_load_hint(*sound, *flag);

            Ret::None
        }
        (
            0x00FD,
            [
                A::Obj(channel),
                A::Value(volume),
                A::Value(duration),
                A::Value(notify),
            ],
        ) => {
            glk.glk_schannel_set_volume_ext(*channel, *volume, *duration, *notify);

            Ret::None
        }
        (0x00FE, [A::Obj(channel)]) => {
            glk.glk_schannel_pause(*channel);

            Ret::None
        }
        (0x00FF, [A::Obj(channel)]) => {
            glk.glk_schannel_unpause(*channel);

            Ret::None
        }

        (0x0100, [A::Value(linkval)]) => {
            glk.glk_set_hyperlink(*linkval);

            Ret::None
        }
        (0x0101, [A::Obj(stream), A::Value(linkval)]) => {
            glk.glk_set_hyperlink_stream(*stream, *linkval);

            Ret::None
        }
        (0x0102, [A::Obj(win)]) => {
            glk.glk_request_hyperlink_event(*win);

            Ret::None
        }
        (0x0103, [A::Obj(win)]) => {
            glk.glk_cancel_hyperlink_event(*win);

            Ret::None
        }

        (0x0120, [A::Array(buf), A::Value(numchars)]) => {
            Ret::Word(glk.glk_buffer_to_lower_case_uni(memory, *buf, *numchars)?)
        }
        (0x0121, [A::Array(buf), A::Value(numchars)]) => {
            Ret::Word(glk.glk_buffer_to_upper_case_uni(memory, *buf, *numchars)?)
        }
        (0x0122, [A::Array(buf), A::Value(numchars), A::Value(lowerrest)]) => {
            Ret::Word(glk.glk_buffer_to_title_case_uni(memory, *buf, *numchars, *lowerrest)?)
        }
        (0x0123, [A::Array(buf), A::Value(numchars)]) => {
            Ret::Word(glk.glk_buffer_canon_decompose_uni(memory, *buf, *numchars)?)
        }
        (0x0124, [A::Array(buf), A::Value(numchars)]) => {
            Ret::Word(glk.glk_buffer_canon_normalize_uni(memory, *buf, *numchars)?)
        }

        (0x0128, [A::Value(ch)]) => {
            glk.glk_put_char_uni(memory, *ch)?;

            Ret::None
        }
        (0x0129, [A::Str(text)]) => {
            let text = std::mem::take(text);

            glk.glk_put_string_uni(memory, &text)?;

            Ret::None
        }
        (0x012A, [A::Array(buf)]) => {
            glk.glk_put_buffer_uni(memory, *buf)?;

            Ret::None
        }
        (0x012B, [A::Obj(stream), A::Value(ch)]) => {
            glk.glk_put_char_stream_uni(memory, *stream, *ch)?;

            Ret::None
        }
        (0x012C, [A::Obj(stream), A::Str(text)]) => {
            let text = std::mem::take(text);

            glk.glk_put_string_stream_uni(memory, *stream, &text)?;

            Ret::None
        }
        (0x012D, [A::Obj(stream), A::Array(buf)]) => {
            glk.glk_put_buffer_stream_uni(memory, *stream, *buf)?;

            Ret::None
        }

        (0x0130, [A::Obj(stream)]) => Ret::Signed(glk.glk_get_char_stream_uni(memory, *stream)?),
        (0x0131, [A::Obj(stream), A::Array(buf)]) => {
            Ret::Word(glk.glk_get_buffer_stream_uni(memory, *stream, *buf)?)
        }
        (0x0132, [A::Obj(stream), A::Array(buf)]) => {
            Ret::Word(glk.glk_get_line_stream_uni(memory, *stream, *buf)?)
        }
        (0x0138, [A::Obj(fileref), A::Value(fmode), A::Value(rock)]) => Ret::Obj(
            dispatch::CLASS_STREAM,
            glk.glk_stream_open_file_uni(*fileref, *fmode, *rock)?,
        ),
        (0x0139, [A::Array(buf), A::Value(fmode), A::Value(rock)]) => Ret::Obj(
            dispatch::CLASS_STREAM,
            Some(glk.glk_stream_open_memory_uni(*buf, *fmode, *rock)?),
        ),
        (0x013A, [A::Value(filenum), A::Value(rock)]) => Ret::Obj(
            dispatch::CLASS_STREAM,
            glk.glk_stream_open_resource_uni(*filenum, *rock),
        ),
        (0x0140, [A::Obj(win)]) => {
            glk.glk_request_char_event_uni(*win)?;

            Ret::None
        }
        (0x0141, [A::Obj(win), A::Array(buf), A::Value(initlen)]) => {
            glk.glk_request_line_event_uni(*win, *buf, *initlen)?;

            Ret::None
        }

        (0x0150, [A::Obj(win), A::Value(value)]) => {
            glk.glk_set_echo_line_event(*win, *value);

            Ret::None
        }
        (0x0151, [A::Obj(win), A::Array(keycodes)]) => {
            glk.glk_set_terminators_line_event(memory, *win, *keycodes)?;

            Ret::None
        }

        (0x0160, [A::Struct(time)]) => {
            glk.glk_current_time(time.as_mut());

            Ret::None
        }
        (0x0161, [A::Value(factor)]) => Ret::Signed(glk.glk_current_simple_time(*factor)),
        (0x0168, [A::Struct(time), A::Struct(date)]) => {
            glk.glk_time_to_date_utc(time.as_ref(), date.as_mut());

            Ret::None
        }
        (0x0169, [A::Struct(time), A::Struct(date)]) => {
            glk.glk_time_to_date_local(time.as_ref(), date.as_mut());

            Ret::None
        }
        (0x016A, [A::Value(time), A::Value(factor), A::Struct(date)]) => {
            glk.glk_simple_time_to_date_utc(signed_of(*time), *factor, date.as_mut());

            Ret::None
        }
        (0x016B, [A::Value(time), A::Value(factor), A::Struct(date)]) => {
            glk.glk_simple_time_to_date_local(signed_of(*time), *factor, date.as_mut());

            Ret::None
        }
        (0x016C, [A::Struct(date), A::Struct(time)]) => {
            glk.glk_date_to_time_utc(date.as_ref(), time.as_mut());

            Ret::None
        }
        (0x016D, [A::Struct(date), A::Struct(time)]) => {
            glk.glk_date_to_time_local(date.as_ref(), time.as_mut());

            Ret::None
        }
        (0x016E, [A::Struct(date), A::Value(factor)]) => {
            Ret::Signed(glk.glk_date_to_simple_time_utc(date.as_ref(), *factor))
        }
        (0x016F, [A::Struct(date), A::Value(factor)]) => {
            Ret::Signed(glk.glk_date_to_simple_time_local(date.as_ref(), *factor))
        }

        _ => unreachable!("a dispatched selector's arguments match its signature"),
    };

    Ok(ret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glulx::glk::dispatch::{U32, into};
    use crate::glulx::glk::frontend::{Asked, Frontend};
    use crate::glulx::glk::objects::{
        WindowMap, event_type, file_mode, file_usage, seek_mode, window_type,
    };
    use crate::glulx::machine::{Machine, Step};
    use crate::glulx::story::Story;
    use crate::glulx::testing::image;

    const IDLE: &[u8] = &[0xC0, 0x00, 0x00, 0x81, 0x20];
    const PLANT: u32 = 0x180;
    const RESULT: u32 = 0x140;
    const SCRATCH: u32 = 0x2C0;
    const TEXT: u32 = 0x250;

    const GESTALT: u32 = 0x0004;
    const WINDOW_OPEN: u32 = 0x0023;
    const WINDOW_CLOSE: u32 = 0x0024;
    const WINDOW_GET_ROCK: u32 = 0x0021;
    const WINDOW_ITERATE: u32 = 0x0020;
    const STREAM_GET_ROCK: u32 = 0x0041;
    const STREAM_OPEN_MEMORY: u32 = 0x0043;
    const STREAM_CLOSE: u32 = 0x0044;
    const STREAM_SET_POSITION: u32 = 0x0045;
    const STREAM_GET_POSITION: u32 = 0x0046;
    const GET_BUFFER_STREAM: u32 = 0x0092;
    const PUT_STRING: u32 = 0x0082;
    const PUT_STRING_UNI: u32 = 0x0129;
    const PUT_BUFFER: u32 = 0x0084;
    const SELECT: u32 = 0x00C0;
    const PLAY_MULTI: u32 = 0x00F7;
    const FILEREF_CREATE_BY_NAME: u32 = 0x0061;
    const TIME_TO_DATE_UTC: u32 = 0x0168;
    const DATE_TO_TIME_UTC: u32 = 0x016C;

    /// A display that cannot block: never asked, only delivered
    /// to.
    struct Suspending;

    impl Frontend for Suspending {
        fn suspends(&self) -> bool {
            true
        }

        fn size(&self) -> (i64, i64) {
            (80, 24)
        }

        fn flush(&mut self, _windows: &mut WindowMap, _root: Option<u32>) {}

        fn read_line(
            &mut self,
            _windows: &mut WindowMap,
            _window: u32,
            _maxlen: u32,
        ) -> Asked<(String, u32)> {
            unreachable!("a suspending display is never asked for a line");
        }

        fn read_char(&mut self, _windows: &mut WindowMap, _window: u32) -> Asked<u32> {
            unreachable!("a suspending display is never asked for a key");
        }
    }

    /// A machine with a Glk library installed.
    fn bridged() -> Machine {
        let mut machine = Machine::new(Story::new(image(IDLE)).unwrap(), None).unwrap();

        machine.install_glk(Glk::over_nothing());

        machine
    }

    fn perform(machine: &mut Machine, selector: u32, raw: &[u32]) -> Result<Performed, VoxamError> {
        let bridge = machine.bridge.as_mut().unwrap();

        bridge.perform(&mut machine.memory, &mut machine.stack, selector, raw)
    }

    fn performed_value(machine: &mut Machine, selector: u32, raw: &[u32]) -> u32 {
        match perform(machine, selector, raw).unwrap() {
            Performed::Value(value) => value,
            other => panic!("expected a plain value, got {other:?}"),
        }
    }

    // The registry mints sequential ids -- reproducible sessions
    // beat glkop.c's randomized offsets -- and lookups are
    // class-checked, so a stream id in a window seat reads as the
    // null object.
    #[test]
    fn the_registry_minted_ids_are_class_checked() {
        let mut registry = Registry::new();

        let first = registry.register(CLASS_WINDOW, Some(41));

        assert_eq!(first, 1);
        assert_eq!(registry.register(CLASS_WINDOW, Some(41)), 1);
        assert_eq!(registry.register(CLASS_WINDOW, None), 0);
        assert_eq!(registry.lookup(CLASS_WINDOW, first), Some(41));
        assert_eq!(registry.lookup(dispatch::CLASS_STREAM, first), None);
        assert_eq!(registry.lookup(CLASS_WINDOW, 0), None);

        registry.forget(CLASS_WINDOW, 41);
        registry.forget(CLASS_WINDOW, 41);

        assert_eq!(registry.lookup(CLASS_WINDOW, first), None);
        assert_eq!(registry.register(CLASS_WINDOW, Some(42)), 2);
    }

    // A plain call passes words in and a word back; the unknown
    // and the malformed are refused by name.
    #[test]
    fn plain_calls_pass_words() {
        let mut machine = bridged();

        assert_eq!(
            performed_value(&mut machine, GESTALT, &[0, 0]),
            crate::glulx::glk::api::GLK_VERSION
        );

        let error = perform(&mut machine, 0x9999, &[]).unwrap_err();

        assert!(error.to_string().contains("unknown function 0x9999"));

        let error = perform(&mut machine, GESTALT, &[0]).unwrap_err();

        assert!(error.to_string().contains("takes 2 argument words, but 1"));
    }

    // Opaque objects cross as ids: a window opens and its id
    // answers for it, a wrong-class id reads as null, and a closed
    // window's id stops resolving because disposal reaches the
    // registry.
    #[test]
    fn opaque_ids_mint_and_expire() {
        let mut machine = bridged();

        let ident = performed_value(
            &mut machine,
            WINDOW_OPEN,
            &[0, 0, 0, window_type::TEXT_BUFFER, 7],
        );

        assert_eq!(ident, 1);
        assert_eq!(performed_value(&mut machine, WINDOW_GET_ROCK, &[ident]), 7);
        assert_eq!(performed_value(&mut machine, STREAM_GET_ROCK, &[ident]), 0);

        performed_value(&mut machine, WINDOW_CLOSE, &[ident, RESULT]);

        assert_eq!(performed_value(&mut machine, WINDOW_GET_ROCK, &[ident]), 0);
        assert_eq!(machine.memory.read_word(RESULT).unwrap(), 0);
        assert_eq!(machine.memory.read_word(RESULT + 4).unwrap(), 0);
    }

    // Scalar output references write to memory when given an
    // address, skip quietly when given null, and push when given
    // -1.
    #[test]
    fn scalar_references_answer_everywhere() {
        let mut machine = bridged();

        let first = performed_value(
            &mut machine,
            WINDOW_OPEN,
            &[0, 0, 0, window_type::TEXT_BUFFER, 9],
        );

        assert_eq!(
            performed_value(&mut machine, WINDOW_ITERATE, &[0, SCRATCH]),
            first
        );
        assert_eq!(machine.memory.read_word(SCRATCH).unwrap(), 9);
        assert_eq!(
            performed_value(&mut machine, WINDOW_ITERATE, &[first, 0]),
            0
        );

        let depth = machine.stack.count();

        performed_value(&mut machine, WINDOW_ITERATE, &[0, STACK_REF]);

        assert_eq!(machine.stack.count(), depth + 1);
        assert_eq!(machine.stack.pop().unwrap(), 9);
    }

    // A struct written to the stack pushes its fields in order,
    // last on top -- which is why a game closing a stream pops the
    // write count before the read count (Glulx: Miscellaneous).
    #[test]
    fn stack_structs_push_last_on_top() {
        let mut machine = bridged();

        let stream = performed_value(
            &mut machine,
            STREAM_OPEN_MEMORY,
            &[0, 0, file_mode::WRITE, 0],
        );

        {
            let bridge = machine.bridge.as_mut().unwrap();
            let key = bridge
                .registry
                .lookup(dispatch::CLASS_STREAM, stream)
                .unwrap();

            plain(
                bridge
                    .library
                    .glk_put_char_stream(&mut machine.memory, Some(key), 0x41),
            )
            .unwrap();
        }

        performed_value(&mut machine, STREAM_CLOSE, &[stream, STACK_REF]);

        assert_eq!(machine.stack.pop().unwrap(), 1);
        assert_eq!(machine.stack.pop().unwrap(), 0);
    }

    // A struct read from the stack pops its fields first-topmost,
    // and the date comes back through memory: 2023-11-14, a
    // Tuesday counted from Sunday.
    #[test]
    fn stack_structs_pop_first_topmost() {
        let mut machine = bridged();

        machine.stack.push(250).unwrap();
        machine.stack.push(1_700_000_000).unwrap();
        machine.stack.push(0).unwrap();

        performed_value(&mut machine, TIME_TO_DATE_UTC, &[STACK_REF, SCRATCH]);

        let held: Vec<u32> = (0..8)
            .map(|index| machine.memory.read_word(SCRATCH + 4 * index).unwrap())
            .collect();

        assert_eq!(held, [2023, 11, 14, 2, 22, 13, 20, 250]);
    }

    // A struct read from memory decodes its signed fields: an hour
    // of -3 is legal and normalizes away (Glk: Time and Date
    // Conversions).
    #[test]
    fn memory_structs_decode_signed_fields() {
        let mut machine = bridged();

        let fields: [i64; 8] = [2023, 11, 14, 0, -3, 0, 0, 0];

        for (index, value) in fields.iter().enumerate() {
            machine
                .memory
                .write_word(SCRATCH + 4 * index as u32, *value as u32)
                .unwrap();
        }

        performed_value(&mut machine, DATE_TO_TIME_UTC, &[SCRATCH, RESULT]);

        let low = machine.memory.read_word(RESULT + 4).unwrap();

        assert_eq!(
            u64::from(low),
            1_700_000_000 - 22 * 3600 - 13 * 60 - 20 - 3 * 3600
        );
    }

    // Null is refused where the signature forbids it: select's
    // event struct, and put_buffer's character array.
    #[test]
    fn nonnull_seats_refuse_null() {
        let mut machine = bridged();

        let error = perform(&mut machine, SELECT, &[0]).unwrap_err();

        assert!(error.to_string().contains("requires one"));

        let error = perform(&mut machine, PUT_BUFFER, &[0, 3]).unwrap_err();

        assert!(error.to_string().contains("requires one"));
    }

    // A signed plain argument sign-extends: seeking -1 from the
    // end leaves the mark one short of it.
    #[test]
    fn signed_arguments_sign_extend() {
        let mut machine = bridged();

        machine.memory.write_run(SCRATCH, b"abcd").unwrap();

        let stream = performed_value(
            &mut machine,
            STREAM_OPEN_MEMORY,
            &[SCRATCH, 4, file_mode::READ_WRITE, 0],
        );

        performed_value(
            &mut machine,
            STREAM_SET_POSITION,
            &[stream, 0xFFFF_FFFF, seek_mode::END],
        );

        assert_eq!(
            performed_value(&mut machine, STREAM_GET_POSITION, &[stream]),
            3
        );
    }

    // A memory stream opened over VM memory is retained by Glk and
    // stays live: characters put through the library land in the
    // machine's own RAM, and reads come back out of it.
    #[test]
    fn retained_arrays_stay_live() {
        let mut machine = bridged();

        let stream = performed_value(
            &mut machine,
            STREAM_OPEN_MEMORY,
            &[SCRATCH, 8, file_mode::READ_WRITE, 0],
        );

        {
            let bridge = machine.bridge.as_mut().unwrap();
            let key = bridge
                .registry
                .lookup(dispatch::CLASS_STREAM, stream)
                .unwrap();

            bridge.library.glk_stream_set_current(Some(key));
            plain(bridge.library.glk_put_string(&mut machine.memory, "hey")).unwrap();
        }

        assert_eq!(machine.memory.read_run(SCRATCH, 3).unwrap(), b"hey");

        performed_value(
            &mut machine,
            STREAM_SET_POSITION,
            &[stream, 0, seek_mode::START],
        );

        let count = performed_value(&mut machine, GET_BUFFER_STREAM, &[stream, SCRATCH + 8, 3]);

        assert_eq!(count, 3);
        assert_eq!(machine.memory.read_run(SCRATCH + 8, 3).unwrap(), b"hey");
    }

    // String arguments are unencoded string objects, type byte and
    // all: E0 for Latin-1, E2 for Unicode with values that are no
    // code point rendering as '?'. A bare byte array in a string
    // seat is refused by name.
    #[test]
    fn string_arguments_are_objects() {
        let dir = std::env::temp_dir();
        let mut machine = bridged();

        machine.glk_mut().unwrap().save_dir = dir.clone();

        let mut object = vec![0xE0];
        object.extend_from_slice(b"tale");
        object.push(0x00);
        machine.memory.write_run(TEXT, &object).unwrap();

        let ident = performed_value(
            &mut machine,
            FILEREF_CREATE_BY_NAME,
            &[file_usage::DATA, TEXT, 0],
        );

        assert!(ident > 0);

        {
            let bridge = machine.bridge.as_ref().unwrap();
            let key = bridge
                .registry
                .lookup(dispatch::CLASS_FILEREF, ident)
                .unwrap();

            assert_eq!(
                bridge.library.filerefs[&key].filename,
                dir.join("tale.glkdata").to_string_lossy()
            );
        }

        {
            let bridge = machine.bridge.as_mut().unwrap();
            let held = MemArray {
                address: SCRATCH,
                count: 4,
                width: 4,
            };
            let wide = plain(bridge.library.glk_stream_open_memory_uni(
                Some(held),
                file_mode::WRITE,
                0,
            ))
            .unwrap();

            bridge.library.glk_stream_set_current(Some(wide));
        }

        let mut object = vec![0xE2, 0x00, 0x00, 0x00];
        object.extend_from_slice(&0x2603u32.to_be_bytes());
        object.extend_from_slice(&0x110000u32.to_be_bytes());
        object.extend_from_slice(&[0, 0, 0, 0]);
        machine.memory.write_run(TEXT, &object).unwrap();

        performed_value(&mut machine, PUT_STRING_UNI, &[TEXT]);

        assert_eq!(machine.memory.read_word(SCRATCH).unwrap(), 0x2603);
        assert_eq!(
            machine.memory.read_word(SCRATCH + 4).unwrap(),
            u32::from(b'?')
        );

        machine.memory.write_run(TEXT, &[0x41, 0x00]).unwrap();

        let error = perform(&mut machine, PUT_STRING, &[TEXT]).unwrap_err();

        assert!(error.to_string().contains("not an E0"));

        let error = perform(&mut machine, PUT_STRING_UNI, &[TEXT]).unwrap_err();

        assert!(error.to_string().contains("not an E2"));
    }

    // An opaque array crosses as a snapshot of looked-up objects;
    // ids of zero are the null channel, and nothing plays where no
    // sound can.
    #[test]
    fn opaque_arrays_cross_as_objects() {
        let mut machine = bridged();

        machine.memory.write_word(SCRATCH, 0).unwrap();
        machine.memory.write_word(SCRATCH + 4, 0).unwrap();
        machine.memory.write_word(SCRATCH + 8, 3).unwrap();
        machine.memory.write_word(SCRATCH + 12, 4).unwrap();

        let started = performed_value(&mut machine, PLAY_MULTI, &[SCRATCH, 2, SCRATCH + 8, 2, 0]);

        assert_eq!(started, 0);
    }

    // The marshaller's full grammar includes input scalars, which
    // the current Glk API never uses -- they are held to the same
    // rules via a synthetic signature so the grammar stays whole.
    #[test]
    fn input_scalars_read_memory_and_stack() {
        const PROBE_ARGS: [Item; 1] = [into(U32)];
        const PROBE: Signature = Signature {
            number: 0,
            name: "probe",
            args: &PROBE_ARGS,
            result: None,
        };

        let mut machine = bridged();

        machine.memory.write_word(SCRATCH, 99).unwrap();

        let bridge = machine.bridge.as_mut().unwrap();
        let (args, outs) = bridge
            .unmarshal(&machine.memory, &mut machine.stack, &PROBE, &[SCRATCH])
            .unwrap();

        assert!(matches!(&args[0], GlkArg::Ref(Some(slot)) if slot.0.word() == 99));
        assert!(outs.is_empty());

        machine.stack.push(41).unwrap();

        let (args, outs) = bridge
            .unmarshal(&machine.memory, &mut machine.stack, &PROBE, &[STACK_REF])
            .unwrap();

        assert!(matches!(&args[0], GlkArg::Ref(Some(slot)) if slot.0.word() == 41));
        assert!(outs.is_empty());
    }

    // The glk opcode itself: selector and count as operands,
    // arguments off the stack first-topmost, the answer stored --
    // and glk_exit ends the run the way quit does.
    #[test]
    fn the_glk_opcode_calls_and_exits() {
        let mut machine = bridged();

        machine.stack.push(0).unwrap();
        machine.stack.push(0).unwrap();

        let mut plant = vec![0x81, 0x30, 0x11, 0x07, GESTALT as u8, 0x02];
        plant.extend_from_slice(&RESULT.to_be_bytes());
        plant.extend_from_slice(&[0x81, 0x30, 0x11, 0x00, 0x01, 0x00]);
        machine.memory.write_run(PLANT, &plant).unwrap();

        machine.pc = PLANT;

        machine.run(Some(10)).unwrap();

        assert_eq!(
            machine.memory.read_word(RESULT).unwrap(),
            crate::glulx::glk::api::GLK_VERSION
        );
        assert!(!machine.running());
    }

    // Without a library the glk opcode is refused by name, and
    // selecting the Glk output system falls back to null -- the
    // same truth the gestalt answers.
    #[test]
    fn no_library_means_no_glk() {
        let mut machine = Machine::new(Story::new(image(IDLE)).unwrap(), None).unwrap();

        let mut plant = vec![0x81, 0x49, 0x11, 0x02, 0x07];
        plant.extend_from_slice(&[0x81, 0x20]);
        machine.memory.write_run(PLANT, &plant).unwrap();

        machine.pc = PLANT;

        machine.run(Some(10)).unwrap();

        assert_eq!(machine.iosys.mode, 0);
        assert_eq!(machine.iosys.rock, 0);
        assert_eq!(
            crate::glulx::gestalt::answer(&machine.capabilities, 0, &[], 4, 2),
            0
        );

        machine
            .memory
            .write_run(PLANT, &[0x81, 0x30, 0x11, 0x00, 0x01, 0x00, 0x81, 0x20])
            .unwrap();

        machine.pc = PLANT;

        let error = machine.step().unwrap_err();

        assert!(error.to_string().contains("none is installed"));
    }

    // With a library installed the capability flips, iosys mode 2
    // holds, and a streamchar lands in the Glk window -- the
    // machine speaks through Glk for the first time.
    #[test]
    fn the_machine_speaks_through_glk() {
        let mut machine = bridged();

        let window = {
            let glk = machine.glk_mut().unwrap();
            let window = plain(glk.glk_window_open(None, 0, 0, window_type::TEXT_BUFFER, 0))
                .unwrap()
                .unwrap();

            glk.glk_set_window(Some(window));

            window
        };

        assert_eq!(
            crate::glulx::gestalt::answer(&machine.capabilities, 0, &[], 4, 2),
            1
        );

        let mut plant = vec![0x81, 0x49, 0x11, 0x02, 0x00];
        plant.extend_from_slice(&[0x70, 0x01, 0x41]);
        plant.extend_from_slice(&[0x81, 0x20]);
        machine.memory.write_run(PLANT, &plant).unwrap();

        machine.pc = PLANT;

        machine.run(Some(10)).unwrap();

        assert_eq!(machine.iosys.mode, 2);
        assert_eq!(machine.glk_mut().unwrap().windows[&window].text(), "A");
    }

    // A session whose select can never be answered -- the null
    // display's refusal -- stops the machine cleanly too.
    #[test]
    fn an_unanswerable_session_ends_cleanly() {
        let mut machine = bridged();

        {
            let glk = machine.glk_mut().unwrap();
            let window = plain(glk.glk_window_open(None, 0, 0, window_type::TEXT_BUFFER, 0))
                .unwrap()
                .unwrap();

            plain(glk.glk_request_char_event(Some(window))).unwrap();
        }

        let performed = perform(&mut machine, SELECT, &[SCRATCH]).unwrap();

        assert_eq!(performed, Performed::Ended);
    }

    // A select over a suspending display completes its opcode --
    // zero spoken, stack whole -- but the struct's travel back
    // into memory is deferred: the sentinel survives until the
    // host delivers the event, and every call in between is
    // refused, because a suspended machine should be standing
    // still.
    #[test]
    fn a_suspended_select_defers_its_writeback() {
        let mut machine = Machine::new(Story::new(image(IDLE)).unwrap(), None).unwrap();

        machine.install_glk(Glk::new(Box::new(Suspending)));

        let window = {
            let glk = machine.glk_mut().unwrap();
            let window = plain(glk.glk_window_open(None, 0, 0, window_type::TEXT_BUFFER, 0))
                .unwrap()
                .unwrap();

            plain(glk.glk_request_char_event(Some(window))).unwrap();

            window
        };

        for index in 0..4 {
            machine
                .memory
                .write_word(SCRATCH + 4 * index, 0xDEAD_BEEF)
                .unwrap();
        }

        let performed = perform(&mut machine, SELECT, &[SCRATCH]).unwrap();

        assert_eq!(performed, Performed::Suspended(0));
        assert_eq!(machine.memory.read_word(SCRATCH).unwrap(), 0xDEAD_BEEF);

        let error = perform(&mut machine, GESTALT, &[0, 0]).unwrap_err();

        assert!(error.to_string().contains("stands suspended"));

        let event = {
            let glk = machine.glk_mut().unwrap();

            plain(glk.deliver_char(window, 0x41)).unwrap()
        };

        machine.deliver_event(event).unwrap();

        assert_eq!(
            machine.memory.read_word(SCRATCH).unwrap(),
            event_type::CHAR_INPUT
        );
        assert_eq!(machine.memory.read_word(SCRATCH + 4).unwrap(), 1);
        assert_eq!(machine.memory.read_word(SCRATCH + 8).unwrap(), 0x41);
        assert_eq!(machine.memory.read_word(SCRATCH + 12).unwrap(), 0);
    }

    // The suspension round trip through the run loop: the story
    // opens a window, asks for a keystroke, and selects; run
    // returns mid-story with the machine still running and the
    // struct's memory untouched. The delivered event lands where
    // the game will look, and the next run continues from the
    // instruction after the select, all the way to quit.
    #[test]
    fn a_suspended_machine_stands_and_steps_on() {
        const SEAT: u32 = 0x1C0;

        let mut program = vec![0xC0, 0x00, 0x00];
        // glk window_open(split 0, method 0, size 0, buffer, rock
        // 0), the five arguments pushed last-first, the window id
        // stored into RESULT.
        program.extend_from_slice(&[0x40, 0x81, 0x00]);
        program.extend_from_slice(&[0x40, 0x81, 0x03]);
        program.extend_from_slice(&[0x40, 0x81, 0x00]);
        program.extend_from_slice(&[0x40, 0x81, 0x00]);
        program.extend_from_slice(&[0x40, 0x81, 0x00]);
        program.extend_from_slice(&[0x81, 0x30, 0x11, 0x06, 0x23, 0x05, 0x01, 0x40]);
        // glk request_char_event(the window), fetched from RESULT.
        program.extend_from_slice(&[0x40, 0x86, 0x01, 0x40]);
        program.extend_from_slice(&[0x81, 0x30, 0x12, 0x00, 0x00, 0xD2, 0x01]);
        // glk select(SEAT) -- the machine stands down here.
        program.extend_from_slice(&[0x40, 0x82, 0x01, 0xC0]);
        program.extend_from_slice(&[0x81, 0x30, 0x12, 0x00, 0x00, 0xC0, 0x01]);
        // The resume's proof: 7 into RESULT + 4, then quit.
        program.extend_from_slice(&[0x40, 0x61, 0x07, 0x01, 0x44]);
        program.extend_from_slice(&[0x81, 0x20]);

        let mut machine = Machine::new(Story::new(image(&program)).unwrap(), None).unwrap();

        machine.install_glk(Glk::new(Box::new(Suspending)));

        machine.memory.write_word(SEAT, 0xDEAD_BEEF).unwrap();

        assert_eq!(machine.run(None).unwrap(), 10);
        assert!(machine.running());
        assert!(machine.suspended());
        assert_eq!(machine.memory.read_word(SEAT).unwrap(), 0xDEAD_BEEF);

        let window = machine.glk_mut().unwrap().root.expect("the window opened");
        let event = {
            let glk = machine.glk_mut().unwrap();

            plain(glk.deliver_char(window, 0x41)).unwrap()
        };

        machine.deliver_event(event).unwrap();

        assert_eq!(
            machine.memory.read_word(SEAT).unwrap(),
            event_type::CHAR_INPUT
        );
        assert_eq!(machine.memory.read_word(SEAT + 8).unwrap(), 0x41);

        machine.run(None).unwrap();

        assert!(!machine.running());
        assert_eq!(machine.memory.read_word(RESULT + 4).unwrap(), 7);
    }

    // The mid-call round trip: the prompt suspends the call itself
    // -- the sentinel untouched, the store parked -- and the
    // delivered name lands the minted fileref in it; a cancel
    // lands the null reference.
    #[test]
    fn a_prompt_suspends_mid_call() {
        let prompts: &[u8] = &[
            0xC0, 0x00, 0x00, 0x40, 0x81, 0x00, 0x40, 0x81, 0x01, 0x40, 0x81, 0x01, 0x81, 0x30,
            0x11, 0x06, 0x62, 0x03, 0x01, 0x40, 0x81, 0x20,
        ];

        let mut machine = Machine::new(Story::new(image(prompts)).unwrap(), None).unwrap();

        machine.install_glk(Glk::new(Box::new(Suspending)));
        machine.glk_mut().unwrap().save_dir = std::env::temp_dir();

        machine.memory.write_word(RESULT, 0xDEAD_BEEF).unwrap();
        machine.run(None).unwrap();

        assert!(machine.running());
        assert!(machine.suspended());
        assert_eq!(machine.memory.read_word(RESULT).unwrap(), 0xDEAD_BEEF);

        machine.deliver_file(Some("saga")).unwrap();

        assert_eq!(machine.memory.read_word(RESULT).unwrap(), 1);

        machine.run(None).unwrap();

        assert!(!machine.running());

        let mut fresh = Machine::new(Story::new(image(prompts)).unwrap(), None).unwrap();

        fresh.install_glk(Glk::new(Box::new(Suspending)));

        fresh.memory.write_word(RESULT, 0xDEAD_BEEF).unwrap();
        fresh.run(None).unwrap();
        fresh.deliver_file(None).unwrap();

        assert_eq!(fresh.memory.read_word(RESULT).unwrap(), 0);

        fresh.run(None).unwrap();

        assert!(!fresh.running());
    }

    // Save and restore through a Glk stream: the file lands in the
    // stream, and restoring pours the state back -- execution
    // resumes after the save with -1 stored, the turn's changes
    // gone.
    #[test]
    fn save_and_restore_ride_a_glk_stream() {
        const MARKER: u32 = 0x160;
        const SECOND: u32 = 0x148;

        // A roomier map: the save file lands inside VM RAM.
        let mut data = image(IDLE);
        data[16..20].copy_from_slice(&0x2000u32.to_be_bytes());

        let mut machine = Machine::new(Story::new(data).unwrap(), None).unwrap();

        machine.install_glk(Glk::over_nothing());

        let (stream_key, ident) = {
            let held = MemArray {
                address: 0x1000,
                count: 0xE00,
                width: 1,
            };
            let glk = machine.glk_mut().unwrap();
            let key =
                plain(glk.glk_stream_open_memory(Some(held), file_mode::READ_WRITE, 0)).unwrap();
            let bridge = machine.bridge.as_mut().unwrap();
            let ident = bridge.registry.register(dispatch::CLASS_STREAM, Some(key));

            (key, ident)
        };

        let mut save = vec![0x81, 0x23, 0x71, ident as u8];
        save.extend_from_slice(&RESULT.to_be_bytes());

        machine.memory.write_run(PLANT, &save).unwrap();

        machine.pc = PLANT;

        machine.step().unwrap();

        let resumed = PLANT + save.len() as u32;

        assert_eq!(machine.memory.read_word(RESULT).unwrap(), 0);

        // FORM opens the file the stream holds.
        assert_eq!(machine.memory.read_run(0x1000, 4).unwrap(), b"FORM");

        machine.memory.write_byte(MARKER, 0x99).unwrap();

        {
            let glk = machine.glk_mut().unwrap();

            plain(glk.glk_stream_set_position(Some(stream_key), 0, seek_mode::START)).unwrap();
        }

        let mut restore = vec![0x81, 0x24, 0x71, ident as u8];
        restore.extend_from_slice(&SECOND.to_be_bytes());

        machine.memory.write_run(PLANT + 0x20, &restore).unwrap();

        machine.pc = PLANT + 0x20;

        machine.step().unwrap();

        assert_eq!(machine.pc, resumed);
        assert_eq!(machine.memory.read_word(RESULT).unwrap(), 0xFFFF_FFFF);
        assert_eq!(machine.memory.read_byte(MARKER).unwrap(), 0);
        assert_eq!(machine.memory.read_word(SECOND).unwrap(), 0);
    }

    // Every way a save or restore can fail speaks 1 rather than
    // faulting: an id naming no stream, a stream that cannot be
    // written or read, and a machine with no Glk at all.
    #[test]
    fn failed_saves_speak_one() {
        const SECOND: u32 = 0x148;

        let mut machine = bridged();

        let mut save_unknown = vec![0x81, 0x23, 0x71, 0x63];
        save_unknown.extend_from_slice(&RESULT.to_be_bytes());

        machine.memory.write_run(PLANT, &save_unknown).unwrap();

        machine.pc = PLANT;

        machine.step().unwrap();

        assert_eq!(machine.memory.read_word(RESULT).unwrap(), 1);

        // A read-only stream cannot take a save; a write-only
        // stream cannot give a restore.
        let (readable, writable) = {
            let held = MemArray {
                address: SCRATCH,
                count: 8,
                width: 1,
            };
            let glk = machine.glk_mut().unwrap();
            let readable =
                plain(glk.glk_stream_open_memory(Some(held), file_mode::READ, 0)).unwrap();
            let writable =
                plain(glk.glk_stream_open_memory(Some(held), file_mode::WRITE, 0)).unwrap();

            (readable, writable)
        };

        assert_eq!(
            crate::glulx::serial::save(&mut machine, Some(readable)).unwrap(),
            crate::glulx::serial::FAILED
        );
        assert_eq!(
            crate::glulx::serial::restore(&mut machine, Some(writable)).unwrap(),
            crate::glulx::serial::FAILED
        );
        assert_eq!(
            crate::glulx::serial::restore(&mut machine, Some(readable)).unwrap(),
            crate::glulx::serial::FAILED
        );

        let mut restore_unknown = vec![0x81, 0x24, 0x71, 0x63];
        restore_unknown.extend_from_slice(&SECOND.to_be_bytes());

        machine.memory.write_run(PLANT, &restore_unknown).unwrap();

        machine.pc = PLANT;

        machine.step().unwrap();

        assert_eq!(machine.memory.read_word(SECOND).unwrap(), 1);
    }

    // A step that suspends reports Step::Suspended, and a plain
    // one Step::Ran.
    #[test]
    fn steps_name_their_suspensions() {
        let mut machine = Machine::new(Story::new(image(IDLE)).unwrap(), None).unwrap();

        machine.install_glk(Glk::new(Box::new(Suspending)));

        {
            let glk = machine.glk_mut().unwrap();
            let window = plain(glk.glk_window_open(None, 0, 0, window_type::TEXT_BUFFER, 0))
                .unwrap()
                .unwrap();

            plain(glk.glk_request_char_event(Some(window))).unwrap();
        }

        // glk select(SCRATCH): two stack args then the opcode.
        machine.stack.push(SCRATCH).unwrap();
        machine
            .memory
            .write_run(PLANT, &[0x81, 0x30, 0x12, 0x00, 0x00, 0xC0, 0x01])
            .unwrap();

        machine.pc = PLANT;

        assert_eq!(machine.step().unwrap(), Step::Suspended);
        assert!(machine.suspended());
    }
}
