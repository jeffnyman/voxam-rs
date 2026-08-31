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
| `glulx-machine-diff.sh` | the bare Glulx machine -- a byte-exact Quetzal save vector, then every checker `.ulx` booted with no Glk library and run to its quit or halt, step counts and halting errors agreeing whole |
| `glulxercise.sh` | the M3 gate itself: glulxercise run whole under a scripted Glk display, the verdict the story's own -- "All tests passed." |
| `aastory-diff.sh` | every .aastory fixture the reference keeps -- header claims, bibliography, extended characters, chunk census, and the whole dictionary decoded, agreeing line for line |
| `aawalk-diff.sh` | the M4 gate: the vendored gold transcripts -- the community fork's own engine at seed 1234 -- replayed under the port's plain voice, byte for byte: the opcode exercise twice over, Miss Gosling's Last Case whole, format 1.0, and the codepoints walk |
| `aaterminal-diff.sh` | the terminal face under both implementations with the same scripted streams -- Cloak of Darkness on a written walkthrough, the other fixtures on their own input scripts -- the sessions diffed whole |
| `page-diff.sh` | the GlkOte update builder under both implementations -- four drills through the Page, the stanzas diffed as the compact JSON the wire actually speaks, key order and escapes included |
| `zglkote-diff.sh` | the Z wire whole: every Z acceptance recording driven through both implementations' GlkOte serving loops by the same deterministic typist (`zglkote_drive.py`), the update transcripts diffed byte for byte -- the four Version 6 recordings riding the stage dialect |
| `gglkote-diff.sh` | the Glulx wire whole: the Glulx acceptance recordings through both GlkOte serving loops, the same typist, the transcripts diffed byte for byte |
| `aaglkote-diff.sh` | the Å-machine wire whole: the terminal sweep's four sessions through both GlkOte serving loops, the same typist on the plain input scripts, the transcripts diffed byte for byte |
| `png-diff.sh` | every PNG picture of every corpus Blorb, decoded and re-encoded under both implementations -- the Version 6 wire's own transform: chunk walking, inflation of streams other tools compressed, unfiltering, pixel extraction, and the hand-spelled deterministic deflate, the re-encoded bytes diffed whole (the pure-Python side takes minutes) |
| `gallery-diff.sh` | every corpus Blorb's gallery census -- release, the Reso scaling fractions, the APal and BPal palette chunks, and each picture's measured size beside its Elbow Room ratio -- without decoding a pixel |
| `sidecar-diff.sh` | the three wire sweeps run again with the `voxam` support token granted -- every update of every session carrying the sidecar block (location, score, turns, the delivered command, the discontinuity bit), the transcripts still byte-identical |
| `linked-diff.sh` | the desktop shell's transport: every Z acceptance recording driven through the port twice by the same typist -- once over real stdio, once over the in-process pipes the shell links the interpreter with -- and the transcripts diffed byte for byte, so the hand-rolled pipe inherits the Z wire sweep's certification |
| `glass-diff.sh` | the painted terminal's drills: real sessions in a Windows pseudo-console, the VT stream replayed onto a virtual screen and judged as a player would see it -- the status bar, the echoed commands, the cover note, the dirty-screen wipe, and Beyond Zork's palette under the amiga identity (Windows-only; builds its own small uv environment under target/ on first run) |
| `stage-diff.sh` | eleven drills through the §8.8 StageModel under both implementations -- wraps, scrolls, splits, margins, erases, line editing, the [MORE] budget, odd font metrics, the refusals -- grids, cursors, unit paints, and pauses diffed line for line |

Beside the sweeps live the **oracles** -- scripts that run the
genuine reference implementation to manufacture the golden vectors
the port's unit tests pin. They are the provenance of those test
constants, kept rerunnable: a vector generated is never a vector
guessed.

| Oracle | What it pins |
| --- | --- |
| `rng_oracle.py` | the xorshift32 compatibility contract: mixing outputs, raw stream states, and the pinned roll sequences in `rng.rs` |
| `zscii_oracle.py` | the §3 text battery: decode results across the version rules, encode bytes, error prose, and the surrogate fusing in `zscii.rs` |
| `floats_oracle.py` | the Glulx float semantics: encode/decode bits, the saturating conversions with banker's rounding, modulo pairs, pow's promised specials, and the jfeq closeness rules in `floats.rs` |
| `glulx_machine_reference.py` | the machine-era answers `glulx-machine-diff.sh` diffs: the reference's own save bytes and bare-run outcomes |

The deeper certifications -- seeded recordings replaying
identically, glulxercise, the Å-machine batteries -- ride the
acceptance and regtest machinery as milestones reach them; these
sweeps cover the reports those machineries do not.
