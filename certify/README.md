# Certify

The sweeps that judge this port against the Python reference
implementation. Each script renders the same request under both
implementations and diffs the results; the exit code speaks
RegTest's contract -- 0 for identical, 1 for parted, 2 for a sweep
that can't run -- so continuous integration can gate on it.

The reference checkout is found at `../voxam` beside this
repository, or wherever `VOXAM_REFERENCE` points. Running the
reference needs [uv](https://docs.astral.sh/uv/), exactly as
developing it does.

| Sweep | What it certifies |
| --- | --- |
| `header-diff.sh` | `--header` reports over every Z-code story in `entharion/` |
| `abbreviations-diff.sh` | every abbreviation every Z-code story defines, decoded to identical text units -- the whole text decoder over real data |
| `dictionary-diff.sh` | every dictionary entry of every Z-code story, decoded, with sampled round-trip lookups -- the decoder, the encoder, and the §13 table walk agreeing |
| `objects-diff.sh` | every object of every Z-code story -- relations, attributes, short names, and property lists walked linearly with a 64-entry cap against corrupt tables |
| `replay-diff.sh` | every acceptance recording, replayed under both implementations -- IDENTICAL byte for byte, FRONTIER when the port halts at a named unbuilt feature with its output a byte prefix of the reference's, PARTED only on real divergence |

Beside the sweeps live the **oracles** -- scripts that run the
genuine reference implementation to manufacture the golden vectors
the port's unit tests pin. They are the provenance of those test
constants, kept rerunnable: a vector generated is never a vector
guessed.

| Oracle | What it pins |
| --- | --- |
| `rng_oracle.py` | the xorshift32 compatibility contract: mixing outputs, raw stream states, and the pinned roll sequences in `rng.rs` |
| `zscii_oracle.py` | the §3 text battery: decode results across the version rules, encode bytes, error prose, and the surrogate fusing in `zscii.rs` |

The deeper certifications -- seeded recordings replaying
identically, glulxercise, the Å-machine batteries -- ride the
acceptance and regtest machinery as milestones reach them; these
sweeps cover the reports those machineries do not.
