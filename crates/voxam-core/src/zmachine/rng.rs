//! The random number generator and its two states (§2.4).
//!
//! The generator is "random" at game start and after restarts, and
//! becomes "predictable" when seeded. Predictable mode follows the
//! Standard's suggested algorithm (§2 remarks): a seed under 1000
//! cycles the rising sequence 1 to S -- which visits every possible
//! value, for testing -- while larger seeds run a seeded generator,
//! for replaying whole scripts.
//!
//! The stream itself is a xorshift32 owned by this module, ported
//! bit-exact from the Python implementation, so that a seed
//! produces the same session forever: recorded playthroughs must
//! never be invalidated by an interpreter upgrade -- or by a port.

/// Seeds below this cycle the rising sequence; from here up they
/// seed the conventional generator (§2 remarks).
const SEQUENCE_SEED_LIMIT: u32 = 1000;

// The xorshift32 triple and the mixing constants used to spread a
// seed across the 32-bit state.
const XORSHIFT_TRIPLE: (u32, u32, u32) = (13, 17, 5);
const MIX_INCREMENT: u32 = 0x9E37_79B9;
const MIX_MULTIPLIER_1: u32 = 0x85EB_CA6B;
const MIX_MULTIPLIER_2: u32 = 0xC2B2_AE35;

/// Spread a seed over the state space, never yielding zero.
///
/// A xorshift state of zero is a fixed point, and small seeds used
/// raw would start the stream in a correlated corner; one round of
/// integer mixing avoids both.
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

/// The two-state generator behind the random opcode (§2.4).
pub struct Randomizer {
    state: u32,
    /// In the predictable state, the rising sequence's limit S and
    /// the position within it; `None` is the random state.
    sequence: Option<(u32, u32)>,
}

impl Randomizer {
    /// Start in the random state, as at game start (§2.4).
    ///
    /// A session seed makes playthroughs reproducible. It seeds the
    /// stream directly without entering the predictable state: the
    /// game still sees ordinary dice, just the same dice every
    /// session. `None` means true entropy.
    pub fn new(seed: Option<u32>) -> Self {
        Self {
            state: match seed {
                Some(seed) => mixed(seed),
                None => entropy(),
            },
            sequence: None,
        }
    }

    /// Produce a value from 1 to `limit` (§2.4.1).
    ///
    /// In rising-sequence mode the next entry is folded into range;
    /// when the sequence limit is within the requested range, the
    /// results are simply 1, 2, ..., S, repeating. `limit` is at
    /// least 1, as the opcode layer guarantees.
    pub fn roll(&mut self, limit: u32) -> u32 {
        if let Some((sequence_limit, at)) = &mut self.sequence {
            *at = *at % *sequence_limit + 1;

            return (*at - 1) % limit + 1;
        }

        // Folding by modulo skews the distribution by under one part
        // in 131072 for the largest legal range (§2.4.1) -- far below
        // anything a game's dice could notice.
        self.next() % limit + 1
    }

    /// Switch to the predictable state with a seed (§2.4.2).
    ///
    /// `value` is at least 1: the opcode passes the magnitude of a
    /// negative operand, and zero goes to [`Self::randomize`].
    pub fn seed(&mut self, value: u32) {
        if value < SEQUENCE_SEED_LIMIT {
            self.sequence = Some((value, 0));
        } else {
            self.sequence = None;
            self.state = mixed(value);
        }
    }

    /// Return to the random state, seeded as randomly as possible.
    ///
    /// The random opcode with a range of 0 asks for exactly this
    /// (§15 random).
    pub fn randomize(&mut self) {
        self.sequence = None;
        self.state = entropy();
    }

    /// Advance the xorshift32 stream one step.
    fn next(&mut self) -> u32 {
        let mut state = self.state;
        state ^= state << XORSHIFT_TRIPLE.0;
        state ^= state >> XORSHIFT_TRIPLE.1;
        state ^= state << XORSHIFT_TRIPLE.2;
        self.state = state;

        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Golden vectors below were generated from the reference Python
    // implementation (voxam.zmachine.rng); they are the port's proof
    // of bit-exactness, and the same compatibility contract the
    // Python suite pins: these values may never change without
    // invalidating every acceptance fixture ever recorded.

    #[test]
    fn the_mixing_function_matches_the_reference() {
        let vectors: [(u32, u32); 10] = [
            (0, 0x92CA2F0E),
            (1, 0x96A0F96B),
            (3, 0xED81DED3),
            (42, 0x3805EA2C),
            (999, 0x52F4480F),
            (1000, 0x276D0A38),
            (1137, 0x8E3DC42A),
            (5000, 0x5913184C),
            (2147483647, 0x32A452CD),
            (4294967295, 0x36DEB503),
        ];
        for (input, expected) in vectors {
            assert_eq!(mixed(input), expected, "mixed({input})");
        }
    }

    #[test]
    fn the_raw_stream_matches_the_reference() {
        let mut session = Randomizer::new(Some(1137));
        let stream: Vec<u32> = (0..10).map(|_| session.next()).collect();

        assert_eq!(
            stream,
            [
                0xE1AB71B6, 0x7C233978, 0x7A8AAB3E, 0xD242E5C8, 0x518FF415, 0x4EAD71F3, 0xF2FF56FA,
                0x1C2347AA, 0xEE1185E1, 0x0B149C57
            ]
        );
    }

    #[test]
    fn the_stream_is_pinned_forever() {
        let mut session = Randomizer::new(Some(1137));
        let rolls: Vec<u32> = (0..20).map(|_| session.roll(100)).collect();
        assert_eq!(
            rolls,
            [
                67, 57, 59, 61, 30, 48, 19, 55, 94, 20, 73, 2, 2, 45, 18, 58, 60, 74, 78, 90
            ]
        );

        let mut session = Randomizer::new(Some(42));
        let rolls: Vec<u32> = (0..20).map(|_| session.roll(100)).collect();
        assert_eq!(
            rolls,
            [
                9, 47, 85, 64, 37, 1, 46, 49, 22, 24, 64, 50, 90, 57, 41, 99, 69, 52, 72, 90
            ]
        );

        let mut dice = Randomizer::new(Some(42));
        let rolls: Vec<u32> = (0..20).map(|_| dice.roll(6)).collect();
        assert_eq!(
            rolls,
            [1, 5, 3, 6, 1, 3, 6, 1, 4, 2, 4, 2, 6, 5, 3, 1, 1, 4, 2, 6]
        );

        let mut opcode_seeded = Randomizer::new(None);
        opcode_seeded.seed(5000);
        let rolls: Vec<u32> = (0..20).map(|_| opcode_seeded.roll(100)).collect();
        assert_eq!(
            rolls,
            [
                18, 67, 58, 80, 32, 4, 48, 20, 11, 3, 19, 24, 48, 6, 30, 76, 20, 53, 9, 88
            ]
        );
    }

    // A seed under 1000 produces the rising sequence 1 to S,
    // repeating -- the Standard's suggested testing mode (§2 remarks).
    #[test]
    fn low_seeds_cycle_the_rising_sequence() {
        let mut rng = Randomizer::new(None);
        rng.seed(3);

        let rolls: Vec<u32> = (0..5).map(|_| rng.roll(10)).collect();
        assert_eq!(rolls, [1, 2, 3, 1, 2]);
    }

    // When the sequence outgrows the requested range, entries fold
    // back into it.
    #[test]
    fn the_rising_sequence_folds_into_the_range() {
        let mut rng = Randomizer::new(None);
        rng.seed(10);

        let rolls: Vec<u32> = (0..10).map(|_| rng.roll(3)).collect();
        assert_eq!(rolls, [1, 2, 3, 1, 2, 3, 1, 2, 3, 1]);
    }

    // Seeds of 1000 and up run the conventional generator: the same
    // seed must reproduce the same sequence (§2.4.2).
    #[test]
    fn high_seeds_reproduce_their_sequences() {
        let mut first = Randomizer::new(None);
        let mut second = Randomizer::new(None);
        first.seed(5000);
        second.seed(5000);

        for _ in 0..5 {
            assert_eq!(first.roll(100), second.roll(100));
        }
    }

    // A session seed leaves the §2.4 state machine alone: the game's
    // own opcode-level seeding still wins.
    #[test]
    fn a_session_seed_does_not_enter_the_predictable_state() {
        let mut session = Randomizer::new(Some(1137));
        session.seed(3);

        let rolls: Vec<u32> = (0..4).map(|_| session.roll(10)).collect();
        assert_eq!(rolls, [1, 2, 3, 1]);
    }

    #[test]
    fn random_mode_stays_in_range() {
        let mut rng = Randomizer::new(None);

        assert_eq!(rng.roll(1), 1);
        for _ in 0..25 {
            let roll = rng.roll(6);
            assert!((1..=6).contains(&roll));
        }
    }

    #[test]
    fn randomize_leaves_the_predictable_state() {
        let mut rng = Randomizer::new(None);
        rng.seed(3);
        rng.randomize();

        for _ in 0..10 {
            let roll = rng.roll(4);
            assert!((1..=4).contains(&roll));
        }
    }
}
