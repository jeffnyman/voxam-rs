//! The Glk dispatch layer: every function's signature.
//!
//! The C world's gi_dispa.c (vendored with cheapglk) hand-writes a
//! thousand-line switch returning prototype strings like
//! `"4&#!CnIuIu:Qb"`. Voxam does the inverse: each function is
//! declared as a readable argument list, and the prototype string
//! is *generated* from it. The generated strings are then checked
//! against the ones parsed out of gi_dispa.c -- for every function
//! -- in the tests, which turns a transcription error into a test
//! failure instead of a runtime mystery.
//!
//! The grammar, as gi_dispa.c defines it: a prototype is a count
//! followed by items, like `"3Qa<Iu:Qa"` for glk_window_iterate.
//! The count includes the return value, which is the item carrying
//! the `:` prefix; a void function ends with a bare `:` that is
//! not counted. Prefixes appear in the order `[ref][+][#][!]`
//! before the type code -- reference direction, nonnull, array,
//! retained.

/// The opaque class numbers, from gi_dispa.h. The prototype codes
/// Qa through Qd map onto these in order.
pub const CLASS_WINDOW: u32 = 0;
pub const CLASS_STREAM: u32 = 1;
pub const CLASS_FILEREF: u32 = 2;
pub const CLASS_SCHANNEL: u32 = 3;

/// The type codes the table is written in. UNICHAR arguments are
/// plain U32 on purpose: a Unicode character argument is a full
/// word, where the Latin-1 char types are bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// Iu: an unsigned word.
    U32,
    /// Is: a signed word.
    I32,
    /// Cn: an unsigned Latin-1 character, one byte in an array.
    Char,
    /// Cu: an unsigned character argument.
    UChar,
    /// S: the address of an unencoded (E0) string object.
    CString,
    /// U: the address of an unencoded Unicode (E2) string object.
    UString,
    /// Qa: a window id.
    Window,
    /// Qb: a stream id.
    Stream,
    /// Qc: a fileref id.
    Fileref,
    /// Qd: a sound channel id.
    Schannel,
}

impl Code {
    fn prototype(self) -> &'static str {
        match self {
            Self::U32 => "Iu",
            Self::I32 => "Is",
            Self::Char => "Cn",
            Self::UChar => "Cu",
            Self::CString => "S",
            Self::UString => "U",
            Self::Window => "Qa",
            Self::Stream => "Qb",
            Self::Fileref => "Qc",
            Self::Schannel => "Qd",
        }
    }
}

/// The reference direction a prototype prefix spells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ref {
    /// A plain value.
    None,
    /// `<`: an output reference -- Glk writes, the game reads.
    Out,
    /// `>`: an input reference -- the game writes, Glk reads.
    In,
    /// `&`: a reference passing both ways.
    InOut,
}

impl Ref {
    fn prototype(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Out => "<",
            Self::In => ">",
            Self::InOut => "&",
        }
    }
}

/// One item in a prototype: an argument, or the return value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    /// The type code, or None when the item is a struct.
    pub code: Option<Code>,
    /// The field types, when this item is a struct.
    pub fields: &'static [Item],
    /// The reference direction.
    pub ref_dir: Ref,
    /// Whether the item is an array, consuming an address and a
    /// count.
    pub array: bool,
    /// Whether a null reference is forbidden.
    pub nonnull: bool,
    /// Whether Glk keeps the array after the call.
    pub retained: bool,
}

impl Item {
    /// Whether this item is a struct of fields.
    pub fn is_struct(&self) -> bool {
        !self.fields.is_empty()
    }

    /// Whether this item is one of the four opaque classes.
    pub fn is_opaque(&self) -> bool {
        self.opaque_class().is_some()
    }

    /// The opaque class number, or None for a plain type.
    pub fn opaque_class(&self) -> Option<u32> {
        match self.code {
            Some(Code::Window) => Some(CLASS_WINDOW),
            Some(Code::Stream) => Some(CLASS_STREAM),
            Some(Code::Fileref) => Some(CLASS_FILEREF),
            Some(Code::Schannel) => Some(CLASS_SCHANNEL),
            _ => None,
        }
    }

    /// Whether this item is a string object address.
    pub fn is_string(&self) -> bool {
        matches!(self.code, Some(Code::CString | Code::UString))
    }

    /// Whether this item's value is signed.
    pub fn signed(&self) -> bool {
        matches!(self.code, Some(Code::I32))
    }

    /// Bytes per element: 1 for the char types, 4 otherwise.
    pub fn element_size(&self) -> u32 {
        match self.code {
            Some(Code::Char | Code::UChar) => 1,
            _ => 4,
        }
    }

    /// Whether a value passes from the game into Glk.
    pub fn passes_in(&self) -> bool {
        matches!(self.ref_dir, Ref::In | Ref::InOut)
    }

    /// Whether a value passes from Glk back to the game.
    pub fn passes_out(&self) -> bool {
        matches!(self.ref_dir, Ref::Out | Ref::InOut)
    }

    /// Whether the game passes an address rather than a value.
    pub fn is_reference(&self) -> bool {
        !matches!(self.ref_dir, Ref::None)
    }

    /// How many 32-bit Glulx arguments this item consumes.
    ///
    /// "An array argument, unlike a string argument, is always
    /// followed by an array length argument" -- so an array is two
    /// words where everything else is one (Glulx: Miscellaneous,
    /// under the glk opcode).
    pub fn word_count(&self) -> u32 {
        if self.array { 2 } else { 1 }
    }

    /// This item rendered in gi_dispa.c's prototype grammar, its
    /// reference prefix spelled as given -- the return value wears
    /// ":" in place of a direction.
    fn prototype_as(&self, ref_prefix: &str) -> String {
        let mut out = String::from(ref_prefix);

        if self.nonnull {
            out.push('+');
        }

        if self.array {
            out.push('#');
        }

        if self.retained {
            out.push('!');
        }

        if self.is_struct() {
            out.push('[');
            out.push_str(&self.fields.len().to_string());

            for field in self.fields {
                out.push_str(&field.prototype());
            }

            out.push(']');
        } else if let Some(code) = self.code {
            out.push_str(code.prototype());
        }

        out
    }

    /// This item rendered with its own reference prefix.
    pub fn prototype(&self) -> String {
        self.prototype_as(self.ref_dir.prototype())
    }
}

/// A plain value of a type code.
const fn plain(code: Code) -> Item {
    Item {
        code: Some(code),
        fields: &[],
        ref_dir: Ref::None,
        array: false,
        nonnull: false,
        retained: false,
    }
}

/// An output reference: Glk writes, the game reads.
pub const fn out(item: Item) -> Item {
    Item {
        ref_dir: Ref::Out,
        ..item
    }
}

/// An output reference that forbids null.
pub const fn out_nonnull(item: Item) -> Item {
    Item {
        ref_dir: Ref::Out,
        nonnull: true,
        ..item
    }
}

/// An input reference: the game writes, Glk reads.
pub const fn into(item: Item) -> Item {
    Item {
        ref_dir: Ref::In,
        ..item
    }
}

/// An input reference that forbids null.
pub const fn into_nonnull(item: Item) -> Item {
    Item {
        ref_dir: Ref::In,
        nonnull: true,
        ..item
    }
}

/// A reference passing both ways.
pub const fn inout(item: Item) -> Item {
    Item {
        ref_dir: Ref::InOut,
        ..item
    }
}

/// An array of items: an address and a count, two words.
pub const fn array(item: Item, ref_dir: Ref, nonnull: bool, retained: bool) -> Item {
    Item {
        ref_dir,
        array: true,
        nonnull,
        retained,
        ..item
    }
}

/// A struct of fields, passed as one reference.
const fn fields(fields: &'static [Item]) -> Item {
    Item {
        code: None,
        fields,
        ref_dir: Ref::None,
        array: false,
        nonnull: false,
        retained: false,
    }
}

// The atoms the table is written in.
pub const U32: Item = plain(Code::U32);
pub const I32: Item = plain(Code::I32);
pub const CHAR: Item = plain(Code::Char);
pub const UCHAR: Item = plain(Code::UChar);
pub const CSTRING: Item = plain(Code::CString);
pub const USTRING: Item = plain(Code::UString);

pub const WINDOW: Item = plain(Code::Window);
pub const STREAM: Item = plain(Code::Stream);
pub const FILEREF: Item = plain(Code::Fileref);
pub const SCHANNEL: Item = plain(Code::Schannel);

/// The well-known structures, named in gi_dispa.h: event_t,
/// stream_result_t, glktimeval_t, glkdate_t.
pub const EVENT: Item = fields(&[U32, WINDOW, U32, U32]);
pub const STREAM_RESULT: Item = fields(&[U32, U32]);
pub const TIMEVAL: Item = fields(&[I32, U32, I32]);
pub const DATE: Item = fields(&[I32, I32, I32, I32, I32, I32, I32, I32]);

/// One Glk function's dispatch signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature {
    /// The selector the glk opcode names the function by.
    pub number: u32,
    /// The bare function name, without the glk_ prefix.
    pub name: &'static str,
    /// The argument items, in call order.
    pub args: &'static [Item],
    /// The return item, or None for a void function.
    pub result: Option<Item>,
}

impl Signature {
    /// The function's full name, glk_ prefix included.
    pub fn glk_name(&self) -> String {
        format!("glk_{}", self.name)
    }

    /// Total 32-bit arguments the glk opcode must supply.
    pub fn word_count(&self) -> u32 {
        self.args.iter().map(Item::word_count).sum()
    }

    /// The whole signature in gi_dispa.c's prototype grammar.
    pub fn prototype(&self) -> String {
        let count = self.args.len() + usize::from(self.result.is_some());
        let body: String = self.args.iter().map(Item::prototype).collect();
        let tail = match &self.result {
            Some(result) => result.prototype_as(":"),
            None => ":".into(),
        };

        format!("{count}{body}{tail}")
    }
}

const fn sig(
    number: u32,
    name: &'static str,
    args: &'static [Item],
    result: Option<Item>,
) -> Signature {
    Signature {
        number,
        name,
        args,
        result,
    }
}

/// The table, ordered as in gi_dispa.c and generated from the
/// reference's own declarations. glk_set_interrupt_handler
/// (0x0002) is absent on purpose: its prototype there is NULL,
/// meaning it cannot be invoked through the dispatch layer at all.
pub const SIGNATURES: &[Signature] = &[
    sig(0x0001, "exit", &[], None),
    sig(0x0003, "tick", &[], None),
    sig(0x0004, "gestalt", &[U32, U32], Some(U32)),
    sig(
        0x0005,
        "gestalt_ext",
        &[U32, U32, array(U32, Ref::InOut, false, false)],
        Some(U32),
    ),
    sig(0x0020, "window_iterate", &[WINDOW, out(U32)], Some(WINDOW)),
    sig(0x0021, "window_get_rock", &[WINDOW], Some(U32)),
    sig(0x0022, "window_get_root", &[], Some(WINDOW)),
    sig(
        0x0023,
        "window_open",
        &[WINDOW, U32, U32, U32, U32],
        Some(WINDOW),
    ),
    sig(0x0024, "window_close", &[WINDOW, out(STREAM_RESULT)], None),
    sig(
        0x0025,
        "window_get_size",
        &[WINDOW, out(U32), out(U32)],
        None,
    ),
    sig(
        0x0026,
        "window_set_arrangement",
        &[WINDOW, U32, U32, WINDOW],
        None,
    ),
    sig(
        0x0027,
        "window_get_arrangement",
        &[WINDOW, out(U32), out(U32), out(WINDOW)],
        None,
    ),
    sig(0x0028, "window_get_type", &[WINDOW], Some(U32)),
    sig(0x0029, "window_get_parent", &[WINDOW], Some(WINDOW)),
    sig(0x002A, "window_clear", &[WINDOW], None),
    sig(0x002B, "window_move_cursor", &[WINDOW, U32, U32], None),
    sig(0x002C, "window_get_stream", &[WINDOW], Some(STREAM)),
    sig(0x002D, "window_set_echo_stream", &[WINDOW, STREAM], None),
    sig(0x002E, "window_get_echo_stream", &[WINDOW], Some(STREAM)),
    sig(0x002F, "set_window", &[WINDOW], None),
    sig(0x0030, "window_get_sibling", &[WINDOW], Some(WINDOW)),
    sig(0x0040, "stream_iterate", &[STREAM, out(U32)], Some(STREAM)),
    sig(0x0041, "stream_get_rock", &[STREAM], Some(U32)),
    sig(
        0x0042,
        "stream_open_file",
        &[FILEREF, U32, U32],
        Some(STREAM),
    ),
    sig(
        0x0043,
        "stream_open_memory",
        &[array(CHAR, Ref::InOut, false, true), U32, U32],
        Some(STREAM),
    ),
    sig(0x0044, "stream_close", &[STREAM, out(STREAM_RESULT)], None),
    sig(0x0045, "stream_set_position", &[STREAM, I32, U32], None),
    sig(0x0046, "stream_get_position", &[STREAM], Some(U32)),
    sig(0x0047, "stream_set_current", &[STREAM], None),
    sig(0x0048, "stream_get_current", &[], Some(STREAM)),
    sig(0x0049, "stream_open_resource", &[U32, U32], Some(STREAM)),
    sig(0x0060, "fileref_create_temp", &[U32, U32], Some(FILEREF)),
    sig(
        0x0061,
        "fileref_create_by_name",
        &[U32, CSTRING, U32],
        Some(FILEREF),
    ),
    sig(
        0x0062,
        "fileref_create_by_prompt",
        &[U32, U32, U32],
        Some(FILEREF),
    ),
    sig(0x0063, "fileref_destroy", &[FILEREF], None),
    sig(
        0x0064,
        "fileref_iterate",
        &[FILEREF, out(U32)],
        Some(FILEREF),
    ),
    sig(0x0065, "fileref_get_rock", &[FILEREF], Some(U32)),
    sig(0x0066, "fileref_delete_file", &[FILEREF], None),
    sig(0x0067, "fileref_does_file_exist", &[FILEREF], Some(U32)),
    sig(
        0x0068,
        "fileref_create_from_fileref",
        &[U32, FILEREF, U32],
        Some(FILEREF),
    ),
    sig(0x0080, "put_char", &[UCHAR], None),
    sig(0x0081, "put_char_stream", &[STREAM, UCHAR], None),
    sig(0x0082, "put_string", &[CSTRING], None),
    sig(0x0083, "put_string_stream", &[STREAM, CSTRING], None),
    sig(
        0x0084,
        "put_buffer",
        &[array(CHAR, Ref::In, true, false)],
        None,
    ),
    sig(
        0x0085,
        "put_buffer_stream",
        &[STREAM, array(CHAR, Ref::In, true, false)],
        None,
    ),
    sig(0x0086, "set_style", &[U32], None),
    sig(0x0087, "set_style_stream", &[STREAM, U32], None),
    sig(0x0090, "get_char_stream", &[STREAM], Some(I32)),
    sig(
        0x0091,
        "get_line_stream",
        &[STREAM, array(CHAR, Ref::Out, true, false)],
        Some(U32),
    ),
    sig(
        0x0092,
        "get_buffer_stream",
        &[STREAM, array(CHAR, Ref::Out, true, false)],
        Some(U32),
    ),
    sig(0x00A0, "char_to_lower", &[UCHAR], Some(UCHAR)),
    sig(0x00A1, "char_to_upper", &[UCHAR], Some(UCHAR)),
    sig(0x00B0, "stylehint_set", &[U32, U32, U32, I32], None),
    sig(0x00B1, "stylehint_clear", &[U32, U32, U32], None),
    sig(0x00B2, "style_distinguish", &[WINDOW, U32, U32], Some(U32)),
    sig(
        0x00B3,
        "style_measure",
        &[WINDOW, U32, U32, out(U32)],
        Some(U32),
    ),
    sig(0x00C0, "select", &[out_nonnull(EVENT)], None),
    sig(0x00C1, "select_poll", &[out_nonnull(EVENT)], None),
    sig(
        0x00D0,
        "request_line_event",
        &[WINDOW, array(CHAR, Ref::InOut, true, true), U32],
        None,
    ),
    sig(0x00D1, "cancel_line_event", &[WINDOW, out(EVENT)], None),
    sig(0x00D2, "request_char_event", &[WINDOW], None),
    sig(0x00D3, "cancel_char_event", &[WINDOW], None),
    sig(0x00D4, "request_mouse_event", &[WINDOW], None),
    sig(0x00D5, "cancel_mouse_event", &[WINDOW], None),
    sig(0x00D6, "request_timer_events", &[U32], None),
    sig(
        0x00E0,
        "image_get_info",
        &[U32, out(U32), out(U32)],
        Some(U32),
    ),
    sig(0x00E1, "image_draw", &[WINDOW, U32, I32, I32], Some(U32)),
    sig(
        0x00E2,
        "image_draw_scaled",
        &[WINDOW, U32, I32, I32, U32, U32],
        Some(U32),
    ),
    sig(0x00E8, "window_flow_break", &[WINDOW], None),
    sig(
        0x00E9,
        "window_erase_rect",
        &[WINDOW, I32, I32, U32, U32],
        None,
    ),
    sig(
        0x00EA,
        "window_fill_rect",
        &[WINDOW, U32, I32, I32, U32, U32],
        None,
    ),
    sig(0x00EB, "window_set_background_color", &[WINDOW, U32], None),
    sig(
        0x00EC,
        "image_draw_scaled_ext",
        &[WINDOW, U32, I32, I32, U32, U32, U32, U32],
        Some(U32),
    ),
    sig(
        0x00F0,
        "schannel_iterate",
        &[SCHANNEL, out(U32)],
        Some(SCHANNEL),
    ),
    sig(0x00F1, "schannel_get_rock", &[SCHANNEL], Some(U32)),
    sig(0x00F2, "schannel_create", &[U32], Some(SCHANNEL)),
    sig(0x00F3, "schannel_destroy", &[SCHANNEL], None),
    sig(0x00F4, "schannel_create_ext", &[U32, U32], Some(SCHANNEL)),
    sig(
        0x00F7,
        "schannel_play_multi",
        &[
            array(SCHANNEL, Ref::In, true, false),
            array(U32, Ref::In, true, false),
            U32,
        ],
        Some(U32),
    ),
    sig(0x00F8, "schannel_play", &[SCHANNEL, U32], Some(U32)),
    sig(
        0x00F9,
        "schannel_play_ext",
        &[SCHANNEL, U32, U32, U32],
        Some(U32),
    ),
    sig(0x00FA, "schannel_stop", &[SCHANNEL], None),
    sig(0x00FB, "schannel_set_volume", &[SCHANNEL, U32], None),
    sig(0x00FC, "sound_load_hint", &[U32, U32], None),
    sig(
        0x00FD,
        "schannel_set_volume_ext",
        &[SCHANNEL, U32, U32, U32],
        None,
    ),
    sig(0x00FE, "schannel_pause", &[SCHANNEL], None),
    sig(0x00FF, "schannel_unpause", &[SCHANNEL], None),
    sig(0x0100, "set_hyperlink", &[U32], None),
    sig(0x0101, "set_hyperlink_stream", &[STREAM, U32], None),
    sig(0x0102, "request_hyperlink_event", &[WINDOW], None),
    sig(0x0103, "cancel_hyperlink_event", &[WINDOW], None),
    sig(
        0x0120,
        "buffer_to_lower_case_uni",
        &[array(U32, Ref::InOut, true, false), U32],
        Some(U32),
    ),
    sig(
        0x0121,
        "buffer_to_upper_case_uni",
        &[array(U32, Ref::InOut, true, false), U32],
        Some(U32),
    ),
    sig(
        0x0122,
        "buffer_to_title_case_uni",
        &[array(U32, Ref::InOut, true, false), U32, U32],
        Some(U32),
    ),
    sig(
        0x0123,
        "buffer_canon_decompose_uni",
        &[array(U32, Ref::InOut, true, false), U32],
        Some(U32),
    ),
    sig(
        0x0124,
        "buffer_canon_normalize_uni",
        &[array(U32, Ref::InOut, true, false), U32],
        Some(U32),
    ),
    sig(0x0128, "put_char_uni", &[U32], None),
    sig(0x0129, "put_string_uni", &[USTRING], None),
    sig(
        0x012A,
        "put_buffer_uni",
        &[array(U32, Ref::In, true, false)],
        None,
    ),
    sig(0x012B, "put_char_stream_uni", &[STREAM, U32], None),
    sig(0x012C, "put_string_stream_uni", &[STREAM, USTRING], None),
    sig(
        0x012D,
        "put_buffer_stream_uni",
        &[STREAM, array(U32, Ref::In, true, false)],
        None,
    ),
    sig(0x0130, "get_char_stream_uni", &[STREAM], Some(I32)),
    sig(
        0x0131,
        "get_buffer_stream_uni",
        &[STREAM, array(U32, Ref::Out, true, false)],
        Some(U32),
    ),
    sig(
        0x0132,
        "get_line_stream_uni",
        &[STREAM, array(U32, Ref::Out, true, false)],
        Some(U32),
    ),
    sig(
        0x0138,
        "stream_open_file_uni",
        &[FILEREF, U32, U32],
        Some(STREAM),
    ),
    sig(
        0x0139,
        "stream_open_memory_uni",
        &[array(U32, Ref::InOut, false, true), U32, U32],
        Some(STREAM),
    ),
    sig(
        0x013A,
        "stream_open_resource_uni",
        &[U32, U32],
        Some(STREAM),
    ),
    sig(0x0140, "request_char_event_uni", &[WINDOW], None),
    sig(
        0x0141,
        "request_line_event_uni",
        &[WINDOW, array(U32, Ref::InOut, true, true), U32],
        None,
    ),
    sig(0x0150, "set_echo_line_event", &[WINDOW, U32], None),
    sig(
        0x0151,
        "set_terminators_line_event",
        &[WINDOW, array(U32, Ref::In, false, false)],
        None,
    ),
    sig(0x0160, "current_time", &[out_nonnull(TIMEVAL)], None),
    sig(0x0161, "current_simple_time", &[U32], Some(I32)),
    sig(
        0x0168,
        "time_to_date_utc",
        &[into_nonnull(TIMEVAL), out_nonnull(DATE)],
        None,
    ),
    sig(
        0x0169,
        "time_to_date_local",
        &[into_nonnull(TIMEVAL), out_nonnull(DATE)],
        None,
    ),
    sig(
        0x016A,
        "simple_time_to_date_utc",
        &[I32, U32, out_nonnull(DATE)],
        None,
    ),
    sig(
        0x016B,
        "simple_time_to_date_local",
        &[I32, U32, out_nonnull(DATE)],
        None,
    ),
    sig(
        0x016C,
        "date_to_time_utc",
        &[into_nonnull(DATE), out_nonnull(TIMEVAL)],
        None,
    ),
    sig(
        0x016D,
        "date_to_time_local",
        &[into_nonnull(DATE), out_nonnull(TIMEVAL)],
        None,
    ),
    sig(
        0x016E,
        "date_to_simple_time_utc",
        &[into_nonnull(DATE), U32],
        Some(I32),
    ),
    sig(
        0x016F,
        "date_to_simple_time_local",
        &[into_nonnull(DATE), U32],
        Some(I32),
    ),
];

/// Return the signature for a Glk selector, or None if unknown.
pub fn lookup(number: u32) -> Option<&'static Signature> {
    SIGNATURES
        .binary_search_by_key(&number, |signature| signature.number)
        .ok()
        .map(|index| &SIGNATURES[index])
}

#[cfg(test)]
mod tests {
    use super::*;

    // Parsed verbatim out of gidispatch_prototype() in cheapglk's
    // gi_dispa.c, vendored at entharion/vendor/cheapglk, by way of
    // the reference implementation's identical pinned table.
    // Embedded here rather than read live because the suite must
    // run from a plain checkout, submodules present or not -- and
    // the vendored reference is pinned, so these strings are
    // constants, not a moving target.
    const GI_DISPA: &[(u32, &str, &str)] = &[
        (0x0001, "exit", "0:"),
        (0x0003, "tick", "0:"),
        (0x0004, "gestalt", "3IuIu:Iu"),
        (0x0005, "gestalt_ext", "4IuIu&#Iu:Iu"),
        (0x0020, "window_iterate", "3Qa<Iu:Qa"),
        (0x0021, "window_get_rock", "2Qa:Iu"),
        (0x0022, "window_get_root", "1:Qa"),
        (0x0023, "window_open", "6QaIuIuIuIu:Qa"),
        (0x0024, "window_close", "2Qa<[2IuIu]:"),
        (0x0025, "window_get_size", "3Qa<Iu<Iu:"),
        (0x0026, "window_set_arrangement", "4QaIuIuQa:"),
        (0x0027, "window_get_arrangement", "4Qa<Iu<Iu<Qa:"),
        (0x0028, "window_get_type", "2Qa:Iu"),
        (0x0029, "window_get_parent", "2Qa:Qa"),
        (0x002A, "window_clear", "1Qa:"),
        (0x002B, "window_move_cursor", "3QaIuIu:"),
        (0x002C, "window_get_stream", "2Qa:Qb"),
        (0x002D, "window_set_echo_stream", "2QaQb:"),
        (0x002E, "window_get_echo_stream", "2Qa:Qb"),
        (0x002F, "set_window", "1Qa:"),
        (0x0030, "window_get_sibling", "2Qa:Qa"),
        (0x0040, "stream_iterate", "3Qb<Iu:Qb"),
        (0x0041, "stream_get_rock", "2Qb:Iu"),
        (0x0042, "stream_open_file", "4QcIuIu:Qb"),
        (0x0043, "stream_open_memory", "4&#!CnIuIu:Qb"),
        (0x0044, "stream_close", "2Qb<[2IuIu]:"),
        (0x0045, "stream_set_position", "3QbIsIu:"),
        (0x0046, "stream_get_position", "2Qb:Iu"),
        (0x0047, "stream_set_current", "1Qb:"),
        (0x0048, "stream_get_current", "1:Qb"),
        (0x0049, "stream_open_resource", "3IuIu:Qb"),
        (0x0060, "fileref_create_temp", "3IuIu:Qc"),
        (0x0061, "fileref_create_by_name", "4IuSIu:Qc"),
        (0x0062, "fileref_create_by_prompt", "4IuIuIu:Qc"),
        (0x0063, "fileref_destroy", "1Qc:"),
        (0x0064, "fileref_iterate", "3Qc<Iu:Qc"),
        (0x0065, "fileref_get_rock", "2Qc:Iu"),
        (0x0066, "fileref_delete_file", "1Qc:"),
        (0x0067, "fileref_does_file_exist", "2Qc:Iu"),
        (0x0068, "fileref_create_from_fileref", "4IuQcIu:Qc"),
        (0x0080, "put_char", "1Cu:"),
        (0x0081, "put_char_stream", "2QbCu:"),
        (0x0082, "put_string", "1S:"),
        (0x0083, "put_string_stream", "2QbS:"),
        (0x0084, "put_buffer", "1>+#Cn:"),
        (0x0085, "put_buffer_stream", "2Qb>+#Cn:"),
        (0x0086, "set_style", "1Iu:"),
        (0x0087, "set_style_stream", "2QbIu:"),
        (0x0090, "get_char_stream", "2Qb:Is"),
        (0x0091, "get_line_stream", "3Qb<+#Cn:Iu"),
        (0x0092, "get_buffer_stream", "3Qb<+#Cn:Iu"),
        (0x00A0, "char_to_lower", "2Cu:Cu"),
        (0x00A1, "char_to_upper", "2Cu:Cu"),
        (0x00B0, "stylehint_set", "4IuIuIuIs:"),
        (0x00B1, "stylehint_clear", "3IuIuIu:"),
        (0x00B2, "style_distinguish", "4QaIuIu:Iu"),
        (0x00B3, "style_measure", "5QaIuIu<Iu:Iu"),
        (0x00C0, "select", "1<+[4IuQaIuIu]:"),
        (0x00C1, "select_poll", "1<+[4IuQaIuIu]:"),
        (0x00D0, "request_line_event", "3Qa&+#!CnIu:"),
        (0x00D1, "cancel_line_event", "2Qa<[4IuQaIuIu]:"),
        (0x00D2, "request_char_event", "1Qa:"),
        (0x00D3, "cancel_char_event", "1Qa:"),
        (0x00D4, "request_mouse_event", "1Qa:"),
        (0x00D5, "cancel_mouse_event", "1Qa:"),
        (0x00D6, "request_timer_events", "1Iu:"),
        (0x00E0, "image_get_info", "4Iu<Iu<Iu:Iu"),
        (0x00E1, "image_draw", "5QaIuIsIs:Iu"),
        (0x00E2, "image_draw_scaled", "7QaIuIsIsIuIu:Iu"),
        (0x00E8, "window_flow_break", "1Qa:"),
        (0x00E9, "window_erase_rect", "5QaIsIsIuIu:"),
        (0x00EA, "window_fill_rect", "6QaIuIsIsIuIu:"),
        (0x00EB, "window_set_background_color", "2QaIu:"),
        (0x00EC, "image_draw_scaled_ext", "9QaIuIsIsIuIuIuIu:Iu"),
        (0x00F0, "schannel_iterate", "3Qd<Iu:Qd"),
        (0x00F1, "schannel_get_rock", "2Qd:Iu"),
        (0x00F2, "schannel_create", "2Iu:Qd"),
        (0x00F3, "schannel_destroy", "1Qd:"),
        (0x00F4, "schannel_create_ext", "3IuIu:Qd"),
        (0x00F7, "schannel_play_multi", "4>+#Qd>+#IuIu:Iu"),
        (0x00F8, "schannel_play", "3QdIu:Iu"),
        (0x00F9, "schannel_play_ext", "5QdIuIuIu:Iu"),
        (0x00FA, "schannel_stop", "1Qd:"),
        (0x00FB, "schannel_set_volume", "2QdIu:"),
        (0x00FC, "sound_load_hint", "2IuIu:"),
        (0x00FD, "schannel_set_volume_ext", "4QdIuIuIu:"),
        (0x00FE, "schannel_pause", "1Qd:"),
        (0x00FF, "schannel_unpause", "1Qd:"),
        (0x0100, "set_hyperlink", "1Iu:"),
        (0x0101, "set_hyperlink_stream", "2QbIu:"),
        (0x0102, "request_hyperlink_event", "1Qa:"),
        (0x0103, "cancel_hyperlink_event", "1Qa:"),
        (0x0120, "buffer_to_lower_case_uni", "3&+#IuIu:Iu"),
        (0x0121, "buffer_to_upper_case_uni", "3&+#IuIu:Iu"),
        (0x0122, "buffer_to_title_case_uni", "4&+#IuIuIu:Iu"),
        (0x0123, "buffer_canon_decompose_uni", "3&+#IuIu:Iu"),
        (0x0124, "buffer_canon_normalize_uni", "3&+#IuIu:Iu"),
        (0x0128, "put_char_uni", "1Iu:"),
        (0x0129, "put_string_uni", "1U:"),
        (0x012A, "put_buffer_uni", "1>+#Iu:"),
        (0x012B, "put_char_stream_uni", "2QbIu:"),
        (0x012C, "put_string_stream_uni", "2QbU:"),
        (0x012D, "put_buffer_stream_uni", "2Qb>+#Iu:"),
        (0x0130, "get_char_stream_uni", "2Qb:Is"),
        (0x0131, "get_buffer_stream_uni", "3Qb<+#Iu:Iu"),
        (0x0132, "get_line_stream_uni", "3Qb<+#Iu:Iu"),
        (0x0138, "stream_open_file_uni", "4QcIuIu:Qb"),
        (0x0139, "stream_open_memory_uni", "4&#!IuIuIu:Qb"),
        (0x013A, "stream_open_resource_uni", "3IuIu:Qb"),
        (0x0140, "request_char_event_uni", "1Qa:"),
        (0x0141, "request_line_event_uni", "3Qa&+#!IuIu:"),
        (0x0150, "set_echo_line_event", "2QaIu:"),
        (0x0151, "set_terminators_line_event", "2Qa>#Iu:"),
        (0x0160, "current_time", "1<+[3IsIuIs]:"),
        (0x0161, "current_simple_time", "2Iu:Is"),
        (
            0x0168,
            "time_to_date_utc",
            "2>+[3IsIuIs]<+[8IsIsIsIsIsIsIsIs]:",
        ),
        (
            0x0169,
            "time_to_date_local",
            "2>+[3IsIuIs]<+[8IsIsIsIsIsIsIsIs]:",
        ),
        (
            0x016A,
            "simple_time_to_date_utc",
            "3IsIu<+[8IsIsIsIsIsIsIsIs]:",
        ),
        (
            0x016B,
            "simple_time_to_date_local",
            "3IsIu<+[8IsIsIsIsIsIsIsIs]:",
        ),
        (
            0x016C,
            "date_to_time_utc",
            "2>+[8IsIsIsIsIsIsIsIs]<+[3IsIuIs]:",
        ),
        (
            0x016D,
            "date_to_time_local",
            "2>+[8IsIsIsIsIsIsIsIs]<+[3IsIuIs]:",
        ),
        (
            0x016E,
            "date_to_simple_time_utc",
            "3>+[8IsIsIsIsIsIsIsIs]Iu:Is",
        ),
        (
            0x016F,
            "date_to_simple_time_local",
            "3>+[8IsIsIsIsIsIsIsIs]Iu:Is",
        ),
    ];

    // The whole point of generating prototype strings instead of
    // transcribing them: every declared signature must render to
    // exactly the string gi_dispa.c hand-writes -- same selectors,
    // same names, same prototypes, nothing missing and nothing
    // extra. A transcription error anywhere in the table fails
    // here by name.
    #[test]
    fn every_signature_renders_gi_dispa_exactly() {
        assert_eq!(SIGNATURES.len(), GI_DISPA.len());

        for (number, name, prototype) in GI_DISPA {
            let signature =
                lookup(*number).unwrap_or_else(|| panic!("selector {number:#06x} is missing"));

            assert_eq!(signature.name, *name, "{number:#06x}");
            assert_eq!(signature.prototype(), *prototype, "{name}");
        }
    }

    // The table stays sorted by selector, or the binary-search
    // lookup would miss entries silently.
    #[test]
    fn the_table_is_sorted_by_selector() {
        assert!(
            SIGNATURES
                .windows(2)
                .all(|pair| pair[0].number < pair[1].number)
        );
    }

    // Lookup answers by selector; an unknown number -- and 0x0002,
    // which gi_dispa.c declares uninvokable -- answers None.
    #[test]
    fn lookup_finds_declared_selectors_only() {
        let opened = lookup(0x0023).unwrap();

        assert_eq!(opened.glk_name(), "glk_window_open");
        assert!(lookup(0x0002).is_none());
        assert!(lookup(0xFFFF).is_none());
    }

    // The properties the bridge era will marshal by: arrays
    // consume an address and a count where everything else is one
    // word, char arrays are bytes where word arrays are four, the
    // opaque codes name their classes, and the reference
    // directions read off the prefix.
    #[test]
    fn items_answer_what_marshalling_asks() {
        assert_eq!(U32.word_count(), 1);
        assert!(out(U32).is_reference());
        assert!(!U32.is_reference());

        assert_eq!(CHAR.element_size(), 1);
        assert_eq!(U32.element_size(), 4);

        assert!(I32.signed());
        assert!(!U32.signed());

        assert!(CSTRING.is_string());
        assert!(USTRING.is_string());
        assert!(!U32.is_string());

        assert_eq!(WINDOW.opaque_class(), Some(CLASS_WINDOW));
        assert_eq!(STREAM.opaque_class(), Some(CLASS_STREAM));
        assert_eq!(FILEREF.opaque_class(), Some(CLASS_FILEREF));
        assert_eq!(SCHANNEL.opaque_class(), Some(CLASS_SCHANNEL));
        assert!(!U32.is_opaque());
        assert_eq!(U32.opaque_class(), None);

        assert!(!out(U32).passes_in());
        assert!(out(U32).passes_out());
        assert!(into(U32).passes_in());
        assert!(!into(U32).passes_out());
        assert!(inout(U32).passes_in());
        assert!(inout(U32).passes_out());
        assert!(!into_nonnull(U32).passes_out());
        assert!(!out_nonnull(U32).passes_in());

        assert!(EVENT.is_struct());
        assert_eq!(EVENT.fields.len(), 4);
    }

    // A signature's word count is what the glk opcode must supply:
    // request_line_event is a window, an array's address and
    // count, and an initial length -- four words for three
    // arguments.
    #[test]
    fn word_counts_include_array_lengths() {
        assert_eq!(lookup(0x00D0).unwrap().word_count(), 4);
        assert_eq!(lookup(0x0023).unwrap().word_count(), 5);
    }
}
