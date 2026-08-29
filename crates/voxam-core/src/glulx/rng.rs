//! The random number generator behind the random opcode.
//!
//! The stream is a xorshift32 owned by this module -- the same
//! generator, and the same reason, as the Z-Machine's dice: a seed
//! must produce the same session forever, because recorded
//! playthroughs must never be invalidated by an interpreter
//! upgrade, or by a port. Glulx asks less of its generator than
//! the Z-Machine does -- there is no rising-sequence testing mode
//! -- so this is the plain stream: full words, and ranges folded
//! from them (Glulx: The Random Number Generator).

const XORSHIFT_TRIPLE: (u32, u32, u32) = (13, 17, 5);
const MIX_INCREMENT: u32 = 0x9E37_79B9;
const MIX_MULTIPLIER_1: u32 = 0x85EB_CA6B;
const MIX_MULTIPLIER_2: u32 = 0xC2B2_AE35;

/// Spread a seed over the state space, never yielding zero.
fn mixed(value: u32) -> u32 {
    let mut value = value.wrapping_add(MIX_INCREMENT);
    value ^= value >> 16;
    value = value.wrapping_mul(MIX_MULTIPLIER_1);
    value ^= value >> 13;
    value = value.wrapping_mul(MIX_MULTIPLIER_2);
    value ^= value >> 16;

    if value == 0 { MIX_INCREMENT } else { value }
}

/// A fresh state from the operating system's entropy.
fn entropy() -> u32 {
    mixed(getrandom::u32().expect("operating system entropy"))
}

/// The stream the random and setrandom opcodes draw on.
///
/// The generator is deliberately not part of saved state, and a
/// restart leaves it alone (Glulx: The Random Number Generator,
/// Glulx: Game State).
pub struct Randomizer {
    state: u32,
}

impl Randomizer {
    /// Start seeded for a session, or from true entropy.
    pub fn new(seed: Option<u32>) -> Self {
        Self {
            state: match seed {
                Some(seed) => mixed(seed),
                None => entropy(),
            },
        }
    }

    /// The next full 32-bit value off the stream.
    pub fn word(&mut self) -> u32 {
        let mut state = self.state;
        state ^= state << XORSHIFT_TRIPLE.0;
        state ^= state >> XORSHIFT_TRIPLE.1;
        state ^= state << XORSHIFT_TRIPLE.2;
        self.state = state;

        state
    }

    /// A value in 0 through limit - 1, folded from the stream.
    ///
    /// Folding by modulo skews the distribution by well under one
    /// part in a million for any range a game's dice could ask.
    pub fn below(&mut self, limit: u32) -> u32 {
        self.word() % limit
    }

    /// Reseed the stream -- setrandom's work. A seed of zero asks
    /// for genuine unpredictability (Glulx: The Random Number
    /// Generator).
    pub fn seed(&mut self, value: u32) {
        self.state = if value == 0 { entropy() } else { mixed(value) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Golden vectors from certify/rng_oracle.py: the compatibility
    // contract shared with the reference implementation. A failure
    // here is a breaking change, not a test to update.

    #[test]
    fn the_raw_stream_matches_the_reference() {
        let mut session = Randomizer::new(Some(1137));
        let stream: Vec<u32> = (0..10).map(|_| session.word()).collect();

        assert_eq!(
            stream,
            [
                0xE1AB71B6, 0x7C233978, 0x7A8AAB3E, 0xD242E5C8, 0x518FF415, 0x4EAD71F3, 0xF2FF56FA,
                0x1C2347AA, 0xEE1185E1, 0x0B149C57
            ]
        );
    }

    #[test]
    fn folded_ranges_match_the_reference() {
        let mut session = Randomizer::new(Some(42));
        let rolls: Vec<u32> = (0..20).map(|_| session.below(100)).collect();

        assert_eq!(
            rolls,
            [
                8, 46, 84, 63, 36, 0, 45, 48, 21, 23, 63, 49, 89, 56, 40, 98, 68, 51, 71, 89
            ]
        );
    }

    #[test]
    fn reseeding_matches_the_reference() {
        let mut session = Randomizer::new(Some(1137));
        session.seed(5000);
        let rolls: Vec<u32> = (0..20).map(|_| session.below(6)).collect();

        assert_eq!(
            rolls,
            [1, 4, 1, 3, 3, 1, 3, 1, 2, 2, 4, 3, 3, 5, 5, 3, 1, 0, 4, 5]
        );
    }

    #[test]
    fn the_same_seed_reproduces_its_session() {
        let mut first = Randomizer::new(Some(7));
        let mut second = Randomizer::new(Some(7));

        for _ in 0..5 {
            assert_eq!(first.word(), second.word());
        }
    }
}
