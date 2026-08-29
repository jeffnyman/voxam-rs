# The Port

Voxam-rs is a Rust port of [Voxam](https://github.com/jeffnyman/voxam),
the Python implementation at `../voxam`. This document maps the
territory: what goes where, in what order, and what certifies each
step. The Python implementation remains the reference throughout —
every behavior question is answered by reading it, and every seeded
session it records is a test this port must pass.

## Why Rust (recorded for posterity)

The core would port equally well to Go. Rust won on the frontends:
the desktop shell is already Tauri, Tauri 2 carries the same shell
to iOS and Android, the wasm target puts the interpreter in the
browser page itself, and the deluxe ambitions (automapper,
note-taker) want the machine linked in-process with the shell.
Exhaustive `match` over opcodes is also a fair trade for Python's
100%-branch gate: the compiler enforces what discipline used to.

## Workspace shape

```
voxam-rs/
├── Cargo.toml            workspace
├── crates/
│   ├── voxam-core/       the machines and formats; no I/O opinions
│   └── voxam/            the CLI binary: faces, flags, the wire
├── certify/              the parity sweeps and golden-vector oracles
└── entharion/            (submodule) specs and story files, as in voxam
```

The recordings stay in the reference checkout: the replay sweep
reads `../voxam/acceptance/` (or `VOXAM_REFERENCE`), because the
recordings are the contract *between* the implementations and the
reference keeps custody.

Planned additions, in the order they're earned:

- `crates/voxam-glass/` — the painted terminal (ratatui), if it
  outgrows the CLI crate.
- `desktop/` — the Tauri shell, carried over from voxam and taught
  to link `voxam-core` in-process instead of spawning a subprocess;
  later, the Tauri 2 mobile targets.
- a `wasm` target of `voxam-core` married to glkote.js — the
  browser face with no server.

`voxam-core` stays free of display and filesystem opinions so the
same crate serves the CLI, the shell, and wasm. The seam between
core and face is the wire: GlkOte-shaped stanzas, extended with a
`voxam:` sidecar block (location object, printed name, score,
turns, the moving command) that the deluxe features consume.

## Module map

Python (`voxam/src/voxam/`) → Rust, mechanical unless noted.

| Python | Rust | Notes |
| --- | --- | --- |
| `zmachine/` (11.6k lines) | `voxam-core::zmachine` | Mechanical. Masking (`& 0xFFFF`) becomes native `u16`/`u32` wrapping. Dispatch dicts become `match`. |
| `zmachine/rng.py`, `glulx/rng.py` | `voxam-core::zmachine::rng`, … | **Port bit-exact first.** The xorshift32 is what makes every recording replay under the port. |
| `glulx/` (15k lines) | `voxam-core::glulx` | The Glk layer (`glk/`, ~7k) is the subtlest: object registries, dispatch. Floats via `f32::from_bits`. |
| `aamachine/` (5.5k lines) | `voxam-core::aamachine` | Mechanical; certified byte-identical against the reference batteries. |
| `iff.py`, `blorb.py`, `babel.py`, `infocom.py` | `voxam-core::{iff, blorb, babel, …}` | Byte work; IFIDs remain the persistence key for deluxe features. |
| `zmachine/quetzal.py`, `aamachine/saves.py` | with their machines | Interchange formats; saves must round-trip with other interpreters. |
| `png.py`, `aiff.py`, `wav.py`, `sixel.py`, `font3.py` | `voxam-core` or drop | `png` crate or a straight port; the hand-rolled decoders exist for zero-dep purity, which crates.io relaxes. Decide per file. |
| `acceptance.py`, `regtest.py`, `probe.py` | `voxam-core::harness` (or a dev-only crate) | The certification machinery. Ports early — it is how everything else is judged. |
| `listing.py`, `glance.py`, `decompose.py`, `scribe.py` | `voxam` (CLI) | Inspection tools; `--listing` doubles as a decoder test. |
| `cli.py` (2.3k lines) | `voxam` (CLI) | Flag surface preserved; `clap` or hand-rolled. |
| `glass.py`, `painter.py`, `screen.py`, `frontend.py` | ratatui face | **Rewrite in kind, not a port.** blessed's cell painting maps to ratatui's buffer model, not line-for-line. |
| `stage.py`, `gallery.py`, `speaker.py` | deferred | The pygame window. The webview face already renders V6 art, sound, and mouse; decide later whether a native window still earns its keep. |
| `web.py`, `glkote.py`, `*/glkote.py` | `voxam` (CLI) | `--web` on a stdlib-adjacent server (`tiny_http` or hand-rolled), `--glkote` over stdio. The wire types live in `voxam-core`, on a hand-rolled JSON that keeps Python's exact spelling -- insertion-ordered keys, compact separators, ensure_ascii -- because the stanza sweeps diff byte for byte, which serde_json's dialect would part. |
| `filmstrip.py` | with the faces | Screenshot regression; wants the faces standing first. |
| `tests/` (42k lines) | selectively | Port unit tests for the decoders (ZSCII, operands, Quetzal, IFF); the recordings certify the rest end-to-end. |

## Milestones, each with its gate

1. **Plain-stream Z-machine.** ✅ *(2026-08-29)* Memory, ZSCII,
   objects, dictionary, opcodes, frames, the RNG — and past its
   own gate: the whole Version 1–8 opcode era, the §8.8 window
   ledger included. *Gate met:* every Z-code recording in the
   corpus — 42 of the 44, the other two being Glulx — replays
   byte-identically on the plain stream, refusal warnings and
   resource banners included.
2. **Persistence and packaging.** ✅ *(2026-08-29)* Quetzal
   save/restore, Blorb unwrapping and census, `--header` on
   packaged stories. *Gate met:* identical sessions write
   byte-identical saves under both implementations, restores
   cross both ways, and the saves travel to dfrotz and back.
   (Babel identities remain, folded into a later milestone.)
3. **Glulx, plain.** ✅ *(2026-08-29)* The VM, strings, floats,
   accel, Glk over the plain stream. *Gate met:* glulxercise says
   "All tests passed." — 596k instructions, 70 sections, zero
   failures — and both Glulx acceptance recordings replay
   byte-identically, which closes the ledger at 44 of 44.
4. **Å-machine.** ✅ *(2026-08-29)* The story file, the text
   apparatus, the engine with its Prolog heart, the AASV
   savefile, and the plain and terminal voices. *Gate met:* the
   reference batteries replay byte-identical -- the vendored gold
   transcripts (the community fork's own engine at seed 1234)
   land whole under the plain voice, Miss Gosling's Last Case
   included, and the terminal sessions diff clean against the
   reference implementation stream for stream.
5. **The wire.** `--glkote` stanzas over stdio, `--web`, the
   `voxam:` sidecar block. *Gate:* the existing Tauri shell plays
   stories against the Rust binary, unmodified.
6. **The painted terminal.** ratatui glass for all three machines.
   *Gate:* filmstrip comparisons where determinism allows; eyes
   otherwise.
7. **The deluxe shell.** Interpreter linked in-process, automapper
   and notes as panes, keyed by IFID. Then Tauri mobile, then wasm.

Milestones 1–4 are mechanical translation with a safety net.
Milestone 6 is the largest rewrite. Milestone 7 is new work the
Python implementation never had.

## What the sidecar carries

The `voxam:` block rides each GlkOte update stanza, and it is the
whole of the interpreter's contribution to the deluxe features:
a dumb factual feed, with every ounce of graph, layout, and
rendering intelligence living in the face. Surveying prior art
confirmed that this small tuple is sufficient for a working
automapper, and taught one addition. The schema, when milestone 5
designs it:

- **The location**: object id and printed name. Per-machine
  honesty rules here — the Z-machine's global 0 is guaranteed
  only through v3 and conventional after, and Glulx has no fixed
  location global at all — so the fields are optional and honest
  rather than uniformly pretended, and the mapper degrades
  gracefully when location is unknowable.
- **The moving command, as delivered.** The wire layer knows the
  line it handed the machine — scripted input included — which
  beats any face-side memory of what was typed.
- **Score and turns**, as already planned.
- **A discontinuity flag**: this update does not follow causally
  from the last command. The machines all know when an undo,
  restore, restart, or death intervened; one honest bit here
  spares every face the transcript-grepping heuristics earlier
  automappers needed, and the mapper never draws a phantom edge
  across time travel.

And a boundary: no direction-parsing or graph state in
`voxam-core`. Reading a typed command for its compass word is an
English-only, typed-input-only heuristic — fine as a face's
choice, poisonous as a core assumption. Persisted map layouts key
by IFID, per milestone 7's standing plan, which is part of why
the Babel identities work eventually matters.

## Departures the port has recorded

Each is a documented translation, never a behaviour change; the
sweeps prove the outputs identical across all of them.

- **Suspension is a return value, not an exception.** Python
  raises through the step loop; here `step` returns
  `Step::Suspended` and `run` hands back `RunState::Waiting`,
  with the same parked-tail contract.
- **Text flows as 16-bit units.** Rust strings cannot hold lone
  UTF-16 surrogates, so decoding produces units and the fusing
  happens at String boundaries — the reference's exact
  fuse-after-decode composition.
- **State views take their stores as arguments.** ObjectTable,
  Variables, and the header's declare functions are geometry over
  `&Memory`/`&mut Memory` rather than owners, for the borrow
  checker's sake.
- **Opcode tables are matches with version guards** rather than
  dicts of span tuples; a §14 fork reads as two guarded arms.
- **Glk objects live in id-keyed arenas.** The reference's object
  graph — a window holding its parent pair, a stream its window —
  becomes maps keyed by internal id, with references stored as ids
  and the tree walks (`rearrange`, `subtree`) taking the map as an
  argument. Subclass hierarchies become `WindowKind`/`StreamKind`
  enums. The 32-bit ids Glulx sees stay the bridge's separate,
  lazily-minted sequence, exactly as in the reference, so
  transcripts diff identically.
- **Glk buffers are VM coordinates, not live views.** The
  reference's Buffer protocol (a list, or a MemArray over VM
  memory) becomes `MemArray` coordinates with `&Memory`/`&mut
  Memory` passed to every operation that touches one — the
  state-view departure applied to Glk. Retained arrays survive
  `setmemsize` for the same reason the reference's lazy views do.
- **One dependency so far**: `getrandom`, standing in for
  `os.urandom` — the per-file relaxation of the hand-rolled
  purity rule, as anticipated below.

## Standing decisions

- **The RNG is sacred.** xorshift32 with the exact mixing
  constants, before anything that consumes it. A recording that
  replays under Python must replay here, forever.
- **entharion sits at the repo root**, as in voxam, so carried-over
  acceptance scripts keep their relative `GAME` paths.
- **Spec citations carry over.** The `§` references survive the
  translation; they are the project's conscience.
- **The Python implementation is not frozen** — it remains the
  reference and the place behavior questions get settled. The port
  chases it; recordings are the contract between them.
