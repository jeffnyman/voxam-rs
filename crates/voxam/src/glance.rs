//! A read-only tour of a story file's header (§11.1).
//!
//! The header is the story's own manifest: what machine it wants,
//! where its tables live, and which courtesies it hopes the
//! interpreter can offer. This report reads the pristine file --
//! before the interpreter stamps in any capabilities of its own --
//! so every line shows what the compiler shipped, hexadecimal
//! where the value is an address, with the Standard section that
//! defines it. The checksum is computed as §15's verify opcode
//! would and judged against the stored word.
//!
//! Ported line-for-line from the Python implementation's glance
//! module: the two reports must render identically, which is this
//! port's certification.

use voxam_core::zmachine::header::{
    COLOURS_REQUEST_BIT, GRAPHICS_BIT, MENUS_BIT, MOUSE_BIT, OFFSET_VERSIONS, PACKED_PC_VERSION,
    PICTURE_FLAGS_VERSION, SOUND_BIT, STATUS_FLAGS_VERSION, TANDY_BIT, UNDO_BIT,
};
use voxam_core::zmachine::story::Story;

/// Compose the header report for a loaded story.
pub fn report(story: &Story) -> String {
    let mut lines = identity(story);
    lines.push(String::new());
    lines.extend(memory_map(story));
    lines.push(String::new());
    lines.extend(requests(story));

    lines.join("\n")
}

/// One report line: a name, a value, and its meaning.
fn field(name: &str, value: &str, meaning: &str) -> String {
    format!("  {name:<18} {value:>12}   {meaning}")
}

/// A byte address in the Standard's own $hex dress.
fn address(value: u16) -> String {
    format!("${value:04x}")
}

/// The stanza that names the story and judges its checksum.
fn identity(story: &Story) -> Vec<String> {
    let header = story.header();
    let declared = header.declared_file_length();
    let mut lines = vec![
        "Identity".to_string(),
        field(
            "version",
            &header.version().to_string(),
            "the Z-Machine version (§11.1)",
        ),
        field(
            "release",
            &header.release().to_string(),
            "the story's release number (§11.1)",
        ),
        field(
            "serial",
            &header.serial_number(),
            "six characters, conventionally the compile date (§11.1)",
        ),
        field(
            "file length",
            &format!("{declared} bytes"),
            &format!(
                "declared, at the version's scale (§11.1.6); {} on disk",
                story.data().len()
            ),
        ),
    ];

    let stored = header.stored_checksum();
    let computed = header.computed_checksum();

    let verdict = if header.verify() {
        format!("${stored:04x} stored and computed agree (§15 verify)")
    } else if stored == 0 {
        format!(
            "stored $0000, computed ${computed:04x} -- some early Version 3 files \
             store none (§11.1)"
        )
    } else {
        format!("MISMATCH: stored ${stored:04x}, computed ${computed:04x} (§15 verify)")
    };

    lines.push(field("checksum", "", &verdict).trim_end().to_string());

    lines
}

/// The stanza of table addresses and region boundaries.
fn memory_map(story: &Story) -> Vec<String> {
    let header = story.header();
    let mut lines = vec!["Memory map".to_string()];

    if header.version() == PACKED_PC_VERSION {
        lines.push(field(
            "main routine",
            &address(header.main_routine_packed_address().expect("version 6")),
            "packed routine address; execution calls it (§5.4, §11.1)",
        ));
    } else {
        lines.push(field(
            "initial pc",
            &address(header.initial_program_counter().expect("not version 6")),
            "the first instruction's byte address (§5.5, §11.1)",
        ));
    }

    lines.extend([
        field(
            "static memory",
            &address(header.static_memory_base()),
            "writes stop here (§1.1.1, §1.1.2)",
        ),
        field(
            "high memory",
            &address(header.high_memory_base()),
            "routines and strings begin (§1.1.3)",
        ),
        field(
            "dictionary",
            &address(header.dictionary_address()),
            "the parser's word list (§13, §11.1)",
        ),
        field(
            "objects",
            &address(header.object_table_address()),
            "the object table (§12, §11.1)",
        ),
        field(
            "globals",
            &address(header.global_variables_address()),
            "240 global variables (§6.2, §11.1)",
        ),
        field(
            "abbreviations",
            &address(header.abbreviations_table_address()),
            "the abbreviations table (§3.3, §11.1)",
        ),
    ]);

    let alphabet = header.alphabet_table_address();

    lines.push(if alphabet != 0 {
        field(
            "alphabet table",
            &address(alphabet),
            "custom alphabets (§3.5.5)",
        )
    } else {
        field(
            "alphabet table",
            "standard",
            "the standard alphabets (§3.5)",
        )
    });

    let unicode_table = header.unicode_translation_address();

    lines.push(if unicode_table != 0 {
        field(
            "unicode table",
            &address(unicode_table),
            "custom translations (§3.8.5.2)",
        )
    } else {
        field("unicode table", "default", "the default table (§3.8.5.3)")
    });

    if OFFSET_VERSIONS.contains(&header.version()) {
        lines.extend([
            field(
                "routines offset",
                &address(header.routines_offset().expect("version 6 or 7")),
                "as stored, divided by 8 (§1.2.3)",
            ),
            field(
                "strings offset",
                &address(header.static_strings_offset().expect("version 6 or 7")),
                "as stored, divided by 8 (§1.2.3)",
            ),
        ]);
    }

    lines
}

/// The stanza of flags: what the game declares and asks for.
fn requests(story: &Story) -> Vec<String> {
    let header = story.header();
    let flags_1 = header.flags_1();
    let flags_2 = header.flags_2();
    let mut lines = vec![
        "Flags, as shipped".to_string(),
        field(
            "flags 1",
            &format!("${flags_1:02x}"),
            "the byte at $01 (§11.1)",
        ),
        field(
            "flags 2",
            &format!("${flags_2:04x}"),
            "the word at $10 (§11.1)",
        ),
    ];

    if header.version() <= STATUS_FLAGS_VERSION {
        let kind = if header.time_game().expect("version 3 or below") {
            "time of day"
        } else {
            "score and turns"
        };

        lines.push(field(
            "status line",
            kind,
            "what the top line shows (§8.2.3)",
        ));

        if flags_1 & TANDY_BIT != 0 {
            lines.push(field(
                "tandy bit",
                "set",
                "shipped minding its manners (§11.1.4 remarks)",
            ));
        }
    }

    let mut asks: Vec<&str> = Vec::new();

    if flags_2 & GRAPHICS_BIT != 0 {
        asks.push(if header.version() >= PICTURE_FLAGS_VERSION {
            "pictures (§11.1)"
        } else {
            "the §16 character graphics font (§8.1.5.1)"
        });
    }

    if flags_2 & UNDO_BIT != 0 {
        asks.push("undo (§11.1)");
    }

    if flags_2 & MOUSE_BIT != 0 {
        asks.push("a mouse (§11.1.2)");
    }

    if flags_2 & COLOURS_REQUEST_BIT != 0 {
        asks.push("colours (§8.3.3)");
    }

    if flags_2 & SOUND_BIT != 0 {
        asks.push("sound effects (§9, §11.1)");
    }

    if flags_2 & MENUS_BIT != 0 {
        asks.push("menus (§11.1.2)");
    }

    if asks.is_empty() {
        lines.push("  the game asks for no optional courtesies".to_string());
    } else {
        lines.push("  the game asks for:".to_string());
        lines.extend(asks.iter().map(|ask| format!("    - {ask}")));
    }

    lines
}
