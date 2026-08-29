//! The Treaty of Babel's bibliography: the iFiction record.
//!
//! This rung carries what the wire's doorway courtesy needs -- the
//! record's bibliographic heart, read from the Blorb's IFmd chunk
//! (Babel: The iFiction format). The story identities (IFIDs
//! computed from the bytes themselves) arrive with the Babel
//! milestone.
//!
//! The XML walking is done by hand, and deliberately small:
//! elements matched by local name alone, since the treaty
//! namespaces `<ifindex>` but records in the wild are not always
//! so careful, and bibliography is a courtesy that should survive
//! a missing xmlns. What cannot be read answers None -- never an
//! error, because a card is never a gate.

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
