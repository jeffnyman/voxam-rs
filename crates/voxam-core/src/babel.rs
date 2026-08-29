//! The Treaty of Babel: a story's identity and its bibliography.
//!
//! The treaty gives every work of interactive fiction an IFID,
//! "analogous to the ISBN code assigned to every published book"
//! (Babel: The IFID unique identifier), and lays down per-format
//! rules for computing one where none is embedded. This module
//! carries the rules for the formats Voxam plays -- modern files
//! brand a UUID://...// string into byte-accessible memory and the
//! brand wins wherever it is found; legacy files earn their IFIDs
//! from their header numbers instead, human-readable identities
//! like ZCODE-88-840726 -- and the iFiction record the wire's
//! doorway courtesy reads from the Blorb's IFmd chunk (Babel: The
//! iFiction format).
//!
//! The XML walking is done by hand, and deliberately small:
//! elements matched by local name alone, since the treaty
//! namespaces `<ifindex>` but records in the wild are not always
//! so careful, and bibliography is a courtesy that should survive
//! a missing xmlns. What cannot be read answers None -- never an
//! error, because a card is never a gate.

// Serial codes that never earn a checksum suffix: the test and
// user-modified forms the treaty names (Babel: The IFID for a
// legacy Z-code story file).
const UNTRUSTED_SERIALS: [&str; 3] = ["000000", "999999", "------"];

// The Z-code header's identifying words (§11.1): release, serial,
// checksum -- the treaty's three elements.
const Z_RELEASE: usize = 0x02;
const Z_SERIAL: std::ops::Range<usize> = 0x12..0x18;
const Z_CHECKSUM: usize = 0x1C;

// The Glulx header's identifying words (Glulx: The Header), plus
// the Inform-compiled fields past its end (Babel: The IFID for a
// legacy Glulx story file).
const GLULX_EXTENT: std::ops::Range<usize> = 12..16;
const GLULX_CHECKSUM: std::ops::Range<usize> = 32..36;
const GLULX_COMPILER: std::ops::Range<usize> = 36..40;
const GLULX_RELEASE: usize = 52;
const GLULX_SERIAL: std::ops::Range<usize> = 54..60;
const INFORM: &[u8] = b"Info";

/// A story too short to hold the identifying header words can hold
/// no identity either.
const HEADER_EXTENT: usize = 0x40;

/// The Z-Machine's eight story file versions (§11.1): a plausible
/// version byte is what marks loose bytes as Z-code.
const LAST_Z_VERSION: u8 = 8;

/// The IFID for a story file's bytes; None for neither format.
///
/// A Glulx file answers by its magic word, an Å-machine form by
/// its own, anything else with a plausible version byte as Z-code.
/// The caller unwraps blorbs first: a blorbed story's IFID is its
/// packaged story's, until an iFiction record says otherwise
/// (Babel: The IFID for a blorbed story file).
pub fn ifid(data: &[u8]) -> Option<String> {
    if data.len() < HEADER_EXTENT {
        return None;
    }

    if &data[..4] == b"Glul" {
        return Some(glulx_ifid(data));
    }

    if &data[..4] == b"FORM" && &data[8..12] == b"AAVM" {
        return aamachine_ifid(data);
    }

    if (1..=LAST_Z_VERSION).contains(&data[0]) {
        return Some(zcode_ifid(data));
    }

    None
}

/// The IFID an Å-machine story carries in its HEAD, or None.
///
/// The embedded UUID is the treaty answer for a Dialog story --
/// the compiler stamps one at build time -- and a story without
/// the optional field, or one this reader cannot parse, answers
/// nothing rather than an invented hash (Aa-machine: HEAD).
pub fn aamachine_ifid(data: &[u8]) -> Option<String> {
    crate::aamachine::story::Story::new(data)
        .ok()
        .and_then(|story| story.ifid)
}

/// A Z-code story's IFID from its brand or its header.
///
/// The serial gates the brand scan: a file whose serial dates it
/// before 2006 -- the 1980s, the 1990s, 2000 through 2005 --
/// cannot carry the UUID brand, so "searching for this is
/// unnecessary" and only the rest are scanned (Babel: The IFID for
/// a legacy Z-code story file).
pub fn zcode_ifid(data: &[u8]) -> String {
    let serial = cleaned(&data[Z_SERIAL]);
    let dated =
        serial.starts_with('8') || serial.starts_with('9') || ("00"..="05").contains(&&serial[..2]);

    if !dated && let Some(brand) = branded_scan(data) {
        return brand;
    }

    let release = u16::from_be_bytes([data[Z_RELEASE], data[Z_RELEASE + 1]]);
    let head = format!("ZCODE-{release}-{serial}");
    let leading = serial.as_bytes()[0];

    if b"012345679".contains(&leading) && !UNTRUSTED_SERIALS.contains(&serial.as_str()) {
        // The post-1990 form: Inform-era serials carry the
        // checksum as four hexadecimal digits, while Infocom's 8x
        // serials -- and the untrusted forms -- stay bare (Babel:
        // The IFID for a legacy Z-code story file).
        let checksum = u16::from_be_bytes([data[Z_CHECKSUM], data[Z_CHECKSUM + 1]]);

        return format!("{head}-{checksum:04X}");
    }

    head
}

/// A Glulx story's IFID from its brand or its header.
///
/// An Inform-compiled file identifies like Z-code -- release,
/// serial, checksum -- and announces itself with "Info" past the
/// header proper; a file from any other tool has only its
/// checksum, supplemented by the stated size of the initial memory
/// map (Babel: The IFID for a legacy Glulx story file).
pub fn glulx_ifid(data: &[u8]) -> String {
    if let Some(brand) = branded_scan(data) {
        return brand;
    }

    let checksum = u32::from_be_bytes(data[GLULX_CHECKSUM].try_into().expect("four bytes"));

    if &data[GLULX_COMPILER] == INFORM {
        let release = u16::from_be_bytes([data[GLULX_RELEASE], data[GLULX_RELEASE + 1]]);
        let serial = cleaned(&data[GLULX_SERIAL]);

        return format!("GLULX-{release}-{serial}-{checksum:08X}");
    }

    let extent = u32::from_be_bytes(data[GLULX_EXTENT].try_into().expect("four bytes"));

    format!("GLULX-{extent:08X}-{checksum:08X}")
}

/// The embedded UUID://...// brand, uppercased, or None.
///
/// "Its location cannot be guaranteed, so the whole of
/// byte-accessible memory must be scanned" (Babel: Game formats
/// that embed an IFID) -- and the file is the practical superset
/// of byte-accessible memory. The treaty spells an IFID with
/// digits, capitals, and hyphens, but Alan writes lowercase
/// hexadecimal, "converted to upper case when reading" -- so the
/// scan accepts both cases and the answer wears capitals.
fn branded_scan(data: &[u8]) -> Option<String> {
    const OPENING: &[u8] = b"UUID://";

    let mut from = 0;

    while let Some(at) = find(&data[from..], OPENING).map(|found| from + found) {
        let start = at + OPENING.len();
        let end = data[start..]
            .iter()
            .position(|&byte| !(byte.is_ascii_alphanumeric() || byte == b'-'))
            .map_or(data.len(), |run| start + run);

        if end > start && data[end..].starts_with(b"//") {
            let told: String = data[start..end]
                .iter()
                .map(|&byte| (byte as char).to_ascii_uppercase())
                .collect();

            return Some(told);
        }

        from = start;
    }

    None
}

/// The first offset a needle appears at, or None.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Serial bytes as text, non-alphanumerics turned to hyphens.
///
/// Only ASCII alphanumerics survive: "converting any
/// non-alphanumeric characters (in particular, nulls) to hyphens"
/// (Babel: The IFID for a legacy Z-code story file).
fn cleaned(serial: &[u8]) -> String {
    serial
        .iter()
        .map(|&byte| {
            if byte.is_ascii_alphanumeric() {
                byte as char
            } else {
                '-'
            }
        })
        .collect()
}

/// The bibliographic heart of an iFiction record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IFiction {
    /// The record's primary IFID -- the first listed, which the
    /// treaty puts foremost when a work carries several (Babel:
    /// The iFiction format).
    pub ifid: Option<String>,
    /// The work's title, or None unrecorded.
    pub title: Option<String>,
    /// The author, or None unrecorded.
    pub author: Option<String>,
    /// The subtitle-like headline, or None unrecorded.
    pub headline: Option<String>,
    /// The work's blurb, or None unrecorded -- its `<br/>` line
    /// breaks carried as newlines, since the treaty spells
    /// paragraph breaks with them (Babel: The iFiction format).
    pub description: Option<String>,
}

/// The first story record in iFiction XML; None for unreadable.
///
/// Records the treaty itself warns about -- the pre-1.0 versions
/// still circulating -- answer whatever of the record they can
/// (Babel: The iFiction format).
pub fn ifiction(xml: &[u8]) -> Option<IFiction> {
    let root = parse(xml)?;
    let story = child(&root, "story")?;
    let identification = child(story, "identification");
    let bibliographic = child(story, "bibliographic");

    Some(IFiction {
        ifid: field(identification, "ifid"),
        title: field(bibliographic, "title"),
        author: field(bibliographic, "author"),
        headline: field(bibliographic, "headline"),
        description: broken_field(bibliographic, "description"),
    })
}

/// One parsed element: its qualified name, the text before its
/// first child, its children in order, and the text that follows
/// it inside its parent.
struct Element {
    name: String,
    text: String,
    tail: String,
    children: Vec<Element>,
}

/// The first child whose local name matches, namespace-blind.
fn child<'held>(element: &'held Element, name: &str) -> Option<&'held Element> {
    element
        .children
        .iter()
        .find(|held| local(&held.name) == name)
}

/// A name's local part: what follows any namespace prefix.
fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// A section's first named child's text, stripped, or None.
fn field(section: Option<&Element>, name: &str) -> Option<String> {
    let found = child(section?, name)?;
    let held = found.text.trim();

    if held.is_empty() {
        None
    } else {
        Some(held.to_string())
    }
}

/// A field whose `<br/>` children mark line breaks, walked whole.
///
/// A description is mixed content: taking the leading text alone
/// would silently drop everything after the first break, so the
/// walk keeps every piece with a newline at each `<br/>` (Babel:
/// The iFiction format).
fn broken_field(section: Option<&Element>, name: &str) -> Option<String> {
    let found = child(section?, name)?;
    let mut pieces = vec![found.text.clone()];

    for held in &found.children {
        if local(&held.name) == "br" {
            pieces.push("\n".to_string());
        }

        pieces.push(held.text.clone());
        pieces.push(held.tail.clone());
    }

    let joined = pieces.concat();
    let lines: Vec<&str> = joined.split('\n').map(str::trim).collect();
    let held = lines.join("\n");
    let held = held.trim();

    if held.is_empty() {
        None
    } else {
        Some(held.to_string())
    }
}

// -- the walker itself ------------------------------------------------------

/// Parse the document's root element, or None for the unreadable.
fn parse(xml: &[u8]) -> Option<Element> {
    let text = std::str::from_utf8(xml).ok()?;
    let mut position = 0;

    skip_prolog(text, &mut position)?;
    parse_element(text, &mut position)
}

/// Step over the prolog: whitespace, the XML declaration,
/// comments, and any document type declaration.
fn skip_prolog(text: &str, position: &mut usize) -> Option<()> {
    loop {
        let rest = &text[*position..];
        let trimmed = rest.trim_start();

        *position += rest.len() - trimmed.len();

        if let Some(after) = trimmed.strip_prefix("<?") {
            let end = after.find("?>")?;

            *position += 2 + end + 2;
        } else if trimmed.starts_with("<!--") {
            skip_comment(text, position)?;
        } else if trimmed.starts_with("<!") {
            skip_doctype(text, position)?;
        } else {
            return Some(());
        }
    }
}

/// Step over one comment, position at its `<!--`.
fn skip_comment(text: &str, position: &mut usize) -> Option<()> {
    let end = text[*position + 4..].find("-->")?;

    *position += 4 + end + 3;

    Some(())
}

/// Step over a `<!DOCTYPE ...>`, internal subset included.
fn skip_doctype(text: &str, position: &mut usize) -> Option<()> {
    let mut in_subset = false;

    for (at, held) in text[*position..].char_indices() {
        match held {
            '[' => in_subset = true,
            ']' => in_subset = false,
            '>' if !in_subset => {
                *position += at + 1;

                return Some(());
            }
            _ => {}
        }
    }

    None
}

/// Parse one element at the position, which must be its `<`.
fn parse_element(text: &str, position: &mut usize) -> Option<Element> {
    let rest = &text[*position..];

    if !rest.starts_with('<') {
        return None;
    }

    let after = &rest[1..];
    let name_len = after.find(|held: char| held.is_whitespace() || held == '>' || held == '/')?;
    let name = after[..name_len].to_string();

    if name.is_empty() {
        return None;
    }

    // Step over the attributes, quote-aware, to the tag's close.
    let mut cursor = 1 + name_len;
    let mut quote: Option<char> = None;
    let self_closing;

    loop {
        let held = rest[cursor..].chars().next()?;

        match (quote, held) {
            (Some(open), _) if held == open => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(held),
            (None, '>') => {
                self_closing = rest[..cursor].ends_with('/');
                cursor += 1;

                break;
            }
            (None, _) => {}
        }

        cursor += held.len_utf8();
    }

    *position += cursor;

    let mut element = Element {
        name,
        text: String::new(),
        tail: String::new(),
        children: Vec::new(),
    };

    if self_closing {
        return Some(element);
    }

    parse_content(text, position, &mut element)?;

    Some(element)
}

/// Parse an element's content up to and over its closing tag,
/// filling its text, children, and their tails.
fn parse_content(text: &str, position: &mut usize, element: &mut Element) -> Option<()> {
    loop {
        let rest = &text[*position..];
        let stretch = rest.find('<')?;
        let run = decoded(&rest[..stretch])?;

        into_text(element, run);

        *position += stretch;

        let rest = &text[*position..];

        if let Some(after) = rest.strip_prefix("</") {
            let end = after.find('>')?;

            if after[..end].trim() != element.name {
                return None;
            }

            *position += 2 + end + 1;

            return Some(());
        }

        if rest.starts_with("<!--") {
            skip_comment(text, position)?;

            continue;
        }

        if let Some(after) = rest.strip_prefix("<![CDATA[") {
            let end = after.find("]]>")?;

            into_text(element, after[..end].to_string());

            *position += 9 + end + 3;

            continue;
        }

        let held = parse_element(text, position)?;

        element.children.push(held);
    }
}

/// Append a run of character data where it belongs: the last
/// child's tail, or the element's own leading text.
fn into_text(element: &mut Element, run: String) {
    match element.children.last_mut() {
        Some(child) => child.tail.push_str(&run),
        None => element.text.push_str(&run),
    }
}

/// Decode the five predefined entities and character references;
/// an entity the language does not define is unreadable XML.
fn decoded(run: &str) -> Option<String> {
    if !run.contains('&') {
        return Some(run.to_string());
    }

    let mut held = String::with_capacity(run.len());
    let mut rest = run;

    while let Some(at) = rest.find('&') {
        held.push_str(&rest[..at]);

        let after = &rest[at + 1..];
        let end = after.find(';')?;
        let name = &after[..end];

        match name {
            "amp" => held.push('&'),
            "lt" => held.push('<'),
            "gt" => held.push('>'),
            "quot" => held.push('"'),
            "apos" => held.push('\''),
            _ => {
                let code = if let Some(hex) = name.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()?
                } else {
                    name.strip_prefix('#')?.parse().ok()?
                };

                held.push(char::from_u32(code)?);
            }
        }

        rest = &after[end + 1..];
    }

    held.push_str(rest);

    Some(held)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IFICTION: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<ifindex version="1.0" xmlns="http://babel.ifarchive.org/protocol/iFiction/">
 <story>
  <identification>
   <ifid>1974A053-7DB0-4103-93A1-767C1382C0B7</ifid>
   <ifid>ZCODE-8-040205-6630</ifid>
   <format>zcode</format>
  </identification>
  <bibliographic>
   <title>Savoir-Faire</title>
   <author>Emily Short</author>
   <headline>An Interactive Vivification</headline>
  </bibliographic>
 </story>
</ifindex>"#;

    // An iFiction record answers its first IFID -- the treaty puts
    // the newest foremost when a work carries several -- and its
    // bibliography whole. Local names alone are matched, so a
    // record missing the treaty's namespace answers all the same.
    #[test]
    fn ifiction_records_read_whole() {
        let record = ifiction(IFICTION).expect("the record parses");

        assert_eq!(
            record.ifid.as_deref(),
            Some("1974A053-7DB0-4103-93A1-767C1382C0B7")
        );
        assert_eq!(record.title.as_deref(), Some("Savoir-Faire"));
        assert_eq!(record.author.as_deref(), Some("Emily Short"));
        assert_eq!(
            record.headline.as_deref(),
            Some("An Interactive Vivification")
        );

        let bare = ifiction(
            b"<ifindex><story><identification><ifid>DUMMY-1</ifid>\
              </identification></story></ifindex>",
        )
        .expect("the bare record parses");

        assert_eq!(bare.ifid.as_deref(), Some("DUMMY-1"));
        assert!(bare.title.is_none());
        assert!(bare.description.is_none());
    }

    // A description is mixed content: its <br/> children mark line
    // breaks, so the walk keeps every piece -- text alone would
    // drop everything after the first break -- while a stray
    // non-br child's words survive too, and a blank description is
    // no description.
    #[test]
    fn descriptions_keep_their_breaks() {
        let record = ifiction(
            b"<ifindex><story><bibliographic>\
              <description>One paragraph. <br/> Another one, \
              <em>emphasized</em> even.</description>\
              </bibliographic></story></ifindex>",
        )
        .expect("the record parses");

        assert_eq!(
            record.description.as_deref(),
            Some("One paragraph.\nAnother one, emphasized even.")
        );

        let blank = ifiction(
            b"<ifindex><story><bibliographic><description>  </description>\
              </bibliographic></story></ifindex>",
        )
        .expect("the blank record parses");

        assert!(blank.description.is_none());
    }

    // What cannot be read answers None -- broken XML, an index
    // with no story record -- and absent fields stay None,
    // whitespace-only text included.
    #[test]
    fn unreadable_records_answer_none() {
        assert!(ifiction(b"<not xml").is_none());
        assert!(ifiction(b"<ifindex></ifindex>").is_none());

        let blank = ifiction(
            b"<ifindex><story><bibliographic><title>  </title>\
              </bibliographic></story></ifindex>",
        )
        .expect("the blank record parses");

        assert!(blank.title.is_none());
        assert!(blank.ifid.is_none());
    }

    fn z_header(release: u16, serial: &[u8; 6], checksum: u16, version: u8) -> Vec<u8> {
        let mut data = vec![0u8; 0x40];

        data[0] = version;
        data[0x02..0x04].copy_from_slice(&release.to_be_bytes());
        data[0x12..0x18].copy_from_slice(serial);
        data[0x1C..0x1E].copy_from_slice(&checksum.to_be_bytes());

        data
    }

    fn glulx_image(
        checksum: u32,
        compiler: &[u8; 4],
        release: u16,
        serial: &[u8; 6],
        extent: u32,
        tail: &[u8],
    ) -> Vec<u8> {
        let mut data = vec![0u8; 0x40];

        data[0..4].copy_from_slice(b"Glul");
        data[12..16].copy_from_slice(&extent.to_be_bytes());
        data[32..36].copy_from_slice(&checksum.to_be_bytes());
        data[36..40].copy_from_slice(compiler);
        data[52..54].copy_from_slice(&release.to_be_bytes());
        data[54..60].copy_from_slice(serial);
        data.extend_from_slice(tail);

        data
    }

    // The treaty's own worked example: Savoir-Faire release 8,
    // serial 040205, checksum 0x6630 -- an Inform-era serial, so
    // the checksum rides as four hexadecimal digits (Babel: The
    // IFID for a legacy Z-code story file).
    #[test]
    fn the_treatys_worked_example() {
        assert_eq!(
            zcode_ifid(&z_header(8, b"040205", 0x6630, 8)),
            "ZCODE-8-040205-6630"
        );
    }

    // Infocom-era serials stay bare: an 8x date earns no checksum
    // suffix, and no brand scan either.
    #[test]
    fn infocom_identities_stay_bare() {
        assert_eq!(
            zcode_ifid(&z_header(88, b"840726", 0x1234, 3)),
            "ZCODE-88-840726"
        );
        assert_eq!(zcode_ifid(&z_header(2, b"AS000C", 0, 1)), "ZCODE-2-AS000C");
    }

    // The untrusted serials never earn a checksum, and serial
    // bytes outside the alphanumerics turn to hyphens -- an
    // all-null serial reads as six of them.
    #[test]
    fn serials_clean_and_untrusted_forms_stay_bare() {
        assert_eq!(
            zcode_ifid(&z_header(5, &[0; 6], 0xF00D, 1)),
            "ZCODE-5-------"
        );
        assert_eq!(
            zcode_ifid(&z_header(15, b"999999", 0xF00D, 5)),
            "ZCODE-15-999999"
        );
        assert_eq!(
            zcode_ifid(&z_header(
                1,
                &[b'9', 0xB5, b'0', b'1', b'0', b'1'],
                0xBEEF,
                5
            )),
            "ZCODE-1-9-0101-BEEF"
        );
    }

    // The brand wins where it may exist: a post-2005 serial scans
    // for the UUID and a lowercase brand reads uppercased; a dated
    // serial never scans at all.
    #[test]
    fn the_brand_wins_where_it_may_exist() {
        let mut branded = z_header(3, b"070101", 0xAAAA, 8);

        branded.extend_from_slice(b"junk UUID://abc-DEF-123// tail");

        assert_eq!(zcode_ifid(&branded), "ABC-DEF-123");

        let mut dated = z_header(3, b"840726", 0xAAAA, 8);

        dated.extend_from_slice(b"junk UUID://abc-DEF-123// tail");

        assert_eq!(zcode_ifid(&dated), "ZCODE-3-840726");
    }

    // Glulx identities: Inform-compiled files identify like
    // Z-code, any other tool's by extent and checksum, and a
    // brand wins over both.
    #[test]
    fn glulx_identities() {
        assert_eq!(
            glulx_ifid(&glulx_image(7, b"Info", 12, b"040205", 0x100, b"")),
            "GLULX-12-040205-00000007"
        );
        assert_eq!(
            glulx_ifid(&glulx_image(7, &[0; 4], 0, &[0; 6], 0x100, b"")),
            "GLULX-00000100-00000007"
        );
        assert_eq!(
            glulx_ifid(&glulx_image(
                7,
                &[0; 4],
                0,
                &[0; 6],
                0x100,
                b"UUID://1974a053-7db0//"
            )),
            "1974A053-7DB0"
        );
    }

    // The front door routes by what the bytes claim to be: the
    // Glulx magic word, a plausible Z-code version byte, or
    // nothing at all -- and a fragment too short for a header has
    // no identity either.
    #[test]
    fn ifid_routes_by_format() {
        assert_eq!(
            ifid(&glulx_image(7, &[0; 4], 0, &[0; 6], 0x100, b"")).as_deref(),
            Some("GLULX-00000100-00000007")
        );
        assert_eq!(
            ifid(&z_header(8, b"040205", 0x6630, 8)).as_deref(),
            Some("ZCODE-8-040205-6630")
        );
        assert_eq!(ifid(&[0u8; 64]), None);

        let mut exe = b"MZ".to_vec();

        exe.extend(vec![0u8; 62]);

        assert_eq!(ifid(&exe), None);
        assert_eq!(ifid(&[0x05]), None);
    }

    // The walker's own corners: entities decode (undefined ones
    // are unreadable), CDATA rides raw, comments vanish into the
    // surrounding text, and a namespace-prefixed record still
    // answers by local name.
    #[test]
    fn the_walker_reads_the_languages_corners() {
        let entitied = ifiction(
            b"<ifindex><story><bibliographic>\
              <title>Trinity &amp; Beyond&#33;</title>\
              </bibliographic></story></ifindex>",
        )
        .expect("the entities decode");

        assert_eq!(entitied.title.as_deref(), Some("Trinity & Beyond!"));
        assert!(
            ifiction(
                b"<ifindex><story><bibliographic><title>&nope;</title>\
                  </bibliographic></story></ifindex>",
            )
            .is_none()
        );

        let prefixed = ifiction(
            b"<b:ifindex xmlns:b=\"urn:x\"><b:story><b:bibliographic>\
              <b:title><![CDATA[A <Tale>]]></b:title>\
              <!-- a comment --><b:author>Someone</b:author>\
              </b:bibliographic></b:story></b:ifindex>",
        )
        .expect("the prefixed record parses");

        assert_eq!(prefixed.title.as_deref(), Some("A <Tale>"));
        assert_eq!(prefixed.author.as_deref(), Some("Someone"));
    }
}
