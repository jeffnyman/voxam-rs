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

The deeper certifications -- seeded recordings replaying
identically, glulxercise, the Å-machine batteries -- ride the
acceptance and regtest machinery as milestones reach them; these
sweeps cover the reports those machineries do not.
