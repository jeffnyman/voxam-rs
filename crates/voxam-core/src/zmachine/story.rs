//! Loading and validation of Z-Machine story files.

use crate::errors::VoxamError;
use crate::zmachine::header::{HEADER_SIZE, Header};

/// The byte at address 0 holds the version number, 1 to 8 (§11.1).
const VERSION_RANGE: std::ops::RangeInclusive<u8> = 1..=8;

/// A story file held in memory, validated enough to identify it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Story {
    data: Vec<u8>,
}

impl Story {
    /// Accept byte content, rejecting what cannot be a story file.
    pub fn new(data: Vec<u8>) -> Result<Self, VoxamError> {
        if data.len() < HEADER_SIZE {
            return Err(VoxamError::ZMachineStory(format!(
                "story file is {} bytes, but the header alone requires {} (§1.1.1.1)",
                data.len(),
                HEADER_SIZE
            )));
        }

        if !VERSION_RANGE.contains(&data[0]) {
            return Err(VoxamError::ZMachineStory(format!(
                "story file declares version {}, but only versions 1 to 8 exist (§11.1)",
                data[0]
            )));
        }

        Ok(Self { data })
    }

    /// The raw bytes of the story file.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Typed access to this story's header fields (§11.1).
    pub fn header(&self) -> Header<'_> {
        Header::over(&self.data)
    }

    /// The Z-Machine version this story targets (§11.1).
    pub fn version(&self) -> u8 {
        self.data[0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zmachine::testing::story_bytes;

    #[test]
    fn rejects_content_too_short_for_header() {
        let error = Story::new(vec![3; 63]).unwrap_err();

        assert!(matches!(error, VoxamError::ZMachineStory(_)));
        assert!(error.to_string().contains("§1.1.1.1"));
    }

    #[test]
    fn rejects_empty_content() {
        assert!(Story::new(Vec::new()).is_err());
    }

    #[test]
    fn accepts_every_valid_version() {
        for version in 1..=8 {
            let story = Story::new(story_bytes(version, 64, 64, 64)).unwrap();

            assert_eq!(story.version(), version);
        }
    }

    #[test]
    fn rejects_versions_that_do_not_exist() {
        for version in [0, 9, 255] {
            let error = Story::new(story_bytes(version, 64, 64, 64)).unwrap_err();

            assert!(error.to_string().contains("§11.1"));
        }
    }
}
