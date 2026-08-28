//! Recognizing a story file by its own magic.
//!
//! Every format Voxam speaks announces itself in its first bytes:
//! a Z-code story opens with its version number (§11.1.1), a Glulx
//! story with `Glul`, and the IFF containers with `FORM` plus a
//! type -- `IFRS` for a Blorb resource file, `AAVM` for an
//! Å-machine story.

/// The story formats Voxam recognizes, each by its own magic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoryFormat {
    /// A bare Z-code story; the version byte is the header's first (§11.1.1).
    ZCode { version: u8 },
    /// A bare Glulx story, opening with `Glul`.
    Glulx,
    /// A Blorb resource file (`FORM` type `IFRS`), packaged story or sidecar.
    Blorb,
    /// An Å-machine story (`FORM` type `AAVM`).
    AaMachine,
}

/// Recognize a story by its opening bytes, or decline.
///
/// The IFF forms are checked before the Z-code version byte, though
/// they cannot collide: `F` is 0x46, well outside versions 1 to 8.
pub fn sniff(bytes: &[u8]) -> Option<StoryFormat> {
    if bytes.starts_with(b"Glul") {
        return Some(StoryFormat::Glulx);
    }

    // An IFF container is `FORM`, a length, then the form's type.
    if bytes.starts_with(b"FORM") {
        return match bytes.get(8..12) {
            Some(b"IFRS") => Some(StoryFormat::Blorb),
            Some(b"AAVM") => Some(StoryFormat::AaMachine),
            _ => None,
        };
    }

    match bytes.first() {
        Some(&version @ 1..=8) => Some(StoryFormat::ZCode { version }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zcode_versions_carry_their_byte() {
        for version in 1..=8 {
            let story = [version, 0, 0, 0];
            assert_eq!(sniff(&story), Some(StoryFormat::ZCode { version }));
        }
    }

    #[test]
    fn glulx_answers_to_its_magic() {
        assert_eq!(sniff(b"Glul\x00\x03\x01\x02"), Some(StoryFormat::Glulx));
    }

    #[test]
    fn blorb_is_a_form_of_type_ifrs() {
        assert_eq!(sniff(b"FORM\x00\x00\x00\x08IFRS"), Some(StoryFormat::Blorb));
    }

    #[test]
    fn aastory_is_a_form_of_type_aavm() {
        assert_eq!(
            sniff(b"FORM\x00\x00\x00\x08AAVM"),
            Some(StoryFormat::AaMachine)
        );
    }

    #[test]
    fn a_form_of_unknown_type_declines() {
        assert_eq!(sniff(b"FORM\x00\x00\x00\x08AIFF"), None);
    }

    #[test]
    fn out_of_range_version_bytes_decline() {
        assert_eq!(sniff(&[0, 0, 0, 0]), None);
        assert_eq!(sniff(&[9, 0, 0, 0]), None);
    }

    #[test]
    fn short_and_empty_files_decline() {
        assert_eq!(sniff(b""), None);
        assert_eq!(sniff(b"FORM\x00\x00\x00\x08"), None);
    }
}
